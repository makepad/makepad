//! Doom doors as an animating state of the exported map.
//!
//! A Doom door is a sector authored CLOSED: its ceiling sits on its floor, so
//! a static export bakes a solid slab across the doorway and a walker gets
//! stuck in it. The engine opens it by raising the ceiling to the lowest
//! neighbouring ceiling minus 4 units (`EV_DoDoor`).
//!
//! The exporter therefore:
//! - bakes every door sector at its OPEN ceiling, so the level mesh is
//!   walkable everywhere the game is walkable, and
//! - moves the visible door leaf (the upper-texture band the neighbour sees
//!   across the doorway) into its own glTF node `door_N`, authored CLOSED,
//!   whose rest transform is the OPEN pose and which carries a `door_N`
//!   translation animation from closed to open.
//!
//! Because Doom's open height is `min(neighbour ceiling) - 4`, the leaf's
//! bottom 4 units land exactly on the lintel when open: no hole, no
//! coplanar double surface.

use std::collections::{BTreeMap, BTreeSet};

/// Linedef specials that open a sector as a door. Manual (DR/D1) specials
/// act on the linedef's BACK sector; tagged ones (S1/SR/W1/WR) act on every
/// sector carrying the linedef's tag. Locked/keyed and blazing doors open
/// exactly the same way — a walker must not be stuck behind a red door.
pub(crate) const DOOR_SPECIALS: &[u16] = &[
    // Manual: open-wait-close and open-stay, normal and blazing, plus the
    // keyed variants (blue 26/32, yellow 27/34, red 28/33).
    1, 26, 27, 28, 31, 32, 33, 34, 117, 118, //
    // Switched / walk-over / gun, tagged: open, open-stay, close, raise.
    2, 3, 4, 16, 29, 42, 46, 50, 61, 63, 75, 76, 86, 90, 103, 105, 106, 107, 108, 109, 110, 111,
    112, 113, 114, 115, 116, 133, 135, 137,
];

/// Doom's `VDOORSPEED` headroom: an opening door stops 4 map units below the
/// lowest neighbouring ceiling.
pub(crate) const DOOR_HEADROOM: i16 = 4;

/// `ML_SECRET`: the line is drawn as a wall on the automap. A door behind it
/// is a hidden door — it opens exactly like any other, and a walker that
/// cannot find it is stuck in a courtyard forever.
pub(crate) const ML_SECRET: u16 = 0x0020;

/// One door sector of a map.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DoomDoor {
    pub sector: usize,
    /// Triggered by a line the automap draws as a wall (`ML_SECRET`).
    pub secret: bool,
    /// `blue` / `yellow` / `red` when the door is locked.
    pub key: Option<&'static str>,
    /// Ceiling as authored (closed), in map units.
    pub closed_ceil: i16,
    /// Ceiling the engine raises to, in map units.
    pub open_ceil: i16,
    /// Sector centre (map units) — the anchor position a catalog consumer
    /// gets without parsing the GLB.
    pub centre: [f32; 2],
}

/// Every door of a map, indexed both by order and by sector.
#[derive(Clone, Debug, Default)]
pub(crate) struct DoomDoors {
    pub doors: Vec<DoomDoor>,
    /// Sector index → position in `doors`.
    pub by_sector: BTreeMap<usize, usize>,
}

impl DoomDoors {
    pub fn door_of(&self, sector: usize) -> Option<&DoomDoor> {
        self.by_sector.get(&sector).map(|&i| &self.doors[i])
    }

    pub fn index_of(&self, sector: usize) -> Option<usize> {
        self.by_sector.get(&sector).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.doors.is_empty()
    }

    /// Ceiling heights with every door raised to its open position — the
    /// array the static level mesh is built from.
    pub fn open_ceilings(&self, sec_ceil: &[i16]) -> Vec<i16> {
        let mut out = sec_ceil.to_vec();
        for d in &self.doors {
            if let Some(c) = out.get_mut(d.sector) {
                *c = d.open_ceil;
            }
        }
        out
    }
}

