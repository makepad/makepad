// UTF-8 validation, string iteration, patterns, boundaries, escapes and case conversion.
// Differential file: `| ` lines equal under rustc --test and makepad test.
// Replaces the former std/check text_str.rs.

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn add<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) {
        use std::fmt::Write;
        write!(self, "{:?};", value).unwrap();
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl std::fmt::Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "a", "b", "ab", "é", "€", "𝄞", " ", "\n", "\r\n", "\t", "Σ", "σ", "A", "'", "\u{301}", "ß",
    "\u{3000}", "x", "aa",
];
fn random_string(rng: &mut Rng, max_pieces: usize) -> String {
    let n = rng.below(max_pieces + 1);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(PIECES[rng.below(PIECES.len())]);
    }
    s
}

fn panicked<R>(f: impl FnOnce() -> R) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

macro_rules! observe {
    ($d:expr; $($value:expr),* $(,)?) => {{
        $($d.add(&$value);)*
    }};
}
fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    const FRAGS: &[&[u8]] = &[
        b"a", b"\xc3\xa9", b"\xe2\x82\xac", b"\xf0\x9d\x84\x9e", b"\xc3",
        b"\xe2\x82", b"\xf0\x9d", b"\x80", b"\xbf", b"\xc0\x80", b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80", b"\xf8", b"\xff", b"\xe0\x80\x80", b"\xf0\x80\x80\x80", b" ",
    ];
    let n = rng.below(7);
    let mut bytes = Vec::new();
    for _ in 0..n {
        bytes.extend_from_slice(FRAGS[rng.below(FRAGS.len())]);
    }
    bytes
}

#[test]
fn a01_from_utf8_random() {
    let mut rng = Rng(9);
    let mut d = D::new();
    for _ in 0..2_000 {
        let bytes = random_bytes(&mut rng);
        match std::str::from_utf8(&bytes) {
            Ok(s) => observe!(d; true, s),
            Err(error) => observe!(d; false, error.valid_up_to(), error.error_len(), error.to_string()),
        }
        let chunks: Vec<_> = bytes.utf8_chunks()
            .map(|c| (c.valid().to_string(), c.invalid().to_vec())).collect();
        observe!(d; chunks, String::from_utf8_lossy(&bytes).to_string());
    }
    d.line("str utf8");
}

#[test]
fn a02_chars_bytes_indices() {
    let mut rng = Rng(3);
    let mut d = D::new();
    for _ in 0..2_000 {
        let s = random_string(&mut rng, 8);
        observe!(d;
            s.chars().collect::<Vec<_>>(), s.chars().rev().collect::<Vec<_>>(),
            s.chars().count(), s.chars().last(), s.char_indices().collect::<Vec<_>>(),
            s.char_indices().rev().collect::<Vec<_>>(), s.bytes().collect::<Vec<_>>(),
            s.bytes().rev().collect::<Vec<_>>(), s.encode_utf16().collect::<Vec<_>>(),
            s.chars().size_hint(), s.encode_utf16().size_hint(),
        );
        let mut chars = s.chars();
        for _ in 0..12 {
            let next = if (rng.next() >> 3) & 1 == 0 { chars.next() } else { chars.next_back() };
            observe!(d; next, chars.as_str());
        }
        let mut indices = s.char_indices();
        for _ in 0..6 {
            observe!(d; indices.next(), indices.offset());
        }
    }
    d.line("str iteration");
}

const NEEDLES: &[&str] = &["", "a", "aa", "ab", "é", "€𝄞", "\n", " ", "Σ", "ba", "xyz"];
const CHARS: &[char] = &['a', 'é', '€', '𝄞', ' ', '\n', 'Σ', 'q'];

macro_rules! patterns {
    ($d:expr, $s:expr, $n:expr) => {{
        let s = $s;
        let n = $n;
        observe!($d;
            s.split(n).collect::<Vec<_>>(), s.rsplit(n).collect::<Vec<_>>(),
            s.split_terminator(n).collect::<Vec<_>>(), s.rsplit_terminator(n).collect::<Vec<_>>(),
            s.split_inclusive(n).collect::<Vec<_>>(),
        );
        for k in 0..4 {
            observe!($d; s.splitn(k, n).collect::<Vec<_>>(), s.rsplitn(k, n).collect::<Vec<_>>());
        }
        observe!($d;
            s.matches(n).collect::<Vec<_>>(), s.rmatches(n).collect::<Vec<_>>(),
            s.match_indices(n).collect::<Vec<_>>(), s.rmatch_indices(n).collect::<Vec<_>>(),
            s.find(n), s.rfind(n), s.contains(n), s.starts_with(n), s.ends_with(n),
            s.strip_prefix(n), s.strip_suffix(n), s.trim_start_matches(n), s.trim_end_matches(n),
            s.split_once(n), s.rsplit_once(n),
        );
    }};
}

#[test]
fn a03_str_patterns() {
    let mut rng = Rng(11);
    let mut d = D::new();
    for _ in 0..400 {
        let s = random_string(&mut rng, 7);
        let s = s.as_str();
        for &n in NEEDLES {
            patterns!(d, s, n);
            d.add(&s.replace(n, "<>"));
            for k in 0..3 {
                d.add(&s.replacen(n, "_", k));
            }
            let nn: &&str = &n;
            d.add(&s.split(nn).collect::<Vec<_>>());
            let ns = String::from(n);
            d.add(&s.split(&ns).collect::<Vec<_>>());
        }
    }
    d.line("str string patterns");
}

