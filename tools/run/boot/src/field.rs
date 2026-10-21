//! The Ignition Field: the boot window while the program compiles. A tilted spiral galaxy grows
//! outward from main(): one arm per crate, one star per four functions installed, the arm being
//! compiled brightest and a star flashing white as it is born; source files orbit as dust until
//! their crate is parsed; O3 installs ignite gold stars with a cross flare; the compiler-zone
//! swap clears the dust with an expanding ring. Every function compiled glints its star (a short
//! white sparkle), in the boot window and over the running app (the window layer: the warm-up
//! after the first present, the JIT loader of later sustained JIT waiting).
//!
//! Every number is the loader's own (`relax_base::boot_stats`, read once per frame). The picture
//! is ONE draw of ONE retained instance buffer that only ever grows: the frame appends the
//! stars and flares born since the last one (one `buffer_update` of the new tail) and the GPU
//! places everything from the uniforms. No per-star state is kept beyond the log needed to
//! write the buffer again after a document reload.
use crate::{
    gpu::Pipeline,
    text::Text,
    document::Line,
    Action, Card, Layout, State, TextLine,
};
use makepad_hal::{buffer_kind, Attr, CmdWriter, Id};
use makepad_script::{id, LiveId};
use relax_base::{
    boot_stats::{self as stats, ARMS},
    run_state::Phase,
};
use std::f32::consts::TAU;

pub const INSTANCES: Id = Id::new(2, 1);
const DUST: usize = 1200;
const STAR_CAP: usize = 8192;
const O3_CAP: usize = 2048;
/// backdrop, main() glow, swap ring, corner brackets, warm-up arc
const FIXED: usize = 5;
/// stars glinted in one frame at most (one buffer write each, contiguous ones merged)
const TWINKLES: usize = 1024;
const CAPACITY: usize = FIXED + DUST + STAR_CAP + O3_CAP;
const FNS_PER_STAR: u32 = 4;
/// arms drawn on the spiral; the last arm (REST) is a scatter
const SPIRAL: usize = ARMS - 1;
const TILT: f32 = 0.62;

const KIND_STAR: f32 = 0.0;
const KIND_DUST: f32 = 1.0;
const KIND_O3: f32 = 2.0;
const KIND_GLOW: f32 = 3.0;
const KIND_RING: f32 = 4.0;
const KIND_BRACKETS: f32 = 5.0;
const KIND_BACKDROP: f32 = 6.0;
const KIND_ARC: f32 = 7.0;

// the concept's tokens
const INK: [f32; 4] = [0.925, 0.902, 1.0, 1.0];
const DIM: [f32; 4] = [0.431, 0.396, 0.576, 1.0];
const BASE: [f32; 4] = [0.616, 0.549, 1.0, 1.0];
const GOLD: [f32; 4] = [1.0, 0.812, 0.416, 1.0];

#[derive(Clone, Copy)]
struct Born {
    kind: f32,
    arm: u8,
    rank: u32,
    at: f32,
    /// the last glint of the star (window time; 0: none)
    twinkle: f32,
}

pub struct Field {
    /// the pipeline the instance buffer was written for (a reload makes a new one)
    pipe: Option<Id>,
    stride: usize,
    /// instances in the GPU buffer
    count: usize,
    log: Vec<Born>,
    floats: Vec<f32>,
    bytes: Vec<u8>,
    /// `ms_since_process_start` when this run of the field began
    began_ms: f64,
    time: f32,
    base_fns: [u32; ARMS],
    emitted: [u32; ARMS],
    /// functions of each arm glinted so far (counted from `base_fns`)
    glinted: [u32; ARMS],
    /// the log entries of each arm's stars, by rank (a star's instance is FIXED + DUST + entry)
    stars_of: [Vec<u32>; ARMS],
    touched: Vec<u32>,
    recent: [f32; ARMS],
    o3_base: u32,
    o3_seen: u32,
    superseded_base: u32,
    p_smooth: f32,
    /// the galaxy's turn at the last frame that moved
    spin: f32,
    labels: [String; ARMS],
    labels_at: f32,
    hud: Hud,
    prime: Vec<(std::sync::Arc<crate::text::Shaped>, f32)>,
}

