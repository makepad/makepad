//! Which tile of a named tileset each cell draws with.
//!
//! The rule that makes a plateau look drawn rather than diced: a cell whose
//! terrain must edge-match asks for the piece whose own neighbourhood in the
//! ARTWORK matches its neighbourhood in the MAP, and every cell of one
//! connected blob picks from the same seeded style. Lift that away and a
//! cliff line becomes confetti — which is exactly what the D2K painter was
//! written to avoid, so this is that painter, generalised.
//!
//! A `TileSet` holds opaque `u32` ids. What an id MEANS is the producer's
//! business: an index into a template table, a frame in a learned tile bank,
//! a rect in an atlas. The picker never looks inside one.

use crate::rng::{hash2, Rng};
use crate::{RtsMap, Terrain};
use std::collections::BTreeMap;

pub const MASK_N: u8 = 1;
pub const MASK_E: u8 = 2;
pub const MASK_S: u8 = 4;
pub const MASK_W: u8 = 8;
pub const MASK_ALL: u8 = MASK_N | MASK_E | MASK_S | MASK_W;

/// Terrains that share a mask group are ONE blob as far as edge matching is
/// concerned. A plateau top and its cliff rim are the same raised ground, so
/// the top must not pick edge art just because a rim cell sits beside it.
pub fn mask_group(terrain: Terrain) -> u8 {
    match terrain {
        Terrain::Cliff | Terrain::Plateau => 1,
        Terrain::Water => 2,
        Terrain::Resource => 3,
        Terrain::Road => 4,
        Terrain::Clear | Terrain::Rough | Terrain::Shore => 0,
    }
}

/// The tiles one art pack offers, indexed the way the picker asks for them.
#[derive(Clone, Debug, Default)]
pub struct TileSet {
    masked: BTreeMap<(Terrain, u8), Vec<u32>>,
    singles: BTreeMap<Terrain, Vec<u32>>,
}

impl TileSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// A piece that is drawn for `terrain` when its neighbourhood is `mask`
    /// — an authored edge or corner.
    pub fn push_masked(&mut self, terrain: Terrain, mask: u8, id: u32) {
        self.masked.entry((terrain, mask & MASK_ALL)).or_default().push(id);
    }

    /// A piece that stands on its own: the one-cell variants a full interior
    /// uses, and the fallback for anything with no authored edge.
    pub fn push_single(&mut self, terrain: Terrain, id: u32) {
        self.singles.entry(terrain).or_default().push(id);
    }

    pub fn is_empty(&self) -> bool {
        self.masked.is_empty() && self.singles.is_empty()
    }

    pub fn covers(&self, terrain: Terrain) -> bool {
        self.singles.contains_key(&terrain)
            || self.masked.keys().any(|(candidate, _)| *candidate == terrain)
    }

    /// Every terrain the set can draw at all.
    pub fn terrains(&self) -> Vec<Terrain> {
        let mut out: Vec<Terrain> = self.singles.keys().copied().collect();
        for (terrain, _) in self.masked.keys() {
            if !out.contains(terrain) {
                out.push(*terrain);
            }
        }
        out.sort();
        out
    }

    /// Does the set have an authored piece for exactly this edge?
    pub fn has_masked(&self, terrain: Terrain, mask: u8) -> bool {
        self.masked.get(&(terrain, mask & MASK_ALL)).is_some_and(|pieces| !pieces.is_empty())
    }

    fn candidates(&self, terrain: Terrain, mask: u8, directional: bool) -> Option<&Vec<u32>> {
        if directional && mask != MASK_ALL {
            if let Some(found) = self.masked.get(&(terrain, mask)) {
                if !found.is_empty() {
                    return Some(found);
                }
            }
        }
        if let Some(found) = self.singles.get(&terrain) {
            if !found.is_empty() {
                return Some(found);
            }
        }
        // Last resort: any authored piece of this terrain, whatever its edge.
        self.masked
            .iter()
            .find(|((candidate, _), pieces)| *candidate == terrain && !pieces.is_empty())
            .map(|(_, pieces)| pieces)
    }
}

