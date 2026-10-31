//! GPU safety: what the engine may cost a frame, measured, and the caps it
//! never passes, so no scene, kernel or GPU can hang or flood a machine.
//!
//! - **Only on change.** Nothing here asks for frames: a bake records its
//!   batch once and an empty recording after the last one (`bake::Baker`),
//!   the march draws when its host repaints. An idle view costs nothing.
//! - **Measured, steered.** [`GpuMeter`] reads the platform's frame timer
//!   (`gpu_frame_timer`: per-pass GPU spans from timestamp queries on Metal,
//!   Vulkan, D3D11 and the web) and the bake's share of each frame; the
//!   bricks baked per frame follow the measured cost per brick so the bake
//!   stays inside [`Budget::bake_ms`].
//! - **Degrades, never switches.** The field look stays the field look: a
//!   frame over [`Budget::frame_ms`] turns its own levers down a [`Quality`]
//!   level (march steps, the coarse pass's resolution, exact kernel
//!   evaluation, the bake's share), a frame well under it turns them back up.
//!   The judgement reads settled frames only: none for [`SETTLE`] frames
//!   after a start or a level change (pipelines compile, the bake catches
//!   up), none that folded repaints which never presented (several frames'
//!   work, or a stall), and the bake's own passes left out (they keep their
//!   own budget). Meshes are only ever the person's choice.
//! - **Hard caps** that hold before any measurement: bricks per frame
//!   ([`Budget::max_bricks_per_frame`]), kernel work per frame
//!   ([`Budget::bake_ops`], with a kernel's own worst case per sample
//!   capped by `kernel::MAX_KERNEL_COST`), march steps (256 per ray, in the
//!   shaders) and texture memory ([`Budget::vram_bytes`],
//!   [`BYTES_PER_BRICK`]). A kernel's loops are bounded by construction.

use std::collections::VecDeque;

/// Texture bytes one baked brick takes: the truth atlas (64 x 8 R32F and R8)
/// and the render copy (81 x 9 R16F and R8). Atlas rows grow in powers of
/// two, so a volume may hold up to twice this per brick.
pub const BYTES_PER_BRICK: u64 = 64 * 8 * 4 + 64 * 8 + 81 * 9 * 2 + 81 * 9;

/// What the engine may spend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// GPU ms a frame's bake (truth and copy passes) may take.
    pub bake_ms: f32,
    /// Whole-frame GPU ms (median of settled frames, the bake left out) the
    /// field look keeps inside by its [`Quality`] levels.
    pub frame_ms: f32,
    /// Bricks a frame bakes at most (and at least, while any wait).
    pub max_bricks_per_frame: usize,
    pub min_bricks_per_frame: usize,
    /// Bricks a frame bakes before anything is measured.
    pub start_bricks_per_frame: usize,
    /// Worst-case kernel ops (AIR ops, 512 samples a brick) a frame's bake
    /// may run: a slow kernel bakes fewer bricks a frame, never a long draw.
    pub bake_ops: u64,
    /// Texture memory all volumes may hold.
    pub vram_bytes: u64,
}

impl Budget {
    /// A desktop GPU.
    pub fn desktop() -> Budget {
        Budget { bake_ms: 4.0, frame_ms: 33.0, max_bricks_per_frame: 32768, min_bricks_per_frame: 32, start_bricks_per_frame: 1024, bake_ops: 2_000_000_000, vram_bytes: 512 << 20 }
    }

    /// The web (and any GPU a host calls weak): a quarter of the work, a
    /// fifth of the memory (a tab shares the GPU with the whole browser).
    pub fn web() -> Budget {
        Budget { bake_ms: 2.0, frame_ms: 25.0, max_bricks_per_frame: 2048, min_bricks_per_frame: 16, start_bricks_per_frame: 256, bake_ops: 500_000_000, vram_bytes: 96 << 20 }
    }

    /// This build's default.
    pub fn for_platform() -> Budget {
        if cfg!(target_arch = "wasm32") {
            Budget::web()
        } else {
            Budget::desktop()
        }
    }

    /// Baked bricks the texture budget holds.
    pub fn max_bricks(&self) -> u64 {
        self.vram_bytes / (BYTES_PER_BRICK * 2)
    }

    /// Bricks a frame may bake of a volume whose costliest kernel runs
    /// `kernel_cost` ops per sample (0: no kernels).
    pub fn bricks_for_kernels(&self, kernel_cost: u64) -> usize {
        if kernel_cost == 0 {
            return self.max_bricks_per_frame;
        }
        ((self.bake_ops / (512 * kernel_cost)) as usize).clamp(1, self.max_bricks_per_frame)
    }
}