/// The text the HUD shows, as the loader's numbers left it at the last frame.
#[derive(Default)]
struct Hud {
    fns: u32,
    o3: u32,
    superseded: u32,
    swapped: bool,
    main: bool,
    /// the arm most stars were just born in (-1: none)
    hot: f32,
}

/// What the frame needs to draw it: the uniforms and the instance count.
pub struct Frame {
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

/// The angle, radius (in field radii) and size of star `rank` of `arm`: stars of an arm
/// spread outward from the middle as the arm grows.
fn star_at(arm: usize, rank: u32) -> (f32, f32, f32) {
    let seed = arm as u32 * 100_003 + rank;
    let n = rank as f32 + 1.0;
    let f = (1.0 + n / 40.0).ln() / (1.0 + 2500.0 / 40.0_f32).ln();
    let r = 0.10 + 0.86 * f + (hash(seed, 2) - 0.5) * 0.05;
    let base = if arm < SPIRAL {
        arm as f32 / SPIRAL as f32 * TAU
    } else {
        hash(seed, 3) * TAU
    };
    let a = base + r * 2.2 + (hash(seed, 1) - 0.5) * (0.22 + 0.25 * (1.0 - f));
    (a, r, 0.6 + hash(seed, 4) * 1.1)
}

impl Field {
    pub fn new() -> Self {
        let mut field = Self {
            pipe: None,
            stride: 0,
            count: 0,
            log: Vec::with_capacity(STAR_CAP + O3_CAP),
            floats: Vec::new(),
            bytes: Vec::new(),
            began_ms: 0.0,
            time: 0.0,
            base_fns: [0; ARMS],
            emitted: [0; ARMS],
            glinted: [0; ARMS],
            stars_of: Default::default(),
            touched: Vec::new(),
            recent: [0.0; ARMS],
            o3_base: 0,
            o3_seen: 0,
            superseded_base: 0,
            p_smooth: 0.0,
            spin: 0.0,
            labels: Default::default(),
            labels_at: -1.0,
            hud: Hud::default(),
            prime: Vec::new(),
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
            self.recent[arm] = 0.0;
        }
        self.o3_base = stats::O3_INSTALLED.load(std::sync::atomic::Ordering::Relaxed);
        self.o3_seen = 0;
        self.superseded_base = stats::O3_SUPERSEDED.load(std::sync::atomic::Ordering::Relaxed);
        self.p_smooth = 0.0;
        self.labels_at = -1.0;
        self.log.clear();
        // written again by the next frame
        self.pipe = None;
    }

    pub fn create(writer: &mut CmdWriter, stride: usize) {
        writer.buffer_create(
            INSTANCES,
            buffer_kind::INSTANCE,
            (CAPACITY * stride * 4) as u64,
            Attr::default(),
        );
    }

    pub fn destroy(writer: &mut CmdWriter) {
        writer.buffer_destroy(INSTANCES);
    }

    fn push(&mut self, pipe: &Pipeline, rect: [f32; 4], info: [f32; 4]) {
        pipe.instances
            .write(&[(id!(rect), &rect), (id!(info), &info)], &mut self.floats);
    }

    fn born(&mut self, pipe: &Pipeline, b: Born) {
        let (a, r, size) = star_at(b.arm as usize, b.rank);
        if b.kind == KIND_O3 {
            self.push(pipe, [a, r, size, b.at], [KIND_O3, b.arm as f32, 0.0, 0.0]);
        } else {
            let twinkle = if b.twinkle > 0.0 { b.twinkle + 1.0 } else { 0.0 };
            self.push(pipe, [a, r, size, b.at], [KIND_STAR, b.arm as f32, twinkle, 0.0]);
        }
    }