/// Which of a cell's four neighbours share its mask group. Off-map counts as
/// different, so a blob that runs to the edge still gets an edge piece.
pub fn cell_mask(terrain: &[Terrain], w: usize, h: usize, x: i32, y: i32) -> u8 {
    let at = |x: i32, y: i32| -> Option<Terrain> {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            None
        } else {
            terrain.get(y as usize * w + x as usize).copied()
        }
    };
    let Some(here) = at(x, y) else { return 0 };
    let group = mask_group(here);
    let same = |dx: i32, dy: i32| at(x + dx, y + dy).map(|t| mask_group(t) == group).unwrap_or(false);
    (if same(0, -1) { MASK_N } else { 0 })
        | (if same(1, 0) { MASK_E } else { 0 })
        | (if same(0, 1) { MASK_S } else { 0 })
        | (if same(-1, 0) { MASK_W } else { 0 })
}

/// `cell_mask` for a whole map.
pub fn neighbour_mask(map: &RtsMap, x: i32, y: i32) -> u8 {
    cell_mask(&map.terrain, map.width as usize, map.height as usize, x, y)
}

/// For every cell of a directional group, the index of its blob's first cell.
/// One blob, one seeded style — the difference between a drawn plateau and a
/// patchwork of unrelated frames.
pub fn component_anchors(terrain: &[Terrain], w: usize, h: usize) -> Vec<usize> {
    let count = terrain.len().min(w * h);
    let mut anchors = vec![usize::MAX; terrain.len()];
    for start in 0..count {
        let group = mask_group(terrain[start]);
        if anchors[start] != usize::MAX || group == 0 {
            continue;
        }
        anchors[start] = start;
        let mut pending = vec![start];
        while let Some(at) = pending.pop() {
            let (x, y) = ((at % w) as i32, (at / w) as i32);
            for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let next = ny as usize * w + nx as usize;
                if next < count && anchors[next] == usize::MAX && mask_group(terrain[next]) == group {
                    anchors[next] = start;
                    pending.push(next);
                }
            }
        }
    }
    anchors
}

/// One tile id per cell, or `None` where the set can draw nothing for that
/// terrain (the producer then falls back to whatever it uses for "no art").
///
/// This works on a bare terrain grid rather than an `RtsMap` so that a
/// producer with its OWN class vocabulary — the D2K template table's class
/// letters, say — can paint through the same picker by translating letters
/// to `Terrain`, instead of keeping a second copy of the algorithm.
pub fn pick_tiles_for(
    terrain: &[Terrain],
    w: usize,
    h: usize,
    set: &TileSet,
    seed: u32,
) -> Vec<Option<u32>> {
    let anchors = component_anchors(terrain, w, h);
    let mut rng = Rng::new(seed ^ 0x2f31_9b17);
    let mut out = Vec::with_capacity(terrain.len());
    for y in 0..h {
        for x in 0..w {
            let at = y * w + x;
            let Some(&here) = terrain.get(at) else {
                out.push(None);
                continue;
            };
            let mask = cell_mask(terrain, w, h, x as i32, y as i32);
            let directional = here.directional();
            let Some(candidates) = set.candidates(here, mask, directional) else {
                out.push(None);
                continue;
            };
            let pick = if directional {
                // Blob-anchored, so the whole plateau agrees on a style —
                // and the SAME cell picks the same tile however many times
                // this runs.
                hash2(seed, anchors[at] as i32, (u32::from(mask) << 8) as i32 ^ here as i32) as usize
            } else {
                rng.next_u32() as usize
            };
            out.push(Some(candidates[pick % candidates.len()]));
        }
    }
    out
}

/// `pick_tiles_for` over a generated map.
pub fn pick_tiles(map: &RtsMap, set: &TileSet, seed: u32) -> Vec<Option<u32>> {
    pick_tiles_for(&map.terrain, map.width as usize, map.height as usize, set, seed)
}

/// Whether a cell used an AUTHORED edge piece rather than a plain interior
/// one — the number the converters report as "multi-cell templates used".
pub fn masked_pick_count(terrain: &[Terrain], w: usize, h: usize, set: &TileSet) -> usize {
    let mut count = 0;
    for y in 0..h {
        for x in 0..w {
            let at = y * w + x;
            let Some(&here) = terrain.get(at) else { continue };
            if !here.directional() {
                continue;
            }
            let mask = cell_mask(terrain, w, h, x as i32, y as i32);
            if mask != MASK_ALL && set.has_masked(here, mask) {
                count += 1;
            }
        }
    }
    count
}
