//! Teleporters and lifts: the two other things a Doom sector does that a
//! static mesh cannot say.
//!
//! A teleporter is a linedef special whose tagged sector holds a `thing`
//! of type 14 (teleport destination). Walking over the line puts you at
//! that thing, facing its angle — a navigator that does not know this walks
//! into a wall forever, because the room beyond is only reachable through
//! the pad.
//!
//! A lift is a plat special: the tagged sector's floor drops to the lowest
//! neighbouring floor, waits, and rises again. It exports exactly like a
//! door (`lift_N` node, states `up`/`down`, one LINEAR clip) so one runtime
//! handles both.

use std::collections::{BTreeMap, BTreeSet};

/// `W1/WR/S1/SR Teleport` (39/97) plus the Boom silent and monster-only
/// variants. All of them land the player on the same destination thing.
pub(crate) const TELEPORT_SPECIALS: &[u16] =
    &[39, 97, 125, 126, 174, 195, 207, 208, 209, 210];

/// Plat "down, wait, up, stay" specials — what a player rides.
pub(crate) const LIFT_SPECIALS: &[u16] = &[10, 21, 62, 88, 120, 121, 123];

/// Doom's `PLATWAIT`: three seconds at the bottom before it rises.
pub(crate) const LIFT_WAIT_SECONDS: f32 = 3.0;

/// Thing type of a teleport destination.
pub(crate) const TELEPORT_DEST_THING: u16 = 14;

/// How far into the front sector a teleport line's pad reaches when the
/// sector polygon is unknown, in map units (Doom's own line-crossing test
/// is a plane, so a thin slab is the honest shape).
pub(crate) const PAD_DEPTH: f32 = 16.0;

/// One teleporter: step anywhere in `pad`, arrive at `dst` facing `yaw`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Teleporter {
    /// `teleport_1`, `teleport_2`, … in map order.
    pub name: String,
    /// Pad AABB in map units (x, y of the Doom plane).
    pub pad_min: [f32; 2],
    pub pad_max: [f32; 2],
    /// Destination in map units + the floor it stands on.
    pub dst: [f32; 2],
    pub dst_floor: i16,
    pub yaw_degrees: f32,
}

/// One lift: the tagged sector's floor, and how far it drops.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Lift {
    /// `lift_1`, `lift_2`, … in sector order.
    pub name: String,
    pub sector: usize,
    /// Floor as authored (the UP pose the level is baked in).
    pub up_floor: i16,
    /// Lowest neighbouring floor — where it waits for you.
    pub down_floor: i16,
    pub centre: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DoomMovers {
    pub teleporters: Vec<Teleporter>,
    pub lifts: Vec<Lift>,
    /// Sector index → position in `lifts`.
    pub lift_by_sector: BTreeMap<usize, usize>,
}

impl DoomMovers {
    pub fn lift_index(&self, sector: usize) -> Option<usize> {
        self.lift_by_sector.get(&sector).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.teleporters.is_empty() && self.lifts.is_empty()
    }
}

