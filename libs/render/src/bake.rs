//! CPU light baker (game.md §Rendering: single pass, Quest-affordable).
//!
//! The platform cannot sample a pass depth texture, so there are no shadow
//! maps and no post-process to hide behind. What is left is arithmetic the
//! CPU can do once and the GPU never repeats: bake occlusion into the data
//! we already upload.
//!
//! Three products, all folded into colours the renderer was sending anyway,
//! so **none of this costs a byte of bandwidth or a cycle of GPU time**:
//!
//! * `ao` — per static entity, how enclosed it is. Sun-independent, so it is
//!   baked once per `render_rev` and survives a moving sun.
//! * `sun_vis` — per static entity, whether the sun reaches it. One ray each,
//!   so it can be rebaked whenever the sun swings without a hitch.
//! * [`ProbeGrid`] — a coarse lattice of the same two numbers, trilinearly
//!   sampled by *dynamic* objects each frame on the CPU. A crate rolling under
//!   a bridge darkens; the shader never learns why.
//!
//! **RNG isolation** is structural, exactly as it is for particles: the bake
//! lives in the renderer, `GameWorld` has no field for it, and the ray
//! directions come from a fixed Fibonacci set rather than any random source —
//! so there is no RNG here to share with the simulation in the first place.

use makepad_draw::*;
use makepad_game_sim::{BodyKind, Entity, GameWorld, Terrain};

use crate::sun::SunLight;

/// An axis-aligned occluder. Boxes never rotate their collision volume (the
/// visual-only yaw convention), so the bake sees the same shape the physics
/// does.
#[derive(Clone, Copy)]
struct Occluder {
    min: Vec3f,
    max: Vec3f,
}

impl Occluder {
    fn from_entity(e: &Entity) -> Self {
        let h = vec3f(
            e.half.x * e.scale.x,
            e.half.y * e.scale.y,
            e.half.z * e.scale.z,
        );
        Self {
            min: e.pos - h,
            max: e.pos + h,
        }
    }

    /// Slab test. `inv_dir` is precomputed per ray; infinities are fine and
    /// are what make an axis-parallel ray fall out correctly.
    fn hit(&self, origin: Vec3f, inv_dir: Vec3f, t_max: f32) -> bool {
        let mut t0 = 0.0f32;
        let mut t1 = t_max;
        let lo = [
            (self.min.x - origin.x) * inv_dir.x,
            (self.min.y - origin.y) * inv_dir.y,
            (self.min.z - origin.z) * inv_dir.z,
        ];
        let hi = [
            (self.max.x - origin.x) * inv_dir.x,
            (self.max.y - origin.y) * inv_dir.y,
            (self.max.z - origin.z) * inv_dir.z,
        ];
        for axis in 0..3 {
            let (a, b) = (lo[axis], hi[axis]);
            let (near, far) = if a <= b { (a, b) } else { (b, a) };
            if near.is_nan() || far.is_nan() {
                continue;
            }
            t0 = t0.max(near);
            t1 = t1.min(far);
            if t0 > t1 {
                return false;
            }
        }
        true
    }

    fn center(&self) -> Vec3f {
        (self.min + self.max) * 0.5
    }

    /// Radius of the bounding sphere — the cheap cull before the slab test.
    fn radius(&self) -> f32 {
        ((self.max - self.min) * 0.5).length()
    }
}

/// How the bake spends its time. Every field is a hard bound: a pathological
/// world makes the bake coarser, never slower without limit.
#[derive(Clone, Copy, Debug)]
pub struct BakeSettings {
    /// Hemisphere rays per sample point. 8 is enough for a scalar term.
    pub ao_rays: usize,
    /// How far a surface looks for occluders. Short: this is contact
    /// darkening, not global illumination.
    pub ao_radius: f32,
    /// How dark full occlusion goes (0 = bake disabled, 1 = black).
    pub ao_strength: f32,
    /// How dark a static in the sun's shadow goes.
    pub shadow_strength: f32,
    /// Target spacing of the probe lattice, world units.
    pub probe_spacing: f32,
    /// Hard cap on probes; the lattice coarsens to fit.
    pub max_probes: usize,
    /// Occluders considered at all. Beyond this the bake degrades to
    /// "nearest N" rather than growing without bound.
    pub max_occluders: usize,
    /// Rebake the sun term when the sun has swung past this angle (radians).
    pub sun_rebake_angle: f32,
}

