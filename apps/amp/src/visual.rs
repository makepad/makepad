//! The picture of the sound going through the equalizer: a spectrum of
//! log-spaced bars with falling peak marks over a waveform strip, or, in
//! `compact` mode, just the bars (the now-playing bar's small one).
//!
//! Each frame reads the newest samples of the sound playing ([`Model::sound`]:
//! a local file's, or the routed player's) from its ring
//! ([`makepad_audio_route::AnalyzerHandle`]), runs one FFT
//! (`makepad-audio-picture`) and folds it into bands. It asks for the next frame only while samples keep arriving or
//! the bars are still falling: paused, off or silent, nothing redraws.

use crate::model::Model;
use crate::widgets::DrawMusicRound;
use crate::{mix, with_alpha, Theme};
use makepad_audio_picture::{log_bands, Fft};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A style's own bar: one quad per band over the band's whole column,
    // drawn instead of the classic bars when `custom_bars` is on. Its
    // pixel gets the band's `band` index, the `bands` count, the `level`
    // and falling `peak` (0..1), and `color` (the classic bar's colour).
    set_type_default() do #(DrawSpectrumBar::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let h = self.rect_size.y
            let y = (1.0 - self.pos.y) * h
            let lit = step(y, h * self.level)
            let pk = 1.0 - smoothstep(0.0, 2.0, abs(y - h * self.peak))
            return vec4(self.color.xyz, 1.0) * max(lit, pk * step(0.001, self.peak))
        }
    }

    // A style's own picture of the waveform (and anything else over the
    // whole screen), drawn instead of the classic line when `custom_scope`
    // is on: one quad over the picture. `samples` (sample_nearest, x in
    // 0..1, 512 samples, -1..1 already scaled by the gain) and `levels`
    // (x = band, row y 0.25 = level, y 0.75 = peak) are textures;
    // `split` is where the spectrum ends and the waveform strip starts
    // (0..1 down the quad), `band_count` the bands, `loud` the overall
    // level (0..1), `ink` the accent and `ink2` the text colour.
    set_type_default() do #(DrawScope::script_shader(vm)){
        ..mod.draw.DrawQuad
        samples: texture_2d(float)
        levels: texture_2d(float)
        split: uniform(0.7)
        band_count: uniform(72.0)
        loud: uniform(0.0)
        ink: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        ink2: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // Where the picture sits in this quad (x0, y0, x1, y1, 0..1): the
        // quad is larger by the Visualizer's `scope_pad`.
        inner_rect: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        pixel: fn() {
            // The classic line, as a trace over the strip.
            let q = (self.pos - self.inner_rect.xy) / max(self.inner_rect.zw - self.inner_rect.xy, vec2(0.001, 0.001))
            let strip = (q.y - self.split) / max(1.0 - self.split, 0.001)
            if strip < 0.0 || strip > 1.0 || q.x < 0.0 || q.x > 1.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let v = self.samples.sample_nearest(vec2(q.x, 0.5)).x
            let y = 0.5 - v * 0.5
            let hpx = self.rect_size.y * (self.inner_rect.w - self.inner_rect.y) * (1.0 - self.split)
            let d = abs(strip - y) * hpx
            let a = 1.0 - smoothstep(0.75, 1.75, d)
            return vec4(self.ink.xyz * a * 0.9, a * 0.9)
        }
    }

    mod.widgets.VisualizerBase = #(Visualizer::register_widget(vm))
    mod.widgets.Visualizer = set_type_default() do mod.widgets.VisualizerBase{
        width: Fill
        height: Fill
        draw_round: mod.widgets.DrawMusicRound{}
        draw_light: mod.widgets.DrawMusicRound{}
        draw_label +: {text_style: theme.font_regular{font_size: 8.5}}
    }
}