/// Find every teleporter and lift of a map.
pub(crate) fn detect_movers(
    linedefs: &[u8],
    sidedefs: &[u8],
    sectors: &[u8],
    things: &[u8],
    verts: &[[f32; 2]],
) -> DoomMovers {
    let n_line = linedefs.len() / 14;
    let n_side = sidedefs.len() / 30;
    let n_sec = sectors.len() / 26;
    let n_vert = verts.len();
    if n_sec == 0 || n_line == 0 {
        return DoomMovers::default();
    }
    let sec_floor: Vec<i16> = (0..n_sec).map(|i| i16_le(sectors, i * 26)).collect();
    let sec_tag: Vec<u16> = (0..n_sec).map(|i| u16_le(sectors, i * 26 + 24)).collect();
    let sector_of_side = |snum: u16| -> Option<usize> {
        if snum == 0xFFFF || snum as usize >= n_side {
            return None;
        }
        let sec = u16_le(sidedefs, snum as usize * 30 + 28) as usize;
        (sec < n_sec).then_some(sec)
    };

    // Destination things, by the sector they stand in — resolved through the
    // sector polygons below.
    let mut dests: Vec<([f32; 2], f32)> = Vec::new();
    for i in 0..things.len() / 10 {
        let o = i * 10;
        if u16_le(things, o + 6) != TELEPORT_DEST_THING {
            continue;
        }
        dests.push((
            [i16_le(things, o) as f32, i16_le(things, o + 2) as f32],
            u16_le(things, o + 4) as f32,
        ));
    }

    // Sector outlines (for lift centres and for placing destinations).
    let mut outline: BTreeMap<usize, Vec<[f32; 2]>> = BTreeMap::new();
    let mut neighbours: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for li in 0..n_line {
        let lo = li * 14;
        let v1 = u16_le(linedefs, lo) as usize;
        let v2 = u16_le(linedefs, lo + 2) as usize;
        if v1 >= n_vert || v2 >= n_vert {
            continue;
        }
        let front = sector_of_side(u16_le(linedefs, lo + 10));
        let back = sector_of_side(u16_le(linedefs, lo + 12));
        for (a, b) in [(front, back), (back, front)] {
            let Some(a) = a else { continue };
            let pts = outline.entry(a).or_default();
            pts.push(verts[v1]);
            pts.push(verts[v2]);
            if let Some(b) = b {
                if b != a {
                    neighbours.entry(a).or_default().insert(b);
                }
            }
        }
    }

    let mut teleporters = Vec::new();
    let mut lift_sectors: BTreeSet<usize> = BTreeSet::new();
    for li in 0..n_line {
        let lo = li * 14;
        let special = u16_le(linedefs, lo + 6);
        let tag = u16_le(linedefs, lo + 8);
        if LIFT_SPECIALS.contains(&special) && tag != 0 {
            for (sec, t) in sec_tag.iter().enumerate() {
                if *t == tag {
                    lift_sectors.insert(sec);
                }
            }
            continue;
        }
        if !TELEPORT_SPECIALS.contains(&special) || tag == 0 {
            continue;
        }
        let v1 = u16_le(linedefs, lo) as usize;
        let v2 = u16_le(linedefs, lo + 2) as usize;
        if v1 >= n_vert || v2 >= n_vert {
            continue;
        }
        // The destination thing standing in a sector with this tag.
        let target: Vec<usize> = (0..n_sec).filter(|&s| sec_tag[s] == tag).collect();
        let Some((dst, angle, dst_sector)) = target.iter().find_map(|&sec| {
            let bounds = outline.get(&sec).map(|p| aabb(p))?;
            dests
                .iter()
                .find(|(p, _)| inside(bounds, *p))
                .map(|(p, a)| (*p, *a, sec))
        }) else {
            continue;
        };
        let pad = pad_of_line(verts[v1], verts[v2], sector_of_side(u16_le(linedefs, lo + 10)), &outline);
        teleporters.push(Teleporter {
            name: format!("teleport_{}", teleporters.len() + 1),
            pad_min: pad.0,
            pad_max: pad.1,
            dst,
            dst_floor: sec_floor[dst_sector],
            yaw_degrees: angle,
        });
    }

    let mut lifts = Vec::new();
    let mut lift_by_sector = BTreeMap::new();
    for sec in lift_sectors {
        let Some(near) = neighbours.get(&sec) else {
            continue;
        };
        let Some(lowest) = near.iter().map(|&n| sec_floor[n]).min() else {
            continue;
        };
        if lowest >= sec_floor[sec] {
            // Nothing to ride: the "lift" is already at the bottom.
            continue;
        }
        let centre = outline.get(&sec).map(|p| centre_of(p)).unwrap_or([0.0, 0.0]);
        lift_by_sector.insert(sec, lifts.len());
        lifts.push(Lift {
            name: format!("lift_{}", lifts.len() + 1),
            sector: sec,
            up_floor: sec_floor[sec],
            down_floor: lowest,
            centre,
        });
    }
    DoomMovers {
        teleporters,
        lifts,
        lift_by_sector,
    }
}

