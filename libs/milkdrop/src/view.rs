//! `MilkdropView`: a widget that plays MilkDrop presets from pushed audio.
//!
//! A host feeds it audio with [`MilkdropView::feed_audio`] (or
//! [`MilkdropView::push_audio`] for interleaved stereo) and presets with
//! [`MilkdropView::load_preset`] (a file: read and parsed on the task pool,
//! applied when it arrives), [`MilkdropView::load_preset_source`] (text in
//! hand) or [`MilkdropView::load_parsed`]. The view renders at its own size (times
//! `resolution_scale`) and draws the picture into its rect; hosts that want
//! the picture elsewhere (a VJ layer) read [`MilkdropView::output_texture`].
//!
//! Two decks: loading a preset while one plays starts the new one on the
//! other deck, seeded with the old deck's picture (MilkDrop hands the
//! feedback buffer over), and cross-fades the two over
//! `transition_seconds`. Each deck owns its subclassed shaders (the preset's
//! warp, composite, per-vertex and per-point code) and its render targets.
//!
//! No repaint at rest: frames are requested only while playing.

use crate::audio::{AudioAnalyzer, AudioFrame};
use crate::hlsl::{self, ShaderKind, Texture as MilkTexture};
use crate::noise;
use crate::parse::Preset;
use crate::render::*;
use crate::shaders::*;
use crate::state::{aspect, Engine, FrameInput, FrameParams, LineBatch, LoadReport, StageTimes, MAX_WAVE_POINTS};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MilkdropViewBase = #(MilkdropView::register_widget(vm))
    mod.widgets.MilkdropView = set_type_default() do mod.widgets.MilkdropViewBase{
        width: Fill
        height: Fill
    }
}

/// What went into the loaded preset, for a host's status line.
#[derive(Clone, Debug, Default)]
pub struct PresetStatus {
    pub name: String,
    pub equations: LoadReport,
    /// Shader translation or compile problems (the MilkDrop 1 default runs
    /// in their place).
    pub shader_errors: Vec<String>,
    pub warp_shader: bool,
    pub comp_shader: bool,
    /// UI-thread time the load took (equations, shader translation and
    /// compile), in ms.
    pub load_ms: f64,
}

/// Textures and targets of one deck.
struct DeckTargets {
    main: [Target; 2],
    /// Which `main` holds the current picture.
    current: usize,
    mesh: Target,
    cpu_mesh: Texture,
    blur: [Target; 6],
    out: Target,
    /// The first frame reads this instead of `main[current]` (another
    /// deck's picture, or black): `main` has never been written yet.
    seed: Option<Texture>,
}

/// One preset's pipeline.
#[derive(Script, ScriptHook)]
pub struct MilkDeck {
    #[live]
    draw_mesh: DrawMilkMesh,
    #[live]
    draw_warp: DrawMilkWarp,
    #[live]
    draw_comp: DrawMilkComp,
    #[live]
    draw_wave0: DrawMilkWave,
    #[live]
    draw_wave1: DrawMilkWave,
    #[live]
    draw_wave2: DrawMilkWave,
    #[live]
    draw_wave3: DrawMilkWave,
    #[rust]
    engine: Option<Engine>,
    #[rust]
    targets: Option<DeckTargets>,
    #[rust]
    params: FrameParams,
    /// Named image textures by slot, for each of warp and comp.
    #[rust]
    images: [Vec<String>; 2],
    /// Highest blur level either shader reads (0 = none).
    #[rust]
    blur_levels: usize,
    #[rust]
    status: PresetStatus,
    /// The picture the next first frame starts from (the outgoing deck's).
    #[rust]
    pending_seed: Option<Texture>,
}

/// Things every deck shares: noise, stand-in images, fixed draws.
struct Shared {
    black: Texture,
    noise_lq: Texture,
    noise_lq_lite: Texture,
    noise_mq: Texture,
    noise_hq: Texture,
    noisevol_lq: Texture,
    noisevol_hq: Texture,
    stand_ins: Vec<(String, Texture)>,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct MilkdropView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    deck_a: MilkDeck,
    #[live]
    deck_b: MilkDeck,
    #[live]
    draw_blur: DrawMilkBlur,
    #[live]
    draw_line: DrawMilkWave,
    #[live]
    draw_shape: DrawMilkShape,
    #[live]
    draw_fill: DrawMilkFill,
    #[live]
    draw_blit: DrawMilkBlit,

    /// Render size relative to the view's pixels.
    #[live(1.0)]
    resolution_scale: f64,
    /// The most pixels a frame renders at (0: no limit): a larger view
    /// renders smaller, its shape kept, and shows the picture scaled up.
    /// A phone GPU's budget, whatever the view's size.
    #[live(0.0)]
    max_pixels: f64,
    /// Warp mesh cells; per-vertex code on the GPU makes a dense grid free.
    #[live(96)]
    mesh_x: usize,
    #[live(72)]
    mesh_y: usize,
    #[live(2.5)]
    transition_seconds: f64,
    /// How strongly the picture lights the surfaces around the view
    /// (`relief_texture_light`); 0 lights nothing.
    #[live(0.0)]
    light: f64,
    /// Soft knee of that light: its brightest frames approach this.
    #[live(2.0)]
    light_knee: f64,
    /// The picture shown in one colour (a monochrome tube): rgb, amount.
    #[live]
    tint: Vec4f,
    /// A colour map: the picture's luminance through four stops (low to
    /// high), blended in by `map_amount`; the light it casts is mapped too.
    #[live]
    map0: Vec4f,
    #[live]
    map1: Vec4f,
    #[live]
    map2: Vec4f,
    #[live]
    map3: Vec4f,
    #[live(0.0)]
    map_amount: f64,
    /// A screen filter over the picture: kind (0 none, 1 CRT, 2 LCD,
    /// 3 halftone, 4 etched glass), strength, cell (pt), CRT curvature.
    #[live]
    filter: Vec4f,
    /// The light it casts graded to one colour: rgb, amount.
    #[live]
    light_tint: Vec4f,

