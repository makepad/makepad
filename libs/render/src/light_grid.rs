//! Precomputed static-light selection grid — the "racetrack with 300
//! lights" answer.
//!
//! Ranking lights per batch per frame is O(instances × lights) and grows
//! with the world. Static lights do not move, so the ranking is done ONCE,
//! at harvest/settle time: a uniform world grid where every cell stores the
//! ≤8 strongest lights whose radius REACHES the cell's AABB — the box test,
//! not the centre, so neighbouring cells share their border lights and an
//! object crossing a cell edge never sees its light set pop. Each cell's
//! answer is stored PRE-PACKED in the shader's `dl_pos/dl_col` vec4 layout,
//! so the per-frame work is a cell lookup and a memcpy: O(1) per object,
//! zero allocation, independent of how many lights the world carries.
//!
//! Transient lights (firework flashes, host frame lights — always a
//! handful) are merged on top per frame, displacing the weakest static
//! entries when the block is full ([`merge_transients_into_block`]).

use crate::lightmap::LmLight;
use crate::renderer::MAX_DYNAMIC_LIGHTS;
use makepad_draw::makepad_math::Vec3f;

/// Grid cell side, world units. Sized against the lamp radii in play (6-10):
/// a light reaches at most a ring of neighbouring cells, so per-cell
/// candidate counts stay small, while an object crossing a 20-unit cell
/// keeps its set for many frames.
pub const LIGHT_GRID_CELL: f32 = 20.0;

/// Hard cap on grid resolution per axis; the cell size grows to cover a
/// sprawling world rather than the cell count exploding.
const LIGHT_GRID_MAX_CELLS: usize = 128;

/// Floats in one pre-packed block: 8 × (pos.xyz+radius, rgb+spot).
pub const LIGHT_BLOCK_FLOATS: usize = MAX_DYNAMIC_LIGHTS * 8;

/// One cell's ready-to-upload uniform block, strongest light first.
#[derive(Clone)]
pub struct LightBlock {
    pub packed: [f32; LIGHT_BLOCK_FLOATS],
    pub count: usize,
}

impl Default for LightBlock {
    fn default() -> Self {
        LightBlock {
            packed: [0.0; LIGHT_BLOCK_FLOATS],
            count: 0,
        }
    }
}

fn pack_light(l: &LmLight, slot: usize, out: &mut [f32; LIGHT_BLOCK_FLOATS]) {
    let at = slot * 8;
    out[at] = l.pos.x;
    out[at + 1] = l.pos.y;
    out[at + 2] = l.pos.z;
    out[at + 3] = l.radius;
    out[at + 4] = l.color.x;
    out[at + 5] = l.color.y;
    out[at + 6] = l.color.z;
    out[at + 7] = l.spot;
}

fn intensity(l: &LmLight) -> f32 {
    l.color.x.max(l.color.y).max(l.color.z)
}

/// The static-light grid. Built once per static-light-set change (the same
/// settle trigger as the light bake), never on the hot path.
pub struct LightGrid {
    origin_x: f32,
    origin_z: f32,
    cell: f32,
    nx: usize,
    nz: usize,
    cells: Vec<LightBlock>,
    /// Handed out for lookups outside the grid (or an empty grid).
    empty: LightBlock,
}

impl Default for LightGrid {
    fn default() -> Self {
        LightGrid {
            origin_x: 0.0,
            origin_z: 0.0,
            cell: LIGHT_GRID_CELL,
            nx: 0,
            nz: 0,
            cells: Vec::new(),
            empty: LightBlock::default(),
        }
    }
}

