// diff_fast corpus program: `str` / byte-slice / byte-array equality against literals and constants
// (`match s { "lit" => .. }`, `b"..."` patterns, `const` patterns, `==` / `assert_eq!` on &str): literal
// lengths 0..=24 around the inline word compares (1, 2, 4, 8 bytes, two overlapping words up to 16) and the
// call beyond them, inputs of equal length differing in the first, a middle or the last byte, unknown
// lengths on both sides. The first round runs the quick register allocation (O3 not yet installed).
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

const KEY: &str = "transform";
const BKEY: &[u8] = b"\x00\xffmid\x80";

#[inline(never)]
fn kw(s: &str) -> u32 {
    match s {
        "" => 1,
        "a" => 2,
        "if" => 3,
        "for" => 4,
        "else" => 5,
        "match" => 6,
        "return" => 7,
        "default" => 8,
        "position" => 9,
        KEY => 10,
        "background" => 12,
        "border_width" => 13,
        "draw_bg.color" => 14,
        "letter_spacing" => 15,
        "text_wrap_width" => 16,
        "line_spacing_abc" => 17,
        "seventeen_bytes_x" => 18,
        "twenty_four_bytes_long__" => 19,
        _ => 0,
    }
}
#[inline(never)]
fn bytes_pat(b: &[u8]) -> u32 {
    match b {
        b"" => 1,
        b"\xff" => 2,
        b"GET" => 3,
        b"HEAD" => 4,
        BKEY => 5,
        b"\x89PNG\r\n\x1a\n" => 6,
        b"Content-Length" => 7,
        b"Transfer-Encoding" => 8,
        _ => 0,
    }
}
#[inline(never)]
fn arr_pat(b: &[u8; 4]) -> u32 {
    match b {
        b"RIFF" => 1,
        b"OggS" => 2,
        b"\0\0\0\0" => 3,
        _ => 0,
    }
}
#[inline(never)]
fn arr_pat3(b: &[u8; 3]) -> u32 {
    match b {
        b"ID3" => 1,
        b"\xff\xfb\x90" => 2,
        _ => 0,
    }
}
#[inline(never)]
fn arr_pat16(b: &[u8; 16]) -> u32 {
    match b {
        b"0123456789abcdef" => 1,
        b"0123456789abcdeF" => 2,
        b"X123456789abcdef" => 3,
        _ => 0,
    }
}
#[inline(never)]
fn eq_dyn(a: &str, b: &str) -> bool { a == b }
#[inline(never)]
fn ne_dyn(a: &str, b: &str) -> bool { a != b }

fn variants(s: &str) -> Vec<String> {
    let mut v = vec![s.to_string()];
    let b = s.as_bytes();
    for at in [0, b.len() / 2, b.len().wrapping_sub(1)] {
        if at < b.len() {
            let mut m = b.to_vec();
            m[at] = if m[at] == b'z' { b'y' } else { b'z' };
            v.push(String::from_utf8(m).unwrap());
        }
    }
    v.push(format!("{}x", s));
    if !s.is_empty() { v.push(s[..s.len() - 1].to_string()); }
    v
}

fn work() {
    let words = ["", "a", "if", "for", "else", "match", "return", "default", "position", "transform", "background",
        "border_width", "draw_bg.color", "letter_spacing", "text_wrap_width", "line_spacing_abc", "seventeen_bytes_x",
        "twenty_four_bytes_long__", "b", "of", "fur", "elsa", "matcH"];
    let mut sum = 0u64;
    let mut line = String::new();
    for w in words.iter() {
        for v in variants(w) {
            let s = std::hint::black_box(v.as_str());
            let k = kw(s);
            let p = bytes_pat(s.as_bytes());
            sum = sum.wrapping_mul(31).wrapping_add(k as u64 * 7 + p as u64);
            line.push_str(&format!("{}{} ", k, p));
        }
    }
    d!("kw {} {}", sum, line);
    let raw: [&[u8]; 12] = [b"", b"\xff", b"\xfe", b"GET", b"GEt", b"HEAD", b"\x00\xffmid\x80", b"\x00\xffmid\x81",
        b"\x89PNG\r\n\x1a\n", b"\x89PNG\r\n\x1a\x0b", b"Transfer-Encoding", b"Transfer-EncodinG"];
    let r: Vec<u32> = raw.iter().map(|b| bytes_pat(std::hint::black_box(b))).collect();
    d!("bytes {:?}", r);
    let a4: [[u8; 4]; 5] = [*b"RIFF", *b"RIFx", *b"xIFF", *b"OggS", [0; 4]];
    let r: Vec<u32> = a4.iter().map(|b| arr_pat(std::hint::black_box(b))).collect();
    d!("arr4 {:?}", r);
    let a3: [[u8; 3]; 4] = [*b"ID3", *b"ID4", *b"\xff\xfb\x90", *b"\xff\xfb\x91"];
    let r: Vec<u32> = a3.iter().map(|b| arr_pat3(std::hint::black_box(b))).collect();
    d!("arr3 {:?}", r);
    let a16: [[u8; 16]; 4] = [*b"0123456789abcdef", *b"0123456789abcdeF", *b"X123456789abcdef", *b"0123456x89abcdef"];
    let r: Vec<u32> = a16.iter().map(|b| arr_pat16(std::hint::black_box(b))).collect();
    d!("arr16 {:?}", r);
    let mut eqs = String::new();
    for a in words.iter() {
        for b in words.iter() {
            let (x, y) = (std::hint::black_box(*a), std::hint::black_box(*b));
            eqs.push(if eq_dyn(x, y) { '1' } else if ne_dyn(x, y) { '0' } else { '?' });
        }
    }
    d!("eq {}", eqs);
    let owned = String::from("transform");
    assert_eq!(owned.as_str(), KEY);
    d!("const {}", kw(std::hint::black_box(&owned)));
}
