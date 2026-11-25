//! Learning our own public address: a STUN binding (RFC 5389) asks "what address do you see me
//! at?". Two kinds of reflector answer it, and this module is both sides of that question:
//! - a public STUN server, asked with a plain binding request ([`Style::Plain`]): the fallback when
//!   the router can't tell us ([`super::portmap`]) and no peer can. The default list,
//!   [`DEFAULT_SERVERS`], is Cloudflare's (documented: `stun.cloudflare.com` on 3478/udp, and
//!   `turn.cloudflare.com` also answers binding requests); a setting can replace it;
//! - the peer: once one side is reachable it answers the other side's binding requests ([`answer`]),
//!   so only one side ever needs a public server. A peer answers only [`Style::Padded`] requests.
//!
//! Two reflectors at different destinations, asked from one socket, tell how the NAT maps that
//! socket ([`Mapping`]): one public address for both means a peer can reach the reflected address
//! once the hole is punched; a different one per destination ("symmetric") means only a router
//! mapping helps. How the NAT *filters* (whether a port-restricted firewall drops a peer we haven't
//! sent to) can't be learned from public servers, which don't answer from another address
//! (RFC 5780); the punch decides that.
//!
//! No amplification: [`answer`] never answers a request smaller than its answer, so a peer can't be
//! turned into a flood of more bytes than were sent to it. Padded requests are [`REQUEST_LEN`] bytes
//! (PADDING, RFC 5780); a bare request gets no answer from a peer.
//!
//! The client side is sans-IO ([`Query`]: bytes and the time in, bytes out), so it shares a socket
//! with the hole punch: the reflected address must be the mapping of the very socket that punches.
//! [`ask`] runs a query on a socket of its own thread for callers that have nothing else to do.

use super::stun::{self, attr, Class, BINDING};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// The public STUN servers asked by default: two destinations, so the answers tell the mapping.
/// Cloudflare documents both as answering binding requests on 3478/udp. A user setting replaces
/// the list; nothing here is asked unless the router and the peer couldn't tell us our address.
pub const DEFAULT_SERVERS: [&str; 2] = ["stun.cloudflare.com:3478", "turn.cloudflare.com:3478"];
/// The size of a padded request: at least any answer (52 bytes for an IPv6 address).
pub const REQUEST_LEN: usize = 64;
/// The first retransmission of an unanswered request, in ms; each next one waits twice as long, up
/// to [`MAX_RETRY_MS`], and they go on until the caller stops asking.
pub const FIRST_RETRY_MS: u64 = 250;
pub const MAX_RETRY_MS: u64 = 4000;

/// How a binding request is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// RFC 5389 as every public STUN server takes it: the header and FINGERPRINT (28 bytes).
    Plain,
    /// Padded to [`REQUEST_LEN`] for a peer that reflects ([`answer`] takes nothing smaller).
    Padded,
}

/// The reflector's answer to `request` from `from`: a binding success carrying `from` as
/// XOR-MAPPED-ADDRESS, then FINGERPRINT. `None` for anything else: not a binding request, an
/// attribute we must understand but don't (RFC 5389 asks for a 420 error; we stay silent), a bad
/// fingerprint, or a request smaller than the answer. An IPv4 asker that a dual-stack socket
/// reports as `::ffff:a.b.c.d` is answered with its IPv4 address.
pub fn answer(request: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    let from = SocketAddr::new(from.ip().to_canonical(), from.port());
    let message = stun::parse(request).ok()?;
    if message.class != Class::Request || message.method != BINDING {
        return None;
    }
    if message.attributes.iter().any(|(kind, _)| *kind < 0x8000 && *kind != attr::PADDING) {
        return None;
    }
    if message.has(attr::FINGERPRINT) && !stun::check_fingerprint(request) {
        return None;
    }
    let (value, length) = stun::xor_mapped_value(&message.transaction, from);
    let mut packet = stun::new_binding(&message.transaction, Class::Success, 20 + 4 + length + 8);
    stun::append_attribute(&mut packet, attr::XOR_MAPPED_ADDRESS, &value[..length]);
    stun::append_fingerprint(&mut packet);
    (packet.len() <= request.len()).then_some(packet)
}

/// A binding request in `style`, ending with FINGERPRINT.
pub fn request(transaction: &[u8; 12], style: Style) -> Vec<u8> {
    match style {
        Style::Plain => {
            let mut packet = stun::new_binding(transaction, Class::Request, 28);
            stun::append_fingerprint(&mut packet);
            packet
        }
        Style::Padded => {
            let mut packet = stun::new_binding(transaction, Class::Request, REQUEST_LEN);
            stun::append_attribute(&mut packet, attr::PADDING, &[0; REQUEST_LEN - 20 - 4 - 8]);
            stun::append_fingerprint(&mut packet);
            debug_assert_eq!(packet.len(), REQUEST_LEN);
            packet
        }
    }
}