impl Default for BakeSettings {
    fn default() -> Self {
        Self {
            ao_rays: 8,
            ao_radius: 4.0,
            ao_strength: 0.35,
            shadow_strength: 0.35,
            probe_spacing: 4.0,
            max_probes: 1024,
            max_occluders: 1024,
            sun_rebake_angle: 0.03,
        }
    }
}

/// What one bake cost, for BUDGETS.md and the host profiler.
#[derive(Default, Clone, Copy, Debug)]
pub struct BakeStats {
    pub ao_us: u64,
    pub sun_us: u64,
    pub probe_us: u64,
    pub occluders: u64,
    pub probes: u64,
    pub statics: u64,
    /// Full (AO + probes) rebakes since start — should track world edits.
    pub full_bakes: u64,
    /// Sun-only rebakes — cheap, tracks the sun moving.
    pub sun_bakes: u64,
}

/// Trilinearly sampled lattice of baked lighting, used by things that move.
#[derive(Default)]
pub struct ProbeGrid {
    origin: Vec3f,
    spacing: f32,
    dims: (usize, usize, usize),
    /// `(sky_visibility, sun_visibility)` per probe, x-major.
    probes: Vec<(f32, f32)>,
}

impl ProbeGrid {
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims.1 + y) * self.dims.0 + x
    }

    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }

    /// Sample `(sky_visibility, sun_visibility)` at a world point. Outside
    /// the lattice both read fully lit, so a game with no baked volume looks
    /// exactly as it did before the baker existed.
    pub fn sample(&self, p: Vec3f) -> (f32, f32) {
        if self.probes.is_empty() {
            return (1.0, 1.0);
        }
        let f = (p - self.origin) * (1.0 / self.spacing);
        let clampi = |v: f32, n: usize| -> (usize, usize, f32) {
            if n <= 1 {
                return (0, 0, 0.0);
            }
            let hi_bound = (n - 1) as f32;
            let v = v.clamp(0.0, hi_bound);
            let i0 = v.floor() as usize;
            let i0 = i0.min(n - 2);
            (i0, i0 + 1, v - i0 as f32)
        };
        let (x0, x1, fx) = clampi(f.x, self.dims.0);
        let (y0, y1, fy) = clampi(f.y, self.dims.1);
        let (z0, z1, fz) = clampi(f.z, self.dims.2);
        let mut sky = 0.0;
        let mut sun = 0.0;
        for (zi, wz) in [(z0, 1.0 - fz), (z1, fz)] {
            for (yi, wy) in [(y0, 1.0 - fy), (y1, fy)] {
                for (xi, wx) in [(x0, 1.0 - fx), (x1, fx)] {
                    let w = wx * wy * wz;
                    if w <= 0.0 {
                        continue;
                    }
                    let p = self.probes[self.index(xi, yi, zi)];
                    sky += p.0 * w;
                    sun += p.1 * w;
                }
            }
        }
        (sky, sun)
    }
}

/// The baker. Owned by the renderer, never by the world.
pub struct LightBake {
    settings: BakeSettings,
    /// Per static entity, keyed by id and kept in the entity list's own
    /// ascending-id order so lookups are a binary search.
    ao: Vec<(u64, f32)>,
    sun_vis: Vec<(u64, f32)>,
    probes: ProbeGrid,
    occluders: Vec<Occluder>,
    /// Highest point of the heightfield, so a ray above it going up can skip
    /// the march entirely (see [`Self::occluded`]).
    terrain_max: f32,
    /// World revision the geometry-dependent half was baked against.
    baked_rev: Option<u64>,
    /// Sun direction the sun-dependent half was baked against.
    baked_sun: Option<Vec3f>,
    /// Bumped on every rebake; the renderer folds it into its slab key so a
    /// rebake reaches the packed static instances.
    generation: u64,
    stats: BakeStats,
}