    #[rust]
    area: Area,
    #[rust]
    shared: Option<Shared>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    playing: bool,
    #[rust]
    time: f64,
    #[rust]
    last_clock: Option<f64>,
    #[rust]
    frame: u64,
    #[rust]
    fps: f64,
    /// Which deck is current (0 = a, 1 = b).
    #[rust]
    front: usize,
    /// While fading: time the fade started.
    #[rust]
    fade_start: Option<f64>,
    #[rust]
    audio: Option<AudioAnalyzer>,
    #[rust]
    audio_frame: AudioFrame,
    #[rust]
    seed: u64,
    /// CPU time of the last frame's equations, ms.
    #[rust]
    cpu_ms: f64,
    /// Pixels of the last rendered frame.
    #[rust]
    render_size: (usize, usize),
    /// Preset files being read and parsed on the task pool.
    #[rust]
    loading: Vec<(PathBuf, TaskHandle<std::io::Result<Preset>>)>,
    /// Render at these pixels instead of the view's (a larger window
    /// showing the same picture); the view shows it cropped to its shape.
    #[rust]
    render_pixels: Option<(usize, usize)>,
    /// What the last frame blitted: pictures a and b, the mix toward b.
    #[rust]
    shown: Option<(Texture, Texture, f32)>,
    /// The last frame recorded: its composite pass, the repaint it was
    /// recorded before, and when (app seconds).
    #[rust]
    recorded: Option<(DrawPassId, u64, f64)>,
}

