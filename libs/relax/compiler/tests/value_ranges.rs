// Compares that value ranges decide (codegen ranges.rs, O3) and wide loads built from mixed
// narrow ones (opt.rs combine_loads): masked indices into fixed arrays, an index bounded by a
// dominating branch through `x - 1`, `(x + 8) - x`, a u64 made of a u16 and six bytes -- and the
// cases that must keep their checks: a mask wider than the array (must panic), u8 sums that wrap,
// negative signed values, a loop variable at its header. Called often enough for O3;
// output = rustc --test.
const N: usize = 0x8102;
const MAX_LEN: usize = 258;

struct Dict {
    b: [u8; N],
    next: [u16; 0x8000],
}

#[inline(always)]
fn read_u16_le(s: &[u8; N], pos: usize) -> u16 {
    s[pos] as u16 | ((s[pos + 1] as u16) << 8)
}

#[inline(always)]
fn read_u64_le(s: &[u8; N], pos: usize) -> u64 {
    let lo = read_u16_le(s, pos) as u64;
    lo | (s[pos + 2] as u64) << 16 | (s[pos + 3] as u64) << 24 | (s[pos + 4] as u64) << 32 | (s[pos + 5] as u64) << 40 | (s[pos + 6] as u64) << 48 | (s[pos + 7] as u64) << 56
}

#[inline(never)]
fn probe(d: &Dict, start: usize, match_len: u32, c01: u16) -> (u32, usize) {
    let mut probe_pos = start & 0x7fff;
    let mut hits = 0u32;
    for _ in 0..3 {
        let next = d.next[probe_pos] as usize;
        if next == 0 || match_len as usize - 1 >= MAX_LEN {
            return (hits, probe_pos);
        }
        probe_pos = next & 0x7fff;
        if read_u16_le(&d.b, probe_pos + match_len as usize - 1) == c01 {
            hits += 1;
        }
    }
    (hits, probe_pos)
}

#[inline(never)]
fn words(d: &Dict, p: usize, q: usize) -> u64 {
    let a = p & 0x7fff;
    let b = q & 0x7fff;
    let x = u64::from_le_bytes(d.b[a..a + 8].try_into().unwrap());
    let y = read_u64_le(&d.b, b);
    x ^ y.rotate_left(7)
}

// a mask wider than the array: the check stays and fires for big x
#[inline(never)]
fn wide_mask(d: &Dict, x: usize) -> u8 {
    d.b[x & 0xffff]
}

// u8 arithmetic wraps: (a + b) < 200 is not decided by the operands being small
#[inline(never)]
fn wrap8(a: u8, b: u8) -> u32 {
    let s = a.wrapping_add(b);
    if s < 200 { 1 } else { 2 }
}

// signed: x - 1 may be negative, x < 3 may hold for negative x
#[inline(never)]
fn signed_cmp(x: i32) -> i32 {
    let y = x - 1;
    if y < 3 { y * 2 } else { y + 100 }
}

// a loop variable: its bound at the header comes from the loop, not the entry
#[inline(never)]
fn loop_var(n: usize, d: &Dict) -> u64 {
    let mut i = 0usize;
    let mut s = 0u64;
    while i < n {
        s += d.b[i % N] as u64;
        i += 3;
    }
    s + i as u64
}

// signed division / remainder by powers of two (shift sequences, not idiv): negative values
#[inline(never)]
fn sdiv(x: i32, y: i64) -> i64 {
    (x / 8) as i64 + (x % 1024) as i64 + y / 16 + y % 4096 + (x as i8 % 4) as i64
}

#[test]
fn value_ranges() {
    let mut d = Box::new(Dict { b: [0; N], next: [0; 0x8000] });
    for i in 0..N {
        d.b[i] = (i * 131 + 7) as u8;
    }
    for i in 0..0x8000 {
        d.next[i] = ((i * 7919 + 1) % 0x10000) as u16;
    }
    let mut h = 0u64;
    let mut panics = 0u32;
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for rep in 0..60000usize {
        let (hits, pos) = probe(&d, rep * 31, (rep % 300) as u32 + 1, (rep as u16).wrapping_mul(97));
        h = h.rotate_left(5) ^ hits as u64 ^ pos as u64;
        h = h.wrapping_add(words(&d, rep * 13, rep * 17 + 3));
        if rep % 1000 == 0 {
            let x = rep * 7;
            match std::panic::catch_unwind(|| wide_mask(&d, x)) {
                Ok(v) => h ^= v as u64,
                Err(_) => panics += 1,
            }
        }
        h = h.wrapping_add(wrap8(rep as u8, (rep >> 3) as u8) as u64);
        h = h.wrapping_add(signed_cmp((rep as i32 % 9) - 4) as u64);
        h = h.rotate_left(3) ^ sdiv(rep as i32 * 37 - 1_000_000, (rep as i64 - 30000) * 1_000_003) as u64;
        if rep % 100 == 0 {
            h ^= loop_var(rep % 97, &d);
        }
    }
    std::panic::set_hook(prev);
    println!("| {:016x} {}", h, panics);
}
