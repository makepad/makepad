//! `VjFxView` — the VJ-effect host widget.
//!
//! A normal makepad widget (registered like every other) that owns one
//! offscreen 3D pass + an optional post chain, exactly the `VjMeshView`
//! embedding pattern. Load a document with [`VjFxView::set_effect_source`];
//! the widget then runs it forever off its own `NextFrame` clock.
//!
//! Per frame (the ONLY per-frame work — the splash interpreter never runs
//! here except the emitters engine's bounded `frame:` tick):
//!   1. advance clocks, build the [`Signals`] vector
//!   2. evaluate the document's compiled bindings into uniform values
//!   3. run the engine's `regen` (usually a no-op) + the emitters tick
//!   4. draw the scene pass, then the post chain, then present
//!
//! Two hosting modes, mirroring the mesh slot:
//!   * `composite: true` (default, the gallery): the final texture is drawn
//!     onto the widget rect.
//!   * `composite: false` (VJ slot mode): the host pulls
//!     [`VjFxView::output_texture`] every frame and composites it itself
//!     (`VideoProgram` style). Re-fetch per frame: a feedback chain
//!     ping-pongs its output texture identity.
//!
//! Beat: the host feeds [`VjFxView::set_beat`] from its clock (the VJ's
//! `BeatClock::position_at`) and [`VjFxView::set_signals`] with audio
//! energies; without a host the widget free-runs at [`VjFxView::set_bpm`]
//! (default 120) so effects always pulse.

use super::doc::{EffectDoc, GrowMode, StageCfg};
use super::engines::{CamPose, EmitterInst, Engine, ShaderKind};
use super::engines_domino::DrawVjFxDomino;
use super::engines_firefly::DrawVjFxFirefly;
use super::engines_flock::DrawVjFxFlock;
use super::engines_harmonograph::DrawVjFxHarmono;
use super::engines_tiles::DrawVjFxTiles;
use super::expr::Signals;
use super::mesh::FxMesh;
use super::post::{PostChain, PostDraws, ResolvedStage};
use super::shaders::*;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.VjFxViewBase = #(VjFxView::register_widget(vm))
    mod.widgets.VjFxView = set_type_default() do mod.widgets.VjFxViewBase{
        width: Fill
        height: Fill
    }
}

/// Offscreen resolution for slot mode (independent of the widget rect, the
/// mesh slot's convention).
const SLOT_PASS: DVec2 = dvec2(1280.0, 720.0);

/// Instruction budget for the emitters engine's per-frame tick.
const TICK_INSTRUCTION_LIMIT: usize = 200_000;

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjFxView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    // Mesh-family shaders (one per engine family).
    #[live]
    draw_mesh: DrawVjFxMesh,
    #[live]
    draw_terrain: DrawVjFxTerrain,
    #[live]
    draw_ribbon: DrawVjFxRibbon,
    #[live]
    draw_tunnel: DrawVjFxTunnel,
    #[live]
    draw_particles: DrawVjFxParticles,
    #[live]
    draw_emitter: DrawVjFxEmitter,
    #[live]
    draw_firefly: DrawVjFxFirefly,
    #[live]
    draw_harmono: DrawVjFxHarmono,
    #[live]
    draw_domino: DrawVjFxDomino,
    #[live]
    draw_tiles: DrawVjFxTiles,
    #[live]
    draw_flock: DrawVjFxFlock,
    // Post + present quads.
    #[live]
    draw_feedback: DrawVjFxFeedback,
    #[live]
    draw_down: DrawVjFxDown,
    #[live]
    draw_up: DrawVjFxUp,
    #[live]
    draw_bloom_mix: DrawVjFxBloomMix,
    #[live]
    draw_tilt_mix: DrawVjFxTiltMix,
    #[live]
    draw_warp: DrawVjFxWarp,
    #[live]
    draw_present: DrawVjFxPresent,

    /// Draw the final texture onto the widget rect. Slot views turn this
    /// off and composite `output_texture()` themselves.
    #[live(true)]
    composite: bool,

    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,

    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust]
    next_frame: NextFrame,

    #[rust]
    doc: Option<EffectDoc>,
    #[rust]
    mesh: FxMesh,
    #[rust]
    geometry: Option<Geometry>,
    #[rust]
    post: PostChain,
    #[rust]
    out_texture: Option<Texture>,
    /// Slot-mode pass size override (default [`SLOT_PASS`]): a thumbnail
    /// host renders small, the program slot renders full size.
    #[rust]
    slot_size: Option<DVec2>,
    /// Mesh bounds captured at build (l-systems auto-frame from them).
    #[rust]
    bounds: Option<(Vec3f, Vec3f)>,

    // Clock. `local_time` is document time (already speed-scaled).
    #[rust]
    last_time: Option<f64>,
    #[rust]
    local_time: f64,
    #[rust]
    frame_dt: f32,
    /// Continuous beat position; advanced by the internal clock unless the
    /// host feeds `set_beat`.
    #[rust]
    beat_pos: f64,
    #[rust(120.0f64)]
    bpm: f64,
    #[rust]
    host_beat: bool,
    /// Audio levels fed by the host ([energy, bass, mid, high]); zero until
    /// something feeds them (the contract exists ahead of the wiring).
    #[rust]
    audio: [f32; 4],

    #[rust]
    input0: Option<Texture>,
    /// The built-in ANIMATED stand-in for input 0: every texture-input
    /// effect renders something meaningful with no channel bound (gallery,
    /// thumbnails, empty channels). Beat-aware so even the dummy pulses.
    #[rust]
    fallback_input: Option<Texture>,
    #[rust]
    fallback_data: Vec<u32>,

    #[rust(true)]
    live: bool,
    #[rust]
    paused: bool,

    /// Status line: what loaded, its cost, any document warnings.
    #[rust]
    pub status: String,
    /// CPU cost of the last mesh regen (ms).
    #[rust]
    pub regen_ms: f32,
    /// Cost of the last frame tick (ms) + instructions used.
    #[rust]
    pub tick_ms: f32,
    #[rust]
    pub tick_instructions: usize,
    /// Last frame-tick error (kept loud in the status, never a stall).
    #[rust]
    pub tick_error: Option<String>,
    /// Which shader family currently carries document hook overrides, so
    /// the next load can restore that shader to its registered base first.
    #[rust]
    hooked_kind: Option<ShaderKind>,
}

