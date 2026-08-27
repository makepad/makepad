//! Walking an imported level: real-triangle collision plus an autonomous
//! first-person walker.
//!
//! Imported Doom/Quake/Duke maps are one big static mesh. The renderer's
//! prop collider (`voxel_collider_boxes`) is a low-res box decomposition
//! built for props — for a 20 000-triangle level it collapses to a few
//! dozen boxes, and a walker standing on a box top stands in mid-air. So
//! this module probes the LEVEL'S OWN TRIANGLES through [`MeshRaycaster`]:
//! the floor under your feet is the triangle you are actually standing on.
//!
//! Nothing here touches `Cx`, the renderer or the asset crates: a level is
//! positions + indices, and a walk step is geometry. That keeps it usable
//! by the VJ's map slot AND by the game sandbox's NPCs, and unit-testable
//! against synthetic triangle soups.

use crate::ao::MeshRaycaster;
use makepad_draw::*;

/// Which axis the source considers "up". Imported packs are Y-up (the
/// importer converts), but a raw Quake/Build mesh is Z-up and would walk on
/// the walls if taken at face value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

/// What a floor triangle IS, for walkers that care. Doom's damaging
/// sectors and Quake's lava/slime brushes are floors you can stand on and
/// should not: the tour keeps to clean ground while it has the choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceKind {
    #[default]
    Floor,
    /// Damaging ground (nukage, lava, slime): walkable, avoided.
    Hazard,
    /// Water and other non-damaging liquid: crossed without complaint.
    Liquid,
}

/// A level's collision geometry: its triangles, indexed for fast probes.
pub struct LevelCollision {
    caster: MeshRaycaster,
    min: Vec3f,
    max: Vec3f,
    /// Per-triangle surface kind, when the source could classify them
    /// (importer `hazard_N` nodes, or a material-name heuristic). Empty
    /// means "everything is plain floor".
    kinds: Vec<SurfaceKind>,
}

/// A downward probe result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloorHit {
    /// Height of the surface (world Y).
    pub y: f32,
    /// How steep it is: 1 = flat floor, 0 = vertical wall.
    pub flatness: f32,
    /// What the surface is — the walker avoids standing on hazards.
    pub kind: SurfaceKind,
}

impl LevelCollision {
    /// From a packed vertex stream (`stride` floats per vertex, position
    /// first — the engine's `MODEL_VERTEX_FLOATS` layout) plus indices.
    pub fn from_packed(
        vertices: &[f32],
        stride: usize,
        indices: &[u32],
        up: UpAxis,
    ) -> Option<LevelCollision> {
        if stride < 3 || vertices.len() < stride || indices.len() < 3 {
            return None;
        }
        let positions: Vec<Vec3f> = vertices
            .chunks_exact(stride)
            .map(|v| match up {
                UpAxis::Y => vec3f(v[0], v[1], v[2]),
                // Z-up → Y-up, keeping the handedness the renderer expects.
                UpAxis::Z => vec3f(v[0], v[2], -v[1]),
            })
            .collect();
        Some(Self::from_positions(positions, indices.to_vec()))
    }

    pub fn from_positions(positions: Vec<Vec3f>, indices: Vec<u32>) -> LevelCollision {
        let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
        for p in &positions {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            min.z = min.z.min(p.z);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            max.z = max.z.max(p.z);
        }
        // A hair of padding so probes starting exactly on the boundary
        // (the scan starts at the ceiling) are not clipped away.
        let pad = vec3f(0.01, 0.01, 0.01);
        LevelCollision {
            caster: MeshRaycaster::new(positions, indices, min - pad, max + pad),
            min,
            max,
            kinds: Vec::new(),
        }
    }

    /// Attach per-triangle surface kinds (one entry per triangle, in index
    /// order). Sources: the importer's `hazard_N` nodes, or a flat/material
    /// name heuristic. Without them every floor is plain [`SurfaceKind::Floor`].
    pub fn with_kinds(mut self, kinds: Vec<SurfaceKind>) -> LevelCollision {
        if kinds.len() == self.caster.tri_count() {
            self.kinds = kinds;
        }
        self
    }

    pub fn kind_of(&self, tri: u32) -> SurfaceKind {
        self.kinds.get(tri as usize).copied().unwrap_or_default()
    }

    /// True when any surface kinds are known (the tour can then avoid
    /// hazards; otherwise it treats every floor alike).
    pub fn has_kinds(&self) -> bool {
        !self.kinds.is_empty()
    }

    pub fn bounds(&self) -> (Vec3f, Vec3f) {
        (self.min, self.max)
    }

    pub fn triangles(&self) -> usize {
        self.caster.tri_count()
    }