    /// Writes the buffer from nothing: the fixed instances, the dust, and every star born so far.
    fn rewrite(&mut self, writer: &mut CmdWriter, pipe: &Pipeline) {
        self.stride = pipe.instances.defaults.len();
        self.floats.clear();
        self.floats.reserve(512 * self.stride);
        for kind in [KIND_BACKDROP, KIND_GLOW, KIND_RING, KIND_BRACKETS, KIND_ARC] {
            self.push(pipe, [0.0; 4], [kind, 0.0, 0.0, 0.0]);
        }
        for i in 0..DUST {
            let a = hash(i as u32, 11) * TAU;
            let r = 1.05 + hash(i as u32, 12) * 0.35;
            self.push(
                pipe,
                [a, r, 0.0, 0.0],
                [KIND_DUST, (i as f32 + 0.5) / DUST as f32, 0.0, 0.0],
            );
        }
        for i in 0..self.log.len() {
            let b = self.log[i];
            self.born(pipe, b);
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
            writer.buffer_update(INSTANCES, (first * self.stride * 4) as u64, &self.bytes);
        }
        self.floats.clear();
    }

    /// One frame: reads the loader's counters, appends what was born since the last frame,
    /// and answers the uniforms. `time` is the window's clock in seconds.
    pub fn frame(
        &mut self,
        writer: &mut CmdWriter,
        pipe: &Pipeline,
        state: &State,
        time: f32,
        region: [f32; 4],
        reduce_motion: bool,
        halted: bool,
        fit: bool,
        resolve: f32,
        scatter: bool,
    ) -> Frame {
        use std::sync::atomic::Ordering::Relaxed;
        if self.pipe != Some(pipe.id) {
            if self.pipe.is_some() || self.stride != 0 {
                // the document was replaced: its pipeline has its own record layout
                Self::destroy(writer);
            }
            Self::create(writer, pipe.instances.defaults.len());
            self.rewrite(writer, pipe);
        }
        let previous = self.time;
        let dt = (time - previous).max(0.0);
        self.time = time;
        let now_ms = relax_base::clock::ms_since_process_start();

        // ---- stars: one per FNS_PER_STAR functions installed in an arm
        self.floats.clear();
        let first = self.count;
        let mut total = 0;
        let mut best = (0.5, -1.0);
        for arm in 0..ARMS {
            let fns = stats::fns(arm).saturating_sub(self.base_fns[arm]);
            total += fns;
            let want = fns / FNS_PER_STAR;
            let new = want.saturating_sub(self.emitted[arm]);
            let room = (STAR_CAP as u32).saturating_sub(self.stars());
            let new = new.min(room);
            self.recent[arm] *= (-dt * 2.0).exp();
            self.recent[arm] += new as f32;
            if self.recent[arm] > best.0 {
                best = (self.recent[arm], arm as f32);
            }
            for i in 0..new {
                // births spread over the frame interval, not stacked at its end
                let at = previous + dt * (i + 1) as f32 / new as f32;
                let b = Born {
                    kind: KIND_STAR,
                    arm: arm as u8,
                    rank: self.emitted[arm] + i,
                    at,
                    twinkle: 0.0,
                };
                self.stars_of[arm].push(self.log.len() as u32);
                self.log.push(b);
                self.born(pipe, b);
            }
            self.emitted[arm] += new;
        }
        // ---- O3 installs: a gold star on a star of the arm that got the code
        let installed = stats::O3_INSTALLED.load(Relaxed).wrapping_sub(self.o3_base);
        let behind = installed.wrapping_sub(self.o3_seen);
        if behind > stats::O3_RING as u32 {
            // fell behind the ring: the oldest installs are gone
            self.o3_seen = installed.wrapping_sub(stats::O3_RING as u32);
        }
        while self.o3_seen != installed && self.log.len() < STAR_CAP + O3_CAP {
            let (arm, key) = stats::o3_install(self.o3_base.wrapping_add(self.o3_seen));
            self.o3_seen = self.o3_seen.wrapping_add(1);
            // the star of that arm (any arm that has stars when it has none)
            let mut arm = arm;
            if self.emitted[arm] == 0 {
                match (0..ARMS).max_by_key(|a| self.emitted[*a]) {
                    Some(a) if self.emitted[a] > 0 => arm = a,
                    _ => continue,
                }
            }
            let rank = key.wrapping_mul(2_654_435_761) % self.emitted[arm].max(1);
            let b = Born {
                kind: KIND_O3,
                arm: arm as u8,
                rank,
                at: time,
                twinkle: 0.0,
            };
            self.log.push(b);
            self.born(pipe, b);
        }
        let added = self.floats.len() / self.stride;
        self.count += added;
        self.upload(writer, first);
        self.glint(writer, pipe, time, scatter);
        self.hud.fns = total;
        self.hud.hot = best.1;
        self.hud.o3 = installed;
        self.hud.superseded = stats::O3_SUPERSEDED
            .load(Relaxed)
            .saturating_sub(self.superseded_base);

        // ---- the geometry of the picture, in `region` (the window, or the halted view's part)
        let [rx, ry, w, h] = region;
        let narrow = w < 600.0;
        let tiny = w < 300.0 || h < 220.0;
        let (radius, cy) = if halted || fit {
            // the whole galaxy, scatter included, inside the region
            ((w * 0.42).min(h * 0.70).max(12.0), h * 0.5)
        } else {
            let radius = (w * if narrow { 0.40 } else { 0.30 })
                .min(h * if tiny { 0.30 } else if narrow { 0.34 } else { 0.38 })
                .max(16.0);
            (radius, if tiny { h * 0.44 } else if narrow { h * 0.47 } else { h * 0.52 })
        };
        let view = [rx + w * 0.5, ry + cy, radius, TILT];
        let scale = (radius / 220.0).clamp(0.75, 2.0);
        // a halted field stands still where it stopped
        if !halted {
            self.spin = if reduce_motion { 0.0 } else { time * 0.04 };
        }
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
        let swap_age = age(stats::SWAP_END_MS.load(Relaxed), self.began_ms);
        self.hud.main = main_age >= 0.0;
        self.hud.swapped = swap_age >= 0.0;

        // ---- source dust falls in as the crates are parsed
        let target = match state.phase {
            Phase::Resolve => 0.0,
            Phase::Parse => match (state.done, state.total) {
                (Some(done), Some(total)) if total > 0 => 1.03 * done as f32 / total as f32,
                _ => 0.0,
            },
            _ => 1.04,
        };
        self.p_smooth += (target - self.p_smooth).max(0.0) * (1.0 - (-dt * 6.0).exp());
        let dust_fade = if swap_age >= 0.0 {
            1.0 - (swap_age / 0.3).clamp(0.0, 1.0)
        } else {
            1.0
        } * if halted { 0.35 } else { 1.0 };
        Frame {
            count: self.count as u32,
            view,
            params: [spin, scale, main_age, swap_age.max(-1.0)],
            fall: [self.p_smooth, dust_fade, best.1, resolve],
        }
    }