impl WidgetNode for MilkdropView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for MilkdropView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Signal = event {
            self.poll_loading(cx);
        }
        if self.next_frame.is_event(event).is_some() && self.playing {
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 1.0 || rect.size.y < 1.0 {
            return DrawStep::done();
        }
        self.ensure_shared(cx.cx);
        // Clock.
        let clock = cx.seconds_since_app_start();
        let dt = self.last_clock.replace(clock).map(|last| (clock - last).clamp(0.0, 0.1)).unwrap_or(0.0);
        if self.playing {
            self.time += dt;
            if dt > 0.0 {
                let inst = 1.0 / dt;
                self.fps = if self.fps > 0.0 { self.fps * 0.9 + inst * 0.1 } else { inst };
            }
            self.frame += 1;
        }
        if let Some(audio) = self.audio.as_mut() {
            audio.frame(dt as f32, &mut self.audio_frame);
        }
        let dpi = cx.current_dpi_factor();
        let scale = self.resolution_scale.clamp(0.1, 4.0);
        let (want_x, want_y) = match self.render_pixels {
            Some((x, y)) => (x as f64, y as f64),
            None => (rect.size.x * dpi, rect.size.y * dpi),
        };
        let mut scale = scale;
        if self.max_pixels > 0.0 && want_x * want_y * scale * scale > self.max_pixels {
            scale = (self.max_pixels / (want_x * want_y)).sqrt();
        }
        let size = (((want_x * scale).round() as usize).clamp(16, 8192), ((want_y * scale).round() as usize).clamp(16, 8192));
        self.render_size = size;
        let fade = self.fade_start.map(|t0| ((self.time - t0) / self.transition_seconds.max(0.01)).clamp(0.0, 1.0));
        if fade.is_some_and(|f| f >= 1.0) {
            self.fade_start = None;
            // The deck that faded out is done.
            let back = self.back_deck_mut();
            back.engine = None;
        }
        let fade = self.fade_start.and(fade);
        let t0 = std::time::Instant::now();
        // A deck is a feedback loop: each frame's passes read the previous
        // frame's picture (ping-pong targets). The backend does not owe a
        // paint for every recording (another window's beat, a skipped or
        // aborted repaint), and recording again before the last recording
        // painted would pair one frame's writes with the next frame's reads,
        // so the picture tears between unrelated states. Until the last
        // frame painted, show it again and keep its passes attached.
        let hold = match self.recorded {
            Some((pass, at, since)) => cx.passes[pass].painted_serial <= at && clock - since < HOLD_MAX_SECONDS,
            None => false,
        };
        let (front_out, back_out) = if hold {
            match self.shown.clone() {
                Some((a, b, mix)) => (Some(b), (fade.is_some() && mix > 0.0).then_some(a)),
                None => (None, None),
            }
        } else {
            let front_out = self.render_deck(cx, self.front, size);
            let back_out = if fade.is_some() { self.render_deck(cx, 1 - self.front, size) } else { None };
            let deck = if self.front == 0 { &self.deck_a } else { &self.deck_b };
            self.recorded = match (&front_out, &deck.targets) {
                (Some(_), Some(t)) => Some((t.out.pass.draw_pass_id(), cx.repaint_id(), clock)),
                _ => None,
            };
            (front_out, back_out)
        };
        // The decks' composites feed this view's pass.
        for (which, out) in [(self.front, &front_out), (1 - self.front, &back_out)] {
            let deck = if which == 0 { &self.deck_a } else { &self.deck_b };
            if let (Some(_), Some(t)) = (out, &deck.targets) {
                cx.make_child_pass(&t.out.pass);
            }
        }
        self.cpu_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if let Some(front_out) = front_out {
            // During a fade `back` is the outgoing preset: blit it as `a`,
            // the incoming (front) as `b`, and mix toward b.
            let (a, b, mix) = match (&back_out, fade) {
                (Some(back), Some(f)) => (back.clone(), front_out.clone(), f as f32),
                _ => (front_out.clone(), front_out.clone(), 0.0),
            };
            self.draw_blit.draw_vars.set_texture(0, &a);
            self.draw_blit.draw_vars.set_texture(1, &b);
            self.draw_blit.draw_vars.set_uniform(cx, live_id!(u_mix), &[mix, 0.0, 0.0, 0.0]);
            self.draw_blit.draw_vars.set_uniform(cx, live_id!(u_tint), &[self.tint.x, self.tint.y, self.tint.z, self.tint.w]);
            for (id, c) in [(live_id!(u_map0), self.map0), (live_id!(u_map1), self.map1), (live_id!(u_map2), self.map2), (live_id!(u_map3), self.map3)] {
                self.draw_blit.draw_vars.set_uniform(cx, id, &[c.x, c.y, c.z, c.w]);
            }
            self.draw_blit.draw_vars.set_uniform(cx, live_id!(u_map_amount), &[self.map_amount as f32, 0.0, 0.0, 0.0]);
            self.draw_blit.draw_vars.set_uniform(cx, live_id!(u_filter), &[self.filter.x, self.filter.y, self.filter.z, self.filter.w]);
            let uv = cover_uv(size.0 as f64 / size.1 as f64, rect.size.x / rect.size.y);
            self.draw_blit.draw_vars.set_uniform(cx, live_id!(u_uv), &uv);
            self.shown = Some((a, b, mix));
            self.draw_blit.draw_abs(cx, rect);
            if self.light > 0.0 {
                let deck = if self.front == 0 { &self.deck_a } else { &self.deck_b };
                if let Some(t) = &deck.targets {
                    relief_texture_light(
                        cx,
                        self.draw_blit.draw_vars.area,
                        &ReliefTextureLight {
                            texture: t.out.texture.clone(),
                            producer: t.out.pass.draw_pass_id(),
                            intensity: self.light as f32,
                            knee: self.light_knee as f32,
                            depth: 0.0,
                            inset: 0.0,
                            radius: 0.0,
                            map: [self.map0, self.map1, self.map2, self.map3],
                            map_amount: self.map_amount as f32,
                            tint: self.light_tint,
                        },
                    );
                }
            }
        }
        if self.playing {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

impl MilkdropView {
    /// Start or stop the clock (and the frames that follow it).
    pub fn set_playing(&mut self, cx: &mut Cx, playing: bool) {
        if self.playing == playing {
            return;
        }
        self.playing = playing;
        self.last_clock = None;
        if playing {
            self.next_frame = cx.new_next_frame();
        }
        self.area.redraw(cx);
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Played audio: interleaved samples with `channels` per frame.
    pub fn push_audio(&mut self, samples: &[f32], channels: usize, sample_rate: f32) {
        let audio = self.audio.get_or_insert_with(|| AudioAnalyzer::new(sample_rate));
        audio.set_sample_rate(sample_rate);
        audio.push_interleaved(samples, channels);
    }

    /// Played audio, mono.
    pub fn feed_audio(&mut self, samples: &[f32], sample_rate: f32) {
        self.push_audio(samples, 1, sample_rate);
    }

    /// Load the preset file at `path`, fading from the current one. The file
    /// is read and parsed on the task pool; the preset appears when that is
    /// done (a later load wins over an earlier one still in flight).
    pub fn load_preset(&mut self, cx: &mut Cx, path: &Path) {
        let owned = path.to_path_buf();
        match cx.task_pool().submit(Lane::Light, move || std::fs::read(&owned).map(|bytes| Preset::parse(&bytes))) {
            Ok(handle) => {
                self.loading.clear();
                self.loading.push((path.to_path_buf(), handle));
            }
            Err(e) => error!("milkdrop: cannot queue {}: {e:?}", path.display()),
        }
    }

    /// Load a preset from its text, fading from the current one.
    pub fn load_preset_source(&mut self, cx: &mut Cx, name: &str, source: &str) {
        self.load_parsed(cx, name, Arc::new(Preset::parse(source.as_bytes())), true);
    }

    fn poll_loading(&mut self, cx: &mut Cx) {
        let mut done = None;
        for (path, handle) in self.loading.iter_mut() {
            if let Some(result) = handle.try_take() {
                done = Some((path.clone(), result));
            }
        }
        let Some((path, result)) = done else { return };
        self.loading.clear();
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        match result {
            Ok(Ok(preset)) => self.load_parsed(cx, &name, Arc::new(preset), true),
            Ok(Err(e)) => error!("milkdrop: cannot read {}: {e}", path.display()),
            Err(e) => error!("milkdrop: loading {} failed: {e:?}", path.display()),
        }
    }

    /// The picture of the current preset (for hosts compositing it
    /// elsewhere), after the view has drawn once.
    pub fn output_texture(&self) -> Option<Texture> {
        let deck = if self.front == 0 { &self.deck_a } else { &self.deck_b };
        deck.targets.as_ref().map(|t| t.out.texture.clone())
    }

    /// The loaded preset's status.
    pub fn status(&self) -> &PresetStatus {
        let deck = if self.front == 0 { &self.deck_a } else { &self.deck_b };
        &deck.status
    }

    /// CPU time of the last frame's equations and layout, in ms.
    pub fn cpu_ms(&self) -> f64 {
        self.cpu_ms
    }

    /// Pixels of the last rendered frame.
    pub fn render_size(&self) -> (usize, usize) {
        self.render_size
    }

    /// CPU time of the current preset's equation stages last frame.
    pub fn stage_times(&self) -> StageTimes {
        let deck = if self.front == 0 { &self.deck_a } else { &self.deck_b };
        deck.engine.as_ref().map(|e| e.times).unwrap_or_default()
    }

    /// Render at `pixels` (times the resolution scale) instead of the
    /// view's own size: a larger window shows the same picture, and this
    /// view shows it cropped to its shape. `None` goes back to the view's.
    pub fn set_render_pixels(&mut self, cx: &mut Cx, pixels: Option<(usize, usize)>) {
        if self.render_pixels != pixels {
            self.render_pixels = pixels;
            self.area.redraw(cx);
        }
    }

    /// What the last frame showed, for a second view of the same picture:
    /// pictures a and b and the mix toward b (a preset transition), with
    /// the pictures' width over height.
    pub fn shown(&self) -> Option<(Texture, Texture, f32, f64)> {
        let (a, b, mix) = self.shown.clone()?;
        let (x, y) = self.render_size;
        Some((a, b, mix, x as f64 / y.max(1) as f64))
    }

    /// Render at `scale` times the view's pixels (0.1..4); lower it to buy
    /// GPU time on large views.
    pub fn set_resolution_scale(&mut self, cx: &mut Cx, scale: f64) {
        self.resolution_scale = scale;
        self.area.redraw(cx);
    }

    /// Render no more than `pixels` per frame (0: no limit); see
    /// `max_pixels`.
    pub fn set_max_pixels(&mut self, cx: &mut Cx, pixels: f64) {
        self.max_pixels = pixels.max(0.0);
        self.area.redraw(cx);
    }

    /// Load a parsed `preset` (named `name` for the status). With a preset
    /// already showing and `transition` set, it fades in over
    /// `transition_seconds`; otherwise it replaces the current one. Either
    /// way it starts from the current picture, as MilkDrop hands the
    /// feedback buffer to the next preset.
    pub fn load_parsed(&mut self, cx: &mut Cx, name: &str, preset: Arc<Preset>, transition: bool) {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let fading = transition && self.front_deck().engine.is_some();
        let seed_texture = self.front_deck().targets.as_ref().map(|t| t.main[t.current].texture.clone());
        if fading {
            self.front = 1 - self.front;
            self.fade_start = Some(self.time);
        }
        let seed = self.seed;
        let deck = self.front_deck_mut();
        deck.load(cx, name, preset, seed);
        deck.pending_seed = seed_texture;
        self.area.redraw(cx);
    }

    fn front_deck(&self) -> &MilkDeck {
        if self.front == 0 {
            &self.deck_a
        } else {
            &self.deck_b
        }
    }

    fn front_deck_mut(&mut self) -> &mut MilkDeck {
        if self.front == 0 {
            &mut self.deck_a
        } else {
            &mut self.deck_b
        }
    }

    fn back_deck_mut(&mut self) -> &mut MilkDeck {
        if self.front == 0 {
            &mut self.deck_b
        } else {
            &mut self.deck_a
        }
    }

    fn ensure_shared(&mut self, cx: &mut Cx) {
        if self.shared.is_some() {
            return;
        }
        let tex = |cx: &mut Cx, img: noise::NoiseImage| bgra_texture(cx, img.width, img.height, img.data);
        self.shared = Some(Shared {
            black: bgra_texture(cx, 1, 1, vec![0xff00_0000]),
            noise_lq: tex(cx, noise::noise_2d(256, 1, 1)),
            noise_lq_lite: tex(cx, noise::noise_2d(32, 1, 2)),
            noise_mq: tex(cx, noise::noise_2d(256, 4, 3)),
            noise_hq: tex(cx, noise::noise_2d(256, 8, 4)),
            noisevol_lq: tex(cx, noise::noise_volume(1, 5)),
            noisevol_hq: tex(cx, noise::noise_volume(4, 6)),
            stand_ins: Vec::new(),
        });
    }

    fn stand_in(&mut self, cx: &mut Cx, name: &str) -> Texture {
        let shared = self.shared.as_mut().unwrap();
        if let Some((_, t)) = shared.stand_ins.iter().find(|(n, _)| n == name) {
            return t.clone();
        }
        let img = noise::stand_in(name);
        let t = bgra_texture(cx, img.width, img.height, img.data);
        shared.stand_ins.push((name.to_string(), t.clone()));
        t
    }

    /// Run deck `which` for one frame at `size`; its composite texture.
    fn render_deck(&mut self, cx: &mut Cx2d, which: usize, size: (usize, usize)) -> Option<Texture> {
        let mut mesh = (self.mesh_x.clamp(4, 256), self.mesh_y.clamp(4, 192));
        // Stand-in images first (they need &mut self).
        let names: Vec<String> = {
            let deck = if which == 0 { &self.deck_a } else { &self.deck_b };
            let engine = deck.engine.as_ref()?;
            if engine.report.vertex_cpu.is_some() {
                // Per-vertex code on the CPU costs per vertex: MilkDrop's
                // own grid.
                mesh = (mesh.0.min(CPU_MESH.0), mesh.1.min(CPU_MESH.1));
            }
            deck.images.iter().flatten().cloned().collect()
        };
        for n in &names {
            self.stand_in(cx.cx, n);
        }
        let Self { deck_a, deck_b, draw_blur, draw_line, draw_shape, draw_fill, shared, time, frame, fps, audio_frame, .. } = self;
        let deck = if which == 0 { deck_a } else { deck_b };
        let shared = shared.as_ref()?;
        let fixed = FixedDraws { blur: draw_blur, line: draw_line, shape: draw_shape, fill: draw_fill };
        deck.render(cx, shared, fixed, size, mesh, *time, *frame, *fps, audio_frame)
    }
}

/// The warp mesh when per-vertex code runs on the CPU (MilkDrop's default).
const CPU_MESH: (usize, usize) = (48, 36);

struct FixedDraws<'a> {
    blur: &'a mut DrawMilkBlur,
    line: &'a mut DrawMilkWave,
    shape: &'a mut DrawMilkShape,
    fill: &'a mut DrawMilkFill,
}

impl MilkDeck {
    fn waves_mut(&mut self) -> [&mut DrawMilkWave; 4] {
        [&mut self.draw_wave0, &mut self.draw_wave1, &mut self.draw_wave2, &mut self.draw_wave3]
    }

    /// Compile the preset's code into this deck's shaders.
    fn load(&mut self, cx: &mut Cx, name: &str, preset: Arc<Preset>, seed: u64) {
        let started = std::time::Instant::now();
        let engine = Engine::new(preset.clone(), seed);
        let mut status = PresetStatus {
            name: name.to_string(),
            equations: engine.report.clone(),
            ..Default::default()
        };
        self.blur_levels = 0;
        self.images = [Vec::new(), Vec::new()];
        // Evaluate each preset shader and apply it to
        // the draw (which compiles it); on failure the base shader applies,
        // which is what MilkDrop runs when a preset shader fails.
        let mut errors = Vec::new();
        let apply_to = |cx: &mut Cx, target: &mut DrawQuad, kind: &str, base: &str, members: &str, errors: &mut Vec<String>| {
            cx.with_vm(|vm| {
                let value = match eval_preset_shader(vm, kind, base, members) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(format!("{kind}: {e}"));
                        match eval_preset_shader(vm, kind, base, "") {
                            Ok(v) => v,
                            Err(_) => return,
                        }
                    }
                };
                let mut scope = Scope::default();
                target.script_apply(vm, &Apply::Eval, &mut scope, value);
            });
            if let Some(e) = makepad_widgets::makepad_platform::shader_error::take() {
                errors.push(format!("{kind}: {e}"));
            }
        };
        // Per-vertex code.
        let vertex = engine.gpu_vertex().unwrap_or("").to_string();
        apply_to(cx, &mut self.draw_mesh.draw_super, "mesh", "DrawMilkMesh", &vertex, &mut errors);
        // Warp and composite shaders.
        for (kind, src, idx) in [(ShaderKind::Warp, &preset.warp_shader, 0usize), (ShaderKind::Comp, &preset.comp_shader, 1)] {
            let (members, images, blur) = match src {
                Some(src) => match hlsl::translate(src, kind) {
                    Ok(t) => {
                        let blur = t.textures.iter().filter_map(|t| if let MilkTexture::Blur(l) = t { Some(*l as usize) } else { None }).max().unwrap_or(0);
                        if idx == 0 {
                            status.warp_shader = true;
                        } else {
                            status.comp_shader = true;
                        }
                        (t.members, t.images, blur)
                    }
                    Err(e) => {
                        errors.push(format!("{} shader: {e}", if idx == 0 { "warp" } else { "comp" }));
                        (String::new(), Vec::new(), 0)
                    }
                },
                None => (String::new(), Vec::new(), 0),
            };
            self.blur_levels = self.blur_levels.max(blur);
            self.images[idx] = images;
            if idx == 0 {
                apply_to(cx, &mut self.draw_warp.draw_super, "warp", "DrawMilkWarp", &members, &mut errors);
            } else {
                apply_to(cx, &mut self.draw_comp.draw_super, "comp", "DrawMilkComp", &members, &mut errors);
            }
        }
        // Custom waves evaluated on the GPU.
        let members: Vec<String> = (0..4).map(|slot| engine.gpu_wave(slot).unwrap_or("").to_string()).collect();
        for (slot, draw) in self.waves_mut().into_iter().enumerate() {
            apply_to(cx, &mut draw.draw_super, "wave", "DrawMilkWave", &members[slot], &mut errors);
        }
        status.shader_errors = errors;
        status.load_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.status = status;
        self.engine = Some(engine);
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        cx: &mut Cx2d,
        shared: &Shared,
        fixed: FixedDraws,
        size: (usize, usize),
        mesh: (usize, usize),
        time: f64,
        frame: u64,
        fps: f64,
        audio: &AudioFrame,
    ) -> Option<Texture> {
        let engine = self.engine.as_mut()?;
        // CPU half.
        let input = FrameInput {
            time,
            fps: if fps > 0.0 { fps } else { 60.0 },
            frame,
            progress: 0.0,
            audio,
            width: size.0 as f32,
            height: size.1 as f32,
            mesh,
        };
        engine.frame(&input, &mut self.params);
        let p = &self.params;
        let uniforms = FrameUniforms {
            time: time as f32,
            fps: input.fps as f32,
            frame: frame as f32,
            progress: 0.0,
            audio: [audio.bass, audio.mid, audio.treb, audio.vol],
            audio_att: [audio.bass_att, audio.mid_att, audio.treb_att, audio.vol_att],
            aspect: aspect(size.0 as f32, size.1 as f32),
            mesh,
            pixels: size,
        };

        // Targets (taken out while rendering, put back at the end).
        if self.targets.is_none() {
            let c: &mut Cx = cx.cx;
            self.targets = Some(DeckTargets {
                main: [Target::bgra(c, "milkdrop main a"), Target::bgra(c, "milkdrop main b")],
                current: 0,
                mesh: Target::rgba_f32(c, "milkdrop mesh"),
                cpu_mesh: Texture::new_with_format(
                    c,
                    TextureFormat::VecRGBAf32 { width: 1, height: 1, data: Some(vec![0.0; 4]), updated: TextureUpdated::Full },
                ).label(c, "milkdrop/mesh"),
                blur: std::array::from_fn(|i| Target::bgra(c, &format!("milkdrop blur {i}"))),
                out: Target::bgra(c, "milkdrop out"),
                seed: Some(shared.black.clone()),
            });
        }
        let mut targets = self.targets.take().unwrap();
        let t = &mut targets;
        if let Some(seed) = self.pending_seed.take() {
            t.seed = Some(seed);
        }

        // 1. The mesh: GPU per-vertex pass, or the CPU grid uploaded.
        let mesh_texture = match &p.cpu_mesh {
            Some(grid) => {
                let (w, h) = (mesh.0 + 1, mesh.1 + 1);
                let mut data = Vec::with_capacity(w * h * 4);
                for uv in grid {
                    data.extend_from_slice(&[uv[0], uv[1], 0.0, 1.0]);
                }
                *t.cpu_mesh.get_format(cx.cx) =
                    TextureFormat::VecRGBAf32 { width: w, height: h, data: Some(data), updated: TextureUpdated::Full };
                t.cpu_mesh.clone()
            }
            None => {
                let draw = &mut self.draw_mesh;
                set_uniforms(&mut draw.draw_vars, cx, &uniforms, p);
                t.mesh.run(cx, (mesh.0 + 1, mesh.1 + 1), &[], &mut |cx, rect| {
                    draw.draw_abs(cx, rect);
                });
                t.mesh.texture.clone()
            }
        };

        // Previous picture and the previous frame's blurs (the warp reads
        // those, as in MilkDrop).
        let prev = t.seed.take().unwrap_or_else(|| t.main[t.current].texture.clone());
        let next = 1 - t.current;
        let img = |i: usize, names: &Vec<String>| -> Texture {
            names
                .get(i)
                .and_then(|n| shared.stand_ins.iter().find(|(s, _)| s == n))
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| shared.black.clone())
        };
        let warp_images: [Texture; 4] = std::array::from_fn(|i| img(i, &self.images[0]));
        let comp_images: [Texture; 4] = std::array::from_fn(|i| img(i, &self.images[1]));
        let blur_tex: [Texture; 3] = std::array::from_fn(|i| {
            if self.blur_levels > i && t.blur[i * 2 + 1].size.0 > 0 {
                t.blur[i * 2 + 1].texture.clone()
            } else {
                shared.black.clone()
            }
        });

        // 2. Warp into the other buffer, then shapes, waves, lines, borders.
        {
            let set = TextureSet {
                main: &prev,
                mesh: &mesh_texture,
                blur: [&blur_tex[0], &blur_tex[1], &blur_tex[2]],
                noise_lq: &shared.noise_lq,
                noise_lq_lite: &shared.noise_lq_lite,
                noise_mq: &shared.noise_mq,
                noise_hq: &shared.noise_hq,
                noisevol_lq: &shared.noisevol_lq,
                noisevol_hq: &shared.noisevol_hq,
                images: [&warp_images[0], &warp_images[1], &warp_images[2], &warp_images[3]],
            };
            bind_textures(&mut self.draw_warp.draw_vars, &set);
            set_uniforms(&mut self.draw_warp.draw_vars, cx, &uniforms, p);
            bind_textures(&mut fixed.shape.draw_vars, &set);
            set_uniforms(&mut fixed.shape.draw_vars, cx, &uniforms, p);
            bind_textures(&mut fixed.line.draw_vars, &set);
            set_uniforms(&mut fixed.line.draw_vars, cx, &uniforms, p);
            let Self { draw_warp, draw_wave0, draw_wave1, draw_wave2, draw_wave3, .. } = self;
            let mut waves: [&mut DrawMilkWave; 4] = [draw_wave0, draw_wave1, draw_wave2, draw_wave3];
            for draw in waves.iter_mut() {
                bind_textures(&mut draw.draw_vars, &set);
                set_uniforms(&mut draw.draw_vars, cx, &uniforms, p);
            }
            let (shape, line, fill) = (&mut *fixed.shape, &mut *fixed.line, &mut *fixed.fill);
            let inputs: &[&DrawPass] = if p.cpu_mesh.is_some() { &[] } else { &[&t.mesh.pass] };
            t.main[next].run(cx, size, inputs, &mut |cx, rect| {
                draw_warp.draw_abs(cx, rect);
                draw_shapes(cx, shape, &p.shapes, rect);
                draw_gpu_waves(cx, &mut waves, p, rect);
                for batch in &p.lines {
                    draw_lines(cx, line, batch, rect);
                }
                draw_borders(cx, fill, p, rect);
            });
        }
        t.current = next;
        let main_now = t.main[next].texture.clone();

        // 3. Blur chain of the new picture (only the levels the shaders
        // read).
        let mut src = main_now.clone();
        // The last pass rendered this frame: the warp, then each blur.
        let mut last: Option<usize> = None;
        let mut w = size.0;
        let mut h = size.1;
        for level in 0..self.blur_levels.min(3) {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            let edge = if level == 0 { p.blur1_edge_darken } else { 0.0 };
            for (pass, dir) in [(level * 2, [1.0f32, 0.0]), (level * 2 + 1, [0.0, 1.0])] {
                let blur = &mut *fixed.blur;
                blur.draw_vars.set_texture(0, &src);
                blur.draw_vars.set_uniform(cx, live_id!(u_dir), &[dir[0], dir[1], if dir[1] > 0.0 { edge } else { 0.0 }, 0.0]);
                let (done, rest) = t.blur.split_at_mut(pass);
                let input = last.map_or(&t.main[next].pass, |i| &done[i].pass);
                rest[0].run(cx, (w, h), &[input], &mut |cx, rect| {
                    blur.draw_abs(cx, rect);
                });
                src = rest[0].texture.clone();
                last = Some(pass);
            }
        }
        let blur_now: [Texture; 3] = std::array::from_fn(|i| {
            if self.blur_levels > i {
                t.blur[i * 2 + 1].texture.clone()
            } else {
                shared.black.clone()
            }
        });

        // 4. Composite.
        {
            let set = TextureSet {
                main: &main_now,
                mesh: &mesh_texture,
                blur: [&blur_now[0], &blur_now[1], &blur_now[2]],
                noise_lq: &shared.noise_lq,
                noise_lq_lite: &shared.noise_lq_lite,
                noise_mq: &shared.noise_mq,
                noise_hq: &shared.noise_hq,
                noisevol_lq: &shared.noisevol_lq,
                noisevol_hq: &shared.noisevol_hq,
                images: [&comp_images[0], &comp_images[1], &comp_images[2], &comp_images[3]],
            };
            let draw = &mut self.draw_comp;
            bind_textures(&mut draw.draw_vars, &set);
            set_uniforms(&mut draw.draw_vars, cx, &uniforms, p);
            set_comp_uniforms(&mut draw.draw_vars, cx, p);
            let input = last.map_or(&t.main[next].pass, |i| &t.blur[i].pass);
            t.out.run(cx, size, &[input], &mut |cx, rect| {
                draw.draw_abs(cx, rect);
            });
        }
        let out = t.out.texture.clone();
        self.targets = Some(targets);
        Some(out)
    }
}

