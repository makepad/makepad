//! What rides in the arena of the font commands and events: field-by-field little-endian
//! records (no padding byte is ever read), and the constants both sides name.

use crate::buffers::CmdWriter;

/// `GlyphRequest::kind`: what to rasterise.
pub mod kind {
    /// What the text draws: the glyph's embedded colour bitmap if it has one, else its outline
    /// as a distance field, upgraded to a multi-channel field in the background when it is large
    /// and simple enough.
    pub const AUTO: u32 = 0;
    /// The stable form: the colour bitmap, else the plain distance field (never upgraded).
    pub const SDF: u32 = 1;
    /// The outline's curves for the slug coverage shader.
    pub const SLUG: u32 = 2;
}

/// `AtlasInfo::kind` / `AtlasEvicted::kind`: which atlas.
pub mod atlas {
    /// The glyph image atlas: BGRA8, four distance-field planes (or colour / MSDF in all four).
    pub const RASTER: u32 = 0;
    /// The slug curve texture: RGBA32F, two texels per curve.
    pub const SLUG: u32 = 1;
}

/// `GlyphEntry::atlas_kind`: what the entry's rectangle holds.
pub mod glyph {
    pub const GRAY: u32 = 0;
    pub const COLOR: u32 = 1;
    pub const MSDF: u32 = 2;
    pub const SLUG: u32 = 3;
    /// the glyph has nothing to draw (a space), or cannot be rasterised
    pub const EMPTY: u32 = 4;
}

/// `FontOpened::status`.
pub mod font_status {
    pub const OPENED: u32 = 0;
    /// the file does not exist or cannot be mapped
    pub const MISSING: u32 = 1;
    /// the bytes are not a font
    pub const BAD: u32 = 2;
}

/// `FontOpen::kind`: where the font comes from.
pub mod source {
    /// a file the host maps itself (a path in the payload)
    pub const PATH: u32 = 0;
    /// the font's bytes in the payload (assets that are not files)
    pub const BYTES: u32 = 1;
}

/// A rasterised glyph as the HAL tells an app. `glyph`, `dpxs_bits` and `kind` repeat the
/// request. A raster entry names a rectangle of the atlas (`rect` = x, y, w, h in pixels, the
/// plane its distance field is in) and the metrics Cx draws with; a slug entry names the curves
/// (`rect[0]` = first curve, `rect[1]` = curve count) and the glyph's box in ems (`origin`,
/// `scale` = width, `size_h` = height). `slot` / `slot_gen` name the atlas slot: an eviction
/// lists slots. `replaces` != 0: this entry upgrades slot `replaces` (a multi-channel field
/// replacing a plain one); the app rewrites every entry it holds for that slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphEntry {
    pub glyph: u32,
    pub dpxs_bits: u32,
    pub kind: u32,
    pub atlas_kind: u32,
    pub plane: u32,
    pub rect: [u32; 4],
    pub padding: u32,
    pub origin: [f32; 2],
    pub scale: f32,
    pub size_h: f32,
    pub slot: u32,
    pub slot_gen: u32,
    pub replaces: u32,
    pub replaces_gen: u32,
}

impl GlyphEntry {
    pub const SIZE: usize = 18 * 4;

    pub fn write(&self, out: &mut Vec<u8>) {
        for w in [self.glyph, self.dpxs_bits, self.kind, self.atlas_kind, self.plane] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for w in self.rect {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&self.padding.to_le_bytes());
        for w in [self.origin[0], self.origin[1], self.scale, self.size_h] {
            out.extend_from_slice(&w.to_bits().to_le_bytes());
        }
        for w in [self.slot, self.slot_gen, self.replaces, self.replaces_gen] {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }

    pub fn read(b: &[u8]) -> GlyphEntry {
        let u = |i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        let f = |i: usize| f32::from_bits(u(i));
        GlyphEntry {
            glyph: u(0),
            dpxs_bits: u(1),
            kind: u(2),
            atlas_kind: u(3),
            plane: u(4),
            rect: [u(5), u(6), u(7), u(8)],
            padding: u(9),
            origin: [f(10), f(11)],
            scale: f(12),
            size_h: f(13),
            slot: u(14),
            slot_gen: u(15),
            replaces: u(16),
            replaces_gen: u(17),
        }
    }
}

pub fn pack_entries(entries: &[GlyphEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.len() * GlyphEntry::SIZE);
    for e in entries {
        e.write(&mut out);
    }
    out
}

/// The entries of a `GlyphsReady` payload (a short payload yields the whole entries in it).
pub fn read_entries(bytes: &[u8], count: u32) -> Vec<GlyphEntry> {
    bytes.chunks_exact(GlyphEntry::SIZE).take(count as usize).map(GlyphEntry::read).collect()
}

/// A requested glyph: its id and the size it is drawn at (pixels per em).
pub fn pack_glyphs(glyphs: &[(u32, f32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(glyphs.len() * 8);
    for (glyph, dpxs) in glyphs {
        out.extend_from_slice(&glyph.to_le_bytes());
        out.extend_from_slice(&dpxs.to_bits().to_le_bytes());
    }
    out
}

pub fn read_glyphs(bytes: &[u8], count: u32) -> Vec<(u32, f32)> {
    bytes
        .chunks_exact(8)
        .take(count as usize)
        .map(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_bits(u32::from_le_bytes([c[4], c[5], c[6], c[7]]))))
        .collect()
}

/// Atlas slots (slot, generation): `GlyphRelease` names the slots an app lets go of, `AtlasEvicted`
/// the slots the HAL took back.
pub fn pack_slots(slots: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(slots.len() * 8);
    for (slot, gen) in slots {
        out.extend_from_slice(&slot.to_le_bytes());
        out.extend_from_slice(&gen.to_le_bytes());
    }
    out
}

pub fn read_slots(bytes: &[u8], count: u32) -> Vec<(u32, u32)> {
    bytes
        .chunks_exact(8)
        .take(count as usize)
        .map(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]), u32::from_le_bytes([c[4], c[5], c[6], c[7]])))
        .collect()
}

