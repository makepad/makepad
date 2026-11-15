//! The party code: the part of the page's address a guest gets from the QR
//! code on the big screen. Without it the server answers nothing but 404.

/// No 0/o, 1/i/l: a code read off a screen is typed without mistakes.
const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
/// 31^8, about 2^40: out of reach of the server's guess limit.
pub const LEN: usize = 8;

pub fn new_code() -> String {
    let mut bytes = [0u8; LEN * 2];
    makepad_network::tls::os_random(&mut bytes).expect("OS randomness");
    // Two bytes per symbol keep the modulo bias under 1/2000.
    bytes.chunks(2).map(|b| ALPHABET[(u16::from_be_bytes([b[0], b[1]]) as usize) % ALPHABET.len()] as char).collect()
}

/// A code given on the command line: the same alphabet, at least 6 symbols.
pub fn valid(code: &str) -> bool {
    code.len() >= 6 && code.len() <= 32 && code.bytes().all(|b| ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    #[test]
    fn codes_are_valid_and_differ() {
        let a = super::new_code();
        assert_eq!(a.len(), super::LEN);
        assert!(super::valid(&a), "{a}");
        assert_ne!(a, super::new_code());
        assert!(!super::valid("abc"), "too short");
        assert!(!super::valid("party01"), "0 and 1 are not in the alphabet");
    }
}
