//! Spoken codes for a known number of payload bits. The sorted English list is part of the wire
//! format: a word's index carries eleven bits. Payload and checksum both start at their high bit.

use super::discover::Hint;
use crate::digest::sha256_hash;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::OnceLock;

pub const BITS_PER_WORD: usize = 11;
pub const MIN_CHECK_BITS: usize = 4;
const MAX_WORD_LEN: usize = 8;

/// The words in wire order.
pub fn list() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&str>> = OnceLock::new();
    WORDS.get_or_init(|| include_str!("words_en.txt").lines().collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The meaningful bits start at the high bit of the first byte. Unused low bits are zero.
    pub payload: Vec<u8>,
    pub corrected: Vec<Correction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// One-based word number, as read aloud.
    pub position: usize,
    /// The token as entered, including its original case.
    pub heard: String,
    pub word: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WordsError {
    Count { expected: usize, got: usize },
    /// `position` is one-based.
    Unknown { position: usize, heard: String, suggestions: Vec<&'static str> },
    Checksum,
    NewerVersion { version: u8 },
}

impl fmt::Display for WordsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count { expected, got } => write!(f, "Expected {expected} words and heard {got}. Read the list back together."),
            Self::Unknown { position, heard, suggestions } => {
                write!(f, "Word {position}, '{heard}', isn't on the list. ")?;
                match suggestions.as_slice() {
                    [] => write!(f, "Ask your peer to spell it."),
                    [word] => write!(f, "Did you mean {word}?"),
                    [a, b] => write!(f, "Did you mean {a} or {b}?"),
                    [a, b, c, ..] => write!(f, "Did you mean {a}, {b}, or {c}?"),
                }
            }
            Self::Checksum => write!(f, "The words don't add up. One was probably misheard. Read them back to each other."),
            Self::NewerVersion { version } => write!(f, "Your peer runs a newer invite version ({version}). Update your app and try again."),
        }
    }
}

impl std::error::Error for WordsError {}

fn word_count(bits: usize) -> usize {
    bits / BITS_PER_WORD + (bits % BITS_PER_WORD + MIN_CHECK_BITS).div_ceil(BITS_PER_WORD)
}

// Read or write up to 16 bits, starting at an arbitrary network-order bit offset.
fn read_bits(bytes: &[u8], start: usize, width: usize) -> u16 {
    let mut value = 0;
    for bit in start..start + width {
        value = (value << 1) | u16::from((bytes[bit / 8] >> (7 - bit % 8)) & 1);
    }
    value
}

fn write_bits(bytes: &mut [u8], start: usize, width: usize, value: u16) {
    for i in 0..width {
        let bit = start + i;
        let mask = 0x80 >> (bit % 8);
        if value & (1 << (width - 1 - i)) != 0 {
            bytes[bit / 8] |= mask;
        } else {
            bytes[bit / 8] &= !mask;
        }
    }
}

fn clear_padding(bytes: &mut [u8], bits: usize) {
    if bits % 8 != 0 {
        bytes[bits / 8] &= 0xff << (8 - bits % 8);
    }
}

fn checksum(domain: &[u8], payload: &[u8], width: usize) -> u16 {
    read_bits(&sha256_hash(&[domain, payload].concat()), 0, width)
}

/// Encode the first `bits` of `payload`, then fill the last word(s) with leading bits of
/// SHA-256(domain || payload). The hash covers exactly ceil(bits / 8) bytes with zero low padding.
///
/// Panics if the slice contains fewer than `bits` bits.
pub fn encode(domain: &[u8], payload: &[u8], bits: usize) -> Vec<&'static str> {
    let count = word_count(bits);
    let mut wire = payload[..bits.div_ceil(8)].to_vec();
    clear_padding(&mut wire, bits);
    let width = count * BITS_PER_WORD - bits;
    let check = checksum(domain, &wire, width);
    wire.resize((count * BITS_PER_WORD).div_ceil(8), 0);
    write_bits(&mut wire, bits, width, check);
    let words = list();
    (0..count).map(|i| words[read_bits(&wire, i * BITS_PER_WORD, BITS_PER_WORD) as usize]).collect()
}