#[test]
fn a04_char_patterns() {
    let mut rng = Rng(12);
    let mut d = D::new();
    for _ in 0..600 {
        let s = random_string(&mut rng, 7);
        let s = s.as_str();
        for &c in CHARS {
            patterns!(d, s, c);
            observe!(d;
                s.split(c).rev().collect::<Vec<_>>(), s.split_terminator(c).rev().collect::<Vec<_>>(),
                s.split_inclusive(c).rev().collect::<Vec<_>>(), s.matches(c).rev().collect::<Vec<_>>(),
                s.trim_matches(c), s.replace(c, "-"),
            );
            let mut split = s.split(c);
            for _ in 0..6 {
                d.add(&if rng.next() & 1 == 0 { split.next() } else { split.next_back() });
            }
            let mut split = s.split_terminator(c);
            for _ in 0..6 {
                d.add(&if rng.next() & 1 == 0 { split.next() } else { split.next_back() });
            }
        }
        let set: &[char] = &['a', 'é', ' '];
        observe!(d;
            s.split(set).collect::<Vec<_>>(), s.split(set).rev().collect::<Vec<_>>(),
            s.trim_matches(set), s.find(['€', '\n']), s.split(['€', '\n']).collect::<Vec<_>>(),
            s.trim_end_matches(&['a', 'b']),
            s.split(|c: char| c.is_whitespace()).collect::<Vec<_>>(),
            s.rsplit(|c: char| c == 'a' || c == 'Σ').collect::<Vec<_>>(),
            s.trim_matches(|c: char| !c.is_alphabetic()), s.rfind(|c: char| c > 'z'),
        );
    }
    d.line("str char patterns");
}

#[test]
fn a05_whitespace_lines_trim() {
    let mut rng = Rng(13);
    let mut d = D::new();
    for _ in 0..3_000 {
        let s = random_string(&mut rng, 9);
        observe!(d;
            s.lines().collect::<Vec<_>>(), s.lines().rev().collect::<Vec<_>>(),
            s.split_whitespace().collect::<Vec<_>>(), s.split_whitespace().rev().collect::<Vec<_>>(),
            s.split_ascii_whitespace().collect::<Vec<_>>(), s.split_ascii_whitespace().rev().collect::<Vec<_>>(),
            s.trim(), s.trim_start(), s.trim_end(), s.trim_ascii(), s.trim_ascii_start(), s.trim_ascii_end(),
        );
    }
    d.line("str whitespace");
}

#[test]
fn a06_slicing_and_panics() {
    let mut rng = Rng(14);
    let mut d = D::new();
    for _ in 0..30 {
        let s = random_string(&mut rng, 5);
        let s = s.as_str();
        for i in 0..=s.len() + 1 {
            observe!(d;
                s.is_char_boundary(i), s.floor_char_boundary(i), s.ceil_char_boundary(i),
                s.split_at_checked(i), panicked(|| s.split_at(i)), s.get(..i), s.get(i..), s.get(..=i),
                panicked(|| &s[..i]), panicked(|| &s[i..]), panicked(|| &s[..=i]),
            );
            for j in 0..=s.len() + 1 {
                observe!(d; s.get(i..j), s.get(i..=j), panicked(|| &s[i..j]), panicked(|| &s[i..=j]));
            }
        }
        d.add(&panicked(|| &s[0..=usize::MAX]));
        let mut range = 0..=0usize;
        let _ = range.next();
        observe!(d; s.get(range.clone()), panicked(|| &s[range.clone()]));
    }
    d.line("str slicing");
}

#[test]
fn a07_escapes_and_debug() {
    let mut rng = Rng(15);
    let extra = [
        "\0", "\"", "\\", "\u{7f}", "\u{200b}", "\u{e000}", "\u{feff}",
        "\u{301}a", "a\u{301}", "\u{ad}", "\u{85}",
    ];
    let mut d = D::new();
    for i in 0..3_000 {
        let mut s = random_string(&mut rng, 6);
        if i % 3 == 0 {
            s.push_str(extra[rng.below(extra.len())]);
        }
        let s = s.as_str();
        observe!(d;
            s.escape_debug().collect::<String>(), s.escape_debug().to_string(),
            s.escape_default().collect::<String>(), s.escape_unicode().collect::<String>(),
            format!("{:?}", s), s.to_lowercase(), s.to_uppercase(), s.to_ascii_lowercase(),
            s.to_ascii_uppercase(), s.is_ascii(),
        );
        let t = s.to_ascii_uppercase();
        observe!(d; s.eq_ignore_ascii_case(&t), s.repeat(3), s.cmp(&t));
    }
    d.line("str escapes case");
}

#[test]
fn a08_final_sigma() {
    let cases = [
        "Σ", "AΣ", "ΑΣ", "ΑΣ ", "ΑΣΑ", "Α'Σ", "ΑΣ'", "ΑΣ'Α", "Α\u{301}Σ", "ΑΣ\u{301}",
        "ΣΑ", " Σ", "Α.Σ", "Α:Σ", "ΑΣ.", "ΌΣΟΣ ΣΟΦΟΣ", "\u{1f88}Σ", "1Σ", "ǅΣ", "ﬀΣ",
    ];
    let mut d = D::new();
    for s in cases {
        d.add(&s.to_lowercase());
    }
    for cp in 0..0x30000u32 {
        if let Some(c) = char::from_u32(cp) {
            d.add(&format!("{}Σ", c).to_lowercase());
            d.add(&format!("AΣ{}", c).to_lowercase());
            d.add(&format!("A{}Σ", c).to_lowercase());
        }
    }
    d.line("str sigma");
}

#[test]
fn a09_parse_bool() {
    for s in ["true", "false", "True", "", "1", "true "] {
        println!("| str bool {:?} {:?}", s, s.parse::<bool>().map_err(|e| e.to_string()));
    }
}

