use super::blowfish_tables::{P, S};

#[derive(Clone)]
pub(crate) struct Blowfish {
    p: [u32; 18],
    s: [[u32; 256]; 4],
}

impl Blowfish {
    pub(crate) fn new(key: &[u8]) -> Option<Self> {
        if key.is_empty() || key.len() > 56 {
            return None;
        }
        let mut blowfish = Self { p: P, s: S };
        let mut key_at = 0usize;
        for word in &mut blowfish.p {
            let mut key_word = 0u32;
            for _ in 0..4 {
                key_word = (key_word << 8) | key[key_at] as u32;
                key_at = (key_at + 1) % key.len();
            }
            *word ^= key_word;
        }

        let (mut left, mut right) = (0, 0);
        for index in (0..18).step_by(2) {
            (left, right) = blowfish.encrypt_words(left, right);
            blowfish.p[index] = left;
            blowfish.p[index + 1] = right;
        }
        for box_index in 0..4 {
            for index in (0..256).step_by(2) {
                (left, right) = blowfish.encrypt_words(left, right);
                blowfish.s[box_index][index] = left;
                blowfish.s[box_index][index + 1] = right;
            }
        }
        Some(blowfish)
    }

    pub(crate) fn encrypt_words(&self, mut left: u32, mut right: u32) -> (u32, u32) {
        for round in 0..16 {
            left ^= self.p[round];
            right ^= self.f(left);
            std::mem::swap(&mut left, &mut right);
        }
        std::mem::swap(&mut left, &mut right);
        right ^= self.p[16];
        left ^= self.p[17];
        (left, right)
    }

    pub(crate) fn decrypt_words(&self, mut left: u32, mut right: u32) -> (u32, u32) {
        for round in (2..18).rev() {
            left ^= self.p[round];
            right ^= self.f(left);
            std::mem::swap(&mut left, &mut right);
        }
        std::mem::swap(&mut left, &mut right);
        right ^= self.p[1];
        left ^= self.p[0];
        (left, right)
    }

    fn f(&self, value: u32) -> u32 {
        let a = (value >> 24) as usize;
        let b = ((value >> 16) & 0xff) as usize;
        let c = ((value >> 8) & 0xff) as usize;
        let d = (value & 0xff) as usize;
        (self.s[0][a].wrapping_add(self.s[1][b]) ^ self.s[2][c]).wrapping_add(self.s[3][d])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cnc_import::blowfish_pi;

    #[test]
    fn cnc_import_blowfish_tables_match_machin_pi_generator() {
        let (p, s) = blowfish_pi::generate();
        assert_eq!(p[0..4], [0x243f_6a88, 0x85a3_08d3, 0x1319_8a2e, 0x0370_7344]);
        assert_eq!(s[0][0..2], [0xd131_0ba6, 0x98df_b5ac]);
        assert_eq!(p, P);
        assert_eq!(s, S);
    }

    #[test]
    fn cnc_import_blowfish_zero_key_known_answer_and_round_trip() {
        let blowfish = Blowfish::new(&[0; 8]).unwrap();
        let encrypted = blowfish.encrypt_words(0, 0);
        assert_eq!(encrypted, (0x4ef9_9745, 0x6198_dd78));
        assert_eq!(blowfish.decrypt_words(encrypted.0, encrypted.1), (0, 0));
    }
}