    /// Nearest surface below `from`, within `max_drop`. `from` should be
    /// the probe START (feet plus the step-up allowance), so a walker can
    /// find ledges above its feet as well as the ground below them.
    pub fn floor_below(&self, from: Vec3f, max_drop: f32) -> Option<FloorHit> {
        let (t, tri) = self.caster.nearest_hit(from, vec3f(0.0, -1.0, 0.0), max_drop)?;
        let (a, b, c) = self.caster.triangle(tri);
        // Winding is not trustworthy in imported maps; flatness is the
        // magnitude of the normal's up component.
        let (e1, e2) = (b - a, c - a);
        let n = vec3f(
            e1.y * e2.z - e1.z * e2.y,
            e1.z * e2.x - e1.x * e2.z,
            e1.x * e2.y - e1.y * e2.x,
        );
        let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1e-9);
        Some(FloorHit {
            y: from.y - t,
            flatness: (n.y / len).abs(),
            kind: self.kind_of(tri),
        })
    }

    /// Nearest surface above `from`, within `max_rise` (a ceiling proves an
    /// interior; the outside of a map has open sky above it).
    pub fn ceiling_above(&self, from: Vec3f, max_rise: f32) -> Option<f32> {
        let (t, _) = self.caster.nearest_hit(from, vec3f(0.0, 1.0, 0.0), max_rise)?;
        Some(from.y + t)
    }

    /// Does the level's own geometry cut the straight line from `from` to
    /// `to`? A line of sight, a shot, a thrown thing: anything that travels
    /// point to point through a streamed map has to ask THIS, because the
    /// map's triangles are not bodies in the sim — a body-only ray cast
    /// passes clean through every wall of the level.
    pub fn segment_blocked(&self, from: Vec3f, to: Vec3f) -> bool {
        let delta = to - from;
        let dist = (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
        if dist < 1e-6 {
            return false;
        }
        self.caster.any_hit(from, delta * (1.0 / dist), dist)
    }

    /// Nearest surface along `dir` (unit length) from `from`, within
    /// `max`: `(distance, unit normal)`, the normal flipped to face back
    /// along the ray — winding is not trustworthy in imported maps, and a
    /// suspension spring wants the push-back direction, not the authored
    /// one.
    pub fn ray_hit(&self, from: Vec3f, dir: Vec3f, max: f32) -> Option<(f32, Vec3f)> {
        let (t, tri) = self.caster.nearest_hit(from, dir, max)?;
        let (a, b, c) = self.caster.triangle(tri);
        let (e1, e2) = (b - a, c - a);
        let mut n = vec3f(
            e1.y * e2.z - e1.z * e2.y,
            e1.z * e2.x - e1.x * e2.z,
            e1.x * e2.y - e1.y * e2.x,
        );
        let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
        n = if len > 1.0e-9 { n * (1.0 / len) } else { dir * -1.0 };
        if n.x * dir.x + n.y * dir.y + n.z * dir.z > 0.0 {
            n = n * -1.0;
        }
        Some((t, n))
    }

    /// The floor a body near `(x, near_y, z)` belongs on.
    ///
    /// Indoors a single probe from far above lands on the CEILING (the
    /// map_actors lesson), so this walks every surface in the column from
    /// the sky down, keeps the ones with at least `head_room` of space
    /// above them (a floor you could stand on), and returns the standable
    /// floor of the room containing `near_y` — else the nearest one. A body
    /// spawned inside a raised slab is lifted onto it; one spawned in the
    /// air belongs to the floor below it. `None` = no floor in this column
    /// (outside the map).
    pub fn ground_under(&self, x: f32, z: f32, near_y: f32, head_room: f32) -> Option<f32> {
        let mut surfaces: Vec<f32> = Vec::new();
        let mut probe = near_y + 200.0;
        for _ in 0..24 {
            let Some(hit) = self.floor_below(vec3f(x, probe, z), 500.0) else { break };
            surfaces.push(hit.y);
            probe = hit.y - 0.02;
        }
        // Standable floors: enough room between this surface and the one
        // above it (the topmost surface has the sky).
        let mut floors: Vec<(f32, f32)> = Vec::new(); // (floor, space above)
        for (i, &y) in surfaces.iter().enumerate() {
            let space = if i == 0 { f32::INFINITY } else { surfaces[i - 1] - y };
            if space >= head_room {
                floors.push((y, space));
            }
        }
        // The room containing near_y wins outright; else nearest floor.
        floors
            .iter()
            .find(|(y, space)| near_y >= *y && near_y < *y + *space)
            .or_else(|| {
                floors.iter().min_by(|a, b| {
                    (a.0 - near_y)
                        .abs()
                        .partial_cmp(&(b.0 - near_y).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            })
            .map(|(y, _)| *y)
    }

    /// The room around `at`: `(headroom, span)` — floor-to-ceiling height
    /// over the floor `at` belongs to, and the narrowest straight
    /// horizontal line through the spot at door height (eight directions,
    /// opposite pairs summed). `None` when there is no ceiling: open
    /// ground, where anything fits.
    pub fn room_at(&self, at: Vec3f) -> Option<(f32, f32)> {
        let floor = self.ground_under(at.x, at.z, at.y, 1.0)?;
        let ceiling = self.ceiling_above(vec3f(at.x, floor + 0.05, at.z), 60.0)?;
        let probe = vec3f(at.x, floor + 1.0, at.z);
        let reach = 30.0;
        let mut dist = [reach; 8];
        for (i, d) in dist.iter_mut().enumerate() {
            let a = std::f32::consts::TAU * i as f32 / 8.0;
            if let Some((t, _)) = self.ray_hit(probe, vec3f(a.sin(), 0.0, -a.cos()), reach) {
                *d = t;
            }
        }
        let span = (0..4)
            .map(|i| dist[i] + dist[i + 4])
            .fold(f32::INFINITY, f32::min);
        Some((ceiling - floor, span))
    }

    /// Are any of these horizontal moves cut by the level? Each pair is a
    /// start/end ground point of one tracked point of a body's footprint
    /// (a wheeled body sweeps its corners and edge midpoints through here).
    /// Rays run at each `height` above `base` — the HIGHER of the two floor
    /// heights, the same rule as [`Self::path_blocked`], so climbing a
    /// step's own riser never reads as a wall.
    pub fn moves_blocked(&self, moves: &[(Vec3f, Vec3f)], base: f32, heights: &[f32]) -> bool {
        for (from, to) in moves {
            let delta = vec3f(to.x - from.x, 0.0, to.z - from.z);
            let dist = (delta.x * delta.x + delta.z * delta.z).sqrt();
            if dist < 1.0e-6 {
                continue;
            }
            let dir = vec3f(delta.x / dist, 0.0, delta.z / dist);
            for &h in heights {
                let origin = vec3f(from.x, base + h, from.z);
                // Barely past the move itself: a fatter margin re-blocked
                // the move AWAY from a wall the body was already against,
                // which is how a car got welded to the plaster it hit.
                if self.caster.any_hit(origin, dir, dist + 0.02) {
                    return true;
                }
            }
        }
        false
    }

    /// Is the straight walk from `from` to `to` (both FEET positions)
    /// obstructed? The body is approximated by rays at knee and chest
    /// height, plus the same pair offset by ±`radius` sideways, so a
    /// doorway narrower than the body reads as blocked.
    ///
    /// The rays are measured above the HIGHER of the two floors. Probing a
    /// rising step this lifts them into the destination's frame, so a
    /// staircase's own next treads (and any thin riser lip an importer
    /// leaves poking above them) never read as walls — the rays are cast
    /// `dist + radius` long, and on a flight steeper than the knee height
    /// per body-radius a start-relative knee ray slams into the tread AFTER
    /// the destination, which is how a perfectly legal staircase used to
    /// have no graph edges at all. A genuine wall is taller than knee
    /// height above EITHER floor and still blocks; flat walks and drops
    /// (probed from the upper floor) are unchanged.
    pub fn path_blocked(
        &self,
        from: Vec3f,
        to: Vec3f,
        radius: f32,
        step_up: f32,
        height: f32,
    ) -> bool {
        let delta = vec3f(to.x - from.x, 0.0, to.z - from.z);
        let dist = (delta.x * delta.x + delta.z * delta.z).sqrt();
        if dist < 1e-6 {
            return false;
        }
        let dir = vec3f(delta.x / dist, 0.0, delta.z / dist);
        let side = vec3f(-dir.z, 0.0, dir.x);
        let base = from.y.max(to.y);
        // Knee starts above the step-up allowance: a ledge you can climb is
        // not a wall, exactly as the step logic assumes.
        let heights = [knee_height(step_up, height), height * 0.85];
        let lanes = [-radius, 0.0, radius];
        for h in heights {
            for l in lanes {
                let origin = vec3f(from.x + side.x * l, base + h, from.z + side.z * l);
                if self.caster.any_hit(origin, dir, dist + radius) {
                    return true;
                }
            }
        }
        false
    }

    /// Is there at least `clear` of empty space all round the body? The
    /// walker keeps this much between its eye and any wall so the camera's
    /// near plane never pokes through geometry.
    ///
    /// The lowest ray sits at the SAME knee height [`path_blocked`] probes
    /// with. The two must agree: any gap between them is a band of wall
    /// tops the walk probe offers and the landing check refuses — the body
    /// walks up to the ledge and then refuses to stand, forever (the E1M1
    /// courtyard rim bug).
    pub fn clearance_ok(&self, feet: Vec3f, clear: f32, step_up: f32, height: f32) -> bool {
        if clear <= 0.0 {
            return true;
        }
        let heights = [knee_height(step_up, height), height * 0.55, height * 0.95];
        for h in heights {
            let origin = vec3f(feet.x, feet.y + h, feet.z);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                if self.caster.any_hit(origin, vec3f(a.sin(), 0.0, -a.cos()), clear) {
                    return false;
                }
            }
        }
        true
    }

    /// Free walking distance along `dir` from `feet`, stopping at a wall,
    /// a step too tall, or the edge of the floor. Marched in body-radius
    /// steps: this is the "how open is this heading" probe.
    pub fn free_run(&self, feet: Vec3f, yaw: f32, cfg: &WalkerConfig) -> f32 {
        let dir = yaw_forward(yaw);
        let step = (cfg.radius * 1.5).max(0.05);
        let mut at = feet;
        let mut travelled = 0.0;
        while travelled < cfg.probe_ahead {
            let want = vec3f(at.x + dir.x * step, at.y, at.z + dir.z * step);
            if self.path_blocked(at, want, cfg.radius, cfg.step_up, cfg.height) {
                break;
            }
            let Some(floor) = self.floor_below(
                vec3f(want.x, want.y + cfg.step_up, want.z),
                cfg.step_up + cfg.fall_limit,
            ) else {
                break; // void ahead
            };
            if floor.y - at.y > cfg.step_up + STEP_EPS || at.y - floor.y > cfg.fall_limit {
                break;
            }
            at = vec3f(want.x, floor.y, want.z);
            travelled += step;
        }
        travelled
    }

    /// How much of the free run along `yaw` crosses hazardous floor
    /// (nukage, lava, slime). Zero when the way is clean — the heading
    /// picker subtracts this so a safe bridge always beats a shortcut
    /// through the ooze.
    pub fn hazard_run(&self, feet: Vec3f, yaw: f32, cfg: &WalkerConfig) -> f32 {
        let dir = yaw_forward(yaw);
        let step = (cfg.radius * 1.5).max(0.05);
        let mut at = feet;
        let (mut travelled, mut bad) = (0.0, 0.0);
        while travelled < cfg.probe_ahead {
            let want = vec3f(at.x + dir.x * step, at.y, at.z + dir.z * step);
            if self.path_blocked(at, want, cfg.radius, cfg.step_up, cfg.height) {
                break;
            }
            let Some(floor) =
                self.floor_below(vec3f(want.x, want.y + cfg.step_up, want.z), cfg.step_up + cfg.fall_limit)
            else {
                break;
            };
            if floor.y - at.y > cfg.step_up + STEP_EPS || at.y - floor.y > cfg.fall_limit {
                break;
            }
            if floor.kind == SurfaceKind::Hazard {
                bad += step;
            }
            at = vec3f(want.x, floor.y, want.z);
            travelled += step;
        }
        bad
    }

    /// A standable spot INSIDE the level: floor under the feet, a ceiling
    /// over the head, and room to stand. Scans a coarse grid and prefers
    /// the most open interior spot nearest the middle of the map.
    ///
    /// This is the fallback for maps whose catalog manifest carries no
    /// `player_start` anchor — today, every classic map.
    pub fn interior_start(&self, cfg: &WalkerConfig) -> Option<Vec3f> {
        let (min, max) = self.bounds();
        let steps = 24;
        let mut best: Option<(f32, Vec3f)> = None;
        for iz in 0..=steps {
            for ix in 0..=steps {
                let x = min.x + (max.x - min.x) * ix as f32 / steps as f32;
                let z = min.z + (max.z - min.z) * iz as f32 / steps as f32;
                for (floor, kind) in self.surfaces_in_column(x, z, cfg) {
                    if kind == SurfaceKind::Hazard {
                        continue; // never spawn the tour in the nukage
                    }
                    let feet = vec3f(x, floor, z);
                    // Room to stand AND somewhere to walk.
                    let open: f32 = (0..4)
                        .map(|q| {
                            self.free_run(feet, q as f32 * std::f32::consts::FRAC_PI_2, cfg)
                        })
                        .sum();
                    if open < cfg.radius * 4.0 {
                        continue;
                    }
                    if best.is_none_or(|(cur, _)| open > cur) {
                        best = Some((open, feet));
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// Floors in the column at `(x, z)` that a body fits on with a ceiling
    /// above it — i.e. interior floors, never the outside of the roof.
    fn surfaces_in_column(&self, x: f32, z: f32, cfg: &WalkerConfig) -> Vec<(f32, SurfaceKind)> {
        let (min, max) = self.bounds();
        // (height, kind, has a ceiling over it)
        let mut raw: Vec<(f32, SurfaceKind, bool)> = Vec::new();
        let mut y = max.y + 0.05;
        // March down through every surface in this column.
        for _ in 0..32 {
            let Some(hit) = self.floor_below(vec3f(x, y, z), y - min.y + 0.1) else {
                break;
            };
            if hit.flatness >= 0.5 {
                let roofed = self
                    .ceiling_above(vec3f(x, hit.y + 0.02, z), MAX_HEADROOM)
                    .is_some_and(|c| c - hit.y >= cfg.height);
                raw.push((hit.y, hit.kind, roofed));
            }
            y = hit.y - 1e-3;
            if y < min.y - 0.05 {
                break;
            }
        }
        // Surfaces come off the march top-down. A ceiling over the head
        // proves we are indoors and the surface is a floor. Open sky above
        // means one of two things — the OUTSIDE OF A ROOF, or genuine open
        // ground — and what tells them apart is whether the column holds
        // another surface underneath: a roof always has its room below it,
        // a courtyard has nothing. (The old rule was "open sky counts only
        // in the lower half of the map", which threw away every outdoor
        // yard on a hill and cut classic maps into disconnected wings.)
        raw.iter()
            .enumerate()
            .filter(|(i, (y, _, roofed))| {
                *roofed || *i + 1 == raw.len() || *y < (min.y + max.y) * 0.5
            })
            .map(|(_, (y, kind, _))| (*y, *kind))
            .collect()
    }
}

/// How far above the head a ceiling may be and still prove "indoors".
const MAX_HEADROOM: f32 = 8.0;

// ---------------------------------------------------------------------------
// Surface kinds out of a GLB
// ---------------------------------------------------------------------------

/// Classify every triangle of a level GLB as floor, hazard or liquid.
///
/// The importer's contract is a `hazard_N` node carrying
/// `extras {kind:"hazard", damage, flat, liquid}`; older files (every
/// classic map published before that lane) carry only the source engine's
/// flat/material name, so the name heuristic stays as a fallback.
///
/// Triangles are counted in EXACTLY the order [`crate::StaticModel`] packs
/// them — nodes in file order, primitives in mesh order, geometry under an
/// animated node left out. `expect_tris` is that model's triangle count and
/// the contract check: a walk that does not reproduce it has drifted from
/// the loader, and `None` (every floor plain) is the honest answer, never a
/// mislabelled map.
pub fn surface_kinds_from_glb(bytes: &[u8], expect_tris: usize) -> Option<Vec<SurfaceKind>> {
    use crate::skin::{JsonParser, Val};
    if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
        return None;
    }
    let mut json_chunk: Option<&[u8]> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        if &bytes[at + 4..at + 8] == b"JSON" {
            json_chunk = bytes.get(at + 8..at + 8 + len);
        }
        at += 8 + len + (4 - len % 4) % 4;
    }
    let json = JsonParser::parse(json_chunk?).ok()?;
    let nodes = json.get("nodes").map(|n| n.arr()).unwrap_or(&[]);
    if nodes.is_empty() {
        return None;
    }
    let mut parents: Vec<Option<usize>> = vec![None; nodes.len()];
    for (p, n) in nodes.iter().enumerate() {
        if let Some(children) = n.get("children") {
            for c in children.arr() {
                if let Some(ci) = c.usize() {
                    if ci < parents.len() {
                        parents[ci] = Some(p);
                    }
                }
            }
        }
    }
    // A node the loader turns into a moving part (`extras.states` naming at
    // least two states) takes its geometry OUT of the static stream.
    let animated: Vec<bool> = nodes
        .iter()
        .map(|n| {
            n.get("extras")
                .and_then(|e| e.get("states"))
                .map(|s| s.arr().iter().filter(|v| v.str().is_some()).count() >= 2)
                .unwrap_or(false)
                && n.get("name").and_then(Val::str).is_some()
        })
        .collect();
    let has_animations = json.get("animations").map(|a| !a.arr().is_empty()).unwrap_or(false);
    // Geometry under a SKY node is the sky's, not the level's — the loader
    // takes it out of the static stream (`StaticModel::sky`), so a walk that
    // counts it can never reproduce the model's triangle count, and the
    // whole classification is discarded as drift. Every classic map with an
    // open-air area publishes one of these, which is exactly the set of maps
    // that also has a nukage pool to mark.
    let sky: Vec<bool> = nodes
        .iter()
        .map(|n| {
            let Some(extras) = n.get("extras") else { return false };
            let marked = extras.get("kind").and_then(Val::str) == Some("sky")
                || n.get("name").and_then(Val::str) == Some("sky");
            // Same contract as the loader: a sky node without a projection
            // it understands stays ordinary geometry.
            marked && extras.get("projection").and_then(Val::str).is_some()
        })
        .collect();
    let owned_by_sky = |mut i: usize| -> bool {
        for _ in 0..64 {
            if sky[i] {
                return true;
            }
            match parents[i] {
                Some(p) => i = p,
                None => break,
            }
        }
        false
    };
    // The node's kind is its own, or the nearest ancestor's that has one:
    // classic importers group a whole nukage sector under one node.
    let kind_of_node = |mut i: usize| -> Option<SurfaceKind> {
        for _ in 0..64 {
            if let Some(k) = node_surface_kind(&nodes[i]) {
                return Some(k);
            }
            match parents[i] {
                Some(p) => i = p,
                None => break,
            }
        }
        None
    };
    let owned_by_part = |mut i: usize| -> bool {
        for _ in 0..64 {
            if animated[i] {
                return true;
            }
            match parents[i] {
                Some(p) => i = p,
                None => break,
            }
        }
        false
    };
    // Two passes: with the anim-part skip (what a level with doors packs)
    // and without (what every file predating that lane packs). The one whose
    // triangle count matches the loaded model is the one to trust.
    let attempts: &[bool] = if has_animations { &[true, false] } else { &[false] };
    for &skip_parts in attempts {
        let mut kinds: Vec<SurfaceKind> = Vec::with_capacity(expect_tris);
        for (ni, n) in nodes.iter().enumerate() {
            let Some(mesh_index) = n.get("mesh").and_then(Val::usize) else { continue };
            if skip_parts && owned_by_part(ni) {
                continue;
            }
            if owned_by_sky(ni) {
                continue;
            }
            let node_kind = kind_of_node(ni);
            let Some(mesh) = json.get("meshes").and_then(|m| m.idx(mesh_index)) else { continue };
            for prim in mesh.get("primitives").map(|p| p.arr()).unwrap_or(&[]) {
                let tris = prim_triangle_count(&json, prim);
                if tris == 0 {
                    continue;
                }
                let kind = node_kind
                    .or_else(|| prim_material_kind(&json, prim))
                    .unwrap_or_default();
                kinds.resize(kinds.len() + tris, kind);
            }
        }
        if kinds.len() == expect_tris && kinds.iter().any(|k| *k != SurfaceKind::Floor) {
            return Some(kinds);
        }
        if skip_parts == has_animations && has_animations {
            continue; // try the no-skip walk
        }
        break;
    }
    None
}

/// A node's own declared surface, from the importer's `hazard_N` contract
/// (`extras {kind, damage, liquid, flat}`) or, failing that, its name.
fn node_surface_kind(node: &crate::skin::Val) -> Option<SurfaceKind> {
    use crate::skin::Val;
    if let Some(extras) = node.get("extras") {
        let declared = extras.get("kind").and_then(Val::str).unwrap_or("");
        let damage = extras.get("damage").and_then(Val::f64).unwrap_or(0.0);
        let liquid = matches!(extras.get("liquid"), Some(Val::Bool(true)))
            || extras.get("liquid").and_then(Val::f64).unwrap_or(0.0) > 0.0;
        if declared.eq_ignore_ascii_case("hazard") {
            // A declared hazard with no damage that calls itself a liquid is
            // just water: crossed, not avoided.
            return Some(if damage <= 0.0 && liquid {
                SurfaceKind::Liquid
            } else {
                SurfaceKind::Hazard
            });
        }
        if damage > 0.0 {
            return Some(SurfaceKind::Hazard);
        }
        if let Some(flat) = extras.get("flat").and_then(Val::str) {
            if let Some(k) = surface_kind_from_name(flat) {
                return Some(k);
            }
        }
        if liquid {
            return Some(SurfaceKind::Liquid);
        }
    }
    node.get("name").and_then(Val::str).and_then(surface_kind_from_name)
}

/// The material's name as a last resort — a Doom import names its material
/// after the flat, which is how every map published before the `hazard_N`
/// contract can still be classified.
fn prim_material_kind(json: &crate::skin::Val, prim: &crate::skin::Val) -> Option<SurfaceKind> {
    use crate::skin::Val;
    let mi = prim.get("material").and_then(Val::usize)?;
    let mat = json.get("materials").and_then(|m| m.idx(mi))?;
    if let Some(k) = mat
        .get("extras")
        .and_then(|e| e.get("flat"))
        .and_then(Val::str)
        .and_then(surface_kind_from_name)
    {
        return Some(k);
    }
    mat.get("name").and_then(Val::str).and_then(surface_kind_from_name)
}

/// Flat/texture names the classic engines use for ground that hurts, and
/// for ground that is merely wet. Matched case-insensitively anywhere in the
/// name, because importers prefix and suffix freely (`doom.NUKAGE1.001`).
fn surface_kind_from_name(name: &str) -> Option<SurfaceKind> {
    let n = name.to_ascii_lowercase();
    // Water first: `fwater` contains neither, but `*water` in Quake and
    // `water` anywhere must not be read as a hazard by a later rule.
    const HAZARD: [&str; 7] = ["nukage", "slime", "lava", "blood", "acid", "sludge", "hazard"];
    const LIQUID: [&str; 3] = ["water", "wtr", "swamp"];
    if HAZARD.iter().any(|h| n.contains(h)) {
        return Some(SurfaceKind::Hazard);
    }
    if LIQUID.iter().any(|l| n.contains(l)) {
        return Some(SurfaceKind::Liquid);
    }
    None
}

/// Triangles a primitive contributes: its index count, or its vertex count
/// when it is drawn unindexed. Only TRIANGLES mode (4, the glTF default)
/// reaches the static stream.
fn prim_triangle_count(json: &crate::skin::Val, prim: &crate::skin::Val) -> usize {
    use crate::skin::Val;
    let count = match prim.get("indices").and_then(Val::usize) {
        Some(acc) => json
            .get("accessors")
            .and_then(|a| a.idx(acc))
            .and_then(|a| a.get("count"))
            .and_then(Val::usize)
            .unwrap_or(0),
        None => prim
            .get("attributes")
            .and_then(|a| a.get("POSITION"))
            .and_then(Val::usize)
            .and_then(|acc| json.get("accessors").and_then(|a| a.idx(acc)))
            .and_then(|a| a.get("count"))
            .and_then(Val::usize)
            .unwrap_or(0),
    };
    count / 3
}

// ---------------------------------------------------------------------------
// Navigation grid
// ---------------------------------------------------------------------------

/// Smallest xz cell the nav grid uses: ONE BODY per cell, i.e. two radii.
///
/// This used to be the bare number 0.5 — a Doom body's width at the Doom
/// importer's metres-per-map-unit. A map published at a different scale (a
/// Quake 1 level is 1/32, twice Doom's 1/64) then got a lattice half the
/// width of the body walking it: several sample points across one body, so
/// every ledge and jamb cut the grid into slivers, the room watershed read
/// the whole map as corridor, and the string-pull had a mandatory waypoint
/// every metre. The cell has to be the body's, or the graph is not a graph
/// of where THIS body can walk.
fn nav_cell_min(cfg: &WalkerConfig) -> f32 {
    // 0.5 is the finest lattice worth probing (a Doom body's width, and the
    // old fixed value); a WIDER body only ever makes it coarser.
    (cfg.radius * 2.0).max(0.5)
}
/// Above this many columns the cell grows instead: a 500-unit outdoor map
/// must not turn into a hundred million probes.
const NAV_MAX_COLUMNS: usize = 40_000;
/// Hard ceiling on the growth above (a cell this big is no longer nav).
const NAV_CELL_MAX: f32 = 4.0;
/// Classic integer grid costs: a diagonal is √2 orthogonals.
const NAV_STEP_COST: u32 = 10;
const NAV_DIAG_COST: u32 = 14;
/// Entering hazardous ground costs twenty ordinary cells, so a bridge is
/// taken whenever a bridge exists and the ooze is still crossable when it
/// is the only way through.
const NAV_HAZARD_COST: u32 = 200;
/// Unreached cell marker in the flood's distance array.
const NAV_UNREACHED: u32 = u32::MAX;

/// One standable spot: a floor point at a column centre, on one of the
/// floors stacked in that column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavCell {
    /// Feet position — the column centre at this floor's height.
    pub pos: Vec3f,
    pub kind: SurfaceKind,
    /// `ix + iz * nx` — which xz column this floor belongs to.
    pub column: u32,
    /// The door part that gates this cell, when one does. The walker asks
    /// the host to open it before walking in.
    pub door: Option<u16>,
    /// Teleporter pad: stepping on this cell moves the body to
    /// `NavGrid::teleport_target`.
    pub teleport: Option<u16>,
    /// This cell is in a pocket the ordinary step cannot leave — a sunken
    /// nukage pool, a lift well — and its way out was linked with the
    /// generous escape step. The body is allowed the same stretch while it
    /// stands here, or it would refuse the edge the graph is routing it
    /// along and grind along the rim instead.
    pub escape: bool,
}

/// A level's walkable cells and how they connect: built ONCE per level from
/// [`LevelCollision`], then used to plan real routes.
///
/// The old walker scored twelve headings from where it stood, which makes a
/// corridor a local optimum: the longest free run is always the way it just
/// came, so it paced between two staircases forever. A graph over the whole
/// map replaces "which way looks open" with "which part of the map have I
/// not seen yet, and how do I get there".
///
/// Columns are a regular xz lattice; a column holds EVERY floor stacked in
/// it (a room over a room is two cells), which is what makes multi-storey
/// maps work. Edges are 8-neighbour, refused through walls by the same
/// knee/chest probe the walker's own step uses.
pub struct NavGrid {
    cell: f32,
    /// World position of column (0, 0)'s centre in x/z.
    origin_x: f32,
    origin_z: f32,
    nx: usize,
    nz: usize,
    cells: Vec<NavCell>,
    /// CSR over columns: column `c` owns `cells[col_start[c]..col_start[c+1]]`.
    col_start: Vec<u32>,
    edge_start: Vec<u32>,
    edge_to: Vec<u32>,
    edge_cost: Vec<u32>,
    /// Which connected piece of the map each cell belongs to, and how big
    /// each piece is. A classic map whose doors are still baked into the
    /// static mesh is a dozen sealed rooms, and the tour has to know that.
    component: Vec<u32>,
    comp_size: Vec<usize>,
    /// Teleporter destinations: `(feet, yaw)`, indexed by `NavCell::teleport`.
    teleports: Vec<(Vec3f, f32)>,
    refused: NavRefusals,
}

/// Why neighbouring cells did NOT become edges. A map the walker cannot
/// leave shows up here before it shows up on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NavRefusals {
    /// Rise over the step limit.
    pub too_tall: usize,
    /// The smallest rise that was still refused — on a Doom map this should
    /// read well over 0.375, and a value AT 0.375 means the step rule is
    /// off by a float.
    pub smallest_refused_rise: f32,
    /// Drop past the fall limit.
    pub too_deep: usize,
    /// A wall between the two centres.
    pub walled: usize,
    /// Edges the escape pass added to un-trap a pocket.
    pub escapes: usize,
}

/// A planned leg: where the tour is going and the cells it walks through.
#[derive(Clone, Debug, PartialEq)]
pub struct NavPlan {
    pub goal: u32,
    /// Cells from the one AFTER the start up to and including the goal.
    pub path: Vec<u32>,
    /// How much of the map the plan could see from the start.
    pub reachable: usize,
    /// Visit count of the LEAST-trodden reachable cell. Once this rises
    /// above one, everything the walker can reach has been seen and the
    /// tour should cut somewhere else.
    pub frontier: f32,
}

impl NavGrid {
    /// Probe the level into a walkable-cell graph. Thousands of short rays:
    /// this belongs on the loader thread, never in a frame.
    pub fn build(level: &LevelCollision, cfg: &WalkerConfig) -> NavGrid {
        let (min, max) = level.bounds();
        let span_x = (max.x - min.x).max(0.001);
        let span_z = (max.z - min.z).max(0.001);
        let mut cell = nav_cell_min(cfg);
        let (mut nx, mut nz);
        loop {
            nx = ((span_x / cell).ceil() as usize).max(1);
            nz = ((span_z / cell).ceil() as usize).max(1);
            if nx * nz <= NAV_MAX_COLUMNS || cell >= NAV_CELL_MAX {
                break;
            }
            cell *= 1.25;
        }
        // Columns are CENTRES, half a cell in from the bounds. A lattice
        // that starts exactly on `min` puts its first column inside the
        // boundary wall, where a zero-length clearance ray reports "clear"
        // and a cell appears in the masonry.
        let (origin_x, origin_z) = (min.x + cell * 0.5, min.z + cell * 0.5);
        let columns = nx * nz;
        let mut cells: Vec<NavCell> = Vec::new();
        let mut col_start: Vec<u32> = Vec::with_capacity(columns + 1);
        for iz in 0..nz {
            for ix in 0..nx {
                col_start.push(cells.len() as u32);
                let x = origin_x + ix as f32 * cell;
                let z = origin_z + iz as f32 * cell;
                for (y, kind) in level.surfaces_in_column(x, z, cfg) {
                    let feet = vec3f(x, y, z);
                    // The cell exists only if the BODY fits standing here:
                    // a capsule of the walker's radius, its full height.
                    if !level.clearance_ok(feet, cfg.radius, cfg.step_up, cfg.height) {
                        continue;
                    }
                    cells.push(NavCell {
                        pos: feet,
                        kind,
                        column: (ix + iz * nx) as u32,
                        door: None,
                        teleport: None,
                        escape: false,
                    });
                }
            }
        }
        col_start.push(cells.len() as u32);
        let mut grid = NavGrid {
            cell,
            origin_x,
            origin_z,
            nx,
            nz,
            cells,
            col_start,
            edge_start: Vec::new(),
            edge_to: Vec::new(),
            edge_cost: Vec::new(),
            component: Vec::new(),
            comp_size: Vec::new(),
            teleports: Vec::new(),
            refused: NavRefusals { smallest_refused_rise: f32::MAX, ..NavRefusals::default() },
        };
        grid.link(level, cfg);
        grid.label_components();
        grid.link_escapes(level, cfg);
        grid
    }

    /// Second linking pass: every cell in a pocket that is NOT the biggest
    /// piece of the map gets its refused neighbours retried with the
    /// generous escape step.
    ///
    /// A Doom nukage pool sits 24 map units below its rim, which is exactly
    /// the step limit — and a float baked through the importer lands either
    /// side of it. One pool that reads a hair too tall is a walker who can
    /// never leave the goo. The rule stays Doom's everywhere the map is
    /// already connected; it only relaxes where the alternative is a trap.
    fn link_escapes(&mut self, level: &LevelCollision, cfg: &WalkerConfig) {
        const ORTHO: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        // Two rounds, so a pool that escapes into a ledge that was itself
        // trapped ends up connected to the map rather than to another trap.
        for _ in 0..2 {
            let Some(core) = self.best_start() else { return };
            // Which cells can REACH the core. Connectivity here is directed:
            // you fall into a nukage pool through a one-way drop, so the
            // pool is "reachable" from the map while being a prison. Only
            // the reverse flood tells the two apart.
            let out = self.can_reach(core);
            let mut added: Vec<(u32, u32, u32)> = Vec::new();
            for a in 0..self.cells.len() {
                if out[a] {
                    continue;
                }
                let (ix, iz) = self.column_xz(self.cells[a].column);
                for (dx, dz) in ORTHO {
                    let Some(col) = self.column_index(ix as i32 + dx, iz as i32 + dz) else {
                        continue;
                    };
                    for b in self.column_cells(col) {
                        if !out[b] {
                            continue; // no use escaping into another prison
                        }
                        let dy = self.cells[b].pos.y - self.cells[a].pos.y;
                        let reach = cfg.step_up + STEP_EPS + hazard_escape_step(cfg.step_up);
                        if dy <= cfg.step_up + STEP_EPS || dy > reach {
                            continue; // already linked, or genuinely a cliff
                        }
                        // The knee ray starts just above the step the body
                        // is allowed to climb. Probing an ESCAPE step with
                        // the ordinary knee height hits the riser of the
                        // very step it is asking about, and every way out
                        // of the pool reads as a wall.
                        if level.path_blocked(
                            self.cells[a].pos,
                            self.cells[b].pos,
                            cfg.radius,
                            (cfg.step_up + hazard_escape_step(cfg.step_up)).min(cfg.height * 0.8),
                            cfg.height,
                        ) {
                            continue;
                        }
                        added.push((a as u32, b as u32, NAV_STEP_COST * 3));
                    }
                }
            }
            if added.is_empty() {
                return;
            }
            self.refused.escapes += added.len();
            for (a, b, cost) in added {
                self.cells[a as usize].escape = true;
                self.push_edge(a, b, cost);
            }
            self.label_components();
        }
    }

    /// Cells from which `target` can be walked to — a flood over the
    /// REVERSED edges. The answer to "is this a place you can leave".
    pub fn can_reach(&self, target: u32) -> Vec<bool> {
        let n = self.cells.len();
        let mut rev_head: Vec<u32> = vec![u32::MAX; n];
        let mut rev_next: Vec<u32> = vec![u32::MAX; self.edge_to.len()];
        let mut rev_from: Vec<u32> = vec![0; self.edge_to.len()];
        for a in 0..n {
            let (s, e) = (self.edge_start[a] as usize, self.edge_start[a + 1] as usize);
            for k in s..e {
                let b = self.edge_to[k] as usize;
                rev_from[k] = a as u32;
                rev_next[k] = rev_head[b];
                rev_head[b] = k as u32;
            }
        }
        let mut seen = vec![false; n];
        if (target as usize) < n {
            seen[target as usize] = true;
        }
        let mut stack = vec![target];
        while let Some(b) = stack.pop() {
            let mut k = rev_head[b as usize];
            while k != u32::MAX {
                let a = rev_from[k as usize] as usize;
                if !seen[a] {
                    seen[a] = true;
                    stack.push(a as u32);
                }
                k = rev_next[k as usize];
            }
        }
        seen
    }

    /// What the build refused, and why — the numbers that answer "why can
    /// the walker not get out of there".
    pub fn refusals(&self) -> NavRefusals {
        self.refused
    }

    /// Flood-fill the graph into connected pieces, largest first in
    /// `comp_size`'s value order (ids are assignment order, not rank).
    fn label_components(&mut self) {
        let n = self.cells.len();
        self.component = vec![u32::MAX; n];
        self.comp_size = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        for seed in 0..n {
            if self.component[seed] != u32::MAX {
                continue;
            }
            let id = self.comp_size.len() as u32;
            let mut size = 0usize;
            self.component[seed] = id;
            stack.push(seed as u32);
            while let Some(a) = stack.pop() {
                size += 1;
                let (s, e) = (
                    self.edge_start[a as usize] as usize,
                    self.edge_start[a as usize + 1] as usize,
                );
                for k in s..e {
                    let b = self.edge_to[k] as usize;
                    if self.component[b] == u32::MAX {
                        self.component[b] = id;
                        stack.push(b as u32);
                    }
                }
            }
            self.comp_size.push(size);
        }
    }

    /// Which connected piece `i` belongs to.
    pub fn component_of(&self, i: u32) -> Option<u32> {
        self.component.get(i as usize).copied().filter(|c| *c != u32::MAX)
    }

    /// Sizes of every connected piece, biggest first — the honest picture
    /// of how cut up a map is.
    pub fn component_sizes(&self) -> Vec<usize> {
        let mut sizes = self.comp_size.clone();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        sizes
    }

    /// Teleporters, as `(pad min, pad max, destination feet, destination
    /// yaw)`. A pad's cells get a ONE-WAY edge to the destination cell, so
    /// the planner routes through a teleporter exactly as Doom's player
    /// does, and the walker cuts to the far side when it steps on one.
    pub fn mark_teleports(&mut self, pads: &[(Vec3f, Vec3f, Vec3f, f32)]) {
        for (min, max, dst, yaw) in pads {
            if self.teleports.len() > u16::MAX as usize {
                break;
            }
            let Some(target) = self.cell_at(*dst) else { continue };
            let id = self.teleports.len() as u16;
            self.teleports.push((*dst, *yaw));
            let pad = self.cell * 0.5;
            let mut sources: Vec<u32> = Vec::new();
            for (i, c) in self.cells.iter_mut().enumerate() {
                if c.pos.x < min.x - pad || c.pos.x > max.x + pad {
                    continue;
                }
                if c.pos.z < min.z - pad || c.pos.z > max.z + pad {
                    continue;
                }
                if c.pos.y < min.y - 1.0 || c.pos.y > max.y + 1.0 {
                    continue;
                }
                c.teleport = Some(id);
                sources.push(i as u32);
            }
            for s in sources {
                self.push_edge(s, target, NAV_STEP_COST);
            }
        }
        self.label_components();
    }

    /// Append one directed edge after the graph is built. CSR is rebuilt
    /// for the tail only: teleporters are a handful, not a hot path.
    fn push_edge(&mut self, from: u32, to: u32, cost: u32) {
        let at = self.edge_start[from as usize + 1] as usize;
        self.edge_to.insert(at, to);
        self.edge_cost.insert(at, cost);
        for s in self.edge_start.iter_mut().skip(from as usize + 1) {
            *s += 1;
        }
    }

    /// A teleporter's destination.
    pub fn teleport_target(&self, id: u16) -> Option<(Vec3f, f32)> {
        self.teleports.get(id as usize).copied()
    }

    /// Orthogonal edges first (one wall probe each), then diagonals, which
    /// are allowed only where BOTH flanking orthogonals are — a body cannot
    /// squeeze through the corner between two walls.
    fn link(&mut self, level: &LevelCollision, cfg: &WalkerConfig) {
        let n = self.cells.len();
        let mut refused = NavRefusals { smallest_refused_rise: f32::MAX, ..Default::default() };
        let mut adj: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
        const ORTHO: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        const DIAG: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
        for a in 0..n {
            let (ix, iz) = self.column_xz(self.cells[a].column);
            for (dx, dz) in ORTHO {
                let (jx, jz) = (ix as i32 + dx, iz as i32 + dz);
                let Some(col) = self.column_index(jx, jz) else { continue };
                // One wall probe serves every floor in the neighbour column
                // only if they share the walk height, so probe per candidate.
                for b in self.column_cells(col) {
                    match self.step_cost(level, cfg, a, b, NAV_STEP_COST) {
                        StepVerdict::Ok(cost) => adj[a].push((b as u32, cost)),
                        StepVerdict::TooTall(rise) => {
                            refused.too_tall += 1;
                            refused.smallest_refused_rise =
                                refused.smallest_refused_rise.min(rise);
                        }
                        StepVerdict::TooDeep => refused.too_deep += 1,
                        StepVerdict::Walled => refused.walled += 1,
                    }
                }
            }
        }
        // A diagonal needs its two flanking orthogonal moves to exist, from
        // the SAME cell — that is the standard no-corner-cutting rule.
        for a in 0..n {
            let (ix, iz) = self.column_xz(self.cells[a].column);
            for (dx, dz) in DIAG {
                let Some(col) = self.column_index(ix as i32 + dx, iz as i32 + dz) else { continue };
                let flank_x = self.column_index(ix as i32 + dx, iz as i32);
                let flank_z = self.column_index(ix as i32, iz as i32 + dz);
                let has = |col: Option<usize>| match col {
                    Some(c) => adj[a].iter().any(|(b, _)| {
                        self.cells[*b as usize].column == c as u32
                    }),
                    None => false,
                };
                if !has(flank_x) || !has(flank_z) {
                    continue;
                }
                for b in self.column_cells(col) {
                    if let StepVerdict::Ok(cost) = self.step_cost(level, cfg, a, b, NAV_DIAG_COST) {
                        adj[a].push((b as u32, cost));
                    }
                }
            }
        }
        self.edge_start = Vec::with_capacity(n + 1);
        for list in &adj {
            self.edge_start.push(self.edge_to.len() as u32);
            for (b, cost) in list {
                self.edge_to.push(*b);
                self.edge_cost.push(*cost);
            }
        }
        self.edge_start.push(self.edge_to.len() as u32);
        self.refused = refused;
    }

    /// Cost of stepping a → b, or `None` when the body cannot make it:
    /// a rise over the step-up allowance, a drop past the fall limit, or a
    /// wall between the two centres. A drop between those two is ONE-WAY —
    /// you can walk off a ledge you cannot climb back up.
    fn step_cost(
        &self,
        level: &LevelCollision,
        cfg: &WalkerConfig,
        a: usize,
        b: usize,
        base: u32,
    ) -> StepVerdict {
        let (pa, pb) = (self.cells[a].pos, self.cells[b].pos);
        let dy = pb.y - pa.y;
        // Same inclusive step rule the body uses, and the same extra reach
        // out of a hazard: a nukage pit whose rim is exactly one step tall
        // must be a place you can leave.
        let reach = cfg.step_up
            + STEP_EPS
            + if self.cells[a].kind == SurfaceKind::Hazard { hazard_escape_step(cfg.step_up) } else { 0.0 };
        if dy > reach {
            return StepVerdict::TooTall(dy);
        }
        if -dy > cfg.fall_limit {
            return StepVerdict::TooDeep;
        }
        if level.path_blocked(pa, pb, cfg.radius, cfg.step_up, cfg.height) {
            return StepVerdict::Walled;
        }
        let hazard = if self.cells[b].kind == SurfaceKind::Hazard { NAV_HAZARD_COST } else { 0 };
        // A drop is walkable but not free: prefer the stairs.
        let fall = if dy < -cfg.step_up { base } else { 0 };
        StepVerdict::Ok(base + hazard + fall)
    }

    fn column_xz(&self, column: u32) -> (usize, usize) {
        let c = column as usize;
        (c % self.nx, c / self.nx)
    }

    fn column_index(&self, ix: i32, iz: i32) -> Option<usize> {
        if ix < 0 || iz < 0 || ix as usize >= self.nx || iz as usize >= self.nz {
            return None;
        }
        Some(ix as usize + iz as usize * self.nx)
    }

    fn column_cells(&self, column: usize) -> std::ops::Range<usize> {
        let (s, e) = (self.col_start[column] as usize, self.col_start[column + 1] as usize);
        s..e
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.nx, self.nz)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn edge_count(&self) -> usize {
        self.edge_to.len()
    }

    pub fn cell(&self, i: u32) -> Option<&NavCell> {
        self.cells.get(i as usize)
    }

    /// Every outgoing edge of `i`, as `(neighbour, cost)`.
    pub fn edges(&self, i: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let (s, e) = (self.edge_start[i as usize] as usize, self.edge_start[i as usize + 1] as usize);
        (s..e).map(move |k| (self.edge_to[k], self.edge_cost[k]))
    }

    /// The cell a body at `p` is standing in: same column (or a neighbouring
    /// one, since a cell centre is up to half a cell away), the floor whose
    /// height is nearest the feet.
    pub fn cell_at(&self, p: Vec3f) -> Option<u32> {
        let fx = ((p.x - self.origin_x) / self.cell).round();
        let fz = ((p.z - self.origin_z) / self.cell).round();
        if !fx.is_finite() || !fz.is_finite() {
            return None;
        }
        let (cx, cz) = (fx as i32, fz as i32);
        let mut best: Option<(f32, u32)> = None;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let Some(col) = self.column_index(cx + dx, cz + dz) else { continue };
                for i in self.column_cells(col) {
                    let c = &self.cells[i];
                    let dy = (c.pos.y - p.y).abs();
                    // A floor more than a step out of reach is a different
                    // storey, not the one under these feet.
                    if dy > 1.0 + self.cell {
                        continue;
                    }
                    let dxz = (c.pos.x - p.x).abs().max((c.pos.z - p.z).abs());
                    let score = dy * 4.0 + dxz;
                    if best.is_none_or(|(cur, _)| score < cur) {
                        best = Some((score, i as u32));
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Mark the cells a door part's footprint covers, by index into `doors`.
    /// Cells under a door are walkable — the walker opens it on approach —
    /// so the door must NOT also be part of the collision mesh.
    pub fn mark_doors(&mut self, doors: &[(Vec3f, Vec3f)]) {
        for (d, (min, max)) in doors.iter().enumerate() {
            if d > u16::MAX as usize {
                break;
            }
            let pad = self.cell * 0.5;
            for c in self.cells.iter_mut() {
                if c.pos.x < min.x - pad || c.pos.x > max.x + pad {
                    continue;
                }
                if c.pos.z < min.z - pad || c.pos.z > max.z + pad {
                    continue;
                }
                // The door's own storey: its box starts at that floor.
                if c.pos.y < min.y - 1.0 || c.pos.y > max.y + 1.0 {
                    continue;
                }
                c.door = Some(d as u16);
            }
        }
    }

    /// Dijkstra from `from` over every edge not in `blocked`, filling
    /// `dist`/`parent` (both resized to the cell count). Returns how many
    /// cells were reached, `from` included.
    pub fn flood(
        &self,
        from: u32,
        blocked: &std::collections::HashSet<u64>,
        dist: &mut Vec<u32>,
        parent: &mut Vec<u32>,
    ) -> usize {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        dist.clear();
        dist.resize(self.cells.len(), NAV_UNREACHED);
        parent.clear();
        parent.resize(self.cells.len(), NAV_UNREACHED);
        if from as usize >= self.cells.len() {
            return 0;
        }
        dist[from as usize] = 0;
        let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
        heap.push(Reverse((0, from)));
        let mut reached = 0;
        while let Some(Reverse((d, a))) = heap.pop() {
            if d > dist[a as usize] {
                continue;
            }
            reached += 1;
            for (b, cost) in self.edges(a) {
                if blocked.contains(&edge_key(a, b)) {
                    continue;
                }
                let nd = d + cost;
                if nd < dist[b as usize] {
                    dist[b as usize] = nd;
                    parent[b as usize] = a;
                    heap.push(Reverse((nd, b)));
                }
            }
        }
        reached
    }

    /// Where to drop the tour: the cell nearest the middle of the LARGEST
    /// connected component, never on hazardous ground.
    ///
    /// This replaces the old open-space heuristic outright. That one scored
    /// a 24×24 lattice by how far it could walk in four directions, which
    /// happily picked a wide sealed courtyard the walker could never leave;
    /// "the biggest piece of the map that is actually one piece" cannot.
    pub fn best_start(&self) -> Option<u32> {
        let best_comp = (0..self.comp_size.len() as u32)
            .max_by_key(|c| self.comp_size[*c as usize])?;
        self.middle_of(best_comp)
    }

    /// The cell nearest the middle of one connected piece, never on
    /// hazardous ground.
    pub fn middle_of(&self, comp: u32) -> Option<u32> {
        let (mut cx, mut cz, mut count) = (0.0f32, 0.0f32, 0.0f32);
        for (i, c) in self.component.iter().enumerate() {
            if *c == comp {
                cx += self.cells[i].pos.x;
                cz += self.cells[i].pos.z;
                count += 1.0;
            }
        }
        if count == 0.0 {
            return None;
        }
        let (cx, cz) = (cx / count, cz / count);
        let mut best: Option<(f32, u32)> = None;
        for (i, c) in self.component.iter().enumerate() {
            if *c != comp || self.cells[i].kind == SurfaceKind::Hazard {
                continue;
            }
            let p = self.cells[i].pos;
            let d = (p.x - cx) * (p.x - cx) + (p.z - cz) * (p.z - cz);
            if best.is_none_or(|(cur, _)| d < cur) {
                best = Some((d, i as u32));
            }
        }
        best.map(|(_, i)| i)
    }

    /// Somewhere else to continue the tour when the region round the walker
    /// has been seen: the middle of the biggest piece of the map whose cells
    /// are least trodden. Deterministic — `visits` and the sizes decide.
    pub fn next_region(&self, from: u32, visits: &[f32]) -> Option<u32> {
        let here = self.component_of(from);
        let mut best: Option<(f32, usize, u32)> = None;
        for comp in 0..self.comp_size.len() as u32 {
            if Some(comp) == here || self.comp_size[comp as usize] < 4 {
                continue;
            }
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for (i, c) in self.component.iter().enumerate() {
                if *c == comp {
                    sum += visits.get(i).copied().unwrap_or(0.0);
                    n += 1.0;
                }
            }
            let mean = if n > 0.0 { sum / n } else { 0.0 };
            let size = self.comp_size[comp as usize];
            // Least trodden first, biggest first among equals.
            let better = match best {
                None => true,
                Some((bm, bs, _)) => mean < bm - 1e-6 || ((mean - bm).abs() <= 1e-6 && size > bs),
            };
            if better {
                best = Some((mean, size, comp));
            }
        }
        best.and_then(|(_, _, comp)| self.middle_of(comp))
    }

    /// How much of the map is reachable from `i` (a spawn sanity check).
    pub fn reachable_from(&self, i: u32) -> usize {
        let (mut dist, mut parent) = (Vec::new(), Vec::new());
        self.flood(i, &std::collections::HashSet::new(), &mut dist, &mut parent)
    }

    /// Waypoints from (but excluding) the flood's start to `goal`.
    pub fn path_to(&self, goal: u32, parent: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        let mut at = goal;
        // Bounded by the cell count: a parent chain cannot be longer.
        for _ in 0..=self.cells.len() {
            out.push(at);
            let p = *parent.get(at as usize).unwrap_or(&NAV_UNREACHED);
            if p == NAV_UNREACHED {
                break;
            }
            at = p;
        }
        out.reverse();
        // The first entry is the start cell itself.
        if !out.is_empty() {
            out.remove(0);
        }
        out
    }

    /// Pick the next leg: the least-visited cell the tour can reach,
    /// nearest first, with a minimum leg length so it does not spend the
    /// evening shuffling one cell at a time along the frontier.
    ///
    /// `jitter` (0..1) breaks ties deterministically from the walker's seed.
    pub fn explore(
        &self,
        from: u32,
        visits: &[f32],
        blocked: &std::collections::HashSet<u64>,
        jitter: f32,
        dist: &mut Vec<u32>,
        parent: &mut Vec<u32>,
    ) -> Option<NavPlan> {
        let reachable = self.flood(from, blocked, dist, parent);
        // One visit is worth forty cells of walking: an unseen room across
        // the map beats trodden ground next door, which is the whole point.
        const VISIT_PENALTY: f32 = 400.0;
        /// Shorter legs than this are not worth replanning for.
        const MIN_LEG: u32 = 100;
        let mut best: Option<(f32, u32)> = None;
        let mut best_near: Option<(f32, u32)> = None;
        let mut frontier = f32::MAX;
        for i in 0..self.cells.len() {
            let d = dist[i];
            if d == NAV_UNREACHED || i as u32 == from {
                continue;
            }
            if self.cells[i].kind == SurfaceKind::Hazard {
                continue; // never park the tour in the ooze
            }
            frontier = frontier.min(visits.get(i).copied().unwrap_or(0.0));
            let score = d as f32
                + visits.get(i).copied().unwrap_or(0.0) * VISIT_PENALTY
                + jitter * 30.0 * ((i % 7) as f32 / 7.0);
            if best_near.is_none_or(|(cur, _)| score < cur) {
                best_near = Some((score, i as u32));
            }
            if d >= MIN_LEG && best.is_none_or(|(cur, _)| score < cur) {
                best = Some((score, i as u32));
            }
        }
        let (_, goal) = best.or(best_near)?;
        Some(NavPlan {
            goal,
            path: self.path_to(goal, parent),
            reachable,
            frontier: if frontier == f32::MAX { 0.0 } else { frontier },
        })
    }
}

/// What happened to one candidate step of the body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepRefusal {
    Ok { rise: f32 },
    /// The knee/chest probe found geometry between here and there.
    Walled,
    /// No floor at all under the destination.
    NoFloor,
    TooTall { rise: f32, reach: f32 },
    Pit { drop: f32 },
    /// The landing is closer than a body radius to a wall.
    NoClearance { rise: f32 },
}

/// Why one neighbour did or did not become an edge.
enum StepVerdict {
    Ok(u32),
    /// Rise, in world units — the number that says whether the step rule is
    /// off by a float or the wall is genuinely too tall.
    TooTall(f32),
    TooDeep,
    Walled,
}

/// Undirected-safe key for one DIRECTED edge (a→b is invalidated on its own:
/// a ledge you fell off may still be walkable the other way).
fn edge_key(a: u32, b: u32) -> u64 {
    (a as u64) << 32 | b as u64
}

/// What the tour is doing right now, for the trace log.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NavStats {
    pub cells: usize,
    pub reachable: usize,
    pub at: Option<u32>,
    pub goal: Option<u32>,
    pub path_left: usize,
    /// Cells stood in at least once since the level loaded.
    pub distinct: usize,
    /// Reachable cells the tour has never stood in (or has forgotten).
    pub unseen: usize,
    /// Visit count of the least-trodden reachable cell.
    pub frontier: f32,
    pub replans: u64,
    pub invalidated: usize,
    /// Teleporter jumps + exhausted-region cuts so far.
    pub cuts: u64,
}

/// Which game's head-bob the camera imitates. Constants are the originals
/// where the source is public; amplitudes are a fraction of eye height so
/// they survive each importer's unit conversion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BobStyle {
    /// Doom `P_CalcHeight` (p_user.c): `bob = (momx² + momy²) >> 2` capped
    /// at `MAXBOB` (16 units), applied as `bob/2 · sin(angle)` with
    /// `angle = FINEANGLES/20 · leveltime` — one cycle per 20 tics, 1.75 Hz
    /// at 35 tics/s. Peak 8 of 41 view-height units.
    #[default]
    Doom,
    /// Build engine (Duke 3D): `bobcounter` advances with horizontal speed
    /// and drives the view offset — quicker and deeper than Doom, with the
    /// engine's lateral sway. Matched by feel; the fixed-point constants
    /// are not transcribed.
    Duke,
    /// Quake `V_CalcBob` (view.c): `cl_bobcycle 0.6`, `cl_bobup 0.5`,
    /// `cl_bob 0.02` × speed, shaped `0.3 + 0.7·sin`, clamped +4/−7 units;
    /// plus `cl_rollangle 2.0` roll as the view swings.
    Quake,
    None,
}

impl BobStyle {
    /// Family from a catalog alias/namespace (`doom/doom/worlds/…`,
    /// `duke/duke3d/…`, `quake/id1/…`). Unknown sources keep Doom's.
    pub fn from_source(text: &str) -> BobStyle {
        let t = text.to_ascii_lowercase();
        if t.contains("duke") || t.contains("build") {
            BobStyle::Duke
        } else if t.contains("quake") || t.contains("q3") || t.contains("q2") {
            BobStyle::Quake
        } else {
            BobStyle::Doom
        }
    }

    fn frequency(self) -> f32 {
        match self {
            BobStyle::Doom => 35.0 / 20.0,
            BobStyle::Duke => 2.2,
            BobStyle::Quake => 1.0 / 0.6,
            BobStyle::None => 0.0,
        }
    }

    /// Peak vertical travel as a fraction of eye height.
    ///
    /// Doom's MAXBOB/2 is 8 map units — a quarter of a metre, which reads
    /// as seasickness on a big screen at 60 fps rather than as walking.
    /// These are the originals' CADENCE at a comfortable magnitude: about
    /// 3 cm at walk speed for Doom/Build, 2 cm for Quake (`cl_bob 0.02` ×
    /// speed lands there too).
    fn amplitude(self) -> f32 {
        match self {
            BobStyle::Doom => 0.024,
            BobStyle::Duke => 0.030,
            BobStyle::Quake => 0.016,
            BobStyle::None => 0.0,
        }
    }

    fn sway(self) -> f32 {
        match self {
            BobStyle::Duke => 0.012,
            _ => 0.0,
        }
    }

    fn roll(self) -> f32 {
        match self {
            BobStyle::Quake => 2.0f32.to_radians(),
            _ => 0.0,
        }
    }

    fn wave(self, phase: f32) -> f32 {
        let p = phase.rem_euclid(1.0);
        match self {
            BobStyle::Quake => {
                const BOB_UP: f32 = 0.5;
                let cycle = if p < BOB_UP {
                    std::f32::consts::PI * p / BOB_UP
                } else {
                    std::f32::consts::PI + std::f32::consts::PI * (p - BOB_UP) / (1.0 - BOB_UP)
                };
                0.3 + 0.7 * cycle.sin()
            }
            BobStyle::None => 0.0,
            _ => (p * std::f32::consts::TAU).sin(),
        }
    }
}

/// Body + gait, in world units. Defaults are the classic-importer scale
/// (Doom map units / 64): a 41-unit view height is 0.64, a 24-unit step is
/// 0.375.
#[derive(Clone, Copy, Debug)]
pub struct WalkerConfig {
    pub eye_height: f32,
    /// Body radius — the originals all use 16 map units (0.25 here). The
    /// walker never ends a step with a wall closer than this, which is
    /// also what keeps the camera's near plane out of the geometry.
    pub radius: f32,
    pub height: f32,
    pub speed: f32,
    pub turn_rate: f32,
    /// Ledge the walker steps straight onto, the way the originals do
    /// (Doom 24 map units = 0.375, Quake 18 = 0.28).
    pub step_up: f32,
    /// Deepest drop it will walk off. Beyond this it turns away rather
    /// than diving into a pit; a void (no floor at all) always refuses.
    pub fall_limit: f32,
    /// Downward acceleration while falling, world units/s². Doom's
    /// GRAVITY is 1 map unit per tic² = 19.1 u/s² at 35 tics; Quake's is
    /// 800 Quake units/s² ≈ 12.5 here.
    pub gravity: f32,
    pub probe_ahead: f32,
    pub repick_secs: f32,
    pub bob: BobStyle,
}

impl Default for WalkerConfig {
    fn default() -> Self {
        Self {
            eye_height: 0.64,
            radius: 0.25,
            height: 0.85,
            speed: 1.6,
            turn_rate: 1.6,
            step_up: 0.375,
            fall_limit: 2.5,
            gravity: 19.1,
            probe_ahead: 6.0,
            repick_secs: 4.0,
            bob: BobStyle::Doom,
        }
    }
}

impl WalkerConfig {
    /// The body/gait each engine gives its player, at the classic
    /// importer's ORIGINAL metres per map unit (1/64).
    ///
    /// These are ratios of an engine's own constants, not the map's units:
    /// an importer is free to publish at whatever scale suits its source
    /// (Quake 1 is 1/32, twice this), and every such map DECLARES its step
    /// height. Use [`Self::with_declared`] to land the body in the units of
    /// the map it is about to walk — a preset alone is a guess.
    pub fn for_style(bob: BobStyle) -> WalkerConfig {
        let base = WalkerConfig { bob, ..WalkerConfig::default() };
        match bob {
            // Doom: radius 16, height 56, view 41, step 24, gravity 1/tic².
            BobStyle::Doom | BobStyle::None => base,
            // Quake: radius 16, height 56, view 22 above the feet of a
            // 56-tall box, step 18, gravity 800 u/s².
            BobStyle::Quake => WalkerConfig {
                eye_height: 0.66,
                step_up: 0.28,
                gravity: 12.5,
                speed: 1.9,
                ..base
            },
            // Build (Duke): comparable body, brisker walk.
            BobStyle::Duke => WalkerConfig {
                eye_height: 0.62,
                step_up: 0.34,
                gravity: 16.0,
                speed: 1.8,
                ..base
            },
        }
    }

    /// Put the preset body into the units of the map it is about to walk,
    /// from the facts the importer DECLARED about that map.
    ///
    /// Every classic converter publishes its engine's step height and eye
    /// height as anchors, in the GLB's own metres — `step_height` /
    /// `eye_height` (`world_nav.rs`). A preset says "18 units"; the map says
    /// what a unit is worth. Doom's 24 at 1/64 is 0.375 and matches the
    /// preset; Quake 1's 18 at **1/32** is 0.5625, exactly twice the 18-at-
    /// 1/64 the Quake preset carries (Quake II and III publish at 1/64, so
    /// the preset was written for those and silently halved the body on
    /// every Quake 1 map).
    ///
    /// A halved body is not a smaller walker, it is a broken one: it cannot
    /// climb the map's own stairs, its nav lattice is finer than its own
    /// width, and the graph it plans on stops being a graph of where it can
    /// go. So the declared step sets the SCALE and the whole body travels
    /// with it — radius, height, eye, step, fall limit, walking speed, and
    /// gravity (length/s², so once as well). Angles and durations do not
    /// scale, and neither does the bob (already a fraction of eye height).
    ///
    /// Declaring nothing (Build states no step height) keeps the preset.
    pub fn with_declared(mut self, step_height: Option<f32>, eye_height: Option<f32>) -> Self {
        let scale = step_height
            .filter(|s| s.is_finite() && *s > 1.0e-4)
            .map(|s| s / self.step_up.max(1.0e-4))
            // A map whose step is 30× the preset's is not a scale, it is a
            // bad sidecar: keep the body we know rather than invent one.
            .filter(|k| (0.1..=10.0).contains(k) && (*k - 1.0).abs() > 0.02);
        if let Some(k) = scale {
            self.radius *= k;
            self.height *= k;
            self.step_up *= k;
            self.eye_height *= k;
            self.fall_limit *= k;
            self.speed *= k;
            self.gravity *= k;
            self.probe_ahead *= k;
        }
        // The eye is declared outright, so it needs no scaling guess — and
        // `PlayerNav` already reads the same anchor to turn the authored
        // `player_start` eye into feet. The two must agree or the walker
        // spawns at one height and looks from another.
        if let Some(e) = eye_height.filter(|e| e.is_finite() && *e > 0.05) {
            self.eye_height = e;
        }
        self
    }

    /// Radius of the circle a body traces when it walks and turns as hard
    /// as it can — walking speed over turn rate.
    pub fn turn_radius(&self) -> f32 {
        self.speed / self.turn_rate.max(0.01)
    }
}

/// Camera pose produced by one walker tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub eye: Vec3f,
    pub yaw: f32,
    pub roll: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkerEvent {
    Stepped,
    /// The way ahead was solid: the walker turned instead of clipping.
    Blocked,
    /// Nothing under the feet at all — a bad start. The host can respawn.
    Stranded,
}

/// Directions tried when picking a new heading (whole turn, evenly spaced).
const PROBE_DIRS: usize = 12;
/// How far down a landing spot is looked for while falling.
const MAX_FALL_SEARCH: f32 = 40.0;
/// A drop smaller than this is standing on the floor, not falling.
const LAND_EPS: f32 = 0.02;
/// Doom's step rule is `step <= 24` map units — INCLUSIVE. At the classic
/// importer's 1/64 scale that is exactly 0.375, and a sector floor baked
/// through a float pipeline lands on 0.3750001 as often as on 0.375. A rim
/// that is exactly one step tall must never read as a wall, or a walker in
/// the nukage is walled into it.
const STEP_EPS: f32 = 0.02;
/// The ONE knee-probe height above a floor, shared by
/// [`LevelCollision::path_blocked`] (the walk probe — the graph's and the
/// body's "is that a wall?") and [`LevelCollision::clearance_ok`] (the
/// landing check — "can the body stand here?"). It sits safely above a
/// maximal legal step — float noise and the thin riser lip classic
/// importers leave on step edges included — so a climbable ledge is never
/// a wall, while anything meaningfully taller than a step still is. For
/// the Doom body (step 0.375, height 0.85) this is ≈ 0.446.
///
/// The two probes MUST use the same height: any gap between them is a
/// band of wall tops one probe offers and the other refuses forever.
pub(crate) fn knee_height(step_up: f32, height: f32) -> f32 {
    step_up + (height - step_up) * 0.15
}
/// Standing in something that hurts — or in any pocket of floor the normal
/// step cannot leave — the way OUT is worth a bigger stretch than an
/// ordinary step: about 38 map units instead of 24. Being stuck in the goo
/// is worse than a walker who once climbs a step Doom would not have.
///
/// A RATIO of the body's own step, not a length: the same 38-vs-24 stretch
/// has to mean the same thing on a map published at half the metres per map
/// unit. (`0.375 · 0.6133` is Doom's 0.23 to the last float.)
const HAZARD_ESCAPE_RATIO: f32 = 0.23 / 0.375;

pub(crate) fn hazard_escape_step(step_up: f32) -> f32 {
    step_up * HAZARD_ESCAPE_RATIO
}
/// Ticks between forward look-ahead probes (60 Hz → five per second).
const LOOKAHEAD_TICKS: u32 = 12;
/// Time constant of the eased turn (a corner takes about this long).
const TURN_EASE_SECS: f32 = 0.35;
/// How far the camera's yaw trails the body's heading.
const CAM_LAG_SECS: f32 = 0.12;
/// Acceleration/deceleration time.
const SPEED_EASE_SECS: f32 = 0.30;
/// Spring rate of the eye's catch-up after a step up (~0.25 s settle).
const VIEW_CATCHUP_RATE: f32 = 11.0;

pub fn yaw_forward(yaw: f32) -> Vec3f {
    // The renderer's camera convention: yaw 0 looks down -Z.
    vec3f(yaw.sin(), 0.0, -yaw.cos())
}

fn wrap_pi(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut a = a % tau;
    if a > std::f32::consts::PI {
        a -= tau;
    } else if a < -std::f32::consts::PI {
        a += tau;
    }
    a
}

/// Deterministic xorshift64* — a level tour must repeat exactly.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // Mix first: a bare `seed | 1` maps 42 and 43 onto the same state.
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xd1b5_4a32_d192_ed03 | 1)
    }

    fn unit(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        ((x.wrapping_mul(0x2545_f491_4f6c_dd1d)) >> 40) as f32 / 16_777_216.0
    }
}

