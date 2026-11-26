//! The seal uses AES-128-CTR with HMAC-SHA256-128 in encrypt-then-MAC order. It
//! authenticates opaque associated data, the sequence and the ciphertext. One sealer
//! per key and stream owns the outgoing counter. Each opener owns its replay window.
//!
//! The caller defines the packet layout and supplies the associated data. Its boundary
//! must be unambiguous, for example through a fixed header length.

pub use crate::tls::Aes128Ecb;
use crate::digest::{sha256_hash, Sha256};
use std::io;

/// XORs data in place with AES-CTR starting at `counter`. Only the low 32 bits
/// count up, in big-endian order. If they would wrap, returns `InvalidInput`
/// before modifying data. Each counter block used for encryption must be unique per key.
pub fn ctr_xor(aes: &mut Aes128Ecb, counter: [u8; 16], data: &mut [u8]) -> io::Result<()> {
    let mut number = u32::from_be_bytes(counter[12..].try_into().unwrap());
    let available = u64::from(u32::MAX - number) + 1;
    if data.len().div_ceil(16) as u64 > available {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut stream = [0u8; 64 * 16];
    for chunk in data.chunks_mut(stream.len()) {
        let batch = &mut stream[..chunk.len().div_ceil(16) * 16];
        for block in batch.chunks_exact_mut(16) {
            block[..12].copy_from_slice(&counter[..12]);
            block[12..].copy_from_slice(&number.to_be_bytes());
            number = number.wrapping_add(1);
        }
        aes.encrypt_blocks(batch)?;
        for (byte, mask) in chunk.iter_mut().zip(batch.iter()) {
            *byte ^= mask;
        }
    }
    Ok(())
}

/// HMAC-SHA256 over the parts in order, as one message.
pub fn hmac_sha256_parts(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    hmac_sha256_with(key, |hash| {
        for part in parts {
            hash.update(part);
        }
    })
}

fn hmac_sha256_with(key: &[u8], message: impl FnOnce(&mut Sha256)) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&sha256_hash(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(&block.map(|b| b ^ 0x36));
    message(&mut inner);
    let mut outer = Sha256::new();
    outer.update(&block.map(|b| b ^ 0x5c));
    outer.update(&inner.finalise());
    outer.finalise()
}

/// RFC 5869 HKDF-Extract followed by HKDF-Expand with SHA-256.
///
/// # Panics
/// Panics if `out` exceeds 255 × 32 bytes.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) {
    let prk = hmac_sha256_parts(salt, &[ikm]);
    hkdf_sha256_expand(&prk, &[info], out);
}

/// RFC 5869 HKDF-Expand with SHA-256 over the concatenated info parts.
///
/// # Panics
/// Panics if `out` exceeds 255 × 32 bytes.
pub fn hkdf_sha256_expand(prk: &[u8; 32], info_parts: &[&[u8]], out: &mut [u8]) {
    assert!(out.len() <= 255 * 32, "HKDF-SHA256 output exceeds 255 blocks");
    let mut previous = [0u8; 32];
    for (index, chunk) in out.chunks_mut(32).enumerate() {
        previous = hmac_sha256_with(prk, |hash| {
            if index != 0 {
                hash.update(&previous);
            }
            for part in info_parts {
                hash.update(part);
            }
            hash.update(&[(index + 1) as u8]);
        });
        chunk.copy_from_slice(&previous[..chunk.len()]);
    }
}

/// AES-128 key, HMAC-SHA256 key and CTR salt for one stream, derived by HKDF-SHA256.
pub struct SealKeys {
    aes: Aes128Ecb,
    mac: [u8; 32],
    salt: [u8; 8],
}

