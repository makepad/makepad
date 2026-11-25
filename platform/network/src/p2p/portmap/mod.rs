//! Blocking IPv4 UDP port mapping. Call these functions from a network worker.
//! Each lease retains its router and transaction identity for renewal and deletion.

pub mod natpmp;
pub mod pcp;
mod upnp;

use crate::lan;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

const SITE: &str = "p2p portmap";
const DISCOVERY: &str = "p2p upnp discovery";
const FIRST_RETRY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Pcp,
    NatPmp,
    Upnp,
}

#[derive(Clone, Debug)]
pub struct Targets {
    pub gateways: Vec<Ipv4Addr>,
    pub pcp_port: u16,
    pub ssdp: SocketAddr,
}

impl Default for Targets {
    fn default() -> Self {
        Self {
            gateways: makepad_core_util::netif::gateway_candidates(),
            pcp_port: 5351,
            ssdp: SocketAddr::from(([239, 255, 255, 250], 1900)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MapRequest {
    pub internal: SocketAddrV4,
    pub lifetime_secs: u32,
    pub wait: Duration,
}

#[derive(Clone, Debug)]
pub struct PortMapping {
    pub protocol: Protocol,
    pub external: SocketAddrV4,
    pub internal: SocketAddrV4,
    /// Renew halfway through the granted lease. Zero is a permanent UPnP mapping (a router that
    /// keeps no timed ones): it needs no renewal, and `unmap` releases it.
    pub lifetime_secs: u32,
    /// Earlier refusals or port conflicts, including a conflict resolved on retry.
    pub attempts: Vec<Attempt>,
    lease: Lease,
    request: MapRequest,
}

impl PortMapping {
    /// The external address, when it is one a peer on the internet can reach. A router behind
    /// another NAT (a cable modem, a carrier's CGNAT) reports its own outside address, which is
    /// private, shared or link-local: then `None`. The mapping still pins this hop's port; the
    /// public address comes from a reflector. Words never carry a private address.
    pub fn public(&self) -> Option<SocketAddrV4> {
        is_public_v4(*self.external.ip()).then_some(self.external)
    }
}

/// Whether `ip` can be a host's address on the public internet: not private (RFC 1918), shared
/// (RFC 6598 CGNAT, 100.64/10), link-local, loopback, `0/8`, the IETF block `192.0.0/24`
/// (DS-Lite's B4 address lives there), benchmarking (198.18/15), multicast or `240/4`. The
/// documentation ranges count as public: tests stand for public addresses with them.
pub fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19)))
}

#[derive(Clone, Debug)]
enum Lease {
    Pcp(SocketAddr, [u8; 12]),
    NatPmp(SocketAddr),
    Upnp(upnp::Service),
}

#[derive(Clone, Debug)]
pub struct Attempt {
    pub protocol: Protocol,
    pub why: String,
}

impl Attempt {
    fn new(protocol: Protocol, why: impl ToString) -> Self {
        Self { protocol, why: why.to_string() }
    }
}

fn all_failed(why: impl ToString) -> Vec<Attempt> {
    [Protocol::Pcp, Protocol::NatPmp, Protocol::Upnp].map(|p| Attempt::new(p, why.to_string())).into()
}

fn deadline(wait: Duration) -> io::Result<Instant> {
    Instant::now()
        .checked_add(wait)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "wait exceeds the system clock range"))
}

fn remaining(end: Instant) -> io::Result<Duration> {
    let left = end.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::Error::new(io::ErrorKind::TimedOut, "router did not answer before the deadline"))
    } else {
        Ok(left)
    }
}

fn next_port(port: u16) -> u16 {
    port.checked_add(1).unwrap_or(49152)
}

pub fn local_address_towards(gateway: Ipv4Addr) -> io::Result<Ipv4Addr> {
    let addr = SocketAddr::from((gateway, 5351));
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    lan::check(SITE, addr)?;
    socket.connect(addr)?;
    match socket.local_addr()? {
        SocketAddr::V4(addr) => Ok(*addr.ip()),
        _ => Err(io::Error::other("no IPv4 route to gateway")),
    }
}

fn send(socket: &UdpSocket, site: &str, addr: SocketAddr, bytes: &[u8], end: Instant) -> io::Result<()> {
    socket.set_write_timeout(Some(remaining(end)?))?;
    lan::check(site, addr)?;
    socket.send_to(bytes, addr)?;
    Ok(())
}