/// The spectrum's range: bands from `LOW_HZ` to `HIGH_HZ`, levels from
/// `FLOOR_DB` (empty) to 0 dB (full), after a tilt of `TILT_DB` per octave
/// around 1 kHz so ordinary music reads level rather than sloping away.
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 16_000.0;
const FLOOR_DB: f32 = -66.0;
const TILT_DB: f32 = 3.0;
/// How fast bars and peak marks fall, in full heights per second, and how
/// long a peak mark waits before it falls.
const FALL: f32 = 1.6;
const PEAK_FALL: f32 = 0.7;
const PEAK_HOLD: f64 = 0.6;
/// Samples stop arriving for this long and the picture winds down.
const STALE: f64 = 0.25;
/// Samples no louder than this (about -100 dB) are silence.
const SILENCE: f32 = 1.0e-5;

#[derive(Script, ScriptHook, Widget)]
pub struct Visualizer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_round: DrawMusicRound,
    #[live]
    draw_line: DrawChartSegment,
    /// A style's bars (`custom_bars`).
    #[live]
    draw_bar: DrawSpectrumBar,
    /// A style's picture over the whole screen (`custom_scope`).
    #[live]
    draw_scope: DrawScope,
    /// Invisible quads over the lit part of each bar: the lights the bars
    /// give the device.
    #[live]
    draw_light: DrawMusicRound,
    /// Draw `draw_bar` per band instead of the classic bars.
    #[live(false)]
    custom_bars: bool,
    /// Draw `draw_scope` over the picture instead of the classic waveform line.
    #[live(false)]
    custom_scope: bool,
    /// The light the bars give the device: `light_low` at a quiet band
    /// fading to `light_high` at a loud one (rgb, and w > 0 to use them);
    /// otherwise each bar's own colour.
    #[live]
    light_low: Vec4f,
    #[live]
    light_high: Vec4f,
    /// The classic waveform: line thickness (pt) and a fill under it
    /// (alpha, 0 = a line only).
    #[live(1.5)]
    wave_thickness: f64,
    #[live(0.0)]
    wave_fill: f64,
    /// Room round the `draw_scope` quad (pt), so a style's glow is not cut
    /// at the picture's edge; `inner_rect` says where the picture is in it.
    #[live(0.0)]
    scope_pad: f64,
    #[rust]
    samples_tex: Option<Texture>,
    #[rust]
    levels_tex: Option<Texture>,
    #[live]
    draw_label: DrawText,
    /// Bars only, fewer of them: the now-playing bar's small spectrum.
    #[live(false)]
    compact: bool,
    /// How strongly the bars light the surfaces round the picture (the
    /// relief buffer); 0 lights nothing.
    #[live(0.0)]
    light: f64,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    state: Option<Analysis>,
}

/// What the picture keeps between frames; made on the first draw.
struct Analysis {
    fft: Fft,
    samples: Vec<f32>,
    bins: Vec<f32>,
    bands: Vec<f32>,
    /// The tilt added to each band, dB.
    tilt: Vec<f32>,
    /// Shown heights 0..1, and the peak marks above them.
    level: Vec<f32>,
    peak: Vec<f32>,
    peak_at: Vec<f64>,
    /// The waveform strip's gain, following the loudness slowly.
    wave_gain: f32,
    written: u64,
    fresh_at: f64,
    drawn_at: f64,
}

impl Analysis {
    fn new(compact: bool) -> Self {
        let (size, bands) = if compact { (1024, 20) } else { (2048, 72) };
        let ratio = HIGH_HZ / LOW_HZ;
        let tilt = (0..bands)
            .map(|band| {
                let centre = LOW_HZ * ratio.powf((band as f32 + 0.5) / bands as f32);
                TILT_DB * (centre / 1000.0).log2()
            })
            .collect();
        Self {
            fft: Fft::new(size),
            samples: vec![0.0; size],
            bins: vec![-120.0; size / 2],
            bands: vec![-120.0; bands],
            tilt,
            level: vec![0.0; bands],
            peak: vec![0.0; bands],
            peak_at: vec![0.0; bands],
            wave_gain: 1.0,
            written: 0,
            fresh_at: f64::MIN,
            drawn_at: 0.0,
        }
    }