/// How finely the field look draws: the levers a frame over its budget
/// turns down, one level at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    /// 0: full; [`Quality::LEVELS`]`.len() - 1`: the least.
    pub level: usize,
    /// The share of a view's sphere-tracing steps a ray takes.
    pub steps: f32,
    /// The coarse pass's size, of the march's (0.5: half per axis).
    pub coarse_scale: f64,
    /// Exact kernel evaluation where a pixel spans fewer cells than this
    /// (else the render copy: level of detail).
    pub exact_cells: f32,
    /// The share of the bake budget a frame takes (fewer re-baked bricks).
    pub bake: f32,
}

impl Quality {
    pub const LEVELS: [Quality; 4] = [
        Quality { level: 0, steps: 1.0, coarse_scale: 0.5, exact_cells: 2.0, bake: 1.0 },
        Quality { level: 1, steps: 0.85, coarse_scale: 0.5, exact_cells: 1.0, bake: 0.75 },
        Quality { level: 2, steps: 0.7, coarse_scale: 0.25, exact_cells: 0.5, bake: 0.5 },
        Quality { level: 3, steps: 0.6, coarse_scale: 0.25, exact_cells: 0.0, bake: 0.25 },
    ];

    pub fn full() -> Quality {
        Quality::LEVELS[0]
    }

    /// `steps` (a view's budget) at this level, at least 16.
    pub fn steps_of(&self, steps: u32) -> u32 {
        ((steps as f32 * self.steps) as u32).max(16)
    }
}

/// Frames not judged after a start or a level change.
pub const SETTLE: usize = 30;
/// Settled frames a judgement takes the median of.
const WINDOW: usize = 60;

/// One measured frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameCost {
    /// The whole frame's GPU busy time, ms.
    pub busy_ms: f32,
    /// The bake's passes (`sdf_bake`, `sdf_copy`), ms.
    pub bake_ms: f32,
    /// Every pass named `sdf`/`fab_sdf` (bake, copy, coarse), ms.
    pub sdf_ms: f32,
}

/// Frame GPU times from the platform's frame timer.
pub struct GpuMeter {
    pub budget: Budget,
    frames: VecDeque<FrameCost>,
    /// Bricks recorded since the last poll (the bakers report them).
    bricks: usize,
    /// Measured bake ms per brick (a moving average; 0: none yet).
    pub ms_per_brick: f32,
    /// Frames measured since [`GpuMeter::reset_health`].
    pub measured: usize,
    /// Frames still to skip before judging.
    skip: usize,
    quality: Quality,
}

/// Frames kept for the medians.
const KEEP: usize = 120;

impl GpuMeter {
    pub fn new(budget: Budget) -> GpuMeter {
        GpuMeter { budget, frames: VecDeque::new(), bricks: 0, ms_per_brick: 0.0, measured: 0, skip: SETTLE, quality: Quality::full() }
    }

    /// Bricks a baker recorded (for the cost per brick).
    pub fn baked(&mut self, bricks: usize) {
        self.bricks += bricks;
    }

    /// Takes the frames the GPU finished (turning the frame timer on).
    pub fn poll(&mut self) {
        use makepad_draw::makepad_platform::gpu_frame_timer;
        if !gpu_frame_timer::enabled() {
            gpu_frame_timer::set_enabled(true);
            return;
        }
        let mut bake_ms = 0.0f32;
        for f in gpu_frame_timer::take_frames() {
            let mut c = FrameCost { busy_ms: f.busy_ms, ..FrameCost::default() };
            let mut folded = false;
            for (name, ms) in &f.passes {
                folded |= name.starts_with("unpresented ");
                let name = name.trim_start_matches("unpresented ");
                if name == "sdf_bake" || name == "sdf_copy" {
                    c.bake_ms += ms;
                }
                if name.starts_with("sdf") || name.starts_with("fab_sdf") {
                    c.sdf_ms += ms;
                }
            }
            bake_ms += c.bake_ms;
            // settled frames only: past the settling ones, none with folded repaints
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            if folded {
                continue;
            }
            self.frames.push_back(c);
            self.measured += 1;
            if self.frames.len() > KEEP {
                self.frames.pop_front();
            }
        }
        if self.bricks >= 64 && bake_ms > 0.0 {
            let m = bake_ms / self.bricks as f32;
            self.ms_per_brick = if self.ms_per_brick > 0.0 { self.ms_per_brick * 0.7 + m * 0.3 } else { m };
            self.bricks = 0;
        }
    }