fn draw_shapes(cx: &mut Cx2d, draw: &mut DrawMilkShape, shapes: &[crate::state::ShapeInstance], rect: Rect) {
    let short = rect.size.x.min(rect.size.y);
    for s in shapes {
        let radius = (s.rad.abs() as f64 * short * 0.5 + 6.0).max(1.0);
        let center = dvec2(s.x as f64 * rect.size.x, (1.0 - s.y as f64) * rect.size.y);
        let visible = s.inner[3] > 0.001 || s.outer[3] > 0.001 || s.border[3] > 0.001;
        if !visible || !radius.is_finite() || radius > 16.0 * rect.size.x.max(rect.size.y) {
            continue;
        }
        draw.sh0 = vec4(s.x, s.y, s.rad, s.ang);
        draw.sh1 = vec4(s.sides, if s.textured { 1.0 } else { 0.0 }, s.tex_zoom, s.tex_ang);
        draw.sh2 = vec4(if s.additive { 1.0 } else { 0.0 }, if s.thick { 1.0 } else { 0.0 }, 0.0, 0.0);
        draw.c_in = vec4(s.inner[0], s.inner[1], s.inner[2], s.inner[3]);
        draw.c_out = vec4(s.outer[0], s.outer[1], s.outer[2], s.outer[3]);
        draw.c_border = vec4(s.border[0], s.border[1], s.border[2], s.border[3]);
        draw.draw_abs(cx, Rect { pos: center - dvec2(radius, radius), size: dvec2(radius * 2.0, radius * 2.0) });
    }
}

