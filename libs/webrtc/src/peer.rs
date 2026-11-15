//! One browser's call, from its offer to its pictures. ICE-lite answers its
//! connectivity checks, DTLS-SRTP gives the keys, SRTP packets become H.264
//! access units, and the RTCP a receiver owes goes back: NACK and PLI as the
//! stream needs them, a receiver report and REMB every second. Sans-IO like
//! its parts: the caller hands it datagrams and the time, and sends what it
//! gives back.
//!
//! Only addresses that passed a connectivity check (signed with the
//! password in our answer) are heard; everything else is dropped unread.

use crate::dtls::{Certificate, Dtls, DtlsError, SrtpKeys};
use crate::rtp::{self, AccessUnit, Feedback, H264Receiver};
use crate::srtp::Session;
use crate::stun::IceLite;
use crate::{rtcp, sdp};
use makepad_network::tls;
use std::net::SocketAddr;

/// RFC 7675 consent: a browser checks every few seconds, so one silent this
/// long has left.
pub const CONSENT_TIMEOUT: f64 = 30.0;
/// Between receiver reports (each with REMB).
pub const REPORT_EVERY: f64 = 1.0;
/// The bandwidth a call is offered unless the app says otherwise: a phone
/// camera at a quality worth redrawing, which Wi-Fi carries easily. The
/// sender stays below it on its own when its packets get lost.
pub const DEFAULT_BITRATE: u64 = 4_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerError {
    Sdp(sdp::SdpError),
    Dtls(DtlsError),
    /// The offer's fingerprint is not SHA-256 (its hash name).
    Fingerprint(String),
    /// The OS gave no random bytes for the ICE credentials.
    Random(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The handshake is done: pictures can come.
    Connected,
    Picture(AccessUnit),
    /// The call is over (why, for a log line). Nothing more comes.
    Gone(String),
}

pub struct Peer {
    answer: String,
    ice: IceLite,
    dtls: Dtls,
    payload_type: u8,
    clock: f64,
    /// Addresses that passed a connectivity check: the only ones heard.
    checked: Vec<SocketAddr>,
    /// Where our packets go: the nominated address, else the last checked.
    remote: Option<SocketAddr>,
    nominated: bool,
    inbound: Option<Session>,
    outbound: Option<Session>,
    video: H264Receiver,
    media_ssrc: Option<u32>,
    ssrc: u32,
    /// What REMB offers the sender, in bits per second.
    pub max_bitrate: u64,
    last_consent: f64,
    next_report: f64,
    /// The last sender report: the middle of its NTP time, when it came.
    sender_report: Option<(u32, f64)>,
    /// Packets received and lost at the last report.
    reported: (u64, u64),
    jitter: f64,
    last_arrival: Option<(f64, u32)>,
    out: Vec<(Vec<u8>, SocketAddr)>,
    gone: bool,
}

const ICE_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

impl Peer {
    /// A call for this offer, answered for `candidates` (our host
    /// addresses with the UDP port the browser should send to).
    pub fn new(offer: &str, cert: &Certificate, candidates: &[SocketAddr], now: f64) -> Result<Peer, PeerError> {
        let offer = sdp::parse_offer(offer);
        let mut random = [0u8; 8 + 24 + 12];
        tls::os_random(&mut random).map_err(|e| PeerError::Random(e.to_string()))?;
        let ice_char = |b: &u8| ICE_CHARS[(b & 63) as usize] as char;
        let ice_ufrag: String = random[..8].iter().map(ice_char).collect();
        let ice_pwd: String = random[8..32].iter().map(ice_char).collect();
        let session_id = u64::from_be_bytes(random[32..40].try_into().unwrap()) >> 1;
        let ssrc = u32::from_be_bytes(random[40..44].try_into().unwrap());
        let local = sdp::Local { ice_ufrag: &ice_ufrag, ice_pwd: &ice_pwd, fingerprint: &cert.fingerprint, candidates, session_id };
        let answer = sdp::answer(&offer, &local).map_err(PeerError::Sdp)?;
        let (hash, bytes) = &answer.remote_fingerprint;
        let remote_fingerprint: [u8; 32] = match (hash.as_str(), bytes.as_slice().try_into()) {
            ("sha-256", Ok(fingerprint)) => fingerprint,
            _ => return Err(PeerError::Fingerprint(hash.clone())),
        };
        let dtls = Dtls::server(cert, remote_fingerprint).map_err(PeerError::Dtls)?;
        Ok(Peer {
            ice: IceLite { local_ufrag: ice_ufrag, local_pwd: ice_pwd, remote_ufrag: answer.remote_ufrag.clone() },
            dtls,
            payload_type: answer.codec.payload_type,
            clock: answer.codec.clock.max(1) as f64,
            answer: answer.sdp,
            checked: Vec::new(),
            remote: None,
            nominated: false,
            inbound: None,
            outbound: None,
            video: H264Receiver::default(),
            media_ssrc: None,
            ssrc,
            max_bitrate: DEFAULT_BITRATE,
            last_consent: now,
            next_report: now + REPORT_EVERY,
            sender_report: None,
            reported: (0, 0),
            jitter: 0.0,
            last_arrival: None,
            out: Vec::new(),
            gone: false,
        })
    }

    /// The SDP answer to send back to the browser.
    pub fn answer(&self) -> &str {
        &self.answer
    }

    /// Ours in every check's USERNAME (`<ours>:<theirs>`): which call a
    /// check belongs to.
    pub fn local_ufrag(&self) -> &str {
        &self.ice.local_ufrag
    }

    /// Whether `from` passed a connectivity check of this call.
    pub fn knows(&self, from: SocketAddr) -> bool {
        self.checked.contains(&from)
    }

    pub fn is_gone(&self) -> bool {
        self.gone
    }

    /// Takes one datagram from `from`, demultiplexed by its first byte
    /// (RFC 7983): STUN, DTLS, or SRTP/SRTCP.
    pub fn receive(&mut self, datagram: &[u8], from: SocketAddr, now: f64) -> Vec<Event> {
        let mut events = Vec::new();
        if self.gone {
            return events;
        }
        match datagram.first() {
            Some(0..=3) => {
                if let Some(check) = self.ice.answer(datagram, from) {
                    self.out.push((check.response, from));
                    self.last_consent = now;
                    if !self.checked.contains(&from) {
                        self.checked.push(from);
                    }
                    if check.nominated || !self.nominated {
                        self.nominated |= check.nominated;
                        self.remote = Some(from);
                    }
                }
            }
            Some(20..=63) if self.knows(from) => {
                let result = self.dtls.receive(datagram);
                self.flush_dtls(from);
                match result {
                    Ok(Some(keys)) => {
                        self.keys(&keys);
                        events.push(Event::Connected);
                    }
                    Ok(None) => {}
                    Err(e) => self.end(format!("DTLS: {e:?}"), &mut events),
                }
            }
            Some(128..=191) if self.knows(from) => self.media(datagram, now, &mut events),
            _ => {}
        }
        events
    }

    /// Timers: a lost DTLS flight, holes given up, keyframe requests, the
    /// receiver report, and consent.
    pub fn poll(&mut self, now: f64) -> Vec<Event> {
        let mut events = Vec::new();
        if self.gone {
            return events;
        }
        if now - self.last_consent > CONSENT_TIMEOUT {
            self.end(format!("no connectivity check for {CONSENT_TIMEOUT} s"), &mut events);
            return events;
        }
        if self.dtls.next_timeout().is_some_and(|left| left.is_zero()) {
            if let Err(e) = self.dtls.timeout() {
                self.end(format!("DTLS: {e:?}"), &mut events);
                return events;
            }
            if let Some(to) = self.remote {
                self.flush_dtls(to);
            }
        }
        events.extend(self.video.poll(now).into_iter().map(Event::Picture));
        self.feedback(now);
        if now >= self.next_report {
            self.next_report = now + REPORT_EVERY;
            self.report(now);
        }
        events
    }

    /// When [`Peer::poll`] next has something to do.
    pub fn next_wake(&self, now: f64) -> f64 {
        let dtls = self.dtls.next_timeout().map(|left| now + left.as_secs_f64());
        [Some(self.next_report), Some(self.last_consent + CONSENT_TIMEOUT), dtls, self.video.next_deadline()]
            .into_iter()
            .flatten()
            .fold(f64::INFINITY, f64::min)
    }

    /// The datagrams to send, with where each goes.
    pub fn transmit(&mut self) -> Vec<(Vec<u8>, SocketAddr)> {
        std::mem::take(&mut self.out)
    }

    /// Hangs up: a DTLS close_notify for [`Peer::transmit`].
    pub fn close(&mut self) {
        if self.gone {
            return;
        }
        self.dtls.close();
        if let Some(to) = self.remote {
            self.flush_dtls(to);
        }
        self.gone = true;
    }

    fn end(&mut self, why: String, events: &mut Vec<Event>) {
        self.gone = true;
        events.push(Event::Gone(why));
    }

    fn flush_dtls(&mut self, to: SocketAddr) {
        for datagram in self.dtls.transmit() {
            self.out.push((datagram, to));
        }
    }

    fn keys(&mut self, keys: &SrtpKeys) {
        self.inbound = Some(Session::new(&keys.remote_key, &keys.remote_salt));
        self.outbound = Some(Session::new(&keys.local_key, &keys.local_salt));
    }

    fn media(&mut self, packet: &[u8], now: f64, events: &mut Vec<Event>) {
        let Some(inbound) = self.inbound.as_mut() else { return };
        if rtp::is_rtcp(packet) {
            if let Ok(plain) = inbound.unprotect_rtcp(packet) {
                for (ssrc, ntp_middle) in rtcp::sender_reports(&plain) {
                    if Some(ssrc) == self.media_ssrc {
                        self.sender_report = Some((ntp_middle, now));
                    }
                }
            }
            return;
        }
        let Ok(plain) = inbound.unprotect_rtp(packet) else { return };
        let Some(header) = rtp::parse(&plain) else { return };
        // One stream: the first SSRC that sends the answered format.
        if header.payload_type != self.payload_type || *self.media_ssrc.get_or_insert(header.ssrc) != header.ssrc {
            return;
        }
        // RFC 3550 A.8 interarrival jitter, in timestamp units.
        if let Some((at, timestamp)) = self.last_arrival.replace((now, header.timestamp)) {
            let d = (now - at) * self.clock - header.timestamp.wrapping_sub(timestamp) as i32 as f64;
            self.jitter += (d.abs() - self.jitter) / 16.0;
        }
        events.extend(self.video.push(&plain, now).into_iter().map(Event::Picture));
        self.feedback(now);
    }

    fn feedback(&mut self, now: f64) {
        let Some(media) = self.media_ssrc else { return };
        let mut compound = Vec::new();
        for feedback in self.video.feedback(now) {
            compound.extend(match feedback {
                Feedback::Nack(lost) => rtcp::nack(self.ssrc, media, &lost),
                Feedback::Pli => rtcp::pli(self.ssrc, media),
            });
        }
        if !compound.is_empty() {
            self.send_rtcp(&compound);
        }
    }

    fn report(&mut self, now: f64) {
        let Some(media) = self.media_ssrc else { return };
        let (received, lost) = (self.video.received, self.video.lost);
        let (got, missed) = (received - self.reported.0, lost - self.reported.1);
        self.reported = (received, lost);
        let (last_sr, delay_since_sr) = self.sender_report.map_or((0, 0), |(ntp, at)| (ntp, ((now - at) * 65536.0) as u32));
        let report = rtcp::Report {
            source: media,
            fraction_lost: if got + missed == 0 { 0 } else { (missed * 256 / (got + missed)).min(255) as u8 },
            cumulative_lost: lost.min(u32::MAX as u64) as u32,
            // The receiver counts its first cycle as one.
            highest_seq: self.video.highest().map_or(0, |h| h.saturating_sub(1 << 16) as u32),
            jitter: self.jitter as u32,
            last_sr,
            delay_since_sr,
        };
        let mut compound = rtcp::receiver_report(self.ssrc, &report);
        compound.extend(rtcp::remb(self.ssrc, self.max_bitrate, &[media]));
        self.send_rtcp(&compound);
    }

    fn send_rtcp(&mut self, plain: &[u8]) {
        let (Some(outbound), Some(to)) = (self.outbound.as_mut(), self.remote) else { return };
        if let Ok(packet) = outbound.protect_rtcp(plain) {
            self.out.push((packet, to));
        }
    }
}

/// A phone's end of a call, made of our own parts: enough to drive a
/// [`Peer`] (or a server of them) in tests.
#[cfg(test)]
pub(crate) mod browser {
    use super::*;
    use crate::stun;

    pub(crate) const SSRC: u32 = 0x5eed_0001;

    pub(crate) struct Browser {
        pub addr: SocketAddr,
        cert: Certificate,
        ufrag: String,
        pwd: String,
        dtls: Option<Dtls>,
        send: Option<Session>,
        hear: Option<Session>,
        seq: u16,
        transaction: u8,
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
            self.transaction += 1;
            stun::binding_request(&[self.transaction; 12], &format!("{ufrag}:{}", self.ufrag), &pwd, nominate)
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
    pub(crate) fn pictures() -> Vec<AccessUnit> {
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
}

#[cfg(test)]
mod tests {
    use super::browser::{pictures, Browser, SSRC};
    use super::*;
    use crate::stun;

    /// RTCP packet types (and FMT) in a compound packet.
    fn kinds(compound: &[u8]) -> Vec<(u8, u8)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 4 <= compound.len() {
            out.push((compound[at + 1], compound[at] & 0x1f));
            at += (u16::from_be_bytes([compound[at + 2], compound[at + 3]]) as usize + 1) * 4;
        }
        out
    }

    #[test]
    fn a_phone_call_becomes_pictures_and_feedback() {
        let cert = Certificate::generate().unwrap();
        let here: SocketAddr = "192.0.2.1:40000".parse().unwrap();
        let stranger: SocketAddr = "192.0.2.99:40000".parse().unwrap();
        let mut phone = Browser::new("192.0.2.20:50000");
        let mut peer = Peer::new(&phone.offer(), &cert, &[here], 0.0).unwrap();
        let answer = peer.answer().to_string();
        assert!(answer.contains("a=ice-lite") && answer.contains("192.0.2.1 40000"), "an ICE-lite answer with our candidate");

        // Nothing is heard from an address before it passes a check.
        for d in phone.hello(&answer) {
            assert!(peer.receive(&d, stranger, 0.0).is_empty());
        }
        let mut wrong = Browser::new("192.0.2.99:40000");
        let forged = wrong.check(&answer.replace(&peer.ice.local_pwd, "not+the+password+of+24ch"), true);
        peer.receive(&forged, stranger, 0.0);
        assert!(peer.transmit().is_empty() && !peer.knows(stranger), "an unchecked address and a forged check get nothing");

        // The check: answered to where it came from, signed with our password.
        let check = phone.check(&answer, true);
        peer.receive(&check, phone.addr, 0.01);
        let out = peer.transmit();
        assert_eq!(out.len(), 1, "one answer to one check");
        assert_eq!(out[0].1, phone.addr);
        assert!(stun::check_integrity(&out[0].0, peer.ice.local_pwd.as_bytes()), "the answer is signed");

        // The handshake.
        let mut to_peer = phone.hello(&answer);
        let mut connected = false;
        for _ in 0..10 {
            for d in std::mem::take(&mut to_peer) {
                connected |= peer.receive(&d, phone.addr, 0.02).contains(&Event::Connected);
            }
            for (d, to) in peer.transmit() {
                assert_eq!(to, phone.addr, "DTLS goes back to the phone");
                to_peer.extend(phone.take(&d));
            }
            if connected && phone.connected() && to_peer.is_empty() {
                break;
            }
        }
        assert!(connected && phone.connected(), "both ends have their keys");

        // Pictures, with the second P picture's packet lost once: a NACK at
        // once, the resent packet completes the pictures in order.
        let sent = pictures();
        let mut packets: Vec<Vec<u8>> = sent.iter().flat_map(|p| phone.video(p)).collect();
        let lost = packets.len() - 2;
        let resend = packets.remove(lost);
        let mut got = Vec::new();
        let mut now = 0.1;
        for p in &packets {
            now += 0.005;
            got.extend(peer.receive(p, phone.addr, now));
        }
        let feedback: Vec<Vec<u8>> = peer.transmit().iter().filter_map(|(d, _)| phone.feedback(d)).collect();
        assert_eq!(feedback.len(), 1, "one feedback packet, and no keyframe asked for: the stream began with one");
        assert_eq!(kinds(&feedback[0]), vec![(205, 1)], "a generic NACK");
        assert_eq!(u32::from_be_bytes(feedback[0][8..12].try_into().unwrap()), SSRC, "about the phone's stream");
        got.extend(peer.receive(&resend, phone.addr, now + 0.01));
        assert_eq!(got, sent.into_iter().map(Event::Picture).collect::<Vec<_>>(), "every picture, in order");

        // Each second: a receiver report with REMB.
        assert!(peer.next_wake(now) <= REPORT_EVERY + 1e-9, "the report is the next thing due");
        assert!(peer.poll(1.2).is_empty());
        let report: Vec<Vec<u8>> = peer.transmit().iter().filter_map(|(d, _)| phone.feedback(d)).collect();
        assert_eq!(report.len(), 1);
        assert_eq!(kinds(&report[0]), vec![(201, 1), (206, 15)], "RR then REMB");
        assert_eq!(report[0][12], 0, "no loss this interval: the NACKed packet came");

        // Hanging up ends the call, and so does silence.
        for d in phone.hang_up() {
            let events = peer.receive(&d, phone.addr, 1.3);
            assert!(matches!(events.as_slice(), [Event::Gone(_)]), "close_notify ends the call: {events:?}");
        }
        assert!(peer.is_gone());
        let mut quiet = Peer::new(&Browser::new("192.0.2.21:50000").offer(), &cert, &[here], 0.0).unwrap();
        assert!(quiet.poll(CONSENT_TIMEOUT - 1.0).is_empty());
        assert!(matches!(quiet.poll(CONSENT_TIMEOUT + 1.0).as_slice(), [Event::Gone(_)]), "consent runs out");

        // An offer we cannot take.
        let vp8_only = phone.offer().replace(" 96 102", " 96").replace("a=rtpmap:102 H264/90000", "");
        assert_eq!(Peer::new(&vp8_only, &cert, &[here], 0.0).err(), Some(PeerError::Sdp(sdp::SdpError::NoH264)));
    }
}
