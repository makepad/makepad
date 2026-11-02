//! The Ignition Field: the boot window while the program compiles. A face-on spiral galaxy forms
//! as the program does: it opens on a field of scattered source dust that, over its first second
//! and a half, warps in place into the spiral (each grain eases along a short curve to a place on
//! an arm while the disk starts to turn), the core lighting up as the parse completes; then one
//! star is born per four functions installed, arriving with a brief brighter twinkle, filling the disk from the
//! core outward along two logarithmic arms (and two fainter ones) over an exponential falloff, so
//! the galaxy's reach is the work done. A soft core, arm glow with dust lanes and the stars all
//! turn slowly, the inner parts faster. O3 installs light gold stars; every function compiled
//! brightens its star for a moment, in the boot window and over the running app (the window
//! layer: the warm-up after the first present, the JIT loader of later sustained JIT waiting).
//! While functions install fast the whole galaxy shimmers a little more (`glitter`, a smoothed
//! install rate mapped into 0..1, so a burst of thousands raises it, never past its cap).
//!
//! The HUD shows one status line, without numbers. The picture is ONE draw of ONE retained
//! instance buffer that only ever grows: the frame appends the stars and flares born since the
//! last one (one `buffer_update` of the new tail) and the GPU places everything from the
//! uniforms (boot.splash `ignition`). No per-star state is kept beyond the log needed to write the
//! buffer again after a document reload.
use crate::{document::Line, gpu::Pipeline, text::Text, Card, Layout, State, TextLine};
use makepad_hal::{buffer_kind, Attr, CmdWriter, Id};
use makepad_script::{id, LiveId};
use relax_base::{
    boot_stats::{self as stats, ARMS},
    run_state::Phase,
};
use std::f32::consts::{PI, TAU};

const INSTANCES: u32 = 2;
const DUST: usize = 1200;
const STAR_CAP: usize = 8192;
const O3_CAP: usize = 2048;
/// backdrop, the disk (core and arm glow), swap ring, corner brackets, warm-up arc
const FIXED: usize = 5;
/// the warp's ease into the turn: the disk's spin speeds up from rest over about this (s)
const SPIN_EASE: f32 = 0.9;
/// functions installed per second at which the glitter is at 63% of its cap
const GLITTER_RATE: f32 = 2000.0;
/// stars glinted in one frame at most (one buffer write each, contiguous ones merged)
const TWINKLES: usize = 1024;
const CAPACITY: usize = FIXED + DUST + STAR_CAP + O3_CAP;
const FNS_PER_STAR: u32 = 4;

// the spiral, shared with boot.splash (`arm_phase`): an arm at base angle b runs along
// a = b - WIND * ln(r / R0), a logarithmic spiral of pitch atan(1 / WIND) (about 23 degrees)
const WIND: f32 = 2.3;
const R0: f32 = 0.08;
/// the disk's reach after the first star and its growth: the reach after `g` stars is
/// REACH0 + (1 - REACH0) * (1 - exp(-g / REACH_STARS))
const REACH0: f32 = 0.18;
const REACH_STARS: f32 = 1400.0;

const KIND_STAR: f32 = 0.0;
const KIND_DUST: f32 = 1.0;
const KIND_O3: f32 = 2.0;
const KIND_DISK: f32 = 3.0;
const KIND_RING: f32 = 4.0;
const KIND_BRACKETS: f32 = 5.0;
const KIND_BACKDROP: f32 = 6.0;
const KIND_ARC: f32 = 7.0;

// the concept's ink
const INK: [f32; 4] = [0.925, 0.902, 1.0, 1.0];

#[derive(Clone, Copy)]
struct Born {
    kind: f32,
    /// a star: its birth order among all stars (its place in the disk); an O3 star: the birth
    /// order of the star it lights
    rank: u32,
    at: f32,
    /// the last glint of the star (window time; 0: none)
    twinkle: f32,
}

