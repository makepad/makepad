fn le16(b: &[u8], i: usize) -> u16 { b[i] as u16 | ((b[i + 1] as u16) << 8) }
fn be16(b: &[u8], i: usize) -> u16 { u16::from_be_bytes([b[i], b[i + 1]]) }
fn le32(b: &[u8], i: usize) -> u32 { u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) }
fn be32(b: &[u8], i: usize) -> u32 { (b[i] as u32) << 24 | (b[i + 1] as u32) << 16 | (b[i + 2] as u32) << 8 | b[i + 3] as u32 }
fn trunc(b: &[u8], i: usize) -> u16 { ((b[i] as u32) | ((b[i + 1] as u32) << 16)) as u16 }
fn le64(b: &[u8; 16], i: usize) -> u64 { u64::from_le_bytes(b[i..i + 8].try_into().unwrap()) }
// Load combining (opt::combine_loads): byte loads assembled little / big endian into 16 / 32 / 64-bit values,
// and one narrowed (the high byte shifted out of a u16: no combining). rustc prints the same hash.
#[test]
fn t() {
    let mut b = [0u8; 40];
    for i in 0..40 { b[i] = (i as u8).wrapping_mul(37) ^ 0x5a; }
    let mut h = 0u64;
    for i in 0..30 {
        h = h.wrapping_mul(31) ^ le16(&b, i) as u64 ^ (be16(&b, i) as u64) << 16 ^ (le32(&b, i) as u64) << 1 ^ (be32(&b, i) as u64) << 7 ^ trunc(&b, i) as u64;
    }
    let c: [u8; 16] = b[..16].try_into().unwrap();
    for i in 0..8 { h ^= le64(&c, i); }
    println!("hash {}", h);
    assert_eq!(h, 14477832544368060906);
}