fn receive(socket: &UdpSocket, bytes: &mut [u8], end: Instant) -> io::Result<Option<(usize, SocketAddr)>> {
    let Ok(left) = remaining(end) else { return Ok(None) };
    socket.set_read_timeout(Some(left))?;
    match socket.recv_from(bytes) {
        Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => Ok(None),
        other => other.map(Some),
    }
}

enum Answer<T> {
    Ignore,
    // A NAT-PMP address reply proves there is a fallback. Give the outstanding
    // PCP MAP time for retransmissions, reserving time to actually create the lease.
    Backup(T),
    Done(T),
}

fn exchange<T>(
    socket: &UdpSocket,
    router: SocketAddr,
    packets: &[&[u8]],
    end: Instant,
    mut accept: impl FnMut(&[u8]) -> Answer<T>,
) -> io::Result<T> {
    // One extra byte distinguishes an oversized (truncated) PCP datagram.
    let mut bytes = [0; 1101];
    let mut interval = FIRST_RETRY;
    let mut stop = end;
    let mut backup = None;
    loop {
        for packet in packets {
            send(socket, SITE, router, packet, stop)?;
        }
        let mut until = Instant::now().checked_add(interval).unwrap_or(stop).min(stop);
        while let Some((len, from)) = receive(socket, &mut bytes, until)? {
            if from != router {
                continue;
            }
            match accept(&bytes[..len]) {
                Answer::Done(result) => return Ok(result),
                Answer::Backup(value) if backup.is_none() => {
                    stop = end - end.saturating_duration_since(Instant::now()) / 3;
                    until = until.min(stop);
                    backup = Some(value);
                }
                _ => (),
            }
        }
        if Instant::now() >= stop {
            return backup.ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "router did not answer before the deadline")
            });
        }
        interval = interval.saturating_mul(2);
    }
}

fn pcp_map(
    socket: &UdpSocket,
    router: SocketAddr,
    map: &mut pcp::Map,
    end: Instant,
    discover: bool,
    nat_ip: &mut Option<Ipv4Addr>,
    attempts: &mut Vec<Attempt>,
) -> io::Result<(SocketAddrV4, u32)> {
    let mut retried = false;
    loop {
        let bytes = map.encode();
        let mut packets = vec![bytes.as_slice()];
        if discover {
            packets.push(&natpmp::EXTERNAL_ADDRESS);
        }
        let reply = exchange(socket, router, &packets, end, |bytes| {
            if let Some(reply) = map.reply(bytes) {
                return Answer::Done(Ok(reply));
            }
            if pcp::nat_pmp_only(bytes) {
                return Answer::Done(Err("router replied UNSUPP_VERSION"));
            }
            if discover {
                if let Some(natpmp::Reply {
                    result: natpmp::ResultCode::Success,
                    payload: natpmp::Payload::Address(ip),
                    ..
                }) = natpmp::reply(bytes)
                {
                    if !ip.is_unspecified() {
                        *nat_ip = Some(ip);
                        return Answer::Backup(Err("no PCP reply while NAT-PMP answered"));
                    }
                }
            }
            Answer::Ignore
        })?
        .map_err(io::Error::other)?;
        let ip = reply.external_addr.to_ipv4_mapped();
        match reply.result {
            pcp::ResultCode::Success if map.lifetime == 0 && reply.lifetime == 0 => {
                return Ok((map.suggested, 0))
            }
            pcp::ResultCode::Success
                if map.lifetime != 0 && reply.lifetime != 0 && reply.external_port != 0 =>
            {
                let ip = ip
                    .filter(|ip| !ip.is_unspecified())
                    .ok_or_else(|| io::Error::other("PCP returned no IPv4 address"))?;
                return Ok((SocketAddrV4::new(ip, reply.external_port), reply.lifetime));
            }
            pcp::ResultCode::CannotProvideExternal if !retried && map.lifetime != 0 => {
                retried = true;
                let old = map.suggested.port();
                let port = if reply.external_port != 0 && reply.external_port != old {
                    reply.external_port
                } else {
                    next_port(old)
                };
                attempts.push(Attempt::new(
                    Protocol::Pcp,
                    format!("port {old} conflicted (CANNOT_PROVIDE_EXTERNAL), retrying {port}"),
                ));
                map.suggested = SocketAddrV4::new(ip.unwrap_or(*map.suggested.ip()), port);
            }
            code => {
                return Err(io::Error::other(format!(
                    "PCP MAP returned {code:?} (lifetime {}, port {})",
                    reply.lifetime, reply.external_port
                )))
            }
        }
    }
}

