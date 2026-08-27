//! Character set (SCS) support: G0..G3 slots, GL/GR, and the DEC Special
//! Graphics table. Port of ghostty `src/terminal/charsets.zig`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Ascii,
    British,
    DecSpecial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    G0,
    G1,
    G2,
    G3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveSlot {
    GL,
    GR,
}

/// Charset state: the four designated slots plus which slot GL/GR reference,
/// and a single-shift override (SS2/SS3) applying to the next char only.
#[derive(Clone, Debug)]
pub struct CharsetState {
    pub charsets: [Charset; 4],
    pub gl: Slot,
    pub gr: Slot,
    pub single_shift: Option<Slot>,
}

impl Default for CharsetState {
    fn default() -> Self {
        Self {
            charsets: [Charset::Utf8, Charset::Utf8, Charset::Utf8, Charset::Utf8],
            gl: Slot::G0,
            gr: Slot::G2,
            single_shift: None,
        }
    }
}

impl CharsetState {
    /// Map an ASCII codepoint through the active (GL) charset. Codepoints
    /// above 0x7f pass through untouched (we are a UTF-8 terminal; GR
    /// charsets are legacy we do not translate).
    #[inline]
    pub fn map_gl(&mut self, cp: u32) -> u32 {
        let slot = self.single_shift.take().unwrap_or(self.gl);
        let set = self.charsets[slot as usize];
        if cp > 0x7f {
            return cp;
        }
        match set {
            Charset::Utf8 | Charset::Ascii => cp,
            Charset::British => {
                if cp == 0x23 {
                    0xa3 // '#' -> '£'
                } else {
                    cp
                }
            }
            Charset::DecSpecial => dec_special(cp),
        }
    }

    /// True when translation could apply (fast-path check: all-UTF8 and no
    /// pending single shift means printing can skip mapping entirely).
    #[inline]
    pub fn needs_mapping(&self) -> bool {
        self.single_shift.is_some()
            || self.charsets[self.gl as usize] != Charset::Utf8
    }
}

/// DEC Special Graphics (https://en.wikipedia.org/wiki/DEC_Special_Graphics),
/// exactly the table from ghostty charsets.zig.
#[inline]
fn dec_special(cp: u32) -> u32 {
    match cp {
        0x60 => 0x25c6, // ◆
        0x61 => 0x2592, // ▒
        0x62 => 0x2409,
        0x63 => 0x240c,
        0x64 => 0x240d,
        0x65 => 0x240a,
        0x66 => 0x00b0, // °
        0x67 => 0x00b1, // ±
        0x68 => 0x2424,
        0x69 => 0x240b,
        0x6a => 0x2518, // ┘
        0x6b => 0x2510, // ┐
        0x6c => 0x250c, // ┌
        0x6d => 0x2514, // └
        0x6e => 0x253c, // ┼
        0x6f => 0x23ba,
        0x70 => 0x23bb,
        0x71 => 0x2500, // ─
        0x72 => 0x23bc,
        0x73 => 0x23bd,
        0x74 => 0x251c, // ├
        0x75 => 0x2524, // ┤
        0x76 => 0x2534, // ┴
        0x77 => 0x252c, // ┬
        0x78 => 0x2502, // │
        0x79 => 0x2264, // ≤
        0x7a => 0x2265, // ≥
        0x7b => 0x03c0, // π
        0x7c => 0x2260, // ≠
        0x7d => 0x00a3, // £
        0x7e => 0x00b7, // ·
        _ => cp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dec_line_drawing() {
        let mut cs = CharsetState::default();
        cs.charsets[Slot::G0 as usize] = Charset::DecSpecial;
        assert_eq!(cs.map_gl('q' as u32), 0x2500);
        assert_eq!(cs.map_gl('x' as u32), 0x2502);
        assert_eq!(cs.map_gl('Z' as u32), 'Z' as u32);
    }

    #[test]
    fn single_shift_applies_once() {
        let mut cs = CharsetState::default();
        cs.charsets[Slot::G2 as usize] = Charset::DecSpecial;
        cs.single_shift = Some(Slot::G2);
        assert_eq!(cs.map_gl('q' as u32), 0x2500);
        assert_eq!(cs.map_gl('q' as u32), 'q' as u32);
    }
}
