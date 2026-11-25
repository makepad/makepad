//! STUN binding messages (RFC 5389) and authenticated ICE-lite connectivity checks: the WebRTC
//! receiver's checks (`makepad-webrtc` re-exports this module) and the reflector's "what address do
//! you see me at" ([`super::reflect`]). The caller owns the transport. This module only reads and
//! writes bytes. Pure Rust (SHA-1 from [`crate::digest`]), so it builds on every platform.

use crate::digest::Sha1;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

/// HMAC-SHA1 (RFC 2104) of the concatenation of `parts` under `key`: MESSAGE-INTEGRITY's MAC.
pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        let mut long = Sha1::new();
        long.update(key);
        block[..20].copy_from_slice(&long.finalise());
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(&block.map(|b| b ^ 0x36));
    for part in parts {
        inner.update(part);
    }
    let inner = inner.finalise();
    let mut outer = Sha1::new();
    outer.update(&block.map(|b| b ^ 0x5c));
    outer.update(&inner);
    outer.finalise()
}

/// The cookie in every STUN header.
pub const MAGIC_COOKIE: u32 = 0x2112_A442;
/// The binding method.
pub const BINDING: u16 = 0x0001;

/// Attribute types used by STUN and ICE.
pub mod attr {
    /// An unmasked transport address.
    pub const MAPPED_ADDRESS: u16 = 0x0001;
    /// The authentication username.
    pub const USERNAME: u16 = 0x0006;
    /// The HMAC-SHA1 authenticator.
    pub const MESSAGE_INTEGRITY: u16 = 0x0008;
    /// An error code and reason.
    pub const ERROR_CODE: u16 = 0x0009;
    /// A transport address masked with the cookie and transaction.
    pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
    /// The ICE candidate priority.
    pub const PRIORITY: u16 = 0x0024;
    /// The ICE nomination flag.
    pub const USE_CANDIDATE: u16 = 0x0025;
    /// Bytes that only make a request bigger (RFC 5780): a reflector answers no request smaller
    /// than its answer.
    pub const PADDING: u16 = 0x0026;
    /// The sending software's name.
    pub const SOFTWARE: u16 = 0x8022;
    /// The CRC-32 fingerprint.
    pub const FINGERPRINT: u16 = 0x8028;
    /// The controlled ICE role and tie breaker.
    pub const ICE_CONTROLLED: u16 = 0x8029;
    /// The controlling ICE role and tie breaker.
    pub const ICE_CONTROLLING: u16 = 0x802A;
}

/// A STUN message class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Request,
    Indication,
    Success,
    Error,
}

/// A decoded message. Attribute values exclude padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub class: Class,
    pub method: u16,
    pub transaction: [u8; 12],
    pub attributes: Vec<(u16, Vec<u8>)>,
}

/// A malformed STUN header or attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StunError {
    Short,
    NotStun,
    BadLength,
    BadAttribute,
}

/// Checks RFC 7983 demultiplexing: first byte 0..=3, at least 20 bytes,
/// and the magic cookie at bytes 4..8.
pub fn is_stun(packet: &[u8]) -> bool {
    packet.len() >= 20 && packet[0] <= 3 && packet[4..8] == MAGIC_COOKIE.to_be_bytes()
}

// Validate the whole packet while borrowing attribute values in wire order.
fn visit_attributes<'a>(packet: &'a [u8], mut visit: impl FnMut(usize, u16, &'a [u8])) -> Result<(), StunError> {
    if packet.len() < 20 {
        return Err(StunError::Short);
    }
    if packet[0] & 0xc0 != 0 || packet[4..8] != MAGIC_COOKIE.to_be_bytes() {
        return Err(StunError::NotStun);
    }
    let length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if length % 4 != 0 || length + 20 != packet.len() {
        return Err(StunError::BadLength);
    }
    let mut offset = 20;
    while offset < packet.len() {
        let header = packet.get(offset..offset + 4).ok_or(StunError::BadAttribute)?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let length = u16::from_be_bytes([header[2], header[3]]) as usize;
        let end = offset + 4 + length;
        let padded_end = (end + 3) & !3;
        if padded_end > packet.len() {
            return Err(StunError::BadAttribute);
        }
        visit(offset, kind, &packet[offset + 4..end]);
        offset = padded_end;
    }
    Ok(())
}

