// Enum payloads with overlapping field kinds and returned tuples copied through a local.
// Run with `makepad test`; this also runs as an ordinary rustc test.
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
