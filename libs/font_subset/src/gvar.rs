//! `gvar`: the shared tuples kept as they are, each kept glyph's variation
//! data copied verbatim (it is self-relative), every other glyph's emptied,
//! and the offsets rebuilt. Axes pinned at their default lose their tuples:
//! a tuple whose peak lies off the default on a pinned axis contributes
//! nothing there.

use {
    crate::{
        sfnt::{u16_at, u32_at},
        SubsetError,
    },
    std::collections::BTreeSet,
};

const HEADER: usize = 20;

/// The length of packed point numbers at the start of `data`.
fn packed_points_len(data: &[u8]) -> Result<usize, SubsetError> {
    let first = *data.first().ok_or(SubsetError::Malformed("gvar point numbers truncated"))?;
    let (count, mut at) = if first & 0x80 != 0 {
        ((((first & 0x7F) as usize) << 8) | *data.get(1).ok_or(SubsetError::Malformed("gvar point numbers truncated"))? as usize, 2)
    } else {
        (first as usize, 1)
    };
    let mut points = 0;
    while points < count {
        let control = *data.get(at).ok_or(SubsetError::Malformed("gvar point numbers truncated"))?;
        let run = (control & 0x7F) as usize + 1;
        at += 1 + if control & 0x80 != 0 { 2 * run } else { run };
        points += run;
    }
    Ok(at)
}

/// A glyph's variation data without the tuples that peak off the default on
/// a `pinned` axis (empty when none is left).
fn pin(glyph: &[u8], shared: &[u8], axis_count: usize, pinned: &[usize]) -> Result<Vec<u8>, SubsetError> {
    const SHARED_POINT_NUMBERS: u16 = 0x8000;
    const EMBEDDED_PEAK: u16 = 0x8000;
    const INTERMEDIATE: u16 = 0x4000;
    let count_field = u16_at(glyph, 0)?;
    let count = (count_field & 0x0FFF) as usize;
    let data_offset = u16_at(glyph, 2)? as usize;
    let mut header = 4;
    let mut data = data_offset;
    if count_field & SHARED_POINT_NUMBERS != 0 {
        data += packed_points_len(glyph.get(data_offset..).unwrap_or(&[]))?;
    }
    let shared_points = glyph.get(data_offset..data).ok_or(SubsetError::Malformed("gvar data truncated"))?;
    let mut headers = Vec::new();
    let mut chunks = Vec::new();
    let mut kept = 0;
    for _ in 0..count {
        let size = u16_at(glyph, header)? as usize;
        let index = u16_at(glyph, header + 2)?;
        let mut len = 4;
        let peak = if index & EMBEDDED_PEAK != 0 {
            len += 2 * axis_count;
            header + 4
        } else {
            // Shared tuples live in the gvar table, not the glyph: read
            // them through `shared` below.
            usize::MAX
        };
        if index & INTERMEDIATE != 0 {
            len += 4 * axis_count;
        }
        let peak_on = |axis: usize| -> Result<i16, SubsetError> {
            let value = if peak == usize::MAX {
                u16_at(shared, 2 * (axis_count * (index & 0x0FFF) as usize + axis))?
            } else {
                u16_at(glyph, peak + 2 * axis)?
            };
            Ok(value as i16)
        };
        let mut off_default = false;
        for &axis in pinned {
            off_default |= peak_on(axis)? != 0;
        }
        if !off_default {
            headers.extend_from_slice(glyph.get(header..header + len).ok_or(SubsetError::Malformed("gvar tuple truncated"))?);
            chunks.extend_from_slice(glyph.get(data..data + size).ok_or(SubsetError::Malformed("gvar tuple data truncated"))?);
            kept += 1;
        }
        header += len;
        data += size;
    }
    if kept == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(4 + headers.len() + shared_points.len() + chunks.len());
    out.extend_from_slice(&((count_field & !0x0FFF) | kept).to_be_bytes());
    out.extend_from_slice(&((4 + headers.len()) as u16).to_be_bytes());
    out.extend_from_slice(&headers);
    out.extend_from_slice(shared_points);
    out.extend_from_slice(&chunks);
    Ok(out)
}

/// `gvar` for the kept glyphs below `glyph_count_out`, without the tuples
/// of the `pinned` axes (indices into `fvar`'s axes).
pub(crate) fn build(
    gvar: &[u8],
    keep: &BTreeSet<u16>,
    num_glyphs: u16,
    glyph_count_out: u16,
    pinned: &[usize],
) -> Result<Vec<u8>, SubsetError> {
    let axis_count = u16_at(gvar, 4)?;
    let shared_count = u16_at(gvar, 6)?;
    let shared_offset = u32_at(gvar, 8)? as usize;
    let glyph_count = u16_at(gvar, 12)?;
    let flags = u16_at(gvar, 14)?;
    let data_offset = u32_at(gvar, 16)? as usize;
    if glyph_count != num_glyphs {
        return Err(SubsetError::Malformed("gvar glyph count differs from maxp"));
    }
    let long_in = flags & 1 != 0;
    let offset = |i: usize| -> Result<usize, SubsetError> {
        Ok(if long_in {
            u32_at(gvar, HEADER + i * 4)? as usize
        } else {
            u16_at(gvar, HEADER + i * 2)? as usize * 2
        })
    };
    let shared_len = axis_count as usize * shared_count as usize * 2;
    let shared = gvar
        .get(shared_offset..shared_offset + shared_len)
        .ok_or(SubsetError::Malformed("gvar shared tuples outside the table"))?;

    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(glyph_count_out as usize + 1);
    let mut start = offset(0)?;
    for id in 0..glyph_count_out.min(glyph_count) as usize {
        let end = offset(id + 1)?;
        offsets.push(data.len());
        if keep.contains(&(id as u16)) && end > start {
            let glyph = gvar
                .get(data_offset + start..data_offset + end)
                .ok_or(SubsetError::Malformed("gvar glyph data outside the table"))?;
            if pinned.is_empty() {
                data.extend_from_slice(glyph);
            } else {
                data.extend_from_slice(&pin(glyph, shared, axis_count as usize, pinned)?);
            }
            data.resize((data.len() + 1) & !1, 0);
        }
        start = end;
    }
    offsets.push(data.len());

    let long = data.len() > 0x1FFFE;
    let offsets_len = offsets.len() * if long { 4 } else { 2 };
    let shared_at = HEADER + offsets_len;
    let data_at = shared_at + shared.len();
    let mut out = Vec::with_capacity(data_at + data.len());
    out.extend_from_slice(&gvar[..4]);
    out.extend_from_slice(&axis_count.to_be_bytes());
    out.extend_from_slice(&shared_count.to_be_bytes());
    out.extend_from_slice(&(shared_at as u32).to_be_bytes());
    out.extend_from_slice(&glyph_count_out.to_be_bytes());
    out.extend_from_slice(&((flags & !1) | long as u16).to_be_bytes());
    out.extend_from_slice(&(data_at as u32).to_be_bytes());
    for offset in offsets {
        if long {
            out.extend_from_slice(&(offset as u32).to_be_bytes());
        } else {
            out.extend_from_slice(&((offset / 2) as u16).to_be_bytes());
        }
    }
    out.extend_from_slice(shared);
    out.extend_from_slice(&data);
    Ok(out)
}