// After the first mismatch, the rest must line up after a single skipped, changed or swapped letter.
fn one_edit(a: &[u8], b: &[u8]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let Some(i) = short.iter().zip(long).position(|(x, y)| x != y) else {
        return short.len() != long.len();
    };
    if short.len() != long.len() {
        return short[i..] == long[i + 1..];
    }
    short[i + 1..] == long[i + 1..]
        || (i + 1 < short.len() && short[i] == long[i + 1] && short[i + 1] == long[i] && short[i + 2..] == long[i + 2..])
}

// Two edits with no net length change can be two replacements, a swap followed by a
// length-preserving edit, or a deletion and insertion. For any pair involving insertion or
// deletion, undo a letter on the longer side first. This also handles overlapping edits (ab -> bca).
fn two_edits(a: &[u8], b: &[u8]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 2 {
        return false;
    }
    let mut scratch = [0; MAX_WORD_LEN + 2];
    let buf = &mut scratch[..long.len()];
    buf.copy_from_slice(long);
    if long.len() == short.len() {
        if short.iter().zip(long).filter(|(x, y)| x != y).count() == 2 {
            return true;
        }
        for i in 1..long.len() {
            if buf[i - 1] != buf[i] {
                buf.swap(i - 1, i);
                if one_edit(short, buf) {
                    return true;
                }
                buf.swap(i - 1, i);
            }
        }
    }
    // Start with the last letter missing, then move the gap one place left at a time.
    for i in (0..long.len()).rev() {
        if one_edit(short, &buf[..long.len() - 1]) {
            return true;
        }
        if i > 0 {
            buf[i - 1] = long[i];
        }
    }
    false
}

fn match_word(heard: &str, position: usize, corrected: &mut Vec<Correction>) -> Result<usize, WordsError> {
    // The dictionary is ASCII. Each non-ASCII letter counts as one mismatching letter, regardless
    // of its UTF-8 width. A token longer than any word by three cannot have a suggestion.
    let mut letters = [0; MAX_WORD_LEN + 2];
    let mut len = 0;
    for c in heard.chars() {
        if len == letters.len() {
            return Err(WordsError::Unknown { position, heard: heard.into(), suggestions: Vec::new() });
        }
        letters[len] = if c.is_ascii() { c.to_ascii_lowercase() as u8 } else { 0xff };
        len += 1;
    }
    let letters = &letters[..len];
    let words = list();
    match words.binary_search_by(|word| word.as_bytes().cmp(letters)) {
        Ok(index) => return Ok(index),
        Err(next) if len >= 4 && words.get(next).is_some_and(|word| word.as_bytes().starts_with(letters)) => return Ok(next),
        _ => {}
    }
    let mut nearest = words.iter().enumerate().filter(|(_, w)| one_edit(letters, w.as_bytes()));
    if let Some((index, &word)) = nearest.next() {
        if nearest.next().is_none() {
            corrected.push(Correction { position, heard: heard.into(), word });
            return Ok(index);
        }
    }
    let suggestions = words.iter().copied().filter(|w| two_edits(letters, w.as_bytes())).take(3).collect();
    Err(WordsError::Unknown { position, heard: heard.into(), suggestions })
}

/// Decode case-insensitive words separated by whitespace or punctuation. A unique four-letter
/// (or longer) prefix is accepted as shorthand. A unique single edit is reported to the caller.
pub fn decode(domain: &[u8], text: &str, bits: usize) -> Result<Decoded, WordsError> {
    let tokens = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty());
    let expected = word_count(bits);
    let got = tokens.clone().count();
    if got != expected {
        return Err(WordsError::Count { expected, got });
    }
    let wire_bits = expected * BITS_PER_WORD;
    let mut payload = vec![0; wire_bits.div_ceil(8)];
    let mut corrected = Vec::new();
    for (i, token) in tokens.enumerate() {
        let index = match_word(token, i + 1, &mut corrected)?;
        write_bits(&mut payload, i * BITS_PER_WORD, BITS_PER_WORD, index as u16);
    }
    let width = wire_bits - bits;
    let check = read_bits(&payload, bits, width);
    payload.truncate(bits.div_ceil(8));
    clear_padding(&mut payload, bits);
    if check != checksum(domain, &payload, width) {
        return Err(WordsError::Checksum);
    }
    Ok(Decoded { payload, corrected })
}

