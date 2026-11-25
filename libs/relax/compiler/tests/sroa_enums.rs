// Enum payloads with overlapping field kinds and returned tuples copied through a local; an enum
// built across match arms and copied out whole, each arm leaving the other variants' bytes
// unwritten; a `MaybeUninit` copied before it is written. Run with `makepad test`; this also runs
// as an ordinary rustc test.
#[inline(always)]
fn option_payload(i: u32) -> Option<f64> {
    if i % 3 == 0 { None } else { Some(i as f64 + 0.5) }
}

#[inline(always)]
fn result_payload(i: u32) -> Result<u32, (u8, u8)> {
    if i % 2 == 0 { Ok(3 * i + 1) } else { Err((i as u8, (31 - i) as u8)) }
}

#[inline(never)]
fn returned_pair(i: u32) -> (u32, Option<char>) {
    let ch = match i % 4 {
        0 => Some('A'),
        1 => Some('\u{03bb}'),
        2 => Some('\u{1f642}'),
        _ => None,
    };
    (5 * i + 7, ch)
}

#[inline(always)]
fn pair_through_slot(i: u32) -> (u32, Option<char>) {
    let mut slot = returned_pair(i);
    slot.0 += 1;
    slot
}

#[inline(never)]
fn enum_checksum(n: u32) -> u64 {
    let mut checksum = 0;
    for i in 0..n {
        checksum += match option_payload(i) {
            Some(x) => (2.0 * x) as u64,
            None => 11,
        };
        checksum += match result_payload(i) {
            Ok(x) => x as u64,
            Err((a, b)) => ((a as u64) << 8) | b as u64,
        };
        let pair = pair_through_slot(i);
        checksum += pair.0 as u64;
        checksum += match pair.1 {
            Some(ch) => ch as u64,
            None => 13,
        };
    }
    checksum
}

#[test]
fn sroa_enum_and_sret_checksum() {
    // i = 0..31: Option contributes 1024 - 341 + 11*11 = 804.
    // Result contributes 736 + 256*256 + 240 = 66512.
    // Tuple integers contribute 5*496 + 8*32 = 2736; characters contribute
    // 8*(65 + 955 + 128578 + 13) = 1036888. Total: 1106940.
    assert_eq!(enum_checksum(std::hint::black_box(32)), 1_106_940);
}

/// Variants that lay their fields out differently: a byte at 1 and a u32 at 4, two words at 8,
/// a word and a u32.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    Short { op: &'static str, why: &'static str },
    Missing { kind: u8, id: u32 },
    Stalled { op: &'static str, rounds: u32 },
}

#[inline(never)]
fn fault_of(i: u32) -> Result<f64, Fault> {
    match i % 4 {
        0 => Err(Fault::Short { op: "span", why: "few knots" }),
        1 => Err(Fault::Missing { kind: (i % 7) as u8, id: i * 3 }),
        2 => Err(Fault::Stalled { op: "solve", rounds: i }),
        _ => Ok(i as f64 * 0.5),
    }
}

/// The error goes on whole through `?`.
#[inline(always)]
fn fault_read(i: u32) -> Result<f64, Fault> {
    let x = fault_of(i)?;
    Ok(x + 1.0)
}

#[inline(never)]
fn fault_checksum(n: u32) -> u64 {
    let mut sum = 0u64;
    for i in 0..n {
        sum += match fault_read(i) {
            Ok(x) => x as u64,
            Err(Fault::Short { op, why }) => (op.len() * 100 + why.len()) as u64,
            Err(Fault::Missing { kind, id }) => kind as u64 * 1000 + id as u64,
            Err(Fault::Stalled { op, rounds }) => op.len() as u64 + rounds as u64 * 7,
        };
    }
    sum
}

/// A copy of uninitialised bytes, then reads only of what was written.
#[inline(never)]
fn uninit_reads(n: u32) -> u64 {
    use std::mem::MaybeUninit;
    let mut sum = 0u64;
    for i in 0..n {
        let blank: MaybeUninit<[u64; 3]> = MaybeUninit::uninit();
        let mut buf = blank;
        let p = buf.as_mut_ptr() as *mut u64;
        unsafe {
            p.write(i as u64);
            if i % 2 == 0 {
                p.add(2).write(5);
            }
            sum += p.read() + if i % 2 == 0 { p.add(2).read() } else { 1 };
        }
    }
    sum
}

#[test]
fn sroa_partly_written_enums_and_uninit_reads() {
    // i % 4 == 0: 409 each, 8 of them; == 1: kind * 1000 + 3i, kinds sum to 22 and the i to 120;
    // == 2: 5 + 7i, the i sum to 128; == 3: i / 2 + 1 truncated, 2 + 4 + .. + 16. Total 26640.
    assert_eq!(fault_checksum(std::hint::black_box(32)), 26_640, "an enum built across match arms, copied out whole");
    // the i sum to 496, the even ones add 5 and the odd ones 1: 592
    assert_eq!(uninit_reads(std::hint::black_box(32)), 592, "a MaybeUninit copied before its write");
}
