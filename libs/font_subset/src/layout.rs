//! GPOS pair kerning cut to the keep set without moving a byte: the pair
//! sets of glyphs that are gone are emptied, the remaining pair sets keep
//! only pairs whose second glyph is kept, and the class-kerning rows and
//! columns no kept glyph uses are zeroed. Every offset, count of covered
//! glyphs and class assignment stays where it was, so the table parses and
//! kerns the kept glyphs exactly as before; the cleared bytes cost next to
//! nothing once the font is compressed. The class definitions put the glyphs
//! that are gone in class 0, likewise in place.

use {
    crate::{
        sfnt::{put_u16, u16_at, u32_at},
        SubsetError,
    },
    std::collections::{BTreeMap, BTreeSet},
};

/// `(glyph, coverage index)` for every glyph a coverage table lists.
pub(crate) fn coverage(data: &[u8], at: usize) -> Result<Vec<(u16, u16)>, SubsetError> {
    let count = u16_at(data, at + 2)? as usize;
    let mut out = Vec::new();
    match u16_at(data, at)? {
        1 => {
            for i in 0..count {
                out.push((u16_at(data, at + 4 + 2 * i)?, i as u16));
            }
        }
        2 => {
            for i in 0..count {
                let record = at + 4 + 6 * i;
                let (start, end, index) = (u16_at(data, record)?, u16_at(data, record + 2)?, u16_at(data, record + 4)?);
                for g in start..=end.max(start) {
                    out.push((g, index.wrapping_add(g - start)));
                }
            }
        }
        _ => return Err(SubsetError::Malformed("unknown coverage format")),
    }
    Ok(out)
}

/// The class of `glyph` in a class definition table (0 when unlisted).
pub(crate) fn class_of(data: &[u8], at: usize, glyph: u16) -> Result<u16, SubsetError> {
    match u16_at(data, at)? {
        1 => {
            let start = u16_at(data, at + 2)?;
            let count = u16_at(data, at + 4)?;
            if glyph >= start && glyph - start < count {
                u16_at(data, at + 6 + 2 * (glyph - start) as usize)
            } else {
                Ok(0)
            }
        }
        2 => {
            let count = u16_at(data, at + 2)? as usize;
            for i in 0..count {
                let record = at + 4 + 6 * i;
                if (u16_at(data, record)?..=u16_at(data, record + 2)?).contains(&glyph) {
                    return u16_at(data, record + 4);
                }
            }
            Ok(0)
        }
        _ => Err(SubsetError::Malformed("unknown class definition format")),
    }
}

/// Puts every glyph that is gone in class 0 (the class of unlisted glyphs),
/// in place; the kept glyphs keep their classes.
fn clear_classes(data: &mut [u8], at: usize, keep: &BTreeSet<u16>) -> Result<(), SubsetError> {
    match u16_at(data, at)? {
        1 => {
            let start = u16_at(data, at + 2)?;
            let count = u16_at(data, at + 4)?;
            for i in 0..count {
                if !keep.contains(&start.wrapping_add(i)) {
                    put_u16(data, at + 6 + 2 * i as usize, 0);
                }
            }
        }
        2 => {
            let count = u16_at(data, at + 2)? as usize;
            for i in 0..count {
                let record = at + 4 + 6 * i;
                let (start, end) = (u16_at(data, record)?, u16_at(data, record + 2)?);
                if keep.range(start..=end.max(start)).next().is_none() {
                    put_u16(data, record + 4, 0);
                }
            }
        }
        _ => return Err(SubsetError::Malformed("unknown class definition format")),
    }
    Ok(())
}

fn value_size(format: u16) -> usize {
    (format & 0xFF).count_ones() as usize * 2
}

fn clear(data: &mut [u8], range: std::ops::Range<usize>) -> Result<(), SubsetError> {
    data.get_mut(range)
        .ok_or(SubsetError::Malformed("GPOS record outside the table"))?
        .fill(0);
    Ok(())
}

/// A format 1 pair set: its record size and whether a kept glyph uses it as
/// first glyph. A set may serve several first glyphs and several subtables;
/// it is emptied only when none of them is kept.
#[derive(Default)]
struct PairSet {
    record: usize,
    used: bool,
    mixed_formats: bool,
}