/// An autonomous first-person walker: stays on real floors, slides along
/// walls, turns away from voids and dead ends, and never ends a tick inside
/// geometry. Deterministic from its seed.
pub struct LevelWalker {
    pos: Vec3f,
    yaw: f32,
    target_yaw: f32,
    cfg: WalkerConfig,
    rng: Rng,
    since_pick: f32,
    bob_phase: f32,
    gait: f32,
    turn_rate: f32,
    stuck_ticks: u32,
    /// Falling state: the originals drop you off a ledge instead of
    /// snapping you to the floor below.
    airborne: bool,
    vel_y: f32,
    /// How far the EYE is currently below its nominal height because the
    /// body just stepped up. Doom keeps `viewheight` separate from the
    /// mobj's z for exactly this: the body snaps onto the tread, the view
    /// catches up over ~0.25 s, and stairs read as a smooth rise instead
    /// of a series of jumps.
    view_offset: f32,
    view_vel: f32,
    /// Eased walking speed, so starts, stops and corners do not snap.
    speed_now: f32,
    /// Camera yaw, lagging the body's heading slightly.
    cam_yaw: f32,
    probe_countdown: u32,
    /// Distance to the point the route is steering at, when there is one.
    /// The tick's speed limit reads it: a body may not walk faster than its
    /// own turn can steer toward what it is aiming at.
    aim_dist: Option<f32>,
    /// Stand still this tick (waiting for a door to open).
    hold: bool,
    /// The cell under the feet is in a pocket the graph had to link with
    /// the escape step: the body gets the same reach while it stands there.
    escape_step: bool,
    // ---- player_nav seam ------------------------------------------------
    /// An outside planner owns the route: the built-in least-visited tour
    /// stops choosing goals, and everything BELOW the heading — eased turn,
    /// step, gravity, wall-slide, bob, door wait, teleport, relocate/flash —
    /// keeps working exactly as it does for the built-in tour.
    external: bool,
    /// Route handed in before the first tick (the nav state does not exist
    /// until a grid has been seen).
    ext_route: Option<Vec<u32>>,
    /// Heading to hold when the route is empty — a look-around pan.
    ext_yaw: Option<f32>,
    /// Door the planner wants opened, when it drives that itself.
    ext_door: Option<u16>,
    // player_nav: stand in place while the route is empty (a look-around
    // pan turns the body without walking it into a wall).
    ext_hold: bool,
    // player_nav: walk-speed scale the planner sets for the slight
    // slow-down at doors and glances. 1.0 = the config's full speed.
    ext_speed: f32,
    /// Exploration state, created the first time a nav grid is handed in.
    nav: Option<NavState>,
}

/// Everything the tour remembers about where it has been. Kept in the
/// walker (not the grid) so two walkers can explore one shared map.
struct NavState {
    /// Per-cell visit count, decayed slowly so a map is eventually re-toured.
    visits: Vec<f32>,
    /// Cells stood in and not yet forgotten. Unlike `visits` this does not
    /// decay every tick, so "have I seen everything I can reach" is a
    /// question with a stable answer; a cell whose visit count has faded
    /// away is forgotten with it.
    seen: Vec<bool>,
    /// Reachable cells never stood in, as of the last plan.
    unseen: usize,
    dist: Vec<u32>,
    parent: Vec<u32>,
    /// Directed edges the walker proved it cannot actually make.
    blocked: std::collections::HashSet<u64>,
    /// Remaining waypoints, in order.
    path: std::collections::VecDeque<u32>,
    goal: Option<u32>,
    at: Option<u32>,
    reachable: usize,
    distinct: usize,
    replans: u64,
    since_plan: f32,
    since_decay: f32,
    /// Seconds since the walker last made real ground toward its waypoint.
    since_progress: f32,
    /// Closest the body has come to the current waypoint. Displacement on
    /// its own is not progress: a walker jiggling along a wall covers
    /// metres a second and arrives nowhere.
    best_gap: f32,
    /// Ticks spent unable to move at all.
    blocked_ticks: u32,
    /// Which way it is currently trying to slide past an obstruction.
    slide: f32,
    /// Door the host should open, and how long we have waited for it.
    want_door: Option<u16>,
    door_wait: f32,
    open_doors: Vec<u16>,
    /// Goals handed out this session, newest last (bounded, for the trace).
    goal_log: Vec<u32>,
    /// Least-visited reachable cell as of the last plan.
    frontier: f32,
    /// The host owes the picture a cut-flash (Doom's teleport white-out).
    flash: bool,
    cuts: u64,
    since_cut: f32,
    /// Unbroken seconds spent standing in something that hurts.
    hazard_secs: f32,
}

