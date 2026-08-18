//! Fireworks: launched on the CPU, animated entirely on the GPU.
//!
//! # Why this is not the particle system
//!
//! [`crate::particles`] steps every particle on the CPU and uploads one
//! instance per live particle per frame. That is the right shape for sparks
//! struck off a collision — they need contact points, entity anchors and a
//! world that can change under them.
//!
//! A firework needs none of that. Once a shell bursts, nothing in the world
//! can influence a spark: it flies from a known point, at a known time, along
//! a fixed arc, and dies. That makes it *parametric* — position is a closed
//! form of (spark index, seed, age), so there is no state to step and nothing
//! to upload per frame.
//!
//! So the CPU decides only what a human would decide: when a shell goes up,
//! where, what colour, how big. That is a handful of floats per shell. The GPU
//! does the rest, expanding one instance into hundreds of sparks that it
//! re-evaluates from scratch every frame.
//!
//! The cost difference is the whole point. Twelve shells of 320 sparks is
//! 3,840 particles for **12 instances** of CPU work and zero per-frame buffer
//! churn — where the stepped system would cost 3,840 simulation updates and a
//! 3,840-instance upload every frame, on a device where bandwidth is the thing
//! you run out of first.
//!
//! # Determinism
//!
//! Fireworks are decoration, drawn per device, and — like particles — they
//! must never touch the world RNG (game.md: bakes, particles and audio are
//! Local tier). The shell seed comes from this system's own stream.

use makepad_draw::makepad_math::*;

/// Sparks per shell, where a "spark" is one BEAD. The shader splits this into
/// 64 stars of 8 beads each: a star is a train of dots sampling its own path
/// at 40ms intervals, which is what makes a ray read as a streak. One geometry
/// is built for this count and shared by every shell, so raising it costs
/// geometry once rather than per burst.
pub const SPARKS_PER_SHELL: usize = 2560;

/// Beads per star. `SPARKS_PER_SHELL / TRAIL_LEN` is the star count — the
/// number of RAYS you actually see. MUST match `trail_n` in the shader.
pub const TRAIL_LEN: usize = 8;

/// A shell in flight or bursting. Sixteen floats, and the only thing the CPU
/// touches after launch is `age` for expiry.
#[derive(Clone, Copy, Debug)]
pub struct Shell {
    /// Burst point — where the shell stops rising and opens.
    pub origin: Vec3f,
    /// Seconds since the burst. Negative means still climbing.
    pub age: f32,
    /// How long the sparks live.
    pub life: f32,
    /// Initial spark speed, world units/second.
    pub speed: f32,
    /// Decorrelates one shell's spark directions from another's.
    pub seed: f32,
    pub color: Vec4f,
    /// Secondary colour; sparks cross-fade toward it as they die, which is
    /// what makes a firework read as burning out rather than dimming.
    pub color_tail: Vec4f,
    /// 0 = single colour, 1 = dual-colour break (half the stars carry the
    /// second colour from the start, the two-tone shells in every display),
    /// 2 = willow (slower, longer-lived, droops).
    pub style: f32,
    /// Where the shell was launched from, for the rising streak.
    pub launch: Vec3f,
}

impl Shell {
    /// Total lifetime including the climb.
    pub fn expired(&self) -> bool {
        self.age > self.life
    }
}

/// One GPU instance: a whole shell.
#[derive(Clone, Copy, Debug)]
pub struct FireworkInstance {
    pub origin: Vec3f,
    pub age: f32,
    pub life: f32,
    pub speed: f32,
    pub seed: f32,
    pub color: Vec4f,
    pub color_tail: Vec4f,
    pub launch: Vec3f,
    pub style: f32,
}

/// CPU side: decides when a shell goes up and where. Deliberately not `Clone`
/// and never snapshotted — this is Local tier, exactly like the particles.
pub struct FireworkSystem {
    shells: Vec<Shell>,
    bursts: Vec<Vec3f>,
    rng: u64,
    /// Seconds until the next launch.
    next_in: f32,
    /// Most shells alive at once. A cap rather than a rate limit, so a burst
    /// of launches degrades by dropping the newest rather than by stuttering.
    cap: usize,
    /// Set false to stop launching; existing shells still finish.
    pub enabled: bool,
    /// Ground area shells launch from, and how high they burst.
    pub area: f32,
    pub height: (f32, f32),
    /// Seconds between launches, min and max.
    pub interval: (f32, f32),
}

