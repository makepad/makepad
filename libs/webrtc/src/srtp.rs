//! SRTP and SRTCP with AES-128-CM and HMAC-SHA1-80 from RFC 3711.
//! Keys have a derivation rate of zero. Packets carry no master key identifier.

use crate::crypto::{hmac_sha1, AesCtr};
use std::collections::HashMap;

const TAG_LEN: usize = 10;
const SRTCP_INDEX_MAX: u32 = 0x7fff_ffff;
// A packet must not consume the counter bits reserved for another packet.
const MAX_CIPHER_BYTES: usize = 16 * (1 << 16);

/// Why a packet could not be protected or accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SrtpError {
    /// The packet is truncated.
    Short,
    /// Authentication failed.
    BadTag,
    /// The packet was already accepted or is too old.
    Replayed,
    /// The packet is invalid or the sending index is exhausted.
    Malformed,
}

/// One direction's keys and per-SSRC state.
pub struct Session {
    rtp: Keys,
    rtcp: Keys,
    rtp_recv: HashMap<u32, RtpReceiver>,
    rtp_send: HashMap<u32, (u32, u16)>,
    rtcp_recv: HashMap<u32, ReplayWindow>,
    rtcp_send: HashMap<u32, u32>,
}

struct Keys {
    cipher: AesCtr,
    auth: [u8; 20],
    salt: [u8; 14],
}

impl Keys {
    fn new(master_key: &[u8; 16], master_salt: &[u8; 14], label: u8) -> Self {
        let mut key = [0; 16];
        let mut auth = [0; 20];
        let mut salt = [0; 14];
        key.copy_from_slice(&derive(master_key, master_salt, label, 16));
        auth.copy_from_slice(&derive(master_key, master_salt, label + 1, 20));
        salt.copy_from_slice(&derive(master_key, master_salt, label + 2, 14));
        Self { cipher: AesCtr::new(&key), auth, salt }
    }
}

/// Packets behind the newest that may still come (RFC 3711 allows any
/// window of at least 64). A NACKed packet is resent after the rest of its
/// picture: a keyframe at our bitrate is a few hundred packets, and the
/// resend's round trip a few dozen more.
pub const REPLAY_WINDOW: u64 = 1024;

/// Which of the last [`REPLAY_WINDOW`] indices came: index `i` is bit `i % REPLAY_WINDOW`.
#[derive(Default)]
struct ReplayWindow {
    highest: Option<u64>,
    seen: [u64; (REPLAY_WINDOW / 64) as usize],
}

impl ReplayWindow {
    fn bit(index: u64) -> (usize, u64) {
        let at = index % REPLAY_WINDOW;
        ((at / 64) as usize, 1u64 << (at % 64))
    }

    fn check(&self, index: u64) -> Result<(), SrtpError> {
        let Some(highest) = self.highest else { return Ok(()) };
        if index <= highest {
            let (word, bit) = Self::bit(index);
            if highest - index >= REPLAY_WINDOW || self.seen[word] & bit != 0 {
                return Err(SrtpError::Replayed);
            }
        }
        Ok(())
    }

    // Call only after check and authentication have succeeded.
    fn accept(&mut self, index: u64) {
        match self.highest {
            Some(highest) if index <= highest => {}
            Some(highest) if index - highest < REPLAY_WINDOW => {
                // The indices passed over have not come (yet).
                for skipped in highest + 1..index {
                    let (word, bit) = Self::bit(skipped);
                    self.seen[word] &= !bit;
                }
                self.highest = Some(index);
            }
            _ => {
                self.seen = Default::default();
                self.highest = Some(index);
            }
        }
        let (word, bit) = Self::bit(index);
        self.seen[word] |= bit;
    }
}

struct RtpReceiver {
    roc: u32,
    sequence: u16,
    replay: ReplayWindow,
}

impl RtpReceiver {
    fn estimate_roc(&self, sequence: u16) -> u32 {
        // Appendix A uses signed differences and modulo-2^32 ROC arithmetic.
        let delta = i32::from(sequence) - i32::from(self.sequence);
        if self.sequence < 0x8000 && delta > 0x8000 {
            self.roc.wrapping_sub(1)
        } else if self.sequence >= 0x8000 && delta < -0x8000 {
            self.roc.wrapping_add(1)
        } else {
            self.roc
        }
    }