/// Find every door sector and the height it opens to.
///
/// `sec_ceil`/`sec_floor`/`sec_tag` are the raw sector fields; `line_*` the
/// raw linedef fields. Sectors that would not actually move (open height at
/// or below the authored ceiling) are not doors — a keyed archway that is
/// already open must stay static.
pub(crate) fn detect_doors(
    linedefs: &[u8],
    sidedefs: &[u8],
    sectors: &[u8],
    verts: &[[f32; 2]],
) -> DoomDoors {
    let n_line = linedefs.len() / 14;
    let n_side = sidedefs.len() / 30;
    let n_sec = sectors.len() / 26;
    let n_vert = verts.len();
    if n_sec == 0 || n_line == 0 {
        return DoomDoors::default();
    }
    let sec_floor: Vec<i16> = (0..n_sec).map(|i| i16_le(sectors, i * 26)).collect();
    let sec_ceil: Vec<i16> = (0..n_sec).map(|i| i16_le(sectors, i * 26 + 2)).collect();
    let sec_tag: Vec<u16> = (0..n_sec).map(|i| u16_le(sectors, i * 26 + 24)).collect();
    let sector_of_side = |snum: u16| -> Option<usize> {
        if snum == 0xFFFF || snum as usize >= n_side {
            return None;
        }
        let sec = u16_le(sidedefs, snum as usize * 30 + 28) as usize;
        (sec < n_sec).then_some(sec)
    };

    // Which sectors a door special names, and which of those are hidden.
    let mut candidates: BTreeSet<usize> = BTreeSet::new();
    let mut secret: BTreeSet<usize> = BTreeSet::new();
    let mut keyed: BTreeMap<usize, &'static str> = BTreeMap::new();
    for li in 0..n_line {
        let lo = li * 14;
        let special = u16_le(linedefs, lo + 6);
        if !DOOR_SPECIALS.contains(&special) {
            continue;
        }
        let hidden = u16_le(linedefs, lo + 4) & ML_SECRET != 0;
        let key = super::doom::doom_door_key(special);
        let tag = u16_le(linedefs, lo + 8);
        let mut mark = |sec: usize| {
            candidates.insert(sec);
            if hidden {
                secret.insert(sec);
            }
            if let Some(k) = key {
                keyed.entry(sec).or_insert(k);
            }
        };
        if tag == 0 {
            // Manual door: the sector behind the line.
            if let Some(sec) = sector_of_side(u16_le(linedefs, lo + 12)) {
                mark(sec);
            }
        } else {
            for (sec, t) in sec_tag.iter().enumerate() {
                if *t == tag {
                    mark(sec);
                }
            }
        }
    }
    if candidates.is_empty() {
        return DoomDoors::default();
    }

    // Neighbours (via two-sided linedefs) give both the open height and the
    // door's outline.
    let mut neighbours: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut outline: BTreeMap<usize, Vec<[f32; 2]>> = BTreeMap::new();
    for li in 0..n_line {
        let lo = li * 14;
        let front = sector_of_side(u16_le(linedefs, lo + 10));
        let back = sector_of_side(u16_le(linedefs, lo + 12));
        let v1 = u16_le(linedefs, lo) as usize;
        let v2 = u16_le(linedefs, lo + 2) as usize;
        for (a, b) in [(front, back), (back, front)] {
            let Some(a) = a else { continue };
            if !candidates.contains(&a) {
                continue;
            }
            if v1 < n_vert && v2 < n_vert {
                let pts = outline.entry(a).or_default();
                pts.push(verts[v1]);
                pts.push(verts[v2]);
            }
            if let Some(b) = b {
                if b != a {
                    neighbours.entry(a).or_default().insert(b);
                }
            }
        }
    }

    let mut doors = Vec::new();
    let mut by_sector = BTreeMap::new();
    for sec in candidates {
        let Some(near) = neighbours.get(&sec) else {
            continue;
        };
        let Some(lowest) = near.iter().map(|&n| sec_ceil[n]).min() else {
            continue;
        };
        let open = lowest.saturating_sub(DOOR_HEADROOM).max(sec_floor[sec]);
        // Only sectors that actually travel are doors.
        if open <= sec_ceil[sec] {
            continue;
        }
        let pts = outline.get(&sec).cloned().unwrap_or_default();
        let centre = centre_of(&pts);
        by_sector.insert(sec, doors.len());
        doors.push(DoomDoor {
            sector: sec,
            secret: secret.contains(&sec),
            key: keyed.get(&sec).copied(),
            closed_ceil: sec_ceil[sec],
            open_ceil: open,
            centre,
        });
    }
    DoomDoors { doors, by_sector }
}

