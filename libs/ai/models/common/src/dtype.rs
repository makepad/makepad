/// IEEE f16 bit pattern → f32. Shared by H3 / TRELLIS / BiRefNet weight
/// decoders (and their validate bins) so those families do not depend on
/// each other just for a conversion helper.
pub fn f16_word_to_f32(word: u16) -> f32 {
    let sign = ((word >> 15) & 1) as u32;
    let exp = ((word >> 10) & 0x1f) as u32;
    let frac = (word & 0x3ff) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            // subnormal
            let mut exp32 = 127 - 15 + 1;
            let mut frac32 = frac;
            while frac32 & 0x400 == 0 {
                frac32 <<= 1;
                exp32 -= 1;
            }
            frac32 &= 0x3ff;
            (sign << 31) | ((exp32 as u32) << 23) | (frac32 << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}
