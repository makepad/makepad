//! C64 SID tunes (PSID and RSID files): the tune's own 6502 code run on a
//! [`cpu`] in a small C64 ([`machine`]: RAM, the I/O the players touch,
//! the timers and raster that call them), its writes to the SID chip
//! captured with their times, and the three voices read back as notes
//! with SID-style patches ([`convert`]).

pub mod convert;
pub mod cpu;
pub mod machine;
pub mod render;

/// A SID file's header.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// RSID: needs a real C64 environment (its own interrupts).
    pub rsid: bool,
    pub version: u16,
    pub load: u16,
    pub init: u16,
    pub play: u16,
    pub songs: u16,
    /// 1-based.
    pub start_song: u16,
    /// Bit n: song n+1 runs on the CIA timer (else the 50/60 Hz raster).
    pub speed: u32,
    pub name: String,
    pub author: String,
    pub released: String,
    /// NTSC (60 Hz, 1.0227 MHz) rather than PAL (50 Hz, 0.985 MHz).
    pub ntsc: bool,
    /// The 8580 rather than the 6581.
    pub mos8580: bool,
    /// Second and third SID chips' addresses (0: none).
    pub sid2: u16,
    pub sid3: u16,
}

pub struct Tune {
    pub header: Header,
    /// The C64 data, loaded at `header.load`.
    pub data: Vec<u8>,
}

/// Latin-1 text in a 32-byte field.
fn text(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string()
}

pub fn is_sid(bytes: &[u8]) -> bool {
    bytes.len() > 0x76 && (&bytes[..4] == b"PSID" || &bytes[..4] == b"RSID")
}

pub fn read(bytes: &[u8]) -> Result<Tune, String> {
    if !is_sid(bytes) {
        return Err("not a SID file (PSID or RSID)".into());
    }
    let be = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]);
    let version = be(4);
    let offset = be(6) as usize;
    if offset > bytes.len() {
        return Err("SID: the data offset is past the end".into());
    }
    let mut load = be(8);
    let mut data = bytes[offset..].to_vec();
    if load == 0 {
        if data.len() < 2 {
            return Err("SID: no data".into());
        }
        load = u16::from_le_bytes([data[0], data[1]]);
        data.drain(..2);
    }
    let flags = if version >= 2 && bytes.len() >= 0x7C { be(0x76) } else { 0 };
    let page = |b: u8| if b == 0 || b & 1 != 0 { 0 } else { 0xD000 | (b as u16) << 4 };
    let header = Header {
        rsid: &bytes[..4] == b"RSID",
        version,
        load,
        init: match be(0x0A) {
            0 => load,
            a => a,
        },
        play: be(0x0C),
        songs: be(0x0E).max(1),
        start_song: be(0x10).max(1),
        speed: u32::from_be_bytes([bytes[0x12], bytes[0x13], bytes[0x14], bytes[0x15]]),
        name: text(&bytes[0x16..0x36]),
        author: text(&bytes[0x36..0x56]),
        released: text(&bytes[0x56..0x76]),
        ntsc: (flags >> 2) & 3 == 2,
        mos8580: (flags >> 4) & 3 == 2,
        sid2: if version >= 3 && bytes.len() > 0x7A { page(bytes[0x7A]) } else { 0 },
        sid3: if version >= 4 && bytes.len() > 0x7B { page(bytes[0x7B]) } else { 0 },
    };
    Ok(Tune { header, data })
}

/// The year a `released` field names ("1987 Firebird").
pub fn year(released: &str) -> Option<u16> {
    let digits: String = released.chars().take_while(|c| c.is_ascii_digit()).collect();
    match digits.len() {
        4 => digits.parse().ok().filter(|y| (1980..2100).contains(y)),
        _ => None,
    }
}