fn centre_of(pts: &[[f32; 2]]) -> [f32; 2] {
    if pts.is_empty() {
        return [0.0, 0.0];
    }
    let (mut min, mut max) = (pts[0], pts[0]);
    for p in pts {
        min[0] = min[0].min(p[0]);
        min[1] = min[1].min(p[1]);
        max[0] = max[0].max(p[0]);
        max[1] = max[1].max(p[1]);
    }
    [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]
}

/// The moving geometry of one door, authored in its CLOSED pose. The node
/// that carries it rests at `travel_y` metres above this.
#[derive(Clone, Debug, Default)]
pub(crate) struct DoorLeaf {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Baked sector light of the side that paints the leaf.
    pub colors: Vec<[f32; 3]>,
    /// Metres the leaf rises to reach the open pose.
    pub travel_y: f32,
    /// Sector centre in engine space (metres, Y-up), at the closed ceiling.
    pub centre: [f32; 3],
    pub closed_y: f32,
    pub open_y: f32,
}

impl DoorLeaf {
    pub fn is_empty(&self) -> bool {
        self.indices.len() < 3 || self.positions.is_empty()
    }
}

/// Where the wall emitter sends a door's moving band instead of the static
/// mesh. `leaves` is parallel to `doors.doors`.
pub(crate) struct DoorSink<'a> {
    pub doors: &'a DoomDoors,
    pub leaves: &'a mut Vec<DoorLeaf>,
}

fn i16_le(b: &[u8], o: usize) -> i16 {
    if o + 2 > b.len() {
        return 0;
    }
    i16::from_le_bytes([b[o], b[o + 1]])
}