impl LightGrid {
    /// Build the grid over the lights' reach (position ± radius on xz).
    /// Per cell: candidates whose radius reaches the cell's xz rect, ranked
    /// by intensity × (1 − d/r)² at the cell CENTRE (xz distance — lamps
    /// hang a few units up and receivers stand near the ground, so the
    /// vertical term is common mode and drops out of the ORDERING).
    pub fn build(lights: &[LmLight], cell_size: f32) -> LightGrid {
        let live: Vec<&LmLight> = lights.iter().filter(|l| l.radius > 0.0).collect();
        if live.is_empty() {
            return LightGrid::default();
        }
        let (mut lo_x, mut lo_z) = (f32::MAX, f32::MAX);
        let (mut hi_x, mut hi_z) = (f32::MIN, f32::MIN);
        for l in &live {
            lo_x = lo_x.min(l.pos.x - l.radius);
            lo_z = lo_z.min(l.pos.z - l.radius);
            hi_x = hi_x.max(l.pos.x + l.radius);
            hi_z = hi_z.max(l.pos.z + l.radius);
        }
        let span_x = (hi_x - lo_x).max(1.0e-3);
        let span_z = (hi_z - lo_z).max(1.0e-3);
        let cell = cell_size
            .max(span_x / LIGHT_GRID_MAX_CELLS as f32)
            .max(span_z / LIGHT_GRID_MAX_CELLS as f32);
        let nx = (span_x / cell).ceil() as usize + 1;
        let nz = (span_z / cell).ceil() as usize + 1;
        let mut cells = vec![LightBlock::default(); nx * nz];
        // Scratch reused across cells; the build is off the hot path but
        // there is no reason to churn.
        let mut rank: Vec<(f32, usize)> = Vec::new();
        for gz in 0..nz {
            for gx in 0..nx {
                let (x0, z0) = (lo_x + gx as f32 * cell, lo_z + gz as f32 * cell);
                let (x1, z1) = (x0 + cell, z0 + cell);
                let (cx, cz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
                rank.clear();
                for (i, l) in live.iter().enumerate() {
                    // Reach test against the cell's RECT: border cells share
                    // their lights, which is what stops set-popping.
                    let dx = (x0 - l.pos.x).max(0.0).max(l.pos.x - x1);
                    let dz = (z0 - l.pos.z).max(0.0).max(l.pos.z - z1);
                    if dx * dx + dz * dz >= l.radius * l.radius {
                        continue;
                    }
                    let (ex, ez) = (cx - l.pos.x, cz - l.pos.z);
                    let d = (ex * ex + ez * ez).sqrt();
                    let att = (1.0 - d / l.radius).max(0.0);
                    rank.push((intensity(l) * att * att, i));
                }
                rank.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                let block = &mut cells[gz * nx + gx];
                for (slot, (_, i)) in rank.iter().take(MAX_DYNAMIC_LIGHTS).enumerate() {
                    pack_light(live[*i], slot, &mut block.packed);
                    block.count = slot + 1;
                }
            }
        }
        LightGrid {
            origin_x: lo_x,
            origin_z: lo_z,
            cell,
            nx,
            nz,
            cells,
            empty: LightBlock::default(),
        }
    }

    /// The pre-packed block for a world position: index math and a borrow,
    /// nothing ranked, nothing allocated. Outside the grid = no lights.
    pub fn block_at(&self, x: f32, z: f32) -> &LightBlock {
        match self.cell_of(x, z) {
            Some(c) => self.block_of(c),
            None => &self.empty,
        }
    }

    /// Which cell a world position lands in, `None` outside the grid.
    pub fn cell_of(&self, x: f32, z: f32) -> Option<(i32, i32)> {
        if self.nx == 0 || self.nz == 0 {
            return None;
        }
        let gx = ((x - self.origin_x) / self.cell).floor();
        let gz = ((z - self.origin_z) / self.cell).floor();
        if gx < 0.0 || gz < 0.0 || gx >= self.nx as f32 || gz >= self.nz as f32 {
            return None;
        }
        Some((gx as i32, gz as i32))
    }

    /// A cell's block directly (pair with [`Self::cell_of`] /
    /// [`Self::cell_still_fits`] for hysteresis-stable lookups).
    pub fn block_of(&self, cell: (i32, i32)) -> &LightBlock {
        if cell.0 < 0 || cell.1 < 0 || cell.0 >= self.nx as i32 || cell.1 >= self.nz as i32 {
            return &self.empty;
        }
        &self.cells[cell.1 as usize * self.nx + cell.0 as usize]
    }

    /// Is the position still within `cell` inflated by `margin` world
    /// units? The positional dead-band that keeps an object dithering on a
    /// cell line from flapping between two blocks: it only re-homes after
    /// moving a real distance into the neighbour.
    pub fn cell_still_fits(&self, cell: (i32, i32), x: f32, z: f32, margin: f32) -> bool {
        if self.nx == 0 {
            return false;
        }
        let x0 = self.origin_x + cell.0 as f32 * self.cell;
        let z0 = self.origin_z + cell.1 as f32 * self.cell;
        x >= x0 - margin
            && x <= x0 + self.cell + margin
            && z >= z0 - margin
            && z <= z0 + self.cell + margin
    }
}

/// Merge this frame's transient lights with a cell's static block into
/// `out`: a SINGLE strength ranking at the instance's own position, top 8
/// kept, kept transients packed into the leading slots (returned as the
/// `dl_split` count the static gate reads), kept statics after them.
///
/// Strength-ranked ON PURPOSE, not "transients first": a burst of firework
/// flashes must never evict the lamp an object is standing under — that
/// exact starvation was a car visibly flickering at night. A light enters
/// or leaves the merged set only where its contribution crosses another's
/// (equal at the swap, so nothing pops), and statics win ties so a steady
/// scene is bit-stable frame after frame. Cost: the cell's ≤8 statics plus
/// the handful of live transients — never the world's light list.
pub fn merge_transients_into_block(
    block: &LightBlock,
    lights: &[LmLight],
    transients: std::ops::Range<usize>,
    anchor: Vec3f,
    rank: &mut Vec<(f32, usize)>,
    out: &mut [f32; LIGHT_BLOCK_FLOATS],
) -> usize {
    let score_at = |px: f32, py: f32, pz: f32, radius: f32, peak: f32| -> f32 {
        let (dx, dy, dz) = (anchor.x - px, anchor.y - py, anchor.z - pz);
        let d2 = dx * dx + dy * dy + dz * dz;
        if radius <= 0.0 || d2 >= radius * radius {
            return 0.0;
        }
        let att = 1.0 - d2.sqrt() / radius;
        peak * att * att
    };
    // Transients scored at the instance, strongest first.
    rank.clear();
    for i in transients {
        let l = &lights[i];
        let s = score_at(l.pos.x, l.pos.y, l.pos.z, l.radius, intensity(l));
        if s > 0.0 {
            rank.push((s, i));
        }
    }
    rank.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    // The cell's statics re-scored at the instance (their packed entries
    // carry position/radius/colour — ≤8 of them, a fixed-size loop).
    let mut stat: [(f32, usize); MAX_DYNAMIC_LIGHTS] = [(0.0, 0); MAX_DYNAMIC_LIGHTS];
    let mut stat_n = 0;
    for s in 0..block.count {
        let at = s * 8;
        let peak = block.packed[at + 4]
            .max(block.packed[at + 5])
            .max(block.packed[at + 6]);
        let sc = score_at(
            block.packed[at],
            block.packed[at + 1],
            block.packed[at + 2],
            block.packed[at + 3],
            peak,
        );
        if sc > 0.0 {
            stat[stat_n] = (sc, s);
            stat_n += 1;
        }
    }
    stat[..stat_n].sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    // Top-8 of the union. Statics win ties — the stable thing must not be
    // the one that yields to a decaying flash of equal strength.
    let (mut ti, mut si) = (0usize, 0usize);
    let mut keep_t: [usize; MAX_DYNAMIC_LIGHTS] = [0; MAX_DYNAMIC_LIGHTS];
    let mut keep_s: [usize; MAX_DYNAMIC_LIGHTS] = [0; MAX_DYNAMIC_LIGHTS];
    let (mut nt, mut ns) = (0usize, 0usize);
    while nt + ns < MAX_DYNAMIC_LIGHTS {
        let t_score = rank.get(ti).map(|r| r.0);
        let s_score = if si < stat_n { Some(stat[si].0) } else { None };
        match (t_score, s_score) {
            (Some(t), Some(s)) if t > s => {
                keep_t[nt] = rank[ti].1;
                nt += 1;
                ti += 1;
            }
            (_, Some(_)) => {
                keep_s[ns] = stat[si].1;
                ns += 1;
                si += 1;
            }
            (Some(_), None) => {
                keep_t[nt] = rank[ti].1;
                nt += 1;
                ti += 1;
            }
            (None, None) => break,
        }
    }
    // Kept transients lead (the dl_split prefix), kept statics follow.
    out.fill(0.0);
    for (slot, i) in keep_t[..nt].iter().enumerate() {
        pack_light(&lights[*i], slot, out);
    }
    for (slot, s) in keep_s[..ns].iter().enumerate() {
        let at = (nt + slot) * 8;
        out[at..at + 8].copy_from_slice(&block.packed[s * 8..s * 8 + 8]);
    }
    nt
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_draw::makepad_math::vec3f;

    /// 300 lights around a track loop: every sampled position's grid block
    /// must match a brute-force ranking done the same way the build does it
    /// (candidates reaching the cell rect, scored at the cell centre) — the
    /// property that makes the O(1) lookup a drop-in for the O(lights) scan.
    #[test]
    fn a_300_light_track_matches_brute_force() {
        let mut lights = Vec::new();
        for i in 0..300 {
            let t = i as f32 / 300.0 * std::f32::consts::TAU;
            lights.push(LmLight::omni(
                vec3f(t.cos() * 100.0, 3.0, t.sin() * 60.0),
                vec3f(1.0 + (i % 5) as f32 * 0.25, 1.0, 0.8),
                6.0 + (i % 3) as f32,
            ));
        }
        let grid = LightGrid::build(&lights, LIGHT_GRID_CELL);
        for s in 0..60 {
            let t = (s as f32 + 0.37) / 60.0 * std::f32::consts::TAU;
            let p = vec3f(t.cos() * 100.0, 0.0, t.sin() * 60.0);
            let block = grid.block_at(p.x, p.z);
            // Brute force at the sample's cell, same scoring.
            let gx = ((p.x - grid.origin_x) / grid.cell).floor();
            let gz = ((p.z - grid.origin_z) / grid.cell).floor();
            let (x0, z0) = (
                grid.origin_x + gx * grid.cell,
                grid.origin_z + gz * grid.cell,
            );
            let (x1, z1) = (x0 + grid.cell, z0 + grid.cell);
            let (cx, cz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
            let mut want: Vec<(f32, usize)> = Vec::new();
            for (i, l) in lights.iter().enumerate() {
                let dx = (x0 - l.pos.x).max(0.0).max(l.pos.x - x1);
                let dz = (z0 - l.pos.z).max(0.0).max(l.pos.z - z1);
                if dx * dx + dz * dz >= l.radius * l.radius {
                    continue;
                }
                let (ex, ez) = (cx - l.pos.x, cz - l.pos.z);
                let d = (ex * ex + ez * ez).sqrt();
                let att = (1.0 - d / l.radius).max(0.0);
                want.push((intensity(l) * att * att, i));
            }
            want.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            let n = want.len().min(MAX_DYNAMIC_LIGHTS);
            assert_eq!(block.count, n, "sample {s}: slot count");
            for (slot, (_, i)) in want.iter().take(n).enumerate() {
                let l = &lights[*i];
                let at = slot * 8;
                assert_eq!(
                    (block.packed[at], block.packed[at + 2]),
                    (l.pos.x, l.pos.z),
                    "sample {s} slot {slot}: wrong light"
                );
            }
            // The nearest light to the sample is never missing: it also
            // scores highest at this cell's centre-or-neighbour by
            // construction of the track spacing.
            assert!(block.count > 0, "sample {s}: empty block on the track");
        }
    }

    /// Cell-edge continuity: a light reaching a cell's RECT is listed even
    /// when it sits outside that cell — the border sharing that kills
    /// set-popping when an object crosses an edge.
    #[test]
    fn border_cells_share_their_lights() {
        let l = LmLight::omni(vec3f(0.0, 3.0, 0.0), vec3f(2.0, 2.0, 2.0), 8.0);
        let grid = LightGrid::build(&[l], LIGHT_GRID_CELL);
        // Just across the cell border from the light, still in radius.
        for x in [-7.0f32, 7.0] {
            let b = grid.block_at(x, 0.0);
            assert_eq!(b.count, 1, "x={x} must still see the light");
        }
        // Far outside its radius: nothing.
        assert_eq!(grid.block_at(60.0, 0.0).count, 0);
    }

    /// Stability by construction: while fewer than 8 lights compete, two
    /// cells a boundary apart list the SAME light set (order may differ) —
    /// crossing the line cannot pop a visible light in or out. This is the
    /// no-flicker property the per-instance lookup depends on.
    #[test]
    fn adjacent_cells_agree_when_under_capacity() {
        // A row of lamps 9 apart, radius 8: any point is reached by 1-2.
        let lights: Vec<LmLight> = (0..12)
            .map(|i| {
                LmLight::omni(
                    vec3f(i as f32 * 9.0, 3.0, 0.0),
                    vec3f(2.0, 1.5, 1.0),
                    8.0,
                )
            })
            .collect();
        let grid = LightGrid::build(&lights, LIGHT_GRID_CELL);
        // Walk the row in fine steps; collect each step's set of light xs.
        let set_at = |x: f32| -> Vec<i32> {
            let b = grid.block_at(x, 0.0);
            let mut xs: Vec<i32> = (0..b.count)
                .map(|s| b.packed[s * 8].round() as i32)
                .collect();
            xs.sort_unstable();
            xs
        };
        for step in 0..200 {
            let x = step as f32 * 0.5;
            let here = set_at(x);
            let there = set_at(x + 0.5);
            // Sets may only differ by lights whose radius boundary lies
            // between the two probes — i.e. lights ~zero at both.
            for lx in here.iter().chain(there.iter()) {
                let d_here = (x - *lx as f32).abs();
                let d_there = (x + 0.5 - *lx as f32).abs();
                if d_here < 6.5 && d_there < 6.5 {
                    assert!(
                        here.contains(lx) && there.contains(lx),
                        "light at {lx} popped between x={x} and x={} while well inside \
                         its radius: {:?} vs {:?}",
                        x + 0.5,
                        here,
                        there
                    );
                }
            }
        }
        // And the dead-band helper: a point just past a boundary still fits
        // its old cell within the margin (a dithering object never
        // re-homes), while a genuinely distant point does not.
        let cell = grid.cell_of(19.9, 0.0).unwrap();
        assert!(grid.cell_still_fits(cell, 19.9, 0.0, 1.0), "own position fits");
        assert_ne!(grid.cell_of(45.0, 0.0), Some(cell), "45 is another cell");
        assert!(!grid.cell_still_fits(cell, 45.0, 0.0, 1.0), "distant point re-homes");
    }

    /// The per-frame path does no ranking: a million lookups on the
    /// 300-light grid complete in well under a second (a brute-force scan
    /// would be ~300M score evaluations). Generous bound — this guards the
    /// complexity class, not the constant.
    #[test]
    fn block_lookup_is_constant_time() {
        let mut lights = Vec::new();
        for i in 0..300 {
            let t = i as f32 / 300.0 * std::f32::consts::TAU;
            lights.push(LmLight::omni(
                vec3f(t.cos() * 100.0, 3.0, t.sin() * 60.0),
                vec3f(2.0, 1.5, 1.0),
                8.0,
            ));
        }
        let grid = LightGrid::build(&lights, LIGHT_GRID_CELL);
        let t0 = std::time::Instant::now();
        let mut acc = 0usize;
        for i in 0..1_000_000u32 {
            let x = (i % 211) as f32 - 105.0;
            let z = (i % 127) as f32 - 63.0;
            acc += grid.block_at(x, z).count;
        }
        assert!(acc > 0);
        assert!(
            t0.elapsed().as_millis() < 1000,
            "1M lookups took {}ms — lookup is no longer O(1)",
            t0.elapsed().as_millis()
        );
    }

    /// Transient merge: a single strength ranking at the instance. A strong
    /// nearby flash earns a leading slot; a weak one loses to stronger
    /// statics instead of displacing them; out-of-radius never lands.
    #[test]
    fn transients_compete_by_strength_not_by_arrival() {
        // 8 statics of descending strength around the origin cell.
        let mut lights: Vec<LmLight> = (0..8)
            .map(|i| {
                LmLight::omni(
                    vec3f(i as f32 * 0.1, 3.0, 0.0),
                    vec3f(2.0 - i as f32 * 0.1, 1.0, 1.0),
                    10.0,
                )
            })
            .collect();
        let grid = LightGrid::build(&lights, LIGHT_GRID_CELL);
        let block = grid.block_at(0.0, 0.0).clone();
        assert_eq!(block.count, 8);
        // A blazing transient overhead and a dim one.
        lights.push(LmLight::omni(vec3f(1.0, 5.0, 0.0), vec3f(6.0, 6.0, 6.0), 30.0));
        lights.push(LmLight::omni(vec3f(-1.0, 5.0, 0.0), vec3f(0.2, 0.2, 0.2), 30.0));
        let mut rank = Vec::new();
        let mut out = [0.0f32; LIGHT_BLOCK_FLOATS];
        let split = merge_transients_into_block(
            &block,
            &lights,
            8..10,
            vec3f(0.0, 1.0, 0.0),
            &mut rank,
            &mut out,
        );
        assert_eq!(split, 1, "only the strong transient earns a slot");
        assert_eq!(out[4], 6.0, "slot 0 is the strong transient");
        // The statics keep the remaining 7 slots, strongest first.
        assert_eq!(out[8 + 4], 2.0, "strongest static right after");
        // A transient out of radius never lands a slot.
        lights.push(LmLight::omni(vec3f(500.0, 0.0, 0.0), vec3f(9.0, 9.0, 9.0), 5.0));
        let split = merge_transients_into_block(
            &block,
            &lights,
            10..11,
            vec3f(0.0, 1.0, 0.0),
            &mut rank,
            &mut out,
        );
        assert_eq!(split, 0);
        assert_eq!(out[4], 2.0, "statics fill from slot 0 again");
    }

    /// THE one-lamp flicker regression (user bug): a car parked under the
    /// only lamp in the scene while firework flashes churn overhead. The
    /// lamp's slot must be present and BIT-IDENTICAL for 120 simulated
    /// frames — a burst of weaker transients must never evict the light an
    /// object is standing under, and the stable set must not reorder.
    #[test]
    fn a_parked_car_under_one_lamp_never_flickers() {
        let lamp = LmLight::omni(vec3f(0.0, 2.9, 0.0), vec3f(2.0, 1.55, 0.95), 8.0);
        let grid = LightGrid::build(&[lamp.clone()], LIGHT_GRID_CELL);
        let block = grid.block_at(1.2, 0.0).clone();
        let car = vec3f(1.2, 0.5, 0.0);
        let mut rank = Vec::new();
        let mut out = [0.0f32; LIGHT_BLOCK_FLOATS];
        let mut last_lamp_slot: Option<[u32; 8]> = None;
        for frame in 0..120 {
            // A churning firework display: distant flashes appearing and
            // decaying, different count every frame — up to 18 alive.
            let mut lights = vec![lamp.clone()];
            let n = 6 + (frame * 7) % 12;
            for k in 0..n {
                let a = (frame + k * 37) as f32 * 0.7;
                let life = ((frame * 13 + k * 29) % 100) as f32 / 100.0;
                // Near enough that several are genuinely in range of the
                // car every frame (the committed transients-first merge
                // fails THIS exact case).
                lights.push(LmLight::omni(
                    vec3f(a.cos() * 30.0, 36.0, a.sin() * 30.0),
                    vec3f(3.0 * (1.0 - life), 2.0 * (1.0 - life), 1.5 * (1.0 - life)),
                    55.0,
                ));
            }
            let split = merge_transients_into_block(
                &block,
                &lights,
                1..lights.len(),
                car,
                &mut rank,
                &mut out,
            );
            // The lamp must hold a slot, at the same place, bit for bit.
            let at = split * 8; // first static slot
            assert!(
                out[at + 3] == 8.0 && out[at + 4] == 2.0,
                "frame {frame}: lamp missing from the block (split {split})"
            );
            let bits: [u32; 8] =
                std::array::from_fn(|i| out[at + i].to_bits());
            if let Some(prev) = &last_lamp_slot {
                assert_eq!(prev, &bits, "frame {frame}: lamp slot changed bits");
            }
            last_lamp_slot = Some(bits);
        }
    }
}