pub struct Field {
    /// the pipeline the instance buffer was written for (a reload makes a new one)
    pipe: Option<Id>,
    stride: usize,
    /// the instance buffer
    instances: Id,
    /// instances in the GPU buffer
    count: usize,
    log: Vec<Born>,
    floats: Vec<f32>,
    bytes: Vec<u8>,
    /// `ms_since_process_start` when this run of the field began
    began_ms: f64,
    time: f32,
    base_fns: [u32; ARMS],
    /// stars born for each crate (arm of the loader's counters)
    emitted: [u32; ARMS],
    /// stars born in all
    born: u32,
    /// functions of each arm glinted so far (counted from `base_fns`)
    glinted: [u32; ARMS],
    /// the log entries of each arm's stars, by rank (a star's instance is FIXED + DUST + entry)
    stars_of: [Vec<u32>; ARMS],
    touched: Vec<u32>,
    o3_base: u32,
    o3_seen: u32,
    p_smooth: f32,
    /// the disk's reach, eased towards that of the stars born
    reach: f32,
    /// the galaxy's turn at the last frame that moved
    spin: f32,
    /// functions installed per second, smoothed
    rate: f32,
    status: String,
    status_at: f32,
}

/// What the frame needs to draw it: the uniforms and the instance count.
pub struct Frame {
    pub instances: Id,
    pub count: u32,
    pub view: [f32; 4],
    pub params: [f32; 4],
    pub fall: [f32; 4],
}

