//! Every call on one UDP port. One long-lived worker thread owns the socket
//! and each call's [`Peer`]: it takes datagrams, runs the peers' timers and
//! sends what they give back, and hands their events to the app over a
//! channel. A datagram goes to its call by the USERNAME of a connectivity
//! check (our ufrag in it), and after that by the address that passed one.
//! Anything else is dropped unread, and touches no call: a call's timers
//! run when they are due, not on every datagram.
//!
//! Offers are answered on the caller's thread (an HTTP worker, say): it
//! makes the peer, and the worker takes it over through a channel, woken by
//! an empty datagram to its own socket.

use crate::dtls::Certificate;
use crate::peer::{Event, Peer, PeerError};
use crate::stun;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub type CallId = u64;

enum Command {
    Add(CallId, Box<Peer>),
    Keyframe(CallId),
    Close(CallId),
    Stop,
}

/// The calls on one port, and the worker that runs them. Dropping it hangs
/// up every call and ends the worker.
pub struct Calls {
    port: u16,
    cert: Certificate,
    candidates: Vec<SocketAddr>,
    commands: Sender<Command>,
    waker: UdpSocket,
    wake: SocketAddr,
    next_id: AtomicU64,
    started: Instant,
    worker: Option<JoinHandle<()>>,
}

