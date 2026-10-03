//! Shared helpers for the text tests (char, str, String vs real std).
#![allow(dead_code)]

use rapid_std_check::fmt as hf;

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Pieces that exercise 1-4 byte chars, whitespace, line ends and Σ contexts.
pub const PIECES: &[&str] = &[
    "a", "b", "ab", "é", "€", "𝄞", " ", "\n", "\r\n", "\t", "Σ", "σ", "A", "'", "\u{301}", "ß", "\u{3000}", "x", "aa",
];

pub fn random_string(rng: &mut Rng, max_pieces: usize) -> String {
    let n = rng.below(max_pieces + 1);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(PIECES[rng.below(PIECES.len())]);
    }
    s
}

/// The panic message of `f`, or None if it returned.
pub fn panic_msg<R, F: FnOnce() -> R + std::panic::UnwindSafe>(f: F) -> Option<String> {
    match std::panic::catch_unwind(f) {
        Ok(_) => None,
        Err(e) => {
            if let Some(s) = e.downcast_ref::<String>() {
                Some(s.clone())
            } else if let Some(s) = e.downcast_ref::<&str>() {
                Some(s.to_string())
            } else {
                Some("<non-string panic>".to_string())
            }
        }
    }
}

/// `format!("{:?}", v)` through Rapid's fmt.
pub fn hdebug<T: hf::Debug>(v: &T) -> String {
    let a = [hf::rt::Argument::new_debug(v)];
    hf::format(hf::Arguments::new_v1(&[""], &a)).as_str().to_string()
}

/// `format!("{}", v)` through Rapid's fmt.
pub fn hdisplay<T: hf::Display>(v: &T) -> String {
    let a = [hf::rt::Argument::new_display(v)];
    hf::format(hf::Arguments::new_v1(&[""], &a)).as_str().to_string()
}