fn nat_reply(
    socket: &UdpSocket,
    router: SocketAddr,
    request: &[u8],
    end: Instant,
    matches: impl Fn(natpmp::Payload) -> bool,
) -> io::Result<natpmp::Payload> {
    exchange(socket, router, &[request], end, |bytes| {
        if bytes.get(1) != Some(&(128 + request[1])) {
            return Answer::Ignore;
        }
        let Some(reply) = natpmp::reply(bytes) else { return Answer::Ignore };
        if reply.result != natpmp::ResultCode::Success {
            return Answer::Done(Err(io::Error::other(format!("NAT-PMP returned {:?}", reply.result))));
        }
        if matches(reply.payload) {
            Answer::Done(Ok(reply.payload))
        } else {
            Answer::Ignore
        }
    })?
}

fn nat_map(
    socket: &UdpSocket,
    router: SocketAddr,
    req: &MapRequest,
    suggested: SocketAddrV4,
    mut address: Option<Ipv4Addr>,
    end: Instant,
) -> io::Result<(SocketAddrV4, u32)> {
    if address.is_none() && req.lifetime_secs != 0 {
        let reply = nat_reply(socket, router, &natpmp::EXTERNAL_ADDRESS, end, |p| {
            matches!(p, natpmp::Payload::Address(_))
        })?;
        if let natpmp::Payload::Address(ip) = reply {
            if ip.is_unspecified() {
                return Err(io::Error::other("NAT-PMP returned no external address"));
            }
            address = Some(ip);
        }
    }
    let port = req.internal.port();
    let bytes = natpmp::map(port, suggested.port(), req.lifetime_secs);
    let reply = nat_reply(
        socket,
        router,
        &bytes,
        end,
        |p| matches!(p, natpmp::Payload::Mapping { internal, .. } if internal == port),
    )?;
    if let natpmp::Payload::Mapping { external, lifetime, .. } = reply {
        if (req.lifetime_secs == 0 && external == 0 && lifetime == 0)
            || (req.lifetime_secs != 0 && external != 0 && lifetime != 0)
        {
            return Ok((SocketAddrV4::new(address.unwrap_or(*suggested.ip()), external), lifetime));
        }
    }
    Err(io::Error::other("NAT-PMP returned an invalid lease"))
}

fn discovery_allowed(target: SocketAddr) -> io::Result<()> {
    if target.ip().is_multicast() && !lan::service(DISCOVERY) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            lan::off_why().unwrap_or_else(|| "local network discovery is off".into()),
        ));
    }
    lan::check(DISCOVERY, target)
}

fn upnp_map(
    service: &upnp::Service,
    req: &MapRequest,
    port: u16,
    end: Instant,
    attempts: &mut Vec<Attempt>,
) -> Result<(SocketAddrV4, u32), upnp::SoapError> {
    // Discover the outside address before changing any router state.
    let ip = service.external_ip(end)?;
    let mut lifetime = service.lease_duration(req.lifetime_secs);
    let mut external = port;
    let mut conflicted = false;
    loop {
        match service.add(req.internal, external, lifetime, end) {
            Ok(()) => break,
            Err(upnp::SoapError::Fault(718)) if !conflicted => {
                conflicted = true;
                external = next_port(port);
                attempts.push(Attempt::new(
                    Protocol::Upnp,
                    format!("port {port} conflicted (UPnP 718), retrying {external}"),
                ));
            }
            // Many IGDv1 routers keep only permanent mappings. Take one; `unmap` releases it.
            Err(upnp::SoapError::Fault(725)) if lifetime != 0 => {
                lifetime = 0;
                attempts.push(Attempt::new(
                    Protocol::Upnp,
                    "router keeps only permanent mappings (UPnP 725); mapped until released",
                ));
            }
            Err(e) => return Err(e),
        }
    }
    Ok((SocketAddrV4::new(ip, external), lifetime))
}