/// The pad a teleport line covers: the line itself extruded [`PAD_DEPTH`]
/// into its front sector, clamped to that sector's own bounds so the pad
/// never sticks through a wall.
fn pad_of_line(
    a: [f32; 2],
    b: [f32; 2],
    front: Option<usize>,
    outline: &BTreeMap<usize, Vec<[f32; 2]>>,
) -> ([f32; 2], [f32; 2]) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    // Doom's front side is to the RIGHT of v1->v2.
    let nx = dy / len * PAD_DEPTH;
    let ny = -dx / len * PAD_DEPTH;
    let pts = [a, b, [a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny]];
    let (mut lo, mut hi) = aabb(&pts);
    if let Some(bounds) = front.and_then(|s| outline.get(&s)).map(|p| aabb(p)) {
        lo[0] = lo[0].max(bounds.0[0]);
        lo[1] = lo[1].max(bounds.0[1]);
        hi[0] = hi[0].min(bounds.1[0]);
        hi[1] = hi[1].min(bounds.1[1]);
        // A clamp that inverted the box means the line is not on that
        // sector's boundary; keep the unclamped pad rather than an empty one.
        if lo[0] > hi[0] || lo[1] > hi[1] {
            return aabb(&pts);
        }
    }
    (lo, hi)
}

fn aabb(pts: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for p in pts {
        lo[0] = lo[0].min(p[0]);
        lo[1] = lo[1].min(p[1]);
        hi[0] = hi[0].max(p[0]);
        hi[1] = hi[1].max(p[1]);
    }
    (lo, hi)
}

fn inside(bounds: ([f32; 2], [f32; 2]), p: [f32; 2]) -> bool {
    p[0] >= bounds.0[0] && p[0] <= bounds.1[0] && p[1] >= bounds.0[1] && p[1] <= bounds.1[1]
}

fn centre_of(pts: &[[f32; 2]]) -> [f32; 2] {
    let (lo, hi) = aabb(pts);
    [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]
}

/// Floor geometry of one lift, authored in its UP pose.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiftFloor {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub colors: Vec<[f32; 3]>,
}

impl LiftFloor {
    pub fn is_empty(&self) -> bool {
        self.indices.len() < 3 || self.positions.is_empty()
    }
}