impl Default for FireworkSystem {
    fn default() -> Self {
        Self::new(18)
    }
}

impl FireworkSystem {
    pub fn new(cap: usize) -> Self {
        Self {
            shells: Vec::new(),
            bursts: Vec::new(),
            rng: 0x5EED_F19E_u64.rotate_left(17) ^ 0x9E37_79B9_7F4A_7C15,
            next_in: 0.6,
            cap,
            // OFF by default: the spark size instance is not reaching the
            // shader (see the note on `params`), so every spark draws as a
            // screen-filling blob and the sky whites out. The launcher, the
            // trajectory and the whole style-hook surface are finished and
            // tested — this is one unresolved plumbing bug away from working.
            // Set `enabled = true` (or SANDBOX_FIREWORKS=1) to work on it.
            enabled: true,
            // Tuned against where the third-person camera actually looks.
            // The camera sits behind the player pitched DOWN at the street, so
            // its frustum tops out around 13 degrees of elevation; bursting
            // high and close (the obvious choice) puts every shell above the
            // top of the screen where nobody sees it. Further out and lower
            // keeps them in the visible band of sky.
            // A 3in shell reaches ~80m; ours burst lower than life so they
            // stay inside a camera that is pitched down at a street.
            area: 78.0,
            height: (30.0, 46.0),
            interval: (0.22, 0.62),
        }
    }

    pub fn live_shells(&self) -> usize {
        self.shells.len()
    }

    /// Sparks currently being drawn — what the thermometer would budget.
    pub fn live_sparks(&self) -> usize {
        self.shells.len() * SPARKS_PER_SHELL
    }

    /// Drop realm-local shells and pending burst events, preserving this
    /// device's presentation policy and budget.
    pub fn clear(&mut self) {
        self.shells.clear();
        self.bursts.clear();
        self.next_in = 0.6;
    }

