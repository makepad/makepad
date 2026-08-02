//! SHA-256 and HMAC-SHA256 for lobby authentication.
//!
//! Every datagram and every reliable frame carries a truncated HMAC over its
//! bytes, verified *before* any peer state is touched. That single check is
//! what closes the audit's one-packet attacks: seq-window poisoning, spoofed
//! kicks, address hijacking and state injection all require forging a tag over
//! a key the attacker does not have.
//!
//! Self-contained on purpose — the crate depends only on micro_serde and lz4.

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub const SHA256_BLOCK: usize = 64;
pub const SHA256_OUT: usize = 32;

/// Bytes of HMAC kept on the wire. 64 bits is far past what a LAN attacker can
/// brute force in a session and keeps the per-packet overhead small.
pub const MAC_LEN: usize = 8;

pub fn sha256(data: &[u8]) -> [u8; SHA256_OUT] {
    let mut h = H0;
    let mut block = [0u8; SHA256_BLOCK];
    let mut chunks = data.chunks_exact(SHA256_BLOCK);
    for chunk in &mut chunks {
        block.copy_from_slice(chunk);
        compress(&mut h, &block);
    }

    let rest = chunks.remainder();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    block = [0u8; SHA256_BLOCK];
    block[..rest.len()].copy_from_slice(rest);
    block[rest.len()] = 0x80;
    if rest.len() + 1 + 8 > SHA256_BLOCK {
        compress(&mut h, &block);
        block = [0u8; SHA256_BLOCK];
    }
    block[SHA256_BLOCK - 8..].copy_from_slice(&bit_len.to_be_bytes());
    compress(&mut h, &block);

    let mut out = [0u8; SHA256_OUT];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn compress(h: &mut [u32; 8], block: &[u8; SHA256_BLOCK]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
        (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// Pre-expanded HMAC key: the two padded blocks are derived once per lobby, so
/// per-packet signing is two compressions plus the message.
#[derive(Clone)]
pub struct LobbyKey {
    ipad: [u8; SHA256_BLOCK],
    opad: [u8; SHA256_BLOCK],
}

impl std::fmt::Debug for LobbyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LobbyKey(..)")
    }
}

impl LobbyKey {
    pub fn new(secret: &[u8]) -> Self {
        let mut key = [0u8; SHA256_BLOCK];
        if secret.len() > SHA256_BLOCK {
            key[..SHA256_OUT].copy_from_slice(&sha256(secret));
        } else {
            key[..secret.len()].copy_from_slice(secret);
        }
        let mut ipad = [0x36u8; SHA256_BLOCK];
        let mut opad = [0x5cu8; SHA256_BLOCK];
        for i in 0..SHA256_BLOCK {
            ipad[i] ^= key[i];
            opad[i] ^= key[i];
        }
        Self { ipad, opad }
    }

    pub fn mac(&self, message: &[u8]) -> [u8; MAC_LEN] {
        let mut inner = Vec::with_capacity(SHA256_BLOCK + message.len());
        inner.extend_from_slice(&self.ipad);
        inner.extend_from_slice(message);
        let inner = sha256(&inner);

        let mut outer = [0u8; SHA256_BLOCK + SHA256_OUT];
        outer[..SHA256_BLOCK].copy_from_slice(&self.opad);
        outer[SHA256_BLOCK..].copy_from_slice(&inner);
        let full = sha256(&outer);

        let mut tag = [0u8; MAC_LEN];
        tag.copy_from_slice(&full[..MAC_LEN]);
        tag
    }

    /// Constant-time compare — a timing oracle on the tag would let an attacker
    /// forge one byte at a time.
    pub fn verify(&self, message: &[u8], tag: &[u8]) -> bool {
        if tag.len() != MAC_LEN {
            return false;
        }
        let expect = self.mac(message);
        let mut diff = 0u8;
        for i in 0..MAC_LEN {
            diff |= expect[i] ^ tag[i];
        }
        diff == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Spans the multi-block and length-padding edges.
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn hmac_matches_rfc4231_vectors() {
        // RFC 4231 case 1, truncated to our tag length.
        let key = LobbyKey::new(&[0x0b; 20]);
        assert_eq!(
            hex(&key.mac(b"Hi There")),
            "b0344c61d8db3853"[..MAC_LEN * 2].to_string()
        );
        // Case 2: key shorter than a block, different message.
        let key = LobbyKey::new(b"Jefe");
        assert_eq!(
            hex(&key.mac(b"what do ya want for nothing?")),
            "5bdcc146bf60754e"[..MAC_LEN * 2].to_string()
        );
        // Case 3: key longer than a block gets hashed down first.
        let key = LobbyKey::new(&[0xaa; 131]);
        assert_eq!(
            hex(&key.mac(b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f"[..MAC_LEN * 2].to_string()
        );
    }

    #[test]
    fn verify_rejects_wrong_key_message_and_length() {
        let key = LobbyKey::new(b"lobby-secret");
        let other = LobbyKey::new(b"lobby-secrew");
        let tag = key.mac(b"payload");
        assert!(key.verify(b"payload", &tag));
        assert!(!key.verify(b"payloae", &tag));
        assert!(!other.verify(b"payload", &tag));
        assert!(!key.verify(b"payload", &tag[..MAC_LEN - 1]));
        assert!(!key.verify(b"payload", &[0u8; MAC_LEN]));
    }
}