fn discover(
    target: SocketAddr,
    req: &MapRequest,
    end: Instant,
    attempts: &mut Vec<Attempt>,
) -> io::Result<(Lease, SocketAddrV4, u32)> {
    discovery_allowed(target)?;
    let socket = UdpSocket::bind((*req.internal.ip(), 0))?;
    send(&socket, DISCOVERY, target, upnp::search(target).as_bytes(), end)?;
    let mut bytes = vec![0; u16::MAX as usize];
    let mut last_error = None;
    while let Some((len, from)) = receive(&socket, &mut bytes, end)? {
        if !target.ip().is_multicast() && from.ip() != target.ip() {
            continue;
        }
        let Some(location) = upnp::location(&bytes[..len]) else { continue };
        let service = match upnp::Url::parse(location, from.ip()).and_then(|url| upnp::describe(url, end)) {
            Ok(s) => s,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        match upnp_map(&service, req, req.internal.port(), end, attempts) {
            Ok((external, lifetime)) => return Ok((Lease::Upnp(service), external, lifetime)),
            Err(e) => last_error = Some(io::Error::other(e.to_string())),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "no SSDP answer")))
}

fn mapping(
    req: &MapRequest,
    lease: Lease,
    external: SocketAddrV4,
    lifetime_secs: u32,
    attempts: Vec<Attempt>,
) -> PortMapping {
    let protocol = match lease {
        Lease::Pcp(..) => Protocol::Pcp,
        Lease::NatPmp(_) => Protocol::NatPmp,
        Lease::Upnp(_) => Protocol::Upnp,
    };
    PortMapping {
        protocol,
        internal: req.internal,
        external,
        lifetime_secs,
        attempts,
        request: req.clone(),
        lease,
    }
}

pub fn map_udp(targets: &Targets, req: &MapRequest) -> Result<PortMapping, Vec<Attempt>> {
    let end = deadline(req.wait).map_err(all_failed)?;
    if targets.gateways.is_empty() {
        discovery_allowed(targets.ssdp).map_err(all_failed)?;
    }
    for gateway in &targets.gateways {
        lan::check(SITE, SocketAddr::from((*gateway, targets.pcp_port))).map_err(all_failed)?;
    }
    if req.internal.port() == 0 || req.internal.ip().is_unspecified() || req.lifetime_secs == 0 {
        return Err(all_failed(
            "supply the local socket's IPv4 address, nonzero port and positive lease duration",
        ));
    }
    // Reserve one third for discovery and SOAP if every UDP gateway is silent.
    // An early refusal leaves all unused time for the next candidate.
    let reserve = req.wait / 3;
    let udp_end = end - reserve;
    let mut attempts = Vec::new();
    let suggested = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, req.internal.port());
    for (i, gateway) in targets.gateways.iter().enumerate() {
        let router = SocketAddr::from((*gateway, targets.pcp_port));
        let now = Instant::now();
        let share = udp_end.saturating_duration_since(now)
            / u32::try_from(targets.gateways.len() - i).unwrap_or(u32::MAX);
        let router_end = now + share;
        let socket = UdpSocket::bind((*req.internal.ip(), 0)).map_err(all_failed)?;
        let mut nonce = [0; 12];
        let mut address = None;
        let result = crate::tls::os_random(&mut nonce).and_then(|()| {
            let mut map = pcp::Map { nonce, internal: req.internal, suggested, lifetime: req.lifetime_secs };
            pcp_map(&socket, router, &mut map, router_end, true, &mut address, &mut attempts)
        });
        match result {
            Ok((external, secs)) => {
                return Ok(mapping(req, Lease::Pcp(router, nonce), external, secs, attempts))
            }
            Err(e) => attempts.push(Attempt::new(Protocol::Pcp, format!("{router}: {e}"))),
        }
        match nat_map(&socket, router, req, suggested, address, router_end) {
            Ok((external, secs)) => return Ok(mapping(req, Lease::NatPmp(router), external, secs, attempts)),
            Err(e) => attempts.push(Attempt::new(Protocol::NatPmp, format!("{router}: {e}"))),
        }
    }
    if targets.gateways.is_empty() {
        for protocol in [Protocol::Pcp, Protocol::NatPmp] {
            attempts.push(Attempt::new(protocol, "no IPv4 gateway candidates"));
        }
    }
    match discover(targets.ssdp, req, end, &mut attempts) {
        Ok((lease, external, secs)) => Ok(mapping(req, lease, external, secs, attempts)),
        Err(e) => {
            attempts.push(Attempt::new(Protocol::Upnp, e));
            Err(attempts)
        }
    }
}