fn wave_mode(draw: &mut DrawMilkWave, cx: &mut Cx2d, gpu: bool, thick: bool, dots: bool, additive: bool) {
    let half = if dots {
        if thick { 1.5 } else { 1.0 }
    } else if thick {
        1.25
    } else {
        0.75
    };
    draw.draw_vars.set_uniform(
        cx,
        live_id!(u_wave_mode),
        &[if gpu { 1.0 } else { 0.0 }, half, 0.0, if additive { 1.0 } else { 0.0 }],
    );
}

/// Segments (or dots) of a CPU-laid-out batch. NaN points break a line.
fn draw_lines(cx: &mut Cx2d, draw: &mut DrawMilkWave, batch: &LineBatch, rect: Rect) {
    wave_mode(draw, cx, false, batch.thick, batch.dots, batch.additive);
    let n = batch.points.len();
    let seg = |draw: &mut DrawMilkWave, cx: &mut Cx2d, i: usize, j: usize| {
        let (a, b) = (batch.points[i], batch.points[j]);
        if !(a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()) {
            return;
        }
        draw.a = vec4(0.0, a[0], a[1], 0.0);
        draw.b = vec4(0.0, b[0], b[1], 0.0);
        let (ca, cb) = (batch.colors[i], batch.colors[j]);
        draw.ca = vec4(ca[0], ca[1], ca[2], ca[3]);
        draw.cb = vec4(cb[0], cb[1], cb[2], cb[3]);
        draw.draw_abs(cx, rect);
    };
    if batch.dots {
        for i in 0..n {
            seg(draw, cx, i, i);
        }
    } else if batch.pairs {
        for i in (0..n.saturating_sub(1)).step_by(2) {
            seg(draw, cx, i, i + 1);
        }
    } else {
        for i in 0..n.saturating_sub(1) {
            seg(draw, cx, i, i + 1);
        }
    }
}

