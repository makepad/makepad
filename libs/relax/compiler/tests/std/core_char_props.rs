// Character properties, mappings, escapes, encodings, digits and parsing.
// Differential file: `| ` lines equal under rustc --test and makepad test.
// All scalar values share one exhaustive test.

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
    fn add<T: Obs + ?Sized>(&mut self, value: &T) {
        value.obs(self);
        self.1 += 1;
    }
    fn chars(&mut self, chars: impl Iterator<Item = char>) {
        for c in chars {
            c.obs(self);
        }
        0x110000u32.obs(self);
        self.1 += 1;
    }
    fn formatted(&mut self, args: std::fmt::Arguments<'_>) {
        std::fmt::write(self, args).unwrap();
        self.bytes(&[255]);
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
trait Obs {
    fn obs(&self, d: &mut D);
}
macro_rules! obs_int {
    ($($t:ty),*) => {$(
        impl Obs for $t {
            fn obs(&self, d: &mut D) {
                d.bytes(&self.to_le_bytes());
            }
        }
    )*};
}
obs_int!(u8, u16, u32, usize);
impl Obs for bool {
    fn obs(&self, d: &mut D) {
        d.bytes(&[*self as u8]);
    }
}
impl Obs for char {
    fn obs(&self, d: &mut D) {
        (*self as u32).obs(d);
    }
}
impl<T: Obs> Obs for Option<T> {
    fn obs(&self, d: &mut D) {
        match self {
            None => d.bytes(&[0]),
            Some(value) => {
                d.bytes(&[1]);
                value.obs(d);
            }
        }
    }
}
impl<T: Obs, E: Obs> Obs for Result<T, E> {
    fn obs(&self, d: &mut D) {
        match self {
            Ok(value) => {
                d.bytes(&[0]);
                value.obs(d);
            }
            Err(error) => {
                d.bytes(&[1]);
                error.obs(d);
            }
        }
    }
}
impl<A: Obs, B: Obs> Obs for (A, B) {
    fn obs(&self, d: &mut D) {
        self.0.obs(d);
        self.1.obs(d);
    }
}
impl<T: Obs> Obs for [T] {
    fn obs(&self, d: &mut D) {
        self.len().obs(d);
        for value in self {
            value.obs(d);
        }
    }
}
impl Obs for str {
    fn obs(&self, d: &mut D) {
        self.len().obs(d);
        d.bytes(self.as_bytes());
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

fn panicked<R>(f: impl FnOnce() -> R) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

#[test]
fn a01_all_code_points() {
    let mut properties = D::new();
    let mut case = D::new();
    let mut escapes = D::new();
    let mut encoding = D::new();
    let mut n = 0;
    for c in (0..=0x10ffffu32).filter_map(char::from_u32) {
        let flags = [
            c.is_alphabetic(), c.is_lowercase(), c.is_uppercase(), c.is_whitespace(),
            c.is_numeric(), c.is_alphanumeric(), c.is_control(), c.is_ascii(),
            c.is_ascii_alphabetic(), c.is_ascii_uppercase(), c.is_ascii_lowercase(),
            c.is_ascii_alphanumeric(), c.is_ascii_digit(), matches!(c, '0'..='7'),
            c.is_ascii_hexdigit(), c.is_ascii_punctuation(), c.is_ascii_graphic(),
            c.is_ascii_whitespace(), c.is_ascii_control(),
        ];
        properties.add(&flags[..]);
        properties.add(&c.to_ascii_uppercase());
        properties.add(&c.to_ascii_lowercase());
        properties.add(&c.len_utf8());
        properties.add(&c.len_utf16());
        case.chars(c.to_lowercase());
        case.chars(c.to_uppercase());
        case.chars(c.to_uppercase().rev());
        case.add(&c.to_uppercase().len());
        case.formatted(format_args!("{}", c.to_lowercase()));
        escapes.chars(c.escape_debug());
        escapes.formatted(format_args!("{}", c.escape_debug()));
        escapes.chars(c.escape_default());
        escapes.chars(c.escape_unicode());
        escapes.add(&c.escape_unicode().len());
        escapes.formatted(format_args!("{:?}", c));
        let mut utf8 = [0u8; 4];
        encoding.add(c.encode_utf8(&mut utf8).as_bytes());
        let mut utf16 = [0u16; 2];
        encoding.add(c.encode_utf16(&mut utf16));
        n += 1;
    }
    assert_eq!(n, 0x110000 - 0x800);
    println!("| char scalar count {n}");
    properties.line("char properties");
    case.line("char case");
    escapes.line("char escapes");
    encoding.line("char encodings");
}

#[test]
fn a02_digits() {
    let mut d = D::new();
    for radix in 2..=36u32 {
        for c in (0..=0x10ffffu32).filter_map(char::from_u32).take(0x3000) {
            d.add(&c.to_digit(radix));
            d.add(&c.is_digit(radix));
        }
        for num in 0..40u32 {
            d.add(&char::from_digit(num, radix));
        }
    }
    d.add(&panicked(|| '1'.to_digit(1)));
    d.add(&panicked(|| '1'.to_digit(37)));
    d.add(&panicked(|| char::from_digit(1, 37)));
    d.line("char digits");
}

#[test]
fn a03_from_u32_and_try_from() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut probes = vec![
        0, 0x7f, 0x80, 0xd7ff, 0xd800, 0xdbff, 0xdc00, 0xdfff,
        0xe000, 0xffff, 0x10000, 0x10ffff, 0x110000, u32::MAX,
    ];
    for _ in 0..1_000 {
        probes.push(rng.next() as u32 % 0x120000);
    }
    let mut d = D::new();
    for u in probes {
        d.add(&char::from_u32(u));
        d.add(&char::try_from(u).is_ok());
    }
    d.add(char::try_from(0xd800u32).unwrap_err().to_string().as_str());
    d.line("char from u32");
}

#[test]
fn a04_encode_short_buffers() {
    let mut d = D::new();
    for c in ['a', 'é', '€', '𝄞'] {
        for n in 0..c.len_utf8() {
            d.add(&panicked(|| {
                let mut buf = vec![0u8; n];
                c.encode_utf8(&mut buf);
            }));
        }
        for n in 0..c.len_utf16() {
            d.add(&panicked(|| {
                let mut buf = vec![0u16; n];
                c.encode_utf16(&mut buf);
            }));
        }
    }
    d.line("char short buffers");
}

#[test]
fn a05_decode_utf16_random() {
    let mut rng = Rng(77);
    let units = [0x41u16, 0xe9, 0x20ac, 0xd834, 0xdd1e, 0xd800, 0xdc00, 0xdfff, 0xffff, 0x0];
    let mut d = D::new();
    for _ in 0..2_000 {
        let n = rng.below(8);
        let v: Vec<_> = (0..n).map(|_| units[rng.below(units.len())]).collect();
        let decoded: Vec<_> = char::decode_utf16(v.iter().copied())
            .map(|r| r.map_err(|e| e.unpaired_surrogate())).collect();
        d.add(&decoded[..]);
        let mut it = char::decode_utf16(v.iter().copied());
        loop {
            d.add(&it.size_hint());
            match it.next() {
                None => break,
                Some(Err(error)) => d.add(error.to_string().as_str()),
                Some(Ok(_)) => {}
            }
        }
    }
    d.line("char decode utf16");
}

#[test]
fn a06_parse_char() {
    let mut d = D::new();
    for s in ["", "a", "ab", "é", "𝄞", "é\u{301}", " "] {
        let result = s.parse::<char>();
        d.add(&result.is_ok());
        match result {
            Ok(c) => d.add(&c),
            Err(error) => d.add(error.to_string().as_str()),
        }
    }
    d.line("char parse");
}

