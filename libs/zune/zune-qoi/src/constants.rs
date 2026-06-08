pub const QOI_OP_INDEX: u8 = 0x00;
// 00xxxxxx
pub const QOI_OP_DIFF: u8 = 0x40;
// 01xxxxxx
pub const QOI_OP_LUMA: u8 = 0x80;
// 10xxxxxx
pub const QOI_OP_RUN: u8 = 0xc0;
// 11xxxxxx
pub const QOI_OP_RGB: u8 = 0xfe;
// 11111110
pub const QOI_OP_RGBA: u8 = 0xff; // 11111111

pub const QOI_MASK_2: u8 = 0xc0; // (11)000000

pub const QOI_MAGIC: u32 = u32::from_be_bytes(*b"qoif");
pub const QOI_HEADER_SIZE: usize = 14;
pub const QOI_PADDING: usize = 8;