/// Parses the header and aligned attributes. Values exclude padding.
/// The header's message length must match the packet exactly.
pub fn parse(packet: &[u8]) -> Result<Message, StunError> {
    let mut attributes = Vec::new();
    visit_attributes(packet, |_, kind, value| attributes.push((kind, value.to_vec())))?;
    let kind = u16::from_be_bytes([packet[0], packet[1]]);
    let class = match kind & 0x0110 {
        0x0000 => Class::Request,
        0x0010 => Class::Indication,
        0x0100 => Class::Success,
        _ => Class::Error,
    };
    let method = (kind & 0x000f) | ((kind & 0x00e0) >> 1) | ((kind & 0x3e00) >> 2);
    let mut transaction = [0; 12];
    transaction.copy_from_slice(&packet[8..20]);
    Ok(Message {
        class,
        method,
        transaction,
        attributes,
    })
}

impl Message {
    /// Returns the first attribute of this type.
    pub fn get(&self, kind: u16) -> Option<&[u8]> {
        self.attributes.iter().find(|(ty, _)| *ty == kind).map(|(_, value)| value.as_slice())
    }

    /// Checks whether this attribute type is present.
    pub fn has(&self, kind: u16) -> bool {
        self.get(kind).is_some()
    }

    /// Returns USERNAME if its value is valid UTF-8.
    pub fn username(&self) -> Option<&str> {
        std::str::from_utf8(self.get(attr::USERNAME)?).ok()
    }

    /// Decodes an IPv4 or IPv6 XOR-MAPPED-ADDRESS as specified by RFC 5389.
    pub fn xor_mapped_address(&self) -> Option<SocketAddr> {
        let value = self.get(attr::XOR_MAPPED_ADDRESS)?;
        let header = value.get(..4)?;
        let port = u16::from_be_bytes([header[2], header[3]]) ^ (MAGIC_COOKIE >> 16) as u16;
        let cookie = MAGIC_COOKIE.to_be_bytes();
        match (header[1], value.len()) {
            (1, 8) => {
                let mut ip = [0; 4];
                for i in 0..4 {
                    ip[i] = value[4 + i] ^ cookie[i];
                }
                Some(SocketAddr::new(Ipv4Addr::from(ip).into(), port))
            }
            (2, 20) => {
                let mut ip = [0; 16];
                for i in 0..16 {
                    ip[i] = value[4 + i] ^ if i < 4 { cookie[i] } else { self.transaction[i - 4] };
                }
                Some(SocketAddr::new(Ipv6Addr::from(ip).into(), port))
            }
            _ => None,
        }
    }
}

fn find_attribute(packet: &[u8], kind: u16) -> Option<(usize, &[u8])> {
    let mut found = None;
    visit_attributes(packet, |offset, ty, value| {
        if ty == kind && found.is_none() {
            found = Some((offset, value));
        }
    })
    .ok()?;
    found
}

/// Checks MESSAGE-INTEGRITY using RFC 5389's adjusted header length.
/// The HMAC covers bytes before the attribute. Comparison is constant-time.
/// Returns false for a malformed message or an absent authenticator.
pub fn check_integrity(packet: &[u8], key: &[u8]) -> bool {
    let Some((offset, value)) = find_attribute(packet, attr::MESSAGE_INTEGRITY) else {
        return false;
    };
    if value.len() != 20 {
        return false;
    }
    let length = ((offset + 24 - 20) as u16).to_be_bytes();
    let expected = hmac_sha1(key, &[&packet[..2], &length, &packet[4..offset]]);
    let mut difference = 0u8;
    for i in 0..20 {
        difference |= std::hint::black_box(expected[i] ^ value[i]);
    }
    difference == 0
}

/// CRC-32 as FINGERPRINT uses it (RFC 5389 section 15.5): IEEE 802.3, reflected, polynomial
/// 0xEDB88320, initial value and final XOR all ones.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &b| CRC32_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8))
}

/// Checks the CRC-32 before FINGERPRINT, XOR 0x5354554e, as in RFC 5389.
/// Returns false for a malformed message or an absent or nonfinal fingerprint.
pub fn check_fingerprint(packet: &[u8]) -> bool {
    let Some((offset, value)) = find_attribute(packet, attr::FINGERPRINT) else {
        return false;
    };
    value.len() == 4 && offset + 8 == packet.len() && value == (crc32(&packet[..offset]) ^ 0x5354_554e).to_be_bytes()
}

pub(crate) fn new_binding(transaction: &[u8; 12], class: Class, capacity: usize) -> Vec<u8> {
    let class_bits = match class {
        Class::Request => 0x0000,
        Class::Indication => 0x0010,
        Class::Success => 0x0100,
        Class::Error => 0x0110,
    };
    let mut packet = Vec::with_capacity(capacity);
    packet.extend_from_slice(&(BINDING | class_bits).to_be_bytes());
    packet.extend_from_slice(&[0, 0]);
    packet.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    packet.extend_from_slice(transaction);
    packet
}