/// No progress for this long means the plan is a lie: drop the edge and
/// re-plan rather than grinding into a wall the probe said was open.
const NAV_STUCK_SECS: f32 = 3.0;
/// Ground covered under which "no progress" is declared.
const NAV_PROGRESS_EPS: f32 = 0.25;
/// A leg is re-planned at least this often, so a changed world (a door that
/// opened, an edge that was invalidated) is noticed.
const NAV_REPLAN_SECS: f32 = 12.0;
/// Visit counts fade by this factor every `NAV_DECAY_SECS`.
const NAV_DECAY: f32 = 0.6;
const NAV_DECAY_SECS: f32 = 30.0;
/// How long the tour waits at a closed door before walking on regardless.
const NAV_DOOR_WAIT: f32 = 1.5;
/// How far ahead the tour looks for a door to open.
const NAV_DOOR_LOOKAHEAD: usize = 3;
/// Newest goals kept for the trace log.
const NAV_GOAL_LOG: usize = 64;
/// A tour that has not moved for this long is not going to: cut elsewhere.
/// (A classic map still bakes its doors into the static mesh, so a route
/// can genuinely end at a wall that Doom would have opened.)
const NAV_CUT_AFTER_SECS: f32 = 10.0;
/// Minimum gap between cuts, so a bad corner cannot strobe the picture.
const NAV_CUT_COOLDOWN: f32 = 6.0;
/// Longest the tour may stand in a hazard before it is cut out of it.
const NAV_HAZARD_ESCAPE_SECS: f32 = 6.0;

impl NavState {
    fn new(cells: usize) -> NavState {
        NavState {
            visits: vec![0.0; cells],
            seen: vec![false; cells],
            unseen: cells,
            dist: Vec::new(),
            parent: Vec::new(),
            blocked: std::collections::HashSet::new(),
            path: std::collections::VecDeque::new(),
            goal: None,
            at: None,
            reachable: 0,
            distinct: 0,
            replans: 0,
            since_plan: f32::MAX,
            since_decay: 0.0,
            since_progress: 0.0,
            best_gap: f32::MAX,
            blocked_ticks: 0,
            slide: 0.0,
            want_door: None,
            door_wait: 0.0,
            open_doors: Vec::new(),
            goal_log: Vec::new(),
            frontier: 0.0,
            flash: false,
            cuts: 0,
            since_cut: NAV_CUT_COOLDOWN,
            hazard_secs: 0.0,
        }
    }
}

impl LevelWalker {
    pub fn new(start: Vec3f, yaw: f32, cfg: WalkerConfig, seed: u64) -> LevelWalker {
        LevelWalker {
            pos: start,
            yaw,
            target_yaw: yaw,
            cfg,
            rng: Rng::new(seed),
            since_pick: 0.0,
            bob_phase: 0.0,
            gait: 0.0,
            turn_rate: 0.0,
            stuck_ticks: 0,
            airborne: false,
            vel_y: 0.0,
            view_offset: 0.0,
            view_vel: 0.0,
            speed_now: 0.0,
            cam_yaw: yaw,
            probe_countdown: 0,
            aim_dist: None,
            hold: false,
            escape_step: false,
            external: false,
            ext_route: None,
            ext_yaw: None,
            ext_door: None,
            ext_hold: false,
            ext_speed: 1.0,
            nav: None,
        }
    }

    /// The door the tour wants opened before it walks on, if any. The host
    /// answers with `Renderer::set_model_state(level, "door_N", "open", …)`
    /// and then calls [`Self::set_door_open`].
    pub fn wanted_door(&self) -> Option<u16> {
        self.nav.as_ref().and_then(|n| n.want_door)
    }

    /// Tell the walker a door part is now passable (or shut again).
    pub fn set_door_open(&mut self, door: u16, open: bool) {
        let Some(nav) = self.nav.as_mut() else { return };
        let held = nav.open_doors.iter().position(|d| *d == door);
        match (open, held) {
            (true, None) => nav.open_doors.push(door),
            (false, Some(i)) => {
                nav.open_doors.remove(i);
            }
            _ => {}
        }
        if open && nav.want_door == Some(door) {
            nav.want_door = None;
            nav.door_wait = 0.0;
        }
    }

    /// What the exploration is doing — the numbers `VJ_WALKER_TRACE` logs.
    pub fn nav_stats(&self) -> NavStats {
        match &self.nav {
            Some(n) => NavStats {
                cells: n.visits.len(),
                reachable: n.reachable,
                at: n.at,
                goal: n.goal,
                path_left: n.path.len(),
                distinct: n.distinct,
                unseen: n.unseen,
                frontier: n.frontier,
                replans: n.replans,
                invalidated: n.blocked.len(),
                cuts: n.cuts,
            },
            None => NavStats::default(),
        }
    }

    /// The goals handed out so far, newest last (bounded).
    pub fn nav_goal_log(&self) -> &[u32] {
        self.nav.as_ref().map(|n| n.goal_log.as_slice()).unwrap_or(&[])
    }

    // ---- player_nav seam ----------------------------------------------------
    // A planner that thinks in ROOMS lives outside this module and drives the
    // body through these four calls. Note when routing: a DIAGONAL step out of
    // a rim corner is refused by the landing clearance check (`StepRefusal::
    // NoClearance`) even where the grid offers the edge — orthogonal exits out
    // of a sunken area are the reliable ones, so prefer them.

    /// Hand the route over. While this is on, the built-in least-visited
    /// tour stops picking goals and cutting to other regions; locomotion,
    /// waypoint following, the stuck watchdog, doors, teleporters and the
    /// cut flash all keep working.
    pub fn set_external_planner(&mut self, on: bool) {
        self.external = on;
    }

    pub fn has_external_planner(&self) -> bool {
        self.external
    }

    /// Replace the waypoint queue with `route` (nav-grid cell ids, in order).
    /// Taken verbatim: the walker consumes waypoints as it reaches them and
    /// stops steering when the queue empties.
    pub fn set_route(&mut self, route: Vec<u32>) {
        match self.nav.as_mut() {
            Some(nav) => {
                nav.path = route.into();
                nav.goal = nav.path.back().copied();
                nav.best_gap = f32::MAX;
                nav.since_progress = 0.0;
                nav.blocked_ticks = 0;
                // A fresh route is a fresh plan, so the "try the other way
                // round the obstruction" offset goes with the old one. The
                // built-in tour cleared it on its own replan; an EXTERNAL
                // planner never runs that branch, so once a watchdog set
                // the offset the body steered 34° off every bearing it was
                // given, for the rest of the level. That crab is a body
                // that cannot converge on any waypoint — it circles one
                // forever, which is the tour that looks demented.
                nav.slide = 0.0;
            }
            None => self.ext_route = Some(route),
        }
    }

    /// What is left of the route.
    pub fn route(&self) -> Vec<u32> {
        self.nav.as_ref().map(|n| n.path.iter().copied().collect()).unwrap_or_default()
    }

    /// Hold this heading while the route is empty (a look-around pan).
    /// `None` lets the body keep the heading it has.
    pub fn set_target_yaw(&mut self, yaw: Option<f32>) {
        self.ext_yaw = yaw;
        if let Some(y) = yaw {
            if self.nav.as_ref().is_none_or(|n| n.path.is_empty()) {
                self.target_yaw = wrap_pi(y);
            }
        }
    }

    // player_nav: stand still while the route is empty — the look-around
    // pan turns the body in place instead of walking it forward. Has no
    // effect while a route is being followed (a door wait still holds).
    pub fn set_hold(&mut self, hold: bool) {
        self.ext_hold = hold;
    }

    // player_nav: walk-speed scale (the slight slow-down through doorways
    // and during glances); clamped to 0..1, 1 = the config's full speed.
    pub fn set_speed_scale(&mut self, scale: f32) {
        self.ext_speed = scale.clamp(0.0, 1.0);
    }

    /// Ask the host to open a door (what `wanted_door` will report until
    /// `set_door_open` clears it). The built-in look-ahead does this by
    /// itself for the built-in tour.
    pub fn request_door(&mut self, door: Option<u16>) {
        self.ext_door = door;
        if let Some(nav) = self.nav.as_mut() {
            nav.want_door = door.filter(|d| !nav.open_doors.contains(d));
        }
    }

    /// Put the body somewhere else instantly — a teleporter, or a cut to
    /// another wing of a map the tour cannot walk to. Physics and the eye
    /// spring are reset so the arrival is a CUT, not a fall.
    pub fn relocate(&mut self, feet: Vec3f, yaw: f32) {
        self.pos = feet;
        self.yaw = wrap_pi(yaw);
        self.target_yaw = self.yaw;
        self.cam_yaw = self.yaw;
        self.vel_y = 0.0;
        self.airborne = false;
        self.view_offset = 0.0;
        self.view_vel = 0.0;
        self.speed_now = 0.0;
        self.gait = 0.0;
        self.stuck_ticks = 0;
        self.hold = false;
        if let Some(n) = self.nav.as_mut() {
            n.path.clear();
            n.goal = None;
            n.at = None;
            n.since_plan = f32::MAX;
            n.since_progress = 0.0;
            n.best_gap = f32::MAX;
            n.blocked_ticks = 0;
            n.slide = 0.0;
            n.since_cut = 0.0;
            n.cuts += 1;
            n.flash = true;
        }
    }

    /// True once per cut: the host paints Doom's white teleport flash.
    pub fn take_flash(&mut self) -> bool {
        match self.nav.as_mut() {
            Some(n) => std::mem::replace(&mut n.flash, false),
            None => false,
        }
    }

    /// How many cuts (teleporters + exhausted-region jumps) so far.
    pub fn cut_count(&self) -> u64 {
        self.nav.as_ref().map(|n| n.cuts).unwrap_or(0)
    }

    /// True while the walker is falling (no floor under its feet).
    pub fn is_airborne(&self) -> bool {
        self.airborne
    }