impl VjFxView {
    /// Evaluate + load an effect document. `key` names the document slot
    /// (stable per file so re-loading replaces instead of accreting).
    /// Returns the effect name, or the evaluation error.
    pub fn set_effect_source(
        &mut self,
        cx: &mut Cx,
        key: &str,
        source: &str,
    ) -> Result<String, String> {
        let doc = cx.with_vm(|vm| EffectDoc::parse(vm, key, source))?;
        let name = doc.name.clone();
        // Post chain rebuilt for the new stage list (textures/passes only
        // allocated for what the doc asks for).
        self.post.configure(cx, &doc.stages);
        // A previous document's hook overrides must not leak into this one:
        // restore that shader family to its registered base first.
        if let Some(kind) = self.hooked_kind.take() {
            self.apply_shader_value(cx, kind, None);
        }
        // Apply the document's shader hooks (a SUBCLASS object of the base
        // shader — validated at parse): recompiles the family's draw shader
        // with the doc's fn overrides.
        if let Some(hooks) = &doc.shader_hooks {
            let value: ScriptValue = hooks.as_object().into();
            let kind = doc.engine.shader_kind();
            self.apply_shader_value(cx, kind, Some(value));
            self.hooked_kind = Some(kind);
        }
        let warn = if doc.warnings.is_empty() {
            String::new()
        } else {
            format!(" — {}", doc.warnings.join("; "))
        };
        self.status = format!("{} [{}]{}", name, doc.engine.name(), warn);
        self.doc = Some(doc);
        self.bounds = None;
        self.local_time = 0.0;
        self.last_time = None;
        self.tick_error = None;
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
        Ok(name)
    }

    pub fn clear_effect(&mut self, cx: &mut Cx) {
        self.doc = None;
        self.status.clear();
        self.area.redraw(cx);
    }

