// Integer parsing (from_str_radix / str::parse) on random strings over all radixes: values and
// error kinds per type, as real core (including which error wins).
fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

#[test]
fn a01_random() {
    let alphabet = b"0123456789000111999azAZfFgGz+-x ";
    let mut x = 0x9E3779B97F4A7C15u64;
    for _ in 0..3000 {
        let r = next(&mut x);
        let len = (r % 24) as usize;
        let radix = 2 + ((r >> 8) % 35) as u32;
        let mut s = String::new();
        for _ in 0..len {
            let y = next(&mut x);
            s.push(alphabet[(y % alphabet.len() as u64) as usize] as char);
        }
        let a = match u8::from_str_radix(&s, radix) { Ok(v) => format!("{}", v), Err(e) => format!("{:?}", e.kind()) };
        let b = match i16::from_str_radix(&s, radix) { Ok(v) => format!("{}", v), Err(e) => format!("{:?}", e.kind()) };
        let c = match u32::from_str_radix(&s, radix) { Ok(v) => format!("{}", v), Err(e) => format!("{:?}", e.kind()) };
        let d = match i64::from_str_radix(&s, radix) { Ok(v) => format!("{}", v), Err(e) => format!("{:?}", e.kind()) };
        let e = match u64::from_str_radix(&s, radix) { Ok(v) => format!("{}", v), Err(e) => format!("{:?}", e.kind()) };
        println!("| {:?} r{} u8 {} i16 {} u32 {} i64 {} u64 {}", s, radix, a, b, c, d, e);
    }
}

#[test]
fn a02_decimal_edges() {
    for s in ["0", "-0", "+0", "18446744073709551615", "18446744073709551616", "9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809", "0000000000000000000000001", "4294967295", "4294967296", "300x", "x300", "-", "+", ""] {
        println!("| {:?} u64 {:?} i64 {:?} u32 {:?} i32 {:?}", s, s.parse::<u64>(), s.parse::<i64>(), s.parse::<u32>(), s.parse::<i32>());
    }
}
