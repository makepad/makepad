//! Item variation stores cut to the rows in use, and `HVAR` rebuilt for the
//! kept glyphs on such a store.

use {
    crate::{
        graph::{Graph, Id, W},
        sfnt::{u16_at, u32_at},
        SubsetError,
    },
    std::collections::{BTreeMap, BTreeSet},
};

/// The store at `at` with only the `used` `(outer, inner)` rows, its outer
/// and inner indices renumbered; `None` when no row is used.
pub(crate) fn subset(
    d: &[u8],
    at: usize,
    used: &BTreeSet<(u16, u16)>,
    g: &mut Graph,
) -> Result<(Option<Id>, BTreeMap<(u16, u16), (u16, u16)>), SubsetError> {
    let mut remap = BTreeMap::new();
    if used.is_empty() {
        return Ok((None, remap));
    }
    let regions = at + u32_at(d, at + 2)? as usize;
    let axis_count = u16_at(d, regions)? as usize;
    let region_count = u16_at(d, regions + 2)? as usize;
    let mut r = W::new();
    r.bytes(d.get(regions..regions + 4 + 6 * axis_count * region_count).ok_or(SubsetError::Malformed("variation regions truncated"))?);
    let regions = g.add(r, 1);
    let data_count = u16_at(d, at + 6)?;
    let mut by_outer: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
    for &(outer, inner) in used {
        by_outer.entry(outer).or_default().push(inner);
    }
    let mut store = W::new();
    store.u16(1).off32(Some(regions));
    let mut datas = Vec::new();
    for (outer, inners) in by_outer {
        if outer >= data_count {
            return Err(SubsetError::Malformed("variation index outside the store"));
        }
        let data = at + u32_at(d, at + 8 + 4 * outer as usize)? as usize;
        let items = u16_at(d, data)?;
        let word_count = u16_at(d, data + 2)?;
        let index_count = u16_at(d, data + 4)? as usize;
        let words = (word_count & 0x7FFF) as usize;
        let row = if word_count & 0x8000 != 0 {
            4 * words + 2 * (index_count - words.min(index_count))
        } else {
            2 * words + (index_count - words.min(index_count))
        };
        let rows = data + 6 + 2 * index_count;
        let new_outer = datas.len() as u16;
        let mut w = W::new();
        w.u16(inners.len() as u16).u16(word_count).u16(index_count as u16);
        w.bytes(d.get(data + 6..rows).ok_or(SubsetError::Malformed("variation data truncated"))?);
        for (new_inner, &inner) in inners.iter().enumerate() {
            if inner >= items {
                return Err(SubsetError::Malformed("variation index outside the store"));
            }
            let at = rows + row * inner as usize;
            w.bytes(d.get(at..at + row).ok_or(SubsetError::Malformed("variation data truncated"))?);
            remap.insert((outer, inner), (new_outer, new_inner as u16));
        }
        datas.push(g.add(w, 1));
    }
    store.u16(datas.len() as u16);
    for id in datas {
        store.off32(Some(id));
    }
    Ok((Some(g.add(store, 1)), remap))
}

/// A delta-set index map: `(outer, inner)` for each glyph.
struct IndexMap<'a> {
    data: &'a [u8],
    count: usize,
    entry_size: usize,
    inner_bits: u32,
}

impl<'a> IndexMap<'a> {
    fn read(d: &'a [u8], at: usize) -> Result<Self, SubsetError> {
        let format = *d.get(at).ok_or(SubsetError::Malformed("index map truncated"))?;
        let entry_format = *d.get(at + 1).ok_or(SubsetError::Malformed("index map truncated"))?;
        let (count, start) = if format == 0 { (u16_at(d, at + 2)? as usize, at + 4) } else { (u32_at(d, at + 2)? as usize, at + 6) };
        let entry_size = ((entry_format >> 4) & 3) as usize + 1;
        Ok(Self {
            data: d.get(start..start + count * entry_size).ok_or(SubsetError::Malformed("index map truncated"))?,
            count,
            entry_size,
            inner_bits: (entry_format & 0xF) as u32 + 1,
        })
    }