    pub fn feet(&self) -> Vec3f {
        self.pos
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn config(&self) -> &WalkerConfig {
        &self.cfg
    }

    /// Camera for this tick: eye height above the feet plus the engine's
    /// head bob (speed-proportional, zero standing still), and its roll.
    pub fn camera(&self) -> CameraPose {
        let cfg = &self.cfg;
        // Doom's bob is momentum-SQUARED, so it fades away quickly as the
        // walker slows and is zero at rest.
        let effort = self.gait * self.gait;
        let lift = cfg.bob.wave(self.bob_phase) * cfg.bob.amplitude() * cfg.eye_height * effort;
        let sway = cfg.bob.sway()
            * cfg.eye_height
            * effort
            * (self.bob_phase * std::f32::consts::PI).sin();
        let right = vec3f(self.cam_yaw.cos(), 0.0, self.cam_yaw.sin());
        CameraPose {
            eye: vec3f(
                self.pos.x + right.x * sway,
                self.pos.y + cfg.eye_height + lift - self.view_offset,
                self.pos.z + right.z * sway,
            ),
            yaw: self.cam_yaw,
            roll: (self.turn_rate / cfg.turn_rate.max(0.01)).clamp(-1.0, 1.0) * cfg.bob.roll(),
        }
    }

    /// One fixed step against the level's triangles, wandering by local
    /// probes alone. Use [`Self::tick_in`] to tour a whole map.
    pub fn tick(&mut self, dt: f32, level: &LevelCollision) -> WalkerEvent {
        self.tick_in(dt, level, None)
    }

    /// One fixed step. With a [`NavGrid`] the heading comes from a planned
    /// route through the map's least-visited ground; without one it falls
    /// back to scoring the twelve directions from where it stands.
    pub fn tick_in(
        &mut self,
        dt: f32,
        level: &LevelCollision,
        nav: Option<&NavGrid>,
    ) -> WalkerEvent {
        let dt = dt.clamp(0.0, 0.1);
        let nav = nav.filter(|g| !g.is_empty());
        if let Some(grid) = nav {
            self.nav_steer(dt, grid);
        }
        self.since_pick += dt;
        self.probe_countdown = self.probe_countdown.saturating_sub(1);
        // Everything that raises the BODY within this tick is instant (a
        // step onto a tread, or the ground snap when a slope rises under
        // the feet). The eye must absorb all of it, so the reference is
        // taken here, before either can happen.
        let body_before = self.pos.y;
        // Eased turn: the rate ramps down as the heading is reached, so a
        // corner is a curve rather than a snap. Still rate-limited by the
        // engine's turn speed (~90°/s).
        let delta = wrap_pi(self.target_yaw - self.yaw);
        let rate = (delta / TURN_EASE_SECS).clamp(-self.cfg.turn_rate, self.cfg.turn_rate);
        let turn = (rate * dt).clamp(-delta.abs(), delta.abs());
        self.yaw = wrap_pi(self.yaw + turn);
        self.turn_rate = if dt > 0.0 { turn / dt } else { 0.0 };
        // The camera trails the body's heading a little.
        let cam_delta = wrap_pi(self.yaw - self.cam_yaw);
        self.cam_yaw = wrap_pi(self.cam_yaw + cam_delta * (dt / CAM_LAG_SECS).min(1.0));
        let facing_target = delta.abs() < 0.6;

        // Where the ground is right now. A walker with nothing at all
        // beneath it does not move: better a stopped tour than a camera
        // drifting through the void.
        let ground = self.ground_below(self.pos, level);
        let Some(ground) = ground else {
            self.stuck_ticks += 1;
            return WalkerEvent::Stranded;
        };

        // Falling: the originals do not snap you down a ledge, they drop
        // you (Doom GRAVITY, Quake sv_gravity). Horizontal motion carries
        // on mid-air, and the landing dips the view like `deltaviewheight`.
        if self.airborne || self.pos.y - ground.y > LAND_EPS {
            self.airborne = true;
            self.vel_y -= self.cfg.gravity * dt;
            self.pos.y += self.vel_y * dt;
            if self.pos.y <= ground.y {
                self.pos.y = ground.y;
                // Doom's landing sets `deltaviewheight` — a VELOCITY, not
                // a height: the view sinks over the next few tics and
                // springs back. Adding the dip to the offset directly is
                // what makes a landing snap.
                self.view_vel += (-self.vel_y * 0.35).min(1.6);
                self.vel_y = 0.0;
                self.airborne = false;
            }
        } else {
            self.pos.y = ground.y;
            self.vel_y = 0.0;
        }

        // Accelerate/decelerate over ~0.3 s instead of stepping at full
        // speed from a standstill.
        // player_nav: `ext_speed` is 1.0 unless an external planner asks
        // for the slight door/glance slow-down, so the built-in tours are
        // unchanged by the multiply.
        let want_speed = match (self.hold, facing_target) {
            (true, _) => 0.0,
            (false, true) => self.cfg.speed * self.ext_speed,
            (false, false) => self.cfg.speed * 0.25 * self.ext_speed,
        };
        // A body walking `v` and turning at `w` traces a circle of radius
        // `v / w`. If what it is steering at lies INSIDE that circle it can
        // never turn toward it — it orbits, at full turn rate, until
        // something else rescues it. So the turn sets the speed: never walk
        // faster than `turn_rate · distance to the aim point`. That is the
        // slow-into-the-corner a person does, and it is a HARD requirement,
        // not a comfort: without it the tour circles a waypoint forever
        // wherever the route bends tighter than the body can turn. Doom's
        // 1 m circle sits well inside its string-pulled waypoints and never
        // meets the cap; a map published at twice the metres per map unit
        // walks twice as fast and meets it at every corner.
        let want_speed = match self.aim_dist {
            Some(d) => want_speed.min(self.cfg.turn_rate * d),
            None => want_speed,
        };
        self.speed_now += (want_speed - self.speed_now) * (dt / SPEED_EASE_SECS).min(1.0);
        let step = self.speed_now * dt;
        let dir = yaw_forward(self.yaw);
        let mut moved = false;
        // The body walks THROUGH its turns, at the quarter speed decided
        // just above. It used to stand still for every heading change wider
        // than `facing_target` — and that dead stop was the whole jitter:
        // an eight-neighbour route turns 45° at a corner, 45° is wider than
        // the gate, so the walker stopped, spun, took a step, stopped again.
        // Worse, it was self-feeding: standing still is no progress, no
        // progress trips the planner's watchdog, a dropped leg makes the
        // next one CELL-BY-CELL, and a cell-by-cell leg turns at every
        // single cell. Stepping while turning is safe on its own — every
        // candidate step still goes through `try_move`, which refuses to
        // enter geometry — and a person walks the curve rather than pausing
        // to aim.
        for delta in [
            vec3f(dir.x * step, 0.0, dir.z * step),
            vec3f(dir.x * step, 0.0, 0.0),
            vec3f(0.0, 0.0, dir.z * step),
        ] {
            if delta.x == 0.0 && delta.z == 0.0 {
                continue;
            }
            if let Some(landed) = self.try_move(delta, level) {
                self.pos.x = landed.x;
                self.pos.z = landed.z;
                // Step UP (or level) instantly, as the originals do.
                // A lower floor is NOT snapped to: the walker is now
                // unsupported and next tick's gravity drops it.
                if !self.airborne && landed.y >= self.pos.y - LAND_EPS {
                    self.pos.y = landed.y;
                }
                moved = true;
                break;
            }
        }
        // A body step UP is instant and collision-correct; the EYE is not
        // moved with it — `view_offset` holds it back and the spring below
        // eases it up (Doom's `deltaviewheight`).
        let climbed = self.pos.y - body_before;
        if climbed > LAND_EPS {
            self.view_offset = (self.view_offset + climbed).min(self.cfg.eye_height * 0.8);
        }
        // Critically damped catch-up, ~0.25 s.
        let w = VIEW_CATCHUP_RATE;
        self.view_vel += (-w * w * self.view_offset - 2.0 * w * self.view_vel) * dt;
        self.view_offset += self.view_vel * dt;

        let target_gait = if moved && !self.airborne {
            (self.speed_now / self.cfg.speed.max(0.01)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.gait += (target_gait - self.gait) * (dt * 4.0).min(1.0);
        self.bob_phase = (self.bob_phase + self.cfg.bob.frequency() * self.gait * dt).fract();
        if moved {
            self.stuck_ticks = 0;
            if let Some(n) = self.nav.as_mut() {
                n.blocked_ticks = 0;
            } else {
                // Turn EARLY: a heading with less than two body widths of
                // floor left is already a dead end as far as the camera is
                // concerned.
                if self.probe_countdown == 0 {
                    self.probe_countdown = LOOKAHEAD_TICKS;
                    if level.free_run(self.pos, self.yaw, &self.cfg) < self.cfg.radius * 2.0 {
                        self.pick_heading(level);
                    }
                }
                if self.since_pick >= self.cfg.repick_secs {
                    self.pick_heading(level);
                }
            }
            return WalkerEvent::Stepped;
        }
        if facing_target && !self.airborne {
            self.stuck_ticks += 1;
            // With a route, a refused step is evidence about ONE edge — the
            // planner deals with it (see `nav_steer`). Re-scoring headings
            // here would fight the plan.
            match self.nav.as_mut() {
                Some(n) => n.blocked_ticks += 1,
                None => self.pick_heading(level),
            }
            if self.stuck_ticks > 240 {
                return WalkerEvent::Stranded;
            }
            return WalkerEvent::Blocked;
        }
        WalkerEvent::Stepped
    }

    /// Route the tour: keep track of where it is, plan a new leg when the
    /// old one is done (or has been proven wrong), and aim the heading at
    /// the next waypoint. Everything else — turning, stepping, gravity,
    /// wall-slide — is the ordinary tick.
    fn nav_steer(&mut self, dt: f32, grid: &NavGrid) {
        if self.nav.as_ref().is_none_or(|n| n.visits.len() != grid.len()) {
            self.nav = Some(NavState::new(grid.len()));
            // A route handed in before the first grid was seen.
            if let Some(route) = self.ext_route.take() {
                self.set_route(route);
            }
        }
        let external = self.external;
        let feet = self.pos;
        let cell_size = grid.cell_size();
        // Drawn every tick, not only when a plan happens: the borrow of
        // `self.nav` below covers the whole routine, and a tie-break that
        // advances on a fixed schedule is still deterministic.
        let jitter = self.rng.unit();
        let nav = self.nav.as_mut().expect("just created");
        nav.since_plan += dt;
        nav.since_decay += dt;
        nav.since_progress += dt;
        nav.since_cut += dt;
        // Visit counts fade, so a map toured for ten minutes is toured
        // again rather than settling into one loop through the leftovers.
        if nav.since_decay >= NAV_DECAY_SECS {
            nav.since_decay = 0.0;
            for v in nav.visits.iter_mut() {
                *v *= NAV_DECAY;
            }
        }
        // Where are we? A body between storeys (mid-fall) may be nowhere.
        let here = grid.cell_at(feet);
        let in_hazard = here
            .and_then(|c| grid.cell(c))
            .is_some_and(|c| c.kind == SurfaceKind::Hazard);
        self.escape_step = here.and_then(|c| grid.cell(c)).is_some_and(|c| c.escape);
        nav.hazard_secs = if in_hazard { nav.hazard_secs + dt } else { 0.0 };
        if let Some(c) = here {
            if nav.at != Some(c) {
                nav.at = Some(c);
                let v = &mut nav.visits[c as usize];
                if *v <= 0.0 {
                    nav.distinct += 1;
                }
                *v += 1.0;
                nav.seen[c as usize] = true;
                // Reaching a new cell IS progress, however slowly.
                nav.since_progress = 0.0;
                nav.best_gap = f32::MAX;
                nav.blocked_ticks = 0;
            }
        }
        // Consume waypoints already walked through. Matching the CELL alone
        // is not enough: a body that clips the corner of a waypoint without
        // landing in it leaves that waypoint in the queue, and the next aim
        // turns it round to go back for it — the ping-pong the plan exists
        // to prevent.
        while let Some(w) = nav.path.front().copied() {
            let reached = Some(w) == here
                || grid.cell(w).is_some_and(|c| {
                    let (dx, dz) = (c.pos.x - feet.x, c.pos.z - feet.z);
                    (dx * dx + dz * dz).sqrt() < cell_size * 0.5
                        && (c.pos.y - feet.y).abs() < self.cfg.step_up + 0.1
                });
            if !reached {
                break;
            }
            // A waypoint walked PAST counts as walked THROUGH: without this
            // the cell stays "unseen", is picked as a goal again, is passed
            // again, and the tour circles one spot forever.
            let w = w as usize;
            if !nav.seen[w] {
                nav.seen[w] = true;
                if nav.visits[w] <= 0.0 {
                    nav.distinct += 1;
                }
                nav.visits[w] += 1.0;
            }
            nav.path.pop_front();
            nav.best_gap = f32::MAX;
        }
        // Progress is measured against the WAYPOINT, not against the last
        // position: a body sliding back and forth along a wall covers metres
        // a second and gets no nearer to anything.
        match nav.path.front().and_then(|w| grid.cell(*w)) {
            Some(c) => {
                let (dx, dz) = (c.pos.x - feet.x, c.pos.z - feet.z);
                let gap = (dx * dx + dz * dz).sqrt();
                if gap + NAV_PROGRESS_EPS < nav.best_gap {
                    nav.best_gap = gap;
                    nav.since_progress = 0.0;
                }
            }
            None => nav.since_progress = 0.0,
        }
        let give_up = nav.since_progress >= NAV_STUCK_SECS || nav.blocked_ticks > 40;
        if give_up {
            // The edge we were trying to take does not exist in practice.
            if let (Some(a), Some(b)) = (nav.at, nav.path.front().copied()) {
                nav.blocked.insert(edge_key(a, b));
            }
            nav.path.clear();
            nav.goal = None;
            nav.since_progress = 0.0;
            nav.blocked_ticks = 0;
            // Slide the other way next time this happens.
            nav.slide = if nav.slide >= 0.0 { -0.6 } else { 0.6 };
        }
        let need_plan = !external && (nav.path.is_empty() || nav.since_plan >= NAV_REPLAN_SECS);
        if need_plan {
            nav.since_plan = 0.0;
            let start = here.or(nav.at);
            if let Some(start) = start {
                let (mut dist, mut parent) = (
                    std::mem::take(&mut nav.dist),
                    std::mem::take(&mut nav.parent),
                );
                let plan = grid.explore(
                    start,
                    &nav.visits,
                    &nav.blocked,
                    jitter,
                    &mut dist,
                    &mut parent,
                );
                nav.dist = dist;
                nav.parent = parent;
                match plan {
                    Some(plan) => {
                        nav.reachable = plan.reachable;
                        nav.frontier = plan.frontier;
                        // How much of what it can reach it has not seen —
                        // the honest "is there anything left here" test,
                        // and one the visit decay cannot blur.
                        nav.unseen = (0..nav.seen.len())
                            .filter(|i| {
                                nav.dist.get(*i).copied().unwrap_or(NAV_UNREACHED) != NAV_UNREACHED
                                    && !nav.seen[*i]
                                    && grid.cell(*i as u32).is_some_and(|c| {
                                        c.kind != SurfaceKind::Hazard
                                    })
                            })
                            .count();
                        nav.goal = Some(plan.goal);
                        nav.path = plan.path.into();
                        nav.replans += 1;
                        nav.goal_log.push(plan.goal);
                        if nav.goal_log.len() > NAV_GOAL_LOG {
                            nav.goal_log.remove(0);
                        }
                        nav.slide = 0.0;
                    }
                    None => {
                        // Nothing reachable at all: forget the invalidated
                        // edges rather than paralysing the tour forever.
                        nav.blocked.clear();
                        nav.goal = None;
                    }
                }
            }
        }
        // Stepping onto a teleport pad IS the edge: cut to the far side the
        // way Doom does, white flash and all.
        let pad = here.and_then(|c| grid.cell(c)).and_then(|c| c.teleport);
        // Nothing new within reach (or a route that has gone nowhere for ten
        // seconds — a classic map still bakes its doors into the walls):
        // cut to another wing rather than pacing a room it has memorised.
        // Paddling in the nukage for this long means the way out is not
        // one the graph can see: cut, rather than stand in it dying.
        // With an outside planner, WHERE to go next is not this module's
        // business — only a hazard it is drowning in still forces a cut.
        let exhausted = match external {
            true => nav.hazard_secs >= NAV_HAZARD_ESCAPE_SECS,
            false => {
                nav.unseen == 0
                    || nav.since_progress >= NAV_CUT_AFTER_SECS
                    || nav.hazard_secs >= NAV_HAZARD_ESCAPE_SECS
            }
        };
        let elsewhere = (exhausted && nav.since_cut >= NAV_CUT_COOLDOWN)
            .then(|| nav.at.and_then(|a| grid.next_region(a, &nav.visits)))
            .flatten();
        if let Some(dst) = pad.and_then(|t| grid.teleport_target(t)) {
            self.relocate(dst.0, dst.1);
            return;
        }
        if let Some(cell) = elsewhere.and_then(|c| grid.cell(c)) {
            // Forget the region being left behind, so coming back to it in
            // ten minutes is a tour again and not an instant bounce out.
            for i in 0..nav.seen.len() {
                if nav.dist.get(i).copied().unwrap_or(NAV_UNREACHED) != NAV_UNREACHED {
                    nav.seen[i] = false;
                }
            }
            let (pos, yaw) = (cell.pos, self.yaw);
            self.relocate(pos, yaw);
            return;
        }
        // A door on the near stretch of the route is opened before we get
        // there; the tour pauses briefly if it is still shut.
        let ext_door = self.ext_door;
        let nav = self.nav.as_mut().expect("still there");
        nav.want_door = match external {
            // The planner says which door; the built-in tour finds it itself.
            true => ext_door.filter(|d| !nav.open_doors.contains(d)),
            false => nav
                .path
                .iter()
                .take(NAV_DOOR_LOOKAHEAD)
                .filter_map(|w| grid.cell(*w).and_then(|c| c.door))
                .find(|d| !nav.open_doors.contains(d)),
        };
        let waiting = match nav.want_door {
            Some(_) => {
                nav.door_wait += dt;
                nav.door_wait < NAV_DOOR_WAIT
            }
            None => {
                nav.door_wait = 0.0;
                false
            }
        };
        // Aim at the next waypoint. Waypoints are cell centres, so the
        // heading is simply the bearing to the next one, plus the slide
        // offset that walks the body along a wall it snagged on.
        //
        // The aim NEVER jumps a waypoint the queue still holds: a route is
        // a proven chain of cells and the straight line to a later one may
        // go through a wall. What keeps a close waypoint reachable is the
        // speed cap below, not skipping it.
        let mut aim = None;
        for w in nav.path.iter() {
            let Some(c) = grid.cell(*w) else { continue };
            let (dx, dz) = (c.pos.x - feet.x, c.pos.z - feet.z);
            if (dx * dx + dz * dz).sqrt() < cell_size * 0.35 {
                continue; // standing on it already
            }
            aim = Some((dx, dz));
            break;
        }
        // How far the thing being steered at is — the tick's speed limit.
        // A body walking `v` while turning at `w` traces a circle of radius
        // `v/w`; steering at anything INSIDE that circle is a circle round
        // it, never a path to it, and the body orbits at full turn rate
        // until something else rescues it. That orbit is the tour a viewer
        // calls demented, and it is what a corner does to a walker whose
        // route bends tighter than its own turning circle.
        self.aim_dist = aim.map(|(dx, dz)| (dx * dx + dz * dz).sqrt());
        let slide = nav.slide;
        match aim {
            // yaw 0 looks down -Z (see `yaw_forward`).
            Some((dx, dz)) => self.target_yaw = wrap_pi(dx.atan2(-dz) + slide),
            // Route done: hold whatever heading the planner asked for.
            None => {
                if let Some(y) = self.ext_yaw {
                    self.target_yaw = wrap_pi(y);
                }
            }
        }
        // A closed door stops the tour where it stands rather than letting
        // it grind into the leaf. player_nav: an external planner may also
        // hold the body for its look-around pan (route empty, turning only).
        self.hold = waiting
            || (external && self.ext_hold && self.nav.as_ref().is_none_or(|n| n.path.is_empty()));
    }

    /// The floor under `p`: a step up is allowed, and the search reaches
    /// far enough down to find the landing spot of a fall.
    fn ground_below(&self, p: Vec3f, level: &LevelCollision) -> Option<FloorHit> {
        self.ground_below_up(p, level, 0.0)
    }

    /// The probe has to START above the tallest tread it may find, or a
    /// ledge exactly `step_up` up is missed by the ray that was supposed to
    /// find it — which is how a rim of exactly 24 map units became a wall.
    fn ground_below_up(&self, p: Vec3f, level: &LevelCollision, extra: f32) -> Option<FloorHit> {
        let up = self.cfg.step_up + STEP_EPS + extra;
        let from = vec3f(p.x, p.y + up, p.z);
        let reach = up + self.cfg.fall_limit.max(MAX_FALL_SEARCH);
        let hit = level.floor_below(from, reach)?;
        (hit.flatness >= 0.5).then_some(hit)
    }

    /// Why one candidate step was refused — the diagnostic behind
    /// "it will not climb that ledge".
    pub fn step_refusal(&self, level: &LevelCollision, delta: Vec3f) -> StepRefusal {
        let want = vec3f(self.pos.x + delta.x, self.pos.y, self.pos.z + delta.z);
        let standing_in = self
            .ground_below(self.pos, level)
            .map(|f| f.kind)
            .unwrap_or_default();
        let escape = match standing_in == SurfaceKind::Hazard || self.escape_step {
            true => hazard_escape_step(self.cfg.step_up),
            false => 0.0,
        };
        let step = (self.cfg.step_up + escape).min(self.cfg.height * 0.8);
        if level.path_blocked(self.pos, want, self.cfg.radius, step, self.cfg.height) {
            return StepRefusal::Walled;
        }
        let Some(floor) = self.ground_below_up(want, level, escape) else {
            return StepRefusal::NoFloor;
        };
        let rise = floor.y - self.pos.y;
        let reach = self.cfg.step_up + STEP_EPS + escape;
        if rise > reach {
            return StepRefusal::TooTall { rise, reach };
        }
        if !self.airborne && -rise > self.cfg.fall_limit {
            return StepRefusal::Pit { drop: -rise };
        }
        // The landing check gets the SAME step allowance as the walk probe
        // (escape stretch included): the two must never disagree about a
        // ledge, or the body is offered a step it refuses forever.
        let stand = vec3f(want.x, floor.y.max(self.pos.y), want.z);
        if !level.clearance_ok(stand, self.cfg.radius, step, self.cfg.height) {
            return StepRefusal::NoClearance { rise };
        }
        StepRefusal::Ok { rise }
    }

    /// Apply a horizontal step if the body fits, the walls stay a body
    /// radius away, and there is floor within the step/drop budget.
    ///
    /// A step UP happens instantly, the way the originals do it. A step
    /// DOWN does NOT snap: the walker simply becomes unsupported and
    /// gravity drops it, so ledges are fallen off rather than teleported
    /// down. A drop deeper than `fall_limit` is refused outright — that is
    /// a pit, and the tour turns away from it.
    fn try_move(&self, delta: Vec3f, level: &LevelCollision) -> Option<Vec3f> {
        let want = vec3f(self.pos.x + delta.x, self.pos.y, self.pos.z + delta.z);
        // Standing in the ooze — or anywhere the graph had to link with the
        // generous escape step — the rim is always climbable.
        let standing_in = self
            .ground_below(self.pos, level)
            .map(|f| f.kind)
            .unwrap_or_default();
        let escape = match standing_in == SurfaceKind::Hazard || self.escape_step {
            true => hazard_escape_step(self.cfg.step_up),
            false => 0.0,
        };
        // The knee ray must start above the tallest step this body may take,
        // or climbing out of a pool is refused by the riser of the very step
        // being asked about.
        let step = (self.cfg.step_up + escape).min(self.cfg.height * 0.8);
        if level.path_blocked(self.pos, want, self.cfg.radius, step, self.cfg.height) {
            return None;
        }
        let floor = self.ground_below_up(want, level, escape)?;
        let reach = self.cfg.step_up + STEP_EPS + escape;
        if floor.y - self.pos.y > reach {
            return None; // too tall to step onto
        }
        if !self.airborne && self.pos.y - floor.y > self.cfg.fall_limit {
            return None; // a pit, not a step
        }
        // Keep the camera's near plane clear of the walls: the body never
        // ends a step with geometry inside its radius. Same step allowance
        // as the walk probe above — the two must never disagree.
        let stand = vec3f(want.x, floor.y.max(self.pos.y), want.z);
        if !level.clearance_ok(stand, self.cfg.radius, step, self.cfg.height) {
            return None;
        }
        Some(vec3f(want.x, floor.y, want.z))
    }

    /// Turn toward the most open direction; ties and a small preference
    /// jitter come from the seed, so the wander repeats exactly.
    fn pick_heading(&mut self, level: &LevelCollision) {
        self.since_pick = 0.0;
        let mut best = (f32::MIN, self.yaw);
        for i in 0..PROBE_DIRS {
            let yaw = wrap_pi(self.yaw + std::f32::consts::TAU * i as f32 / PROBE_DIRS as f32);
            let open = level.free_run(self.pos, yaw, &self.cfg);
            let ahead = 1.0 - wrap_pi(yaw - self.yaw).abs() / std::f32::consts::PI;
            // Nukage, lava and slime are floors the tour would rather not
            // stand in: a hazardous run scores far below a clean one, so
            // the bridge wins whenever there is a bridge.
            let hazard = if level.has_kinds() {
                level.hazard_run(self.pos, yaw, &self.cfg)
            } else {
                0.0
            };
            // The forward bias has to outweigh ordinary differences in
            // open distance, or the walker ping-pongs along a corridor:
            // the way it just came is always the longest free run.
            let score = open - hazard * 4.0 + ahead * 2.5 + self.rng.unit() * 0.5;
            if score > best.0 {
                best = (score, yaw);
            }
        }
        self.target_yaw = best.1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append a quad (two triangles) to a triangle soup.
    fn quad(pos: &mut Vec<Vec3f>, idx: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, d: Vec3f) {
        let n = pos.len() as u32;
        pos.extend_from_slice(&[a, b, c, d]);
        idx.extend_from_slice(&[n, n + 1, n + 2, n, n + 2, n + 3]);
    }

    /// A horizontal slab top at `y` spanning the given x/z range.
    fn floor(pos: &mut Vec<Vec3f>, idx: &mut Vec<u32>, x0: f32, x1: f32, z0: f32, z1: f32, y: f32) {
        quad(
            pos,
            idx,
            vec3f(x0, y, z0),
            vec3f(x1, y, z0),
            vec3f(x1, y, z1),
            vec3f(x0, y, z1),
        );
    }

    /// A vertical wall spanning x0..x1 at `z`, from y0 to y1.
    fn wall_z(pos: &mut Vec<Vec3f>, idx: &mut Vec<u32>, x0: f32, x1: f32, z: f32, y0: f32, y1: f32) {
        quad(
            pos,
            idx,
            vec3f(x0, y0, z),
            vec3f(x1, y0, z),
            vec3f(x1, y1, z),
            vec3f(x0, y1, z),
        );
    }

    /// A vertical wall spanning z0..z1 at `x`, from y0 to y1.
    fn wall_x(pos: &mut Vec<Vec3f>, idx: &mut Vec<u32>, z0: f32, z1: f32, x: f32, y0: f32, y1: f32) {
        quad(
            pos,
            idx,
            vec3f(x, y0, z0),
            vec3f(x, y0, z1),
            vec3f(x, y1, z1),
            vec3f(x, y1, z0),
        );
    }

    /// A roofed corridor along -Z: floor at 0, ceiling at 2, side walls,
    /// and an end wall at z = -10. Open end at z = +1.
    fn corridor() -> LevelCollision {
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -1.0, 1.0, -10.0, 1.0, 0.0);
        floor(&mut p, &mut i, -1.0, 1.0, -10.0, 1.0, 2.0); // ceiling
        wall_x(&mut p, &mut i, -10.0, 1.0, -1.0, 0.0, 2.0);
        wall_x(&mut p, &mut i, -10.0, 1.0, 1.0, 0.0, 2.0);
        wall_z(&mut p, &mut i, -1.0, 1.0, -10.0, 0.0, 2.0);
        LevelCollision::from_positions(p, i)
    }

    fn cfg() -> WalkerConfig {
        WalkerConfig {
            radius: 0.2,
            height: 0.8,
            eye_height: 0.6,
            speed: 1.5,
            turn_rate: 6.0,
            step_up: 0.3,
            probe_ahead: 8.0,
            ..WalkerConfig::default()
        }
    }

    #[test]
    fn floor_probe_finds_the_real_triangle_under_the_feet() {
        let level = corridor();
        assert_eq!(level.triangles(), 10);
        let hit = level.floor_below(vec3f(0.0, 1.0, 0.0), 4.0).unwrap();
        assert!((hit.y - 0.0).abs() < 1e-4, "stands on the floor slab: {hit:?}");
        assert!(hit.flatness > 0.99, "a floor is flat: {hit:?}");
        // The ceiling is found looking up, which is what proves "indoors".
        assert_eq!(level.ceiling_above(vec3f(0.0, 0.1, 0.0), 8.0), Some(2.0));
        // Outside the corridor there is no floor at all.
        assert!(level.floor_below(vec3f(5.0, 1.0, 0.0), 4.0).is_none());
    }

    #[test]
    fn walls_block_and_open_space_does_not() {
        let level = corridor();
        let cfg = cfg();
        let from = vec3f(0.0, 0.0, 0.0);
        assert!(
            !level.path_blocked(from, vec3f(0.0, 0.0, -1.0), cfg.radius, cfg.step_up, cfg.height),
            "the corridor is open ahead"
        );
        assert!(
            level.path_blocked(from, vec3f(2.0, 0.0, 0.0), cfg.radius, cfg.step_up, cfg.height),
            "the side wall is solid"
        );
        // Free run stops at the end wall, not before and not through it.
        let run = level.free_run(from, 0.0, &cfg);
        assert!(run > 8.0 && run < 10.5, "ran {run} down a 10-unit corridor");
    }

    #[test]
    fn walker_stays_on_the_floor_and_never_enters_a_wall() {
        let level = corridor();
        let cfg = WalkerConfig { repick_secs: 1.0e6, ..cfg() };
        let start = level.interior_start(&cfg).expect("a corridor is walkable");
        assert!((start.y - 0.0).abs() < 1e-3, "starts on the floor: {start:?}");
        let mut w = LevelWalker::new(start, 0.0, cfg, 7);
        let mut turned_back = false;
        let mut min_z = f32::MAX;
        for _ in 0..1800 {
            let ev = w.tick(1.0 / 60.0, &level);
            assert_ne!(ev, WalkerEvent::Stranded, "a corridor is not a void");
            let feet = w.feet();
            // ON the floor — the mid-air bug this module exists to kill.
            assert!((feet.y - 0.0).abs() < 1e-3, "left the floor: {feet:?}");
            // The body radius is kept clear of every wall, which is also
            // what keeps the camera's near plane out of the geometry.
            let clear = cfg.radius - 1e-3;
            assert!(
                feet.x > -1.0 + clear && feet.x < 1.0 - clear,
                "camera too close to a side wall: {feet:?}"
            );
            assert!(feet.z > -10.0 + clear, "too close to the end wall: {feet:?}");
            min_z = min_z.min(feet.z);
            // It turns AWAY before it ever touches the dead end.
            turned_back |= w.yaw().abs() > 2.0;
        }
        assert!(min_z < -5.0, "should get down the corridor, min_z={min_z}");
        assert!(turned_back, "the dead end turns the walker around");
    }

    #[test]
    fn climbs_a_staircase_and_stands_on_each_tread() {
        let cfg = cfg();
        let (mut p, mut i) = (Vec::new(), Vec::new());
        // Four 0.25-high treads (each within step_up 0.3) going -Z.
        floor(&mut p, &mut i, -1.0, 1.0, -1.0, 2.0, 0.0);
        for n in 0..4 {
            let y = 0.25 * (n + 1) as f32;
            let z0 = -1.0 - n as f32;
            floor(&mut p, &mut i, -1.0, 1.0, z0 - 1.0, z0, y);
        }
        floor(&mut p, &mut i, -1.0, 1.0, -8.0, 2.0, 4.0); // ceiling
        wall_x(&mut p, &mut i, -8.0, 2.0, -1.0, 0.0, 4.0);
        wall_x(&mut p, &mut i, -8.0, 2.0, 1.0, 0.0, 4.0);
        let level = LevelCollision::from_positions(p, i);
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 1.5), 0.0, WalkerConfig { repick_secs: 1.0e6, ..cfg }, 3);
        let mut top = 0.0f32;
        for _ in 0..900 {
            w.tick(1.0 / 60.0, &level);
            let feet = w.feet();
            top = top.max(feet.y);
            // Always exactly on a tread, never between them.
            // Grounded, the feet are exactly on a tread; mid-fall (coming
            // back DOWN the stairs) they are legitimately between them.
            if !w.is_airborne() {
                let tread = (feet.y / 0.25).round() * 0.25;
                assert!((feet.y - tread).abs() < 1e-3, "not on a tread: {feet:?}");
            }
        }
        assert!(top >= 0.75, "climbed the stairs, reached {top}");
    }

    #[test]
    fn stairs_rise_smoothly_even_though_the_body_snaps() {
        // Five treads. The BODY steps instantly (Doom sets z), the EYE
        // must not: no visible jump bigger than a couple of centimetres.
        let cfg = WalkerConfig { repick_secs: 1.0e6, ..cfg() };
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -1.0, 1.0, -1.0, 2.0, 0.0);
        for n in 0..5 {
            let y = 0.25 * (n + 1) as f32;
            let z0 = -1.0 - n as f32;
            floor(&mut p, &mut i, -1.0, 1.0, z0 - 1.0, z0, y);
        }
        floor(&mut p, &mut i, -1.0, 1.0, -9.0, 2.0, 6.0);
        wall_x(&mut p, &mut i, -9.0, 2.0, -1.0, 0.0, 6.0);
        wall_x(&mut p, &mut i, -9.0, 2.0, 1.0, 0.0, 6.0);
        let level = LevelCollision::from_positions(p, i);
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 1.5), 0.0, cfg, 3);
        let mut prev_eye = w.camera().eye.y;
        let (mut body_jumps, mut worst_grounded, mut worst_any) = (0, 0.0f32, 0.0f32);
        let mut prev_body = w.feet().y;
        // The HIGHEST tread reached, not where the wander happened to stop:
        // the walker walks through its turns, so the tail of a free wander
        // is wherever it was going next, and the subject here is the eye.
        let mut top = w.feet().y;
        for _ in 0..900 {
            w.tick(1.0 / 60.0, &level);
            let body = w.feet().y;
            top = top.max(body);
            let eye = w.camera().eye.y;
            if body - prev_body > 0.1 {
                body_jumps += 1; // the body DOES snap up a tread
            }
            let jump = (eye - prev_eye).abs();
            worst_any = worst_any.max(jump);
            // While the feet are on the ground — climbing or walking — the
            // eye may only ease. Falling back DOWN the stairs it follows
            // the body at gravity's pace, which is not a snap.
            if !w.is_airborne() {
                worst_grounded = worst_grounded.max(jump);
            }
            prev_body = body;
            prev_eye = eye;
        }
        assert!(body_jumps >= 3, "the body climbed treads: {body_jumps}");
        // ~0.3 s to absorb a 0.25-unit tread is about 3 cm of eye travel
        // per tick at 60 Hz — smooth, and the same catch-up Doom gives
        // `viewheight`. The BODY's 0.25 jump never reaches the camera.
        assert!(
            worst_grounded < 0.03,
            "climbing must ease the view, not snap: worst {worst_grounded} per tick"
        );
        // Even falling, the eye never moves faster than free fall over one
        // tread (√(2·g·0.25) ≈ 3.1 u/s ≈ 0.052 per tick).
        assert!(worst_any < 0.06, "eye outran gravity: {worst_any} per tick");
        assert!(top >= 0.75, "climbed the stairs, reached {top}");
    }

    #[test]
    fn head_bob_stays_within_a_few_centimetres() {
        // "Sickeningly big" was 25 cm — Doom's raw MAXBOB/2 in map units.
        // At the importer's scale one world unit is about two metres, so
        // the peak has to stay near 0.015 units.
        let level = corridor();
        for style in [BobStyle::Doom, BobStyle::Duke, BobStyle::Quake] {
            let cfg = WalkerConfig { bob: style, repick_secs: 1.0e6, ..cfg() };
            let mut w = LevelWalker::new(vec3f(0.0, 0.0, 0.5), 0.0, cfg, 3);
            let mut peak = 0.0f32;
            for i in 0..600 {
                w.tick(1.0 / 60.0, &level);
                if i > 120 {
                    peak = peak.max((w.camera().eye.y - w.feet().y - cfg.eye_height).abs());
                }
            }
            assert!(peak < 0.025, "{style:?} bob peak {peak} world units (~5 cm cap)");
            assert!(peak > 0.002, "{style:?} bob should still be visible: {peak}");
        }
    }

    #[test]
    fn falls_down_a_drop_instead_of_teleporting() {
        // A 1.2-unit drop: too deep to step down, inside fall_limit, so
        // the originals would let you walk off and fall.
        let cfg = WalkerConfig { repick_secs: 1.0e6, fall_limit: 2.0, ..cfg() };
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -1.0, 1.0, -2.0, 2.0, 1.2); // upper ledge
        floor(&mut p, &mut i, -1.0, 1.0, -8.0, -2.0, 0.0); // lower floor
        floor(&mut p, &mut i, -1.0, 1.0, -8.0, 2.0, 5.0); // ceiling
        wall_x(&mut p, &mut i, -8.0, 2.0, -1.0, 0.0, 5.0);
        wall_x(&mut p, &mut i, -8.0, 2.0, 1.0, 0.0, 5.0);
        let level = LevelCollision::from_positions(p, i);
        let mut w = LevelWalker::new(vec3f(0.0, 1.2, 1.5), 0.0, cfg, 3);
        let mut heights = Vec::new();
        let mut airborne_ticks = 0;
        // Eye above feet on the tick the fall ENDS: that is the landing dip
        // the test is about. Reading it 600 ticks later measures head bob,
        // which rises above the nominal eye height by design.
        let mut landing_dip = None;
        let mut was_airborne = false;
        for _ in 0..600 {
            w.tick(1.0 / 60.0, &level);
            heights.push(w.feet().y);
            if w.is_airborne() {
                airborne_ticks += 1;
            } else if was_airborne && landing_dip.is_none() {
                landing_dip = Some(w.camera().eye.y - w.feet().y);
            }
            was_airborne = w.is_airborne();
        }
        let landed = *heights.last().unwrap();
        assert!((landed - 0.0).abs() < 1e-3, "lands on the lower floor: {landed}");
        assert!(airborne_ticks > 4, "it FELL, over several ticks: {airborne_ticks}");
        // Never teleports: no single tick moves more than gravity allows.
        for pair in heights.windows(2) {
            let drop = pair[0] - pair[1];
            assert!(drop < 0.35, "teleported down {drop} in one tick");
        }
        // Landing dips the view, then eases back (Doom's deltaviewheight).
        let dip = landing_dip.expect("the walker landed");
        assert!(dip <= cfg.eye_height + 1e-3, "the view never rises on landing: {dip}");
        assert!(dip > 0.0, "the eye is still above the feet: {dip}");
    }

    #[test]
    fn walks_the_bridge_instead_of_the_nukage() {
        // Two safe platforms joined by a hazard strip AND a clean bridge.
        // Every triangle's kind is declared, so the tour can choose.
        let cfg = WalkerConfig { repick_secs: 2.0, ..cfg() };
        let (mut p, mut i) = (Vec::new(), Vec::new());
        let mut kinds = Vec::new();
        let push = |p: &mut Vec<Vec3f>,
                        i: &mut Vec<u32>,
                        kinds: &mut Vec<SurfaceKind>,
                        x0: f32,
                        x1: f32,
                        z0: f32,
                        z1: f32,
                        y: f32,
                        kind: SurfaceKind| {
            floor(p, i, x0, x1, z0, z1, y);
            kinds.push(kind);
            kinds.push(kind);
        };
        // Start platform (z 0..2) and far platform (z -6..-4), joined by
        // TWO corridors: nukage on the left (x -3..-1), a clean bridge on
        // the right (x 1..3). The gap between them has no floor at all, so
        // the tour must commit to one or the other.
        push(&mut p, &mut i, &mut kinds, -3.0, 3.0, 0.0, 2.0, 0.0, SurfaceKind::Floor);
        push(&mut p, &mut i, &mut kinds, -3.0, 3.0, -6.0, -4.0, 0.0, SurfaceKind::Floor);
        push(&mut p, &mut i, &mut kinds, -3.0, -1.0, -4.0, 0.0, 0.0, SurfaceKind::Hazard);
        push(&mut p, &mut i, &mut kinds, 1.0, 3.0, -4.0, 0.0, 0.0, SurfaceKind::Floor);
        // Ceiling (nobody stands on it, but it makes the room interior).
        push(&mut p, &mut i, &mut kinds, -3.0, 3.0, -6.0, 2.0, 4.0, SurfaceKind::Floor);
        let level = LevelCollision::from_positions(p, i).with_kinds(kinds);
        assert!(level.has_kinds(), "the fixture declares its surfaces");
        // The probe agrees which corridor is which.
        assert!(level.hazard_run(vec3f(-2.0, 0.0, -0.5), 0.0, &cfg) > 0.5, "left is ooze");
        assert_eq!(level.hazard_run(vec3f(2.0, 0.0, -0.5), 0.0, &cfg), 0.0, "right is clean");
        // Started in the middle of the start platform, facing the gap.
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 1.0), 0.0, cfg, 4);
        let (mut ooze_ticks, mut bridge_ticks, mut reached_far) = (0, 0, false);
        for _ in 0..5400 {
            w.tick(1.0 / 60.0, &level);
            let feet = w.feet();
            if feet.z < -0.05 && feet.z > -4.0 {
                if feet.x < 0.0 {
                    ooze_ticks += 1;
                } else {
                    bridge_ticks += 1;
                }
            }
            reached_far |= feet.z < -4.0;
        }
        assert!(reached_far, "the tour should reach the far platform");
        assert!(
            bridge_ticks > ooze_ticks * 4,
            "took the bridge: {bridge_ticks} bridge vs {ooze_ticks} ooze ticks"
        );
    }

    #[test]
    fn refuses_to_walk_into_a_pit() {
        let cfg = WalkerConfig { fall_limit: 0.5, repick_secs: 1.0e6, ..cfg() };
        let (mut p, mut i) = (Vec::new(), Vec::new());
        // A ledge that ends at z = -3 with nothing beyond: a void.
        floor(&mut p, &mut i, -1.0, 1.0, -3.0, 2.0, 0.0);
        floor(&mut p, &mut i, -1.0, 1.0, -3.0, 2.0, 2.0);
        let level = LevelCollision::from_positions(p, i);
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 1.0), 0.0, cfg, 11);
        for _ in 0..900 {
            let ev = w.tick(1.0 / 60.0, &level);
            if ev == WalkerEvent::Stranded {
                panic!("the walker fell out of the level at {:?}", w.feet());
            }
            assert!(w.feet().z > -3.0 - 1e-3, "stepped into the void: {:?}", w.feet());
            assert!((w.feet().y - 0.0).abs() < 1e-3);
        }
    }

    #[test]
    fn interior_start_is_under_the_roof_not_on_it() {
        let cfg = cfg();
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -6.0, 6.0, -6.0, 6.0, 0.0);
        floor(&mut p, &mut i, -6.0, 6.0, -6.0, 6.0, 3.0); // roof
        wall_x(&mut p, &mut i, -6.0, 6.0, -6.0, 0.0, 3.0);
        wall_x(&mut p, &mut i, -6.0, 6.0, 6.0, 0.0, 3.0);
        wall_z(&mut p, &mut i, -6.0, 6.0, -6.0, 0.0, 3.0);
        wall_z(&mut p, &mut i, -6.0, 6.0, 6.0, 0.0, 3.0);
        let level = LevelCollision::from_positions(p, i);
        let start = level.interior_start(&cfg).expect("a room is walkable");
        assert!((start.y - 0.0).abs() < 1e-3, "on the floor, not the roof: {start:?}");
        let mut w = LevelWalker::new(start, 0.0, cfg, 5);
        for _ in 0..1200 {
            w.tick(1.0 / 60.0, &level);
            assert!(w.feet().y < 2.9, "climbed onto the roof: {:?}", w.feet());
        }
    }

    #[test]
    fn head_bob_is_speed_proportional_and_per_engine() {
        let level = corridor();
        let sample = |style: BobStyle| {
            let cfg = WalkerConfig { bob: style, repick_secs: 1.0e6, ..cfg() };
            let mut w = LevelWalker::new(vec3f(0.0, 0.0, 0.5), 0.0, cfg, 3);
            assert_eq!(w.camera().eye.y, 0.0 + cfg.eye_height, "standing still: no bob");
            let (mut lo, mut hi, mut crossings, mut prev) = (f32::MAX, f32::MIN, 0, 0.0f32);
            for i in 0..600 {
                w.tick(1.0 / 60.0, &level);
                let h = w.camera().eye.y - w.feet().y - cfg.eye_height;
                if i > 120 {
                    lo = lo.min(h);
                    hi = hi.max(h);
                    if prev <= 0.0 && h > 0.0 {
                        crossings += 1;
                    }
                    prev = h;
                }
            }
            (lo, hi, crossings)
        };
        let (lo, hi, cycles) = sample(BobStyle::Doom);
        // Magnitude is pinned by `head_bob_stays_within_a_few_centimetres`;
        // here the shape matters: a real dip below the eye line, and the
        // engine's cadence.
        assert!(hi > 0.002, "doom bob rises: {hi}");
        assert!(lo < -0.002, "doom bob dips below the eye line: {lo}");
        assert!((cycles as f32 - 8.0 * 1.75).abs() < 2.5, "doom ~1.75 Hz, {cycles} in 8 s");
        let (_, _, quake_cycles) = sample(BobStyle::Quake);
        assert!(
            (quake_cycles as f32 - 8.0 / 0.6).abs() < 3.0,
            "quake cl_bobcycle 0.6 s, {quake_cycles} in 8 s"
        );
        let (_, duke_hi, _) = sample(BobStyle::Duke);
        assert!(duke_hi > hi, "Build's stride is deeper than Doom's");
        assert_eq!(sample(BobStyle::None).1, 0.0, "bob off means a locked camera");
    }

    #[test]
    fn bob_style_follows_the_map_source() {
        assert_eq!(BobStyle::from_source("doom/doom/worlds/doom1/e1m1"), BobStyle::Doom);
        assert_eq!(BobStyle::from_source("duke/duke3d/worlds/duke3d/e1l1"), BobStyle::Duke);
        assert_eq!(BobStyle::from_source("quake/id1/worlds/id1/e1m1"), BobStyle::Quake);
        assert_eq!(BobStyle::from_source("quake3/baseq3/worlds/q3dm1"), BobStyle::Quake);
    }

    #[test]
    fn same_seed_walks_the_same_tour() {
        let level = corridor();
        let cfg = cfg();
        let path = |seed: u64| {
            let mut w = LevelWalker::new(vec3f(0.0, 0.0, 0.0), 0.0, cfg, seed);
            (0..900)
                .map(|_| {
                    w.tick(1.0 / 60.0, &level);
                    (w.feet().x, w.feet().z, w.yaw())
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(path(42), path(42), "a seeded tour repeats exactly");
    }

    /// Two storeys joined by a staircase: a lower room (y 0, z -2..6), six
    /// 0.25 treads climbing to y 1.5 over z -8..-2, and an upper room
    /// (y 1.5, z -14..-8). Roofed throughout, walled down both sides.
    fn two_floors() -> LevelCollision {
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -2.0, 2.0, -2.0, 6.0, 0.0);
        floor(&mut p, &mut i, -2.0, 2.0, -2.0, 6.0, 3.0);
        for n in 0..6 {
            let y = 0.25 * (n + 1) as f32;
            let z1 = -2.0 - n as f32;
            floor(&mut p, &mut i, -2.0, 2.0, z1 - 1.0, z1, y);
        }
        floor(&mut p, &mut i, -2.0, 2.0, -8.0, -2.0, 4.0);
        floor(&mut p, &mut i, -2.0, 2.0, -14.0, -8.0, 1.5);
        floor(&mut p, &mut i, -2.0, 2.0, -14.0, -8.0, 4.5);
        wall_x(&mut p, &mut i, -14.0, 6.0, -2.0, 0.0, 5.0);
        wall_x(&mut p, &mut i, -14.0, 6.0, 2.0, 0.0, 5.0);
        wall_z(&mut p, &mut i, -2.0, 2.0, 6.0, 0.0, 5.0);
        wall_z(&mut p, &mut i, -2.0, 2.0, -14.0, 0.0, 5.0);
        LevelCollision::from_positions(p, i)
    }

    /// Wrap a glTF JSON document as a GLB (JSON chunk only — the classifier
    /// never touches the binary chunk).
    fn glb(json: &str) -> Vec<u8> {
        let mut chunk = json.as_bytes().to_vec();
        while chunk.len() % 4 != 0 {
            chunk.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + chunk.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&chunk);
        out
    }

    #[test]
    fn hazard_nodes_classify_their_triangles() {
        // The importer's contract: a `hazard_N` node with extras. Three
        // primitives — clean floor, nukage, water — 2 triangles each.
        let doc = r#"{
            "nodes":[
              {"name":"floor_0","mesh":0},
              {"name":"hazard_1","mesh":1,"extras":{"kind":"hazard","damage":5,"flat":"NUKAGE1","liquid":true}},
              {"name":"hazard_2","mesh":2,"extras":{"kind":"hazard","damage":0,"flat":"FWATER1","liquid":true}}
            ],
            "meshes":[
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]},
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]},
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]}
            ],
            "accessors":[{"count":4},{"count":6}]
        }"#;
        let kinds = surface_kinds_from_glb(&glb(doc), 6).expect("classified");
        assert_eq!(
            kinds,
            vec![
                SurfaceKind::Floor,
                SurfaceKind::Floor,
                SurfaceKind::Hazard,
                SurfaceKind::Hazard,
                SurfaceKind::Liquid,
                SurfaceKind::Liquid,
            ]
        );
    }

    #[test]
    fn old_maps_fall_back_to_the_flat_name_on_the_material() {
        // Every classic map published before the `hazard_N` lane: the only
        // evidence is the source engine's flat name on the material.
        let doc = r#"{
            "nodes":[{"name":"e1m1","mesh":0}],
            "meshes":[{"primitives":[
              {"attributes":{"POSITION":0},"indices":1,"material":0},
              {"attributes":{"POSITION":0},"indices":1,"material":1},
              {"attributes":{"POSITION":0},"indices":1,"material":2}
            ]}],
            "materials":[{"name":"doom.FLOOR4_8"},{"name":"doom.NUKAGE1"},{"name":"doom.FWATER1"}],
            "accessors":[{"count":4},{"count":6}]
        }"#;
        let kinds = surface_kinds_from_glb(&glb(doc), 6).expect("classified");
        assert_eq!(kinds[0], SurfaceKind::Floor);
        assert_eq!(kinds[2], SurfaceKind::Hazard, "NUKAGE damages");
        assert_eq!(kinds[4], SurfaceKind::Liquid, "FWATER is only wet");
    }

    /// The open-air regression: every classic map with an outdoor area
    /// publishes a `sky` node, the loader takes that geometry OUT of the
    /// static stream, and a walk that still counted it could never
    /// reproduce the model's triangle count — so the whole classification
    /// was discarded and the map's nukage stopped hurting. (Doom's E1M1:
    /// 17264 level + 592 hazard triangles loaded, 502 more under `sky`.)
    #[test]
    fn sky_geometry_is_left_out_of_the_static_walk() {
        let doc = r#"{
            "nodes":[
              {"name":"floor_0","mesh":0},
              {"name":"hazard_1","mesh":1,"extras":{"kind":"hazard","damage":5,"flat":"NUKAGE1"}},
              {"name":"sky","mesh":2,"extras":{"kind":"sky","projection":"cylinder"}}
            ],
            "meshes":[
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]},
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]},
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]}
            ],
            "accessors":[{"count":4},{"count":6}]
        }"#;
        // The loader packs 4 triangles (floor + hazard); the sky's 2 are its
        // own. Classification must agree, and the hazard must survive.
        let kinds = surface_kinds_from_glb(&glb(doc), 4).expect("classified without the sky");
        assert_eq!(
            kinds,
            vec![
                SurfaceKind::Floor,
                SurfaceKind::Floor,
                SurfaceKind::Hazard,
                SurfaceKind::Hazard,
            ]
        );
    }

    /// A node NAMED sky with no projection the engine knows is ordinary
    /// geometry to the loader, so it must stay ordinary here too.
    #[test]
    fn a_sky_node_without_a_projection_is_ordinary_geometry() {
        let doc = r#"{
            "nodes":[
              {"name":"hazard_1","mesh":0,"extras":{"kind":"hazard","damage":5}},
              {"name":"sky","mesh":1,"extras":{"kind":"sky"}}
            ],
            "meshes":[
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]},
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]}
            ],
            "accessors":[{"count":4},{"count":6}]
        }"#;
        let kinds = surface_kinds_from_glb(&glb(doc), 4).expect("classified with the sky in");
        assert_eq!(kinds.len(), 4);
        assert_eq!(kinds[0], SurfaceKind::Hazard);
    }

    #[test]
    fn a_triangle_count_that_disagrees_with_the_loader_is_refused() {
        // The contract check: if this walk does not reproduce the loader's
        // triangle count, the mapping is a guess and must not be used.
        let doc = r#"{
            "nodes":[{"name":"hazard_1","mesh":0,"extras":{"kind":"hazard","damage":5}}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
            "accessors":[{"count":4},{"count":6}]
        }"#;
        assert!(surface_kinds_from_glb(&glb(doc), 2).is_some(), "matching count is used");
        assert_eq!(surface_kinds_from_glb(&glb(doc), 99), None, "a mismatch is refused");
        assert_eq!(surface_kinds_from_glb(b"not a glb", 2), None);
    }

    #[test]
    fn door_geometry_is_left_out_of_the_static_walk() {
        // A door node (`extras.states`) is a moving part: its triangles are
        // NOT in the static stream, so the classifier must skip them too or
        // every triangle after it is labelled with its neighbour's kind.
        let doc = r#"{
            "nodes":[
              {"name":"door_1","mesh":0,"extras":{"states":["closed","open"],"default":"closed"}},
              {"name":"hazard_1","mesh":1,"extras":{"kind":"hazard","damage":5}}
            ],
            "meshes":[
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]},
              {"primitives":[{"attributes":{"POSITION":0},"indices":1}]}
            ],
            "animations":[{"name":"door_1","channels":[]}],
            "accessors":[{"count":4},{"count":6}]
        }"#;
        let kinds = surface_kinds_from_glb(&glb(doc), 2).expect("classified without the door");
        assert_eq!(kinds, vec![SurfaceKind::Hazard, SurfaceKind::Hazard]);
    }

    #[test]
    fn nav_grid_connects_two_floors_through_the_stairs() {
        let cfg = cfg();
        let level = two_floors();
        let nav = NavGrid::build(&level, &cfg);
        assert!(nav.len() > 100, "a two-room level has cells: {}", nav.len());
        assert!(nav.edge_count() > nav.len(), "cells are linked: {}", nav.edge_count());
        // Both storeys are represented.
        let lower = (0..nav.len() as u32)
            .find(|i| {
                let c = nav.cell(*i).unwrap();
                c.pos.y.abs() < 1e-3 && c.pos.z > 4.0
            })
            .expect("a cell at the back of the lower room");
        let upper: Vec<u32> = (0..nav.len() as u32)
            .filter(|i| {
                let c = nav.cell(*i).unwrap();
                (c.pos.y - 1.5).abs() < 1e-3 && c.pos.z < -9.0
            })
            .collect();
        assert!(!upper.is_empty(), "the upper room has cells");
        // The graph itself crosses the stairs.
        let (mut dist, mut parent) = (Vec::new(), Vec::new());
        let reached = nav.flood(lower, &std::collections::HashSet::new(), &mut dist, &mut parent);
        assert!(reached > nav.len() / 2, "most of the level is reachable: {reached}/{}", nav.len());
        for u in &upper {
            assert_ne!(
                dist[*u as usize], NAV_UNREACHED,
                "upper-room cell {u} is cut off from the lower room"
            );
        }
        // And the route it hands out actually walks up them.
        let path = nav.path_to(upper[0], &parent);
        assert!(!path.is_empty(), "a route exists");
        let climbed = path
            .windows(2)
            .all(|w| {
                let (a, b) = (nav.cell(w[0]).unwrap(), nav.cell(w[1]).unwrap());
                b.pos.y - a.pos.y <= cfg.step_up + 1e-3
            });
        assert!(climbed, "no waypoint pair asks for a jump");
        let top = path.iter().map(|c| nav.cell(*c).unwrap().pos.y).fold(0.0f32, f32::max);
        assert!(top >= 1.5 - 1e-3, "the route reaches the upper storey: {top}");
    }

    #[test]
    fn the_tour_climbs_to_the_other_floor_and_keeps_finding_new_ground() {
        // The complaint this exists for: the local heading picker paced up
        // and down one corridor. With a plan it must cross the whole map.
        let cfg = cfg();
        let level = two_floors();
        let nav = NavGrid::build(&level, &cfg);
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 5.0), 0.0, cfg, 9);
        let (mut top, mut bottom) = (f32::MIN, f32::MAX);
        let mut distinct_at_30s = 0;
        for t in 0..7200 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            top = top.max(w.feet().y);
            bottom = bottom.min(w.feet().y);
            if t == 1800 {
                distinct_at_30s = w.nav_stats().distinct;
            }
        }
        let stats = w.nav_stats();
        assert!(top >= 1.4, "the tour climbed to the upper room: reached y={top}");
        assert!(bottom <= 0.1, "and it started on the lower one: {bottom}");
        assert!(stats.replans >= 2, "it re-planned as it went: {stats:?}");
        // Coverage keeps GROWING — the ping-pong signature is a distinct
        // count that flattens out after a few seconds.
        assert!(
            stats.distinct > distinct_at_30s,
            "coverage stalled: {distinct_at_30s} cells at 30 s, {} at 120 s",
            stats.distinct
        );
        assert!(
            stats.distinct * 3 > nav.len(),
            "toured only {} of {} cells",
            stats.distinct,
            nav.len()
        );
    }

    #[test]
    fn a_planned_tour_still_prefers_the_bridge_to_the_nukage() {
        let cfg = WalkerConfig { repick_secs: 2.0, ..cfg() };
        let (mut p, mut i) = (Vec::new(), Vec::new());
        let mut kinds = Vec::new();
        let push = |p: &mut Vec<Vec3f>,
                        i: &mut Vec<u32>,
                        kinds: &mut Vec<SurfaceKind>,
                        x0: f32,
                        x1: f32,
                        z0: f32,
                        z1: f32,
                        y: f32,
                        kind: SurfaceKind| {
            floor(p, i, x0, x1, z0, z1, y);
            kinds.push(kind);
            kinds.push(kind);
        };
        push(&mut p, &mut i, &mut kinds, -3.0, 3.0, 0.0, 2.0, 0.0, SurfaceKind::Floor);
        push(&mut p, &mut i, &mut kinds, -3.0, 3.0, -6.0, -4.0, 0.0, SurfaceKind::Floor);
        push(&mut p, &mut i, &mut kinds, -3.0, -1.0, -4.0, 0.0, 0.0, SurfaceKind::Hazard);
        push(&mut p, &mut i, &mut kinds, 1.0, 3.0, -4.0, 0.0, 0.0, SurfaceKind::Floor);
        push(&mut p, &mut i, &mut kinds, -3.0, 3.0, -6.0, 2.0, 4.0, SurfaceKind::Floor);
        let level = LevelCollision::from_positions(p, i).with_kinds(kinds);
        let nav = NavGrid::build(&level, &cfg);
        // The grid knows which corridor is which.
        let ooze = (0..nav.len() as u32)
            .filter(|c| nav.cell(*c).unwrap().kind == SurfaceKind::Hazard)
            .count();
        assert!(ooze > 0, "the hazard strip is in the grid, not deleted");
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 1.0), 0.0, cfg, 4);
        let (mut ooze_ticks, mut bridge_ticks, mut reached_far) = (0, 0, false);
        for _ in 0..5400 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            let feet = w.feet();
            if feet.z < -0.05 && feet.z > -4.0 {
                if feet.x < 0.0 {
                    ooze_ticks += 1;
                } else {
                    bridge_ticks += 1;
                }
            }
            reached_far |= feet.z < -4.0;
        }
        assert!(reached_far, "the planned tour reaches the far platform");
        assert!(
            bridge_ticks > ooze_ticks * 4,
            "took the bridge: {bridge_ticks} bridge vs {ooze_ticks} ooze ticks"
        );
    }

    #[test]
    fn a_seeded_planned_tour_repeats_exactly() {
        let level = two_floors();
        let cfg = cfg();
        let nav = NavGrid::build(&level, &cfg);
        let path = |seed: u64| {
            let mut w = LevelWalker::new(vec3f(0.0, 0.0, 5.0), 0.0, cfg, seed);
            (0..1800)
                .map(|_| {
                    w.tick_in(1.0 / 60.0, &level, Some(&nav));
                    (w.feet().x, w.feet().y, w.feet().z, w.yaw())
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(path(42), path(42), "a seeded planned tour repeats exactly");
        assert_ne!(path(42), path(43), "and two seeds do not walk in lockstep");
    }

    #[test]
    fn a_stuck_leg_is_invalidated_and_replanned() {
        // A wall the probe cannot see (the fixture lies): the walker must
        // notice it makes no ground, drop that edge and route elsewhere.
        let cfg = cfg();
        let level = two_floors();
        let nav = NavGrid::build(&level, &cfg);
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 5.0), 0.0, cfg, 5);
        // Freeze the body: every tick reports no progress, which is exactly
        // what a lying edge looks like from inside the walker.
        for _ in 0..600 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            w.pos = vec3f(0.0, 0.0, 5.0);
        }
        let stats = w.nav_stats();
        assert!(stats.invalidated > 0, "a leg that never moves is dropped: {stats:?}");
        assert!(stats.replans > 1, "and the tour re-plans: {stats:?}");
    }

    #[test]
    fn doors_gate_their_cells_and_are_asked_for_by_the_tour() {
        let cfg = cfg();
        let level = two_floors();
        let mut nav = NavGrid::build(&level, &cfg);
        // A door across the whole lower room at z ≈ 0.
        nav.mark_doors(&[(vec3f(-2.0, 0.0, -0.4), vec3f(2.0, 2.0, 0.4))]);
        let gated = (0..nav.len() as u32)
            .filter(|c| nav.cell(*c).unwrap().door == Some(0))
            .count();
        assert!(gated > 0, "the door's footprint marks cells");
        for c in 0..nav.len() as u32 {
            let cell = nav.cell(c).unwrap();
            if cell.door.is_some() {
                assert!(cell.pos.z.abs() < 1.0, "only the doorway is gated: {cell:?}");
            }
        }
        // Walking toward the stairs, the tour asks for the door ahead.
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 5.0), 0.0, cfg, 3);
        let mut asked = None;
        for _ in 0..3600 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            if let Some(d) = w.wanted_door() {
                asked = Some(d);
                break;
            }
        }
        assert_eq!(asked, Some(0), "the tour asked the host to open door 0");
        // Once the host says it is open, the request clears and it walks on.
        w.set_door_open(0, true);
        w.tick_in(1.0 / 60.0, &level, Some(&nav));
        assert_eq!(w.wanted_door(), None, "an open door is no longer requested");
        let before = w.feet().z;
        for _ in 0..600 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
        }
        assert!(w.feet().z < before, "and it carries on through: {before} → {:?}", w.feet());
    }

    /// Two sealed rooms with no way between them — exactly what a classic
    /// map looks like while its doors are still baked into the walls.
    fn two_sealed_rooms() -> LevelCollision {
        let (mut p, mut i) = (Vec::new(), Vec::new());
        for (z0, z1) in [(0.0f32, 6.0f32), (-20.0, -14.0)] {
            floor(&mut p, &mut i, -3.0, 3.0, z0, z1, 0.0);
            floor(&mut p, &mut i, -3.0, 3.0, z0, z1, 2.5);
            wall_x(&mut p, &mut i, z0, z1, -3.0, 0.0, 2.5);
            wall_x(&mut p, &mut i, z0, z1, 3.0, 0.0, 2.5);
            wall_z(&mut p, &mut i, -3.0, 3.0, z0, 0.0, 2.5);
            wall_z(&mut p, &mut i, -3.0, 3.0, z1, 0.0, 2.5);
        }
        LevelCollision::from_positions(p, i)
    }

    /// Diagnostic against a REAL published map. Skipped unless `LEVEL_GLB`
    /// points at one, so the suite stays hermetic; run as
    /// `LEVEL_GLB=<path> cargo test -p makepad-render --lib -- real_level --nocapture`
    /// when a map on screen behaves in a way the fixtures do not reproduce.
    #[test]
    fn real_level_is_a_place_a_walker_can_get_out_of() {
        let Ok(path) = std::env::var("LEVEL_GLB") else { return };
        let bytes = std::fs::read(&path).expect("LEVEL_GLB readable");
        let model = crate::StaticModel::parse_glb(&bytes).expect("a static GLB");
        let cfg = WalkerConfig::default();
        let level = LevelCollision::from_packed(
            &model.vertices,
            crate::model::MODEL_VERTEX_FLOATS,
            &model.indices,
            UpAxis::Y,
        )
        .expect("collision");
        let level = match surface_kinds_from_glb(&bytes, model.triangle_count()) {
            Some(kinds) => {
                let h = kinds.iter().filter(|k| **k == SurfaceKind::Hazard).count();
                println!("kinds: {h} hazard of {} triangles", kinds.len());
                level.with_kinds(kinds)
            }
            None => {
                println!("kinds: none (no hazard_N nodes, no flat names)");
                level
            }
        };
        let nav = NavGrid::build(&level, &cfg);
        let r = nav.refusals();
        println!(
            "nav: {} cells, {} edges, refused {} too tall (smallest rise {:.4}) / {} deep / {} walled, {} escape links",
            nav.len(),
            nav.edge_count(),
            r.too_tall,
            r.smallest_refused_rise,
            r.too_deep,
            r.walled,
            r.escapes
        );
        println!("components: {:?}", &nav.component_sizes()[..nav.component_sizes().len().min(8)]);
        let core = nav.best_start().expect("a start");
        let out = nav.can_reach(core);
        let trapped = out.iter().filter(|o| !**o).count();
        println!("trapped cells (cannot walk back to the core): {trapped}/{}", nav.len());
        // What actually separates a trapped cell from freedom: the rise to
        // its nearest free neighbour, and whether the body probe refuses it.
        let mut rises: Vec<(f32, bool)> = Vec::new();
        for a in 0..nav.len() {
            if out[a] {
                continue;
            }
            let (ix, iz) = nav.column_xz(nav.cells[a].column);
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let Some(col) = nav.column_index(ix as i32 + dx, iz as i32 + dz) else {
                        continue;
                    };
                    for b in nav.column_cells(col) {
                        if !out[b] {
                            continue;
                        }
                        let dy = nav.cells[b].pos.y - nav.cells[a].pos.y;
                        if dy <= 0.0 {
                            continue;
                        }
                        let blocked = level.path_blocked(
                            nav.cells[a].pos,
                            nav.cells[b].pos,
                            cfg.radius,
                            (cfg.step_up + hazard_escape_step(cfg.step_up)).min(cfg.height * 0.8),
                            cfg.height,
                        );
                        rises.push((dy, blocked));
                    }
                }
            }
        }
        rises.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        println!("trapped→free boundary pairs: {}", rises.len());
        for (dy, blocked) in rises.iter().take(12) {
            println!("  rise {dy:.4} ({} map units) blocked={blocked}", (dy * 64.0).round());
        }
        let climbable = rises
            .iter()
            .filter(|(dy, blocked)| *dy <= cfg.step_up + STEP_EPS + hazard_escape_step(cfg.step_up) && !*blocked)
            .count();
        println!("  of those, {climbable} are within the escape step and unblocked");
        // Drop the body in the LOWEST floor of the map — the bottom of the
        // deepest pit — and see whether it is a place you can leave.
        let low = (0..nav.len() as u32)
            .min_by(|a, b| {
                nav.cell(*a)
                    .unwrap()
                    .pos
                    .y
                    .partial_cmp(&nav.cell(*b).unwrap().pos.y)
                    .unwrap()
            })
            .expect("cells");
        let start = nav.cell(low).unwrap().pos;
        println!("deepest cell {low} at {start:?}, escape flag {}", nav.cell(low).unwrap().escape);
        let mut w = LevelWalker::new(start, 0.0, cfg, 3);
        let (mut top, mut left) = (start.y, false);
        for t in 0..3600 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            top = top.max(w.feet().y);
            if !left && w.feet().y > start.y + 0.3 {
                left = true;
                println!("climbed out after {:.1}s at {:?}", t as f32 / 60.0, w.feet());
            }
        }
        println!(
            "after 60 s: {:?}, highest {top:.3} (started {:.3}), stats {:?}",
            w.feet(),
            start.y,
            w.nav_stats()
        );
        // EVERY low-lying cell, not just the single deepest: a pit that is a
        // prison anywhere on the map is the bug the user is looking at.
        let mut low_cells: Vec<u32> = (0..nav.len() as u32).collect();
        low_cells.sort_by(|a, b| {
            nav.cell(*a)
                .unwrap()
                .pos
                .y
                .partial_cmp(&nav.cell(*b).unwrap().pos.y)
                .unwrap()
        });
        let sample: Vec<u32> = low_cells.iter().step_by(7).take(40).copied().collect();
        let mut prisons = Vec::new();
        for c in &sample {
            let from = nav.cell(*c).unwrap().pos;
            let mut w = LevelWalker::new(from, 0.0, cfg, 5);
            let mut out_ok = false;
            for _ in 0..1800 {
                w.tick_in(1.0 / 60.0, &level, Some(&nav));
                if w.feet().y > from.y + 0.3 || w.cut_count() > 0 {
                    out_ok = true;
                    break;
                }
            }
            if !out_ok {
                prisons.push((*c, from, w.feet()));
            }
        }
        println!(
            "pit sample: {} of {} low starts could not rise 0.3 in 30 s",
            prisons.len(),
            sample.len()
        );
        for (c, from, at) in prisons.iter().take(3) {
            println!("  cell {c} start {from:?} ended {at:?} escape={}", nav.cell(*c).unwrap().escape);
            // Which graph edge leads UP out of here, and what does the BODY
            // say when it tries to take it from a few distances back?
            let up: Vec<(u32, f32)> = nav
                .edges(*c)
                .map(|(b, _)| (b, nav.cell(b).unwrap().pos.y - from.y))
                .filter(|(_, dy)| *dy > 0.05)
                .collect();
            println!("    graph edges rising out of it: {up:?}");
            // Can the graph route from here up to the core at all, and how far?
            let (mut d, mut par) = (Vec::new(), Vec::new());
            nav.flood(*c, &std::collections::HashSet::new(), &mut d, &mut par);
            match d.get(core as usize).copied() {
                Some(cost) if cost != NAV_UNREACHED => {
                    let route = nav.path_to(core, &par);
                    let rises: Vec<f32> = route
                        .windows(2)
                        .map(|w| nav.cell(w[1]).unwrap().pos.y - nav.cell(w[0]).unwrap().pos.y)
                        .collect();
                    let max_rise = rises.iter().cloned().fold(0.0f32, f32::max);
                    println!(
                        "    route to the core: cost {cost}, {} waypoints, biggest single rise {max_rise:.3}",
                        route.len()
                    );
                    // The first RISING pair on the route is the step the body
                    // has to take. Ask the body about it, from a few
                    // distances back, exactly as it would arrive.
                    let full: Vec<u32> = std::iter::once(*c).chain(route.iter().copied()).collect();
                    if let Some(k) = full.windows(2).position(|w| {
                        nav.cell(w[1]).unwrap().pos.y - nav.cell(w[0]).unwrap().pos.y > 0.05
                    }) {
                        let (pa, pb) =
                            (nav.cell(full[k]).unwrap().pos, nav.cell(full[k + 1]).unwrap().pos);
                        let (dx, dz) = (pb.x - pa.x, pb.z - pa.z);
                        let len = (dx * dx + dz * dz).sqrt().max(1e-6);
                        println!(
                            "    rising pair on the route: {pa:?} -> {pb:?} (rise {:.4})",
                            pb.y - pa.y
                        );
                        for back in [0.4f32, 0.25, 0.1, 0.0, -0.1] {
                            let at =
                                vec3f(pa.x - dx / len * back, pa.y, pa.z - dz / len * back);
                            let probe = LevelWalker::new(at, dx.atan2(-dz), cfg, 1);
                            let step = cfg.speed / 60.0;
                            let r = probe
                                .step_refusal(&level, vec3f(dx / len * step, 0.0, dz / len * step));
                            println!("      {back:+.2} back: {r:?}");
                        }
                    }
                }
                _ => println!("    NO route to the core from here"),
            }
            for (b, dy) in up.iter().take(2) {
                let to = nav.cell(*b).unwrap().pos;
                let (dx, dz) = (to.x - from.x, to.z - from.z);
                let len = (dx * dx + dz * dz).sqrt().max(1e-6);
                for back in [0.30f32, 0.15, 0.05, 0.0] {
                    let at = vec3f(from.x - dx / len * back, from.y, from.z - dz / len * back);
                    let mut probe = LevelWalker::new(at, dx.atan2(-dz), cfg, 1);
                    let step = cfg.speed / 60.0;
                    let r = probe.step_refusal(&level, vec3f(dx / len * step, 0.0, dz / len * step));
                    println!("      rise {dy:.3}, {back:.2} back: {r:?}");
                    let _ = &mut probe;
                }
            }
        }
        assert!(left, "the deepest pit in {path} is a prison");
    }

    /// Measurement against a REAL published map: every rising step between
    /// adjacent grid columns (a stair tread by Doom's own step rule), judged
    /// the three ways a step can be judged — by the GRAPH (is the edge
    /// offered?), by the BODY from the far cell centre (`step_refusal` over
    /// the whole edge), and by the BODY arriving incrementally (per-tick
    /// probes marched along the edge, which is how the walker actually
    /// travels). Prints a per-flight table so "the tour avoids stairs" can
    /// be split into geometry (risers/lips walling the probes) versus
    /// policy (the planner not wanting to). Skipped unless `LEVEL_GLB`
    /// points at a map:
    /// `LEVEL_GLB=<path> cargo test -p makepad-render --release --lib -- real_level_stair --nocapture`
    #[test]
    fn real_level_stair_steps_agree_between_graph_and_body() {
        let Ok(path) = std::env::var("LEVEL_GLB") else { return };
        let bytes = std::fs::read(&path).expect("LEVEL_GLB readable");
        let model = crate::StaticModel::parse_glb(&bytes).expect("a static GLB");
        let cfg = WalkerConfig::default();
        let level = LevelCollision::from_packed(
            &model.vertices,
            crate::model::MODEL_VERTEX_FLOATS,
            &model.indices,
            UpAxis::Y,
        )
        .expect("collision");
        let level = match surface_kinds_from_glb(&bytes, model.triangle_count()) {
            Some(kinds) => level.with_kinds(kinds),
            None => level,
        };
        let nav = NavGrid::build(&level, &cfg);
        let tick = cfg.speed / 60.0;

        // The body marching one edge exactly as the walker travels it:
        // per-tick `step_refusal`, advancing over any step it accepts.
        // Returns the first refusal (with how far along it happened).
        let march = |from: Vec3f, to: Vec3f| -> (StepRefusal, f32) {
            let (dx, dz) = (to.x - from.x, to.z - from.z);
            let len = (dx * dx + dz * dz).sqrt().max(1e-6);
            let dir = vec3f(dx / len, 0.0, dz / len);
            let yaw = dir.x.atan2(-dir.z);
            let mut at = from;
            let mut travelled = 0.0f32;
            while travelled < len + cfg.radius {
                let probe = LevelWalker::new(at, yaw, cfg, 1);
                let r = probe.step_refusal(&level, vec3f(dir.x * tick, 0.0, dir.z * tick));
                match r {
                    StepRefusal::Ok { .. } => {}
                    other => return (other, travelled),
                }
                let want = vec3f(at.x + dir.x * tick, at.y, at.z + dir.z * tick);
                let up = cfg.step_up + STEP_EPS + hazard_escape_step(cfg.step_up);
                let Some(floor) =
                    level.floor_below(vec3f(want.x, want.y + up, want.z), up + cfg.fall_limit)
                else {
                    return (StepRefusal::NoFloor, travelled);
                };
                at = vec3f(want.x, floor.y, want.z);
                travelled += tick;
            }
            (StepRefusal::Ok { rise: to.y - from.y }, travelled)
        };
        // Where exactly is the obstruction a clearance refusal saw? Scan the
        // radial rays upward: the highest ray height (above the stand) that
        // still hits something within the body radius is the lip's top.
        let lip_top = |stand: Vec3f| -> Option<f32> {
            let mut top = None;
            let mut h = 0.02f32;
            while h < cfg.height {
                let origin = vec3f(stand.x, stand.y + h, stand.z);
                for i in 0..8 {
                    let a = std::f32::consts::TAU * i as f32 / 8.0;
                    if level.caster.any_hit(origin, vec3f(a.sin(), 0.0, -a.cos()), cfg.radius) {
                        top = Some(h);
                        break;
                    }
                }
                h += 0.01;
            }
            top
        };

        // Every rising pair between adjacent (orthogonal) columns whose rise
        // is a legal step: these ARE the stairs of the map.
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for a in 0..nav.len() {
            let (ix, iz) = nav.column_xz(nav.cells[a].column);
            for (dx, dz) in [(1i32, 0i32), (0, 1), (-1, 0), (0, -1)] {
                let Some(col) = nav.column_index(ix as i32 + dx, iz as i32 + dz) else {
                    continue;
                };
                for b in nav.column_cells(col) {
                    let dy = nav.cells[b].pos.y - nav.cells[a].pos.y;
                    if dy >= 0.1 && dy <= cfg.step_up + STEP_EPS {
                        pairs.push((a as u32, b as u32));
                    }
                }
            }
        }
        // Group the rising pairs into flights (connected stair edges): a
        // per-flight refusal table reads like the staircase on screen.
        let mut flight = vec![usize::MAX; nav.len()];
        let mut nflights = 0usize;
        for (a, b) in &pairs {
            let (fa, fb) = (flight[*a as usize], flight[*b as usize]);
            match (fa, fb) {
                (usize::MAX, usize::MAX) => {
                    flight[*a as usize] = nflights;
                    flight[*b as usize] = nflights;
                    nflights += 1;
                }
                (f, usize::MAX) => flight[*b as usize] = f,
                (usize::MAX, f) => flight[*a as usize] = f,
                (f, g) if f != g => {
                    for s in flight.iter_mut() {
                        if *s == g {
                            *s = f;
                        }
                    }
                }
                _ => {}
            }
        }
        #[derive(Default)]
        struct Tally {
            steps: usize,
            graph_missing: usize,
            up_walled: usize,
            up_no_clear: usize,
            up_other: usize,
            down_walled: usize,
            down_no_clear: usize,
            down_other: usize,
            lip_tops: Vec<f32>,
            sample: Option<Vec3f>,
        }
        let mut tallies: std::collections::HashMap<usize, Tally> =
            std::collections::HashMap::new();
        for (a, b) in &pairs {
            let f = flight[*a as usize];
            let t = tallies.entry(f).or_default();
            t.steps += 1;
            let (pa, pb) = (nav.cells[*a as usize].pos, nav.cells[*b as usize].pos);
            t.sample.get_or_insert(pa);
            if !nav.edges(*a).any(|(c, _)| c == *b) {
                t.graph_missing += 1;
            }
            for (from, to, walled, no_clear, other) in [
                (pa, pb, &mut t.up_walled, &mut t.up_no_clear, &mut t.up_other),
                (pb, pa, &mut t.down_walled, &mut t.down_no_clear, &mut t.down_other),
            ] {
                let (r, at) = march(from, to);
                match r {
                    StepRefusal::Ok { .. } => {}
                    StepRefusal::Walled => *walled += 1,
                    StepRefusal::NoClearance { .. } => {
                        *no_clear += 1;
                        let (dx, dz) = (to.x - from.x, to.z - from.z);
                        let len = (dx * dx + dz * dz).sqrt().max(1e-6);
                        let spot = vec3f(
                            from.x + dx / len * at,
                            from.y.max(to.y),
                            from.z + dz / len * at,
                        );
                        if let Some(top) = lip_top(spot) {
                            t.lip_tops.push(top);
                        }
                    }
                    _ => *other += 1,
                }
            }
        }
        let mut rows: Vec<(usize, Tally)> = tallies.into_iter().collect();
        rows.sort_by(|x, y| y.1.steps.cmp(&x.1.steps));
        let mut total = Tally::default();
        for (_, t) in &rows {
            total.steps += t.steps;
            total.graph_missing += t.graph_missing;
            total.up_walled += t.up_walled;
            total.up_no_clear += t.up_no_clear;
            total.up_other += t.up_other;
            total.down_walled += t.down_walled;
            total.down_no_clear += t.down_no_clear;
            total.down_other += t.down_other;
            total.lip_tops.extend_from_slice(&t.lip_tops);
        }
        println!(
            "stair steps: {} in {} flights; graph refused {}; body UP walled {} / no-clearance {} / other {}; DOWN walled {} / no-clearance {} / other {}",
            total.steps,
            rows.len(),
            total.graph_missing,
            total.up_walled,
            total.up_no_clear,
            total.up_other,
            total.down_walled,
            total.down_no_clear,
            total.down_other,
        );
        println!("  flight  steps  graph-  up W/NC/o  down W/NC/o  near");
        for (f, t) in rows.iter().take(20) {
            println!(
                "  {:>6}  {:>5}  {:>6}  {:>2}/{:>2}/{:>2}   {:>2}/{:>2}/{:>2}    {:?}",
                f,
                t.steps,
                t.graph_missing,
                t.up_walled,
                t.up_no_clear,
                t.up_other,
                t.down_walled,
                t.down_no_clear,
                t.down_other,
                t.sample.unwrap_or(vec3f(0.0, 0.0, 0.0)),
            );
        }
        // The graph-refused pairs, dissected: which probe ray said Walled,
        // how far along it hit, and the smallest knee height that passes.
        let mut shown = 0;
        for (a, b) in &pairs {
            if nav.edges(*a).any(|(c, _)| c == *b) || shown >= 24 {
                continue;
            }
            shown += 1;
            let (pa, pb) = (nav.cells[*a as usize].pos, nav.cells[*b as usize].pos);
            let dy = pb.y - pa.y;
            let (dx, dz) = (pb.x - pa.x, pb.z - pa.z);
            let dist = (dx * dx + dz * dz).sqrt().max(1e-6);
            let dir = vec3f(dx / dist, 0.0, dz / dist);
            let side = vec3f(-dir.z, 0.0, dir.x);
            let knee = cfg.step_up + (cfg.height - cfg.step_up) * 0.15;
            let mut hits = String::new();
            for (label, h) in [("knee", knee), ("chest", cfg.height * 0.85)] {
                for l in [-cfg.radius, 0.0, cfg.radius] {
                    let origin = vec3f(pa.x + side.x * l, pa.y + h, pa.z + side.z * l);
                    if let Some((t, _)) = level.caster.nearest_hit(origin, dir, dist + cfg.radius)
                    {
                        hits.push_str(&format!(" {label}@{l:+.2} t={t:.2}"));
                    }
                }
            }
            let mut min_knee = None;
            let mut h = knee;
            while h < cfg.height * 0.85 {
                if !level.path_blocked(pa, pb, cfg.radius, h, cfg.height) {
                    // path_blocked derives its own ray height from the step
                    // argument; report the raw ray height that passed.
                    min_knee = Some(knee_height(h, cfg.height));
                    break;
                }
                h += 0.02;
            }
            println!(
                "  graph-refused {}->{} rise {dy:.3} at {pa:?}:{hits} (lowest passing ray {min_knee:?})",
                a, b
            );
        }
        if !total.lip_tops.is_empty() {
            let mut tops = total.lip_tops.clone();
            tops.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "  no-clearance lip tops above the stand: min {:.3} median {:.3} max {:.3} ({} measured)",
                tops[0],
                tops[tops.len() / 2],
                tops[tops.len() - 1],
                tops.len()
            );
            let knee = knee_height(cfg.step_up, cfg.height);
            let under = tops.iter().filter(|t| **t < knee).count();
            println!(
                "  of those, {under} sit under the shared knee height {knee:.3} (tolerated once the band probes agree)"
            );
        }
    }

    #[test]
    fn a_nukage_pit_with_a_one_step_rim_is_a_place_you_can_leave() {
        // Doom's rule is step <= 24 map units, INCLUSIVE, and a sector floor
        // baked through a float pipeline lands a hair over as often as under.
        // A rim exactly one step tall used to read as a wall, and a walker in
        // the green swamp was walled into it.
        let cfg = WalkerConfig::default(); // the real Doom body: step 0.375
        let (mut p, mut i) = (Vec::new(), Vec::new());
        let mut kinds = Vec::new();
        let slab = |p: &mut Vec<Vec3f>,
                        i: &mut Vec<u32>,
                        kinds: &mut Vec<SurfaceKind>,
                        x0: f32,
                        x1: f32,
                        z0: f32,
                        z1: f32,
                        y: f32,
                        kind: SurfaceKind| {
            floor(p, i, x0, x1, z0, z1, y);
            kinds.push(kind);
            kinds.push(kind);
        };
        // A sunken nukage pool (z -4..0) with clean ground either side, one
        // step up — and the step is the float the importer actually bakes.
        let rim = 0.375_f32 * 1.000_002;
        slab(&mut p, &mut i, &mut kinds, -3.0, 3.0, 0.0, 4.0, rim, SurfaceKind::Floor);
        slab(&mut p, &mut i, &mut kinds, -3.0, 3.0, -4.0, 0.0, 0.0, SurfaceKind::Hazard);
        slab(&mut p, &mut i, &mut kinds, -3.0, 3.0, -8.0, -4.0, rim, SurfaceKind::Floor);
        slab(&mut p, &mut i, &mut kinds, -3.0, 3.0, -8.0, 4.0, 4.0, SurfaceKind::Floor);
        let level = LevelCollision::from_positions(p, i).with_kinds(kinds);
        let nav = NavGrid::build(&level, &cfg);
        // The graph knows the way out of the pool.
        let in_pool: Vec<u32> = (0..nav.len() as u32)
            .filter(|c| nav.cell(*c).unwrap().kind == SurfaceKind::Hazard)
            .collect();
        assert!(!in_pool.is_empty(), "the pool is in the grid");
        let escapes = in_pool
            .iter()
            .filter(|c| {
                nav.edges(**c)
                    .any(|(b, _)| nav.cell(b).unwrap().kind != SurfaceKind::Hazard)
            })
            .count();
        assert!(escapes > 0, "no cell of the pool has a way out of it");
        // And the body climbs it: dropped in the middle of the pool, the
        // tour is on clean ground inside twenty seconds.
        let start = nav.cell(in_pool[in_pool.len() / 2]).unwrap().pos;
        let mut w = LevelWalker::new(start, 0.0, cfg, 6);
        let mut escaped = false;
        for _ in 0..1200 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            escaped |= w.feet().y > 0.3;
        }
        assert!(escaped, "still in the swamp at {:?}", w.feet());
    }

    #[test]
    fn an_external_route_is_followed_verbatim() {
        // The player_nav seam: the planner owns WHERE, this module owns HOW.
        let cfg = cfg();
        let level = two_floors();
        let nav = NavGrid::build(&level, &cfg);
        let start = vec3f(0.0, 0.0, 5.0);
        let from = nav.cell_at(start).expect("a cell under the start");
        let goal = (0..nav.len() as u32)
            .find(|i| {
                let c = nav.cell(*i).unwrap();
                (c.pos.y - 1.5).abs() < 1e-3 && c.pos.z < -9.0
            })
            .expect("an upstairs cell");
        let (mut dist, mut parent) = (Vec::new(), Vec::new());
        nav.flood(from, &std::collections::HashSet::new(), &mut dist, &mut parent);
        let route = nav.path_to(goal, &parent);
        assert!(route.len() > 4, "a real route: {}", route.len());

        let mut w = LevelWalker::new(start, 0.0, cfg, 8);
        w.set_external_planner(true);
        assert!(w.has_external_planner());
        w.set_route(route.clone());
        for _ in 0..7200 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            if w.route().is_empty() {
                break;
            }
        }
        assert!(w.route().is_empty(), "the route was walked to the end");
        let end = nav.cell(goal).unwrap().pos;
        let (dx, dz) = (w.feet().x - end.x, w.feet().z - end.z);
        assert!((dx * dx + dz * dz).sqrt() < 1.0, "arrived at the goal: {:?}", w.feet());
        // And the tour engine did NOT invent goals of its own on the way.
        let stats = w.nav_stats();
        assert_eq!(stats.replans, 0, "the built-in planner stayed out of it: {stats:?}");
        assert_eq!(stats.cuts, 0, "and it did not cut anywhere");
        // With the route done, the planner's heading is what the body holds.
        w.set_target_yaw(Some(1.0));
        for _ in 0..120 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
        }
        assert!((w.yaw() - 1.0).abs() < 0.15, "held the planner's heading: {}", w.yaw());
        // Doors are the planner's to ask for in this mode.
        w.request_door(Some(3));
        w.tick_in(1.0 / 60.0, &level, Some(&nav));
        assert_eq!(w.wanted_door(), Some(3));
        w.set_door_open(3, true);
        w.tick_in(1.0 / 60.0, &level, Some(&nav));
        assert_eq!(w.wanted_door(), None);
    }

    #[test]
    fn orthogonal_and_diagonal_steps_agree_on_open_ground() {
        // Parity check for the planner: on clean open floor a diagonal step
        // is as good as an orthogonal one. (Against a rim CORNER it is not —
        // the landing clearance refuses it — which is why the planner should
        // prefer orthogonal exits out of a sunken area.)
        let cfg = cfg();
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -4.0, 4.0, -4.0, 4.0, 0.0);
        floor(&mut p, &mut i, -4.0, 4.0, -4.0, 4.0, 3.0);
        let level = LevelCollision::from_positions(p, i);
        let w = LevelWalker::new(vec3f(0.0, 0.0, 0.0), 0.0, cfg, 1);
        let step = cfg.speed / 60.0;
        for (dx, dz) in [(1.0, 0.0), (0.0, 1.0), (0.707, 0.707), (-0.707, 0.707)] {
            let r = w.step_refusal(&level, vec3f(dx * step, 0.0, dz * step));
            assert!(matches!(r, StepRefusal::Ok { .. }), "({dx},{dz}) refused: {r:?}");
        }
    }

    #[test]
    fn a_map_cut_into_rooms_reports_its_pieces() {
        let cfg = cfg();
        let nav = NavGrid::build(&two_sealed_rooms(), &cfg);
        let sizes = nav.component_sizes();
        assert!(sizes.len() >= 2, "two sealed rooms are two pieces: {sizes:?}");
        assert!(sizes[0] > 20 && sizes[1] > 20, "both rooms have room: {sizes:?}");
        let start = nav.best_start().expect("a start");
        let here = nav.component_of(start).unwrap();
        // And there is somewhere else to go.
        let other = nav.next_region(start, &vec![0.0; nav.len()]).expect("another region");
        assert_ne!(nav.component_of(other), Some(here), "the cut lands in a DIFFERENT room");
    }

    #[test]
    fn a_seen_room_is_left_for_one_it_has_not_seen() {
        // The tour must not spend the night in the room it started in just
        // because the rest of the map is behind a wall it cannot open.
        let cfg = cfg();
        let level = two_sealed_rooms();
        let nav = NavGrid::build(&level, &cfg);
        let start = nav.cell(nav.best_start().unwrap()).unwrap().pos;
        let mut w = LevelWalker::new(start, 0.0, cfg, 12);
        let first_room = nav.component_of(nav.cell_at(start).unwrap()).unwrap();
        let (mut cut_to_other, mut flashes) = (false, 0);
        for _ in 0..18_000 {
            w.tick_in(1.0 / 60.0, &level, Some(&nav));
            if w.take_flash() {
                flashes += 1;
            }
            if let Some(c) = nav.cell_at(w.feet()) {
                cut_to_other |= nav.component_of(c) != Some(first_room);
            }
        }
        let stats = w.nav_stats();
        assert!(cut_to_other, "never left the first room: {stats:?}");
        assert!(stats.cuts >= 1, "the cut is counted: {stats:?}");
        assert_eq!(flashes as u64, stats.cuts, "every cut flashes exactly once");
    }

    #[test]
    fn a_teleport_pad_cuts_to_its_destination() {
        let cfg = cfg();
        let level = two_sealed_rooms();
        let mut nav = NavGrid::build(&level, &cfg);
        // A pad in the near room, landing in the far one.
        nav.mark_teleports(&[(
            vec3f(-1.0, 0.0, 0.5),
            vec3f(1.0, 1.0, 1.5),
            vec3f(0.0, 0.0, -17.0),
            std::f32::consts::PI,
        )]);
        let pad = (0..nav.len() as u32)
            .find(|c| nav.cell(*c).unwrap().teleport == Some(0))
            .expect("the pad marked its cells");
        assert!(nav.teleport_target(0).is_some());
        // The planner can now route from the near room into the far one:
        // the pad is a ONE-WAY edge, exactly like Doom's.
        let (mut dist, mut parent) = (Vec::new(), Vec::new());
        nav.flood(pad, &std::collections::HashSet::new(), &mut dist, &mut parent);
        let far = (0..nav.len())
            .filter(|i| nav.cell(*i as u32).unwrap().pos.z < -14.0)
            .filter(|i| dist[*i] != NAV_UNREACHED)
            .count();
        assert!(far > 20, "the far room is reachable through the pad: {far} cells");
        // Standing on the pad cuts to the far room, facing the given way.
        let mut w = LevelWalker::new(nav.cell(pad).unwrap().pos, 0.0, cfg, 2);
        w.tick_in(1.0 / 60.0, &level, Some(&nav));
        assert!(w.feet().z < -14.0, "teleported: {:?}", w.feet());
        assert!((w.yaw() - std::f32::consts::PI).abs() < 1e-3, "arrives facing the anchor's way");
        assert!(w.take_flash(), "a teleport flashes");
        assert_eq!(w.cut_count(), 1);
    }

    #[test]
    fn nav_cells_stay_a_body_clear_of_the_walls() {
        let cfg = cfg();
        let level = corridor();
        let nav = NavGrid::build(&level, &cfg);
        assert!(!nav.is_empty(), "a corridor has walkable cells");
        for c in 0..nav.len() as u32 {
            let p = nav.cell(c).unwrap().pos;
            assert!(
                p.x > -1.0 + cfg.radius - 1e-3 && p.x < 1.0 - cfg.radius + 1e-3,
                "cell inside a wall: {p:?}"
            );
            assert!((p.y - 0.0).abs() < 1e-3, "cells sit on the floor: {p:?}");
        }
        // Every cell is found again from its own position.
        for c in 0..nav.len() as u32 {
            assert_eq!(nav.cell_at(nav.cell(c).unwrap().pos), Some(c));
        }
    }

    #[test]
    fn z_up_sources_are_converted_at_build_time() {
        // One triangle lying flat in a Z-up source (constant z) must become
        // a floor (constant y) after conversion.
        let verts: Vec<f32> = vec![
            0.0, 0.0, 5.0, //
            1.0, 0.0, 5.0, //
            0.0, 1.0, 5.0, //
        ];
        let level =
            LevelCollision::from_packed(&verts, 3, &[0, 1, 2], UpAxis::Z).expect("built");
        let (min, max) = level.bounds();
        assert!((min.y - 5.0).abs() < 1e-6 && (max.y - 5.0).abs() < 1e-6, "{min:?} {max:?}");
        let hit = level.floor_below(vec3f(0.2, 9.0, -0.2), 10.0).expect("floor under the probe");
        assert!((hit.y - 5.0).abs() < 1e-4);
        assert!(hit.flatness > 0.99, "converted triangle is flat: {hit:?}");
    }
}