    fn accept(&mut self, roc: u32, sequence: u16, index: u64) {
        self.replay.accept(index);
        if roc == self.roc {
            self.sequence = self.sequence.max(sequence);
        } else if roc == self.roc.wrapping_add(1) {
            self.roc = roc;
            self.sequence = sequence;
        }
    }
}

impl Session {
    /// Derives separate SRTP and SRTCP keys for one direction.
    pub fn new(master_key: &[u8; 16], master_salt: &[u8; 14]) -> Session {
        Session {
            rtp: Keys::new(master_key, master_salt, 0),
            rtcp: Keys::new(master_key, master_salt, 3),
            rtp_recv: HashMap::new(),
            rtp_send: HashMap::new(),
            rtcp_recv: HashMap::new(),
            rtcp_send: HashMap::new(),
        }
    }

    /// The plain RTP packet, or why it was refused.
    pub fn unprotect_rtp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        if packet.len() < 12 + TAG_LEN {
            return Err(SrtpError::Short);
        }
        let (authenticated, tag) = packet.split_at(packet.len() - TAG_LEN);
        let (header_len, sequence, ssrc) = rtp_header(authenticated)?;
        let state = self.rtp_recv.get(&ssrc);
        let roc = state.map_or(0, |state| state.estimate_roc(sequence));
        let index = (u64::from(roc) << 16) | u64::from(sequence);
        let expected = hmac_sha1(&self.rtp.auth, &[authenticated, &roc.to_be_bytes()]);
        check_tag(tag, &expected)?;
        if let Some(state) = state {
            state.replay.check(index)?;
        }
        let mut plain = authenticated.to_vec();
        self.rtp.cipher.apply(&iv(&self.rtp.salt, ssrc, index), &mut plain[header_len..]);
        rtp_padding(&plain, header_len)?;
        self.rtp_recv
            .entry(ssrc)
            .or_insert_with(|| RtpReceiver { roc: 0, sequence, replay: ReplayWindow::default() })
            .accept(roc, sequence, index);
        Ok(plain)
    }

    /// The sender's side (for the loopback tests, and for anything we might send).
    /// Supply packets in sequence order, allowing gaps.
    pub fn protect_rtp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        let (header_len, sequence, ssrc) = rtp_header(packet)?;
        rtp_padding(packet, header_len)?;
        let roc = match self.rtp_send.get(&ssrc) {
            Some(&(roc, last)) if sequence < last => roc.checked_add(1).ok_or(SrtpError::Malformed)?,
            Some(&(roc, _)) => roc,
            None => 0,
        };
        let index = (u64::from(roc) << 16) | u64::from(sequence);
        let mut protected = packet.to_vec();
        self.rtp.cipher.apply(&iv(&self.rtp.salt, ssrc, index), &mut protected[header_len..]);
        let tag = hmac_sha1(&self.rtp.auth, &[&protected, &roc.to_be_bytes()]);
        protected.extend_from_slice(&tag[..TAG_LEN]);
        self.rtp_send.insert(ssrc, (roc, sequence));
        Ok(protected)
    }

    /// Authenticates SRTCP and decrypts it when the E flag is set.
    pub fn unprotect_rtcp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        if packet.len() < 8 + 4 + TAG_LEN {
            return Err(SrtpError::Short);
        }
        let (authenticated, tag) = packet.split_at(packet.len() - TAG_LEN);
        let body_len = authenticated.len() - 4;
        let ssrc = rtcp_ssrc(&authenticated[..body_len])?;
        let word = read_u32(&authenticated[body_len..]);
        let index = word & SRTCP_INDEX_MAX;
        let expected = hmac_sha1(&self.rtcp.auth, &[authenticated]);
        check_tag(tag, &expected)?;
        if let Some(window) = self.rtcp_recv.get(&ssrc) {
            window.check(u64::from(index))?;
        }
        let mut plain = authenticated[..body_len].to_vec();
        if word & 0x8000_0000 != 0 {
            self.rtcp.cipher.apply(&iv(&self.rtcp.salt, ssrc, u64::from(index)), &mut plain[8..]);
        }
        self.rtcp_recv.entry(ssrc).or_default().accept(u64::from(index));
        Ok(plain)
    }

    /// Encrypts RTCP with an index starting at one for each SSRC.
    pub fn protect_rtcp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        let ssrc = rtcp_ssrc(packet)?;
        let last = self.rtcp_send.get(&ssrc).copied().unwrap_or(0);
        if last == SRTCP_INDEX_MAX {
            return Err(SrtpError::Malformed);
        }
        let index = last + 1;
        let mut protected = packet.to_vec();
        self.rtcp.cipher.apply(&iv(&self.rtcp.salt, ssrc, u64::from(index)), &mut protected[8..]);
        protected.extend_from_slice(&(0x8000_0000 | index).to_be_bytes());
        let tag = hmac_sha1(&self.rtcp.auth, &[&protected]);
        protected.extend_from_slice(&tag[..TAG_LEN]);
        self.rtcp_send.insert(ssrc, index);
        Ok(protected)
    }
}