fn u16_le(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rooms (sector 0 and 2) joined through a closed door sector 1.
    /// Sector heights: rooms floor 0 ceiling 128, door floor 0 ceiling 0.
    pub(crate) fn two_rooms_and_a_door() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<[f32; 2]>) {
        let mut sectors = Vec::new();
        for (floor, ceil, tag) in [(0i16, 128i16, 0u16), (0, 0, 0), (0, 128, 0)] {
            sectors.extend_from_slice(&floor.to_le_bytes());
            sectors.extend_from_slice(&ceil.to_le_bytes());
            sectors.extend_from_slice(b"FLOOR0\0\0");
            sectors.extend_from_slice(b"CEIL1\0\0\0");
            sectors.extend_from_slice(&0i16.to_le_bytes());
            sectors.extend_from_slice(&0i16.to_le_bytes());
            sectors.extend_from_slice(&tag.to_le_bytes());
        }
        // Sidedefs: 0 → sector 0, 1 → sector 1, 2 → sector 2, 3 → sector 1.
        let mut sidedefs = Vec::new();
        for sec in [0u16, 1, 2, 1] {
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(b"DOORTRAK");
            sidedefs.extend_from_slice(b"-\0\0\0\0\0\0\0");
            sidedefs.extend_from_slice(b"-\0\0\0\0\0\0\0");
            sidedefs.extend_from_slice(&sec.to_le_bytes());
        }
        // Two two-sided linedefs, both with door special 1 (manual DR).
        let mut linedefs = Vec::new();
        for (v1, v2, right, left) in [(0u16, 1u16, 0u16, 1u16), (2, 3, 2, 3)] {
            linedefs.extend_from_slice(&v1.to_le_bytes());
            linedefs.extend_from_slice(&v2.to_le_bytes());
            linedefs.extend_from_slice(&0x0004u16.to_le_bytes()); // two-sided
            linedefs.extend_from_slice(&1u16.to_le_bytes()); // special: DR door
            linedefs.extend_from_slice(&0u16.to_le_bytes()); // tag
            linedefs.extend_from_slice(&right.to_le_bytes());
            linedefs.extend_from_slice(&left.to_le_bytes());
        }
        let verts = vec![[0.0, 0.0], [64.0, 0.0], [0.0, 64.0], [64.0, 64.0]];
        (linedefs, sidedefs, sectors, verts)
    }

    #[test]
    fn a_closed_door_sector_opens_to_the_lowest_neighbour_minus_four() {
        let (linedefs, sidedefs, sectors, verts) = two_rooms_and_a_door();
        let doors = detect_doors(&linedefs, &sidedefs, &sectors, &verts);
        assert_eq!(doors.doors.len(), 1, "{doors:?}");
        let d = &doors.doors[0];
        assert_eq!(d.sector, 1);
        assert_eq!(d.closed_ceil, 0);
        assert_eq!(d.open_ceil, 124, "128 - 4 headroom");
        assert_eq!(doors.index_of(1), Some(0));
        assert_eq!(doors.open_ceilings(&[128, 0, 128]), vec![128, 124, 128]);
    }

    #[test]
    fn a_keyed_door_names_the_key_it_wants() {
        let (mut linedefs, sidedefs, sectors, verts) = two_rooms_and_a_door();
        // Special 28 = red-key manual door.
        linedefs[6..8].copy_from_slice(&28u16.to_le_bytes());
        let doors = detect_doors(&linedefs, &sidedefs, &sectors, &verts);
        assert_eq!(doors.doors.len(), 1);
        assert_eq!(doors.doors[0].key, Some("red"));
    }

    #[test]
    fn a_secret_door_is_still_a_door_and_says_so() {
        let (mut linedefs, sidedefs, sectors, verts) = two_rooms_and_a_door();
        // Mark both door lines ML_SECRET (drawn as a wall on the automap).
        for li in 0..linedefs.len() / 14 {
            let flags = u16::from_le_bytes([linedefs[li * 14 + 4], linedefs[li * 14 + 5]]);
            linedefs[li * 14 + 4..li * 14 + 6]
                .copy_from_slice(&(flags | ML_SECRET).to_le_bytes());
        }
        let doors = detect_doors(&linedefs, &sidedefs, &sectors, &verts);
        assert_eq!(doors.doors.len(), 1, "a hidden door still opens");
        assert!(doors.doors[0].secret);
        assert_eq!(doors.doors[0].open_ceil, 124);
    }

    #[test]
    fn a_line_without_a_door_special_leaves_its_sector_static() {
        let (mut linedefs, sidedefs, sectors, verts) = two_rooms_and_a_door();
        for li in 0..linedefs.len() / 14 {
            linedefs[li * 14 + 6..li * 14 + 8].copy_from_slice(&0u16.to_le_bytes());
        }
        assert!(detect_doors(&linedefs, &sidedefs, &sectors, &verts).is_empty());
    }

    #[test]
    fn a_tagged_switch_door_is_found_through_its_tag() {
        let (mut linedefs, sidedefs, mut sectors, verts) = two_rooms_and_a_door();
        // Sector 1 gets tag 7; the lines lose their manual special and one
        // becomes a tagged S1 open (special 103).
        sectors[26 + 24..26 + 26].copy_from_slice(&7u16.to_le_bytes());
        for li in 0..linedefs.len() / 14 {
            linedefs[li * 14 + 6..li * 14 + 8].copy_from_slice(&0u16.to_le_bytes());
        }
        linedefs[6..8].copy_from_slice(&103u16.to_le_bytes());
        linedefs[8..10].copy_from_slice(&7u16.to_le_bytes());
        let doors = detect_doors(&linedefs, &sidedefs, &sectors, &verts);
        assert_eq!(doors.doors.len(), 1);
        assert_eq!(doors.doors[0].sector, 1);
    }

    #[test]
    fn an_already_open_sector_is_not_a_door() {
        let (linedefs, sidedefs, mut sectors, verts) = two_rooms_and_a_door();
        // Door sector ceiling already at 200: opening would lower it.
        sectors[26 + 2..26 + 4].copy_from_slice(&200i16.to_le_bytes());
        assert!(detect_doors(&linedefs, &sidedefs, &sectors, &verts).is_empty());
    }
}
