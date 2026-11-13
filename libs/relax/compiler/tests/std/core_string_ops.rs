// String random mutations and panics, UTF conversions, formatting and iterator traits.
// Differential file: `| ` lines equal under rustc --test and makepad test.

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

fn caught<T: std::fmt::Debug>(d: &mut D, f: impl FnOnce() -> T) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    d.add(&result.is_err());
    if let Ok(value) = result {
        d.add(&value);
    }
}

#[test]
fn a01_random_ops() {
    let mut rng = Rng(21);
    let mut d = D::new();
    for _ in 0..300 {
        let init = random_string(&mut rng, 4);
        let mut s = String::from(init.as_str());
        for _ in 0..20 {
            let op = rng.below(16);
            let len = s.len();
            let i = rng.below(len + 2);
            let j = rng.below(len + 2);
            let piece = PIECES[rng.below(PIECES.len())];
            let ch = piece.chars().next().unwrap();
            match op {
                0 => s.push(ch),
                1 => s.push_str(piece),
                2 => d.add(&s.pop()),
                3 => caught(&mut d, || s.insert(i, ch)),
                4 => caught(&mut d, || s.insert_str(i, piece)),
                5 => caught(&mut d, || s.remove(i)),
                6 => caught(&mut d, || s.truncate(i)),
                7 => s.retain(|c| c != ch),
                8 => caught(&mut d, || s.drain(i..j).collect::<String>()),
                9 => caught(&mut d, || {
                    let mut drain = s.drain(..=i);
                    drain.next_back()
                }),
                10 => caught(&mut d, || s.replace_range(i..j, piece)),
                11 => caught(&mut d, || s.split_off(i)),
                12 => caught(&mut d, || s.extend_from_within(i..j)),
                13 => s += piece,
                14 => s.extend(piece.chars()),
                _ => caught(&mut d, || s[i..j].to_string()),
            }
            d.add(s.as_str());
            d.add(&s.len());
            d.add(&s.is_empty());
        }
        d.add(s.clone().as_str());
        d.add(&format!("{:?}", s));
        d.add(&format!("{}", s));
    }
    d.line("string random");
}

#[test]
fn a02_conversions() {
    let mut rng = Rng(22);
    let mut d = D::new();
    for _ in 0..2_000 {
        let n = rng.below(6);
        let bytes: Vec<u8> = (0..n)
            .map(|_| [b'a', 0xc3, 0xa9, 0xe2, 0x82, 0xac, 0xff, 0x80][rng.below(8)])
            .collect();
        match String::from_utf8(bytes.clone()) {
            Ok(s) => {
                d.add(&true);
                d.add(&s);
            }
            Err(error) => {
                d.add(&false);
                d.add(&error.utf8_error().valid_up_to());
                d.add(&error.utf8_error().error_len());
                d.add(&error.to_string());
                d.add(error.as_bytes());
                d.add(&error.into_utf8_lossy());
            }
        }
        let n = rng.below(5);
        let units: Vec<u16> = (0..n)
            .map(|_| [0x41u16, 0xd834, 0xdd1e, 0xdc00, 0xe9][rng.below(5)])
            .collect();
        match String::from_utf16(&units) {
            Ok(s) => {
                d.add(&true);
                d.add(&s);
            }
            Err(error) => {
                d.add(&false);
                d.add(&error.to_string());
            }
        }
        d.add(&String::from_utf16_lossy(&units));
    }
    d.line("string conversions");
}

#[test]
fn a03_traits_and_iterators() {
    let mut d = D::new();
    let parts = ["ab", "é", "", "𝄞z"];
    let s: String = parts.iter().copied().collect();
    d.add(s.as_str());
    let s: String = "héllo".chars().collect();
    assert_eq!(s.as_str(), "héllo");
    let s: String = ['x', 'ÿ'].iter().collect();
    assert_eq!(s.as_str(), "xÿ");
    let owned: Vec<String> = parts.iter().map(|p| String::from(*p)).collect();
    let s: String = owned.into_iter().collect();
    d.add(s.as_str());
    let s = String::from("ab") + "cd";
    assert_eq!(s.as_str(), "abcd");
    assert!(s == *"abcd");
    assert!(s == "abcd");
    assert!(*"abcd" == s);
    assert!(String::from("a") < String::from("b"));
    d.add(&String::from("é").cmp(&String::from("e")));
    assert_eq!(String::from('€').as_str(), "€");
    let bytes: Vec<u8> = String::from("hi").into();
    assert_eq!(bytes, b"hi");
    d.add(&format!("{:?}", String::from("a\"b\n")));
    let mut s = String::new();
    std::fmt::Write::write_str(&mut s, "x").unwrap();
    std::fmt::Write::write_char(&mut s, 'é').unwrap();
    assert_eq!(s.as_str(), "xé");
    let mut s = String::from("abc");
    s.as_mut_str().make_ascii_uppercase();
    assert_eq!(s.as_str(), "ABC");
    assert_eq!(&s[1..], "BC");
    assert!(s.as_str().parse::<u32>().is_err());
    let drain = s.drain(..1);
    d.add(&format!("{:?}", drain));
    d.line("string traits");
}