impl SealKeys {
    /// Expands 56 bytes under the concatenated info parts: 16 bytes for AES,
    /// 32 for HMAC and 8 for the CTR salt.
    pub fn derive(ikm: &[u8], salt: &[u8], info: &[&[u8]]) -> io::Result<Self> {
        let mut prk = hmac_sha256_parts(salt, &[ikm]);
        let mut material = [0u8; 56];
        hkdf_sha256_expand(&prk, info, &mut material);
        wipe(&mut prk);
        let keys = Aes128Ecb::new(material[..16].try_into().unwrap()).map(|aes| Self {
            aes,
            mac: material[16..48].try_into().unwrap(),
            salt: material[48..].try_into().unwrap(),
        });
        wipe(&mut material);
        keys
    }

    fn crypt(&mut self, seq: u64, data: &mut [u8]) -> io::Result<()> {
        let mut counter = [0u8; 16];
        counter[..8].copy_from_slice(&self.salt);
        counter[8..14].copy_from_slice(&seq.to_be_bytes()[2..]);
        // The length check keeps the block counter in the final two bytes.
        ctr_xor(&mut self.aes, counter, data)
    }

    fn tag(&self, aad: &[u8], seq_and_ciphertext: &[u8]) -> [u8; TAG_LEN] {
        hmac_sha256_parts(&self.mac, &[aad, seq_and_ciphertext])[..TAG_LEN].try_into().unwrap()
    }
}

fn wipe<const N: usize>(secret: &mut [u8; N]) {
    // SAFETY: this array is live and exclusively borrowed. A volatile store keeps the erase.
    unsafe { std::ptr::write_volatile(secret, [0; N]) };
}

impl Drop for SealKeys {
    fn drop(&mut self) {
        wipe(&mut self.mac);
        wipe(&mut self.salt);
    }
}

const SEQ_LEN: usize = 6;
const TAG_LEN: usize = 16;
const SEQ_LIMIT: u64 = 1 << 48;
const MAX_PLAINTEXT: usize = (1 << 16) * 16;

/// Owns the outgoing sequence for one stream. Use fresh keys when starting a new sealer.
pub struct Sealer {
    keys: SealKeys,
    next: u64,
}

impl Sealer {
    /// Starts the stream at sequence zero.
    pub fn new(keys: SealKeys) -> Self {
        Self { keys, next: 0 }
    }

    /// Appends a six-byte big-endian sequence, ciphertext and 16-byte tag. The tag
    /// authenticates `aad | seq48 | ciphertext`. On error the output keeps its original length.
    pub fn seal(&mut self, aad: &[u8], plaintext: &[u8], out: &mut Vec<u8>) -> io::Result<u64> {
        if self.next >= SEQ_LIMIT {
            return Err(io::Error::other("seal sequence exhausted"));
        }
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        out.try_reserve(SEQ_LEN + plaintext.len() + TAG_LEN).map_err(|_| io::ErrorKind::OutOfMemory)?;
        let start = out.len();
        let seq = self.next;
        // Once encryption starts, even a backend failure cannot make this counter available again.
        self.next += 1;
        out.extend_from_slice(&seq.to_be_bytes()[2..]);
        out.extend_from_slice(plaintext);
        if let Err(e) = self.keys.crypt(seq, &mut out[start + SEQ_LEN..]) {
            out.truncate(start);
            return Err(e);
        }
        let tag = self.keys.tag(aad, &out[start..]);
        out.extend_from_slice(&tag);
        Ok(seq)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Too few bytes to contain both a sequence and a tag.
    Short,
    /// Already opened, or too old for this opener's window.
    Replayed,
    /// Invalid tag, ciphertext beyond the block limit, or a local decryption failure.
    BadTag,
}

/// Authenticates incoming data for one stream and remembers the sequences it opens.
pub struct Opener {
    keys: SealKeys,
    window: ReplayWindow,
}

impl Opener {
    pub fn new(keys: SealKeys, window: usize) -> Self {
        Self { keys, window: ReplayWindow::new(window) }
    }