fn update(lease: &PortMapping, delete: bool, attempts: &mut Vec<Attempt>) -> io::Result<(SocketAddrV4, u32)> {
    let mut req = lease.request.clone();
    let end = deadline(req.wait)?;
    if delete {
        req.lifetime_secs = 0;
    }
    match &lease.lease {
        Lease::Pcp(router, nonce) => {
            let mut map = pcp::Map {
                internal: req.internal,
                nonce: *nonce,
                suggested: lease.external,
                lifetime: req.lifetime_secs,
            };
            let socket = UdpSocket::bind((*req.internal.ip(), 0))?;
            pcp_map(&socket, *router, &mut map, end, false, &mut None, attempts)
        }
        Lease::NatPmp(router) => {
            let socket = UdpSocket::bind((*req.internal.ip(), 0))?;
            nat_map(&socket, *router, &req, lease.external, None, end)
        }
        Lease::Upnp(service) => if delete {
            service.delete(lease.external.port(), end).map(|()| (lease.external, 0))
        } else {
            upnp_map(service, &req, lease.external.port(), end, attempts)
        }
        .map_err(|e| io::Error::other(e.to_string())),
    }
}

/// Refresh on the original router, even if the list of candidates has changed.
pub fn renew(_targets: &Targets, mapping: &PortMapping) -> Result<PortMapping, Vec<Attempt>> {
    let mut attempts = Vec::new();
    match update(mapping, false, &mut attempts) {
        Ok((external, secs)) => {
            Ok(self::mapping(&mapping.request, mapping.lease.clone(), external, secs, attempts))
        }
        Err(e) => {
            attempts.push(Attempt::new(mapping.protocol, e));
            Err(attempts)
        }
    }
}