impl Default for LightBake {
    fn default() -> Self {
        Self {
            settings: BakeSettings::default(),
            ao: Vec::new(),
            sun_vis: Vec::new(),
            probes: ProbeGrid::default(),
            occluders: Vec::new(),
            terrain_max: f32::MIN,
            baked_rev: None,
            baked_sun: None,
            generation: 0,
            stats: BakeStats::default(),
        }
    }
}

/// Fixed hemisphere directions around +y, generated by the Fibonacci spiral.
/// Deterministic and RNG-free by construction — see the module docs.
fn hemisphere_dirs(n: usize) -> Vec<Vec3f> {
    let mut out = Vec::with_capacity(n);
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    for i in 0..n {
        // Cosine-ish distribution biased away from the horizon, which is
        // where a short-radius AO ray is most likely to hit a neighbour.
        let y = 1.0 - (i as f32 + 0.5) / n as f32 * 0.85;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * i as f32;
        out.push(vec3f(theta.cos() * r, y, theta.sin() * r));
    }
    out
}

/// Rotate a +y hemisphere direction onto `normal`.
fn orient(dir: Vec3f, normal: Vec3f) -> Vec3f {
    let up = if normal.y.abs() > 0.99 {
        vec3f(1.0, 0.0, 0.0)
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    let t = Vec3f::cross(up, normal).normalize();
    let b = Vec3f::cross(normal, t);
    (t * dir.x + normal * dir.y + b * dir.z).normalize()
}

impl LightBake {
    /// Forget every result derived from the current world while retaining
    /// this device's bake policy. A newly loaded realm may start its
    /// `render_rev` at the same value as the previous realm, so invalidating
    /// only by revision is not sufficient at that lifecycle boundary.
    pub(crate) fn enter_realm(&mut self) {
        self.ao.clear();
        self.sun_vis.clear();
        self.probes = ProbeGrid::default();
        self.occluders.clear();
        self.terrain_max = f32::MIN;
        self.baked_rev = None;
        self.baked_sun = None;
        // Slabs include this generation in their cache key. The renderer
        // also drops its slab key, but keeping the generation monotonic
        // makes the invalidation robust for any future consumer.
        self.generation = self.generation.wrapping_add(1);
        // Per-result gauges must not describe the departed realm. Keep the
        // lifetime bake counters truthful: their contract is "since start".
        self.stats.ao_us = 0;
        self.stats.sun_us = 0;
        self.stats.probe_us = 0;
        self.stats.occluders = 0;
        self.stats.probes = 0;
        self.stats.statics = 0;
    }

    pub fn settings(&self) -> BakeSettings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: BakeSettings) {
        self.settings = settings;
        // Force both halves to rebake: strengths and radii change the answer.
        self.baked_rev = None;
        self.baked_sun = None;
    }

    pub fn stats(&self) -> BakeStats {
        self.stats
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn probes(&self) -> &ProbeGrid {
        &self.probes
    }

    /// Baked shade for a static entity: **ambient occlusion only**.
    ///
    /// Cast shadows deliberately do NOT go through here. A per-entity sun
    /// term is one bit for a whole object, so a single ray from the middle of
    /// a floor slab hitting one pillar darkens the entire floor — which is
    /// exactly what it did before this was split out. Statics get their cast
    /// shadows as real silhouette meshes (shadow_mesh.rs) instead, which land
    /// where the shadow actually falls. AO stays because it is a different
    /// thing: contact darkening in crevices, and it is genuinely per-object.
    pub fn static_shade(&self, id: u64) -> f32 {
        let ao = lookup(&self.ao, id).unwrap_or(1.0);
        1.0 - (1.0 - ao.clamp(0.0, 1.0)) * self.settings.ao_strength
    }

    /// Shade for a point that moves, from the probe lattice.
    pub fn dynamic_shade(&self, p: Vec3f) -> f32 {
        if self.probes.is_empty() {
            return 1.0;
        }
        let (sky, sun) = self.probes.sample(p);
        self.combine(sky, sun)
    }

    /// Visibility terms → one multiplier. Both are "how much light reaches
    /// here", so both darken toward their configured strength.
    fn combine(&self, sky_vis: f32, sun_vis: f32) -> f32 {
        let ao = 1.0 - (1.0 - sky_vis.clamp(0.0, 1.0)) * self.settings.ao_strength;
        let sh = 1.0 - (1.0 - sun_vis.clamp(0.0, 1.0)) * self.settings.shadow_strength;
        (ao * sh).clamp(0.0, 1.0)
    }

    /// Refresh whatever is stale. Returns true when anything changed, which
    /// is the renderer's cue that its static slabs need repacking.
    pub fn update(&mut self, world: &GameWorld, sun: &SunLight) -> bool {
        let geometry_stale = self.baked_rev != Some(world.render_rev);
        let sun_stale = match self.baked_sun {
            None => true,
            Some(d) => d.dot(sun.dir) < self.settings.sun_rebake_angle.cos(),
        };
        if !geometry_stale && !sun_stale {
            return false;
        }
        if geometry_stale {
            self.collect_occluders(world);
            self.bake_ao(world);
            self.bake_probes_sky(world);
            self.baked_rev = Some(world.render_rev);
            self.stats.full_bakes += 1;
        }
        // The sun half is one ray per target, so it rides along with a
        // geometry rebake and also stands alone when only the sun moved.
        self.bake_sun(world, sun);
        self.baked_sun = Some(sun.dir);
        self.stats.sun_bakes += 1;
        self.generation += 1;
        true
    }

    /// Everything that can block light: statics and kinematics (a moving
    /// platform still casts), skipping sensors and decoration.
    fn collect_occluders(&mut self, world: &GameWorld) {
        self.occluders.clear();
        self.terrain_max = world
            .terrain
            .as_ref()
            .map(|t| t.heights.iter().copied().fold(f32::MIN, f32::max))
            .unwrap_or(f32::MIN);
        for e in world.entities.iter() {
            if e.sensor || e.bake_skip || !matches!(e.kind, BodyKind::Static | BodyKind::Kinematic) {
                continue;
            }
            if self.occluders.len() >= self.settings.max_occluders {
                break;
            }
            self.occluders.push(Occluder::from_entity(e));
        }
        self.stats.occluders = self.occluders.len() as u64;
    }

    /// Occluders whose bounding sphere could be reached from `origin` within
    /// `radius`. Cuts the inner loop from "every static" to "the neighbours",
    /// which is what keeps the bake in microseconds rather than milliseconds.
    fn gather_near(&self, origin: Vec3f, radius: f32, out: &mut Vec<Occluder>) {
        out.clear();
        for o in self.occluders.iter() {
            let reach = radius + o.radius();
            if (o.center() - origin).length_squared() <= reach * reach {
                out.push(*o);
            }
        }
    }

    /// Is anything between `origin` and `origin + dir * t_max`? Boxes first
    /// (cheap and exact), then the terrain (a bounded march).
    ///
    /// `terrain_max` is the heightfield's highest point. A ray that starts
    /// above it and travels level or upward can never reach it, which skips
    /// the march for the overwhelming majority of rays — the difference
    /// between a 5 ms bake and a 0.3 ms one, since the march is the only part
    /// that samples per step.
    fn occluded(
        near: &[Occluder],
        terrain: Option<&Terrain>,
        terrain_max: f32,
        origin: Vec3f,
        dir: Vec3f,
        t_max: f32,
    ) -> bool {
        let inv = vec3f(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        for o in near {
            if o.hit(origin, inv, t_max) {
                return true;
            }
        }
        let Some(t) = terrain else { return false };
        if origin.y >= terrain_max && dir.y >= 0.0 {
            return false;
        }
        // A heightfield has no analytic ray test; step it. Step count scales
        // with the ray so a 4-unit AO probe does not pay a 64-unit sun ray's
        // sampling.
        let steps = ((t_max * 0.5).ceil() as usize).clamp(3, 12);
        for i in 1..=steps {
            let s = t_max * i as f32 / steps as f32;
            let p = origin + dir * s;
            if p.y >= terrain_max && dir.y >= 0.0 {
                return false;
            }
            if let Some(h) = t.height_at(p.x, p.z) {
                if p.y < h {
                    return true;
                }
            }
        }
        false
    }

    /// Sky visibility for one surface point, as a fraction of unoccluded
    /// hemisphere rays.
    fn sky_visibility(
        &self,
        near: &[Occluder],
        terrain: Option<&Terrain>,
        origin: Vec3f,
        normal: Vec3f,
        dirs: &[Vec3f],
    ) -> f32 {
        let mut open = 0usize;
        for d in dirs {
            let ray = orient(*d, normal);
            if !Self::occluded(near, terrain, self.terrain_max, origin, ray, self.settings.ao_radius) {
                open += 1;
            }
        }
        open as f32 / dirs.len().max(1) as f32
    }

    /// Per-static ambient occlusion. Sampled from five face centres so a box
    /// wedged against a wall darkens even though its top is open — a single
    /// centre sample cannot tell those apart.
    fn bake_ao(&mut self, world: &GameWorld) {
        let t0 = std::time::Instant::now();
        let dirs = hemisphere_dirs(self.settings.ao_rays);
        let terrain = world.terrain.as_ref();
        let mut near = Vec::new();
        self.ao.clear();
        let mut statics = 0u64;
        for e in world.entities.iter() {
            if e.kind != BodyKind::Static || e.sensor || e.bake_skip {
                continue;
            }
            statics += 1;
            let h = vec3f(
                e.half.x * e.scale.x,
                e.half.y * e.scale.y,
                e.half.z * e.scale.z,
            );
            // Nudge off the surface so the entity's own box does not
            // self-occlude every ray.
            let eps = 0.02;
            let faces = [
                (vec3f(0.0, h.y + eps, 0.0), vec3f(0.0, 1.0, 0.0)),
                (vec3f(h.x + eps, 0.0, 0.0), vec3f(1.0, 0.0, 0.0)),
                (vec3f(-h.x - eps, 0.0, 0.0), vec3f(-1.0, 0.0, 0.0)),
                (vec3f(0.0, 0.0, h.z + eps), vec3f(0.0, 0.0, 1.0)),
                (vec3f(0.0, 0.0, -h.z - eps), vec3f(0.0, 0.0, -1.0)),
            ];
            self.gather_near(e.pos, self.settings.ao_radius + h.length(), &mut near);
            // Its own box would occlude every ray from its own surface.
            let self_box = Occluder::from_entity(e);
            near.retain(|o| {
                (o.center() - self_box.center()).length_squared() > 1.0e-6
                    || (o.max - self_box.max).length_squared() > 1.0e-6
            });
            let mut total = 0.0;
            for (offset, normal) in faces {
                total += self.sky_visibility(&near, terrain, e.pos + offset, normal, &dirs);
            }
            self.ao.push((e.id, total / faces.len() as f32));
        }
        self.stats.statics = statics;
        self.stats.ao_us = t0.elapsed().as_micros() as u64;
    }

    /// Per-static sun visibility: one ray each, toward the sun. Cheap enough
    /// to redo whenever the sun swings, which is what lets a day cycle move
    /// the baked shadows instead of freezing them at dawn.
    fn bake_sun(&mut self, world: &GameWorld, sun: &SunLight) {
        let t0 = std::time::Instant::now();
        let terrain = world.terrain.as_ref();
        // A long ray: a tower should shadow the ground well away from it.
        let reach = 64.0;
        let mut near = Vec::new();
        self.sun_vis.clear();
        for e in world.entities.iter() {
            if e.kind != BodyKind::Static || e.sensor || e.bake_skip {
                continue;
            }
            let h = vec3f(
                e.half.x * e.scale.x,
                e.half.y * e.scale.y,
                e.half.z * e.scale.z,
            );
            let origin = e.pos + vec3f(0.0, h.y + 0.02, 0.0);
            self.gather_near(origin, reach, &mut near);
            let self_box = Occluder::from_entity(e);
            near.retain(|o| {
                (o.center() - self_box.center()).length_squared() > 1.0e-6
                    || (o.max - self_box.max).length_squared() > 1.0e-6
            });
            let lit = !Self::occluded(&near, terrain, self.terrain_max, origin, sun.dir, reach);
            self.sun_vis.push((e.id, if lit { 1.0 } else { 0.0 }));
        }
        // Probes carry the same term for anything that moves.
        let positions: Vec<Vec3f> = self.probe_positions().collect();
        for (i, probe) in positions.into_iter().enumerate() {
            self.gather_near(probe, reach, &mut near);
            let lit = !Self::occluded(&near, terrain, self.terrain_max, probe, sun.dir, reach);
            self.probes.probes[i].1 = if lit { 1.0 } else { 0.0 };
        }
        self.stats.sun_us = t0.elapsed().as_micros() as u64;
    }

    fn probe_positions(&self) -> impl Iterator<Item = Vec3f> + '_ {
        let (nx, ny, nz) = self.probes.dims;
        let origin = self.probes.origin;
        let spacing = self.probes.spacing;
        (0..nx * ny * nz).map(move |i| {
            let x = i % nx;
            let y = (i / nx) % ny;
            let z = i / (nx * ny);
            origin + vec3f(x as f32 * spacing, y as f32 * spacing, z as f32 * spacing)
        })
    }

    /// Lay out the lattice over the world's contents and bake its sky term.
    /// Sun visibility is filled in by [`Self::bake_sun`].
    fn bake_probes_sky(&mut self, world: &GameWorld) {
        let t0 = std::time::Instant::now();
        let Some((min, max)) = world_bounds(world) else {
            self.probes = ProbeGrid::default();
            self.stats.probe_us = 0;
            self.stats.probes = 0;
            return;
        };
        let mut spacing = self.settings.probe_spacing.max(0.5);
        let span = max - min;
        // Coarsen until the lattice fits the cap: a huge world gets a
        // blurrier bake, never a slower one.
        let dims_for = |spacing: f32| {
            let n = |extent: f32| ((extent / spacing).ceil() as usize + 1).max(2);
            (n(span.x), n(span.y.max(spacing)), n(span.z))
        };
        let mut dims = dims_for(spacing);
        while dims.0 * dims.1 * dims.2 > self.settings.max_probes {
            spacing *= 1.5;
            dims = dims_for(spacing);
        }
        self.probes = ProbeGrid {
            origin: min,
            spacing,
            dims,
            probes: vec![(1.0, 1.0); dims.0 * dims.1 * dims.2],
        };
        let dirs = hemisphere_dirs(self.settings.ao_rays);
        let terrain = world.terrain.as_ref();
        let mut near = Vec::new();
        let positions: Vec<Vec3f> = self.probe_positions().collect();
        for (i, p) in positions.into_iter().enumerate() {
            self.gather_near(p, self.settings.ao_radius, &mut near);
            // A probe floats in space: sample the full upper hemisphere.
            let vis = self.sky_visibility(&near, terrain, p, vec3f(0.0, 1.0, 0.0), &dirs);
            self.probes.probes[i].0 = vis;
        }
        self.stats.probes = self.probes.probes.len() as u64;
        self.stats.probe_us = t0.elapsed().as_micros() as u64;
    }
}

/// Binary search over an ascending-id table (entity ids are spawn-ordered,
/// so the bake tables inherit that order for free).
fn lookup(table: &[(u64, f32)], id: u64) -> Option<f32> {
    table
        .binary_search_by_key(&id, |(k, _)| *k)
        .ok()
        .map(|i| table[i].1)
}

/// World-space box the probe lattice should cover: everything solid, plus
/// enough headroom above it for things to move through.
fn world_bounds(world: &GameWorld) -> Option<(Vec3f, Vec3f)> {
    let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
    let mut any = false;
    for e in world.entities.iter() {
        if e.sensor || e.bake_skip {
            continue;
        }
        let h = vec3f(
            e.half.x * e.scale.x,
            e.half.y * e.scale.y,
            e.half.z * e.scale.z,
        );
        min = vec3f(
            min.x.min(e.pos.x - h.x),
            min.y.min(e.pos.y - h.y),
            min.z.min(e.pos.z - h.z),
        );
        max = vec3f(
            max.x.max(e.pos.x + h.x),
            max.y.max(e.pos.y + h.y),
            max.z.max(e.pos.z + h.z),
        );
        any = true;
    }
    if !any {
        return None;
    }
    // Headroom so a jumping character stays inside the lattice.
    max.y += 4.0;
    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_game_sim::{BodyKind, Entity, GameWorld};

    /// NB `Entity::default()` leaves `scale` at (0,0,0) — the same
    /// not-a-playable-default trap that has already bitten `rng`,
    /// `gravity_scale` and ground friction. A zero-scale entity draws at zero
    /// size, so the bake agreeing with the renderer and ignoring it is
    /// correct; a test fixture just has to say what it means.
    fn static_box(id: u64, pos: Vec3f, half: Vec3f) -> Entity {
        Entity {
            id,
            kind: BodyKind::Static,
            pos,
            half,
            scale: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        }
    }

    /// A big flat slab plus whatever the test adds on top of it.
    fn world_with(entities: Vec<Entity>) -> GameWorld {
        let mut world = GameWorld::new();
        world.entities = entities;
        world
    }

    #[test]
    fn a_box_in_the_open_is_brighter_than_one_in_a_pit() {
        // Open box, and one ringed by four walls.
        let mut entities = vec![
            static_box(1, vec3f(0.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5)),
            static_box(2, vec3f(40.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5)),
        ];
        for (i, (dx, dz)) in [(2.0, 0.0), (-2.0, 0.0), (0.0, 2.0), (0.0, -2.0)]
            .into_iter()
            .enumerate()
        {
            entities.push(static_box(
                10 + i as u64,
                vec3f(40.0 + dx, 2.0, dz),
                vec3f(1.0, 2.0, 1.0),
            ));
        }
        entities.sort_by_key(|e| e.id);
        let world = world_with(entities);
        let mut bake = LightBake::default();
        bake.update(&world, &SunLight::default());
        let open = lookup(&bake.ao, 1).unwrap();
        let enclosed = lookup(&bake.ao, 2).unwrap();
        assert!(
            enclosed < open - 0.05,
            "enclosed box should be darker: open {open}, enclosed {enclosed}"
        );
        assert!(open > 0.7, "an isolated box should be mostly open: {open}");
    }

    #[test]
    fn a_wall_shadows_the_ground_behind_it_and_not_beside_it() {
        // Sun straight up-and-east; the shadow falls to -x of the wall.
        let mut sun = SunLight::default();
        sun.dir = vec3f(1.0, 1.0, 0.0).normalize();
        let entities = vec![
            // The wall.
            static_box(1, vec3f(0.0, 3.0, 0.0), vec3f(0.5, 3.0, 6.0)),
            // Ground slab in the wall's shadow (sun-side is +x, so -x is dark).
            static_box(2, vec3f(-2.0, 0.1, 0.0), vec3f(1.0, 0.1, 1.0)),
            // Ground slab out to the side, far along z, unshadowed.
            static_box(3, vec3f(-2.0, 0.1, 20.0), vec3f(1.0, 0.1, 1.0)),
        ];
        let world = world_with(entities);
        let mut bake = LightBake::default();
        bake.update(&world, &sun);
        assert_eq!(lookup(&bake.sun_vis, 2), Some(0.0), "slab behind the wall must be shadowed");
        assert_eq!(lookup(&bake.sun_vis, 3), Some(1.0), "slab beside the wall must be lit");
        assert!(bake.static_shade(2) < bake.static_shade(3));
    }

    #[test]
    fn a_probe_under_an_overhang_is_darker_than_one_in_the_open() {
        let entities = vec![
            // A wide roof at y=6 over the origin.
            static_box(1, vec3f(0.0, 6.0, 0.0), vec3f(6.0, 0.3, 6.0)),
            // A far-away marker so the lattice spans open ground too.
            static_box(2, vec3f(40.0, 0.25, 0.0), vec3f(0.5, 0.25, 0.5)),
        ];
        let world = world_with(entities);
        let mut bake = LightBake::default();
        bake.update(&world, &SunLight::default());
        let under = bake.dynamic_shade(vec3f(0.0, 1.0, 0.0));
        let outside = bake.dynamic_shade(vec3f(40.0, 1.0, 0.0));
        assert!(
            under < outside - 0.02,
            "under the roof {under} should be darker than the open {outside}"
        );
    }

    #[test]
    fn an_empty_world_bakes_to_no_change() {
        let world = world_with(Vec::new());
        let mut bake = LightBake::default();
        bake.update(&world, &SunLight::default());
        assert_eq!(bake.static_shade(1), 1.0);
        assert_eq!(bake.dynamic_shade(vec3f(0.0, 0.0, 0.0)), 1.0);
    }

    #[test]
    fn geometry_bakes_once_but_the_sun_can_rebake_alone() {
        let entities = vec![static_box(1, vec3f(0.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5))];
        let world = world_with(entities);
        let mut bake = LightBake::default();
        let sun = SunLight::default();
        assert!(bake.update(&world, &sun));
        let after_first = bake.stats();
        // Same world, same sun: nothing to do.
        assert!(!bake.update(&world, &sun));
        assert_eq!(bake.stats().full_bakes, after_first.full_bakes);
        // Sun swings: the cheap half reruns, the expensive half does not.
        let mut moved = sun;
        moved.dir = vec3f(-0.5, 0.6, 0.2).normalize();
        assert!(bake.update(&world, &moved));
        assert_eq!(bake.stats().full_bakes, after_first.full_bakes);
        assert!(bake.stats().sun_bakes > after_first.sun_bakes);
    }

    /// The rays are a fixed set, so two bakes of the same world agree
    /// exactly — and there is no RNG in this module to share with the sim.
    #[test]
    fn the_bake_is_deterministic() {
        let entities = vec![
            static_box(1, vec3f(0.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5)),
            static_box(2, vec3f(1.2, 0.5, 0.0), vec3f(0.5, 0.5, 0.5)),
        ];
        let world = world_with(entities);
        let sun = SunLight::default();
        let mut a = LightBake::default();
        let mut b = LightBake::default();
        a.update(&world, &sun);
        b.update(&world, &sun);
        assert_eq!(a.ao, b.ao);
        assert_eq!(a.sun_vis, b.sun_vis);
        assert_eq!(a.probes.probes, b.probes.probes);
    }

    #[test]
    fn the_probe_lattice_respects_its_cap() {
        // A world far larger than the default spacing would allow.
        let entities = vec![
            static_box(1, vec3f(-200.0, 0.5, -200.0), vec3f(0.5, 0.5, 0.5)),
            static_box(2, vec3f(200.0, 0.5, 200.0), vec3f(0.5, 0.5, 0.5)),
        ];
        let world = world_with(entities);
        let mut bake = LightBake::default();
        bake.update(&world, &SunLight::default());
        let s = bake.settings();
        assert!(
            bake.stats().probes <= s.max_probes as u64,
            "probe count {} exceeded the cap {}",
            bake.stats().probes,
            s.max_probes
        );
    }

    #[test]
    fn entering_a_realm_drops_world_results_but_preserves_bake_policy() {
        let world = world_with(vec![static_box(
            1,
            vec3f(0.0, 0.5, 0.0),
            vec3f(0.5, 0.5, 0.5),
        )]);
        let mut bake = LightBake::default();
        let mut settings = bake.settings();
        settings.ao_rays = 3;
        settings.max_probes = 17;
        bake.set_settings(settings);
        assert!(bake.update(&world, &SunLight::default()));
        assert!(!bake.ao.is_empty());
        assert!(!bake.occluders.is_empty());
        let generation = bake.generation();
        let lifetime_stats = bake.stats();

        bake.enter_realm();

        assert!(bake.ao.is_empty());
        assert!(bake.sun_vis.is_empty());
        assert!(bake.probes.is_empty());
        assert!(bake.occluders.is_empty());
        assert_eq!(bake.baked_rev, None);
        assert_eq!(bake.baked_sun, None);
        assert_eq!(bake.terrain_max, f32::MIN);
        assert_eq!(bake.generation(), generation.wrapping_add(1));
        assert_eq!(bake.stats().full_bakes, lifetime_stats.full_bakes);
        assert_eq!(bake.stats().sun_bakes, lifetime_stats.sun_bakes);
        assert_eq!(bake.stats().occluders, 0);
        assert_eq!(bake.stats().probes, 0);
        assert_eq!(bake.stats().statics, 0);
        assert_eq!(bake.settings().ao_rays, 3);
        assert_eq!(bake.settings().max_probes, 17);
        assert_eq!(bake.dynamic_shade(Vec3f::default()), 1.0);
    }
}