/// Where a font comes from, as the app names it.
#[derive(Clone, Copy, Debug)]
pub enum FontSource<'a> {
    Path(&'a str),
    Bytes(&'a [u8]),
}

/// The `FontOpen` payload: the variation coordinates (tag, value) then the path or the bytes.
pub fn pack_font_open(vars: &[(u32, f32)], source: FontSource<'_>) -> (u32, Vec<u8>) {
    let mut out = Vec::new();
    for (tag, value) in vars {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    match source {
        FontSource::Path(p) => {
            out.extend_from_slice(p.as_bytes());
            (self::source::PATH, out)
        }
        FontSource::Bytes(b) => {
            out.extend_from_slice(b);
            (self::source::BYTES, out)
        }
    }
}

/// The parts of a `FontOpen` payload; `None` when it is shorter than its variations.
pub fn parse_font_open(kind: u32, nvars: u32, desc: &[u8]) -> Option<(Vec<(u32, f32)>, FontSource<'_>)> {
    let head = (nvars as usize).checked_mul(8)?;
    let vars = desc.get(..head)?;
    let rest = &desc[head..];
    let vars = vars
        .chunks_exact(8)
        .map(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_bits(u32::from_le_bytes([c[4], c[5], c[6], c[7]]))))
        .collect();
    let source = if kind == self::source::BYTES { FontSource::Bytes(rest) } else { FontSource::Path(std::str::from_utf8(rest).ok()?) };
    Some((vars, source))
}

impl CmdWriter {
    /// Opens a font for the session under the app's own id `font`.
    pub fn open_font(&mut self, font: u32, index: u32, vars: &[(u32, f32)], source: FontSource<'_>) {
        let (kind, desc) = pack_font_open(vars, source);
        self.font_open(font, index, kind, vars.len() as u32, &desc);
    }

    /// Asks for glyphs of an open font; one batch is one `kind`.
    pub fn request_glyphs(&mut self, font: u32, kind: u32, glyphs: &[(u32, f32)]) {
        self.glyph_request(font, kind, glyphs.len() as u32, &pack_glyphs(glyphs));
    }

    /// Lets go of atlas slots the app no longer draws from.
    pub fn release_glyphs(&mut self, slots: &[(u32, u32)]) {
        self.glyph_release(slots.len() as u32, &pack_slots(slots));
    }

    /// Names the atlas slots the app drew from recently (see `GlyphTouch`).
    pub fn touch_glyphs(&mut self, slots: &[(u32, u32)]) {
        self.glyph_touch(slots.len() as u32, &pack_slots(slots));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip() {
        let e = GlyphEntry {
            glyph: 7,
            dpxs_bits: 24.5f32.to_bits(),
            kind: kind::AUTO,
            atlas_kind: glyph::MSDF,
            plane: 2,
            rect: [1, 2, 3, 4],
            padding: 4,
            origin: [-1.5, 2.25],
            scale: 32.0,
            size_h: 0.5,
            slot: 9,
            slot_gen: 3,
            replaces: 5,
            replaces_gen: 1,
        };
        let bytes = pack_entries(&[e, e]);
        assert_eq!(bytes.len(), 2 * GlyphEntry::SIZE);
        assert_eq!(read_entries(&bytes, 2), vec![e, e]);
        assert_eq!(read_entries(&bytes, 1), vec![e]);
    }

    #[test]
    fn font_open_round_trip() {
        let (k, desc) = pack_font_open(&[(0x77676874, 600.0)], FontSource::Path("/fonts/a.ttf"));
        let (vars, source) = parse_font_open(k, 1, &desc).unwrap();
        assert_eq!(vars, vec![(0x77676874, 600.0)]);
        assert!(matches!(source, FontSource::Path("/fonts/a.ttf")));
        assert!(parse_font_open(k, 9, &desc).is_none());
    }
}