/// Custom waves whose per-point code runs in their wave shader.
fn draw_gpu_waves(cx: &mut Cx2d, waves: &mut [&mut DrawMilkWave; 4], params: &FrameParams, rect: Rect) {
    for w in &params.gpu_waves {
        let draw: &mut DrawMilkWave = &mut *waves[w.slot.min(3)];
        wave_mode(draw, cx, true, w.thick, w.dots, w.additive);
        draw.draw_vars.set_uniform(cx, live_id!(u_wave_rgba), &w.rgba);
        draw.draw_vars.set_uniform(cx, live_id!(u_wave_t0), &w.t[0..4]);
        draw.draw_vars.set_uniform(cx, live_id!(u_wave_t1), &w.t[4..8]);
        const Q: [LiveId; 8] = [
            live_id!(u_q0),
            live_id!(u_q1),
            live_id!(u_q2),
            live_id!(u_q3),
            live_id!(u_q4),
            live_id!(u_q5),
            live_id!(u_q6),
            live_id!(u_q7),
        ];
        for (i, id) in Q.iter().enumerate() {
            draw.draw_vars.set_uniform(cx, *id, &w.q[i * 4..i * 4 + 4]);
        }
        let n = w.inputs.len().min(MAX_WAVE_POINTS);
        let emit = |draw: &mut DrawMilkWave, cx: &mut Cx2d, i: usize, j: usize| {
            let (a, b) = (w.inputs[i], w.inputs[j]);
            draw.a = vec4(a[0], a[1], a[2], 0.0);
            draw.b = vec4(b[0], b[1], b[2], 0.0);
            draw.draw_abs(cx, rect);
        };
        if w.dots {
            for i in 0..n {
                emit(draw, cx, i, i);
            }
        } else {
            for i in 0..n.saturating_sub(1) {
                emit(draw, cx, i, i + 1);
            }
        }
    }
}

