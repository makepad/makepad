//! A phone browser's end of a call, made of this crate's own parts: an
//! offer as a phone makes it, its checks, its DTLS handshake and its video
//! as SRTP. Enough to drive a [`crate::peer::Peer`] or [`crate::calls::Calls`]
//! in tests, here and in the apps that take calls (the `browser` feature).

use crate::dtls::{Certificate, Dtls};
use crate::rtp::{self, AccessUnit};
use crate::sdp;
use crate::srtp::Session;
use crate::stun;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const SSRC: u32 = 0x5eed_0001;

pub struct Browser {
    pub addr: SocketAddr,
    cert: Certificate,
    ufrag: String,
    pwd: String,
    dtls: Option<Dtls>,
    send: Option<Session>,
    hear: Option<Session>,
    seq: u16,
    transaction: u32,
}

/// Our ufrag, password and fingerprint, as the answer gives them.
fn credentials(answer: &str) -> (String, String, [u8; 32]) {
    let answer = sdp::parse_offer(answer);
    let video = &answer.media[0];
    let fingerprint = video.fingerprint.clone().or(answer.session.fingerprint.clone()).expect("the answer's fingerprint");
    (
        video.ice_ufrag.clone().or(answer.session.ice_ufrag.clone()).expect("the answer's ufrag"),
        video.ice_pwd.clone().or(answer.session.ice_pwd.clone()).expect("the answer's password"),
        fingerprint.1.try_into().expect("a SHA-256 fingerprint"),
    )
}

impl Browser {
    pub fn new(addr: &str) -> Browser {
        Browser {
            addr: addr.parse().unwrap(),
            cert: Certificate::generate().unwrap(),
            ufrag: "phOn".into(),
            pwd: "a+phone/password+of+24ch".into(),
            dtls: None,
            send: None,
            hear: None,
            seq: 65_000,
            transaction: 0,
        }
    }

    /// An offer as a phone browser makes it: VP8 first, H.264 too.
    pub fn offer(&self) -> String {
        let fingerprint: Vec<String> = self.cert.fingerprint.iter().map(|b| format!("{b:02X}")).collect();
        [
            "v=0",
            "o=- 4611731400430051336 2 IN IP4 127.0.0.1",
            "s=-",
            "t=0 0",
            "a=group:BUNDLE 0",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 102",
            "c=IN IP4 0.0.0.0",
            &format!("a=ice-ufrag:{}", self.ufrag),
            &format!("a=ice-pwd:{}", self.pwd),
            &format!("a=fingerprint:sha-256 {}", fingerprint.join(":")),
            "a=setup:actpass",
            "a=mid:0",
            "a=sendonly",
            "a=rtcp-mux",
            "a=rtcp-rsize",
            "a=rtpmap:96 VP8/90000",
            "a=rtpmap:102 H264/90000",
            "a=rtcp-fb:102 nack",
            "a=rtcp-fb:102 nack pli",
            "a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f",
            "",
        ]
        .join("\r\n")
    }

    /// A connectivity check signed with the answer's password.
    pub fn check(&mut self, answer: &str, nominate: bool) -> Vec<u8> {
        let (ufrag, pwd, _) = credentials(answer);
        self.transaction = self.transaction.wrapping_add(1);
        let mut transaction = [0x7a; 12];
        transaction[8..].copy_from_slice(&self.transaction.to_be_bytes());
        stun::binding_request(&transaction, &format!("{ufrag}:{}", self.ufrag), &pwd, nominate)
    }

    /// The call over `socket` to `to` (the answer's candidate): a check,
    /// then the handshake, until this end has its keys or `within` passes.
    /// The socket should have a short read timeout.
    pub fn connect(&mut self, socket: &UdpSocket, answer: &str, to: SocketAddr, within: Duration) -> bool {
        let started = Instant::now();
        let mut buf = [0u8; 2048];
        if socket.send_to(&self.check(answer, true), to).is_err() {
            return false;
        }
        let mut hello = false;
        while !self.connected() && started.elapsed() < within {
            let Ok((n, _)) = socket.recv_from(&mut buf) else { continue };
            let replies = match buf[0] {
                // The check's answer: the handshake starts.
                0..=3 if !hello => {
                    hello = true;
                    self.hello(answer)
                }
                20..=63 if hello => self.take(&buf[..n]),
                _ => Vec::new(),
            };
            for d in replies {
                let _ = socket.send_to(&d, to);
            }
        }
        self.connected()
    }

    /// The ClientHello (`a=setup:passive` in the answer makes us the client).
    pub fn hello(&mut self, answer: &str) -> Vec<Vec<u8>> {
        let (_, _, fingerprint) = credentials(answer);
        let mut dtls = Dtls::client(&self.cert, fingerprint).unwrap();
        let out = dtls.transmit();
        self.dtls = Some(dtls);
        out
    }

    /// Takes a DTLS datagram from the call; answers the next flight.
    pub fn take(&mut self, datagram: &[u8]) -> Vec<Vec<u8>> {
        let dtls = self.dtls.as_mut().expect("hello first");
        if let Ok(Some(keys)) = dtls.receive(datagram) {
            self.send = Some(Session::new(&keys.local_key, &keys.local_salt));
            self.hear = Some(Session::new(&keys.remote_key, &keys.remote_salt));
        }
        dtls.transmit()
    }

    pub fn connected(&self) -> bool {
        self.send.is_some()
    }

    /// One picture as SRTP packets.
    pub fn video(&mut self, picture: &AccessUnit) -> Vec<Vec<u8>> {
        let send = self.send.as_mut().expect("connected");
        rtp::packetize(&picture.data, picture.timestamp, &mut self.seq, SSRC, 1200)
            .iter()
            .map(|packet| send.protect_rtp(packet).unwrap())
            .collect()
    }

    /// What the call sent back as RTCP, in the clear.
    pub fn feedback(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        self.hear.as_mut()?.unprotect_rtcp(datagram).ok()
    }

    pub fn hang_up(&mut self) -> Vec<Vec<u8>> {
        let dtls = self.dtls.as_mut().expect("hello first");
        dtls.close();
        dtls.transmit()
    }
}

/// A keyframe, then three pictures that depend on it.
pub fn pictures() -> Vec<AccessUnit> {
    let picture = |nals: &[(u8, usize)], i: u32| {
        let mut data = Vec::new();
        for (k, &(kind, size)) in nals.iter().enumerate() {
            data.extend_from_slice(&[0, 0, 0, 1, 0x60 | kind]);
            data.extend((1..size).map(|j| ((j * 7 + k * 13 + i as usize) % 251 + 2) as u8));
        }
        AccessUnit { data, timestamp: 3000 * i, keyframe: i == 0 }
    };
    let mut out = vec![picture(&[(7, 12), (8, 5), (5, 3000)], 0)];
    out.extend((1..4).map(|i| picture(&[(1, 400 + 100 * i as usize)], i)));
    out
}
