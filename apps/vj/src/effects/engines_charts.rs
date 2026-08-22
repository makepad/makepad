//! Stockcharts — a procedural candlestick terminal, driven by the beat.
//!
//! A random-walk OHLC series lives on the CPU (the regen family, like
//! ribbons: rebuilt per frame into capacity-STABLE buffers). Candles commit
//! at exactly `per_beat` candles per beat — beats are detected from the
//! phase wrap, no host wiring needed — while the LIVE candle ticks every
//! frame with a volatility that SPIKES on the beat envelope. The chart
//! scrolls smoothly (sub-candle offset = the fractional candle clock), the
//! y-scale eases toward the visible price window, and an optional cascade
//! state machine slams multi-beat crashes for the red-terminal mood.
//!
//! No glyphs: the effect stack has no text path, so there are NO numbers —
//! axis ticks are dash quads and the terminal reads as a terminal through
//! grid, candles, wicks, moving-average ribbon, crosshair and scanlines.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   a_id  = element class: 0 body, 1 wick, 2 grid, 3 crosshair, 4 MA
//!           ribbon, 5 axis tick dash
//!   a_aux = candle age 0..1 (0 = live edge; grid/crosshair: 0)
//!   uv    = quad-local uv (edge falloff shaping)
//!   a_r0  = up/down (1 = up candle; crosshair: axis select)
//!   a_r1  = |return| 0..1 (move size — big candles burn brighter)
//!   normal = +Z (the chart is a lit plane)
//!
//! # Document keys (`engine: "stockcharts"`)
//! `candles` (96 visible, ≤400), `per_beat` (1.0 candles per beat),
//! `vol` (0.9 — random-walk step, % of price), `drift` (0.0 bias),
//! `spike` (1.2 — beat-envelope volatility gain), `cascade` (0.0..1 —
//! chance per bar of a multi-beat crash run), `bar` (4 beats/bar for the
//! cascade clock), `width`/`height` (14/7 world), `body_w` (0.62 of the
//! candle pitch), `ma` (12-candle moving average, 0 = off), `grid_x`
//! (a vertical line every 4 candles), `grid_y` (7 horizontal lines),
//! `scan` (14 — scanline density, 0 = off). Bindings: `p0` = brightness
//! gain, `p1` = crosshair glow, `p2` = panic tint 0..1 (red overlay wash),
//! `p3` free. Hook: `fx_color` (t = age01, attr = (class, age, up, size)).

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

pub struct ChartsConfig {
    pub candles: usize,
    pub per_beat: f32,
    /// Random-walk step size, percent of price.
    pub vol: f32,
    /// Drift bias per commit (percent of price).
    pub drift: f32,
    /// Beat-envelope volatility gain.
    pub spike: f32,
    /// Chance per bar of starting a crash cascade.
    pub cascade: f32,
    /// Beats per bar (the cascade clock).
    pub bar: f32,
    pub width: f32,
    pub height: f32,
    /// Body width as a fraction of the candle pitch.
    pub body_w: f32,
    /// Moving-average window (candles); 0 disables the ribbon.
    pub ma: usize,
    /// A vertical grid line every N candles.
    pub grid_x: usize,
    /// Horizontal grid line count.
    pub grid_y: usize,
    /// Scanline density (lines per world unit * 2); 0 = off.
    pub scan: f32,
    pub seed: u64,
}

impl Default for ChartsConfig {
    fn default() -> Self {
        Self {
            candles: 96,
            per_beat: 1.0,
            vol: 0.9,
            drift: 0.0,
            spike: 1.2,
            cascade: 0.0,
            bar: 4.0,
            width: 14.0,
            height: 7.0,
            body_w: 0.62,
            ma: 12,
            grid_x: 4,
            grid_y: 7,
            scan: 14.0,
            seed: 13,
        }
    }
}

#[derive(Clone, Copy)]
struct Candle {
    open: f32,
    high: f32,
    low: f32,
    close: f32,
}

pub struct ChartsEngine {
    pub cfg: ChartsConfig,
    /// Ring of committed candles, oldest first (rotated on commit).
    ring: Vec<Candle>,
    live: Candle,
    price: f32,
    /// Beat counting from phase wraps.
    last_phase: f32,
    beats: f64,
    committed: u64,
    /// Crash cascade beats remaining.
    cascade_left: f32,
    /// Smoothed view window.
    view_lo: f32,
    view_hi: f32,
    rng: FxRng,
    warmed: bool,
}