// player_nav: shared synthetic-level builders, so the player-behaviour
// planner's test suite builds its fixtures the same way this module's tests
// do (triangle soups through `LevelCollision::from_positions`). The `tests`
// module above keeps its private copies — it is frozen; new suites use these.
#[cfg(test)]
pub(crate) mod test_geometry {
    use super::*;

    /// Append a quad (two triangles) to a triangle soup.
    pub fn quad(pos: &mut Vec<Vec3f>, idx: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, d: Vec3f) {
        let n = pos.len() as u32;
        pos.extend_from_slice(&[a, b, c, d]);
        idx.extend_from_slice(&[n, n + 1, n + 2, n, n + 2, n + 3]);
    }

    /// A horizontal slab top at `y` spanning the given x/z range.
    pub fn floor(
        pos: &mut Vec<Vec3f>,
        idx: &mut Vec<u32>,
        x0: f32,
        x1: f32,
        z0: f32,
        z1: f32,
        y: f32,
    ) {
        quad(
            pos,
            idx,
            vec3f(x0, y, z0),
            vec3f(x1, y, z0),
            vec3f(x1, y, z1),
            vec3f(x0, y, z1),
        );
    }

    /// A vertical wall spanning x0..x1 at `z`, from y0 to y1.
    pub fn wall_z(
        pos: &mut Vec<Vec3f>,
        idx: &mut Vec<u32>,
        x0: f32,
        x1: f32,
        z: f32,
        y0: f32,
        y1: f32,
    ) {
        quad(
            pos,
            idx,
            vec3f(x0, y0, z),
            vec3f(x1, y0, z),
            vec3f(x1, y1, z),
            vec3f(x0, y1, z),
        );
    }