    /// Checks freshness and the tag without decrypting or recording the sequence.
    pub fn verify(&self, aad: &[u8], sealed: &[u8]) -> Result<u64, OpenError> {
        if sealed.len() < SEQ_LEN + TAG_LEN {
            return Err(OpenError::Short);
        }
        let (body, tag) = sealed.split_at(sealed.len() - TAG_LEN);
        let mut sequence = [0u8; 8];
        sequence[2..].copy_from_slice(&body[..SEQ_LEN]);
        let seq = u64::from_be_bytes(sequence);
        if !self.window.check(seq) {
            return Err(OpenError::Replayed);
        }
        if body.len() - SEQ_LEN > MAX_PLAINTEXT || !crate::tls::constant_time_eq(&self.keys.tag(aad, body), tag) {
            return Err(OpenError::BadTag);
        }
        Ok(seq)
    }

    /// Clears `out`, verifies the tag, records the sequence, then writes the plaintext.
    /// On a local cipher failure the authenticated sequence stays consumed and `out` is empty.
    pub fn open(&mut self, aad: &[u8], sealed: &[u8], out: &mut Vec<u8>) -> Result<u64, OpenError> {
        out.clear();
        let seq = self.verify(aad, sealed)?;
        self.window.commit(seq);
        out.extend_from_slice(&sealed[SEQ_LEN..sealed.len() - TAG_LEN]);
        if self.keys.crypt(seq, out).is_err() {
            out.clear();
            return Err(OpenError::BadTag);
        }
        Ok(seq)
    }
}

/// A replay window for one stream. Each bitmap word represents 64 consecutive
/// absolute sequence numbers. Words are reused as their numbers expire.
pub struct ReplayWindow {
    highest: u64,
    width: u64,
    words: Box<[u64]>,
}

impl ReplayWindow {
    /// Allocates the bitmap once. The smallest window remembers one sequence.
    pub fn new(width: usize) -> Self {
        let width = width.max(1);
        // An unaligned window can straddle a word boundary even for a width of 64.
        let word_count = 1 + (width - 1).div_ceil(64);
        Self { highest: 0, width: width as u64, words: vec![0; word_count].into_boxed_slice() }
    }

    fn slot(&self, word: u64) -> usize {
        (word % self.words.len() as u64) as usize
    }

    /// Accepts newer numbers and unseen numbers within the window, without recording them.
    pub fn check(&self, seq: u64) -> bool {
        if seq > self.highest {
            return true;
        }
        self.highest - seq < self.width && self.words[self.slot(seq / 64)] & (1 << (seq % 64)) == 0
    }