    /// Every function installed since the last frame glints its star: function `i` of an arm
    /// belongs to its star `i / FNS_PER_STAR` (the newest star while that one is not born yet),
    /// or with `scatter` (over the running app) to a star of the arm picked by its hash, so the
    /// compiles of a tab switch sparkle across the galaxy. The stars' records are written again with
    /// the glint's time.
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
                self.born(pipe, b);
            }
            self.upload(writer, FIXED + DUST + touched[run_start] as usize);
            run_start = end;
        }
        self.touched = touched;
    }

    /// Keeps the glyphs of the numbers that change every frame in the atlas, so no frame waits
    /// for a digit it shows for the first time.
    pub fn prime(&mut self, text: &mut Text, dpi: f32, requests: &mut Vec<(u32, f32)>) {
        for run in &self.prime {
            for glyph in &run.0.glyphs {
                text.lookup(glyph.id, run.1, dpi, requests);
            }
        }
    }

    fn stars(&self) -> u32 {
        self.emitted.iter().sum()
    }

    /// The HUD (corner brackets are drawn by the field itself) as text lines in window points,
    /// all fixed to the window; and the close button's card.
    #[allow(clippy::too_many_arguments)]
    pub fn hud(
        &mut self,
        text: &mut Text,
        state: &State,
        hint: &str,
        status: Option<&str>,
        viewport: [f32; 2],
        view: [f32; 4],
        spin: f32,
        time: f32,
        close: bool,
        offscreen: bool,
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
        let margin = 22.0;
        let wide = (w - 2.0 * margin).max(1.0);
        let line = |text: &mut Text, s: &str, size: f32, x: f32, base: f32, align: f32, color: [f32; 4]| -> TextLine {
            let style = Line {
                role: crate::document::Role::Status,
                size,
                height: size * 1.3,
                gap: 0.0,
                color,
                align,
            };
            let run = text.shape(s, size, wide, style.height);
            let left = match align {
                a if a >= 1.0 => x - run.width,
                a if a > 0.0 => x - run.width * a,
                _ => x,
            };
            TextLine {
                position: [left, base - text.ascender(size)],
                run,
                style,
                fixed: true,
            }
        };
        let dim = DIM;
        if self.prime.is_empty() {
            for size in [9.0, 16.0, 18.0, 22.0, 30.0] {
                self.prime.push((text.shape("0123456789,.", size, 1.0e4, size * 1.3), size));
            }
        }
        let t = if offscreen { time as f64 } else { state.elapsed };
        let clock = format!("T+{:.3}", t);
        let reserve = if close { 34.0 } else { 0.0 };
        let fixed = &mut layout.fixed_text;
        let top_size = if narrow { 16.0 } else { 18.0 };
        fixed.push(line(text, &clock, top_size, w - margin - reserve, 32.0, 1.0, INK));
        if !tiny {
            let name = state.app.to_uppercase();
            fixed.push(line(text, &name, 10.0, margin, 30.0, 0.0, dim));
        }
        // the item line: the swap's own numbers, then the parse, then the item being compiled
        if !tiny {
            let s = if self.hud.swapped {
                format!(
                    "zone swap · {} chunks freed · rss {} → {} MB",
                    stats::SWAP_CHUNKS.load(std::sync::atomic::Ordering::Relaxed),
                    stats::SWAP_RSS_BEFORE_MB.load(std::sync::atomic::Ordering::Relaxed),
                    stats::SWAP_RSS_AFTER_MB.load(std::sync::atomic::Ordering::Relaxed),
                )
            } else if !self.hud.main && matches!(state.phase, Phase::Resolve | Phase::Parse) {
                format!(
                    "{} files · {} lines parsed",
                    group(stats::FILES_PARSED.load(std::sync::atomic::Ordering::Relaxed) as u64),
                    group(stats::LINES_PARSED.load(std::sync::atomic::Ordering::Relaxed)),
                )
            } else {
                cut(&state.current, if narrow { 36 } else { 48 })
            };
            let c = if self.hud.swapped || (!self.hud.main && matches!(state.phase, Phase::Resolve | Phase::Parse)) { INK } else { dim };
            fixed.push(line(text, &s, 9.0, margin, 48.0, 0.0, c));
        }
        // fns compiled, bottom left
        let big = if tiny { 16.0 } else if narrow { 22.0 } else { 30.0 };
        fixed.push(line(text, &group(self.hud.fns as u64), big, margin, h - if narrow { 48.0 } else { 44.0 }, 0.0, INK));
        if !tiny {
            let what = if state.warm { "FNS READY" } else { "FNS COMPILED" };
            fixed.push(line(text, &format!("{what} · 1 STAR = {FNS_PER_STAR} FNS"), 9.0, margin, h - 26.0, 0.0, dim));
            // bottom right: the hint, the phase, the O3 / parse line
            let present = state.phase == Phase::FirstFrame;
            let right = w - margin;
            fixed.push(line(text, &hint.to_uppercase(), 10.0, right, h - if narrow { 52.0 } else { 50.0 }, 1.0, if present { GOLD } else { BASE }));
            let phase = status.map_or_else(|| state.phase.name().replace('_', " "), str::to_string);
            fixed.push(line(text, &phase, 11.0, right, h - 38.0, 1.0, INK));
            let (s, c) = if self.hud.o3 > 0 {
                (
                    format!("O3 {} ignited · {} superseded", self.hud.o3, self.hud.superseded),
                    GOLD,
                )
            } else {
                let (done, total) = (state.done.unwrap_or(0), state.total.unwrap_or(0));
                if state.phase == Phase::Parse && total > 0 {
                    (format!("dust: {} of {} crates unparsed", total - done.min(total), total), dim)
                } else {
                    (String::new(), dim)
                }
            };
            if !s.is_empty() {
                fixed.push(line(text, &s, 9.0, right, h - 24.0, 1.0, c));
            }
        }
        // arm labels at the tips of the arms that have compiled something
        if self.hud.main && w >= 360.0 && !tiny {
            if time - self.labels_at > 0.2 || self.labels_at < 0.0 {
                self.labels_at = time;
                for arm in 0..SPIRAL {
                    let n = stats::fns(arm).saturating_sub(self.base_fns[arm]);
                    self.labels[arm].clear();
                    if n >= 30 {
                        let name = stats::arm_name(arm);
                        let len = name.iter().position(|b| *b == 0).unwrap_or(name.len());
                        let name = String::from_utf8_lossy(&name[..len]);
                        if narrow {
                            let short = name.strip_prefix("makepad_").unwrap_or(&name);
                            self.labels[arm] = format!("{}  {}", cut(&short.to_uppercase(), 8), group(n as u64));
                        } else {
                            self.labels[arm] = format!("{}  {}", name.replace('_', "-"), group(n as u64));
                        }
                    }
                }
            }
            let size = if narrow { 8.5 } else { 9.5 };
            for arm in 0..SPIRAL {
                if self.labels[arm].is_empty() {
                    continue;
                }
                let a = arm as f32 / SPIRAL as f32 * TAU + 0.96 * 2.2 + spin;
                let x = view[0] + a.cos() * view[2];
                let y = view[1] + a.sin() * view[2] * view[3];
                let right = x > view[0];
                let c = if self.hud.hot == arm as f32 { INK } else { dim };
                let label = &self.labels[arm];
                let l = line(text, label, size, x, y + size * 0.35, if right { 0.0 } else { 1.0 }, c);
                // a label the window cuts off is left out
                if l.position[0] >= 4.0 && l.position[0] + l.run.width <= w - 4.0 {
                    fixed.push(l);
                }
            }
        }
        if close {
            let rect = [w - 14.0 - 28.0, 14.0, 28.0, 28.0];
            let style = Line {
                role: crate::document::Role::Status,
                size: 16.0,
                height: 28.0,
                gap: 0.0,
                color: dim,
                align: 0.5,
            };
            let run = text.shape("\u{00d7}", style.size, rect[2], style.height);
            let x = rect[0] + (rect[2] - run.width).max(0.0) * 0.5;
            fixed.push(TextLine {
                run,
                position: [x, rect[1]],
                style,
                fixed: true,
            });
            layout.cards.push(Card {
                rect,
                action: Action::Close,
                accent: 4.0,
                members: 0..0,
                fixed: true,
            });
        }
    }
}

/// 12345 -> "12,345"
pub(crate) fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
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
