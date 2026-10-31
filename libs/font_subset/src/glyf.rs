//! `glyf` and `loca`: splitting the glyph data, the composite-component
//! closure, dropping hinting instructions and rebuilding both tables with
//! every glyph outside the keep set empty (its id stays, its data is gone).

use {
    crate::{sfnt::u16_at, SubsetError},
    std::collections::BTreeSet,
};

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;

/// Every glyph's bytes, by glyph id, as `loca` cuts `glyf`.
pub(crate) fn split<'a>(
    glyf: &'a [u8],
    loca: &[u8],
    long_offsets: bool,
    num_glyphs: u16,
) -> Result<Vec<&'a [u8]>, SubsetError> {
    let offset = |i: usize| -> Result<usize, SubsetError> {
        Ok(if long_offsets {
            crate::sfnt::u32_at(loca, i * 4)? as usize
        } else {
            u16_at(loca, i * 2)? as usize * 2
        })
    };
    let mut glyphs = Vec::with_capacity(num_glyphs as usize);
    let mut start = offset(0)?;
    for i in 0..num_glyphs as usize {
        let end = offset(i + 1)?;
        // A glyph whose range is inverted or runs past `glyf` has no outline
        // for the parser either; keep it empty rather than fail the font.
        glyphs.push(glyf.get(start..end).unwrap_or(&[]));
        start = end;
    }
    Ok(glyphs)
}

/// Walks a composite glyph's component records: for each, its glyph id, and
/// where the records end (the instructions follow when the flag says so).
fn components(glyph: &[u8]) -> Result<(Vec<u16>, usize, bool), SubsetError> {
    let mut ids = Vec::new();
    let mut at = 10;
    let mut has_instructions = false;
    loop {
        let flags = u16_at(glyph, at)?;
        ids.push(u16_at(glyph, at + 2)?);
        at += 4;
        at += if flags & ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
        if flags & WE_HAVE_A_SCALE != 0 {
            at += 2;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            at += 4;
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            at += 8;
        }
        has_instructions |= flags & WE_HAVE_INSTRUCTIONS != 0;
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    Ok((ids, at, has_instructions))
}

fn is_composite(glyph: &[u8]) -> bool {
    glyph.len() >= 10 && i16::from_be_bytes([glyph[0], glyph[1]]) < 0
}

/// Adds every component of every composite glyph in `keep`, recursively.
pub(crate) fn component_closure(glyphs: &[&[u8]], keep: &mut BTreeSet<u16>) -> Result<(), SubsetError> {
    let mut todo: Vec<u16> = keep.iter().copied().collect();
    while let Some(id) = todo.pop() {
        let Some(glyph) = glyphs.get(id as usize) else {
            continue;
        };
        if !is_composite(glyph) {
            continue;
        }
        for component in components(glyph)?.0 {
            if (component as usize) < glyphs.len() && keep.insert(component) {
                todo.push(component);
            }
        }
    }
    Ok(())
}

/// A glyph without its TrueType instructions; the outline bytes are copied
/// unchanged.
fn strip_instructions(glyph: &[u8]) -> Result<Vec<u8>, SubsetError> {
    if glyph.len() < 10 {
        return Ok(glyph.to_vec());
    }
    let contours = i16::from_be_bytes([glyph[0], glyph[1]]);
    if contours >= 0 {
        let length_at = 10 + 2 * contours as usize;
        let length = u16_at(glyph, length_at)? as usize;
        let rest = glyph
            .get(length_at + 2 + length..)
            .ok_or(SubsetError::Malformed("glyph instructions past the glyph"))?;
        let mut out = Vec::with_capacity(glyph.len() - length);
        out.extend_from_slice(&glyph[..length_at]);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(rest);
        Ok(out)
    } else {
        let (_, end, has_instructions) = components(glyph)?;
        let mut out = glyph
            .get(..end)
            .ok_or(SubsetError::Malformed("composite glyph truncated"))?
            .to_vec();
        if has_instructions {
            // Clear the flag on every record (only the last one may carry it).
            let mut at = 10;
            loop {
                let flags = u16_at(&out, at)?;
                crate::sfnt::put_u16(&mut out, at, flags & !WE_HAVE_INSTRUCTIONS);
                at += 4 + if flags & ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
                if flags & WE_HAVE_A_SCALE != 0 {
                    at += 2;
                } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                    at += 4;
                } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
                    at += 8;
                }
                if flags & MORE_COMPONENTS == 0 {
                    break;
                }
            }
        }
        Ok(out)
    }
}

/// The new `glyf` and `loca`, and whether `loca` is in the long format.
pub(crate) fn build(
    glyphs: &[&[u8]],
    keep: &BTreeSet<u16>,
    strip_hinting: bool,
) -> Result<(Vec<u8>, Vec<u8>, bool), SubsetError> {
    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(glyphs.len() + 1);
    for (id, glyph) in glyphs.iter().enumerate() {
        offsets.push(glyf.len());
        if !keep.contains(&(id as u16)) || glyph.is_empty() {
            continue;
        }
        if strip_hinting {
            glyf.extend_from_slice(&strip_instructions(glyph)?);
        } else {
            glyf.extend_from_slice(glyph);
        }
        // Short `loca` offsets count words, so every glyph starts even.
        glyf.resize((glyf.len() + 1) & !1, 0);
    }
    offsets.push(glyf.len());
    let long = glyf.len() > 0x1FFFE;
    let mut loca = Vec::with_capacity(offsets.len() * if long { 4 } else { 2 });
    for offset in offsets {
        if long {
            loca.extend_from_slice(&(offset as u32).to_be_bytes());
        } else {
            loca.extend_from_slice(&((offset / 2) as u16).to_be_bytes());
        }
    }
    Ok((glyf, loca, long))
}