    /// A vertical wall spanning z0..z1 at `x`, from y0 to y1.
    pub fn wall_x(
        pos: &mut Vec<Vec3f>,
        idx: &mut Vec<u32>,
        z0: f32,
        z1: f32,
        x: f32,
        y0: f32,
        y1: f32,
    ) {
        quad(
            pos,
            idx,
            vec3f(x, y0, z0),
            vec3f(x, y0, z1),
            vec3f(x, y1, z1),
            vec3f(x, y1, z0),
        );
    }
}

// ---------------------------------------------------------------------------
// The map's units, and a route a body can actually walk
// ---------------------------------------------------------------------------

/// What went wrong on Quake 1 maps, pinned.
///
/// Three separate faults, all of which read to a viewer as one thing — a
/// walker that spins, stops and gets stuck on corners:
///
/// 1. the body was in the PRESET's units, not the map's (a Quake 1 level is
///    published at 1/32, twice a Doom level's metres per map unit);
/// 2. the nav lattice was a fixed 0.5 m instead of one body wide, so the
///    graph was finer than the legs walking it;
/// 3. the walker stood still for any heading change over 34°, and walked
///    at a speed its own turn rate could not steer — so it orbited close
///    waypoints instead of reaching them.
#[cfg(test)]
mod walk_in_the_maps_units {
    use super::test_geometry::{floor, wall_x, wall_z};
    use super::*;