// Only fixed-size protocol values and bounded test values reach this builder.
pub(crate) fn append_attribute(packet: &mut Vec<u8>, kind: u16, value: &[u8]) {
    packet.extend_from_slice(&kind.to_be_bytes());
    packet.extend_from_slice(&(value.len() as u16).to_be_bytes());
    packet.extend_from_slice(value);
    packet.resize((packet.len() + 3) & !3, 0);
    let length = (packet.len() - 20) as u16;
    packet[2..4].copy_from_slice(&length.to_be_bytes());
}

pub(crate) fn authenticate(packet: &mut Vec<u8>, key: &[u8]) {
    let offset = packet.len();
    append_attribute(packet, attr::MESSAGE_INTEGRITY, &[0; 20]);
    let mac = hmac_sha1(key, &[&packet[..offset]]);
    packet[offset + 4..offset + 24].copy_from_slice(&mac);
    append_fingerprint(packet);
}

/// Appends FINGERPRINT, which must be the last attribute.
pub(crate) fn append_fingerprint(packet: &mut Vec<u8>) {
    let offset = packet.len();
    append_attribute(packet, attr::FINGERPRINT, &[0; 4]);
    let fingerprint = crc32(&packet[..offset]) ^ 0x5354_554e;
    packet[offset + 4..offset + 8].copy_from_slice(&fingerprint.to_be_bytes());
}

/// Builds a binding success with XOR-MAPPED-ADDRESS, MESSAGE-INTEGRITY,
/// and FINGERPRINT in that order. The integrity key is `key`.
pub fn binding_success(transaction: &[u8; 12], from: SocketAddr, key: &[u8]) -> Vec<u8> {
    let (value, length) = xor_mapped_value(transaction, from);
    let mut packet = new_binding(transaction, Class::Success, 20 + 4 + length + 24 + 8);
    append_attribute(&mut packet, attr::XOR_MAPPED_ADDRESS, &value[..length]);
    authenticate(&mut packet, key);
    packet
}