/// Separate spoken groups of four with a middle dot.
pub fn group(words: &[&str]) -> String {
    let mut text = String::new();
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            text.push_str(if i % 4 == 0 { " · " } else { " " });
        }
        text.push_str(word);
    }
    text
}

pub const INVITE_DOMAIN: &[u8] = b"makepad p2p invite v1";
const VERSION: u8 = 0;
const COMMIT_DOMAIN: &[u8] = b"makepad p2p invite commitment v1\0";
pub const COMMIT_BITS: usize = 54;
const COMMIT_MASK: u64 = (1 << COMMIT_BITS) - 1;

// Field positions in the v1 payload, counting from its least significant bit.
const PORT_SHIFT: usize = COMMIT_BITS;
const IP_SHIFT: usize = PORT_SHIFT + 16;
const HINT_SHIFT: usize = IP_SHIFT + 32;
const VERSION_SHIFT: usize = HINT_SHIFT + 2;
const INVITE_PAYLOAD_BITS: usize = VERSION_SHIFT + 1;
const INVITE_BITS: usize = INVITE_PAYLOAD_BITS + 5;
const _: () = assert!(INVITE_BITS == 10 * BITS_PER_WORD);

// Independently computed for the fixed test public key [0x11; 32] in an invite for 192.0.2.1:0
// with an unknown NAT.
#[cfg(test)]
pub(super) const TEST_COMMITMENT: u64 = 0x33_8a4f_7910_99dd;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    pub nat: Hint,
    pub addr: SocketAddrV4,
    /// The low 54 bits hold the inviter's key commitment.
    pub commit: u64,
}

impl Invite {
    /// Hashes the commitment domain, the invite's own fields and the public key together, then
    /// applies SHA-256 another 2^20 times. The first 54 bits of the final hash form the invite's
    /// key commitment. The fields salt it: `version u8 | hint u8 | IPv4 [4] | port u16`, network
    /// order, so work done ahead for one address and port serves no other.
    /// Run this off the UI thread. Each candidate key requires the full stretch.
    pub fn commitment(nat: Hint, addr: SocketAddrV4, public_key: &[u8]) -> u64 {
        let salt = [&[VERSION, nat as u8][..], &addr.ip().octets(), &addr.port().to_be_bytes()].concat();
        let mut hash = sha256_hash(&[COMMIT_DOMAIN, &salt, public_key].concat());
        for _ in 0..(1 << 20) {
            hash = sha256_hash(&hash);
        }
        u64::from_be_bytes(hash[..8].try_into().unwrap()) >> (64 - COMMIT_BITS)
    }

    /// Checks the peer's public key against the invite. Run this off the UI thread.
    pub fn matches(&self, public_key: &[u8]) -> bool {
        Self::commitment(self.nat, self.addr, public_key) == (self.commit & COMMIT_MASK)
    }