/// RFC 3711 4.3 key derivation (rate 0): `len` bytes for `label`.
pub fn derive(master_key: &[u8; 16], master_salt: &[u8; 14], label: u8, len: usize) -> Vec<u8> {
    let mut counter = [0; 16];
    counter[..14].copy_from_slice(master_salt);
    counter[7] ^= label;
    AesCtr::new(master_key).keystream(&counter, len)
}

/// RFC 3711 4.1.1 IV.
pub fn iv(salt: &[u8; 14], ssrc: u32, index: u64) -> [u8; 16] {
    let mut counter = [0; 16];
    counter[..14].copy_from_slice(salt);
    for (byte, value) in counter[4..8].iter_mut().zip(ssrc.to_be_bytes()) {
        *byte ^= value;
    }
    for (byte, value) in counter[8..14].iter_mut().zip(&index.to_be_bytes()[2..]) {
        *byte ^= value;
    }
    counter
}

fn check_tag(tag: &[u8], expected: &[u8; 20]) -> Result<(), SrtpError> {
    if tag.len() != TAG_LEN {
        return Err(SrtpError::Short);
    }
    // Always compare all ten bytes, with no data-dependent exit.
    let mut difference = 0u8;
    for i in 0..TAG_LEN {
        difference |= tag[i] ^ expected[i];
    }
    if difference == 0 { Ok(()) } else { Err(SrtpError::BadTag) }
}

// Callers first check that four bytes are available.
fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn rtp_header(packet: &[u8]) -> Result<(usize, u16, u32), SrtpError> {
    if packet.len() < 12 {
        return Err(SrtpError::Short);
    }
    if packet[0] >> 6 != 2 {
        return Err(SrtpError::Malformed);
    }
    let mut len = 12 + 4 * usize::from(packet[0] & 0x0f);
    if packet.len() < len {
        return Err(SrtpError::Short);
    }
    if packet[0] & 0x10 != 0 {
        let extension = packet.get(len..len + 4).ok_or(SrtpError::Short)?;
        len += 4 + 4 * usize::from(u16::from_be_bytes([extension[2], extension[3]]));
        if packet.len() < len {
            return Err(SrtpError::Short);
        }
    }
    if packet.len() - len > MAX_CIPHER_BYTES {
        return Err(SrtpError::Malformed);
    }
    Ok((len, u16::from_be_bytes([packet[2], packet[3]]), read_u32(&packet[8..12])))
}

fn rtp_padding(packet: &[u8], header_len: usize) -> Result<(), SrtpError> {
    if packet[0] & 0x20 != 0 {
        let count = usize::from(packet[packet.len() - 1]);
        if count == 0 || count > packet.len() - header_len {
            return Err(SrtpError::Malformed);
        }
    }
    Ok(())
}

