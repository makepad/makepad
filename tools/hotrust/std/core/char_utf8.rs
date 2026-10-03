//! UTF-8 encoding of a scalar value into a buffer; returns the length.
pub fn encode(c: u32, buf: &mut [u8; 4]) -> usize {
    if c < 0x80 {
        buf[0] = c as u8;
        1
    } else if c < 0x800 {
        buf[0] = (0xc0 | (c >> 6)) as u8;
        buf[1] = (0x80 | (c & 0x3f)) as u8;
        2
    } else if c < 0x10000 {
        buf[0] = (0xe0 | (c >> 12)) as u8;
        buf[1] = (0x80 | ((c >> 6) & 0x3f)) as u8;
        buf[2] = (0x80 | (c & 0x3f)) as u8;
        3
    } else {
        buf[0] = (0xf0 | (c >> 18)) as u8;
        buf[1] = (0x80 | ((c >> 12) & 0x3f)) as u8;
        buf[2] = (0x80 | ((c >> 6) & 0x3f)) as u8;
        buf[3] = (0x80 | (c & 0x3f)) as u8;
        4
    }
}