/// Best-effort release, bounded by the original request's wait.
pub fn unmap(_targets: &Targets, mapping: &PortMapping) {
    let _ = update(mapping, true, &mut Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    const INSIDE: u16 = 32000;
    const OUTSIDE: u16 = 32123;
    const PUBLIC: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);
    const TEST_WAIT: Duration = Duration::from_secs(2);

    fn udp() -> UdpSocket {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket.set_read_timeout(Some(TEST_WAIT)).unwrap();
        socket
    }

    fn targets(router: &UdpSocket, ssdp: &UdpSocket) -> Targets {
        Targets {
            gateways: vec![Ipv4Addr::LOCALHOST],
            pcp_port: router.local_addr().unwrap().port(),
            ssdp: ssdp.local_addr().unwrap(),
        }
    }

    fn request() -> MapRequest {
        MapRequest {
            internal: SocketAddrV4::new(Ipv4Addr::LOCALHOST, INSIDE),
            lifetime_secs: 120,
            wait: TEST_WAIT,
        }
    }

    fn packet(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buf = [0; 1500];
        let (len, from) = socket.recv_from(&mut buf).unwrap();
        (buf[..len].to_vec(), from)
    }

    fn answer(socket: &UdpSocket, peer: SocketAddr, bytes: &[u8]) {
        send(socket, SITE, peer, bytes, Instant::now() + TEST_WAIT).unwrap();
    }

    fn address_reply() -> [u8; 12] {
        [0, 128, 0, 0, 0, 0, 0, 7, 203, 0, 113, 7]
    }

    #[test]
    fn only_public_external_addresses_reach_the_words() {
        for ip in ["10.77.0.1", "172.16.77.1", "192.168.77.1", "100.64.0.1", "100.127.255.254", "169.254.1.1", "127.0.0.1",
            "0.0.0.0", "192.0.0.2", "198.18.0.1", "224.0.0.1", "240.0.0.1", "255.255.255.255"] {
            assert!(!is_public_v4(ip.parse().unwrap()), "{ip} is not public");
        }
        for ip in ["203.0.113.7", "198.51.100.23", "192.0.2.7", "100.63.255.255", "100.128.0.0"] {
            assert!(is_public_v4(ip.parse().unwrap()), "{ip} is public");
        }
    }

    #[test]
    fn pcp_preferred_retry_renew_and_release() {
        let router = udp();
        let ssdp = udp();
        let targets = targets(&router, &ssdp);
        let server = thread::spawn(move || {
            // Lose only the first MAP. A NAT-PMP address reply must not preempt its retry.
            let (lost, client) = packet(&router);
            assert_eq!(packet(&router), (vec![0, 0], client), "parallel NAT-PMP address query");
            answer(&router, client, &address_reply());
            for (step, (suggested, requested, code)) in
                [(INSIDE, 120u32, 11u8), (OUTSIDE, 120, 0), (OUTSIDE, 120, 0), (OUTSIDE, 0, 0)]
                    .into_iter()
                    .enumerate()
            {
                let (bytes, peer) = packet(&router);
                assert_eq!(bytes[..2], [2, 1], "expected PCP MAP on step {step}");
                if step == 0 {
                    assert_eq!((&bytes, peer), (&lost, client), "identical retransmission");
                }
                assert_eq!(bytes[24..36], lost[24..36], "lease nonce changed on step {step}");
                assert_eq!(bytes[4..8], requested.to_be_bytes(), "requested lifetime on step {step}");
                assert_eq!(bytes[8..24], Ipv4Addr::LOCALHOST.to_ipv6_mapped().octets(), "PCP client IP");
                assert_eq!(bytes[40..42], INSIDE.to_be_bytes(), "mapped socket");
                assert_eq!(bytes[42..44], suggested.to_be_bytes(), "suggestion on step {step}");
                if step >= 2 {
                    assert_eq!(bytes[44..60], PUBLIC.to_ipv6_mapped().octets(), "retained public address");
                }
                if step < 2 {
                    assert_eq!(packet(&router), (vec![0, 0], peer), "same socket for both probes");
                    answer(&router, peer, &address_reply()); // arrives before PCP
                }
                let mut reply = bytes;
                reply[1] = 129;
                reply[3] = code;
                reply[4..8].copy_from_slice(&requested.min(60).to_be_bytes());
                reply[8..24].fill(0);
                reply[42..44].copy_from_slice(&OUTSIDE.to_be_bytes());
                reply[44..60].copy_from_slice(&PUBLIC.to_ipv6_mapped().octets());
                answer(&router, peer, &reply);
            }
        });
        let lease = map_udp(&targets, &request()).unwrap();
        let renewed = renew(&targets, &lease).unwrap();
        unmap(&targets, &renewed);
        server.join().unwrap();
        assert_eq!(lease.protocol, Protocol::Pcp, "PCP wins when both answer");
        assert_eq!(lease.public(), Some(SocketAddrV4::new(PUBLIC, OUTSIDE)), "a public external address goes in the words");
        assert_eq!(renewed.external, SocketAddrV4::new(PUBLIC, OUTSIDE), "router suggestion retained");
        assert_eq!(renewed.lifetime_secs, 60, "granted lifetime");
        assert!(
            lease.attempts.iter().any(|a| a.why.contains("CANNOT_PROVIDE_EXTERNAL")),
            "conflict reported"
        );
    }

    #[test]
    fn nat_pmp_only_renew_and_release() {
        let router = udp();
        let ssdp = udp();
        let targets = targets(&router, &ssdp);
        let server = thread::spawn(move || {
            let mut mappings = 0;
            while mappings < 3 {
                let (bytes, peer) = packet(&router);
                match &bytes[..2] {
                    [2, 1] => answer(&router, peer, &[0, 0, 0, 1, 0, 0, 0, 7]),
                    [0, 0] => answer(&router, peer, &address_reply()),
                    [0, 1] => {
                        let port = [INSIDE, OUTSIDE, 0][mappings];
                        let life = [120u32, 120, 0][mappings];
                        assert_eq!(bytes[4..6], INSIDE.to_be_bytes(), "internal port on step {mappings}");
                        assert_eq!(bytes[6..8], port.to_be_bytes(), "external port on step {mappings}");
                        assert_eq!(bytes[8..12], life.to_be_bytes(), "lifetime on step {mappings}");
                        let mut reply = vec![0, 129, 0, 0, 0, 0, 0, 7];
                        reply.extend_from_slice(&INSIDE.to_be_bytes());
                        reply.extend_from_slice(&(if life == 0 { 0 } else { OUTSIDE }).to_be_bytes());
                        reply.extend_from_slice(&life.min(30).to_be_bytes());
                        answer(&router, peer, &reply);
                        mappings += 1;
                    }
                    _ => panic!("unexpected UDP request {bytes:?}"),
                }
            }
        });
        let lease = map_udp(&targets, &request()).unwrap();
        let renewed = renew(&targets, &lease).unwrap();
        unmap(&targets, &renewed);
        server.join().unwrap();
        assert_eq!(lease.protocol, Protocol::NatPmp, "legacy router fallback");
        assert_eq!(renewed.external, SocketAddrV4::new(PUBLIC, OUTSIDE), "renewed public endpoint");
        assert_eq!(renewed.lifetime_secs, 30, "granted NAT-PMP lease");
        assert!(lease.attempts.iter().any(|a| a.why.contains("UNSUPP_VERSION")), "recorded version fallback");
    }

    #[test]
    fn upnp_fallback_conflict_renew_and_release() {
        let router = udp();
        let ssdp = udp();
        let targets = targets(&router, &ssdp);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let http = listener.local_addr().unwrap();
        let datagrams = thread::spawn(move || {
            // Explicit UDP refusals leave the full unused deadline for UPnP.
            let (_, peer) = packet(&router);
            answer(&router, peer, &[0, 0, 0, 1, 0, 0, 0, 7]);
            let mut denied = address_reply();
            denied[3] = 2;
            let (query, peer) = packet(&router);
            assert_eq!(query, [0, 0], "parallel query");
            answer(&router, peer, &denied);
            let (search, client) = packet(&ssdp);
            let search = String::from_utf8(search).unwrap();
            for part in [
                "M-SEARCH * HTTP/1.1",
                "MX: 2\r\n",
                "MAN: \"ssdp:discover\"",
                "ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1",
            ] {
                assert!(search.contains(part), "missing SSDP field {part}");
            }
            answer(
                &ssdp,
                client,
                format!("HTTP/1.1 200 OK\r\nLocation: http://{http}/root/desc.xml\r\n\r\n").as_bytes(),
            );
        });
        let server = thread::spawn(move || {
            let kind = "urn:schemas-upnp-org:service:WANIPConnection:1";
            let mut seen = Vec::new();
            for conn in listener.incoming() {
                let mut conn = conn.unwrap();
                conn.set_read_timeout(Some(TEST_WAIT)).unwrap();
                conn.set_write_timeout(Some(TEST_WAIT)).unwrap();
                let mut reader = BufReader::new(&conn);
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap() == 0 {
                    break;
                } // test finished
                let mut length = 0;
                let mut action = String::new();
                let mut close = false;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0, "incomplete request headers");
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length: ") {
                        length = value.trim().parse().unwrap();
                    }
                    if let Some(value) = line.strip_prefix("SOAPAction: ") {
                        action =
                            value.trim().trim_matches('"').strip_prefix(&format!("{kind}#")).unwrap().into();
                    }
                    close |= line == "Connection: close\r\n";
                }
                assert!(close, "one HTTP request per connection");
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                if first == "GET /root/desc.xml HTTP/1.1\r\n" {
                    let xml = format!("<root><service><serviceType>{kind}</serviceType><controlURL>soap</controlURL></service></root>");
                    write!(conn, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x};ext=1\r\n{xml}\r\n0\r\nX-End: yes\r\n\r\n", xml.len()).unwrap();
                } else {
                    assert_eq!(first, "POST /root/soap HTTP/1.1\r\n", "resolved control URL");
                    assert!(
                        body.contains(&format!("<u:{action} xmlns:u=\"{kind}\">")),
                        "SOAP envelope and action agree"
                    );
                    let mut status = "200 OK";
                    let xml = match action.as_str() {
                        "GetExternalIPAddress" => format!("<u:{action}Response><NewExternalIPAddress>{PUBLIC}</NewExternalIPAddress></u:{action}Response>"),
                        "AddPortMapping" | "DeletePortMapping" => {
                            for field in ["<NewRemoteHost></NewRemoteHost>", "<NewProtocol>UDP</NewProtocol>"] {
                                assert!(body.contains(field), "missing {field}");
                            }
                            if action == "AddPortMapping" {
                                for field in ["<NewInternalPort>32000</NewInternalPort>", "<NewInternalClient>127.0.0.1</NewInternalClient>",
                                    "<NewEnabled>1</NewEnabled>", "<NewPortMappingDescription>makepad p2p</NewPortMappingDescription>"] {
                                    assert!(body.contains(field), "missing {field}");
                                }
                            }
                            let fault = |code: u32| format!("<s:Fault><detail><UPnPError><errorCode>{code}</errorCode></UPnPError></detail></s:Fault>");
                            if body.contains("<NewExternalPort>32000</NewExternalPort>") {
                                status = "500 Internal Server Error";
                                fault(718)
                            } else if body.contains("<NewLeaseDuration>120</NewLeaseDuration>") {
                                // An IGDv1 router that keeps only permanent mappings.
                                status = "500 Internal Server Error";
                                fault(725)
                            } else {
                                assert!(body.contains("<NewExternalPort>32001</NewExternalPort>"), "retried external port");
                                if action == "AddPortMapping" {
                                    assert!(body.contains("<NewLeaseDuration>0</NewLeaseDuration>"), "permanent lease after 725");
                                }
                                format!("<u:{action}Response/>")
                            }
                        }
                        _ => panic!("unexpected SOAP action {action}"),
                    };
                    // Exercise both length-delimited and close-delimited SOAP responses.
                    let size = if status.starts_with("500") {
                        String::new()
                    } else {
                        format!("Content-Length: {}\r\n", xml.len())
                    };
                    write!(conn, "HTTP/1.1 {status}\r\n{size}\r\n{xml}").unwrap();
                }
                seen.push(action);
            }
            seen
        });
        let result = map_udp(&targets, &request()).and_then(|lease| {
            let renewed = renew(&targets, &lease)?;
            unmap(&targets, &renewed);
            Ok((lease, renewed))
        });
        // Wake accept even when a client operation failed.
        drop(lan::connect_timeout(SITE, &http, TEST_WAIT));
        let seen = server.join().unwrap();
        datagrams.join().unwrap();
        let (lease, renewed) = result.unwrap();
        assert_eq!(lease.protocol, Protocol::Upnp, "IGD after both UDP protocols refused");
        assert_eq!(renewed.external, SocketAddrV4::new(PUBLIC, INSIDE + 1), "renewed IGD port");
        assert!(lease.attempts.iter().any(|a| a.why.contains("718")), "IGD conflict reported");
        assert!(lease.attempts.iter().any(|a| a.why.contains("725")), "permanent-only router reported");
        assert_eq!(renewed.lifetime_secs, 0, "permanent mapping after UPnP 725");
        assert_eq!(
            seen,
            [
                "",
                "GetExternalIPAddress",
                "AddPortMapping",
                "AddPortMapping",
                "AddPortMapping",
                "GetExternalIPAddress",
                "AddPortMapping",
                "AddPortMapping",
                "DeletePortMapping"
            ],
            "SOAP lifecycle"
        );
    }

    #[test]
    fn silent_gateways_share_one_deadline() {
        let router = udp();
        let ssdp = udp();
        let targets = targets(&router, &ssdp);
        let sink = thread::spawn(move || {
            assert_eq!(packet(&router).0[0], 2, "PCP attempted");
            assert_eq!(packet(&router).0, [0, 0], "NAT-PMP attempted");
            assert!(packet(&ssdp).0.starts_with(b"M-SEARCH "), "UPnP attempted");
        });
        let req = MapRequest { wait: Duration::from_millis(300), ..request() };
        assert_eq!(local_address_towards(Ipv4Addr::LOCALHOST).unwrap(), Ipv4Addr::LOCALHOST);
        if !lan::allowed() {
            let blocked = Targets { gateways: vec![Ipv4Addr::BROADCAST], ..targets.clone() };
            let attempts = map_udp(&blocked, &req).unwrap_err();
            let reason = lan::check(SITE, SocketAddr::from((Ipv4Addr::BROADCAST, blocked.pcp_port)))
                .unwrap_err()
                .to_string();
            assert_eq!(attempts.len(), 3, "one LAN refusal per protocol");
            assert!(attempts.iter().all(|a| a.why == reason), "report the LAN policy refusal verbatim");
        }
        let start = Instant::now();
        let failures = map_udp(&targets, &req).unwrap_err();
        let elapsed = start.elapsed();
        sink.join().unwrap();
        assert!(elapsed < req.wait + Duration::from_millis(350), "timeouts accumulated: {elapsed:?}");
        assert_eq!(
            failures.iter().map(|a| a.protocol).collect::<Vec<_>>(),
            [Protocol::Pcp, Protocol::NatPmp, Protocol::Upnp]
        );
    }
}