fn rtcp_ssrc(packet: &[u8]) -> Result<u32, SrtpError> {
    if packet.len() < 8 {
        return Err(SrtpError::Short);
    }
    if packet[0] >> 6 != 2 || packet.len() % 4 != 0 || packet.len() - 8 > MAX_CIPHER_BYTES {
        return Err(SrtpError::Malformed);
    }
    let first_len = 4 * (usize::from(u16::from_be_bytes([packet[2], packet[3]])) + 1);
    if first_len < 8 {
        return Err(SrtpError::Malformed);
    }
    if first_len > packet.len() {
        return Err(SrtpError::Short);
    }
    Ok(read_u32(&packet[4..8]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::hex;

    fn master_keys() -> ([u8; 16], [u8; 14]) {
        (
            hex("E1F97A0D3E018BE0D64FA32C06DE4139").try_into().unwrap(),
            hex("0EC675AD498AFEEBB6960B3AABE6").try_into().unwrap(),
        )
    }

    fn rtp(sequence: u16, ssrc: u32, extended: bool) -> Vec<u8> {
        let mut packet = vec![if extended { 0xb2 } else { 0x80 }, 96];
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(&1234u32.to_be_bytes());
        packet.extend_from_slice(&ssrc.to_be_bytes());
        if extended {
            packet.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2]);
            packet.extend_from_slice(&[0xbe, 0xde, 0, 1, 0x12, 3, 4, 5]);
        }
        packet.extend_from_slice(b"RTP payload");
        if extended {
            packet.extend_from_slice(&[0, 0, 0, 4]);
        }
        packet
    }

    fn rtcp(ssrc: u32) -> Vec<u8> {
        // A reduced-size RTCP PLI, followed by an empty receiver report.
        let mut packet = vec![0x81, 206, 0, 2];
        packet.extend_from_slice(&ssrc.to_be_bytes());
        packet.extend_from_slice(&42u32.to_be_bytes());
        packet.extend_from_slice(&[0x80, 201, 0, 1]);
        packet.extend_from_slice(&ssrc.to_be_bytes());
        packet
    }

    #[test]
    fn rfc3711_keystream_and_key_derivation() {
        let key = hex("2B7E151628AED2A6ABF7158809CF4F3C").try_into().unwrap();
        let salt = hex("F0F1F2F3F4F5F6F7F8F9FAFBFCFD").try_into().unwrap();
        let counter = iv(&salt, 0, 0);
        assert_eq!(counter.as_slice(), hex("F0F1F2F3F4F5F6F7F8F9FAFBFCFD0000"), "B.2 IV");
        let mut cipher = AesCtr::new(&key);
        for (block, expected) in [
            (0u16, "E03EAD0935C95E80E166B16DD92B4EB4 D23513162B02D0F72A43A2FE4A5F97AB 41E95B3BB0A2E8DD477901E4FCA894C0"),
            (0xfeffu16, "EC8CDF7398607CB0F2D21675EA9EA1E4 362B7C3C6773516318A077D7FC5073AE 6A2CC3787889374FBEB4C81B17BA6C44"),
        ] {
            let mut start = counter;
            start[14..].copy_from_slice(&block.to_be_bytes());
            assert_eq!(cipher.keystream(&start, 48), hex(expected), "B.2 three blocks starting at {block:#06x}");
        }
        let (key, salt) = master_keys();
        for (label, len, expected) in [
            (0, 16, "C61E7A93744F39EE10734AFE3FF7A087"),
            (2, 14, "30CBBC08863D8C85D49DB34A9AE1"),
            (1, 20, "CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4"),
        ] {
            assert_eq!(derive(&key, &salt, label, len), hex(expected), "B.3 derivation label {label}");
        }
    }

    #[test]
    fn rtp_roundtrips_authentication_and_headers() {
        let (key, salt) = master_keys();
        let mut sender = Session::new(&key, &salt);
        let mut receiver = Session::new(&key, &salt);
        for (sequence, extended) in [(7, false), (8, true)] {
            let plain = rtp(sequence, 42, extended);
            let wire = sender.protect_rtp(&plain).expect("protect valid RTP");
            let header_len = if extended { 28 } else { 12 };
            assert_eq!(wire.len(), plain.len() + TAG_LEN, "RTP adds only its tag");
            assert_eq!(wire[..header_len], plain[..header_len], "CSRCs and extension stay clear");
            assert_ne!(wire[header_len..plain.len()], plain[header_len..], "RTP payload and padding are encrypted");
            for offset in [2, header_len, wire.len() - 1] {
                let mut corrupt = wire.clone();
                corrupt[offset] ^= 1;
                assert_eq!(receiver.unprotect_rtp(&corrupt), Err(SrtpError::BadTag), "RTP tamper at byte {offset}");
            }
            assert_eq!(receiver.unprotect_rtp(&wire), Ok(plain), "RTP round trip after rejected forgeries");
            assert_eq!(receiver.unprotect_rtp(&wire), Err(SrtpError::Replayed), "RTP duplicate");
        }
        let plain = rtp(7, 43, false);
        let wire = sender.protect_rtp(&plain).expect("protect second SSRC");
        assert_eq!(receiver.unprotect_rtp(&wire), Ok(plain), "RTP replay state belongs to the SSRC");

        for len in 0..12 {
            assert_eq!(sender.protect_rtp(&vec![0; len]), Err(SrtpError::Short), "short RTP header of {len} bytes");
        }
        for len in 0..22 {
            assert_eq!(receiver.unprotect_rtp(&vec![0; len]), Err(SrtpError::Short), "short SRTP packet of {len} bytes");
        }
        let extended = rtp(9, 42, true);
        for len in 12..28 {
            assert_eq!(sender.protect_rtp(&extended[..len]), Err(SrtpError::Short), "truncated CSRC or extension at {len}");
        }
        let mut malformed = extended;
        malformed[0] = 0x40;
        assert_eq!(sender.protect_rtp(&malformed), Err(SrtpError::Malformed), "invalid RTP version");
        malformed[0] = 0xb2;
        *malformed.last_mut().unwrap() = 0;
        assert_eq!(sender.protect_rtp(&malformed), Err(SrtpError::Malformed), "zero RTP padding count");
        *malformed.last_mut().unwrap() = 255;
        assert_eq!(sender.protect_rtp(&malformed), Err(SrtpError::Malformed), "padding cannot consume the RTP header");
    }

    #[test]
    fn rtp_rollover_reordering_and_window_edges() {
        let (key, salt) = master_keys();
        let mut sender = Session::new(&key, &salt);
        let mut receiver = Session::new(&key, &salt);
        let sequences = [65534, 65535, 0, 1, 2, 3, 4, 1025, 1026, 2050];
        let packets: Vec<_> = sequences.iter().map(|&seq| rtp(seq, 42, false)).collect();
        let wires: Vec<_> = packets.iter().map(|packet| sender.protect_rtp(packet).expect("protect across wrap")).collect();
        let mut ordered = Session::new(&key, &salt);
        for i in 0..4 {
            assert_eq!(ordered.unprotect_rtp(&wires[i]), Ok(packets[i].clone()), "ordered wrap at SEQ {}", sequences[i]);
        }
        assert_eq!(receiver.unprotect_rtp(&wires[0]), Ok(packets[0].clone()), "initial sequence near wrap");
        let mut forged = wires[2].clone();
        *forged.last_mut().unwrap() ^= 1;
        assert_eq!(receiver.unprotect_rtp(&forged), Err(SrtpError::BadTag), "forged rollover is refused");
        for i in [2, 1, 3, 7, 4, 6, 8] {
            assert_eq!(receiver.unprotect_rtp(&wires[i]), Ok(packets[i].clone()), "reordered SEQ {}", sequences[i]);
        }
        assert_eq!(receiver.unprotect_rtp(&wires[4]), Err(SrtpError::Replayed), "accepted packet ages out at the window's width");
        assert_eq!(receiver.unprotect_rtp(&wires[5]), Ok(packets[5].clone()), "unseen packet just inside the window: a resend after a keyframe's worth");
        assert_eq!(receiver.unprotect_rtp(&wires[5]), Err(SrtpError::Replayed), "reordered packet is accepted once");
        assert_eq!(receiver.unprotect_rtp(&wires[9]), Ok(packets[9].clone()), "a jump of the window's width resets it");
        assert_eq!(receiver.unprotect_rtp(&wires[8]), Err(SrtpError::Replayed), "packet before the reset is too old");

        let fresh = rtp(65535, 43, false);
        let wire = sender.protect_rtp(&fresh).expect("protect independent SSRC near wrap");
        assert_eq!(receiver.unprotect_rtp(&wire), Ok(fresh), "ROC is independent for each SSRC");
        assert_eq!(sender.unprotect_rtp(&wires[0]), Ok(packets[0].clone()), "sender and receiver ROCs are separate");
        sender.rtp_send.insert(44, (u32::MAX, 65535));
        assert_eq!(sender.protect_rtp(&rtp(0, 44, false)), Err(SrtpError::Malformed), "RTP refuses key exhaustion");
    }

    #[test]
    fn rtcp_roundtrips_authentication_and_replay() {
        let (key, salt) = master_keys();
        let mut sender = Session::new(&key, &salt);
        let mut receiver = Session::new(&key, &salt);
        let plain = rtcp(42);
        let wire = sender.protect_rtcp(&plain).expect("protect RTCP");
        assert_eq!(wire.len(), plain.len() + 4 + TAG_LEN, "SRTCP trailer length");
        assert_eq!(wire[..8], plain[..8], "SRTCP clear header");
        assert_ne!(wire[8..plain.len()], plain[8..], "SRTCP encrypts all remaining compound RTCP");
        assert_eq!(read_u32(&wire[plain.len()..]), 0x8000_0001, "first SRTCP index is one with E set");
        for offset in [4, 8, plain.len(), plain.len() + 3, wire.len() - 1] {
            let mut corrupt = wire.clone();
            corrupt[offset] ^= if offset == plain.len() { 0x80 } else { 1 };
            assert_eq!(receiver.unprotect_rtcp(&corrupt), Err(SrtpError::BadTag), "SRTCP tamper at byte {offset}");
        }
        assert_eq!(receiver.unprotect_rtcp(&wire), Ok(plain.clone()), "SRTCP round trip after rejected forgeries");
        assert_eq!(receiver.unprotect_rtcp(&wire), Err(SrtpError::Replayed), "SRTCP duplicate");
        let second = sender.protect_rtcp(&plain).expect("protect second SRTCP packet");
        let third = sender.protect_rtcp(&plain).expect("protect third SRTCP packet");
        assert_eq!(read_u32(&second[plain.len()..]), 0x8000_0002, "SRTCP index increases");
        assert_eq!(receiver.unprotect_rtcp(&third), Ok(plain.clone()), "SRTCP ahead of missing packet");
        assert_eq!(receiver.unprotect_rtcp(&second), Ok(plain.clone()), "SRTCP out of order within window");
        assert_eq!(receiver.unprotect_rtcp(&second), Err(SrtpError::Replayed), "SRTCP reordered duplicate");
        let other = rtcp(43);
        let other_wire = sender.protect_rtcp(&other).expect("protect second SRTCP SSRC");
        assert_eq!(read_u32(&other_wire[other.len()..]), 0x8000_0001, "SRTCP sender index belongs to the SSRC");
        assert_eq!(receiver.unprotect_rtcp(&other_wire), Ok(other), "SRTCP replay state belongs to the SSRC");

        // E=0 still requires authentication over the explicit index.
        let mut clear = plain.clone();
        clear.extend_from_slice(&4u32.to_be_bytes());
        let auth = derive(&key, &salt, 4, 20);
        clear.extend_from_slice(&hmac_sha1(&auth, &[&clear])[..TAG_LEN]);
        assert_eq!(receiver.unprotect_rtcp(&clear), Ok(plain.clone()), "authenticated SRTCP with E clear");
        assert_eq!(receiver.unprotect_rtcp(&clear), Err(SrtpError::Replayed), "E=0 packets share the replay window");
        sender.rtcp_send.insert(42, 3 + REPLAY_WINDOW as u32);
        let ahead = sender.protect_rtcp(&plain).expect("protect SRTCP at window boundary");
        assert_eq!(receiver.unprotect_rtcp(&ahead), Ok(plain.clone()), "SRTCP window advances by its width");
        assert_eq!(receiver.unprotect_rtcp(&clear), Err(SrtpError::Replayed), "old SRTCP index is refused");
        sender.rtcp_send.insert(42, SRTCP_INDEX_MAX);
        assert_eq!(sender.protect_rtcp(&plain), Err(SrtpError::Malformed), "SRTCP never reuses an exhausted index");

        for len in 0..8 {
            assert_eq!(sender.protect_rtcp(&vec![0; len]), Err(SrtpError::Short), "short RTCP header of {len} bytes");
        }
        for len in 0..22 {
            assert_eq!(receiver.unprotect_rtcp(&vec![0; len]), Err(SrtpError::Short), "short SRTCP packet of {len} bytes");
        }
        let mut malformed = plain;
        malformed[0] = 0;
        assert_eq!(sender.protect_rtcp(&malformed), Err(SrtpError::Malformed), "invalid RTCP version");
        malformed[0] = 0x81;
        malformed[3] = 255;
        assert_eq!(sender.protect_rtcp(&malformed), Err(SrtpError::Short), "truncated RTCP body");
    }
}
