// Integer Display, Debug, hexadecimal, octal and binary formatting for all integer types.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::fmt as sf;

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
    fn text(&mut self, s: &str) {
        self.bytes(&(s.len() as u64).to_le_bytes());
        self.bytes(s.as_bytes());
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const NCASES: usize = 19;

fn real_case<T>(i: usize, v: T, w: usize) -> String
where
    T: sf::Display + sf::Debug + sf::LowerHex + sf::UpperHex + sf::Octal + sf::Binary,
{
    match i {
        0 => format!("{}", v),
        1 => format!("{:w$}", v, w = w),
        2 => format!("{:+}", v),
        3 => format!("{:0w$}", v, w = w),
        4 => format!("{:x}", v),
        5 => format!("{:#x}", v),
        6 => format!("{:X}", v),
        7 => format!("{:#010X}", v),
        8 => format!("{:o}", v),
        9 => format!("{:#o}", v),
        10 => format!("{:b}", v),
        11 => format!("{:#w$b}", v, w = w),
        12 => format!("{:?}", v),
        13 => format!("{:x?}", v),
        14 => format!("{:#X?}", v),
        15 => format!("{:<w$}", v, w = w),
        16 => format!("{:-^w$}", v, w = w),
        17 => format!("{:+0w$}", v, w = w),
        _ => format!("{:w$.3}", v, w = w),
    }
}

fn check_all<T>(tag: &str, vals: &[T])
where
    T: Copy + sf::Display + sf::Debug + sf::LowerHex + sf::UpperHex + sf::Octal + sf::Binary,
{
    for i in 0..NCASES {
        let mut d = D::new();
        for &v in vals {
            for w in [0usize, 3, 12, 45] {
                d.text(&real_case(i, v, w));
                d.1 += 1;
            }
        }
        d.line(&format!("int {} case {}", tag, i));
    }
}

#[test]
fn a01_ints_match_real() {
    let mut r = Rng(12345);
    let mut u64s: Vec<u64> = vec![0, 1, 9, 10, 99, 100, u64::MAX, u64::MAX / 2, 1 << 63, 255, 256];
    for _ in 0..20 {
        let x = r.next();
        u64s.push(x >> r.below(64));
    }
    let i8s: Vec<i8> = u64s.iter().map(|&x| x as i8).chain([i8::MIN, i8::MAX, -1]).collect();
    let i16s: Vec<i16> = u64s.iter().map(|&x| x as i16).chain([i16::MIN, i16::MAX, -1]).collect();
    let i32s: Vec<i32> = u64s.iter().map(|&x| x as i32).chain([i32::MIN, i32::MAX, -1]).collect();
    let i64s: Vec<i64> = u64s.iter().map(|&x| x as i64).chain([i64::MIN, i64::MAX, -1]).collect();
    let isizes: Vec<isize> = u64s.iter().map(|&x| x as isize).collect();
    let u8s: Vec<u8> = u64s.iter().map(|&x| x as u8).collect();
    let u16s: Vec<u16> = u64s.iter().map(|&x| x as u16).collect();
    let u32s: Vec<u32> = u64s.iter().map(|&x| x as u32).collect();
    let usizes: Vec<usize> = u64s.iter().map(|&x| x as usize).collect();
    let mut u128s: Vec<u128> = vec![0, u128::MAX, 1 << 127, 10_000_000_000_000_000_000, 9_999_999_999_999_999_999];
    let mut i128s: Vec<i128> = vec![0, i128::MIN, i128::MAX, -1, -10_000_000_000_000_000_000];
    for _ in 0..5 {
        let x = ((r.next() as u128) << 64) | r.next() as u128;
        let s = r.below(128) as u32;
        u128s.push(x >> s);
        i128s.push((x as i128) >> s);
    }
    check_all("i8", &i8s);
    check_all("i16", &i16s);
    check_all("i32", &i32s);
    check_all("i64", &i64s);
    check_all("isize", &isizes);
    check_all("u8", &u8s);
    check_all("u16", &u16s);
    check_all("u32", &u32s);
    check_all("u64", &u64s);
    check_all("usize", &usizes);
    check_all("u128", &u128s);
    check_all("i128", &i128s);
}