/// XOR-MAPPED-ADDRESS's value for `from` (RFC 5389 section 15.2) and its length (8 or 20).
pub(crate) fn xor_mapped_value(transaction: &[u8; 12], from: SocketAddr) -> ([u8; 20], usize) {
    let mut value = [0; 20];
    value[2..4].copy_from_slice(&(from.port() ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
    let cookie = MAGIC_COOKIE.to_be_bytes();
    let length = match from {
        SocketAddr::V4(address) => {
            value[1] = 1;
            for (i, byte) in address.ip().octets().iter().enumerate() {
                value[4 + i] = byte ^ cookie[i];
            }
            8
        }
        SocketAddr::V6(address) => {
            value[1] = 2;
            for (i, byte) in address.ip().octets().iter().enumerate() {
                value[4 + i] = byte ^ if i < 4 { cookie[i] } else { transaction[i - 4] };
            }
            20
        }
    };
    (value, length)
}

/// A browser's connectivity check, for tests of what answers it.
pub fn binding_request(transaction: &[u8; 12], username: &str, password: &str, nominate: bool) -> Vec<u8> {
    let mut packet = new_binding(transaction, Class::Request, 100);
    append_attribute(&mut packet, attr::USERNAME, username.as_bytes());
    append_attribute(&mut packet, attr::PRIORITY, &0x6e00_01ffu32.to_be_bytes());
    append_attribute(&mut packet, attr::ICE_CONTROLLING, &0x932f_f9b1_5126_3b36u64.to_be_bytes());
    if nominate {
        append_attribute(&mut packet, attr::USE_CANDIDATE, &[]);
    }
    authenticate(&mut packet, password.as_bytes());
    packet
}

/// ICE-lite credentials used to answer browser connectivity checks.
pub struct IceLite {
    pub local_ufrag: String,
    pub local_pwd: String,
    pub remote_ufrag: String,
}

/// A response to send and the check's nomination flag.
#[derive(Debug, PartialEq, Eq)]
pub struct Check {
    pub response: Vec<u8>,
    pub nominated: bool,
}

impl IceLite {
    /// Answers binding requests with USERNAME "<local_ufrag>:<remote_ufrag>".
    /// MESSAGE-INTEGRITY must verify under the bytes of `local_pwd`.
    /// FINGERPRINT must verify when present. Bad checks are silently dropped.
    /// Only attributes before MESSAGE-INTEGRITY can identify or nominate a peer.
    pub fn answer(&self, packet: &[u8], from: SocketAddr) -> Option<Check> {
        let mut username = None;
        let mut nominated = false;
        let mut integrity_seen = false;
        let mut fingerprint_seen = false;
        visit_attributes(packet, |_, kind, value| {
            if kind == attr::FINGERPRINT {
                fingerprint_seen = true;
            }
            if !integrity_seen {
                match kind {
                    attr::USERNAME if username.is_none() => username = Some(value),
                    attr::USE_CANDIDATE => nominated = true,
                    attr::MESSAGE_INTEGRITY => integrity_seen = true,
                    _ => {}
                }
            }
        })
        .ok()?;
        if packet[..2] != BINDING.to_be_bytes()
            || username?.strip_prefix(self.local_ufrag.as_bytes())?.strip_prefix(b":")? != self.remote_ufrag.as_bytes()
            || !check_integrity(packet, self.local_pwd.as_bytes())
            || (fingerprint_seen && !check_fingerprint(packet))
        {
            return None;
        }
        let mut transaction = [0; 12];
        transaction.copy_from_slice(&packet[8..20]);
        Some(Check {
            response: binding_success(&transaction, from, self.local_pwd.as_bytes()),
            nominated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        digits.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
    }

    #[test]
    fn hmac_sha1_rfc2202() {
        // RFC 2202 cases 1, 2 and 6 (a key longer than the block is hashed first).
        assert_eq!(hmac_sha1(&[0x0b; 20], &[b"Hi There"]).to_vec(), hex("b617318655057264e28bc0b6fb378c8ef146be00"), "case 1");
        assert_eq!(hmac_sha1(b"Jefe", &[b"what do ya want ", b"for nothing?"]).to_vec(), hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"), "case 2, in parts");
        assert_eq!(
            hmac_sha1(&[0xaa; 80], &[b"Test Using Larger Than Block-Size Key - Hash Key First"]).to_vec(),
            hex("aa4ae5e15272d00e95705637ce8a3b55ed402112"),
            "case 6"
        );
    }

    #[test]
    fn rfc5769_request_and_ipv4_response() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926, "the standard CRC-32 check value");
        let request = hex("00 01 00 58 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 10 53 54 55 4e 20 74 65 73 74 20 63 6c 69 65 6e 74
             00 24 00 04 6e 00 01 ff
             80 29 00 08 93 2f f9 b1 51 26 3b 36
             00 06 00 09 65 76 74 6a 3a 68 36 76 59 20 20 20
             00 08 00 14 9a ea a7 0c bf d8 cb 56 78 1e f2 b5 b2 d3 f2 49 c1 b5 71 a2
             80 28 00 04 e5 7a 3b cf");
        let response = hex("01 01 00 3c 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 0b 74 65 73 74 20 76 65 63 74 6f 72 20
             00 20 00 08 00 01 a1 47 e1 12 a6 43
             00 08 00 14 2b 91 f5 99 fd 9e 90 c3 8c 74 89 f9 2a f9 ba 53 f0 6b e7 d7
             80 28 00 04 c0 7d 4c 96");
        let password = b"VOkJxbRl1RmTxUk/WvJxBt";
        let transaction = [0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae];
        for (packet, class, software) in [(&request, Class::Request, "STUN test client"), (&response, Class::Success, "test vector")] {
            assert!(is_stun(packet), "RFC 5769 demultiplexing: {class:?}");
            let message = parse(packet).expect("RFC 5769 packet parses");
            assert_eq!(message.class, class, "RFC 5769 class");
            assert_eq!(message.method, BINDING, "RFC 5769 method");
            assert_eq!(message.transaction, transaction, "RFC 5769 transaction");
            assert_eq!(
                message.get(attr::SOFTWARE),
                Some(software.as_bytes()),
                "RFC 5769 software without padding"
            );
            assert!(check_integrity(packet, password), "RFC 5769 integrity: {class:?}");
            assert!(check_fingerprint(packet), "RFC 5769 fingerprint: {class:?}");
            assert!(!check_integrity(packet, b"wrong password"), "wrong password rejected: {class:?}");
            let mut corrupt = packet.clone();
            corrupt[24] ^= 1;
            assert!(!check_integrity(&corrupt, password), "flipped payload bit fails integrity: {class:?}");
            assert!(!check_fingerprint(&corrupt), "flipped payload bit fails fingerprint: {class:?}");
            for end in 0..packet.len() {
                assert!(parse(&packet[..end]).is_err(), "truncated packet rejected at {end}: {class:?}");
                assert!(
                    !check_integrity(&packet[..end], password),
                    "truncated integrity rejected at {end}: {class:?}"
                );
                assert!(!check_fingerprint(&packet[..end]), "truncated fingerprint rejected at {end}: {class:?}");
            }
        }
        let message = parse(&request).expect("RFC 5769 request parses");
        assert_eq!(message.username(), Some("evtj:h6vY"), "RFC 5769 username without padding");
        let message = parse(&response).expect("RFC 5769 response parses");
        assert_eq!(
            message.xor_mapped_address(),
            Some("192.0.2.1:32853".parse().unwrap()),
            "RFC 5769 IPv4 address"
        );
        let mut bad_attribute = request.clone();
        bad_attribute[22..24].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(parse(&bad_attribute), Err(StunError::BadAttribute), "attribute cannot overrun the packet");
        let mut trailing = response.clone();
        append_attribute(&mut trailing, attr::SOFTWARE, b"test vector");
        assert!(!check_fingerprint(&trailing), "fingerprint must be last");
    }

    #[test]
    fn binding_round_trips_and_ice_lite_checks() {
        let transaction = [0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae];
        let ice = IceLite {
            local_ufrag: "local".into(),
            local_pwd: "local_pwd".into(),
            remote_ufrag: "remote".into(),
        };
        let key = ice.local_pwd.as_bytes();
        for address in ["192.0.2.10:5000", "[2001:db8::1]:6000"] {
            let address: SocketAddr = address.parse().expect("round trip address");
            let packet = binding_success(&transaction, address, key);
            let message = parse(&packet).expect("binding success parses");
            assert_eq!(message.class, Class::Success, "round trip success class: {address}");
            assert_eq!(message.method, BINDING, "round trip binding method: {address}");
            assert_eq!(message.transaction, transaction, "round trip transaction: {address}");
            assert!(check_integrity(&packet, key), "round trip integrity: {address}");
            assert!(check_fingerprint(&packet), "round trip fingerprint: {address}");
            assert_eq!(message.xor_mapped_address(), Some(address), "round trip address: {address}");
        }
        let from: SocketAddr = "192.0.2.10:5000".parse().unwrap();
        let request = |username: &[u8], password: &[u8]| {
            let mut packet = new_binding(&transaction, Class::Request, 100);
            append_attribute(&mut packet, attr::USERNAME, username);
            append_attribute(&mut packet, attr::PRIORITY, &0x6e00_01ffu32.to_be_bytes());
            append_attribute(&mut packet, attr::ICE_CONTROLLING, &0x932f_f9b1_5126_3b36u64.to_be_bytes());
            append_attribute(&mut packet, attr::USE_CANDIDATE, &[]);
            authenticate(&mut packet, password);
            packet
        };
        let packet = request(b"local:remote", key);
        let check = ice.answer(&packet, from).expect("authenticated ICE request is answered");
        assert!(check.nominated, "USE-CANDIDATE nominates the check");
        assert!(check_integrity(&check.response, key), "ICE response integrity uses local password");
        assert!(check_fingerprint(&check.response), "ICE response fingerprint");
        let message = parse(&check.response).expect("ICE response parses");
        assert_eq!(message.transaction, transaction, "ICE response transaction");
        assert_eq!(message.xor_mapped_address(), Some(from), "ICE response source address");
        assert_eq!(ice.answer(&request(b"remote:local", key), from), None, "wrong username is dropped");
        assert_eq!(
            ice.answer(&request(b"local:remote", b"wrong password"), from),
            None,
            "wrong password is dropped"
        );
        assert_eq!(ice.answer(&check.response, from), None, "a success response is not a check");
        let mut no_fingerprint = packet[..packet.len() - 8].to_vec();
        let length = (no_fingerprint.len() - 20) as u16;
        no_fingerprint[2..4].copy_from_slice(&length.to_be_bytes());
        assert!(ice.answer(&no_fingerprint, from).is_some(), "fingerprint is optional");
        assert!(!check_fingerprint(&no_fingerprint), "absent fingerprint does not verify");
        let mut corrupt = packet.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(check_integrity(&corrupt, key), "fingerprint lies outside the HMAC");
        assert_eq!(ice.answer(&corrupt, from), None, "bad fingerprint is dropped");
        for end in 0..packet.len() {
            assert_eq!(ice.answer(&packet[..end], from), None, "truncated ICE check is dropped at {end}");
        }
    }
}