    fn get(&self, glyph: u16) -> (u16, u16) {
        if self.count == 0 {
            return (0, glyph);
        }
        let i = (glyph as usize).min(self.count - 1) * self.entry_size;
        let value = self.data[i..i + self.entry_size].iter().fold(0u32, |v, b| v << 8 | *b as u32);
        ((value >> self.inner_bits) as u16, (value & ((1 << self.inner_bits) - 1)) as u16)
    }
}

fn bits(v: u16) -> u32 {
    (16 - v.leading_zeros()).max(1)
}

fn write_map(entries: &[(u16, u16)]) -> W {
    let inner_bits = entries.iter().map(|e| bits(e.1)).max().unwrap_or(1);
    let outer_bits = entries.iter().map(|e| bits(e.0)).max().unwrap_or(1);
    let entry_size = (inner_bits + outer_bits).div_ceil(8).max(1) as usize;
    let mut w = W::new();
    if entries.len() <= 0xFFFF {
        w.u8(0).u8((((entry_size - 1) << 4) as u8) | (inner_bits - 1) as u8).u16(entries.len() as u16);
    } else {
        w.u8(1).u8((((entry_size - 1) << 4) as u8) | (inner_bits - 1) as u8).u32(entries.len() as u32);
    }
    for &(outer, inner) in entries {
        let value = (outer as u32) << inner_bits | inner as u32;
        w.bytes(&value.to_be_bytes()[4 - entry_size..]);
    }
    w
}

/// `HVAR` for the kept glyphs: a store of only their rows and explicit
/// maps covering glyph ids below `glyph_count` (glyphs that are gone map to
/// the first row; nothing measures them).
pub(crate) fn hvar(d: &[u8], keep: &BTreeSet<u16>, glyph_count: u16) -> Result<Vec<u8>, SubsetError> {
    let store_at = u32_at(d, 4)? as usize;
    let map_at = |field: usize| -> Result<Option<usize>, SubsetError> {
        Ok(match u32_at(d, field)? {
            0 => None,
            o => Some(o as usize),
        })
    };
    let advance = match map_at(8)? {
        Some(at) => Some(IndexMap::read(d, at)?),
        None => None,
    };
    let sides = [map_at(12)?, map_at(16)?]
        .into_iter()
        .map(|m| m.map(|at| IndexMap::read(d, at)).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let glyphs: Vec<u16> = keep.iter().copied().filter(|&g| g < glyph_count).collect();
    let lookup = |map: &Option<IndexMap>, g: u16| map.as_ref().map_or((0, g), |m| m.get(g));
    let mut used = BTreeSet::new();
    for &g in &glyphs {
        used.insert(lookup(&advance, g));
        for side in sides.iter().flatten() {
            used.insert(side.get(g));
        }
    }
    let mut graph = Graph::new(false);
    let (store, remap) = subset(d, store_at, &used, &mut graph)?;
    let count = glyphs.last().map_or(1, |&g| g as usize + 1);
    let entries = |map: &dyn Fn(u16) -> (u16, u16)| -> Vec<(u16, u16)> {
        (0..count as u16)
            .map(|g| if keep.contains(&g) { remap.get(&map(g)).copied().unwrap_or((0, 0)) } else { (0, 0) })
            .collect()
    };
    let advance_map = graph.add(write_map(&entries(&|g| lookup(&advance, g))), 1);
    let mut w = W::new();
    w.u32(0x0001_0000).off32(store).off32(Some(advance_map));
    for side in &sides {
        let id = side.as_ref().map(|m| graph.add(write_map(&entries(&|g| m.get(g))), 1));
        w.off32(id);
    }
    let root = graph.add(w, 0);
    graph.pack(root)
}