    /// Take the newest samples, if any came, and move the bars toward them.
    /// `true` while there is anything left to animate.
    fn step(&mut self, model: &Model, now: f64) -> bool {
        let dt = (now - self.drawn_at).clamp(0.0, 0.1) as f32;
        self.drawn_at = now;
        let sound = model.sound();
        let listening = sound.is_some();
        if let Some(analyzer) = sound {
            let written = analyzer.written();
            if written != self.written {
                self.written = written;
                analyzer.latest(&mut self.samples);
                // A paused player still sends blocks, of silence: only sound
                // keeps the picture moving.
                if self.samples.iter().any(|sample| sample.abs() > SILENCE) {
                    self.fresh_at = now;
                    self.fft.magnitudes_db(&self.samples, &mut self.bins);
                    log_bands(&self.bins, analyzer.sample_rate(), LOW_HZ, HIGH_HZ, &mut self.bands);
                }
            }
        }
        let fresh = listening && now - self.fresh_at < STALE;
        let mut moving = fresh;
        for band in 0..self.level.len() {
            let target = if fresh { ((self.bands[band] + self.tilt[band] - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0) } else { 0.0 };
            let level = &mut self.level[band];
            *level = if target >= *level { target } else { (*level - FALL * dt).max(target) };
            if *level >= self.peak[band] {
                self.peak[band] = *level;
                self.peak_at[band] = now;
            } else if now - self.peak_at[band] > PEAK_HOLD {
                self.peak[band] = (self.peak[band] - PEAK_FALL * dt).max(*level);
            }
            moving |= *level > 0.0 || self.peak[band] > 0.0;
        }
        let loudest = self.samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        let wanted = 0.85 / loudest.max(0.02);
        self.wave_gain += (wanted - self.wave_gain) * (dt * 4.0).min(1.0);
        if !fresh {
            // The strip settles to a flat line with the bars.
            for sample in &mut self.samples {
                *sample *= (1.0 - dt * 8.0).max(0.0);
            }
            moving |= self.samples.iter().any(|s| s.abs() > 1.0e-4);
        }
        moving
    }
}

impl Widget for Visualizer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.area.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let Some(model) = scope.data_mut().get::<Model>() else { return DrawStep::done() };
        let theme = model.theme;
        let now = cx.seconds_since_app_start();
        let compact = self.compact;
        let state = self.state.get_or_insert_with(|| Analysis::new(compact));
        let moving = state.step(model, now);
        let listening = model.sound().is_some();
        if compact {
            self.draw_bars(cx, &theme, rect, 1.5);
        } else {
            // The connection's own hint (`connection.rs`) says what is
            // missing while the music system plays unconnected.
            let hint = if !model.has_route {
                "Play a local file to see the sound"
            } else if model.eq.connect {
                "Play something to see the sound"
            } else {
                "Play a local file, or connect Spotify's audio (top bar), to see the sound"
            };
            self.draw_full(cx, &theme, rect, (!listening && !model.connect_hint).then_some(hint));
        }
        if moving {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

impl Visualizer {
    /// `hint`: what to say over the empty picture while nothing is heard.
    fn draw_full(&mut self, cx: &mut Cx2d, theme: &Theme, rect: Rect, hint: Option<&str>) {
        let well = mix(theme.background, theme.foreground, 0.035);
        self.draw_round.color = well;
        self.draw_round.radius = 10.0;
        self.draw_round.part = 1.0;
        self.draw_round.draw_abs(cx, rect);
        let inner = Rect { pos: rect.pos + dvec2(18.0, 16.0), size: rect.size - dvec2(36.0, 32.0) };
        if inner.size.x < 40.0 || inner.size.y < 60.0 {
            return;
        }
        let spectrum = Rect { pos: inner.pos, size: dvec2(inner.size.x, (inner.size.y * 0.68 - 16.0).floor()) };
        let wave = Rect { pos: dvec2(inner.pos.x, spectrum.pos.y + spectrum.size.y + 24.0), size: dvec2(inner.size.x, inner.size.y - spectrum.size.y - 24.0) };

        // Level lines every 12 dB, frequency marks along the bottom.
        let rule = with_alpha(theme.foreground, 0.06);
        for step in 1..(-FLOOR_DB / 12.0) as i32 + 1 {
            let y = spectrum.pos.y + spectrum.size.y * (step as f64 * 12.0 / -FLOOR_DB as f64);
            self.draw_round.color = rule;
            self.draw_round.radius = 0.0;
            self.draw_round.part = 2.0;
            self.draw_round.draw_abs(cx, Rect { pos: dvec2(spectrum.pos.x, y.floor()), size: dvec2(spectrum.size.x, 1.0) });
        }
        self.draw_label.color = with_alpha(theme.muted, 0.8);
        let ratio = (HIGH_HZ / LOW_HZ) as f64;
        for (hz, text) in [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k")] {
            let x = spectrum.pos.x + spectrum.size.x * ((hz / LOW_HZ as f64).ln() / ratio.ln());
            self.draw_label.draw_abs(cx, dvec2(x - 6.0, spectrum.pos.y + spectrum.size.y + 4.0), text);
        }
        self.draw_bars(cx, theme, spectrum, 2.0);

        // The waveform: the newest samples across the strip, scaled to the
        // strip by a gain that follows the loudness.
        let mid = wave.pos.y + wave.size.y * 0.5;
        self.draw_round.color = rule;
        self.draw_round.radius = 0.0;
        self.draw_round.part = 6.0;
        self.draw_round.draw_abs(cx, Rect { pos: dvec2(wave.pos.x, mid.floor()), size: dvec2(wave.size.x, 1.0) });
        if self.custom_scope {
            self.draw_scope_quad(cx, theme, inner, spectrum);
        } else {
            self.draw_wave_line(cx, theme, wave, mid);
        }

        if let Some(text) = hint {
            self.draw_label.color = theme.muted;
            let width = crate::widgets::text_width(&self.draw_label, cx, text);
            self.draw_label.draw_abs(cx, dvec2(spectrum.pos.x + (spectrum.size.x - width) * 0.5, spectrum.pos.y + spectrum.size.y * 0.5), text);
        }
    }

    fn light_color(&self, bar: Vec4f, level: f32) -> Vec4f {
        if self.light_low.w > 0.0 {
            mix(self.light_low, self.light_high, level)
        } else {
            bar
        }
    }

    /// The classic waveform line.
    fn draw_wave_line(&mut self, cx: &mut Cx2d, theme: &Theme, wave: Rect, mid: f64) {
        let Some(state) = &self.state else { return };
        let points = ((wave.size.x / 2.0) as usize).clamp(2, 1024);
        let take = state.samples.len().min(1024);
        let samples = &state.samples[state.samples.len() - take..];
        let gain = state.wave_gain as f64;
        let half = wave.size.y * 0.5;
        self.draw_line.color = with_alpha(theme.accent, 0.9);
        self.draw_line.thickness = self.wave_thickness as f32;
        self.draw_line.mode = 0.0;
        let at = |point: usize| {
            let sample = samples[point * (take - 1) / (points - 1)] as f64;
            dvec2(wave.pos.x + wave.size.x * point as f64 / (points - 1) as f64, mid - (sample * gain).clamp(-1.0, 1.0) * half)
        };
        if self.wave_fill > 0.0 {
            // The area between the midline and the trace.
            self.draw_round.color = with_alpha(theme.accent, self.wave_fill as f32);
            self.draw_round.radius = 0.0;
            self.draw_round.part = 7.0;
            let step = wave.size.x / (points - 1) as f64;
            for point in 0..points {
                let here = at(point);
                let (y0, y1) = if here.y < mid { (here.y, mid) } else { (mid, here.y) };
                self.draw_round.draw_abs(cx, Rect { pos: dvec2(here.x - step * 0.5, y0), size: dvec2(step + 0.5, (y1 - y0).max(0.5)) });
            }
        }
        let mut previous = at(0);
        for point in 1..points {
            let here = at(point);
            segment(&mut self.draw_line, cx, previous, here);
            previous = here;
        }
    }

    /// A style's picture over the whole screen: the newest samples and the
    /// bands' levels handed over as textures.
    fn draw_scope_quad(&mut self, cx: &mut Cx2d, theme: &Theme, inner: Rect, spectrum: Rect) {
        let Some(state) = &self.state else { return };
        const N: usize = 512;
        let samples_tex = self.samples_tex.get_or_insert_with(|| {
            Texture::new_with_format(cx.cx, TextureFormat::VecRf32 { width: N, height: 1, data: Some(vec![0.0; N]), updated: TextureUpdated::Full })
        });
        let mut data = samples_tex.take_vec_f32(cx.cx);
        data.resize(N, 0.0);
        let take = state.samples.len().min(1024);
        let src = &state.samples[state.samples.len() - take..];
        for (i, v) in data.iter_mut().enumerate() {
            *v = (src[i * (take - 1) / (N - 1)] * state.wave_gain).clamp(-1.0, 1.0);
        }
        samples_tex.put_back_vec_f32(cx.cx, data, None);
        let count = state.level.len();
        let levels_tex = self.levels_tex.get_or_insert_with(|| {
            Texture::new_with_format(cx.cx, TextureFormat::VecRf32 { width: count, height: 2, data: Some(vec![0.0; count * 2]), updated: TextureUpdated::Full })
        });
        let mut data = levels_tex.take_vec_f32(cx.cx);
        data.resize(count * 2, 0.0);
        data[..count].copy_from_slice(&state.level);
        data[count..].copy_from_slice(&state.peak);
        levels_tex.put_back_vec_f32(cx.cx, data, None);
        let loud = state.level.iter().sum::<f32>() / count.max(1) as f32;
        let split = ((spectrum.size.y + 24.0) / inner.size.y.max(1.0)) as f32;
        let dv = &mut self.draw_scope.draw_vars;
        dv.set_texture(0, samples_tex);
        dv.set_texture(1, levels_tex);
        dv.set_uniform(cx, live_id!(split), &[split]);
        dv.set_uniform(cx, live_id!(band_count), &[count as f32]);
        dv.set_uniform(cx, live_id!(loud), &[loud]);
        dv.set_uniform(cx, live_id!(ink), &[theme.accent.x, theme.accent.y, theme.accent.z, theme.accent.w]);
        dv.set_uniform(cx, live_id!(ink2), &[theme.foreground.x, theme.foreground.y, theme.foreground.z, theme.foreground.w]);
        let pad = self.scope_pad.max(0.0);
        let quad = Rect { pos: inner.pos - dvec2(pad, pad), size: inner.size + dvec2(pad * 2.0, pad * 2.0) };
        let ir = [
            (pad / quad.size.x) as f32,
            (pad / quad.size.y) as f32,
            ((pad + inner.size.x) / quad.size.x) as f32,
            ((pad + inner.size.y) / quad.size.y) as f32,
        ];
        self.draw_scope.draw_vars.set_uniform(cx, live_id!(inner_rect), &ir);
        self.draw_scope.draw_abs(cx, quad);
    }

    /// The bars and their peak marks, bottom-aligned in `rect`.
    fn draw_bars(&mut self, cx: &mut Cx2d, theme: &Theme, rect: Rect, gap: f64) {
        let Some(state) = &self.state else { return };
        let count = state.level.len();
        let width = ((rect.size.x - gap * (count - 1) as f64) / count as f64).max(1.0);
        let bottom = rect.pos.y + rect.size.y;
        let radius = (width * 0.5).min(3.0) as f32;
        for band in 0..count {
            let x = rect.pos.x + band as f64 * (width + gap);
            let level = state.level[band] as f64;
            if self.custom_bars {
                // The style's bar over the whole column.
                let color = mix(theme.accent, theme.foreground, 0.1 + 0.35 * state.level[band]);
                self.draw_bar.color = color;
                self.draw_bar.band = band as f32;
                self.draw_bar.bands = count as f32;
                self.draw_bar.level = state.level[band];
                self.draw_bar.peak = state.peak[band];
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, rect.pos.y), size: dvec2(width, rect.size.y) });
                if level > 0.0 && self.light > 0.0 {
                    // Its light: an invisible quad over the lit part.
                    let height = (rect.size.y * level).max(width.min(2.0));
                    self.draw_light.color = vec4(0.0, 0.0, 0.0, 0.0);
                    self.draw_light.radius = radius;
                    self.draw_light.draw_abs(cx, Rect { pos: dvec2(x, bottom - height), size: dvec2(width, height) });
                    let slot = relief_open(cx, 0.0);
                    let lc = self.light_color(color, state.level[band]);
                    let emissive = vec4(lc.x, lc.y, lc.z, self.light as f32 * (0.35 + 0.65 * state.level[band]));
                    relief_close(cx, slot, self.draw_light.draw_vars.area, Some(ReliefProxy { inset: 0.0, radius: radius as f64, disc: false, emissive }));
                }
                continue;
            }
            // A faint slot shows where every bar lives, even at rest.
            self.draw_round.radius = radius;
            self.draw_round.color = with_alpha(theme.foreground, 0.05);
            self.draw_round.part = 3.0; self.draw_round.band = band as f32; self.draw_round.bands = count as f32; self.draw_round.level = state.level[band]; self.draw_round.span = rect.size.y as f32;
            self.draw_round.draw_abs(cx, Rect { pos: dvec2(x, rect.pos.y), size: dvec2(width, rect.size.y) });
            if level > 0.0 {
                let height = (rect.size.y * level).max(width.min(2.0));
                let color = mix(theme.accent, theme.foreground, 0.1 + 0.35 * state.level[band]);
                self.draw_round.color = color;
                self.draw_round.part = 4.0;
                self.draw_round.draw_abs(cx, Rect { pos: dvec2(x, bottom - height), size: dvec2(width, height) });
                if self.light > 0.0 {
                    // Each lit bar is a light for the controls round the screen.
                    let slot = relief_open(cx, 0.0);
                    let lc = self.light_color(color, state.level[band]);
                    let emissive = vec4(lc.x, lc.y, lc.z, self.light as f32 * (0.35 + 0.65 * state.level[band]));
                    relief_close(cx, slot, self.draw_round.draw_vars.area, Some(ReliefProxy { inset: 0.0, radius: radius as f64, disc: false, emissive }));
                }
            }
            let peak = state.peak[band] as f64;
            if peak > 0.0 {
                let y = bottom - rect.size.y * peak;
                self.draw_round.radius = 1.0;
                self.draw_round.color = with_alpha(theme.foreground, 0.7);
                self.draw_round.part = 5.0;
                self.draw_round.draw_abs(cx, Rect { pos: dvec2(x, y - 2.0), size: dvec2(width, 2.0) });
            }
        }
    }
}

/// A style's bar: see `DrawSpectrumBar` in the script above.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSpectrumBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub band: f32,
    #[live]
    pub bands: f32,
    #[live]
    pub level: f32,
    #[live]
    pub peak: f32,
}

/// A style's picture over the screen: see `DrawScope` in the script above.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScope {
    #[deref]
    draw_super: DrawQuad,
}

/// One anti-aliased segment through `DrawChartSegment` (the shader
/// `widgets/src/chart.rs` registers).
fn segment(seg: &mut DrawChartSegment, cx: &mut Cx2d, a: Vec2d, b: Vec2d) {
    let margin = seg.thickness as f64 + 2.0;
    let pos = dvec2(a.x.min(b.x) - margin, a.y.min(b.y) - margin);
    let size = dvec2((a.x - b.x).abs() + 2.0 * margin, (a.y - b.y).abs() + 2.0 * margin);
    seg.seg_a = Vec2f { x: (a.x - pos.x) as f32, y: (a.y - pos.y) as f32 };
    seg.seg_b = Vec2f { x: (b.x - pos.x) as f32, y: (b.y - pos.y) as f32 };
    seg.draw_abs(cx, Rect { pos, size });
}