fn hash(x: u32, salt: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn reach_of(stars: f32) -> f32 {
    REACH0 + (1.0 - REACH0) * (1.0 - (-stars / REACH_STARS).exp())
}

/// The angle and radius (in disk radii, before the turn), size and seed of the `g`th star born:
/// a tenth in the bulge, most along the arms (two main, two faint), the rest between them; the
/// radius falls off exponentially inside the reach the disk had at its birth.
fn star_at(g: u32) -> (f32, f32, f32, f32) {
    let h = |salt| hash(g, salt);
    let reach = reach_of(g as f32);
    let kind = h(1);
    let (a, r) = if kind < 0.1 {
        let r = (0.12 * (-(h(2).max(1e-4)).ln()).sqrt()).min(0.4).min(reach);
        (h(3) * TAU, r)
    } else {
        // a truncated exponential on [0, 1] of scale 0.5
        let s: f32 = 0.5;
        let x = -s * (1.0 - h(2) * (1.0 - (-1.0 / s).exp())).ln();
        let r = 0.03 + x * reach;
        let (base, spread) = if kind < 0.86 {
            let arm = h(4);
            let base = if arm < 0.4 {
                0.0
            } else if arm < 0.8 {
                PI
            } else if arm < 0.9 {
                0.5 * PI
            } else {
                1.5 * PI
            };
            // about Gaussian across the arm, wider outward
            (base, (h(5) + h(6) - 1.0) * (0.26 + 0.2 * r))
        } else {
            (h(5) * TAU, 0.0)
        };
        (base + spread - WIND * (r / R0).ln(), r)
    };
    // most stars small, a few large
    let size = 0.5 + 2.3 * h(7).powi(7);
    (a, r, size, h(8))
}

impl Field {
    pub fn new() -> Self {
        let mut field = Self {
            pipe: None,
            stride: 0,
            instances: Id::new(INSTANCES, 1),
            count: 0,
            log: Vec::with_capacity(STAR_CAP + O3_CAP),
            floats: Vec::new(),
            bytes: Vec::new(),
            began_ms: 0.0,
            time: 0.0,
            base_fns: [0; ARMS],
            emitted: [0; ARMS],
            born: 0,
            glinted: [0; ARMS],
            stars_of: Default::default(),
            touched: Vec::new(),
            o3_base: 0,
            o3_seen: 0,
            p_smooth: 0.0,
            reach: REACH0,
            spin: 0.0,
            rate: 0.0,
            status: String::new(),
            status_at: 0.0,
        };
        field.reset();
        field
    }

    /// A new run begins (a relaunch): the counters count from here, the buffer starts again.
    pub fn reset(&mut self) {
        self.began_ms = relax_base::clock::ms_since_process_start();
        self.time = 0.0;
        for arm in 0..ARMS {
            self.base_fns[arm] = stats::fns(arm);
            self.emitted[arm] = 0;
            self.glinted[arm] = 0;
            self.stars_of[arm].clear();
        }
        self.born = 0;
        self.o3_base = stats::O3_INSTALLED.load(std::sync::atomic::Ordering::Relaxed);
        self.o3_seen = 0;
        self.p_smooth = 0.0;
        self.reach = REACH0;
        self.rate = 0.0;
        self.status.clear();
        self.status_at = 0.0;
        self.log.clear();
        // written again by the next frame
        self.pipe = None;
    }

    /// The buffer for records of `stride`: one that fits stays (a relaunch only writes it again);
    /// a new size is a new generation, as the HAL refuses a destroyed id's generation.
    fn buffer_for(&mut self, writer: &mut CmdWriter, stride: usize) {
        if self.stride == stride {
            return;
        }
        if self.stride != 0 {
            writer.buffer_destroy(self.instances);
            self.instances = Id::new(INSTANCES, self.instances.gen + 1);
        }
        writer.buffer_create(self.instances, buffer_kind::INSTANCE, (CAPACITY * stride * 4) as u64, Attr::default());
    }

    fn push(&mut self, pipe: &Pipeline, rect: [f32; 4], info: [f32; 4]) {
        pipe.instances
            .write(&[(id!(rect), &rect), (id!(info), &info)], &mut self.floats);
    }

    fn write_born(&mut self, pipe: &Pipeline, b: Born) {
        let (a, r, size, seed) = star_at(b.rank);
        let twinkle = if b.twinkle > 0.0 { b.twinkle + 1.0 } else { 0.0 };
        self.push(pipe, [a, r, size, b.at], [b.kind, 0.0, twinkle, seed]);
    }

    /// Writes the buffer from nothing: the fixed instances, the dust, and every star born so far.
    fn rewrite(&mut self, writer: &mut CmdWriter, pipe: &Pipeline) {
        self.stride = pipe.instances.defaults.len();
        self.floats.clear();
        self.floats.reserve(512 * self.stride);
        for kind in [KIND_BACKDROP, KIND_DISK, KIND_RING, KIND_BRACKETS, KIND_ARC] {
            self.push(pipe, [0.0; 4], [kind, 0.0, 0.0, 0.0]);
        }
        // the source dust: scattered over the disk, each grain with the place it warps to (its
        // radius in rect.w, the turn to it in info.z: the nearest ridge of an arm at about its own
        // radius, so every path is short) and when it starts (info.y)
        for i in 0..DUST {
            let i = i as u32;
            let h = |salt| hash(i, salt);
            let a = h(11) * TAU;
            let r = 0.08 + 1.02 * h(12).powf(0.65);
            let to = (r * (0.9 + 0.1 * h(13))).min(1.0);
            let kind = h(16);
            let delta = if kind < 0.86 {
                // two main arms, a few grains on the faint pair between them
                let base = if kind < 0.7 { 0.0 } else { 0.5 * PI };
                let spread = (h(17) + h(18) - 1.0) * (0.26 + 0.2 * to);
                let ridge = base + spread - WIND * (to / R0).ln();
                (ridge - a + 0.5 * PI).rem_euclid(PI) - 0.5 * PI
            } else {
                // between the arms: a small drift
                (h(17) - 0.5) * 0.4
            };
            let size = 0.8 + 1.0 * h(14).powi(3);
            self.push(pipe, [a, r, size, to], [KIND_DUST, h(19), delta, h(15)]);
        }
        for i in 0..self.log.len() {
            let b = self.log[i];
            self.write_born(pipe, b);
        }
        self.count = self.floats.len() / self.stride;
        self.upload(writer, 0);
        self.pipe = Some(pipe.id);
    }

    /// Sends the instances in `floats` (the first one is number `first`) to the buffer.
    fn upload(&mut self, writer: &mut CmdWriter, first: usize) {
        self.bytes.clear();
        for v in &self.floats {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        if !self.bytes.is_empty() {
            writer.buffer_update(self.instances, (first * self.stride * 4) as u64, &self.bytes);
        }
        self.floats.clear();
    }

    /// One frame: reads the loader's counters, appends what was born since the last frame,
    /// and answers the uniforms. `time` is the window's clock in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        writer: &mut CmdWriter,
        pipe: &Pipeline,
        state: &State,
        time: f32,
        region: [f32; 4],
        reduce_motion: bool,
        halted: bool,
        resolve: f32,
        scatter: bool,
    ) -> Frame {
        use std::sync::atomic::Ordering::Relaxed;
        if self.pipe != Some(pipe.id) {
            // a replaced document's pipeline has its own record layout
            self.buffer_for(writer, pipe.instances.defaults.len());
            self.rewrite(writer, pipe);
        }
        let previous = self.time;
        let dt = (time - previous).max(0.0);
        self.time = time;
        let now_ms = relax_base::clock::ms_since_process_start();

        // ---- stars: one per FNS_PER_STAR functions installed in a crate, placed by birth order
        self.floats.clear();
        let first = self.count;
        let mut installed_now = 0u32;
        for arm in 0..ARMS {
            let fns = stats::fns(arm).saturating_sub(self.base_fns[arm]);
            installed_now += fns.saturating_sub(self.glinted[arm]);
            let want = fns / FNS_PER_STAR;
            let new = want.saturating_sub(self.emitted[arm]);
            let room = (STAR_CAP as u32).saturating_sub(self.born);
            let new = new.min(room);
            for i in 0..new {
                // births spread over the frame interval, not stacked at its end
                let at = previous + dt * (i + 1) as f32 / new as f32;
                let b = Born {
                    kind: KIND_STAR,
                    rank: self.born,
                    at,
                    twinkle: 0.0,
                };
                self.born += 1;
                self.stars_of[arm].push(self.log.len() as u32);
                self.log.push(b);
                self.write_born(pipe, b);
            }
            self.emitted[arm] += new;
        }
        // ---- O3 installs: a gold star on a star of the crate that got the code
        let installed = stats::O3_INSTALLED.load(Relaxed).wrapping_sub(self.o3_base);
        let behind = installed.wrapping_sub(self.o3_seen);
        if behind > stats::O3_RING as u32 {
            // fell behind the ring: the oldest installs are gone
            self.o3_seen = installed.wrapping_sub(stats::O3_RING as u32);
        }
        while self.o3_seen != installed && self.log.len() < STAR_CAP + O3_CAP {
            let (arm, key) = stats::o3_install(self.o3_base.wrapping_add(self.o3_seen));
            self.o3_seen = self.o3_seen.wrapping_add(1);
            // a star of that crate (of any crate that has stars when it has none)
            let mut arm = arm;
            if self.stars_of[arm].is_empty() {
                match (0..ARMS).max_by_key(|a| self.stars_of[*a].len()) {
                    Some(a) if !self.stars_of[a].is_empty() => arm = a,
                    _ => continue,
                }
            }
            let stars = &self.stars_of[arm];
            let entry = stars[(key.wrapping_mul(2_654_435_761) % stars.len() as u32) as usize];
            let b = Born {
                kind: KIND_O3,
                rank: self.log[entry as usize].rank,
                at: time,
                twinkle: 0.0,
            };
            self.log.push(b);
            self.write_born(pipe, b);
        }
        let added = self.floats.len() / self.stride;
        self.count += added;
        self.upload(writer, first);
        self.glint(writer, pipe, time, scatter);

        // ---- the geometry of the picture, in `region` (the window, or the halted view's part):
        // round at any size, the radius one length in points for both axes
        let [rx, ry, w, h] = region;
        let narrow = w < 600.0;
        let tiny = w < 300.0 || h < 220.0;
        let (radius, cy) = if halted {
            ((w * 0.40).min(h * 0.40).max(12.0), h * 0.5)
        } else {
            let radius = (w * if narrow { 0.36 } else { 0.30 })
                .min(h * if tiny { 0.30 } else { 0.33 })
                .max(16.0);
            (radius, h * if tiny { 0.46 } else { 0.48 })
        };
        // the reach follows the stars born, eased over about half a second
        self.reach += (reach_of(self.born as f32) - self.reach) * (1.0 - (-dt * 4.0).exp());
        let view = [rx + w * 0.5, ry + cy, radius, self.reach];
        let scale = (radius / 220.0).clamp(0.75, 2.0);
        // a halted field stands still where it stopped; the turn eases in from rest with the warp
        if !halted {
            self.spin = if reduce_motion {
                0.0
            } else {
                0.035 * (time - SPIN_EASE * (1.0 - (-time / SPIN_EASE).exp()))
            };
        }
        // the glitter: the install rate, smoothed over about 0.4 s, eased into 0..1
        if dt > 0.0 {
            let rate = installed_now as f32 / dt.max(1.0 / 240.0);
            self.rate += (rate - self.rate) * (1.0 - (-dt * 2.5).exp());
        }
        let glitter = 1.0 - (-self.rate / GLITTER_RATE).exp();
        let spin = self.spin;

        // ---- events on the loader's clock, as ages in the window's
        let age = |ms: u64, since: f64| -> f32 {
            if ms == 0 || (ms as f64 - 1.0) < since {
                -1.0
            } else {
                ((now_ms - (ms as f64 - 1.0)) / 1000.0) as f32
            }
        };
        let main_age = age(stats::MAIN_AT_MS.load(Relaxed), self.began_ms);
        let swap_age = age(stats::SETTLE_END_MS.load(Relaxed), self.began_ms);

        // ---- the core lights up as the crates are parsed
        let target = match state.phase {
            Phase::Resolve => 0.0,
            Phase::Parse => match (state.done, state.total) {
                (Some(done), Some(total)) if total > 0 => done as f32 / total as f32,
                _ => 0.0,
            },
            _ => 1.0,
        };
        self.p_smooth += (target - self.p_smooth).max(0.0) * (1.0 - (-dt * 3.0).exp());
        if target - self.p_smooth < 0.002 {
            // lands: the shader knows the parse is over (the ring is gone)
            self.p_smooth = self.p_smooth.max(target);
        }
        let dust_fade = if swap_age >= 0.0 {
            let x = (swap_age / 0.5).clamp(0.0, 1.0);
            1.0 - x * x * (3.0 - 2.0 * x)
        } else {
            1.0
        } * if halted { 0.35 } else { 1.0 };
        Frame {
            instances: self.instances,
            count: self.count as u32,
            view,
            params: [spin, scale, main_age, swap_age.max(-1.0)],
            fall: [self.p_smooth, dust_fade, glitter, resolve],
        }
    }

    /// Every function installed since the last frame glints its star: function `i` of a crate
    /// belongs to its star `i / FNS_PER_STAR` (the newest star while that one is not born yet),
    /// or with `scatter` (over the running app) to a star of the crate picked by its hash, so the
    /// compiles of a tab switch sparkle across the galaxy. The stars' records are written again
    /// with the glint's time.
    fn glint(&mut self, writer: &mut CmdWriter, pipe: &Pipeline, time: f32, scatter: bool) {
        self.touched.clear();
        for arm in 0..ARMS {
            let fns = stats::fns(arm).saturating_sub(self.base_fns[arm]);
            let stars = self.stars_of[arm].len() as u32;
            if stars == 0 {
                self.glinted[arm] = fns;
                continue;
            }
            // a burst larger than a frame can show glints its newest functions
            let from = self.glinted[arm].max(fns.saturating_sub(TWINKLES as u32 * FNS_PER_STAR));
            let mut last = u32::MAX;
            for i in from..fns {
                let rank = if scatter {
                    (hash(i, arm as u32 + 7) * stars as f32) as u32 % stars
                } else {
                    (i / FNS_PER_STAR).min(stars - 1)
                };
                if rank != last {
                    self.touched.push(self.stars_of[arm][rank as usize]);
                    last = rank;
                }
            }
            self.glinted[arm] = fns;
        }
        if self.touched.is_empty() {
            return;
        }
        let mut touched = std::mem::take(&mut self.touched);
        touched.sort_unstable();
        touched.dedup();
        touched.truncate(TWINKLES);
        let mut run_start = 0;
        while run_start < touched.len() {
            let mut end = run_start + 1;
            while end < touched.len() && touched[end] == touched[end - 1] + 1 {
                end += 1;
            }
            for &entry in &touched[run_start..end] {
                let b = &mut self.log[entry as usize];
                b.twinkle = time.max(1e-3);
                let b = *b;
                self.write_born(pipe, b);
            }
            self.upload(writer, FIXED + DUST + touched[run_start] as usize);
            run_start = end;
        }
        self.touched = touched;
    }

    /// One status line, without numbers, changing at most every 0.7 s so it reads calmly; the
    /// caption's title and its window buttons' cards.
    #[allow(clippy::too_many_arguments)]
    pub fn hud(
        &mut self,
        text: &mut Text,
        state: &State,
        hint: &str,
        status: Option<&str>,
        viewport: [f32; 2],
        time: f32,
        caption: &crate::caption::Caption,
        layout: &mut Layout,
    ) {
        let [w, h] = viewport;
        layout.fixed_text.clear();
        layout.text.clear();
        layout.cards.clear();
        layout.icon = None;
        layout.panel = [0.0, 0.0, w, h];
        layout.clip = [0.0, 0.0, w, h];
        layout.content_height = 0.0;
        layout.text_ends.clear();
        let tiny = w < 300.0 || h < 220.0;
        let narrow = w < 600.0;
        let fixed = &mut layout.fixed_text;
        if caption.height > 0.0 {
            let style = Line {
                role: crate::document::Role::App,
                size: caption.size,
                height: (caption.size * 1.3).round(),
                gap: 0.0,
                color: caption.ink,
                align: 0.0,
            };
            let name = caption.title.clone().unwrap_or_else(|| state.app.clone());
            fixed.extend(crate::caption::title(text, caption, w, &[(name, style)]));
        }
        if !tiny {
            let wanted = match status {
                Some(s) => s.to_string(),
                None => match state.phase {
                    Phase::Resolve => hint.to_string(),
                    Phase::Parse => {
                        let current = state.current.trim();
                        if current.is_empty() {
                            hint.to_string()
                        } else {
                            format!("{hint} {}", cut(current, 40))
                        }
                    }
                    Phase::Typecheck | Phase::Codegen | Phase::Link | Phase::FirstFrame => {
                        let app = caption.title.as_deref().unwrap_or(&state.app);
                        format!("{hint} {app}")
                    }
                },
            };
            if wanted != self.status && (self.status.is_empty() || time - self.status_at >= 0.7) {
                self.status = wanted;
                self.status_at = time;
            }
            let size = if narrow { 11.0 } else { 12.0 };
            let style = Line {
                role: crate::document::Role::Status,
                size,
                height: size * 1.3,
                gap: 0.0,
                color: [INK[0], INK[1], INK[2], 0.72],
                align: 0.5,
            };
            let run = text.shape(&self.status, size, f32::INFINITY, style.height);
            let base = h - if narrow { 28.0 } else { 34.0 };
            fixed.push(TextLine {
                position: [w * 0.5 - run.width * 0.5, base - text.ascender(size)],
                run,
                style,
                fixed: true,
            });
        }
        for (action, rect) in &caption.buttons {
            layout.cards.push(Card {
                rect: *rect,
                action: *action,
                accent: crate::CAPTION_BUTTON,
                members: 0..0,
                fixed: true,
            });
        }
    }
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