    /// A roofed room `span` across, floor at 0.
    fn room(span: f32, height: f32) -> LevelCollision {
        let (mut p, mut i) = (Vec::new(), Vec::new());
        let h = span * 0.5;
        floor(&mut p, &mut i, -h, h, -h, h, 0.0);
        floor(&mut p, &mut i, -h, h, -h, h, height);
        wall_x(&mut p, &mut i, -h, h, -h, 0.0, height);
        wall_x(&mut p, &mut i, -h, h, h, 0.0, height);
        wall_z(&mut p, &mut i, -h, h, -h, 0.0, height);
        wall_z(&mut p, &mut i, -h, h, h, 0.0, height);
        LevelCollision::from_positions(p, i)
    }

    #[test]
    fn a_declared_step_puts_the_whole_body_in_the_maps_units() {
        let preset = WalkerConfig::for_style(BobStyle::Quake);
        // Quake 1 publishes at 1/32: its 18-unit step is 0.5625, twice the
        // 18-at-1/64 the preset carries.
        let quake1 = preset.with_declared(Some(0.5625), Some(1.4375));
        let k = 0.5625 / preset.step_up;
        assert!((k - 2.0).abs() < 0.01, "the Quake 1 scale is 2x: {k}");
        assert!((quake1.step_up - 0.5625).abs() < 1e-4, "{}", quake1.step_up);
        assert!((quake1.radius - preset.radius * k).abs() < 1e-4, "{}", quake1.radius);
        assert!((quake1.height - preset.height * k).abs() < 1e-4, "{}", quake1.height);
        assert!((quake1.speed - preset.speed * k).abs() < 1e-3, "{}", quake1.speed);
        // Gravity is a length per second squared: it scales once too, or a
        // body in a double-scale world falls in slow motion.
        assert!((quake1.gravity - preset.gravity * k).abs() < 1e-3, "{}", quake1.gravity);
        assert!((quake1.fall_limit - preset.fall_limit * k).abs() < 1e-3, "{}", quake1.fall_limit);
        // The eye is declared outright, not scaled: it is what the map says.
        assert!((quake1.eye_height - 1.4375).abs() < 1e-4, "{}", quake1.eye_height);

        // A map at the preset's own scale changes nothing but the eye.
        let doom = WalkerConfig::for_style(BobStyle::Doom).with_declared(Some(0.375), Some(0.6406));
        let base = WalkerConfig::for_style(BobStyle::Doom);
        assert!((doom.step_up - base.step_up).abs() < 1e-6, "{}", doom.step_up);
        assert!((doom.radius - base.radius).abs() < 1e-6, "{}", doom.radius);
        assert!((doom.speed - base.speed).abs() < 1e-6, "{}", doom.speed);

        // Declaring nothing (Build states no step height) keeps the preset,
        // and a nonsense declaration is refused rather than believed.
        let none = WalkerConfig::for_style(BobStyle::Duke).with_declared(None, None);
        assert_eq!(none.step_up, WalkerConfig::for_style(BobStyle::Duke).step_up);
        let junk = WalkerConfig::for_style(BobStyle::Doom).with_declared(Some(400.0), None);
        assert_eq!(junk.step_up, base.step_up, "a 400 m step is a bad sidecar, not a scale");
        let nan = WalkerConfig::for_style(BobStyle::Doom).with_declared(Some(f32::NAN), None);
        assert_eq!(nan.step_up, base.step_up);
    }

    #[test]
    fn the_nav_lattice_is_one_body_wide_whatever_the_map_scale() {
        let level = room(24.0, 4.0);
        let doom = WalkerConfig::for_style(BobStyle::Doom);
        let quake1 = WalkerConfig::for_style(BobStyle::Quake).with_declared(Some(0.5625), None);
        let small = NavGrid::build(&level, &doom);
        let big = NavGrid::build(&level, &quake1);
        assert!((small.cell_size() - doom.radius * 2.0).abs() < 1e-4, "{}", small.cell_size());
        assert!((big.cell_size() - quake1.radius * 2.0).abs() < 1e-4, "{}", big.cell_size());
        assert!(
            big.cell_size() > small.cell_size() * 1.9,
            "a body twice as wide gets a lattice twice as coarse: {} vs {}",
            big.cell_size(),
            small.cell_size()
        );
    }

    /// Put a walker in an open room on an external route to one cell, and
    /// report how long it took to get there (in ticks) and how far it
    /// wandered on the way.
    fn drive_to(cfg: WalkerConfig, target: Vec3f, ticks: usize) -> (Option<usize>, f32) {
        let level = room(32.0, 6.0);
        let grid = NavGrid::build(&level, &cfg);
        let start = vec3f(0.0, 0.0, 0.0);
        let goal = grid.cell_at(target).expect("target is walkable");
        let goal_pos = grid.cell(goal).expect("cell").pos;
        // Facing -Z: the target below is off to the side, which is what a
        // route corner looks like from the body's point of view.
        let mut w = LevelWalker::new(start, 0.0, cfg, 3);
        w.set_external_planner(true);
        w.set_route(vec![goal]);
        let mut travelled = 0.0;
        let mut prev = start;
        for t in 0..ticks {
            w.tick_in(1.0 / 60.0, &level, Some(&grid));
            let feet = w.feet();
            travelled += ((feet.x - prev.x).powi(2) + (feet.z - prev.z).powi(2)).sqrt();
            prev = feet;
            let gap =
                ((feet.x - goal_pos.x).powi(2) + (feet.z - goal_pos.z).powi(2)).sqrt();
            if gap < grid.cell_size() * 0.6 || w.route().is_empty() {
                return (Some(t), travelled);
            }
        }
        (None, travelled)
    }

    #[test]
    fn a_waypoint_inside_the_turning_circle_is_reached_and_not_orbited() {
        // The Quake 1 body: 3.8 m/s over 1.6 rad/s is a 2.4 m turning
        // circle, and its nav cells are a metre apart. A waypoint 1.5 m to
        // the side sits well inside that circle.
        let cfg = WalkerConfig::for_style(BobStyle::Quake).with_declared(Some(0.5625), None);
        let radius = cfg.turn_radius();
        assert!(radius > 2.0, "the Quake 1 tour has a wide turning circle: {radius}");
        let target = vec3f(1.5, 0.0, 0.0);
        let (arrived, travelled) = drive_to(cfg, target, 60 * 8);
        let arrived = arrived.expect("the body must reach a waypoint inside its turning circle");
        // Orbiting shows up as distance without arrival: the straight line
        // is 1.5 m, and a body that circles covers many times that.
        assert!(
            travelled < 6.0,
            "walked {travelled:.1} m to a point 1.5 m away — that is an orbit, not a path"
        );
        assert!(arrived < 60 * 5, "took {arrived} ticks to walk 1.5 m");
    }

    #[test]
    fn the_body_covers_ground_while_it_turns() {
        // A route corner behind the shoulder: the heading error starts well
        // over the "facing the target" gate. The body used to stand
        // perfectly still until it had spun to within 34° — which is the
        // stop-spin-go stutter, and (being no progress) it also tripped the
        // planner's watchdog, whose dropped legs then came back cell by
        // cell, so every one of THOSE turned 45° too.
        let cfg = WalkerConfig::for_style(BobStyle::Doom);
        let level = room(32.0, 6.0);
        let grid = NavGrid::build(&level, &cfg);
        // Straight out to the side: yaw 0 looks down -Z, so +X is 90° off.
        let goal = grid.cell_at(vec3f(6.0, 0.0, 0.0)).expect("walkable");
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 0.0), 0.0, cfg, 3);
        w.set_external_planner(true);
        w.set_route(vec![goal]);
        let mut moved_while_turning = 0.0;
        for _ in 0..18 {
            let before = w.feet();
            w.tick_in(1.0 / 60.0, &level, Some(&grid));
            // Still turning: nowhere near the target heading yet.
            assert!(
                w.yaw().abs() < std::f32::consts::FRAC_PI_2 - 0.2,
                "the turn should not be finished in 0.3 s"
            );
            let feet = w.feet();
            moved_while_turning +=
                ((feet.x - before.x).powi(2) + (feet.z - before.z).powi(2)).sqrt();
        }
        assert!(
            moved_while_turning > 0.02,
            "the body stood still for the whole turn ({moved_while_turning:.3} m)"
        );
    }

    #[test]
    fn a_walker_never_walks_faster_than_its_turn_can_steer() {
        // The invariant behind both of the above: the circle a body traces
        // (speed / turn rate) must fit inside the distance to what it is
        // steering at.
        let cfg = WalkerConfig::for_style(BobStyle::Quake).with_declared(Some(0.5625), None);
        let level = room(32.0, 6.0);
        let grid = NavGrid::build(&level, &cfg);
        let goal = grid.cell_at(vec3f(1.2, 0.0, 0.6)).expect("walkable");
        let goal_pos = grid.cell(goal).expect("cell").pos;
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 0.0), 0.0, cfg, 3);
        w.set_external_planner(true);
        w.set_route(vec![goal]);
        for _ in 0..240 {
            let before = w.feet();
            w.tick_in(1.0 / 60.0, &level, Some(&grid));
            if w.route().is_empty() {
                return; // arrived, which is the point
            }
            let feet = w.feet();
            let speed =
                ((feet.x - before.x).powi(2) + (feet.z - before.z).powi(2)).sqrt() * 60.0;
            let gap =
                ((before.x - goal_pos.x).powi(2) + (before.z - goal_pos.z).powi(2)).sqrt();
            // The cap is on the speed the body WANTS; the actual one eases
            // down over `SPEED_EASE_SECS`, and the gap closes while it does.
            let lag = speed * SPEED_EASE_SECS;
            assert!(
                speed <= cfg.turn_rate * (gap + lag) + 0.05,
                "walked {speed:.2} m/s at a waypoint {gap:.2} m away — a circle of \
                 {:.2} m round a point it can never reach",
                speed / cfg.turn_rate
            );
        }
        panic!("never reached a waypoint 1.3 m away in four seconds");
    }

    #[test]
    fn a_fresh_route_forgets_the_old_ones_slide() {
        // The "try the other way round the obstruction" offset is armed by
        // the stuck watchdog and cleared by the next plan — except that the
        // clearing lives in the BUILT-IN tour's replan, which an external
        // planner never runs. So on a `player_nav` tour the first give-up
        // left the body steering 34° off every bearing it was handed, for
        // the rest of the level, and a body that crabs converges on nothing.
        let cfg = WalkerConfig::for_style(BobStyle::Doom);
        let level = room(32.0, 6.0);
        let grid = NavGrid::build(&level, &cfg);
        let goal = grid.cell_at(vec3f(0.0, 0.0, -6.0)).expect("walkable");
        let mut w = LevelWalker::new(vec3f(0.0, 0.0, 0.0), 0.0, cfg, 3);
        w.set_external_planner(true);
        w.set_route(vec![goal]);
        w.tick_in(1.0 / 60.0, &level, Some(&grid));
        // Arm it the way the watchdog does.
        w.nav.as_mut().expect("nav state").slide = -0.6;
        w.set_route(vec![goal]);
        assert_eq!(
            w.nav.as_ref().expect("nav state").slide,
            0.0,
            "a fresh route must not inherit the last one's slide offset"
        );
        // And the body converges on it.
        let goal_pos = grid.cell(goal).expect("cell").pos;
        let mut best = f32::MAX;
        for _ in 0..60 * 8 {
            w.tick_in(1.0 / 60.0, &level, Some(&grid));
            let feet = w.feet();
            best = best
                .min(((feet.x - goal_pos.x).powi(2) + (feet.z - goal_pos.z).powi(2)).sqrt());
        }
        assert!(
            best < grid.cell_size(),
            "a crabbing body never converges: closest approach {best:.2} m"
        );
    }
}

#[cfg(test)]
mod level_vehicle_tests {
    use super::*;

    fn quad(p: &mut Vec<Vec3f>, i: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, d: Vec3f) {
        let n = p.len() as u32;
        p.extend_from_slice(&[a, b, c, d]);
        i.extend_from_slice(&[n, n + 1, n + 2, n, n + 2, n + 3]);
    }

    fn floor(p: &mut Vec<Vec3f>, i: &mut Vec<u32>, x0: f32, x1: f32, z0: f32, z1: f32, y: f32) {
        quad(p, i, vec3f(x0, y, z0), vec3f(x1, y, z0), vec3f(x1, y, z1), vec3f(x0, y, z1));
    }

    /// Wall in the x/y plane at `z`, spanning x0..x1, y0..y1.
    fn wall_z(p: &mut Vec<Vec3f>, i: &mut Vec<u32>, x0: f32, x1: f32, z: f32, y0: f32, y1: f32) {
        quad(p, i, vec3f(x0, y0, z), vec3f(x1, y0, z), vec3f(x1, y1, z), vec3f(x0, y1, z));
    }

    /// A room 8×8, floor 0, ceiling `roof`, with a doorway in the z=0 wall:
    /// opening x -1..1, lintel from `door_h` up to the ceiling. Room extends
    /// z 0..8; open ground z<0.
    fn doorway_room(roof: f32, door_h: f32) -> LevelCollision {
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -8.0, 8.0, -8.0, 8.0, 0.0);
        floor(&mut p, &mut i, -4.0, 4.0, 0.0, 8.0, roof);
        // The z=0 wall around the opening.
        wall_z(&mut p, &mut i, -4.0, -1.0, 0.0, 0.0, roof);
        wall_z(&mut p, &mut i, 1.0, 4.0, 0.0, 0.0, roof);
        wall_z(&mut p, &mut i, -1.0, 1.0, 0.0, door_h, roof); // lintel
        // Enclosing walls: sides at x ±4, far wall at z 8.
        quad(
            &mut p,
            &mut i,
            vec3f(-4.0, 0.0, 0.0),
            vec3f(-4.0, 0.0, 8.0),
            vec3f(-4.0, roof, 8.0),
            vec3f(-4.0, roof, 0.0),
        );
        quad(
            &mut p,
            &mut i,
            vec3f(4.0, 0.0, 0.0),
            vec3f(4.0, 0.0, 8.0),
            vec3f(4.0, roof, 8.0),
            vec3f(4.0, roof, 0.0),
        );
        wall_z(&mut p, &mut i, -4.0, 4.0, 8.0, 0.0, roof);
        LevelCollision::from_positions(p, i)
    }

    #[test]
    fn ray_hit_reports_distance_and_a_normal_facing_the_ray() {
        let level = doorway_room(3.0, 2.0);
        let (t, n) = level
            .ray_hit(vec3f(2.0, 1.0, 2.0), vec3f(0.0, 0.0, -1.0), 10.0)
            .expect("the z=0 wall is 2 m away");
        assert!((t - 2.0).abs() < 1.0e-3, "distance {t}");
        assert!(n.z > 0.99, "normal {n:?} must face back along the ray");
        // Up into the ceiling.
        let (t, n) = level
            .ray_hit(vec3f(0.0, 1.0, 4.0), vec3f(0.0, 1.0, 0.0), 10.0)
            .expect("ceiling above");
        assert!((t - 2.0).abs() < 1.0e-3, "ceiling distance {t}");
        assert!(n.y < -0.99, "ceiling normal {n:?} pushes down");
    }

    #[test]
    fn ground_under_stands_a_body_on_the_floor_not_the_ceiling() {
        let level = doorway_room(3.0, 2.0);
        // Inside the room, spawned mid-air: the room's floor, though the
        // ceiling is a nearer surface for a probe from the sky.
        assert_eq!(level.ground_under(0.0, 4.0, 1.2, 1.0), Some(0.0));
        // Spawned above the roof: the roof is standable from up there.
        assert_eq!(level.ground_under(0.0, 4.0, 3.5, 1.0), Some(3.0));
    }

    #[test]
    fn ground_under_lifts_a_body_buried_under_a_raised_floor() {
        // Two slabs: ground at 0 (z<0 half) — modelled by a low slab — and a
        // raised floor at 1.5 with headroom above.
        let (mut p, mut i) = (Vec::new(), Vec::new());
        floor(&mut p, &mut i, -4.0, 4.0, -4.0, 4.0, 1.5);
        floor(&mut p, &mut i, -4.0, 4.0, -4.0, 4.0, 4.0); // ceiling
        let level = LevelCollision::from_positions(p, i);
        // A car spawned at y 0.6 under the 1.5 floor belongs ON that floor.
        assert_eq!(level.ground_under(0.0, 0.0, 0.6, 1.0), Some(1.5));
    }

    #[test]
    fn room_at_measures_headroom_and_span() {
        let level = doorway_room(3.0, 2.0);
        let (headroom, span) = level.room_at(vec3f(0.0, 0.5, 4.0)).expect("indoors");
        assert!((headroom - 3.0).abs() < 0.05, "headroom {headroom}");
        // Walls at x ±4 bound the narrowest axis (the z probe escapes
        // through the doorway to open ground).
        assert!((span - 8.0).abs() < 0.5, "span {span} should be the 8 m width");
        // Outside: no ceiling, no measurement.
        assert!(level.room_at(vec3f(6.0, 0.5, 4.0)).is_none());
    }

    #[test]
    fn a_body_taller_than_the_door_cannot_pass_a_shorter_one_can() {
        let level = doorway_room(3.0, 2.0);
        // A footprint sweep straight through the doorway, x -0.6..0.6.
        let moves: Vec<(Vec3f, Vec3f)> = [-0.6f32, 0.0, 0.6]
            .iter()
            .map(|&x| (vec3f(x, 0.0, -2.0), vec3f(x, 0.0, 2.0)))
            .collect();
        // 1.4 m tall body: knee, waist, roof rays all under the 2 m lintel.
        assert!(
            !level.moves_blocked(&moves, 0.0, &[0.4, 0.8, 1.3]),
            "a 1.4 m body must fit a 2 m doorway"
        );
        // 2.4 m tall body: the roof ray meets the lintel.
        assert!(
            level.moves_blocked(&moves, 0.0, &[0.4, 1.2, 2.3]),
            "a 2.4 m body must be stopped by the 2 m lintel"
        );
        // Off to the side, any height meets the wall.
        let side = [(vec3f(2.5, 0.0, -2.0), vec3f(2.5, 0.0, 2.0))];
        assert!(level.moves_blocked(&side, 0.0, &[0.4, 1.2]), "the wall blocks");
    }
}
