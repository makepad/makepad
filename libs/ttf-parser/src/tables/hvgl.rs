//! Apple `hvgl` (Hierarchical Variation Font) table detection.
//!
//! `hvgl` is Apple's proprietary outline format used by iOS/macOS system fonts
//! (the CJK / Devanagari / Arabic / Thai super-font, PingFang, SF, …). Its
//! glyphs are *composite parts* referencing shared shape sub-parts with affine
//! transforms and variation deltas — `ttf_parser` cannot decode them itself.
//!
//! Rendering is delegated to Apple's `libhvf` (linked by the makepad platform
//! layer, which can call Apple libraries — `ttf_parser` is `no_std`). This
//! module only *detects* the table and hands out its raw bytes + version so the
//! platform code can feed them to `HVF_open_part_renderer`.
//!
//! IMPORTANT: unlike ordinary sfnt tables (big-endian), the `hvgl` table is
//! stored **little-endian** — Apple's native in-memory format.
//!
//! Reference: <https://developer.apple.com/documentation/hvf>

#[inline]
fn le_u16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

#[inline]
fn le_u32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A parsed `hvgl` table. Holds the raw bytes for delegation to `libhvf`.
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    data: &'a [u8],
    num_glyphs: u32,
    version_major: u16,
    version_minor: u16,
}

impl<'a> Table<'a> {
    /// Parse the `hvgl` table header (little-endian).
    ///
    /// Header layout (first 20 bytes):
    ///   u16 versionMajor, u16 versionMinor, u32 formatFlags,
    ///   u32 numParts, u32 partIndexOffset, u32 numGlyphs, u32 unused.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let version_major = le_u16(data, 0)?;
        let version_minor = le_u16(data, 2)?;
        let _format_flags = le_u32(data, 4)?;
        let _num_parts = le_u32(data, 8)?;
        let _part_index_offset = le_u32(data, 12)?;
        let num_glyphs = le_u32(data, 16)?;
        Some(Table {
            data,
            num_glyphs,
            version_major,
            version_minor,
        })
    }

    /// The raw `hvgl` table bytes, to hand to `libhvf`'s `HVF_open_part_renderer`.
    #[inline]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Number of renderable glyphs (a prefix of all parts).
    #[inline]
    pub fn num_glyphs(&self) -> u32 {
        self.num_glyphs
    }

    /// `(major, minor)` version of the table (e.g. `(3, 1)`).
    #[inline]
    pub fn version(&self) -> (u16, u16) {
        (self.version_major, self.version_minor)
    }
}
