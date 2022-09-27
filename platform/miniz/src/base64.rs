const BASE64_DEC: [u8; 256] = [64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 62, 64, 62, 64, 63, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 64, 64, 64, 0, 64, 64, 64, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 64, 64, 64, 64, 63, 64, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,];
pub const BASE64_STANDARD: [u8; 64] = [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x2B, 0x2F];
pub const BASE64_URL_SAFE: [u8; 64] = [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x2D, 0x5F];

pub fn base64_encode(inp: &[u8], table: &[u8; 64]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < inp.len() { // hop over in chunks of 3 bytes outputting 4 chars
        out.push(table[(inp[i + 0] >> 2) as usize]);
        out.push(table[((inp[i + 0] & 0x3) << 4 | inp[i + 1] >> 4) as usize]);
        out.push(table[((inp[i + 1] & 0xf) << 2 | inp[i + 2] >> 6) as usize]);
        out.push(table[((inp[i + 2] & 0x3f)) as usize]);
        i += 3;
    }
    let bytes_left = inp.len() - i;
    if bytes_left == 1 {
        out.push(table[(inp[i + 0] >> 2) as usize]);
        out.push(table[((inp[i + 0] & 0x3) << 4) as usize]);
    }
    else if bytes_left == 2 {
        out.push(table[(inp[i + 0] >> 2) as usize]);
        out.push(table[((inp[i + 0] & 0x3) << 4 | inp[i + 1] >> 4) as usize]);
        out.push(table[((inp[i + 1] & 0xf) << 2) as usize]);
    }
    let end_pad = 3 - inp.len() % 3; // max 3
    if end_pad == 1 {
        out.push('=' as u8);
    }
    else if end_pad == 2 {
        out.push('=' as u8);
        out.push('=' as u8);
    }
    out
}

pub fn base64_decode(inp: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out = Vec::new();
    if inp.len() & 3 != 0 { // base64 should be padded to 4 char chunks
        return Err(())
    }
    for i in (0..inp.len()).step_by(4) {
        let b0 = BASE64_DEC[inp[i + 0] as usize];
        let b1 = BASE64_DEC[inp[i + 1] as usize];
        let b2 = BASE64_DEC[inp[i + 2] as usize];
        let b3 = BASE64_DEC[inp[i + 3] as usize];
        if b0 == 64 || b1 == 64 || b2 == 64 || b3 == 64 {
            return Err(()) // invalid character used
        }
        out.push((b0 << 2) | (b1 >> 4));
        out.push((b1 & 0xf) << 4 | (b2 >> 2));
        if inp[i + 2] != '=' as u8 { // double == at the end skips last byte
            out.push(((b2 & 0x3) << 6) | b3);
        }
    }
    Ok(out)
}