fn draw_borders(cx: &mut Cx2d, draw: &mut DrawMilkFill, p: &FrameParams, rect: Rect) {
    let short = rect.size.x.min(rect.size.y);
    let mut inset = 0.0;
    for b in [&p.outer_border, &p.inner_border] {
        let w = (b.size.clamp(0.0, 0.5) as f64) * short;
        if b.rgba[3] > 0.001 && w > 0.0 {
            draw.color = vec4(b.rgba[0], b.rgba[1], b.rgba[2], b.rgba[3]);
            let (x0, y0, x1, y1) = (inset, inset, rect.size.x - inset, rect.size.y - inset);
            for r in [
                Rect { pos: dvec2(x0, y0), size: dvec2(x1 - x0, w) },
                Rect { pos: dvec2(x0, y1 - w), size: dvec2(x1 - x0, w) },
                Rect { pos: dvec2(x0, y0 + w), size: dvec2(w, (y1 - y0 - 2.0 * w).max(0.0)) },
                Rect { pos: dvec2(x1 - w, y0 + w), size: dvec2(w, (y1 - y0 - 2.0 * w).max(0.0)) },
            ] {
                draw.draw_abs(cx, r);
            }
        }
        inset += w;
    }
}

impl MilkdropViewRef {
    pub fn set_playing(&self, cx: &mut Cx, playing: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_playing(cx, playing);
        }
    }

    pub fn push_audio(&self, samples: &[f32], channels: usize, sample_rate: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.push_audio(samples, channels, sample_rate);
        }
    }

    pub fn feed_audio(&self, samples: &[f32], sample_rate: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.feed_audio(samples, sample_rate);
        }
    }

    pub fn load_preset(&self, cx: &mut Cx, path: &Path) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_preset(cx, path);
        }
    }

    pub fn load_preset_source(&self, cx: &mut Cx, name: &str, source: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_preset_source(cx, name, source);
        }
    }

    pub fn load_parsed(&self, cx: &mut Cx, name: &str, preset: Arc<Preset>, transition: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_parsed(cx, name, preset, transition);
        }
    }

    pub fn status(&self) -> Option<PresetStatus> {
        self.borrow().map(|inner| inner.status().clone())
    }

    pub fn output_texture(&self) -> Option<Texture> {
        self.borrow().and_then(|inner| inner.output_texture())
    }

    pub fn cpu_ms(&self) -> f64 {
        self.borrow().map(|inner| inner.cpu_ms()).unwrap_or(0.0)
    }

    pub fn render_size(&self) -> (usize, usize) {
        self.borrow().map(|inner| inner.render_size()).unwrap_or((0, 0))
    }

    pub fn stage_times(&self) -> StageTimes {
        self.borrow().map(|inner| inner.stage_times()).unwrap_or_default()
    }

    pub fn set_resolution_scale(&self, cx: &mut Cx, scale: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_resolution_scale(cx, scale);
        }
    }

    pub fn set_max_pixels(&self, cx: &mut Cx, pixels: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_max_pixels(cx, pixels);
        }
    }

    pub fn set_render_pixels(&self, cx: &mut Cx, pixels: Option<(usize, usize)>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_render_pixels(cx, pixels);
        }
    }

    pub fn shown(&self) -> Option<(Texture, Texture, f32, f64)> {
        self.borrow().and_then(|inner| inner.shown())
    }
}

/// The longest a recorded frame waits for its paint before the next one is
/// recorded anyway (a pass the backend dropped must not freeze the view).
const HOLD_MAX_SECONDS: f64 = 0.25;

/// The part of a picture `picture` wide over high that fills a quad
/// `quad` wide over high, centred (the blit's `u_uv`: offset, size).
pub fn cover_uv(picture: f64, quad: f64) -> [f32; 4] {
    if !(picture > 0.0 && quad > 0.0) || (picture - quad).abs() < 1e-3 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    if picture > quad {
        let w = (quad / picture) as f32;
        [(1.0 - w) * 0.5, 0.0, w, 1.0]
    } else {
        let h = (picture / quad) as f32;
        [0.0, (1.0 - h) * 0.5, 1.0, h]
    }
}