impl Calls {
    /// Binds `bind` (port 0: any free one) and starts the worker. Answers
    /// name `addresses` (those of this machine that browsers can reach) at
    /// the bound port. Each call's events go to `events`.
    pub fn start(bind: SocketAddr, addresses: &[IpAddr], events: Sender<(CallId, Event)>) -> io::Result<Calls> {
        let socket = UdpSocket::bind(bind)?;
        let local = socket.local_addr()?;
        let wake_ip = match local.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        let cert = Certificate::generate().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("{e:?}")))?;
        let (commands, commanded) = mpsc::channel();
        let waker = socket.try_clone()?;
        let started = Instant::now();
        let worker = std::thread::Builder::new().name("webrtc-calls".into()).spawn(move || run(socket, commanded, events, started))?;
        Ok(Calls {
            port: local.port(),
            cert,
            candidates: addresses.iter().map(|ip| SocketAddr::new(*ip, local.port())).collect(),
            commands,
            waker,
            wake: SocketAddr::new(wake_ip, local.port()),
            next_id: AtomicU64::new(1),
            started,
            worker: Some(worker),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// A new call for a browser's offer: its id in the events, and the SDP
    /// answer to send back.
    pub fn offer(&self, sdp: &str) -> Result<(CallId, String), PeerError> {
        let now = self.started.elapsed().as_secs_f64();
        let peer = Peer::new(sdp, &self.cert, &self.candidates, now)?;
        let answer = peer.answer().to_string();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.command(Command::Add(id, Box::new(peer)));
        Ok((id, answer))
    }

    /// Asks one call's sender for a keyframe now.
    pub fn keyframe(&self, call: CallId) {
        self.command(Command::Keyframe(call));
    }

    /// Hangs up one call; its `Gone` event follows.
    pub fn close(&self, call: CallId) {
        self.command(Command::Close(call));
    }

    fn command(&self, command: Command) {
        // The worker only ends when this ends: a send cannot fail before.
        let _ = self.commands.send(command);
        let _ = self.waker.send_to(&[], self.wake);
    }
}

impl Drop for Calls {
    fn drop(&mut self) {
        self.command(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    socket: UdpSocket,
    events: Sender<(CallId, Event)>,
    calls: HashMap<CallId, Call>,
    by_ufrag: HashMap<String, CallId>,
    /// Each call's heard addresses (at most [`crate::peer::MAX_CHECKED`] each).
    by_addr: HashMap<SocketAddr, CallId>,
}

struct Call {
    peer: Peer,
    /// When its timers are next due ([`Peer::next_wake`]).
    wake: f64,
}

fn run(socket: UdpSocket, commands: Receiver<Command>, events: Sender<(CallId, Event)>, started: Instant) {
    let mut worker = Worker { socket, events, calls: HashMap::new(), by_ufrag: HashMap::new(), by_addr: HashMap::new() };
    let mut buf = vec![0u8; 65_536];
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Add(id, peer)) => worker.add(id, *peer, started.elapsed().as_secs_f64()),
                Ok(Command::Keyframe(id)) => {
                    if let Some(call) = worker.calls.get_mut(&id) {
                        call.peer.ask_keyframe();
                        worker.after(id, Vec::new(), started.elapsed().as_secs_f64());
                    }
                }
                Ok(Command::Close(id)) => worker.close(id, started.elapsed().as_secs_f64()),
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                    let ids: Vec<CallId> = worker.calls.keys().copied().collect();
                    for id in ids {
                        worker.close(id, started.elapsed().as_secs_f64());
                    }
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let now = started.elapsed().as_secs_f64();
        for id in worker.due(now) {
            let events = worker.calls.get_mut(&id).map(|call| call.peer.poll(now)).unwrap_or_default();
            worker.after(id, events, now);
        }
        let wake = worker.calls.values().map(|call| call.wake).fold(f64::INFINITY, f64::min);
        // At least a millisecond: a deadline just passed is handled on the next round.
        let wait = wake.is_finite().then(|| Duration::from_secs_f64((wake - now).max(0.001)));
        if worker.socket.set_read_timeout(wait).is_err() {
            return;
        }
        match worker.socket.recv_from(&mut buf) {
            Ok((n, from)) => worker.datagram(&buf[..n], from, started.elapsed().as_secs_f64()),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
            // An ICMP error reported on the socket names one peer, not this
            // socket: keep serving the others.
            Err(_) => {}
        }
    }
}

impl Worker {
    fn add(&mut self, id: CallId, peer: Peer, now: f64) {
        self.by_ufrag.insert(peer.local_ufrag().to_string(), id);
        let wake = peer.next_wake(now);
        self.calls.insert(id, Call { peer, wake });
    }

    /// The calls whose timers are due.
    fn due(&self, now: f64) -> Vec<CallId> {
        self.calls.iter().filter(|(_, call)| call.wake <= now).map(|(id, _)| *id).collect()
    }

    fn datagram(&mut self, datagram: &[u8], from: SocketAddr, now: f64) {
        let Some(&first) = datagram.first() else { return };
        let id = if first <= 3 {
            let Ok(message) = stun::parse(datagram) else { return };
            let ours = message.username().and_then(|u| u.split(':').next()).unwrap_or_default();
            self.by_ufrag.get(ours).copied()
        } else {
            self.by_addr.get(&from).copied()
        };
        let Some(id) = id else { return };
        let Some(call) = self.calls.get_mut(&id) else { return };
        let events = call.peer.receive(datagram, from, now);
        // A newly heard address may have pushed out an old one.
        if call.peer.knows(from) && self.by_addr.insert(from, id).is_none() {
            let peer = &call.peer;
            self.by_addr.retain(|addr, other| *other != id || peer.knows(*addr));
        }
        self.after(id, events, now);
    }

    /// Sends what the call gave back, hands on its events, and forgets it
    /// once it is gone.
    fn after(&mut self, id: CallId, events: Vec<Event>, now: f64) {
        let Some(call) = self.calls.get_mut(&id) else { return };
        for (datagram, to) in call.peer.transmit() {
            let _ = self.socket.send_to(&datagram, to);
        }
        call.wake = call.peer.next_wake(now);
        let gone = call.peer.is_gone();
        for event in events {
            let _ = self.events.send((id, event));
        }
        if gone {
            self.forget(id);
        }
    }

    fn close(&mut self, id: CallId, now: f64) {
        let Some(call) = self.calls.get_mut(&id) else { return };
        call.peer.close();
        self.after(id, vec![Event::Gone("hung up here".into())], now);
    }

    fn forget(&mut self, id: CallId) {
        self.calls.remove(&id);
        self.by_ufrag.retain(|_, call| *call != id);
        self.by_addr.retain(|_, call| *call != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{pictures, Browser};

    /// A phone on its own UDP socket, talking to the calls' port.
    struct Phone {
        socket: UdpSocket,
        browser: Browser,
        answer: String,
        id: CallId,
    }

    impl Phone {
        fn new(calls: &Calls) -> Phone {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            socket.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
            let browser = Browser::new(&socket.local_addr().unwrap().to_string());
            let (id, answer) = calls.offer(&browser.offer()).unwrap();
            Phone { socket, browser, answer, id }
        }

        fn send(&self, datagram: &[u8], to: SocketAddr) {
            self.socket.send_to(datagram, to).unwrap();
        }

        fn connect(&mut self, to: SocketAddr) {
            assert!(self.browser.connect(&self.socket, &self.answer, to, Duration::from_secs(3)), "the phone has its keys");
        }
    }

    #[test]
    fn two_phones_on_one_port_get_their_own_calls() {
        let (tx, rx) = mpsc::channel();
        let calls = Calls::start("127.0.0.1:0".parse().unwrap(), &["127.0.0.1".parse().unwrap()], tx).unwrap();
        let port: SocketAddr = ("127.0.0.1".parse::<IpAddr>().unwrap(), calls.port()).into();
        let mut phones = [Phone::new(&calls), Phone::new(&calls)];
        assert!(phones[0].answer.contains(&format!("127.0.0.1 {}", calls.port())), "the answer names the port");
        assert_ne!(phones[0].id, phones[1].id);

        // A stranger is not answered.
        let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
        stranger.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        stranger.send_to(&[22, 0xfe, 0xfd, 0, 0], port).unwrap();
        assert!(stranger.recv_from(&mut [0u8; 64]).is_err(), "an address that passed no check gets nothing");

        for phone in &mut phones {
            phone.connect(port);
        }
        let sent = pictures();
        for phone in &mut phones {
            for picture in &sent {
                for packet in phone.browser.video(picture) {
                    phone.send(&packet, port);
                }
            }
        }
        let mut got: HashMap<CallId, Vec<Event>> = HashMap::new();
        let started = Instant::now();
        while got.values().map(|e| e.iter().filter(|e| matches!(e, Event::Picture(_))).count()).sum::<usize>() < 2 * sent.len() {
            let left = Duration::from_secs(3).checked_sub(started.elapsed()).expect("every picture within 3 s");
            let (id, event) = rx.recv_timeout(left).expect("the calls' events");
            got.entry(id).or_default().push(event);
        }
        for phone in &phones {
            let mut want = vec![Event::Connected];
            want.extend(sent.iter().cloned().map(Event::Picture));
            assert_eq!(got[&phone.id], want, "call {} has its phone's pictures, in order", phone.id);
        }

        // A keyframe asked for: the phone hears a PLI.
        calls.keyframe(phones[0].id);
        let mut buf = [0u8; 2048];
        let mut pli = false;
        while let Ok((n, _)) = phones[0].socket.recv_from(&mut buf) {
            let plain = phones[0].browser.feedback(&buf[..n]).unwrap_or_default();
            pli |= plain.len() >= 2 && plain[1] == 206 && plain[0] & 0x1f == 1;
        }
        assert!(pli, "the phone is asked for a keyframe");

        // Hanging up here sends close_notify, and the call is gone.
        calls.close(phones[0].id);
        let (id, event) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(id == phones[0].id && matches!(event, Event::Gone(_)), "{id} {event:?}");
        let mut alert = false;
        while let Ok((n, _)) = phones[0].socket.recv_from(&mut buf) {
            alert |= buf[0] == 21 && n > 0;
        }
        assert!(alert, "the phone hears the close_notify alert");
        drop(calls);
        let (id, event) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(id == phones[1].id && matches!(event, Event::Gone(_)), "dropping the calls hangs up the rest");
    }

    /// Junk touches no call: nothing becomes due before its time. A phone
    /// heard from many addresses is routed by the last few only.
    #[test]
    fn junk_and_many_addresses_cost_a_call_nothing() {
        let (tx, _rx) = mpsc::channel();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut worker = Worker { socket, events: tx, calls: HashMap::new(), by_ufrag: HashMap::new(), by_addr: HashMap::new() };
        let cert = Certificate::generate().unwrap();
        let here: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut phone = Browser::new("127.0.0.1:50000");
        worker.add(1, Peer::new(&phone.offer(), &cert, &[here], 0.0).unwrap(), 0.0);
        let answer = worker.calls[&1].peer.answer().to_string();
        let wake = worker.calls[&1].wake;
        assert!(wake > 0.5 && worker.due(0.5).is_empty(), "a new call's first timer is its report ({wake})");
        let stranger: SocketAddr = "127.0.0.1:50999".parse().unwrap();
        for junk in [&[22u8, 0xfe, 0xfd, 0, 0][..], &[0x80, 102, 0, 1], &[0, 1, 0, 0], &[200; 40]] {
            worker.datagram(junk, stranger, 0.5);
        }
        assert!(worker.due(0.5).is_empty() && worker.calls[&1].wake == wake, "junk makes no call due");

        worker.datagram(&phone.check(&answer, true), phone.addr, 0.6);
        for port in 0..10 {
            worker.datagram(&phone.check(&answer, false), SocketAddr::new(phone.addr.ip(), 51_000 + port), 0.7);
            assert!(worker.by_addr.len() <= crate::peer::MAX_CHECKED, "{} addresses routed", worker.by_addr.len());
        }
        assert_eq!(worker.by_addr.get(&phone.addr), Some(&1), "the one we send to stays routed");
    }
}