/// The address an answer to our request `transaction` reflects, when `packet` is that answer.
pub fn read_answer(packet: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    let message = stun::parse(packet).ok()?;
    if message.class != Class::Success || message.method != BINDING || &message.transaction != transaction {
        return None;
    }
    if message.has(attr::FINGERPRINT) && !stun::check_fingerprint(packet) {
        return None;
    }
    message.xor_mapped_address()
}

/// How a NAT maps one socket, from the answers of reflectors at different destinations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    /// Fewer than two answers: not known.
    Unknown,
    /// One public address for every destination ("cone"): punchable from the reflected address.
    EndpointIndependent,
    /// A public address per destination ("symmetric"): the reflected address is no use to a peer.
    DestinationDependent,
}

/// What a [`Query`] learned: per reflector address asked, the address it saw us at (if it answered).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reflection {
    pub answers: Vec<(SocketAddr, Option<SocketAddr>)>,
}

impl Reflection {
    /// Our public address as the first answering reflector saw it.
    pub fn mapped(&self) -> Option<SocketAddr> {
        self.answers.iter().find_map(|(_, mapped)| *mapped)
    }

    pub fn mapping(&self) -> Mapping {
        let mut seen = self.answers.iter().filter_map(|(_, mapped)| *mapped);
        let Some(first) = seen.next() else { return Mapping::Unknown };
        let mut answers = 1;
        for other in seen {
            answers += 1;
            if other != first {
                return Mapping::DestinationDependent;
            }
        }
        if answers >= 2 {
            Mapping::EndpointIndependent
        } else {
            Mapping::Unknown
        }
    }
}

struct Ask {
    server: SocketAddr,
    transaction: [u8; 12],
    next_at: u64,
    interval: u64,
    mapped: Option<SocketAddr>,
}

/// Asking reflectors from one socket, sans-IO. Time is ms from any origin the caller keeps.
pub struct Query {
    asks: Vec<Ask>,
    style: Style,
}

impl Query {
    /// A query of every address in `servers` with requests in `style`; `random` fills each
    /// transaction id (the OS's random source in use, [`crate::tls::os_random`]; a seeded one in
    /// tests).
    pub fn new(servers: &[SocketAddr], style: Style, random: &mut dyn FnMut(&mut [u8]), now: u64) -> Query {
        let asks = servers
            .iter()
            .map(|server| {
                let mut transaction = [0; 12];
                random(&mut transaction);
                Ask { server: *server, transaction, next_at: now, interval: FIRST_RETRY_MS, mapped: None }
            })
            .collect();
        Query { asks, style }
    }

    /// The requests due at `now` (first sends and retransmissions), as (to, bytes).
    pub fn poll(&mut self, now: u64) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        for ask in self.asks.iter_mut().filter(|ask| ask.mapped.is_none() && ask.next_at <= now) {
            out.push((ask.server, request(&ask.transaction, self.style)));
            ask.next_at = now + ask.interval;
            ask.interval = (ask.interval * 2).min(MAX_RETRY_MS);
        }
        out
    }

    /// Takes a datagram; `true` when it was the answer to one of our requests (from the address
    /// asked, with its transaction).
    pub fn handle(&mut self, from: SocketAddr, packet: &[u8]) -> bool {
        if !stun::is_stun(packet) {
            return false;
        }
        for ask in self.asks.iter_mut().filter(|ask| ask.server == from && ask.mapped.is_none()) {
            if let Some(mapped) = read_answer(packet, &ask.transaction) {
                ask.mapped = Some(mapped);
                return true;
            }
        }
        false
    }

    /// When [`Query::poll`] next has something to send; `None` once every reflector answered.
    pub fn next_wake(&self) -> Option<u64> {
        self.asks.iter().filter(|ask| ask.mapped.is_none()).map(|ask| ask.next_at).min()
    }

    pub fn done(&self) -> bool {
        self.next_wake().is_none()
    }

    pub fn result(&self) -> Reflection {
        Reflection { answers: self.asks.iter().map(|ask| (ask.server, ask.mapped)).collect() }
    }
}

/// The IPv4 addresses of `servers` (`host:port`, as in [`DEFAULT_SERVERS`] or the user's setting),
/// one per server, in order; a name that doesn't resolve is left out. Resolving blocks: call it on a
/// worker thread. An error only when none resolved.
pub fn resolve(servers: &[&str]) -> io::Result<Vec<SocketAddr>> {
    use std::net::ToSocketAddrs;
    let mut found = Vec::new();
    let mut last_error = None;
    for server in servers {
        match server.to_socket_addrs() {
            Ok(addrs) => match addrs.into_iter().find(|addr| addr.is_ipv4()) {
                Some(addr) => found.push(addr),
                None => last_error = Some(io::Error::new(io::ErrorKind::NotFound, format!("{server} has no IPv4 address"))),
            },
            Err(e) => last_error = Some(e),
        }
    }
    match (found.is_empty(), last_error) {
        (true, Some(e)) => Err(e),
        _ => Ok(found),
    }
}

