// Integer matches the backend turns into jump tables (a chain of `x == k` tests with >= 6 cases
// over a dense range): dense and gapped ranges, negative and large keys, values below, inside and
// above the range, u8 / i32 / i64 scrutinees, arms sharing a body, called often enough for O3
// (= rustc --test).
#[inline(never)]
fn dense(x: u32) -> u32 {
    match x {
        0 => 11,
        1 => 22,
        2 => 33,
        3 => 44,
        4 => 55,
        5 => 66,
        6 => 77,
        7 => 88,
        8 | 9 => 99,
        _ => 1,
    }
}
#[inline(never)]
fn gapped(x: i64) -> i64 {
    match x {
        -40 => 1,
        -37 => 2,
        -30 => 3,
        -21 => 4,
        -20 => 5,
        -3 => 6,
        0 => 7,
        5 => 8,
        _ => x * 2,
    }
}
#[inline(never)]
fn bytes(b: u8) -> u64 {
    match b {
        b'a' => 1,
        b'b' => 2,
        b'c' => 3,
        b'd' => 4,
        b'e' => 5,
        b'f' => 6,
        b'g' => 7,
        b'z' => 26,
        _ => 0,
    }
}
#[derive(Clone, Copy)]
enum Op { A, B, C, D, E, F, G, H, I }
#[inline(never)]
fn ops(o: Op, v: i32) -> i32 {
    match o {
        Op::A => v + 1,
        Op::B => v - 1,
        Op::C => v * 3,
        Op::D => v / 2,
        Op::E => v ^ 5,
        Op::F => -v,
        Op::G => v << 2,
        Op::H => v & 7,
        Op::I => 0,
    }
}
#[inline(never)]
fn far(x: i64) -> i64 {
    match x {
        1_000_000_000_000 => 1,
        1_000_000_000_001 => 2,
        1_000_000_000_002 => 3,
        1_000_000_000_003 => 4,
        1_000_000_000_004 => 5,
        1_000_000_000_005 => 6,
        1_000_000_000_007 => 7,
        _ => 9,
    }
}

#[test]
fn jump_tables() {
    let all = [Op::A, Op::B, Op::C, Op::D, Op::E, Op::F, Op::G, Op::H, Op::I];
    let mut h = 0u64;
    for rep in 0..3000u64 {
        for x in 0..80u32 {
            let s = x as i64 - 45;
            h = h.rotate_left(3) ^ dense(x.wrapping_sub(5)) as u64;
            h = h.rotate_left(3) ^ gapped(s) as u64;
            h = h.rotate_left(3) ^ bytes((x + 60) as u8) ^ bytes(x as u8);
            h = h.rotate_left(3) ^ ops(all[(x as usize + rep as usize) % 9], x as i32 - 9) as u64;
            h = h.rotate_left(3) ^ far(1_000_000_000_000 + s / 4) as u64;
            h = h.rotate_left(3) ^ dense(u32::MAX - x) as u64 ^ gapped(i64::MIN + s) as u64;
        }
    }
    println!("| {:016x}", h);
}