    /// Records a sequence number after its MAC verifies. Older numbers outside the
    /// window have already expired.
    pub fn commit(&mut self, seq: u64) {
        if seq > self.highest {
            if seq - self.highest >= self.width {
                self.words.fill(0);
            } else {
                for word in self.highest / 64 + 1..=seq / 64 {
                    let slot = self.slot(word);
                    self.words[slot] = 0;
                }
            }
            self.highest = seq;
        }
        if self.highest - seq < self.width {
            let slot = self.slot(seq / 64);
            self.words[slot] |= 1 << (seq % 64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn aes128_fips197_and_block_batches() {
        use crate::tls::from_hex;

        let key = from_hex("000102030405060708090a0b0c0d0e0f").unwrap();
        let mut aes = Aes128Ecb::new(&key).unwrap();
        let mut block = from_hex::<16>("00112233445566778899aabbccddeeff").unwrap();
        aes.encrypt_blocks(&mut block).unwrap();
        assert_eq!(block, from_hex("69c4e0d86a7b0430d8cdb78070b4c55a").unwrap(), "FIPS-197 C.1");

        let mut batch: [u8; 1600] = std::array::from_fn(|i| (i / 16 + i % 16) as u8);
        let mut singles = batch;
        let err = aes.encrypt_blocks(&mut batch[..17]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(batch, singles, "a partial block must be rejected before encryption");
        aes.encrypt_blocks(&mut []).unwrap();
        aes.encrypt_blocks(&mut batch).unwrap();
        for block in singles.chunks_exact_mut(16) {
            aes.encrypt_blocks(block).unwrap();
        }
        assert_eq!(batch, singles, "100 blocks together must match separate calls");
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "windows", target_os = "linux")))]
    #[test]
    fn aes128_reports_unsupported() {
        assert!(matches!(Aes128Ecb::new(&[0; 16]), Err(e) if e.kind() == std::io::ErrorKind::Unsupported));
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn ctr_rfc3686_and_counter_boundaries() {
        use crate::tls::from_hex;

        fn vector<const N: usize>(key: &str, counter: &str, plaintext: [u8; N], ciphertext: &str) {
            let mut aes = Aes128Ecb::new(&from_hex(key).unwrap()).unwrap();
            let counter = from_hex(counter).unwrap();
            let mut data = plaintext;
            ctr_xor(&mut aes, counter, &mut data).unwrap();
            assert_eq!(data, from_hex(ciphertext).unwrap(), "RFC 3686, {N} bytes");
            ctr_xor(&mut aes, counter, &mut data).unwrap();
            assert_eq!(data, plaintext, "CTR decryption, {N} bytes");
        }
        vector("ae6852f8121067cc4bf7a5765577f39e", "00000030000000000000000000000001",
            *b"Single block msg", "e4095d4fb7a7b3792d6175a3261311b8");
        vector::<32>("7e24067817fae0d743d6ce1f32539163", "006cb6dbc0543b59da48d90b00000001",
            std::array::from_fn(|i| i as u8), "5104a106168a72d9790d41ee8edad388eb2e1efc46da57c8fce630df9141be28");
        vector::<36>("7691be035e5020a8ac6e618529f9a0dc", "00e0017b27777f3f4a1786f000000001",
            std::array::from_fn(|i| i as u8), "c1cf48a89f2ffdd9cf4652e9efdb72d74540a42bde6d7836d59a5ceaaef3105325b2072f");

        let mut aes = Aes128Ecb::new(&[0x42; 16]).unwrap();
        let mut data = [0; 64 * 16 + 3];
        let mut counter = [0xff; 16];
        counter[12..].copy_from_slice(&(u32::MAX - 64).to_be_bytes());
        ctr_xor(&mut aes, counter, &mut data).unwrap();
        let mut last = [0xff; 16];
        aes.encrypt_blocks(&mut last).unwrap();
        assert_eq!(data[64 * 16..], last[..3], "counter continues across batches");

        let mut too_long = [7; 17];
        let err = ctr_xor(&mut aes, [0xff; 16], &mut too_long).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(too_long, [7; 17], "counter exhaustion is detected before writing");
        ctr_xor(&mut aes, [0xff; 16], &mut []).unwrap();
    }

    #[test]
    fn multipart_hmac_matches_rfc4231_and_joined_input() {
        let parts: &[&[u8]] = &[b"what do ya want ", b"for nothing?"];
        assert_eq!(hmac_sha256_parts(b"Jefe", parts), crate::tls::from_hex(
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843").unwrap(), "RFC 4231 case 2");
        let data: [u8; 150] = std::array::from_fn(|i| i as u8);
        let key = [17; 80];
        let expected = crate::tunnel::hmac_sha256(&key, &data);
        for split in [0, 1, 63, 64, 65, data.len()] {
            assert_eq!(hmac_sha256_parts(&key, &[&data[..split], &[], &data[split..]]),
                expected, "split at byte {split}");
        }
        assert_eq!(hmac_sha256_parts(&[], &[]), crate::tunnel::hmac_sha256(&[], &[]));
    }

    #[test]
    fn hkdf_rfc5869_sha256_vectors() {
        fn vector<const N: usize>(ikm: &[u8], salt: &[u8], info: &[u8], prk_hex: &str, okm_hex: &str) {
            let prk = crate::tls::from_hex(prk_hex).unwrap();
            let expected = crate::tls::from_hex::<N>(okm_hex).unwrap();
            assert_eq!(hmac_sha256_parts(salt, &[ikm]), prk, "HKDF-Extract");
            let mut out = [0; N];
            hkdf_sha256(salt, ikm, info, &mut out);
            assert_eq!(out, expected, "HKDF-Extract and Expand");
            let (a, b) = info.split_at(info.len() / 2);
            let mut expanded = [0; N];
            hkdf_sha256_expand(&prk, &[a, b], &mut expanded);
            assert_eq!(expanded, expected, "HKDF-Expand with multipart info");
        }

        let short_ikm = [0x0b; 22];
        vector::<42>(&short_ikm, &std::array::from_fn::<_, 13, _>(|i| i as u8),
            &std::array::from_fn::<_, 10, _>(|i| 0xf0 + i as u8),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5",
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865");
        vector::<82>(&std::array::from_fn::<_, 80, _>(|i| i as u8),
            &std::array::from_fn::<_, 80, _>(|i| 0x60 + i as u8),
            &std::array::from_fn::<_, 80, _>(|i| 0xb0 + i as u8),
            "06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244",
            concat!("b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c",
                "59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71",
                "cc30c58179ec3e87c14c01d5c1f3434f1d87"));
        vector::<42>(&short_ikm, &[], &[],
            "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04",
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8");
    }

    #[test]
    #[should_panic(expected = "HKDF-SHA256 output exceeds 255 blocks")]
    fn hkdf_rejects_an_unrepresentable_block_number() {
        hkdf_sha256_expand(&[0; 32], &[], &mut [0; 255 * 32 + 1]);
    }

    #[test]
    fn replay_window_tracks_authenticated_sequences() {
        for width in [1, 63, 64, 65, 128, 2048] {
            let mut window = ReplayWindow::new(width);
            assert!(window.check(0), "sequence zero starts a fresh window");
            window.commit(0);
            // Cross word boundaries and reuse ring slots; leave alternate numbers unseen.
            for seq in (2..=4 * width as u64 + 64).step_by(2) {
                assert!(window.check(seq), "first arrival of {seq}");
                window.commit(seq);
                assert!(!window.check(seq), "duplicate {seq}");
                if seq >= width as u64 {
                    let oldest = seq - width as u64 + 1;
                    assert_eq!(window.check(oldest), oldest % 2 == 1, "oldest bit, width {width}");
                    assert!(!window.check(oldest - 1), "one sequence too old, width {width}");
                }
            }
            assert!(window.check(u64::MAX), "checking a jump does not commit it");
            assert!(!window.check(4 * width as u64 + 64), "highest mark survived the check");
            window.commit(u64::MAX);
            let oldest = u64::MAX - (width as u64 - 1);
            assert!(!window.check(oldest - 1), "width below the highest");
            if width > 1 {
                assert!(window.check(oldest), "jump cleared old marks");
                window.commit(oldest);
                assert!(!window.check(oldest), "duplicate reordered sequence");
            }
            assert!(!window.check(0), "full-width sequence distances do not wrap");
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "windows", target_os = "linux"))]
    mod streams {
        use super::*;

        #[test]
        fn stream_keys_match_the_hkdf_split() {
            let mut material = [0; 56];
            hkdf_sha256(b"call", b"secret", b"sender/voice", &mut material);
            let mut keys = SealKeys::derive(b"secret", b"call", &[b"sender/", b"voice"]).unwrap();
            assert_eq!(keys.mac, material[16..48], "middle 32 bytes key the MAC");
            assert_eq!(keys.salt, material[48..], "last 8 bytes salt the counter");
            let mut expected = [0; 16];
            let mut actual = expected;
            Aes128Ecb::new(material[..16].try_into().unwrap()).unwrap().encrypt_blocks(&mut expected).unwrap();
            keys.aes.encrypt_blocks(&mut actual).unwrap();
            assert_eq!(actual, expected, "first 16 bytes key AES");
        }

        fn keys(stream: &[u8]) -> SealKeys {
            SealKeys::derive(b"secret", b"call", &[b"sender/", stream]).unwrap()
        }

        #[test]
        fn sealer_layout_and_limits() {
            let mut sealer = Sealer::new(keys(b"voice"));
            let mut out = b"header".to_vec();
            assert_eq!(sealer.seal(b"aad", &[], &mut out).unwrap(), 0);
            let start = out.len();
            assert_eq!(sealer.seal(b"aad", &[0; 32], &mut out).unwrap(), 1);
            assert_eq!(&out[..6], b"header", "preserve the caller's prefix");
            let sealed = &out[start..];
            assert_eq!(sealed.len(), 6 + 32 + 16);
            assert_eq!(&sealed[..6], &[0, 0, 0, 0, 0, 1]);

            let mut material = [0; 56];
            hkdf_sha256(b"call", b"secret", b"sender/voice", &mut material);
            let mut counters = [0; 32];
            for (i, block) in counters.chunks_exact_mut(16).enumerate() {
                block[..8].copy_from_slice(&material[48..]);
                block[13] = 1;
                block[15] = i as u8;
            }
            Aes128Ecb::new(material[..16].try_into().unwrap()).unwrap().encrypt_blocks(&mut counters).unwrap();
            assert_eq!(sealed[6..38], counters, "CTR uses salt, sequence and a 16-bit block count from zero");
            let mac_input = [b"aad".as_slice(), &sealed[..38]].concat();
            assert_eq!(sealed[38..], crate::tunnel::hmac_sha256(&material[16..48], &mac_input)[..16]);

            let before = out.clone();
            assert!(sealer.seal(b"", &vec![0; MAX_PLAINTEXT + 1], &mut out).is_err());
            assert_eq!(out, before, "oversized input appended nothing");
            assert_eq!(sealer.seal(b"", &[], &mut out).unwrap(), 2, "invalid length consumed no sequence");

            let mut last = Sealer { keys: keys(b"video"), next: SEQ_LIMIT - 1 };
            out.clear();
            assert_eq!(last.seal(b"", &[], &mut out).unwrap(), SEQ_LIMIT - 1);
            assert_eq!(&out[..6], &[0xff; 6]);
            let before = out.clone();
            assert!(last.seal(b"", &[], &mut out).is_err());
            assert_eq!(out, before, "sequence exhaustion appended nothing");
        }

        #[test]
        fn open_round_trips_and_verify_preserves_freshness() {
            let mut sender = Sealer::new(keys(b"voice"));
            let mut receiver = Opener::new(keys(b"voice"), 128);
            let mut packet = Vec::new();
            let mut plain = vec![0xee; 10];
            for (seq, len) in [0, 1, 15, 16, 17, 1171].into_iter().enumerate() {
                let input: Vec<_> = (0..len).map(|i| (i % 251) as u8).collect();
                packet.clear();
                assert_eq!(sender.seal(b"aad", &input, &mut packet).unwrap(), seq as u64);
                assert_eq!(receiver.verify(b"aad", &packet), Ok(seq as u64));
                assert_eq!(receiver.verify(b"aad", &packet), Ok(seq as u64), "verify did not commit");
                assert_eq!(receiver.open(b"aad", &packet, &mut plain), Ok(seq as u64));
                assert_eq!(plain, input, "round trip of {len} bytes");
                assert_eq!(receiver.verify(b"aad", &packet), Err(OpenError::Replayed));
                assert_eq!(receiver.open(b"aad", &packet, &mut plain), Err(OpenError::Replayed));
                assert!(plain.is_empty(), "no stale plaintext after refusal");
            }
            for len in 0..SEQ_LEN + TAG_LEN {
                assert_eq!(receiver.verify(b"aad", &packet[..len]), Err(OpenError::Short));
                assert_eq!(receiver.open(b"aad", &packet[..len], &mut plain), Err(OpenError::Short));
            }
        }

        #[test]
        fn authentication_covers_every_part_before_committing() {
            let mut sender = Sealer::new(keys(b"voice"));
            let mut receiver = Opener::new(keys(b"voice"), 128);
            let mut plain = Vec::new();
            for (seq, field) in ["aad", "sequence", "ciphertext", "tag"].into_iter().enumerate() {
                let aad = *b"hdr";
                let input = b"speech";
                let mut packet = Vec::new();
                sender.seal(&aad, input, &mut packet).unwrap();
                let mut changed_aad = aad;
                let mut changed = packet.clone();
                match field {
                    "aad" => changed_aad[0] ^= 1,
                    "sequence" => changed[0] ^= 1,
                    "ciphertext" => changed[SEQ_LEN] ^= 1,
                    _ => *changed.last_mut().unwrap() ^= 1,
                }
                assert_eq!(receiver.verify(&changed_aad, &changed), Err(OpenError::BadTag), "{field}");
                assert_eq!(receiver.open(&changed_aad, &changed, &mut plain), Err(OpenError::BadTag), "{field}");
                assert!(plain.is_empty());
                assert_eq!(receiver.open(&aad, &packet, &mut plain), Ok(seq as u64), "window after forged {field}");
                assert_eq!(plain, input);
            }

            let mut voice = Sealer::new(keys(b"voice"));
            let mut video = Sealer::new(keys(b"video"));
            let mut a = Vec::new();
            let mut b = Vec::new();
            voice.seal(b"aad", b"data", &mut a).unwrap();
            video.seal(b"aad", b"data", &mut b).unwrap();
            assert_ne!(a, b, "changing stream info changes the seal");
            assert_eq!(Opener::new(keys(b"voice"), 128).open(b"aad", &b, &mut plain), Err(OpenError::BadTag));
            assert_eq!(Opener::new(keys(b"video"), 2048).open(b"aad", &a, &mut plain), Err(OpenError::BadTag));
        }

        #[test]
        fn open_reordered_sequences_with_each_window_width() {
            let mut sender = Sealer::new(keys(b"stream"));
            let mut first = Vec::new();
            let mut second = Vec::new();
            let mut last = Vec::new();
            sender.seal(b"", &[], &mut first).unwrap();
            sender.seal(b"", &[], &mut second).unwrap();
            for _ in 2..=128 {
                last.clear();
                sender.seal(b"", &[], &mut last).unwrap();
            }
            let mut plain = Vec::new();
            for width in [128, 2048] {
                let mut receiver = Opener::new(keys(b"stream"), width);
                assert_eq!(receiver.open(b"", &last, &mut plain), Ok(128));
                assert_eq!(receiver.open(b"", &second, &mut plain), Ok(1), "127 behind");
                let expected = if width == 128 { Err(OpenError::Replayed) } else { Ok(0) };
                assert_eq!(receiver.verify(b"", &first), expected);
                assert_eq!(receiver.open(b"", &first, &mut plain), expected, "128 behind, width {width}");
            }
        }

        #[test]
        fn throughput_at_1000_bytes() {
            use std::time::{Duration, Instant};

            let mut sender = Sealer::new(keys(b"timing"));
            let mut receiver = Opener::new(keys(b"timing"), 128);
            let input = [0x5a; 1000];
            let mut packet = Vec::with_capacity(SEQ_LEN + input.len() + TAG_LEN);
            let mut plain = Vec::with_capacity(input.len());
            let mut seal_time = Duration::ZERO;
            let mut open_time = Duration::ZERO;
            let mut pairs = 0u64;
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(200) {
                packet.clear();
                let before = Instant::now();
                sender.seal(b"aad", &input, &mut packet).unwrap();
                let sealed = Instant::now();
                receiver.open(b"aad", &packet, &mut plain).unwrap();
                std::hint::black_box(&plain);
                open_time += sealed.elapsed();
                seal_time += sealed - before;
                pairs += 1;
            }
            eprintln!("seal throughput (1000 bytes): {:.0} seals/s, {:.0} opens/s",
                pairs as f64 / seal_time.as_secs_f64(), pairs as f64 / open_time.as_secs_f64());
        }
    }
}