/// Where the flat emitters send a lift sector's floor.
pub(crate) struct LiftSink<'a> {
    pub movers: &'a DoomMovers,
    pub floors: &'a mut Vec<LiftFloor>,
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

    /// Room 0 (0..128) with a teleport line on its east edge tagged 3, and
    /// room 1 (256..384) tagged 3 holding a destination thing at (320, 64)
    /// facing 90 degrees. Sector 2 is a lift tagged 4, floor 64, next to
    /// room 0's floor 0.
    fn map_with_a_teleporter_and_a_lift() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<[f32; 2]>) {
        let mut sectors = Vec::new();
        for (floor, tag) in [(0i16, 0u16), (0, 3), (64, 4)] {
            sectors.extend_from_slice(&floor.to_le_bytes());
            sectors.extend_from_slice(&128i16.to_le_bytes());
            sectors.extend_from_slice(b"FLOOR0\0\0");
            sectors.extend_from_slice(b"CEIL1\0\0\0");
            sectors.extend_from_slice(&0i16.to_le_bytes());
            sectors.extend_from_slice(&0i16.to_le_bytes());
            sectors.extend_from_slice(&tag.to_le_bytes());
        }
        let mut sidedefs = Vec::new();
        for sec in [0u16, 1, 2, 0] {
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(b"-\0\0\0\0\0\0\0");
            sidedefs.extend_from_slice(b"-\0\0\0\0\0\0\0");
            sidedefs.extend_from_slice(b"WALL1\0\0\0");
            sidedefs.extend_from_slice(&sec.to_le_bytes());
        }
        let verts = vec![
            [128.0, 0.0],   // 0
            [128.0, 128.0], // 1
            [256.0, 0.0],   // 2
            [384.0, 0.0],   // 3
            [384.0, 128.0], // 4
            [256.0, 128.0], // 5
            [0.0, 0.0],     // 6
            [0.0, 128.0],   // 7
        ];
        // Teleport line (v1 -> v0, front = side 0 = sector 0), special 39 tag 3.
        // Room 1 outline (sides on sector 1), lift edge between sector 2 and 0.
        let lines: [(u16, u16, u16, u16, u16, u16); 5] = [
            (1, 0, 39, 3, 0, 0xFFFF),
            (2, 3, 0, 0, 1, 0xFFFF),
            (3, 4, 0, 0, 1, 0xFFFF),
            (4, 5, 0, 0, 1, 0xFFFF),
            (6, 7, 10, 4, 2, 3),
        ];
        let mut linedefs = Vec::new();
        for (v1, v2, special, tag, right, left) in lines {
            for v in [v1, v2, 0, special, tag, right, left] {
                linedefs.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut things = Vec::new();
        for v in [320i16, 64, 90, TELEPORT_DEST_THING as i16, 7] {
            things.extend_from_slice(&v.to_le_bytes());
        }
        (linedefs, sidedefs, sectors, things, verts)
    }

    #[test]
    fn a_teleport_line_finds_its_destination_thing() {
        let (linedefs, sidedefs, sectors, things, verts) = map_with_a_teleporter_and_a_lift();
        let m = detect_movers(&linedefs, &sidedefs, &sectors, &things, &verts);
        assert_eq!(m.teleporters.len(), 1, "{:?}", m.teleporters);
        let t = &m.teleporters[0];
        assert_eq!(t.name, "teleport_1");
        assert_eq!(t.dst, [320.0, 64.0]);
        assert_eq!(t.yaw_degrees, 90.0);
        assert_eq!(t.dst_floor, 0);
        // The pad is the line extruded 16 units into the front sector
        // (x 112..128 for a line at x=128 whose front faces -x).
        assert!(t.pad_min[0] >= 112.0 - 1e-3 && t.pad_max[0] <= 128.0 + 1e-3, "{t:?}");
        assert!(t.pad_max[0] - t.pad_min[0] >= 15.0, "{t:?}");
        assert!(t.pad_max[1] - t.pad_min[1] >= 127.0, "{t:?}");
    }

    #[test]
    fn a_plat_special_makes_the_tagged_sector_a_lift() {
        let (linedefs, sidedefs, sectors, things, verts) = map_with_a_teleporter_and_a_lift();
        let m = detect_movers(&linedefs, &sidedefs, &sectors, &things, &verts);
        assert_eq!(m.lifts.len(), 1, "{:?}", m.lifts);
        let l = &m.lifts[0];
        assert_eq!(l.name, "lift_1");
        assert_eq!(l.sector, 2);
        assert_eq!(l.up_floor, 64);
        assert_eq!(l.down_floor, 0, "drops to the lowest neighbouring floor");
        assert_eq!(m.lift_index(2), Some(0));
    }

    #[test]
    fn a_teleport_line_without_a_destination_is_not_a_teleporter() {
        let (linedefs, sidedefs, sectors, _things, verts) = map_with_a_teleporter_and_a_lift();
        let m = detect_movers(&linedefs, &sidedefs, &sectors, &[], &verts);
        assert!(m.teleporters.is_empty());
        assert_eq!(m.lifts.len(), 1, "the lift is unaffected");
    }

    #[test]
    fn a_sector_already_at_the_bottom_is_not_a_lift() {
        let (linedefs, sidedefs, mut sectors, things, verts) =
            map_with_a_teleporter_and_a_lift();
        sectors[2 * 26..2 * 26 + 2].copy_from_slice(&0i16.to_le_bytes());
        let m = detect_movers(&linedefs, &sidedefs, &sectors, &things, &verts);
        assert!(m.lifts.is_empty());
    }
}