    /// Apply `value` onto the family's draw shader; `None` re-applies the
    /// REGISTERED BASE definition (`mod.draw.DrawVjFx…`), restoring any
    /// hook overrides a previous document compiled in.
    fn apply_shader_value(&mut self, cx: &mut Cx, kind: ShaderKind, value: Option<ScriptValue>) {
        cx.with_vm(|vm| {
            let value = match value {
                Some(v) => v,
                None => {
                    let name = match kind {
                        ShaderKind::Mesh => live_id!(DrawVjFxMesh),
                        ShaderKind::Terrain => live_id!(DrawVjFxTerrain),
                        ShaderKind::Ribbon => live_id!(DrawVjFxRibbon),
                        ShaderKind::Tunnel => live_id!(DrawVjFxTunnel),
                        ShaderKind::Particles => live_id!(DrawVjFxParticles),
                        ShaderKind::Emitters => live_id!(DrawVjFxEmitter),
                        ShaderKind::Firefly => live_id!(DrawVjFxFirefly),
                        ShaderKind::Harmono => live_id!(DrawVjFxHarmono),
                        ShaderKind::Domino => live_id!(DrawVjFxDomino),
                        ShaderKind::Tiles => live_id!(DrawVjFxTiles),
                        ShaderKind::Flock => live_id!(DrawVjFxFlock),
                    };
                    let modules = vm.bx.heap.modules;
                    let draw_mod = vm.bx.heap.value(modules, live_id!(draw).into(), NoTrap);
                    let Some(dm) = draw_mod.as_object() else { return };
                    let base = vm.bx.heap.value(dm, name.into(), NoTrap);
                    if base.is_err() || base.is_nil() {
                        return;
                    }
                    base
                }
            };
            let mut scope = Scope::default();
            match kind {
                ShaderKind::Mesh => {
                    self.draw_mesh.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Terrain => {
                    self.draw_terrain.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Ribbon => {
                    self.draw_ribbon.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Tunnel => {
                    self.draw_tunnel.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Particles => {
                    self.draw_particles.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Emitters => {
                    self.draw_emitter.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Firefly => {
                    self.draw_firefly.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Harmono => {
                    self.draw_harmono.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Domino => {
                    self.draw_domino.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Tiles => {
                    self.draw_tiles.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Flock => {
                    self.draw_flock.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
            }
        });
    }

    /// The final output (scene, or last post stage). Re-fetch every frame in
    /// slot mode — feedback stages ping-pong the texture identity.
    pub fn output_texture(&self) -> Option<Texture> {
        self.out_texture.clone()
    }

    pub fn has_effect(&self) -> bool {
        self.doc.is_some()
    }

    /// The loaded document's `input0` request ("test", …), for the host to
    /// decide what to bind.
    pub fn doc_input0(&self) -> Option<&str> {
        self.doc.as_ref().and_then(|d| d.input0.as_deref())
    }

    /// Feed the host's beat clock (continuous beat position). Call once per
    /// frame; the widget derives phase and pulse.
    pub fn set_beat(&mut self, beat_pos: f64, bpm: f64) {
        self.beat_pos = beat_pos;
        self.bpm = bpm.clamp(20.0, 300.0);
        self.host_beat = true;
    }

    /// Free-running tempo when no host clock feeds `set_beat`.
    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(20.0, 300.0);
    }

    /// Audio levels for the binding signals: [energy, bass, mid, high],
    /// each 0..1.
    pub fn set_signals(&mut self, audio: [f32; 4]) {
        self.audio = audio;
    }

    /// Texture input 0 (the "extra slot" — a still, or the channel's main
    /// content in effect-pass mode).
    pub fn set_input_texture(&mut self, index: usize, texture: Option<Texture>) {
        if index == 0 {
            self.input0 = texture;
        }
    }

    pub fn set_live(&mut self, cx: &mut Cx, live: bool) {
        if self.live == live {
            return;
        }
        self.live = live;
        if live {
            self.last_time = None;
            self.next_frame = cx.new_next_frame();
            self.area.redraw(cx);
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Slot-mode offscreen resolution (composite off). Thumbnail hosts set
    /// a small one; the program slot keeps the [`SLOT_PASS`] default.
    pub fn set_slot_size(&mut self, size: DVec2) {
        self.slot_size = Some(DVec2 {
            x: size.x.clamp(8.0, 4096.0),
            y: size.y.clamp(8.0, 4096.0),
        });
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        self.next_frame = cx.new_next_frame();
    }

    const FALLBACK_SIZE: usize = 192;

    /// The animated procedural stand-in for input 0: a colorful gradient
    /// sweep with a traveling ring and grid, brightening on the beat.
    /// ~37k pixels per update — negligible, and only paid when an effect
    /// actually samples an unbound input.
    fn update_fallback_input(&mut self, cx: &mut Cx, time: f32, pulse: f32) -> Texture {
        const W: usize = VjFxView::FALLBACK_SIZE;
        self.fallback_data.resize(W * W, 0);
        let boost = 1.0 + pulse * 0.4;
        let sweep = time * 0.35;
        let (rx, ry) = ((time * 0.5).cos() * 0.22, (time * 0.37).sin() * 0.22);
        for y in 0..W {
            let v = y as f32 / W as f32;
            for x in 0..W {
                let u = x as f32 / W as f32;
                // Hue sweep diagonal.
                let h = (u * 0.8 + v * 0.5 + sweep).fract() * std::f32::consts::TAU;
                let mut r = 0.5 + 0.5 * h.sin();
                let mut g = 0.5 + 0.5 * (h + 2.094).sin();
                let mut b = 0.5 + 0.5 * (h + 4.188).sin();
                // A traveling ring.
                let dx = u - 0.5 - rx;
                let dy = v - 0.5 - ry;
                let d = (dx * dx + dy * dy).sqrt();
                if (d - 0.16).abs() < 0.025 {
                    r = 1.0;
                    g = 1.0;
                    b = 1.0;
                }
                // Grid lines.
                if x % 24 == 0 || y % 24 == 0 {
                    r *= 0.45;
                    g *= 0.45;
                    b *= 0.45;
                }
                let (r8, g8, b8) = (
                    ((r * boost).min(1.0) * 255.0) as u32,
                    ((g * boost).min(1.0) * 255.0) as u32,
                    ((b * boost).min(1.0) * 255.0) as u32,
                );
                self.fallback_data[y * W + x] = 0xff00_0000 | (r8 << 16) | (g8 << 8) | b8;
            }
        }
        match &self.fallback_input {
            Some(tex) => {
                // set_data_u32 takes the Vec; keep our copy by cloning (37k
                // u32 — trivial against the pixels we just wrote).
                tex.set_data_u32(cx, W, W, self.fallback_data.clone());
                tex.clone()
            }
            None => {
                let tex = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: W,
                        height: W,
                        data: Some(self.fallback_data.clone()),
                        updated: TextureUpdated::Full,
                    },
                );
                self.fallback_input = Some(tex.clone());
                tex
            }
        }
    }

    /// This frame's signal vector for the binding layer.
    fn signals(&self) -> Signals {
        let doc = self.doc.as_ref();
        let beat_rate = doc.map(|d| d.beat_rate).unwrap_or(1.0) as f64;
        let bar_beats = doc.map(|d| d.bar_beats).unwrap_or(4.0) as f64;
        let phase = (self.beat_pos * beat_rate).fract() as f32;
        let mut s = [0.0f32; super::expr::SIG_COUNT];
        s[Signals::TIME] = self.local_time as f32;
        s[Signals::DT] = self.frame_dt;
        s[Signals::BEAT] = self.beat_pos as f32;
        s[Signals::PHASE] = phase;
        s[Signals::BAR] = (self.beat_pos / bar_beats).fract() as f32;
        s[Signals::BPM] = self.bpm as f32;
        s[Signals::PULSE] = (1.0 - phase).powi(3);
        s[Signals::ENERGY] = self.audio[0];
        s[Signals::BASS] = self.audio[1];
        s[Signals::MID] = self.audio[2];
        s[Signals::HIGH] = self.audio[3];
        Signals(s)
    }

    /// The camera: engine-authored (tunnel) or the orbit rig. L-systems
    /// auto-frame from their mesh bounds; other engines have config-derived
    /// defaults; the document's cam_* keys win over everything.
    fn camera(&self, time: f32) -> CamPose {
        let Some(doc) = &self.doc else {
            return CamPose { eye: vec3f(0.0, 2.0, 8.0), target: Vec3f::default(), fov: 50.0 };
        };
        if let Some(pose) = doc.engine.camera(time) {
            return pose;
        }
        // Config-derived defaults per engine family.
        let (mut target, mut dist, mut height) = match &doc.engine {
            Engine::Particles(e) => (
                vec3f(0.0, 0.0, 0.0),
                e.cfg.spread * 2.2,
                e.cfg.spread * 0.25,
            ),
            Engine::Heightmap(e) => (
                vec3f(0.0, e.cfg.height * 0.45, -e.cfg.size * 0.2),
                e.cfg.size * 0.42,
                e.cfg.height * 1.15 + 1.2,
            ),
            Engine::Metaballs(e) => {
                (vec3f(0.0, 0.0, 0.0), e.cfg.extent * 2.6, e.cfg.extent * 0.5)
            }
            Engine::Ribbons(e) => (vec3f(0.0, 0.0, 0.0), e.cfg.bound * 1.7, e.cfg.bound * 0.3),
            Engine::Grass(e) => {
                (vec3f(0.0, e.cfg.height * 0.6, 0.0), e.cfg.area * 0.95, e.cfg.area * 0.3)
            }
            Engine::Firefly(e) => {
                (vec3f(0.0, 0.9, 0.0), e.cfg.area.clamp(2.0, 60.0) * 1.05, e.cfg.area.clamp(2.0, 60.0) * 0.28)
            }
            Engine::Harmono(_) => (vec3f(0.0, 0.0, 0.0), 8.8, 1.4),
            Engine::Domino(e) => (vec3f(0.0, 0.0, 0.0), e.extent * 1.9, e.extent * 0.95),
            Engine::Flock(e) => {
                let b = if e.cfg.bound.is_finite() { e.cfg.bound.clamp(1.0, 40.0) } else { 6.0 };
                (vec3f(0.0, 0.0, 0.0), b * 2.0, b * 0.32)
            }
            Engine::Emitters(_) => (vec3f(0.0, 2.0, 0.0), 16.0, 4.0),
            _ => (vec3f(0.0, 1.2, 0.0), 7.0, 1.8),
        };
        // L-systems: frame the actual plant.
        if matches!(doc.engine, Engine::Lsystem(_)) {
            if let Some((min, max)) = self.bounds {
                let center = (min + max) * 0.5;
                let ext = max - min;
                let radius =
                    (ext.x * ext.x + ext.y * ext.y + ext.z * ext.z).sqrt().max(0.2) * 0.5;
                target = center;
                dist = radius * 2.1;
                height = center.y + radius * 0.35;
            }
        }
        if let Some(d) = doc.cam.dist {
            dist = d;
        }
        let height = doc.cam.height.unwrap_or(height);
        // Heightmap: the flythrough look is a fixed camera at the corridor
        // mouth; orbit only when the doc explicitly asks.
        if matches!(doc.engine, Engine::Heightmap(_)) && doc.cam.orbit <= 0.121 {
            return CamPose {
                eye: vec3f(0.0, height, dist),
                target: vec3f(target.x, target.y, target.z),
                fov: doc.cam.fov,
            };
        }
        let yaw = time * doc.cam.orbit;
        CamPose {
            eye: vec3f(target.x + yaw.sin() * dist, height, target.z + yaw.cos() * dist),
            target,
            fov: doc.cam.fov,
        }
    }

    /// The emitters engine's per-frame tick: call the document's `frame:`
    /// function with a snapshot object, read back an array of spawn
    /// objects. Bounded by an instruction limit — a runaway script clips
    /// loudly (status) and never stalls the frame.
    fn run_frame_tick(&mut self, cx: &mut Cx) {
        let Some(doc) = &self.doc else { return };
        let Some(frame_fn) = &doc.frame_fn else { return };
        if !matches!(doc.engine, Engine::Emitters(_)) {
            return;
        }
        let fn_obj = frame_fn.as_object();
        let sig = self.signals();
        let emitter_count = match &doc.engine {
            Engine::Emitters(e) => e.emitters.len(),
            _ => 0,
        };
        let t0 = std::time::Instant::now();
        let (spawns, errors, used) = cx.with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            // Input snapshot: what the tick can see.
            let input = vm.bx.heap.new_object();
            let fields: [(LiveId, f64); 8] = [
                (live_id!(time), sig.0[Signals::TIME] as f64),
                (live_id!(dt), sig.0[Signals::DT] as f64),
                (live_id!(beat), sig.0[Signals::BEAT] as f64),
                (live_id!(phase), sig.0[Signals::PHASE] as f64),
                (live_id!(bar), sig.0[Signals::BAR] as f64),
                (live_id!(pulse), sig.0[Signals::PULSE] as f64),
                (live_id!(energy), sig.0[Signals::ENERGY] as f64),
                (live_id!(emitters), emitter_count as f64),
            ];
            for (key, v) in fields {
                vm.bx.heap.set_value_def(input, key.into(), ScriptValue::from_f64(v));
            }
            let args_obj = vm.bx.heap.new_object();
            vm.bx.heap.set_object_storage_vec2(args_obj);
            vm.bx.heap.vec_push_unchecked(args_obj, NIL, input.into());
            let ret = vm.with_instruction_limit(TICK_INSTRUCTION_LIMIT, |vm| {
                vm.call_with_args_object_with_me(fn_obj.into(), args_obj, NIL)
            });
            let used = vm.last_limit_consumed();
            // Read the returned spawn list BEFORE releasing anything.
            let mut spawns: Vec<EmitterInst> = Vec::new();
            if let Some(_arr_or_obj) = Some(&ret) {
                let len = if let Some(a) = ret.as_array() {
                    vm.bx.heap.array_len(a)
                } else if let Some(o) = ret.as_object() {
                    vm.bx.heap.vec_len(o)
                } else {
                    0
                };
                for index in 0..len.min(64) {
                    let item = if let Some(a) = ret.as_array() {
                        vm.bx.heap.array_index(a, index, NoTrap)
                    } else if let Some(o) = ret.as_object() {
                        vm.bx.heap.vec_value(o, index, NoTrap)
                    } else {
                        NIL
                    };
                    let Some(obj) = item.as_object() else { continue };
                    let mut r = super::doc::Reader { vm, obj, warnings: Vec::new() };
                    let style = match r.string(live_id!(kind)).as_deref() {
                        Some("jet") | Some("fountain") => 1.0,
                        Some("sparkle") => 2.0,
                        Some("ring") => 3.0,
                        _ => 0.0,
                    };
                    let slot = {
                        let v = r.f32(live_id!(slot), -1.0);
                        (v >= 0.0).then_some(v as u32)
                    };
                    let spawn = EmitterInst {
                        pos: r.vec3(live_id!(pos), vec3f(0.0, 0.0, 0.0)),
                        vel: r.vec3(live_id!(vel), vec3f(0.0, 0.0, 0.0)),
                        age: 0.0,
                        life: r.f32(live_id!(life), 2.5).clamp(0.05, 30.0),
                        speed: r.f32(live_id!(speed), 4.0),
                        seed: r.f32(live_id!(seed), (index as f32) * 7.7),
                        size: r.f32(live_id!(size), 0.12),
                        style,
                        fraction: r.f32(live_id!(fraction), 1.0).clamp(0.02, 1.0),
                        gravity: r.f32(live_id!(gravity), 1.0),
                        stagger: r.f32(live_id!(stagger), 0.0).max(0.0),
                        spread: r.f32(live_id!(spread), 1.0),
                        col_a: r.color(live_id!(color), vec4(1.0, 0.8, 0.4, 1.0)),
                        col_b: r.color(live_id!(color2), vec4(1.0, 0.3, 0.1, 1.0)),
                        slot,
                    };
                    spawns.push(spawn);
                }
            }
            vm.release_transient(args_obj.into());
            vm.release_transient(input.into());
            (spawns, vm.take_errors(), used)
        });
        self.tick_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.tick_instructions = used;
        if !errors.is_empty() {
            self.tick_error = Some(errors.join("; "));
        }
        if let Some(doc) = self.doc.as_mut() {
            if let Engine::Emitters(e) = &mut doc.engine {
                for s in spawns {
                    e.spawn(s);
                }
            }
        }
    }

    /// Resolve the animatable stage parameters for this frame.
    fn resolve_stages(&self, sig: &Signals) -> Vec<ResolvedStage> {
        let Some(doc) = &self.doc else { return Vec::new() };
        doc.stages
            .iter()
            .map(|s| {
                let p = match s {
                    StageCfg::Feedback { amount, zoom, rotate, dim } => [
                        amount.value(sig).clamp(0.0, 1.0),
                        zoom.value(sig),
                        rotate.value(sig),
                        dim.value(sig).clamp(0.0, 1.0),
                    ],
                    StageCfg::Bloom { threshold, strength, .. } => {
                        [threshold.value(sig).clamp(0.0, 1.0), strength.value(sig), 0.0, 0.0]
                    }
                    StageCfg::Blur { .. } => [0.0; 4],
                    StageCfg::Tiltshift { focus, width, .. } => {
                        [focus.value(sig).clamp(0.0, 1.0), width.value(sig).max(0.01), 0.0, 0.0]
                    }
                    StageCfg::Warp { p1, p2, .. } => {
                        [p1.value(sig), p2.value(sig), 0.0, 0.0]
                    }
                };
                ResolvedStage { p }
            })
            .collect()
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, pass_rect: Rect, sig: &Signals) {
        let Some(doc) = self.doc.as_mut() else { return };
        let time = sig.0[Signals::TIME];
        let beat = sig.0[Signals::BEAT];
        let phase = sig.0[Signals::PHASE];
        let pulse = sig.0[Signals::PULSE] * doc.beat_pulse.value(sig);
        let growth = match doc.grow {
            GrowMode::Off => 1.3,
            GrowMode::Loop => ((self.beat_pos / doc.grow_beats as f64).fract() as f32) * 1.3,
            GrowMode::PingPong => {
                let t = ((self.beat_pos / doc.grow_beats as f64).fract() as f32) * 2.0;
                (if t > 1.0 { 2.0 - t } else { t }) * 1.2
            }
        };

        // CPU path: regen if the engine needs it (static engines: once).
        let t0 = std::time::Instant::now();
        let dirty = doc.engine.regen(self.frame_dt, time, phase, &mut self.mesh);
        if dirty {
            self.regen_ms = t0.elapsed().as_secs_f32() * 1000.0;
            if self.bounds.is_none() && !self.mesh.verts.is_empty() {
                let floats = super::mesh::VERT_FLOATS;
                let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
                let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
                for v in self.mesh.verts.chunks(floats) {
                    min.x = min.x.min(v[0]);
                    min.y = min.y.min(v[1]);
                    min.z = min.z.min(v[2]);
                    max.x = max.x.max(v[0]);
                    max.y = max.y.max(v[1]);
                    max.z = max.z.max(v[2]);
                }
                self.bounds = Some((min, max));
            }
            let geometry = self.geometry.get_or_insert_with(|| Geometry::new(cx.cx));
            self.mesh.upload_recycle(cx.cx, geometry);
        }
        let Some(geometry) = &self.geometry else { return };
        let geometry_id = geometry.geometry_id();

        // Camera → pass uniforms.
        let cam = self.camera(time);
        let aspect = (pass_rect.size.x / pass_rect.size.y).max(0.001) as f32;
        let scene_state = SceneState3D {
            time: self.local_time,
            camera_pos: cam.eye,
            view: Mat4f::look_at(cam.eye, cam.target, vec3f(0.0, 1.0, 0.0)),
            projection: Mat4f::perspective(cam.fov, aspect, 0.05, 300.0),
            viewport_rect: pass_rect,
        };
        fx_set_pass_camera(cx.cx, &self.pass, &scene_state);

        let (eng_u, user, anim, fog, palette) = {
            let doc = self.doc.as_ref().unwrap();
            (
                doc.engine.uniforms(),
                vec4(
                    doc.params[0].value(sig),
                    doc.params[1].value(sig),
                    doc.params[2].value(sig),
                    doc.params[3].value(sig),
                ),
                vec4(
                    doc.sway.value(sig),
                    doc.sway_freq.value(sig),
                    growth,
                    doc.twist.value(sig),
                ),
                vec4(doc.fog.value(sig).max(0.0), doc.glow.value(sig), 0.0, 0.0),
                doc.palette,
            )
        };
        let time_beat = vec4(time, beat, phase, pulse);
        let sig_v = vec4(
            sig.0[Signals::BAR],
            sig.0[Signals::BPM],
            sig.0[Signals::ENERGY],
            sig.0[Signals::DT],
        );

        // Texture input 0, with the animated fallback when nothing is bound.
        let input0 = match self.input0.clone() {
            Some(tex) => Some(tex),
            None => Some(self.update_fallback_input(cx.cx, time, pulse)),
        };
        let doc = self.doc.as_ref().unwrap();
        let cx3d = &mut Cx3d::new(cx.cx);
        self.draw_list.begin_always(cx3d);
        cx3d.cx.draw_lists[self.draw_list.id()].draw_list_uniforms.view_transform =
            Mat4f::identity();

        macro_rules! draw_engine {
            ($draw:expr) => {{
                let d = $draw;
                d.time_beat = time_beat;
                d.sig = sig_v;
                d.user = user;
                d.anim = anim;
                d.shape = eng_u.shape;
                d.flow = eng_u.flow;
                d.col_bg = palette[0];
                d.col_a = palette[1];
                d.col_b = palette[2];
                d.col_c = palette[3];
                d.fog = fog;
                d.draw_vars.geometry_id = Some(geometry_id);
                if d.draw_vars.can_instance() {
                    let new_area = cx3d.add_instance(&d.draw_vars);
                    d.draw_vars.area = cx3d.update_area_refs(d.draw_vars.area, new_area);
                }
            }};
        }
        match doc.engine.shader_kind() {
            ShaderKind::Mesh => draw_engine!(&mut self.draw_mesh),
            ShaderKind::Terrain => {
                if let Some(tex) = &input0 {
                    self.draw_terrain.draw_vars.set_texture(0, tex);
                }
                draw_engine!(&mut self.draw_terrain)
            }
            ShaderKind::Ribbon => draw_engine!(&mut self.draw_ribbon),
            ShaderKind::Tunnel => draw_engine!(&mut self.draw_tunnel),
            ShaderKind::Firefly => draw_engine!(&mut self.draw_firefly),
            ShaderKind::Harmono => draw_engine!(&mut self.draw_harmono),
            ShaderKind::Domino => draw_engine!(&mut self.draw_domino),
            ShaderKind::Tiles => {
                if let Some(tex) = &input0 {
                    self.draw_tiles.draw_vars.set_texture(0, tex);
                }
                draw_engine!(&mut self.draw_tiles)
            }
            ShaderKind::Flock => draw_engine!(&mut self.draw_flock),
            ShaderKind::Particles => {
                if let Some(tex) = &input0 {
                    self.draw_particles.draw_vars.set_texture(0, tex);
                }
                draw_engine!(&mut self.draw_particles)
            }
            ShaderKind::Emitters => {
                if let Engine::Emitters(e) = &doc.engine {
                    let d = &mut self.draw_emitter;
                    d.time_beat = time_beat;
                    d.sig = sig_v;
                    d.draw_vars.geometry_id = Some(geometry_id);
                    for em in &e.emitters {
                        d.e_origin = vec4(em.pos.x, em.pos.y, em.pos.z, em.age);
                        d.e_vel = vec4(em.vel.x, em.vel.y, em.vel.z, em.life);
                        d.e_par = vec4(em.speed, em.seed, em.size, em.style);
                        d.e_par2 = vec4(em.fraction, em.gravity, em.stagger, em.spread);
                        d.e_col_a = em.col_a;
                        d.e_col_b = em.col_b;
                        if d.draw_vars.can_instance() {
                            let new_area = cx3d.add_instance(&d.draw_vars);
                            d.draw_vars.area =
                                cx3d.update_area_refs(d.draw_vars.area, new_area);
                        }
                    }
                }
            }
        }
        self.draw_list.end(cx3d);
    }
}

/// `set_pass_camera`, self-contained (the render lib has an identical one;
/// duplicated here so the effects module depends only on makepad_widgets).
fn fx_set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    let camera_inv = scene.view.invert();
    let pass_uniforms = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    pass_uniforms.camera_projection = scene.projection;
    pass_uniforms.camera_projection_r = scene.projection;
    pass_uniforms.camera_view = scene.view;
    pass_uniforms.camera_view_r = scene.view;
    pass_uniforms.depth_projection = scene.projection;
    pass_uniforms.depth_projection_r = scene.projection;
    pass_uniforms.depth_view = scene.view;
    pass_uniforms.depth_view_r = scene.view;
    pass_uniforms.camera_inv = camera_inv;
    pass_uniforms.camera_inv_r = camera_inv;
}

impl WidgetNode for VjFxView {
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

impl Widget for VjFxView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && self.live {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            let dt = (time - last).clamp(0.0, 0.1);
            if !self.paused {
                if let Some(doc) = &self.doc {
                    self.local_time += dt * doc.speed as f64;
                    self.frame_dt = (dt * doc.speed as f64) as f32;
                }
                if !self.host_beat {
                    self.beat_pos += dt * self.bpm / 60.0;
                }
                // The emitters engine's bounded per-frame script tick.
                self.run_frame_tick(cx);
            }
            if self.doc.is_some() {
                self.area.redraw(cx);
            }
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if self.doc.is_none() {
            return DrawStep::done();
        }
        self.ensure_initialized(cx.cx);
        let pass_rect = if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            Rect { pos: dvec2(0.0, 0.0), size: rect.size }
        } else {
            Rect { pos: dvec2(0.0, 0.0), size: self.slot_size.unwrap_or(SLOT_PASS) }
        };
        let sig = self.signals();
        let is_screen = matches!(self.doc.as_ref().map(|d| &d.engine), Some(Engine::Screen));

        // -- scene pass (skipped by the fullscreen `screen` engine) --------
        let chain_input = if is_screen {
            match self.input0.clone() {
                Some(tex) => tex,
                None => {
                    let t = sig.0[Signals::TIME];
                    let p = sig.0[Signals::PULSE];
                    self.update_fallback_input(cx.cx, t, p)
                }
            }
        } else {
            let clear = self.doc.as_ref().map(|d| d.palette[0]).unwrap_or_default();
            self.pass.set_size(cx, pass_rect.size);
            self.pass.set_color_texture(
                cx,
                &self.color_texture,
                DrawPassClearColor::ClearWith(clear),
            );
            self.pass.set_depth_texture(
                cx,
                &self.depth_texture,
                DrawPassClearDepth::ClearWith(1.0),
            );
            cx.make_child_pass(&self.pass);
            cx.begin_pass(&self.pass, None);
            self.pass.set_size(cx, pass_rect.size);
            self.draw_scene(cx, pass_rect, &sig);
            cx.end_pass(&self.pass);
            self.color_texture.clone()
        };

        // -- post chain ----------------------------------------------------
        let final_texture = if self.post.is_empty() {
            chain_input
        } else {
            let resolved = self.resolve_stages(&sig);
            let beat = [
                sig.0[Signals::BEAT],
                sig.0[Signals::PHASE],
                sig.0[Signals::PULSE],
                sig.0[Signals::TIME],
            ];
            self.post.run(
                cx,
                chain_input,
                pass_rect.size,
                &resolved,
                beat,
                &mut PostDraws {
                    feedback: &mut self.draw_feedback,
                    down: &mut self.draw_down,
                    up: &mut self.draw_up,
                    bloom_mix: &mut self.draw_bloom_mix,
                    tilt_mix: &mut self.draw_tilt_mix,
                    warp: &mut self.draw_warp,
                },
            )
        };
        self.out_texture = Some(final_texture.clone());

        // -- present -------------------------------------------------------
        if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            self.draw_present.draw_vars.set_texture(0, &final_texture);
            self.draw_present.draw_abs(cx, rect);
            self.area = self.draw_present.area();
            if !is_screen {
                cx.set_pass_area(&self.pass, self.area);
            }
        }
        DrawStep::done()
    }
}