    pub fn to_words(&self) -> Vec<&'static str> {
        let payload = (u128::from(self.nat as u8) << HINT_SHIFT)
            | (u128::from(u32::from(*self.addr.ip())) << IP_SHIFT)
            | (u128::from(self.addr.port()) << PORT_SHIFT)
            | u128::from(self.commit & COMMIT_MASK);
        let bytes = (payload << (128 - INVITE_PAYLOAD_BITS)).to_be_bytes();
        encode(INVITE_DOMAIN, &bytes, INVITE_PAYLOAD_BITS)
    }

    pub fn from_words(text: &str) -> Result<(Invite, Vec<Correction>), WordsError> {
        let Decoded { payload, corrected } = decode(INVITE_DOMAIN, text, INVITE_PAYLOAD_BITS)?;
        let mut bytes = [0; 16];
        bytes[..payload.len()].copy_from_slice(&payload);
        let bits = u128::from_be_bytes(bytes) >> (128 - INVITE_PAYLOAD_BITS);
        let version = (bits >> VERSION_SHIFT) as u8;
        if version != VERSION {
            return Err(WordsError::NewerVersion { version });
        }
        let nat = match (bits >> HINT_SHIFT) & 3 {
            0 => Hint::Unknown,
            1 => Hint::Easy,
            2 => Hint::ConeUnknownFiltering,
            _ => Hint::Symmetric,
        };
        let ip = Ipv4Addr::from((bits >> IP_SHIFT) as u32);
        let addr = SocketAddrV4::new(ip, (bits >> PORT_SHIFT) as u16);
        let commit = bits as u64 & COMMIT_MASK;
        Ok((Self { nat, addr, commit }, corrected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn word_list() {
        let words = list();
        assert_eq!(words.len(), 1 << BITS_PER_WORD, "count of words in the wire alphabet");

        // Strings within two edits share a subsequence made by dropping at most two letters
        // from each. Index these short ASCII keys so we only compare plausible pairs.
        fn key(word: &str, skip: usize, also_skip: usize) -> u64 {
            word.bytes().enumerate().filter(|(i, _)| *i != skip && *i != also_skip)
                .fold(0, |key, (_, c)| (key << 5) | u64::from(c - b'a' + 1))
        }
        let mut by_key = HashMap::<u64, Vec<&str>>::new();
        for (i, &word) in words.iter().enumerate() {
            assert!(word.bytes().all(|b| b.is_ascii_lowercase()), "non-English letter in '{word}'");
            assert!((3..=MAX_WORD_LEN).contains(&word.len()), "word length for '{word}'");
            if i > 0 {
                let prev = words[i - 1];
                assert!(prev < word, "'{prev}' and '{word}' must be strictly ordered");
                assert!(!word.starts_with(prev), "'{prev}' is a prefix of '{word}'");
                assert_ne!(prev.get(..4).unwrap_or(prev), word.get(..4).unwrap_or(word), "'{prev}' and '{word}' share four letters");
            }
            for a in 0..=word.len() {
                for b in a..=word.len() {
                    let peers = by_key.entry(key(word, a, b)).or_default();
                    if peers.last() == Some(&word) {
                        continue;
                    }
                    for &peer in peers.iter() {
                        assert!(!one_edit(word.as_bytes(), peer.as_bytes()), "'{word}' and '{peer}' are one edit apart");
                        assert!(!two_edits(word.as_bytes(), peer.as_bytes()), "'{word}' and '{peer}' share a possible typo");
                    }
                    peers.push(word);
                }
            }
        }
        for &word in words {
            for len in 4..word.len() {
                let prefix = &word[..len];
                for skip in 0..=len {
                    if let Some(peers) = by_key.get(&key(prefix, skip, skip)) {
                        for &peer in peers {
                            assert!(peer == word || !one_edit(prefix.as_bytes(), peer.as_bytes()),
                                "a typo in '{peer}' could be accepted as '{prefix}' for '{word}'");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn codec_and_invites() {
        for addr in [
            "0.0.0.0:0", "0.0.0.0:65535", "255.255.255.255:0", "255.255.255.255:65535",
            "192.0.2.1:0", "198.51.100.23:41394", "203.0.113.254:65535",
        ] {
            let addr = addr.parse().unwrap();
            for nat in [Hint::Unknown, Hint::Easy, Hint::ConeUnknownFiltering, Hint::Symmetric] {
                for commit in [0, (1 << 54) - 1] {
                    let invite = Invite { nat, addr, commit };
                    let words = invite.to_words();
                    assert_eq!(words.len(), 10, "spoken length for {invite:?}");
                    assert_eq!(Invite::from_words(&group(&words)), Ok((invite, Vec::new())), "{nat:?} at {addr}");
                }
            }
        }
        // The independently packed payload 78c66c82f43640d8c05bbb9b7d00 has checksum prefix 00100.
        let invite = Invite {
            addr: "198.51.100.23:41394".parse().unwrap(),
            nat: Hint::Symmetric,
            commit: 0x01_b180_b777_36fa,
        };
        let text = "kestrel damask catlike tundra · gulp huggable acoustic jumpers · opossum tuneful";
        assert_eq!(group(&invite.to_words()), text, "v1 wire vector");
        let high_bits_set = Invite { commit: invite.commit | !COMMIT_MASK, ..invite };
        assert_eq!(group(&high_bits_set.to_words()), text, "only the low 54 commitment bits go on the wire");
        assert_eq!(Invite::from_words(text), Ok((invite, Vec::new())), "read a known invite");

        // Crossing the four-bit minimum changes the count from one word to two. Exercise
        // empty, partial-byte and whole-word payloads, including nonzero bits past the end.
        let raw = [0xab, 0xdf, 0xff];
        for bits in 0..=raw.len() * 8 {
            let words = encode(b"word test", &raw, bits);
            assert_eq!(words.len(), (bits + MIN_CHECK_BITS).div_ceil(BITS_PER_WORD), "word count for {bits} bits");
            let decoded = decode(b"word test", &group(&words), bits).expect("decode bit prefix");
            let mut expected = raw[..bits.div_ceil(8)].to_vec();
            if bits % 8 != 0 {
                *expected.last_mut().unwrap() &= u8::MAX << (8 - bits % 8);
            }
            assert_eq!(decoded.payload, expected, "zero-padded {bits}-bit payload");
        }
        // These SHA-256 prefixes were computed independently for "word test" || aa and || ab.
        assert_eq!(encode(b"word test", &raw, 7), [list()[0x55f]], "7 payload bits and 4 checksum bits");
        assert_eq!(encode(b"word test", &raw, 8), [list()[0x55f], list()[0x13d]], "8 payload bits and 14 checksum bits");
        assert_eq!(decode(b"other domain", &group(&encode(b"word test", &raw, 8)), 8), Err(WordsError::Checksum), "domain binding");

        let mut newer = [0; INVITE_PAYLOAD_BITS.div_ceil(8)];
        newer[0] = 0x80;
        let text = group(&encode(INVITE_DOMAIN, &newer, INVITE_PAYLOAD_BITS));
        let error = Invite::from_words(&text).unwrap_err();
        assert_eq!(error, WordsError::NewerVersion { version: 1 }, "version follows the checksum check");
        assert!(error.to_string().contains("newer"), "explain the peer version");
    }

    #[test]
    fn stretched_key_commitment() {
        let invite = Invite {
            addr: "192.0.2.1:0".parse().unwrap(),
            nat: Hint::Unknown,
            commit: TEST_COMMITMENT,
        };
        assert!(invite.matches(&[0x11; 32]), "the key reproduces the independently stretched commitment");
        assert!(!invite.matches(&[0x22; 32]), "a different key has a different commitment");
        let other_port = Invite { addr: "192.0.2.1:1".parse().unwrap(), ..invite };
        assert!(!other_port.matches(&[0x11; 32]), "the invite's address and port salt the commitment");
    }

    #[test]
    fn spoken_input_and_typos() {
        let domain = b"spoken words";
        let spoken = ["dab", "lantern", "orbit", "quiet", "tundra"];
        let bits = spoken.len() * BITS_PER_WORD;
        let mut payload = vec![0; bits.div_ceil(8)];
        for (i, word) in spoken.iter().enumerate() {
            write_bits(&mut payload, i * BITS_PER_WORD, BITS_PER_WORD, list().binary_search(word).unwrap() as u16);
        }
        let mut words = encode(domain, &payload, bits);
        let mixed = format!("DAB,LaNt.ORBIT-quiet\nTUNDRA · {}", words[5]);
        assert_eq!(decode(domain, &mixed, bits), Ok(Decoded { payload: payload.clone(), corrected: Vec::new() }),
            "case, mixed separators, four-letter shorthand and a whole three-letter word");

        for heard in ["lzntern", "lanxtern", "lanern", "lantren", "lÁntern"] {
            words[1] = heard;
            let decoded = decode(domain, &group(&words), bits).expect("correct one changed, extra, missing or swapped letter");
            assert_eq!(decoded.payload, payload, "corrected payload for {heard}");
            assert_eq!(decoded.corrected, [Correction { position: 2, heard: heard.into(), word: "lantern" }], "read-back for {heard}");
        }

        words[1] = "zzntern";
        assert_eq!(decode(domain, &group(&words), bits), Err(WordsError::Unknown {
            position: 2, heard: "zzntern".into(), suggestions: vec!["lantern"],
        }), "suggest a word two edits away");
        assert!(two_edits(b"ab", b"bca"), "a letter inserted inside a transposed pair counts as two edits");
        words[1] = spoken[1];
        let last = words.len() - 1;
        let check = list().binary_search(&words[last]).unwrap();
        words[last] = list()[check ^ 1];
        assert_eq!(decode(domain, &group(&words), bits), Err(WordsError::Checksum), "another list word changes a checksum bit");
        assert_eq!(decode(domain, &group(&words[1..]), bits), Err(WordsError::Count { expected: 6, got: 5 }), "a missing word");
    }
}
