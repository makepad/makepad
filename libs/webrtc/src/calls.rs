//! Every call on one UDP port. One long-lived worker thread owns the socket
//! and each call's [`Peer`]: it takes datagrams, runs the peers' timers and
//! sends what they give back, and hands their events to the app over a
//! channel. A datagram goes to its call by the USERNAME of a connectivity
//! check (our ufrag in it), and after that by the address that passed one.
//! Anything else is dropped unread.
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
    calls: HashMap<CallId, Peer>,
    by_ufrag: HashMap<String, CallId>,
    by_addr: HashMap<SocketAddr, CallId>,
}

fn run(socket: UdpSocket, commands: Receiver<Command>, events: Sender<(CallId, Event)>, started: Instant) {
    let mut worker = Worker { socket, events, calls: HashMap::new(), by_ufrag: HashMap::new(), by_addr: HashMap::new() };
    let mut buf = vec![0u8; 65_536];
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Add(id, peer)) => {
                    worker.by_ufrag.insert(peer.local_ufrag().to_string(), id);
                    worker.calls.insert(id, *peer);
                }
                Ok(Command::Close(id)) => worker.close(id),
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                    let ids: Vec<CallId> = worker.calls.keys().copied().collect();
                    for id in ids {
                        worker.close(id);
                    }
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let now = started.elapsed().as_secs_f64();
        let ids: Vec<CallId> = worker.calls.keys().copied().collect();
        for id in ids {
            let events = worker.calls.get_mut(&id).map(|peer| peer.poll(now)).unwrap_or_default();
            worker.after(id, events);
        }
        let wake = worker.calls.values().map(|peer| peer.next_wake(now)).fold(f64::INFINITY, f64::min);
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
        let Some(peer) = self.calls.get_mut(&id) else { return };
        let events = peer.receive(datagram, from, now);
        if peer.knows(from) {
            self.by_addr.insert(from, id);
        }
        self.after(id, events);
    }

    /// Sends what the call gave back, hands on its events, and forgets it
    /// once it is gone.
    fn after(&mut self, id: CallId, events: Vec<Event>) {
        let Some(peer) = self.calls.get_mut(&id) else { return };
        for (datagram, to) in peer.transmit() {
            let _ = self.socket.send_to(&datagram, to);
        }
        let gone = peer.is_gone();
        for event in events {
            let _ = self.events.send((id, event));
        }
        if gone {
            self.forget(id);
        }
    }

    fn close(&mut self, id: CallId) {
        let Some(peer) = self.calls.get_mut(&id) else { return };
        peer.close();
        self.after(id, vec![Event::Gone("hung up here".into())]);
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
    use crate::peer::browser::{pictures, Browser};

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

        /// Checks, then the handshake: what comes back is answered until the
        /// phone has its keys.
        fn connect(&mut self, to: SocketAddr) {
            let check = self.browser.check(&self.answer, true);
            self.send(&check, to);
            let mut buf = [0u8; 2048];
            let (n, _) = self.socket.recv_from(&mut buf).expect("the check is answered");
            assert!(stun::is_stun(&buf[..n]), "a STUN answer comes first");
            for d in self.browser.hello(&self.answer) {
                self.send(&d, to);
            }
            let started = Instant::now();
            while !self.browser.connected() && started.elapsed() < Duration::from_secs(3) {
                let Ok((n, _)) = self.socket.recv_from(&mut buf) else { continue };
                if (20..=63).contains(&buf[0]) {
                    for d in self.browser.take(&buf[..n]) {
                        self.send(&d, to);
                    }
                }
            }
            assert!(self.browser.connected(), "the phone has its keys");
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

        // Hanging up here sends close_notify, and the call is gone.
        calls.close(phones[0].id);
        let (id, event) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(id == phones[0].id && matches!(event, Event::Gone(_)), "{id} {event:?}");
        let mut buf = [0u8; 2048];
        let mut alert = false;
        while let Ok((n, _)) = phones[0].socket.recv_from(&mut buf) {
            alert |= buf[0] == 21 && n > 0;
        }
        assert!(alert, "the phone hears the close_notify alert");
        drop(calls);
        let (id, event) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(id == phones[1].id && matches!(event, Event::Gone(_)), "dropping the calls hangs up the rest");
    }
}