/// Runs a [`Query`] of `servers` on `socket` for at most `wait` (blocking: a worker thread's call).
/// Datagrams that are not answers are dropped, so use it before the socket carries anything else.
/// What came back by then is the result; no answer at all is an `Ok` with every answer `None`.
pub fn ask(socket: &UdpSocket, servers: &[SocketAddr], style: Style, wait: Duration) -> io::Result<Reflection> {
    let start = std::time::Instant::now();
    let elapsed = || start.elapsed().as_millis() as u64;
    let mut random_error = None;
    let mut query = Query::new(
        servers,
        style,
        &mut |bytes| {
            if let Err(e) = crate::tls::os_random(bytes) {
                random_error = Some(e);
            }
        },
        0,
    );
    if let Some(e) = random_error {
        return Err(e);
    }
    let end = wait.as_millis() as u64;
    let mut buf = [0u8; 1500];
    while !query.done() && elapsed() < end {
        for (to, bytes) in query.poll(elapsed()) {
            crate::lan::check("p2p reflect", to)?;
            socket.send_to(&bytes, to)?;
        }
        let until = query.next_wake().unwrap_or(end).min(end);
        let now = elapsed();
        if until <= now {
            continue;
        }
        socket.set_read_timeout(Some(Duration::from_millis(until - now)))?;
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                query.handle(from, &buf[..len]);
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(query.result())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter(seed: u8) -> impl FnMut(&mut [u8]) {
        let mut next = seed;
        move |bytes: &mut [u8]| {
            for byte in bytes {
                next = next.wrapping_mul(31).wrapping_add(7);
                *byte = next;
            }
        }
    }

    #[test]
    fn answers_only_padded_binding_requests() {
        let transaction = [7u8; 12];
        let ours = request(&transaction, Style::Padded);
        assert_eq!(ours.len(), REQUEST_LEN, "request size");
        assert!(stun::check_fingerprint(&ours), "request fingerprint");
        let plain = request(&transaction, Style::Plain);
        assert_eq!(plain.len(), 28, "a plain RFC 5389 request: header and FINGERPRINT");
        assert!(stun::check_fingerprint(&plain), "plain request fingerprint");
        assert_eq!(stun::parse(&plain).unwrap().attributes.len(), 1, "nothing but FINGERPRINT for a public server");
        assert_eq!(answer(&plain, "192.0.2.7:40112".parse().unwrap()), None, "a peer never answers a plain request: it would amplify");
        for from in ["192.0.2.7:40112", "[2001:db8::7]:51820"] {
            let from: SocketAddr = from.parse().unwrap();
            let reply = answer(&ours, from).expect("a padded binding request is answered");
            assert!(reply.len() <= ours.len(), "no amplification for {from}: {} > {}", reply.len(), ours.len());
            assert!(stun::check_fingerprint(&reply), "answer fingerprint for {from}");
            assert_eq!(read_answer(&reply, &transaction), Some(from), "the answer reflects {from}");
            assert_eq!(read_answer(&reply, &[8u8; 12]), None, "another transaction's answer is not ours");
            assert_eq!(answer(&reply, from), None, "an answer is not answered");
        }
        let mapped_v4: SocketAddr = "[::ffff:192.0.2.7]:40112".parse().unwrap();
        let reply = answer(&ours, mapped_v4).expect("a dual-stack socket's IPv4 asker is answered");
        assert_eq!(read_answer(&reply, &transaction), Some("192.0.2.7:40112".parse().unwrap()), "answered with the IPv4 address");
        let from: SocketAddr = "203.0.113.9:1".parse().unwrap();
        let bare = stun::new_binding(&transaction, Class::Request, 20);
        assert_eq!(answer(&bare, from), None, "a bare 20-byte request would amplify: no answer");
        let mut with_username = stun::new_binding(&transaction, Class::Request, 100);
        stun::append_attribute(&mut with_username, attr::USERNAME, b"someone:else");
        stun::append_attribute(&mut with_username, attr::PADDING, &[0; 40]);
        assert_eq!(answer(&with_username, from), None, "a comprehension-required attribute we don't serve: no answer");
        let mut corrupt = ours.clone();
        corrupt[30] ^= 1;
        assert_eq!(answer(&corrupt, from), None, "a bad fingerprint: no answer");
        for end in 0..ours.len() {
            assert_eq!(answer(&ours[..end], from), None, "truncated at {end}: no answer");
        }
    }

    #[test]
    fn query_retransmits_and_classifies_the_mapping() {
        let servers: Vec<SocketAddr> = vec!["203.0.113.9:3478".parse().unwrap(), "198.51.100.9:3478".parse().unwrap()];
        let mut plain = Query::new(&servers, Style::Plain, &mut counter(1), 0);
        assert!(plain.poll(0).iter().all(|(_, bytes)| bytes.len() == 28), "a plain query sends plain requests");
        let mut query = Query::new(&servers, Style::Padded, &mut counter(1), 1000);
        let first = query.poll(1000);
        assert_eq!(first.iter().map(|(to, _)| *to).collect::<Vec<_>>(), servers, "both asked at once");
        assert!(query.poll(1000 + FIRST_RETRY_MS - 1).is_empty(), "nothing before the first retry");
        assert_eq!(query.poll(1000 + FIRST_RETRY_MS).len(), 2, "both retried");
        assert_eq!(query.next_wake(), Some(1000 + 3 * FIRST_RETRY_MS), "the retry interval doubles");
        let mut t = 1000 + 3 * FIRST_RETRY_MS;
        for _ in 0..10 {
            query.poll(t);
            t = query.next_wake().unwrap();
        }
        let mut late = Query::new(&servers, Style::Padded, &mut counter(1), 0);
        late.poll(0);
        let gaps: Vec<u64> = (0..8).map(|_| {
            let at = late.next_wake().unwrap();
            late.poll(at);
            at
        }).collect();
        assert!(gaps.windows(2).all(|w| w[1] - w[0] <= MAX_RETRY_MS), "retries never wait longer than the cap: {gaps:?}");

        let me: SocketAddr = "198.51.100.23:40112".parse().unwrap();
        let reply_for = |query_bytes: &[u8], seen: SocketAddr| answer(query_bytes, seen).unwrap();
        let sent = query.poll(u64::MAX / 2);
        let (to_a, bytes_a) = &sent[0];
        let (to_b, bytes_b) = &sent[1];
        assert!(!query.handle("203.0.113.10:3478".parse().unwrap(), &reply_for(bytes_a, me)), "an answer from an address not asked is dropped");
        assert!(!query.handle(*to_a, &reply_for(bytes_b, me)), "another request's answer is dropped");
        assert!(!query.handle(*to_a, b"not stun at all, just some bytes"), "non-STUN bytes are not ours");
        assert!(query.handle(*to_a, &reply_for(bytes_a, me)), "the answer is taken");
        assert_eq!(query.result().mapping(), Mapping::Unknown, "one answer: mapping unknown");
        assert_eq!(query.result().mapped(), Some(me), "mapped after one answer");
        assert!(query.poll(u64::MAX / 2 + MAX_RETRY_MS).iter().all(|(to, _)| to == to_b), "an answered reflector is not asked again");

        let mut cone = Query::new(&servers, Style::Padded, &mut counter(2), 0);
        let mut symmetric = Query::new(&servers, Style::Padded, &mut counter(3), 0);
        let (cone_sent, symmetric_sent) = (cone.poll(0), symmetric.poll(0));
        for (i, (to, bytes)) in cone_sent.iter().enumerate() {
            assert!(cone.handle(*to, &reply_for(bytes, me)), "cone answer {i}");
        }
        for (i, (to, bytes)) in symmetric_sent.iter().enumerate() {
            let seen = SocketAddr::new(me.ip(), me.port() + i as u16);
            assert!(symmetric.handle(*to, &reply_for(bytes, seen)), "symmetric answer {i}");
        }
        assert!(cone.done() && cone.next_wake().is_none(), "answered queries are done");
        assert_eq!(cone.result().mapping(), Mapping::EndpointIndependent, "same address for both destinations");
        assert_eq!(symmetric.result().mapping(), Mapping::DestinationDependent, "a port per destination");
    }

    #[test]
    fn ask_over_loopback() {
        // Two peers that reflect, each answering on its own socket until it's quiet for a while.
        let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let peers: Vec<UdpSocket> = (0..2).map(|_| UdpSocket::bind(any).unwrap()).collect();
        let addrs: Vec<SocketAddr> = peers.iter().map(|s| s.local_addr().unwrap()).collect();
        for socket in peers {
            std::thread::spawn(move || {
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut buf = [0u8; 1500];
                while let Ok((len, from)) = socket.recv_from(&mut buf) {
                    if let Some(reply) = answer(&buf[..len], from) {
                        let _ = socket.send_to(&reply, from);
                    }
                }
            });
        }
        let client = UdpSocket::bind(any).unwrap();
        let reflection = ask(&client, &addrs, Style::Padded, Duration::from_secs(10)).unwrap();
        assert_eq!(reflection.mapped(), Some(client.local_addr().unwrap()), "loopback reflects the socket's own address");
        assert_eq!(reflection.mapping(), Mapping::EndpointIndependent, "no NAT on loopback");
    }
}
