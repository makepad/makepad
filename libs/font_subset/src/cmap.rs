//! `cmap`: the kept characters as a Windows BMP subtable (format 4) and,
//! when a character lies past the BMP or format 4 cannot hold them all, a
//! Windows full-repertoire subtable (format 12), which the shaper prefers.

use std::collections::BTreeMap;

/// Format 4 segments: a run of consecutive characters either with one
/// delta (`glyph = char + delta`) or listed in the glyph id array.
struct Segment {
    start: u16,
    end: u16,
    delta: u16,
    glyphs: Option<Vec<u16>>,
}

fn segments(map: &BTreeMap<u32, u16>) -> Vec<Segment> {
    let bmp: Vec<(u16, u16)> = map
        .iter()
        .filter(|(c, _)| **c < 0xFFFF)
        .map(|(c, g)| (*c as u16, *g))
        .collect();
    let mut segments = Vec::new();
    let mut i = 0;
    while i < bmp.len() {
        // A run of consecutive characters.
        let mut j = i + 1;
        while j < bmp.len() && bmp[j].0 == bmp[j - 1].0 + 1 {
            j += 1;
        }
        let run = &bmp[i..j];
        let delta = |&(c, g): &(u16, u16)| g.wrapping_sub(c);
        let mut pieces = Vec::new();
        let mut k = 0;
        while k < run.len() {
            let mut l = k + 1;
            while l < run.len() && delta(&run[l]) == delta(&run[k]) {
                l += 1;
            }
            pieces.push(Segment {
                start: run[k].0,
                end: run[l - 1].0,
                delta: delta(&run[k]),
                glyphs: None,
            });
            k = l;
        }
        // One listed segment costs 8 bytes plus 2 per character; each delta
        // piece costs 8.
        if pieces.len() > 1 && 8 + 2 * run.len() < 8 * pieces.len() {
            segments.push(Segment {
                start: run[0].0,
                end: run[run.len() - 1].0,
                delta: 0,
                glyphs: Some(run.iter().map(|&(_, g)| g).collect()),
            });
        } else {
            segments.extend(pieces);
        }
        i = j;
    }
    segments
}

fn format4(mut segments: Vec<Segment>) -> (Vec<u8>, bool) {
    let size = |segments: &[Segment]| {
        16 + 8 * (segments.len() + 1)
            + segments.iter().map(|s| s.glyphs.as_ref().map_or(0, |g| 2 * g.len())).sum::<usize>()
    };
    let mut complete = true;
    while size(&segments) > 0xFFFF {
        segments.pop();
        complete = false;
    }
    segments.push(Segment {
        start: 0xFFFF,
        end: 0xFFFF,
        delta: 1,
        glyphs: None,
    });
    let count = segments.len() as u16;
    let mut entry_selector = 0u16;
    while (2u32 << entry_selector) <= count as u32 {
        entry_selector += 1;
    }
    let search_range = 2u16 << entry_selector;
    let mut out = Vec::with_capacity(size(&segments[..segments.len() - 1]));
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // length, set below
    out.extend_from_slice(&0u16.to_be_bytes()); // language
    out.extend_from_slice(&(count * 2).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(count * 2 - search_range).to_be_bytes());
    for s in &segments {
        out.extend_from_slice(&s.end.to_be_bytes());
    }
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved pad
    for s in &segments {
        out.extend_from_slice(&s.start.to_be_bytes());
    }
    for s in &segments {
        out.extend_from_slice(&s.delta.to_be_bytes());
    }
    let mut array_len = 0usize;
    for (i, s) in segments.iter().enumerate() {
        let range_offset = match &s.glyphs {
            Some(glyphs) => {
                let offset = 2 * (segments.len() - i) + 2 * array_len;
                array_len += glyphs.len();
                offset as u16
            }
            None => 0,
        };
        out.extend_from_slice(&range_offset.to_be_bytes());
    }
    for s in &segments {
        for g in s.glyphs.iter().flatten() {
            out.extend_from_slice(&g.to_be_bytes());
        }
    }
    let length = out.len() as u16;
    crate::sfnt::put_u16(&mut out, 2, length);
    (out, complete)
}

fn format12(map: &BTreeMap<u32, u16>) -> Vec<u8> {
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    for (&c, &g) in map {
        match groups.last_mut() {
            // Consecutive characters with consecutive glyphs extend a group.
            Some((start, end, start_glyph)) if *end + 1 == c && *start_glyph + (c - *start) == g as u32 => {
                *end = c;
            }
            _ => groups.push((c, c, g as u32)),
        }
    }
    let mut out = Vec::with_capacity(16 + 12 * groups.len());
    out.extend_from_slice(&12u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&((16 + 12 * groups.len()) as u32).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&(groups.len() as u32).to_be_bytes());
    for (start, end, glyph) in groups {
        out.extend_from_slice(&start.to_be_bytes());
        out.extend_from_slice(&end.to_be_bytes());
        out.extend_from_slice(&glyph.to_be_bytes());
    }
    out
}

/// The cmap table for `map` (character to glyph id, glyph 0 excluded).
pub(crate) fn build(map: &BTreeMap<u32, u16>) -> Vec<u8> {
    let (bmp, complete) = format4(segments(map));
    let full = (!complete || map.keys().any(|&c| c > 0xFFFF)).then(|| format12(map));
    let records = 1 + full.is_some() as usize;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(records as u16).to_be_bytes());
    let bmp_at = 4 + 8 * records;
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(bmp_at as u32).to_be_bytes());
    if full.is_some() {
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&10u16.to_be_bytes());
        out.extend_from_slice(&((bmp_at + bmp.len()) as u32).to_be_bytes());
    }
    out.extend_from_slice(&bmp);
    if let Some(full) = full {
        out.extend_from_slice(&full);
    }
    out
}