fn pair_pos(
    data: &mut [u8],
    at: usize,
    keep: &BTreeSet<u16>,
    sets: &mut BTreeMap<usize, PairSet>,
) -> Result<(), SubsetError> {
    let format = u16_at(data, at)?;
    let covered = coverage(data, at + u16_at(data, at + 2)? as usize)?;
    let record_values = value_size(u16_at(data, at + 4)?) + value_size(u16_at(data, at + 6)?);
    match format {
        1 => {
            let set_count = u16_at(data, at + 8)?;
            for (glyph, index) in covered {
                if index < set_count {
                    let at = at + u16_at(data, at + 10 + 2 * index as usize)? as usize;
                    let set = sets.entry(at).or_insert_with(|| PairSet {
                        record: 2 + record_values,
                        ..Default::default()
                    });
                    set.used |= keep.contains(&glyph);
                    set.mixed_formats |= set.record != 2 + record_values;
                }
            }
        }
        2 => {
            let class_def_1 = at + u16_at(data, at + 8)? as usize;
            let class_def_2 = at + u16_at(data, at + 10)? as usize;
            let class_1_count = u16_at(data, at + 12)? as usize;
            let class_2_count = u16_at(data, at + 14)? as usize;
            let mut rows = BTreeSet::new();
            for (glyph, _) in covered {
                if keep.contains(&glyph) {
                    rows.insert(class_of(data, class_def_1, glyph)? as usize);
                }
            }
            let mut columns = BTreeSet::from([0usize]);
            for &glyph in keep {
                columns.insert(class_of(data, class_def_2, glyph)? as usize);
            }
            let matrix = at + 16;
            for row in 0..class_1_count {
                let start = matrix + row * class_2_count * record_values;
                for column in 0..class_2_count {
                    if !rows.contains(&row) || !columns.contains(&column) {
                        let cell = start + column * record_values;
                        clear(data, cell..cell + record_values)?;
                    }
                }
            }
            clear_classes(data, class_def_1, keep)?;
            clear_classes(data, class_def_2, keep)?;
        }
        _ => {}
    }
    Ok(())
}

fn prune_pair_set(data: &mut [u8], at: usize, set: &PairSet, keep: &BTreeSet<u16>) -> Result<(), SubsetError> {
    let record = set.record;
    let count = u16_at(data, at)? as usize;
    let records = at + 2..at + 2 + count * record;
    if !set.used {
        return clear(data, at..records.end);
    }
    // Pairs stay sorted by second glyph: move the kept ones up.
    let mut write = records.start;
    for read in records.clone().step_by(record) {
        if keep.contains(&u16_at(data, read)?) {
            data.copy_within(read..read + record, write);
            write += record;
        }
    }
    put_u16(data, at, ((write - records.start) / record) as u16);
    clear(data, write..records.end)
}

/// GPOS with pair kerning cut to `keep` (see the module comment).
pub(crate) fn prune_gpos(gpos: &[u8], keep: &BTreeSet<u16>) -> Result<Vec<u8>, SubsetError> {
    let mut data = gpos.to_vec();
    let lookup_list = u16_at(gpos, 8)? as usize;
    let lookups = u16_at(gpos, lookup_list)? as usize;
    // Subtables shared between lookups are pruned once.
    let mut done = BTreeSet::new();
    let mut sets = BTreeMap::new();
    for i in 0..lookups {
        let lookup = lookup_list + u16_at(gpos, lookup_list + 2 + 2 * i)? as usize;
        let kind = u16_at(gpos, lookup)?;
        let subtables = u16_at(gpos, lookup + 4)? as usize;
        for j in 0..subtables {
            let mut subtable = lookup + u16_at(gpos, lookup + 6 + 2 * j)? as usize;
            let mut kind = kind;
            if kind == 9 {
                kind = u16_at(gpos, subtable + 2)?;
                subtable += u32_at(gpos, subtable + 4)? as usize;
            }
            if kind == 2 && done.insert(subtable) {
                pair_pos(&mut data, subtable, keep, &mut sets)?;
            }
        }
    }
    for (at, set) in sets {
        if !set.mixed_formats {
            prune_pair_set(&mut data, at, &set, keep)?;
        }
    }
    Ok(data)
}