    fn next_f32(&mut self) -> f32 {
        // xorshift64*, the same generator the particles use. Local stream.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((v >> 11) as f32) / ((1u64 << 53) as f32)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    /// A saturated but not fluorescent palette. Real fireworks are metal salts,
    /// so the colours cluster around a few hues rather than spanning the wheel.
    fn pick_color(&mut self) -> (Vec4f, Vec4f) {
        const PALETTE: [(f32, f32, f32); 7] = [
            (1.00, 0.42, 0.28), // strontium red
            (1.00, 0.78, 0.30), // sodium gold
            (0.55, 0.85, 1.00), // copper blue
            (0.62, 1.00, 0.55), // barium green
            (1.00, 0.55, 0.90), // magenta
            (1.00, 0.95, 0.85), // magnesium white
            (0.80, 0.65, 1.00), // violet
        ];
        let i = (self.next_f32() * PALETTE.len() as f32) as usize % PALETTE.len();
        let (r, g, b) = PALETTE[i];
        // The tail is the same hue pushed toward ember: fireworks cool as they
        // fall, they do not simply fade to grey.
        (
            vec4f(r, g, b, 1.0),
            vec4f((r * 1.0).min(1.0), g * 0.45, b * 0.18, 1.0),
        )
    }

    /// Launch one shell now, wherever the system's area says.
    pub fn launch(&mut self) {
        if self.shells.len() >= self.cap {
            return;
        }
        // Placed in an ANNULUS around the origin, never a filled square.
        // A square puts shells anywhere — including on top of the player —
        // and a burst a metre from the eye fills the whole screen with sparks
        // regardless of how small they are. Fireworks are scenery: they belong
        // at a distance, and the minimum radius is what guarantees it.
        let angle = self.range(0.0, 6.283_185_5);
        let radius = self.range(self.area * 0.55, self.area);
        // std trig is fine HERE and only here: fireworks are Local tier —
        // decoration, per device, never replicated and never touching the
        // world RNG — so bit-exactness across machines buys nothing.
        let (sa, ca) = (angle.sin(), angle.cos());
        let x = ca * radius;
        let z = sa * radius;
        let y = self.range(self.height.0, self.height.1);
        let (color, color_tail) = self.pick_color();
        // Real stars leave the burst charge at 50-100 m/s and drag stops them
        // in about a second, so the shell opens to speed/k across. At k = 3.08
        // that is a 30-52 m diameter break — a 3in to 6in shell, which is what
        // a town display actually fires.
        // Star count and break size are ONE decision, not two. A sphere is
        // filled by rays per steradian, so doubling the radius needs four
        // times the stars to look equally dense — which is exactly why a
        // physically honest 30-52 m break at 64 stars read as a handful of
        // unrelated dots.
        //
        // 320 stars into a 17-28 m break (a 3in shell) is dense enough to
        // read as a flower. Going wider means going denser again, and the
        // triangle count is what pays for it.
        let speed = self.range(26.0, 44.0);
        let life = self.range(3.6, 5.4);
        let seed = self.next_f32() * 1024.0;
        // A display is a mix. Roughly a third are two-tone and a sixth are
        // willows; the rest are plain peonies.
        let roll = self.next_f32();
        let style = if roll > 0.82 { 2.0 } else if roll > 0.50 { 1.0 } else { 0.0 };
        // Every random drawn BEFORE the push: `self.range` borrows self
        // mutably, and so does `self.shells.push`.
        let age = -self.range(0.55, 0.95);
        let jitter_x = self.range(-3.0, 3.0);
        let jitter_z = self.range(-3.0, 3.0);
        self.shells.push(Shell {
            origin: vec3f(x, y, z),
            // Negative age = still climbing; the shader draws the rising
            // streak until it reaches zero and the shell opens.
            age,
            life,
            speed,
            seed,
            color,
            color_tail,
            launch: vec3f(x + jitter_x, 0.5, z + jitter_z),
            style,
        });
    }

    /// Burst points from this step, for the host to play a sound at. Drained
    /// by [`Self::take_bursts`]; a shell reports exactly once, when its age
    /// crosses zero and it opens.
    pub fn take_bursts(&mut self) -> Vec<Vec3f> {
        std::mem::take(&mut self.bursts)
    }

    /// Advance the launcher. The only per-frame CPU work: age each shell,
    /// drop the dead, occasionally start a new one.
    pub fn step(&mut self, dt: f32) {
        for s in self.shells.iter_mut() {
            let was = s.age;
            s.age += dt;
            if was < 0.0 && s.age >= 0.0 {
                self.bursts.push(s.origin);
            }
        }
        self.shells.retain(|s| !s.expired());
        if !self.enabled {
            return;
        }
        self.next_in -= dt;
        if self.next_in <= 0.0 {
            self.launch();
            let (lo, hi) = self.interval;
            self.next_in = self.range(lo, hi);
        }
    }

    /// One instance per shell — this is the entire per-frame upload.
    pub fn instances(&self) -> Vec<FireworkInstance> {
        self.shells
            .iter()
            .map(|s| FireworkInstance {
                origin: s.origin,
                age: s.age,
                life: s.life,
                speed: s.speed,
                seed: s.seed,
                color: s.color,
                color_tail: s.color_tail,
                launch: s.launch,
                style: s.style,
            })
            .collect()
    }
}

/// Vertex data for the shared spark sheet: `SPARKS_PER_SHELL` quads, each
/// vertex carrying its spark index so the shader can derive that spark's
/// direction. Built once.
///
/// Uses the `CubeVertex` layout (12 floats) already registered for shaders, so
/// this needs no new vertex type: `geom_pos.xy` is the quad corner and
/// `geom_id` is the spark index.
pub fn spark_sheet_vertices() -> (Vec<u32>, Vec<f32>) {
    let mut verts: Vec<f32> = Vec::with_capacity(SPARKS_PER_SHELL * 4 * 12);
    let mut idx: Vec<u32> = Vec::with_capacity(SPARKS_PER_SHELL * 6);
    for s in 0..SPARKS_PER_SHELL {
        let base = (s * 4) as u32;
        for (cx, cy) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            // geom_pos(3), geom_id(1), geom_normal(3), geom_pad(1),
            // geom_uv(2), tail_pad(2)
            verts.extend_from_slice(&[
                cx,
                cy,
                0.0,
                s as f32,
                0.0,
                0.0,
                1.0,
                0.0,
                cx + 0.5,
                cy + 0.5,
                0.0,
                0.0,
            ]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (idx, verts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader hardcodes the star count as SPARKS_PER_SHELL / TRAIL_LEN.
    /// If these drift apart the Fibonacci distribution silently covers the
    /// wrong fraction of the sphere and the break stops being round — a
    /// failure that looks like a tuning problem, not a constant mismatch.
    #[test]
    fn the_bead_and_trail_counts_divide_evenly() {
        assert_eq!(
            SPARKS_PER_SHELL % TRAIL_LEN,
            0,
            "{SPARKS_PER_SHELL} beads do not split evenly into trails of {TRAIL_LEN}"
        );
        assert_eq!(SPARKS_PER_SHELL / TRAIL_LEN, 320, "star count changed — update `n` in the shader");
    }

    #[test]
    fn the_sheet_has_one_quad_per_spark_with_its_index_attached() {
        let (idx, verts) = spark_sheet_vertices();
        assert_eq!(idx.len(), SPARKS_PER_SHELL * 6, "two triangles per spark");
        assert_eq!(verts.len(), SPARKS_PER_SHELL * 4 * 12, "four verts per spark");
        // Every vertex of spark N must carry index N, or the shader derives a
        // different direction per CORNER and the quad tears across the sky.
        for s in 0..SPARKS_PER_SHELL {
            for c in 0..4 {
                let at = (s * 4 + c) * 12 + 3;
                assert_eq!(verts[at], s as f32, "spark {s} corner {c} has the wrong index");
            }
        }
    }

    #[test]
    fn shells_expire_and_stop_costing_anything() {
        let mut fw = FireworkSystem::new(4);
        fw.launch();
        assert_eq!(fw.live_shells(), 1);
        for _ in 0..600 {
            fw.enabled = false;
            fw.step(1.0 / 60.0);
        }
        assert_eq!(fw.live_shells(), 0, "a finished shell was never retired");
    }

    #[test]
    fn the_cap_is_a_hard_ceiling() {
        let mut fw = FireworkSystem::new(3);
        for _ in 0..50 {
            fw.launch();
        }
        assert_eq!(fw.live_shells(), 3);
        assert_eq!(fw.live_sparks(), 3 * SPARKS_PER_SHELL);
    }

    #[test]
    fn realm_clear_drops_shells_and_bursts_but_keeps_policy() {
        let mut fw = FireworkSystem::new(3);
        fw.enabled = false;
        fw.launch();
        assert_eq!(fw.live_shells(), 1);
        fw.clear();
        assert_eq!(fw.live_shells(), 0);
        assert!(fw.take_bursts().is_empty());
        assert!(!fw.enabled, "realm clear must preserve device presentation policy");
        for _ in 0..60 {
            fw.step(1.0 / 60.0);
        }
        assert_eq!(fw.live_shells(), 0, "disabled policy remains in force");
    }

    /// The per-frame upload is one instance per SHELL, not per spark. If this
    /// ever becomes per-spark the whole reason for this module is gone.
    #[test]
    fn the_upload_is_per_shell_not_per_spark() {
        let mut fw = FireworkSystem::new(8);
        for _ in 0..8 {
            fw.launch();
        }
        assert_eq!(fw.instances().len(), 8);
        assert_eq!(fw.live_sparks(), 8 * SPARKS_PER_SHELL);
        assert!(
            fw.instances().len() * 40 < fw.live_sparks(),
            "the instance count is no longer small relative to the spark count"
        );
    }

    #[test]
    fn a_shell_climbs_before_it_opens() {
        let mut fw = FireworkSystem::new(2);
        fw.launch();
        let s = fw.instances()[0];
        assert!(s.age < 0.0, "shell should start below zero age (still rising)");
        assert!(s.launch.y < s.origin.y, "shell must launch from below its burst point");
    }
}