    /// Bricks a frame may bake now: the bake budget (the quality's share of
    /// it) over the measured cost per brick, within the caps.
    pub fn bricks_per_frame(&self) -> usize {
        let b = &self.budget;
        if self.ms_per_brick <= 0.0 {
            return b.start_bricks_per_frame;
        }
        ((b.bake_ms * self.quality.bake / self.ms_per_brick) as usize).clamp(b.min_bricks_per_frame, b.max_bricks_per_frame)
    }

    /// The median GPU ms (the bake left out) of the last `n` settled frames
    /// (`None`: fewer measured).
    pub fn median_ms(&self, n: usize) -> Option<f32> {
        if self.frames.len() < n || n == 0 {
            return None;
        }
        let mut v: Vec<f32> = self.frames.iter().rev().take(n).map(|f| (f.busy_ms - f.bake_ms).max(0.0)).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }

    /// The quality the field look draws at now.
    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Judges the settled frames: a median over the frame budget turns the
    /// quality down a level, one under 40% of it back up. Returns the new
    /// quality and the median that moved it.
    pub fn judge(&mut self) -> Option<(Quality, f32)> {
        let m = self.median_ms(WINDOW)?;
        let level = self.quality.level;
        let next = if m > self.budget.frame_ms {
            (level + 1).min(Quality::LEVELS.len() - 1)
        } else if m < self.budget.frame_ms * 0.4 {
            level.saturating_sub(1)
        } else {
            level
        };
        if next == level {
            return None;
        }
        self.quality = Quality::LEVELS[next];
        self.frames.clear();
        self.skip = SETTLE;
        Some((self.quality, m))
    }

    /// Starts judging afresh (the look changed: older frames say nothing),
    /// at full quality.
    pub fn reset_health(&mut self) {
        self.frames.clear();
        self.measured = 0;
        self.skip = SETTLE;
        self.quality = Quality::full();
    }

    /// The last frame's costs.
    pub fn last(&self) -> Option<FrameCost> {
        self.frames.back().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_hold_without_measurements() {
        let b = Budget::desktop();
        // a 20000-op kernel: 195 bricks a frame (2e9 ops), not 32768
        assert_eq!(b.bricks_for_kernels(20_000), 195);
        assert_eq!(b.bricks_for_kernels(0), b.max_bricks_per_frame);
        assert_eq!(b.bricks_for_kernels(u64::MAX / 1024), 1);
        // the texture budget in bricks
        assert!(b.max_bricks() > 50_000 && Budget::web().max_bricks() < 12_000);
        let mut m = GpuMeter::new(b);
        assert_eq!(m.bricks_per_frame(), b.start_bricks_per_frame);
        assert_eq!(m.judge(), None);
        assert_eq!(m.quality(), Quality::full());
    }

    #[test]
    fn the_bake_follows_its_measured_cost() {
        let mut m = GpuMeter::new(Budget::desktop());
        m.ms_per_brick = 1.0 / 256.0;
        assert_eq!(m.bricks_per_frame(), 1024);
        m.ms_per_brick = 1.0;
        assert_eq!(m.bricks_per_frame(), 32);
    }

    #[test]
    fn a_slow_frame_degrades_the_field_never_drops_it() {
        let mut m = GpuMeter::new(Budget::desktop());
        let push = |m: &mut GpuMeter, ms: f32, bake: f32, n: usize| {
            for _ in 0..n {
                m.frames.push_back(FrameCost { busy_ms: ms, bake_ms: bake, ..Default::default() });
            }
        };
        // a bake's passes are their own budget: 50 ms with 30 of bake is 20
        push(&mut m, 50.0, 30.0, WINDOW);
        assert_eq!(m.judge(), None);
        // over budget: one level down at a time, to the least, never further
        for level in 1..Quality::LEVELS.len() + 2 {
            m.frames.clear();
            push(&mut m, 40.0, 0.0, WINDOW);
            let moved = m.judge();
            assert_eq!(m.quality().level, level.min(Quality::LEVELS.len() - 1));
            assert_eq!(moved.is_some(), level < Quality::LEVELS.len());
        }
        // well under: back up
        m.frames.clear();
        push(&mut m, 5.0, 0.0, WINDOW);
        assert_eq!(m.judge().map(|(q, _)| q.level), Some(Quality::LEVELS.len() - 2));
        // fewer settled frames than a window: no judgement
        push(&mut m, 40.0, 0.0, WINDOW - 1);
        assert_eq!(m.judge(), None);
        // a restart judges afresh at full quality
        m.reset_health();
        assert_eq!(m.quality(), Quality::full());
    }
}