impl ChartsEngine {
    pub fn new(cfg: ChartsConfig) -> Self {
        let n = cfg.candles.clamp(16, 400);
        let seed = cfg.seed;
        Self {
            cfg,
            ring: Vec::with_capacity(n),
            live: Candle { open: 100.0, high: 100.0, low: 100.0, close: 100.0 },
            price: 100.0,
            last_phase: 0.0,
            beats: 0.0,
            committed: 0,
            cascade_left: 0.0,
            view_lo: 95.0,
            view_hi: 105.0,
            rng: FxRng::new(seed),
            warmed: false,
        }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    fn vol_pct(&self) -> f32 {
        Self::san(self.cfg.vol, 0.9).clamp(0.01, 8.0)
    }

    /// One live tick of the walk. `boost` multiplies volatility (beat
    /// spike / cascade), `bias` adds drift. Bias is scaled down hard
    /// (×0.03): ticks run at ~90/sec, so an unscaled per-tick bias
    /// compounds into an exponential runaway that the autoscale then
    /// compresses until candles are dots (found the hard way).
    fn tick_price(&mut self, boost: f32, bias: f32) {
        let step_pct = self.vol_pct() * 0.22 * boost;
        let r = self.rng.next_f32() - 0.5;
        let delta = self.price * (r * step_pct + bias * 0.03) * 0.01;
        self.price = (self.price + delta).clamp(5.0, 100_000.0);
        self.live.close = self.price;
        self.live.high = self.live.high.max(self.price);
        self.live.low = self.live.low.min(self.price);
    }

    fn commit_candle(&mut self) {
        let n = self.cfg.candles.clamp(16, 400);
        let done = self.live;
        if self.ring.len() < n {
            self.ring.push(done);
        } else {
            self.ring.remove(0);
            self.ring.push(done);
        }
        self.committed += 1;
        self.live = Candle {
            open: self.price,
            high: self.price,
            low: self.price,
            close: self.price,
        };
        // Cascade clock: chance to start a crash at each bar boundary; a
        // running cascade burns one beat-worth per committed candle.
        let per_beat = Self::san(self.cfg.per_beat, 1.0).clamp(0.05, 16.0);
        if self.cascade_left > 0.0 {
            self.cascade_left -= 1.0 / per_beat;
        } else {
            let bar = Self::san(self.cfg.bar, 4.0).clamp(1.0, 32.0);
            let candles_per_bar = (bar * per_beat).max(1.0) as u64;
            if self.committed % candles_per_bar == 0
                && self.rng.next_f32() < Self::san(self.cfg.cascade, 0.0).clamp(0.0, 1.0)
            {
                self.cascade_left = bar * 0.75;
            }
        }
    }

    fn pre_roll(&mut self) {
        let n = self.cfg.candles.clamp(16, 400);
        for _ in 0..n {
            // ~40 ticks per candle matches the live rate (90/sec at ~120
            // bpm), so pre-rolled candles have live-sized ranges.
            for _ in 0..40 {
                self.tick_price(1.0, Self::san(self.cfg.drift, 0.0).clamp(-4.0, 4.0));
            }
            self.commit_candle();
        }
        // Start the view window on the actual data.
        let (lo, hi) = self.window_bounds();
        self.view_lo = lo;
        self.view_hi = hi;
        // The pre-roll must not count against the beat clock: `committed`
        // is the ledger the beat-target catches up to.
        self.committed = 0;
    }

    fn window_bounds(&self) -> (f32, f32) {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for c in self.ring.iter().chain(std::iter::once(&self.live)) {
            lo = lo.min(c.low);
            hi = hi.max(c.high);
        }
        if !lo.is_finite() || !hi.is_finite() || hi - lo < 1e-3 {
            (self.price - 1.0, self.price + 1.0)
        } else {
            let pad = (hi - lo) * 0.10;
            (lo - pad, hi + pad)
        }
    }

    /// Per-frame update + mesh emission. `phase` is the beat phase 0..1;
    /// beats are counted from its wraps (no host beat bus needed).
    pub(crate) fn regen(&mut self, dt: f32, phase: f32, mesh: &mut FxMesh) {
        if !self.warmed {
            self.warmed = true;
            self.pre_roll();
        }
        // Beat detection: the phase wrapping under us = one beat.
        if phase < self.last_phase - 0.5 {
            self.beats += 1.0;
        }
        self.last_phase = phase;
        let beats_f = self.beats + phase as f64;
        let per_beat = Self::san(self.cfg.per_beat, 1.0).clamp(0.05, 16.0);
        let target = (beats_f * per_beat as f64).floor() as u64;
        // Catch up (bounded — a long stall never floods the ring).
        let mut guard = 0;
        while self.committed < target && guard < 64 {
            self.commit_candle();
            guard += 1;
        }
        if self.committed < target {
            self.committed = target;
        }
        // Live tick: beat-envelope volatility spike + cascade slam.
        let pulse = (1.0 - phase).powi(3);
        let spike = Self::san(self.cfg.spike, 1.2).clamp(0.0, 12.0);
        let casc = if self.cascade_left > 0.0 { 1.0 } else { 0.0 };
        let boost = (1.0 + spike * pulse) * (1.0 + casc * 1.6);
        let bias = Self::san(self.cfg.drift, 0.0).clamp(-4.0, 4.0) - casc * 2.6;
        // Frame-rate independent: ~90 ticks/sec worth of walk.
        let n_ticks = ((dt * 90.0) as usize).clamp(1, 12);
        for _ in 0..n_ticks {
            self.tick_price(boost, bias);
        }
        // Ease the view window toward the data.
        let (lo, hi) = self.window_bounds();
        let k = (dt * 3.0).clamp(0.0, 1.0);
        self.view_lo += (lo - self.view_lo) * k;
        self.view_hi += (hi - self.view_hi) * k;
        if self.view_hi - self.view_lo < 1e-3 {
            self.view_hi = self.view_lo + 1e-3;
        }

        // ---- emit ---------------------------------------------------------
        mesh.clear();
        let n = self.cfg.candles.clamp(16, 400);
        let w = Self::san(self.cfg.width, 14.0).clamp(2.0, 60.0);
        let h = Self::san(self.cfg.height, 7.0).clamp(1.0, 40.0);
        let pitch = w / n as f32;
        let body_w = pitch * Self::san(self.cfg.body_w, 0.62).clamp(0.1, 0.95);
        let wick_w = (body_w * 0.16).max(pitch * 0.05);
        // Smooth scroll: the live candle slides in by the fractional clock.
        let frac = ((beats_f * per_beat as f64).fract()) as f32;
        let x_off = -frac * pitch;
        let vl = self.view_lo;
        let vh = self.view_hi;
        let y_of = |p: f32| ((p - vl) / (vh - vl) - 0.5) * h;

        // Grid first (drawn under everything: emission order = paint order).
        let gy = self.cfg.grid_y.clamp(0, 24);
        for i in 0..=gy {
            let y = (i as f32 / gy.max(1) as f32 - 0.5) * h;
            Self::quad(mesh, 2.0, -w * 0.5, y - 0.006 * h, w * 0.5, y + 0.006 * h, 0.0, 0.0, 0.0);
        }
        let gx = self.cfg.grid_x.clamp(0, 64).max(1);
        let mut ci = 0usize;
        while ci <= n {
            // Vertical lines ride the candle lattice (scroll with x_off).
            // NO culling: a fixed line count keeps the buffer capacity
            // deterministic; the leftmost line drifts at most one candle
            // pitch outside the frame.
            let x = -w * 0.5 + ci as f32 * pitch + x_off;
            Self::quad(mesh, 2.0, x - 0.004 * w, -h * 0.5, x + 0.004 * w, h * 0.5, 0.0, 0.0, 0.0);
            ci += gx;
        }
        // Axis tick dashes on the right edge (the no-glyph number stand-in).
        for i in 0..=gy {
            let y = (i as f32 / gy.max(1) as f32 - 0.5) * h;
            Self::quad(
                mesh, 5.0, w * 0.5 + pitch * 0.4, y - 0.010 * h, w * 0.5 + pitch * 1.6,
                y + 0.010 * h, 0.0, 0.0, 0.0,
            );
        }

        // Candles: ring (oldest left) then the live candle at the right edge.
        let total = self.ring.len() + 1;
        for (i, c) in self.ring.iter().chain(std::iter::once(&self.live)).enumerate() {
            let age = 1.0 - (i as f32 + 1.0) / total as f32;
            let cx = -w * 0.5 + (i as f32 + 0.5) * pitch + x_off;
            let up = if c.close >= c.open { 1.0 } else { 0.0 };
            let ret = ((c.close - c.open).abs() / (c.open.max(1.0)))
                / (self.vol_pct() * 0.02 + 1e-4);
            let size = ret.clamp(0.0, 1.0);
            // Wick.
            Self::quad(
                mesh, 1.0, cx - wick_w * 0.5, y_of(c.low), cx + wick_w * 0.5, y_of(c.high),
                age, up, size,
            );
            // Body (a doji still shows a sliver).
            let (b0, b1) = if c.close >= c.open { (c.open, c.close) } else { (c.close, c.open) };
            let y0 = y_of(b0);
            let y1 = y_of(b1).max(y0 + h * 0.004);
            Self::quad(mesh, 0.0, cx - body_w * 0.5, y0, cx + body_w * 0.5, y1, age, up, size);
        }

        // Moving-average ribbon: thin quads joining successive MA points.
        let maw = self.cfg.ma.min(64);
        if maw >= 2 && self.ring.len() > maw {
            let closes: Vec<f32> = self
                .ring
                .iter()
                .chain(std::iter::once(&self.live))
                .map(|c| c.close)
                .collect();
            let mut prev: Option<(f32, f32)> = None;
            for i in 0..closes.len() {
                let s = i.saturating_sub(maw - 1);
                let ma: f32 = closes[s..=i].iter().sum::<f32>() / (i - s + 1) as f32;
                let x = -w * 0.5 + (i as f32 + 0.5) * pitch + x_off;
                let y = y_of(ma);
                if let Some((px, py)) = prev {
                    let age = 1.0 - (i as f32 + 1.0) / closes.len() as f32;
                    Self::line(mesh, 4.0, px, py, x, y, h * 0.008, age);
                }
                prev = Some((x, y));
            }
        }

        // Crosshair: full-width price line + full-height time line + dot.
        let ly = y_of(self.price);
        let lx = -w * 0.5 + (total as f32 - 0.5) * pitch + x_off;
        Self::quad(mesh, 3.0, -w * 0.5, ly - 0.008 * h, w * 0.5, ly + 0.008 * h, 0.0, 0.0, 1.0);
        Self::quad(mesh, 3.0, lx - 0.004 * w, -h * 0.5, lx + 0.004 * w, h * 0.5, 0.0, 1.0, 1.0);
        Self::quad(
            mesh, 3.0, lx - pitch * 0.6, ly - pitch * 0.6, lx + pitch * 0.6, ly + pitch * 0.6,
            0.0, 2.0, 1.0,
        );

        mesh.pad_to_high_water();
    }

    /// Axis-aligned quad, uv 0..1, facing +Z.
    #[allow(clippy::too_many_arguments)]
    fn quad(
        mesh: &mut FxMesh,
        class: f32,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        age: f32,
        r0: f32,
        r1: f32,
    ) {
        let n = vec3f(0.0, 0.0, 1.0);
        let a = mesh.push_vert(vec3f(x0, y0, 0.0), class, n, age, vec2f(0.0, 0.0), r0, r1);
        let b = mesh.push_vert(vec3f(x1, y0, 0.0), class, n, age, vec2f(1.0, 0.0), r0, r1);
        let c = mesh.push_vert(vec3f(x1, y1, 0.0), class, n, age, vec2f(1.0, 1.0), r0, r1);
        let d = mesh.push_vert(vec3f(x0, y1, 0.0), class, n, age, vec2f(0.0, 1.0), r0, r1);
        mesh.push_quad(a, b, c, d);
    }

    /// A thick line segment as one quad (the MA ribbon stroke).
    #[allow(clippy::too_many_arguments)]
    fn line(
        mesh: &mut FxMesh,
        class: f32,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        half: f32,
        age: f32,
    ) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let l = (dx * dx + dy * dy).sqrt().max(1e-5);
        let (nx, ny) = (-dy / l * half, dx / l * half);
        let n = vec3f(0.0, 0.0, 1.0);
        let a = mesh.push_vert(vec3f(x0 - nx, y0 - ny, 0.01), class, n, age, vec2f(0.0, 0.0), 0.0, 0.0);
        let b = mesh.push_vert(vec3f(x1 - nx, y1 - ny, 0.01), class, n, age, vec2f(1.0, 0.0), 0.0, 0.0);
        let c = mesh.push_vert(vec3f(x1 + nx, y1 + ny, 0.01), class, n, age, vec2f(1.0, 1.0), 0.0, 0.0);
        let d = mesh.push_vert(vec3f(x0 + nx, y0 + ny, 0.01), class, n, age, vec2f(0.0, 1.0), 0.0, 0.0);
        mesh.push_quad(a, b, c, d);
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms {
            shape: vec4(
                Self::san(self.cfg.scan, 14.0).clamp(0.0, 200.0),
                if self.cascade_left > 0.0 { 1.0 } else { 0.0 },
                Self::san(self.cfg.height, 7.0).clamp(1.0, 40.0),
                Self::san(self.cfg.width, 14.0).clamp(2.0, 60.0),
            ),
            flow: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Stockcharts: additive-blended terminal quads on black. Per class the
    // vertex picks color + edge shaping; the pixel runs a soft-box falloff,
    // CRT scanlines, and the beat/panic gains. Paint order = emission order
    // (grid under candles under crosshair — depth writes off).
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxCharts = set_type_default() do #(DrawVjFxCharts::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)
        v_uv: varying(vec2f)
        v_world: varying(vec3f)
        // (edge softness x, edge softness y, core gain, glow)
        v_shape: varying(vec4f)

        // Document hook: element color. t = age 0..1 (1 = oldest),
        // attr = (class, age, up/axis, size).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let class = attr.x
            if class < 0.5 {
                // Candle body: col_a up / col_b down, big moves burn.
                let base = self.col_b.mix(self.col_a, step(0.5, attr.z))
                let burn = 0.75 + attr.w * 0.7 + self.time_beat.w * (1.0 - t) * 0.6
                return vec4(base.xyz * burn, 1.0 - t * 0.55)
            }
            if class < 1.5 {
                let base = self.col_b.mix(self.col_a, step(0.5, attr.z))
                return vec4(base.xyz * 0.55, (1.0 - t * 0.55) * 0.8)
            }
            if class < 2.5 {
                return vec4(self.col_a.xyz * 0.16, 0.5)
            }
            if class < 3.5 {
                // Kept modest: the bloom stage amplifies this line, and an
                // over-gained crosshair blows out into a white bar.
                let g = 0.55 + self.user.y * 0.5 + self.time_beat.w * 0.25
                return vec4(self.col_c.xyz * g, 0.8)
            }
            if class < 4.5 {
                return vec4(self.col_c.xyz * 0.5 + self.col_a.xyz * 0.4, 0.85)
            }
            return vec4(self.col_a.xyz * 0.6, 0.9)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let class = attr.x
            let world = self.draw_list.view_transform
                * vec4(self.geom.geom_pos.x, self.geom.geom_pos.y, self.geom.geom_pos.z, 1.0)
            self.v_world = world.xyz
            self.v_uv = self.geom.geom_uv
            self.v_color = self.fx_color(attr.y, attr, self.geom.geom_normal, world.xyz)
            // Edge shaping per class: crisp bodies, glowing crosshair.
            let mut shp = vec4(0.25, 0.25, 1.0, 0.0)
            if class > 2.5 {
                if class < 3.5 {
                    // Crosshair: long soft glow across the thin axis.
                    shp = vec4(0.5, 0.5, 1.1, 1.0)
                    if attr.z > 1.5 {
                        // The dot: radial glow.
                        shp = vec4(0.5, 0.5, 1.8, 2.0)
                    }
                }
                if class > 3.5 {
                    if class < 4.5 {
                        // MA ribbon: NO fade along the stroke (per-segment
                        // x-fades read as a dotted line), soft across it.
                        shp = vec4(0.02, 0.45, 1.1, 0.0)
                    }
                }
            }
            self.v_shape = shp
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let f = self.v_uv
            let mut a = smoothstep(0.0, self.v_shape.x, min(f.x, 1.0 - f.x))
                * smoothstep(0.0, self.v_shape.y, min(f.y, 1.0 - f.y))
            if self.v_shape.w > 1.5 {
                // Radial dot.
                let d = length(f - vec2(0.5, 0.5)) * 2.0
                a = pow(clamp(1.0 - d, 0.0, 1.0), 2.0)
            }
            a = a * self.v_shape.z
            // CRT scanlines over the whole plane + phosphor gain + panic.
            let scan = 0.88 + 0.12 * sin(self.v_world.y * self.shape.x * 3.14159265)
            let gain = (1.0 + self.user.x) * scan
            let panic = clamp(self.user.z + self.shape.y * (0.3 + self.time_beat.w * 0.5), 0.0, 1.0)
            let rgb = self.v_color.xyz.mix(vec3(1.0, 0.16, 0.10), panic * 0.55)
            let aa = clamp(a * self.v_color.w, 0.0, 1.0)
            return vec4(rgb * aa * gain, aa)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxCharts {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = brightness gain, p1 = crosshair glow, p2 = panic tint.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (scanline density, cascade active, height, width).
    #[live(vec4(14.0, 0.0, 7.0, 14.0))]
    pub shape: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub flow: Vec4f,
    #[live(vec4(0.28, 0.94, 1.0, 1.0))]
    pub col_a: Vec4f,
    #[live(vec4(1.0, 0.25, 0.63, 1.0))]
    pub col_b: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub col_c: Vec4f,
    #[live(vec4(0.01, 0.012, 0.03, 1.0))]
    pub col_bg: Vec4f,
    /// (fog density, emissive gain, tex mix, unused).
    #[live(vec4(0.05, 1.0, 0.0, 0.0))]
    pub fog: Vec4f,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charts_commit_follows_the_beat_clock() {
        let mut e = ChartsEngine::new(ChartsConfig { per_beat: 2.0, ..Default::default() });
        let mut mesh = FxMesh::default();
        // Warm frame (pre-roll commits `candles`).
        e.regen(0.016, 0.0, &mut mesh);
        let base = e.committed;
        // Simulate 4 beats of phase sweeps at 60 fps.
        let mut phase = 0.0f32;
        for _ in 0..240 {
            phase += 1.0 / 60.0;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            e.regen(1.0 / 60.0, phase, &mut mesh);
        }
        let got = e.committed - base;
        assert!((7..=9).contains(&got), "4 beats at 2/beat must commit ~8, got {got}");
    }

    #[test]
    fn charts_buffer_capacity_is_stable() {
        let mut e = ChartsEngine::new(ChartsConfig::default());
        let mut mesh = FxMesh::default();
        e.regen(0.016, 0.1, &mut mesh);
        let len = mesh.verts.len();
        assert!(len > 0);
        let mut phase = 0.1f32;
        for _ in 0..180 {
            phase = (phase + 1.0 / 60.0).fract();
            e.regen(1.0 / 60.0, phase, &mut mesh);
            assert_eq!(mesh.verts.len(), len, "chart buffer must be capacity-stable");
        }
    }

    #[test]
    fn charts_prices_stay_finite_and_ohlc_consistent() {
        let mut e = ChartsEngine::new(ChartsConfig {
            vol: 6.0,
            spike: 12.0,
            cascade: 1.0,
            drift: -4.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        let mut phase = 0.0f32;
        for _ in 0..600 {
            phase = (phase + 1.0 / 30.0).fract();
            e.regen(1.0 / 30.0, phase, &mut mesh);
        }
        assert!(e.price.is_finite() && e.price >= 5.0);
        for c in &e.ring {
            assert!(c.high >= c.low, "high {} < low {}", c.high, c.low);
            assert!(c.high >= c.open.max(c.close) - 1e-3);
            assert!(c.low <= c.open.min(c.close) + 1e-3);
        }
        for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "non-finite vertex data");
            }
        }
    }

    #[test]
    fn charts_degenerate_params_stay_safe() {
        let mut e = ChartsEngine::new(ChartsConfig {
            candles: 0,
            per_beat: f32::NAN,
            vol: f32::INFINITY,
            width: -3.0,
            height: 0.0,
            grid_y: 0,
            grid_x: 0,
            ma: 1,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.regen(0.016, 0.5, &mut mesh);
        assert!(mesh.vertex_count() > 0);
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w] {
            assert!(v.is_finite(), "uniform not sanitized");
        }
    }
}
