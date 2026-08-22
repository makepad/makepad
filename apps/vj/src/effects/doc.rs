//! Effect-document evaluation: a `.splash` source in, an [`EffectDoc`] out.
//!
//! The document is a makepad-script source whose LAST EXPRESSION is one flat
//! object (see the module docs in `mod.rs` for the full key reference — that
//! reference is the contract an LLM authors against). Evaluation happens at
//! LOAD time; the only per-frame script pieces are the compiled binding
//! expressions (`expr.rs`, nanoseconds) and — for the emitters engine — the
//! document's bounded `frame:` tick function (hosted like a game tick).
//!
//! Reading is forgiving by design: a missing key is a default, a wrong-typed
//! key is a default plus a warning collected on the doc (surfaced in the
//! widget status), and unknown keys are ignored. An AI-authored document
//! should degrade, not vanish.

use super::engines::*;
use super::expr::{Animatable, Expr};
use makepad_widgets::makepad_script::numeric::NumericValue;
use makepad_widgets::*;

/// Prepended to every document before evaluation: std + the pod type names
/// (`float`, `vec2`… live in `mod.pod`) + math + the shader io helpers, so
/// `shader:` hook functions can carry type annotations and use `instance`/
/// `uniform` the way widget shaders do.
const DOC_PRELUDE: &str =
    "use mod.std.*\nuse mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw\n";

// ---------------------------------------------------------------------------
// Small forgiving readers (trimmed from the sandbox's value helpers).
// ---------------------------------------------------------------------------

pub(crate) struct Reader<'a, 'v> {
    pub vm: &'a mut ScriptVm<'v>,
    pub obj: ScriptObject,
    pub warnings: Vec<String>,
}

impl<'a, 'v> Reader<'a, 'v> {
    pub fn value(&mut self, key: LiveId) -> ScriptValue {
        let v = self.vm.bx.heap.value(self.obj, key.into(), NoTrap);
        if v.is_err() {
            NIL
        } else {
            v
        }
    }

    pub fn f32(&mut self, key: LiveId, default: f32) -> f32 {
        let v = self.value(key);
        if v.is_nil() {
            return default;
        }
        match NumericValue::from_script_value_heap(&self.vm.bx.heap, v, Default::default()) {
            NumericValue::F64(f) => f as f32,
            _ => {
                self.warnings.push(format!("{key}: expected a number"));
                default
            }
        }
    }

    pub fn usize(&mut self, key: LiveId, default: usize) -> usize {
        self.f32(key, default as f32).max(0.0) as usize
    }

    pub fn string(&mut self, key: LiveId) -> Option<String> {
        let v = self.value(key);
        if v.is_nil() {
            return None;
        }
        self.vm.bx.heap.cast_to_owned_string(v, "vjfx doc string")
    }

    /// A number OR a binding-expression string (the animatable contract).
    pub fn anim(&mut self, key: LiveId, default: f32) -> Animatable {
        let v = self.value(key);
        if v.is_nil() {
            return Animatable::Const(default);
        }
        // is_string_like, NOT as_string: short literals ("p0", "bar",
        // "pulse") are INLINE strings — a different type tag that
        // as_string() misses, which silently turned bare-signal bindings
        // into numeric garbage.
        if v.is_string_like() {
            let src = self
                .vm
                .bx
                .heap
                .cast_to_owned_string(v, "vjfx binding")
                .unwrap_or_default();
            return match Expr::compile(&src) {
                Ok(e) => Animatable::Bound(e),
                Err(err) => {
                    self.warnings.push(format!("{key}: bad binding — {err}"));
                    Animatable::Const(default)
                }
            };
        }
        match NumericValue::from_script_value_heap(&self.vm.bx.heap, v, Default::default()) {
            NumericValue::F64(f) => Animatable::Const(f as f32),
            _ => {
                self.warnings
                    .push(format!("{key}: expected a number or a binding string"));
                Animatable::Const(default)
            }
        }
    }

    pub fn color(&mut self, key: LiveId, default: Vec4f) -> Vec4f {
        let v = self.value(key);
        if v.is_nil() {
            return default;
        }
        match NumericValue::from_script_value_heap(&self.vm.bx.heap, v, Default::default()) {
            NumericValue::Color(c) => c,
            NumericValue::Vec4(c) => c,
            _ => {
                self.warnings
                    .push(format!("{key}: expected a color literal like #x40f0ff"));
                default
            }
        }
    }

    pub fn vec3(&mut self, key: LiveId, default: Vec3f) -> Vec3f {
        let v = self.value(key);
        if v.is_nil() {
            return default;
        }
        match NumericValue::from_script_value_heap(&self.vm.bx.heap, v, Default::default()) {
            NumericValue::Vec3(p) => p,
            _ => {
                self.warnings.push(format!("{key}: expected vec3(x, y, z)"));
                default
            }
        }
    }

    fn list_len(&self, v: ScriptValue) -> usize {
        if let Some(a) = v.as_array() {
            self.vm.bx.heap.array_len(a)
        } else if let Some(o) = v.as_object() {
            self.vm.bx.heap.vec_len(o)
        } else {
            0
        }
    }

    fn list_item(&self, v: ScriptValue, index: usize) -> ScriptValue {
        if let Some(a) = v.as_array() {
            self.vm.bx.heap.array_index(a, index, NoTrap)
        } else if let Some(o) = v.as_object() {
            self.vm.bx.heap.vec_value(o, index, NoTrap)
        } else {
            NIL
        }
    }

    pub fn string_list(&mut self, key: LiveId) -> Vec<String> {
        let v = self.value(key);
        let len = self.list_len(v);
        let mut out = Vec::with_capacity(len);
        for index in 0..len {
            let item = self.list_item(v, index);
            if let Some(s) = self.vm.bx.heap.cast_to_owned_string(item, "vjfx doc list") {
                out.push(s);
            }
        }
        out
    }

    pub fn object_list(&mut self, key: LiveId) -> Vec<ScriptObject> {
        let v = self.value(key);
        let len = self.list_len(v);
        let mut out = Vec::with_capacity(len);
        for index in 0..len {
            if let Some(obj) = self.list_item(v, index).as_object() {
                out.push(obj);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Post-chain stage configs. Numeric params are ANIMATABLE (constants or
// binding expressions) — a kaleidoscope can spin on the bar.
// ---------------------------------------------------------------------------

/// Fullscreen warp modes (`DrawVjFxWarp` u_warp.x).
#[derive(Clone, Copy, PartialEq)]
pub enum WarpMode {
    Kaleido = 0,
    Mirror = 1,
    Chroma = 2,
    Pixelate = 3,
    Swirl = 4,
    Ripple = 5,
    Glitch = 6,
    Posterize = 7,
    RadialBlur = 8,
    Tunnel = 9,
}

#[derive(Clone)]
pub enum StageCfg {
    /// Trails: previous frame re-projected under the fresh one.
    Feedback { amount: Animatable, zoom: Animatable, rotate: Animatable, dim: Animatable },
    /// Bright-pass glow. `levels` = blur pyramid depth (1..=4).
    Bloom { threshold: Animatable, strength: Animatable, levels: usize },
    /// Plain full-frame blur (levels 1..=4).
    Blur { levels: usize },
    /// Pyramid blur mixed back by a screen-Y focus ramp (miniature look).
    Tiltshift { focus: Animatable, width: Animatable, levels: usize },
    /// Fullscreen warp pass: kaleido, chroma, glitch… `p1`/`p2` meaning per
    /// mode (see the contract in mod.rs).
    Warp { mode: WarpMode, p1: Animatable, p2: Animatable },
}

// ---------------------------------------------------------------------------
// Growth animation of the l-system (and anything using u_growth).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum GrowMode {
    Off,
    Loop,
    PingPong,
}

// ---------------------------------------------------------------------------
// The parsed document.
// ---------------------------------------------------------------------------

pub struct CamCfg {
    /// None = auto-framed from the mesh bounds / engine defaults.
    pub dist: Option<f32>,
    pub height: Option<f32>,
    /// Radians/sec orbit around the subject.
    pub orbit: f32,
    pub fov: f32,
}

/// One declared user-param lever: `{name: "SYNC", bind: "p0", default: 0.3}`.
#[derive(Clone, Debug, PartialEq)]
pub struct DialDecl {
    /// Knob legend (kept short — the host's knob columns are narrow).
    pub label: String,
    /// Which user param it drives (0..=3 → p0..p3 → `self.user.xyzw`).
    pub index: usize,
    /// Resting knob position 0..1 (display only — an untouched knob never
    /// overrides the document's own binding).
    pub default: f32,
}

impl DialDecl {
    fn new(label: &str, index: usize, default: f32) -> DialDecl {
        DialDecl { label: label.to_string(), index, default }
    }
}

/// The levers an engine's STOCK shader actually reads, for docs that do not
/// declare their own. An engine that reads no user params declares none —
/// the host hides those knobs rather than showing dead ones.
pub fn engine_default_dials(engine: &str) -> Vec<DialDecl> {
    let d = DialDecl::new;
    match engine {
        "firefly" => vec![d("SYNC", 0, 0.3), d("FLASH", 1, 0.5)],
        "domino" => vec![d("NUDGE", 0, 0.0), d("WAVE", 1, 0.5), d("GLOW", 2, 0.5)],
        "harmonograph" => vec![d("DETUN", 0, 0.3), d("WIDTH", 1, 0.5)],
        "forge" => vec![d("HEAT", 0, 0.5), d("RING", 1, 0.5), d("EMBER", 2, 0.5)],
        "copper" => vec![d("BARS", 0, 0.5), d("GLOW", 1, 0.5)],
        "tiles" => vec![d("DRIVE", 0, 0.0), d("WAVE", 1, 0.5), d("SHADE", 2, 0.5)],
        "city" => vec![d("DENS", 0, 0.5), d("SPEED", 1, 0.5), d("GLOW", 2, 0.5)],
        "pipes" => vec![d("FRONT", 0, 0.0), d("HEAT", 1, 0.5), d("GLOW", 2, 0.5)],
        "stockcharts" => vec![d("GAIN", 0, 0.5), d("GLOW", 1, 0.5), d("PANIC", 2, 0.0)],
        "mountainjet" => vec![d("GLOW", 0, 0.5), d("FLICK", 1, 0.5), d("PLUME", 2, 0.5)],
        "fluid" => vec![d("WARP", 0, 0.5), d("TEXMX", 1, 0.5)],
        "transition" => vec![d("SOFT", 0, 0.15)],
        _ => Vec::new(),
    }
}

pub struct EffectDoc {
    pub name: String,
    pub engine: Engine,
    pub warnings: Vec<String>,
    /// How the HOST engages this doc as the program transition along the
    /// crossfader — see [`EngageProfile`].
    pub engage: EngageProfile,

    pub speed: f32,
    pub beat_pulse: Animatable,
    /// Pulses per beat (2.0 = eighth notes at the VJ's clock).
    pub beat_rate: f32,
    /// Beats per bar for the `bar` signal.
    pub bar_beats: f32,
    pub sway: Animatable,
    pub sway_freq: Animatable,
    pub twist: Animatable,
    pub fog: Animatable,
    pub glow: Animatable,
    /// CONTENT COUPLING strength 0..1 (doc key `content`, animatable):
    /// how strongly the effect folds the live input0 video into its look.
    /// Reaches shaders as `self.fog.z`, PRE-GATED to 0 by the host when no
    /// REAL content is bound (the animated fallback pattern must never
    /// leak through a coupling — the standalone look stays classic).
    pub content: Animatable,
    pub grow: GrowMode,
    /// Beats per growth sweep.
    pub grow_beats: f32,
    /// Free user parameters p0..p3, reachable as `self.user` in shader
    /// hooks — the standard way a doc pipes a custom curve into its shader.
    pub params: [Animatable; 4],
    /// SELF-DESCRIBING DIALS: which of p0..p3 mean something to THIS
    /// effect, by name — hosts label real knobs from these instead of
    /// showing dead "P1/P2" dials. Docs without a `dials:` block inherit
    /// their engine's default set (possibly empty).
    pub dials: Vec<DialDecl>,

    /// bg, a, b, c.
    pub palette: [Vec4f; 4],
    pub cam: CamCfg,
    /// "test" binds the gallery's generated test pattern; in the VJ app the
    /// channel's main content lands here.
    pub input0: Option<String>,
    pub stages: Vec<StageCfg>,
    /// The `shader: { fx_color: fn()... }` object, applied onto the draw
    /// shader at load (recompiles it with the document's hooks).
    pub shader_hooks: Option<ScriptObjectRef>,
    /// The `frame: fn(fx){...}` per-frame tick (emitters engine).
    pub frame_fn: Option<ScriptObjectRef>,
    /// Declared + engine-synthesized SIM FIELDS (float ping-pong state
    /// textures updated per frame on the GPU — see sim.rs).
    pub fields: Vec<super::sim::SimFieldCfg>,
    /// `wind_field: "<name>"` — mesh-family engines swap their analytic
    /// sway for this wind field (the DrawVjFxMeshField shader).
    pub wind_field: Option<String>,
}

/// TRANSITION ENGAGE PROFILE — declared per doc as `engage: "triangle" |
/// "ramp"`.
///
/// * `Triangle` (default): engagement is the fader triangle — zero at both
///   ends, full at mid. Right for TIMED moves (wipe/dissolve/iris/push):
///   their t=0/t=1 endpoints ARE the plain decks, so the rest state at
///   either end is exact and free.
/// * `Ramp`: engagement rises with the fader and STAYS full at B's end —
///   `min(2·mix, 1)`. Right for OVERLAY/KEY docs (luma key, chroma key,
///   additive, PiP…): riding the fader to the end must leave the KEYED
///   COMPOSITE up, never flick the base layer off — full fader means "key
///   fully applied", and only the A end is the plain deck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngageProfile {
    #[default]
    Triangle,
    Ramp,
}

fn warp_kind(kind: &str) -> Option<WarpMode> {
    Some(match kind {
        "kaleido" => WarpMode::Kaleido,
        "mirror" => WarpMode::Mirror,
        "chroma" | "rgb" => WarpMode::Chroma,
        "pixelate" | "mosaic" => WarpMode::Pixelate,
        "swirl" => WarpMode::Swirl,
        "ripple" => WarpMode::Ripple,
        "glitch" => WarpMode::Glitch,
        "posterize" => WarpMode::Posterize,
        "radial_blur" | "radialblur" => WarpMode::RadialBlur,
        "warp_tunnel" | "uv_tunnel" => WarpMode::Tunnel,
        _ => return None,
    })
}

impl EffectDoc {
    /// Evaluate `source` and read the doc. `key` names the document slot;
    /// the module file it evaluates under is CONTENT-ADDRESSED (key + a
    /// hash of the source). This is load-bearing, not cosmetic: the draw
    /// system caches compiled shaders by the hook functions' instruction
    /// pointers (`DrawVars::compute_shader_functions_hash`), and a module
    /// re-evaluated under the SAME file name lands its closures on the
    /// SAME ips — so loading doc B into a slot that held doc A (or
    /// rendering thumbnails through one shared key) would silently REUSE
    /// A's compiled `trans`/`fx_color` shader ("every transition is a
    /// wipe"). A distinct file per distinct content gives distinct ips;
    /// reloading identical content maps to the same module, so nothing
    /// accretes in the steady state.
    pub fn parse(vm: &mut ScriptVm, key: &str, source: &str) -> Result<EffectDoc, String> {
        let content = LiveId(LiveId::SEED).bytes_append(source.as_bytes());
        let script_mod = ScriptMod {
            file: format!("vjfx://{key}/{:016x}", content.0),
            code: format!("{DOC_PRELUDE}{source}"),
            ..Default::default()
        };
        vm.bx.captured_errors = Some(Vec::new());
        let value = vm.eval(script_mod);
        let errors = vm.take_errors();
        if value.is_err() || !errors.is_empty() {
            return Err(format!(
                "effect doc failed to evaluate: {}",
                if errors.is_empty() { "(unknown error)".to_string() } else { errors.join("; ") }
            ));
        }
        let Some(obj) = value.as_object() else {
            return Err(
                "effect doc must end with one { … } object (its last expression)".to_string()
            );
        };
        let mut r = Reader { vm, obj, warnings: Vec::new() };

        let name = r.string(live_id!(name)).unwrap_or_else(|| key.to_string());
        let engine_name = r.string(live_id!(engine)).unwrap_or_default();
        let seed = r.f32(live_id!(seed), 1.0).max(0.0) as u64 + 1;

        // -- engine ---------------------------------------------------------
        let engine = match engine_name.as_str() {
            "particles" => {
                let mut cfg = ParticlesConfig { seed, ..Default::default() };
                if let Some(mode) = r.string(live_id!(mode)) {
                    match ParticleMode::parse(&mode) {
                        Some(m) => cfg.mode = m,
                        None => r.warnings.push(format!(
                            "mode '{mode}' unknown (burst/fountain/tunnel/vortex/rain/galaxy/image)"
                        )),
                    }
                }
                cfg.count = r.usize(live_id!(count), cfg.count).clamp(16, 30_000);
                cfg.spread = r.f32(live_id!(spread), cfg.spread);
                cfg.size = r.f32(live_id!(size), cfg.size);
                cfg.rate = r.f32(live_id!(rate), cfg.rate);
                cfg.gravity = r.f32(live_id!(gravity), cfg.gravity);
                cfg.swirl = r.f32(live_id!(swirl), cfg.swirl);
                cfg.stretch = r.f32(live_id!(stretch), cfg.stretch);
                Engine::Particles(ParticlesEngine::new(cfg))
            }
            "lsystem" => {
                let mut cfg = LsystemConfig { seed, ..Default::default() };
                if let Some(axiom) = r.string(live_id!(axiom)) {
                    cfg.axiom = axiom;
                }
                let rules = r.string_list(live_id!(rules));
                if !rules.is_empty() {
                    cfg.rules.clear();
                    for rule in rules {
                        match rule.split_once('=') {
                            Some((sym, rep)) if sym.trim().chars().count() == 1 => {
                                cfg.rules.push((
                                    sym.trim().chars().next().unwrap(),
                                    rep.trim().to_string(),
                                ));
                            }
                            _ => r
                                .warnings
                                .push(format!("rule '{rule}' is not \"S=REPLACEMENT\"")),
                        }
                    }
                }
                cfg.iterations = r.usize(live_id!(iterations), cfg.iterations).clamp(1, 12);
                cfg.angle = r.f32(live_id!(angle), cfg.angle);
                cfg.angle_jitter = r.f32(live_id!(angle_jitter), cfg.angle_jitter);
                cfg.step = r.f32(live_id!(step), cfg.step);
                cfg.radius = r.f32(live_id!(radius), cfg.radius);
                cfg.radius_decay = r.f32(live_id!(radius_decay), cfg.radius_decay);
                cfg.sides = r.usize(live_id!(sides), cfg.sides).clamp(3, 8);
                cfg.copies = r.usize(live_id!(copies), cfg.copies).clamp(1, 8);
                Engine::Lsystem(LsystemEngine::new(cfg))
            }
            "metaballs" => {
                let mut cfg = MetaballsConfig { seed, ..Default::default() };
                cfg.blobs = r.usize(live_id!(blobs), cfg.blobs).clamp(2, 12);
                cfg.grid = r.usize(live_id!(grid), cfg.grid).clamp(8, 48);
                cfg.extent = r.f32(live_id!(extent), cfg.extent);
                cfg.blob_radius = r.f32(live_id!(blob_radius), cfg.blob_radius);
                cfg.orbit = r.f32(live_id!(orbit), cfg.orbit);
                cfg.speed = r.f32(live_id!(orbit_speed), cfg.speed);
                cfg.beat_swell = r.f32(live_id!(beat_swell), cfg.beat_swell);
                Engine::Metaballs(MetaballsEngine::new(cfg))
            }
            "heightmap" => {
                let mut cfg = HeightmapConfig::default();
                cfg.res = r.usize(live_id!(res), cfg.res).clamp(8, 220);
                cfg.size = r.f32(live_id!(size), cfg.size);
                cfg.height = r.f32(live_id!(height), cfg.height);
                cfg.noise_scale = r.f32(live_id!(noise_scale), cfg.noise_scale);
                cfg.scroll = r.f32(live_id!(scroll), cfg.scroll);
                cfg.ridged = r.f32(live_id!(ridged), cfg.ridged).clamp(0.0, 1.0);
                cfg.tex_displace = r.f32(live_id!(tex_displace), cfg.tex_displace).clamp(0.0, 1.0);
                Engine::Heightmap(HeightmapEngine::new(cfg))
            }
            "ribbons" => {
                let mut cfg = RibbonsConfig { seed, ..Default::default() };
                cfg.ribbons = r.usize(live_id!(ribbons), cfg.ribbons).clamp(2, 96);
                cfg.trail = r.usize(live_id!(trail), cfg.trail).clamp(8, 160);
                cfg.width = r.f32(live_id!(width), cfg.width);
                cfg.speed = r.f32(live_id!(flow_speed), cfg.speed);
                cfg.swirl = r.f32(live_id!(swirl), cfg.swirl);
                cfg.bound = r.f32(live_id!(bound), cfg.bound);
                if let Some(field) = r.string(live_id!(field)) {
                    match RibbonField::parse(&field) {
                        Some(f) => cfg.field = f,
                        None => r
                            .warnings
                            .push(format!("field '{field}' unknown (curl/lorenz/aizawa)")),
                    }
                }
                Engine::Ribbons(RibbonsEngine::new(cfg))
            }
            "tunnel" => {
                let mut cfg = TunnelConfig::default();
                cfg.p = r.f32(live_id!(knot_p), cfg.p);
                cfg.q = r.f32(live_id!(knot_q), cfg.q);
                cfg.major = r.f32(live_id!(major), cfg.major);
                cfg.tube = r.f32(live_id!(tube), cfg.tube);
                cfg.rings = r.usize(live_id!(rings), cfg.rings).clamp(64, 2048);
                cfg.sides = r.usize(live_id!(sides), cfg.sides).clamp(3, 48);
                cfg.fly = r.f32(live_id!(fly), cfg.fly);
                cfg.bands = r.f32(live_id!(bands), cfg.bands);
                if let Some(path) = r.string(live_id!(path)) {
                    match path.as_str() {
                        "knot" => cfg.path = TunnelPath::Knot,
                        "lissajous" => cfg.path = TunnelPath::Lissajous,
                        other => r
                            .warnings
                            .push(format!("path '{other}' unknown (knot/lissajous)")),
                    }
                }
                Engine::Tunnel(TunnelEngine::new(cfg))
            }
            "grass" => {
                let mut cfg = GrassConfig { seed, ..Default::default() };
                cfg.blades = r.usize(live_id!(blades), cfg.blades).clamp(64, 20_000);
                cfg.area = r.f32(live_id!(area), cfg.area);
                cfg.height = r.f32(live_id!(height), cfg.height);
                cfg.width = r.f32(live_id!(width), cfg.width);
                cfg.clump = r.f32(live_id!(clump), cfg.clump).clamp(0.0, 1.0);
                Engine::Grass(GrassEngine::new(cfg))
            }
            "emitters" => {
                let mut cfg = EmittersConfig { seed, ..Default::default() };
                cfg.particles = r.usize(live_id!(particles), cfg.particles).clamp(64, 4096);
                cfg.size = r.f32(live_id!(size), cfg.size);
                cfg.gravity = r.f32(live_id!(gravity), cfg.gravity);
                Engine::Emitters(EmittersEngine::new(cfg))
            }
            "firefly" => {
                use super::engines_firefly::{FireflyConfig, FireflyEngine};
                let mut cfg = FireflyConfig { seed, ..Default::default() };
                cfg.flies = r.usize(live_id!(flies), cfg.flies).clamp(8, 4000);
                cfg.blades = r.usize(live_id!(blades), cfg.blades).min(20_000);
                cfg.area = r.f32(live_id!(area), cfg.area);
                cfg.fly_height = r.f32(live_id!(fly_height), cfg.fly_height);
                cfg.fly_size = r.f32(live_id!(fly_size), cfg.fly_size);
                cfg.sync = r.f32(live_id!(sync), cfg.sync);
                cfg.blink_rate = r.f32(live_id!(blink_rate), cfg.blink_rate);
                cfg.blink_sharp = r.f32(live_id!(blink_sharp), cfg.blink_sharp);
                cfg.wander = r.f32(live_id!(wander), cfg.wander);
                cfg.moon = r.f32(live_id!(moon), cfg.moon);
                cfg.grass_height = r.f32(live_id!(grass_height), cfg.grass_height);
                cfg.clump = r.f32(live_id!(clump), cfg.clump);
                Engine::Firefly(FireflyEngine::new(cfg))
            }
            "harmonograph" | "harmono" => {
                use super::engines_harmonograph::{HarmonographConfig, HarmonographEngine};
                let mut cfg = HarmonographConfig { seed, ..Default::default() };
                cfg.segments = r.usize(live_id!(segments), cfg.segments).clamp(64, 6000);
                cfg.strands = r.usize(live_id!(strands), cfg.strands).clamp(1, 6);
                cfg.freq_x = r.f32(live_id!(freq_x), cfg.freq_x);
                cfg.freq_y = r.f32(live_id!(freq_y), cfg.freq_y);
                cfg.freq_z = r.f32(live_id!(freq_z), cfg.freq_z);
                cfg.damping = r.f32(live_id!(damping), cfg.damping);
                cfg.detune = r.f32(live_id!(detune), cfg.detune);
                cfg.turns = r.f32(live_id!(turns), cfg.turns);
                cfg.width = r.f32(live_id!(width), cfg.width);
                cfg.morph_beats = r.f32(live_id!(morph_beats), cfg.morph_beats);
                Engine::Harmono(HarmonographEngine::new(cfg))
            }
            "domino" => {
                use super::engines_domino::{DominoConfig, DominoEngine, DominoLayout};
                let mut cfg = DominoConfig { seed, ..Default::default() };
                if let Some(layout) = r.string(live_id!(layout)) {
                    match DominoLayout::parse(&layout) {
                        Some(l) => cfg.layout = l,
                        None => r.warnings.push(format!(
                            "layout '{layout}' unknown (spiral/serpent/tree)"
                        )),
                    }
                }
                cfg.count = r.usize(live_id!(count), cfg.count).clamp(32, 6000);
                cfg.per_beat = r.f32(live_id!(per_beat), cfg.per_beat);
                cfg.spacing = r.f32(live_id!(spacing), cfg.spacing);
                cfg.tile_h = r.f32(live_id!(tile_h), cfg.tile_h);
                cfg.tile_w = r.f32(live_id!(tile_w), cfg.tile_w);
                cfg.tile_t = r.f32(live_id!(tile_t), cfg.tile_t);
                cfg.branches = r.usize(live_id!(branches), cfg.branches).clamp(1, 12);
                cfg.jitter = r.f32(live_id!(jitter), cfg.jitter);
                cfg.resurrect = r.f32(live_id!(resurrect), cfg.resurrect);
                cfg.pause_beats = r.f32(live_id!(pause_beats), cfg.pause_beats);
                cfg.flash = r.f32(live_id!(flash), cfg.flash);
                Engine::Domino(DominoEngine::new(cfg))
            }
            "forge" | "kickforge" => {
                use super::engines_forge::{ForgeConfig, ForgeEngine};
                let mut cfg = ForgeConfig { seed, ..Default::default() };
                cfg.shards = r.usize(live_id!(shards), cfg.shards).clamp(64, 6000);
                cfg.radius = r.f32(live_id!(radius), cfg.radius);
                cfg.impulse = r.f32(live_id!(impulse), cfg.impulse);
                cfg.gravity = r.f32(live_id!(gravity), cfg.gravity);
                cfg.spin = r.f32(live_id!(spin), cfg.spin);
                cfg.membrane_wave = r.f32(live_id!(membrane_wave), cfg.membrane_wave);
                cfg.shard_size = r.f32(live_id!(shard_size), cfg.shard_size);
                cfg.scatter = r.f32(live_id!(scatter), cfg.scatter);
                cfg.falloff = r.f32(live_id!(falloff), cfg.falloff);
                cfg.pile = r.f32(live_id!(pile), cfg.pile);
                cfg.auto_pump = r.f32(live_id!(auto_pump), cfg.auto_pump);
                cfg.glint = r.f32(live_id!(glint), cfg.glint);
                // Mirror of the shared beat_rate key: the forge shader needs
                // it to reconstruct seconds-per-pulse and the pulse index.
                cfg.rate = r.f32(live_id!(beat_rate), cfg.rate).clamp(0.05, 8.0);
                Engine::Forge(ForgeEngine::new(cfg))
            }
            "copperbars" | "bars" | "rasterbars" => {
                use super::engines_copper::{CopperConfig, CopperEngine, CopperMode};
                let mut cfg = CopperConfig { seed, ..Default::default() };
                cfg.bars = r.usize(live_id!(bars), cfg.bars).clamp(4, 64);
                cfg.width = r.f32(live_id!(width), cfg.width);
                cfg.span = r.f32(live_id!(span), cfg.span);
                cfg.thickness = r.f32(live_id!(thickness), cfg.thickness);
                cfg.depth = r.f32(live_id!(depth), cfg.depth);
                cfg.amplitude = r.f32(live_id!(amplitude), cfg.amplitude);
                cfg.weave = r.f32(live_id!(weave), cfg.weave);
                cfg.metal = r.f32(live_id!(metal), cfg.metal);
                cfg.drop = r.f32(live_id!(drop), cfg.drop);
                if let Some(mode) = r.string(live_id!(mode)) {
                    match CopperMode::parse(&mode) {
                        Some(m) => {
                            cfg.mode = m;
                            cfg.mode_b = m;
                        }
                        None => r.warnings.push(format!(
                            "mode '{mode}' unknown (sine/pile/scissor/curtain)"
                        )),
                    }
                }
                if let Some(mode) = r.string(live_id!(mode_b)) {
                    match CopperMode::parse(&mode) {
                        Some(m) => cfg.mode_b = m,
                        None => r.warnings.push(format!(
                            "mode_b '{mode}' unknown (sine/pile/scissor/curtain)"
                        )),
                    }
                }
                cfg.rate = r.f32(live_id!(beat_rate), cfg.rate).clamp(0.05, 8.0);
                Engine::Copper(CopperEngine::new(cfg))
            }
            "tiles" => {
                use super::engines_tiles::{TilesConfig, TilesEngine, TilesMode};
                let mut cfg = TilesConfig { seed, ..Default::default() };
                if let Some(mode) = r.string(live_id!(mode)) {
                    match TilesMode::parse(&mode) {
                        Some(m) => cfg.mode = m,
                        None => r.warnings.push(format!(
                            "mode '{mode}' unknown (wave/shatter/conveyor/spiral)"
                        )),
                    }
                }
                cfg.grid = r.usize(live_id!(grid), cfg.grid).clamp(4, 64);
                cfg.spread = r.f32(live_id!(spread), cfg.spread);
                cfg.aspect = r.f32(live_id!(aspect), cfg.aspect);
                cfg.gap = r.f32(live_id!(gap), cfg.gap);
                cfg.amp = r.f32(live_id!(amp), cfg.amp);
                cfg.freq = r.f32(live_id!(freq), cfg.freq);
                cfg.spin = r.f32(live_id!(spin), cfg.spin);
                cfg.scatter = r.f32(live_id!(scatter), cfg.scatter);
                Engine::Tiles(TilesEngine::new(cfg))
            }
            "flock" | "murmuration" => {
                use super::engines_flock::{FlockConfig, FlockEngine};
                let mut cfg = FlockConfig { seed, ..Default::default() };
                cfg.birds = r.usize(live_id!(birds), cfg.birds).clamp(8, 600);
                cfg.size = r.f32(live_id!(size), cfg.size);
                cfg.speed = r.f32(live_id!(flight_speed), cfg.speed);
                cfg.flap = r.f32(live_id!(flap), cfg.flap);
                cfg.bound = r.f32(live_id!(bound), cfg.bound);
                cfg.spacing = r.f32(live_id!(spacing), cfg.spacing);
                cfg.vision = r.f32(live_id!(vision), cfg.vision);
                cfg.goal_beats = r.f32(live_id!(goal_beats), cfg.goal_beats);
                cfg.predator = r.f32(live_id!(predator), cfg.predator);
                cfg.additive = r.f32(live_id!(additive), cfg.additive);
                cfg.bank = r.f32(live_id!(bank), cfg.bank);
                // Mirror of the shared bar_beats key — the predator's clock.
                cfg.bar_beats = r.f32(live_id!(bar_beats), cfg.bar_beats).clamp(1.0, 32.0);
                Engine::Flock(FlockEngine::new(cfg))
            }
            // The two-deck transition compositor: the host feeds BOTH deck
            // textures and sweeps p3 with the crossfader (engines_duo).
            "transition" => {
                use super::engines_duo::{DuoConfig, DuoEngine};
                Engine::Duo(DuoEngine::new(DuoConfig::default()))
            }
            "raymarch" => {
                use super::engines_raymarch::{RaymarchCam, RaymarchConfig, RaymarchEngine};
                let mut cfg = RaymarchConfig::default();
                cfg.steps = r.usize(live_id!(steps), cfg.steps).clamp(16, 120);
                cfg.max_dist = r.f32(live_id!(max_dist), cfg.max_dist);
                if let Some(cam) = r.string(live_id!(cam)) {
                    match RaymarchCam::parse(&cam) {
                        Some(c) => cfg.cam = c,
                        None => r
                            .warnings
                            .push(format!("cam '{cam}' unknown (orbit/fly/dolly)")),
                    }
                }
                cfg.cam_speed = r.f32(live_id!(cam_speed), cfg.cam_speed);
                cfg.cam_dist = r.f32(live_id!(cam_dist), cfg.cam_dist);
                cfg.cam_height = r.f32(live_id!(cam_height), cfg.cam_height);
                cfg.fov = r.f32(live_id!(cam_fov), cfg.fov);
                cfg.shadow = r.f32(live_id!(shadow), cfg.shadow);
                Engine::Raymarch(RaymarchEngine::new(cfg))
            }
            "mountainjet" | "jet" => {
                use super::engines_jet::{JetConfig, JetEngine, JetLook};
                let mut cfg = JetConfig { seed, ..Default::default() };
                cfg.res = r.usize(live_id!(res), cfg.res).clamp(8, 220);
                cfg.size = r.f32(live_id!(size), cfg.size);
                cfg.height = r.f32(live_id!(height), cfg.height);
                cfg.noise_scale = r.f32(live_id!(noise_scale), cfg.noise_scale);
                cfg.scroll = r.f32(live_id!(scroll), cfg.scroll);
                cfg.ridged = r.f32(live_id!(ridged), cfg.ridged).clamp(0.0, 1.0);
                cfg.cells = r.f32(live_id!(cells), cfg.cells);
                cfg.jet_size = r.f32(live_id!(jet_size), cfg.jet_size);
                cfg.weave = r.f32(live_id!(weave), cfg.weave);
                if let Some(look) = r.string(live_id!(look)) {
                    match JetLook::parse(&look) {
                        Some(l) => cfg.look = l,
                        None => r.warnings.push(format!(
                            "look '{look}' unknown (solid/wire/nightvision)"
                        )),
                    }
                }
                Engine::MountainJet(JetEngine::new(cfg))
            }
            "city" => {
                use super::engines_city::{CityConfig, CityEngine, CityStyle};
                let mut cfg = CityConfig { seed, ..Default::default() };
                if let Some(style) = r.string(live_id!(style)) {
                    match CityStyle::parse(&style) {
                        Some(st) => cfg.style = st,
                        None => r.warnings.push(format!(
                            "style '{style}' unknown (night/retro/tron)"
                        )),
                    }
                }
                cfg.blocks = r.usize(live_id!(blocks), cfg.blocks).clamp(2, 14);
                cfg.block = r.f32(live_id!(block), cfg.block);
                cfg.street = r.f32(live_id!(street), cfg.street);
                cfg.towers = r.usize(live_id!(towers), cfg.towers).clamp(4, 900);
                cfg.max_h = r.f32(live_id!(max_h), cfg.max_h);
                cfg.win = r.f32(live_id!(win), cfg.win);
                cfg.density = r.f32(live_id!(density), cfg.density);
                cfg.flicker = r.f32(live_id!(flicker), cfg.flicker);
                cfg.trails = r.usize(live_id!(trails), cfg.trails).min(16);
                cfg.trail_beats = r.f32(live_id!(trail_beats), cfg.trail_beats);
                cfg.wall_h = r.f32(live_id!(wall_h), cfg.wall_h);
                cfg.alt = r.f32(live_id!(alt), cfg.alt);
                cfg.fly = r.f32(live_id!(fly), cfg.fly);
                cfg.bank = r.f32(live_id!(bank), cfg.bank);
                Engine::City(CityEngine::new(cfg))
            }
            "pipes" => {
                use super::engines_pipes::{PipesConfig, PipesEngine};
                let mut cfg = PipesConfig { seed, ..Default::default() };
                cfg.pipes = r.usize(live_id!(pipes), cfg.pipes).clamp(1, 16);
                cfg.bound = r.usize(live_id!(bound), cfg.bound as usize).clamp(2, 10) as i32;
                cfg.cell = r.f32(live_id!(cell), cfg.cell);
                cfg.radius = r.f32(live_id!(radius), cfg.radius);
                cfg.sides = r.usize(live_id!(sides), cfg.sides).clamp(3, 16);
                cfg.steps = r.usize(live_id!(steps), cfg.steps).clamp(32, 2600);
                cfg.turn_chance = r.f32(live_id!(turn_chance), cfg.turn_chance);
                cfg.pop = r.f32(live_id!(pop), cfg.pop);
                cfg.hot = r.f32(live_id!(hot), cfg.hot);
                Engine::Pipes(PipesEngine::new(cfg))
            }
            "stockcharts" | "charts" | "candles" => {
                use super::engines_charts::{ChartsConfig, ChartsEngine};
                let mut cfg = ChartsConfig { seed, ..Default::default() };
                cfg.candles = r.usize(live_id!(candles), cfg.candles).clamp(16, 400);
                cfg.per_beat = r.f32(live_id!(per_beat), cfg.per_beat);
                cfg.vol = r.f32(live_id!(vol), cfg.vol);
                cfg.drift = r.f32(live_id!(drift), cfg.drift);
                cfg.spike = r.f32(live_id!(spike), cfg.spike);
                cfg.cascade = r.f32(live_id!(cascade), cfg.cascade);
                cfg.bar = r.f32(live_id!(bar), cfg.bar);
                cfg.width = r.f32(live_id!(width), cfg.width);
                cfg.height = r.f32(live_id!(height), cfg.height);
                cfg.body_w = r.f32(live_id!(body_w), cfg.body_w);
                cfg.ma = r.usize(live_id!(ma), cfg.ma).min(64);
                cfg.grid_x = r.usize(live_id!(grid_x), cfg.grid_x).clamp(0, 64);
                cfg.grid_y = r.usize(live_id!(grid_y), cfg.grid_y).clamp(0, 24);
                cfg.scan = r.f32(live_id!(scan), cfg.scan);
                Engine::Charts(ChartsEngine::new(cfg))
            }
            "simswarm" | "swarm" | "gpuparticles" => {
                use super::engines_simfx::{SwarmConfig, SwarmEngine};
                let mut cfg = SwarmConfig { seed, ..Default::default() };
                cfg.count = r.usize(live_id!(count), cfg.count).clamp(256, 25_600);
                cfg.size = r.f32(live_id!(size), cfg.size);
                cfg.stretch = r.f32(live_id!(stretch), cfg.stretch);
                cfg.speed_color = r.f32(live_id!(speed_color), cfg.speed_color);
                // bound/life are also (animatable) field keys; the engine
                // only needs constants for framing + the age fade.
                if let Animatable::Const(v) = r.anim(live_id!(bound), cfg.bound) {
                    cfg.bound = v;
                }
                if let Animatable::Const(v) = r.anim(live_id!(life), cfg.life) {
                    cfg.life = v;
                }
                if let Some(f) = r.string(live_id!(state_field)) {
                    cfg.state_field = f;
                }
                Engine::Swarm(SwarmEngine::new(cfg))
            }
            "fluid" => {
                use super::engines_simfx::{FluidConfig, FluidEngine};
                let mut cfg = FluidConfig::default();
                cfg.grid = r.usize(live_id!(grid), cfg.grid).clamp(32, 256);
                if let Some(f) = r.string(live_id!(field)) {
                    cfg.field = f;
                }
                Engine::Fluid(FluidEngine::new(cfg))
            }
            "screen" => Engine::Screen,
            other => {
                return Err(format!(
                    "engine '{other}' unknown — one of particles, lsystem, metaballs, \
                     heightmap, ribbons, tunnel, grass, emitters, firefly, harmonograph, \
                     domino, forge, copperbars, tiles, flock, raymarch, mountainjet, city, \
                     pipes, stockcharts, simswarm, fluid, screen"
                ));
            }
        };

        // -- shared animation (all animatable: number or binding string) ----
        let speed = r.f32(live_id!(speed), 1.0);
        let beat_pulse = r.anim(live_id!(beat_pulse), 0.5);
        let beat_rate = r.f32(live_id!(beat_rate), 1.0).clamp(0.05, 8.0);
        let bar_beats = r.f32(live_id!(bar_beats), 4.0).clamp(1.0, 32.0);
        let sway = r.anim(live_id!(sway), 0.4);
        let sway_freq = r.anim(live_id!(sway_freq), 0.9);
        let twist = r.anim(live_id!(twist), 0.0);
        let fog = r.anim(live_id!(fog), 0.045);
        let glow = r.anim(live_id!(glow), 1.0);
        // Content coupling strength (`content:` — number or binding). The
        // 0.75 default is the ENGINE-TUNED sweet spot: THE BAR IS THAT A
        // VIEWER INSTANTLY SEES THE VIDEO PLAYING IN THE EFFECT — a
        // picture, not a tint (the first 0.5 pass read as a wash and was
        // rejected). Every family scales its own coupling around it: 0.75
        // = the video plainly there with the effect's identity still on
        // top, 1.0 = video-dominant, 0 = the exact classic look.
        let content = r.anim(live_id!(content), 0.75);
        let grow = match r.string(live_id!(grow)).as_deref() {
            None | Some("off") => GrowMode::Off,
            Some("loop") => GrowMode::Loop,
            Some("pingpong") => GrowMode::PingPong,
            Some(other) => {
                r.warnings.push(format!("grow '{other}' unknown (off/loop/pingpong)"));
                GrowMode::Off
            }
        };
        let grow_beats = r.f32(live_id!(grow_beats), 8.0).clamp(0.5, 64.0);
        // Parsed BELOW (dial declarations), used here: a doc that declares
        // a dial default but never sets `p_i:` must BEHAVE at that default
        // — the knob face and the shader value must be the same number.
        // (The wipe's SOFT dial said 0.15 while the shader ran 0.0.)
        let params;

        // -- palette --------------------------------------------------------
        let palette = [
            r.color(live_id!(color_bg), vec4(0.01, 0.012, 0.03, 1.0)),
            r.color(live_id!(color_a), vec4(0.28, 0.94, 1.0, 1.0)),
            r.color(live_id!(color_b), vec4(1.0, 0.25, 0.63, 1.0)),
            r.color(live_id!(color_c), vec4(1.0, 1.0, 1.0, 1.0)),
        ];

        // -- camera ---------------------------------------------------------
        let dist = r.f32(live_id!(cam_dist), -1.0);
        let height = r.f32(live_id!(cam_height), f32::NAN);
        let cam = CamCfg {
            dist: (dist > 0.0).then_some(dist),
            height: height.is_finite().then_some(height),
            orbit: r.f32(live_id!(cam_orbit), 0.12),
            fov: r.f32(live_id!(cam_fov), 50.0).clamp(20.0, 120.0),
        };

        let input0 = r.string(live_id!(input0));

        // -- sim fields (float GPU state textures — sim.rs) -----------------
        // Declared in `fields: [...]`; the sim engines synthesize the field
        // they need from DOCUMENT-level keys when none is declared, and a
        // named-but-missing wind field materializes with defaults (an
        // AI-authored doc degrades, never vanishes).
        let mut fields = super::sim::parse_fields(&mut r);
        let wind_field = r.string(live_id!(wind_field));
        match &engine {
            Engine::Swarm(e) => {
                if !fields.iter().any(|f| f.name == e.cfg.state_field) {
                    let mut cfg = super::sim::SimFieldCfg::swarm(&e.cfg.state_field, e.side);
                    super::sim::read_field_keys(&mut r, &mut cfg);
                    // The engine's quad sheet defines the texel count.
                    cfg.res = e.side;
                    fields.push(cfg);
                }
            }
            Engine::Fluid(e) => {
                if !fields.iter().any(|f| f.name == e.cfg.field) {
                    let mut cfg = super::sim::SimFieldCfg::fluid(&e.cfg.field, e.cfg.grid);
                    super::sim::read_field_keys(&mut r, &mut cfg);
                    cfg.res = e.cfg.grid;
                    fields.push(cfg);
                }
            }
            _ => {}
        }
        if let Some(wf) = &wind_field {
            if !fields.iter().any(|f| f.name == *wf) {
                let mut cfg = super::sim::SimFieldCfg::wind(wf);
                super::sim::read_field_keys(&mut r, &mut cfg);
                fields.push(cfg);
            }
        }

        // -- post stages ----------------------------------------------------
        let stage_objs = r.object_list(live_id!(stages));
        let mut stages = Vec::new();
        for obj in stage_objs {
            if stages.len() >= 4 {
                r.warnings.push("more than 4 stages — extras dropped".to_string());
                break;
            }
            let outer = r.obj;
            r.obj = obj;
            let kind = r.string(live_id!(kind)).unwrap_or_default();
            match kind.as_str() {
                "feedback" | "trails" => stages.push(StageCfg::Feedback {
                    amount: r.anim(live_id!(amount), 0.85),
                    zoom: r.anim(live_id!(zoom), 1.01),
                    rotate: r.anim(live_id!(rotate), 0.0),
                    dim: r.anim(live_id!(dim), 0.97),
                }),
                "bloom" | "glow" => stages.push(StageCfg::Bloom {
                    threshold: r.anim(live_id!(threshold), 0.45),
                    strength: r.anim(live_id!(strength), 1.2),
                    levels: r.usize(live_id!(levels), 3).clamp(1, 4),
                }),
                "blur" => stages.push(StageCfg::Blur {
                    levels: r.usize(live_id!(levels), 2).clamp(1, 4),
                }),
                "tiltshift" | "tilt" => stages.push(StageCfg::Tiltshift {
                    focus: r.anim(live_id!(focus), 0.55),
                    width: r.anim(live_id!(width), 0.25),
                    levels: r.usize(live_id!(levels), 3).clamp(1, 4),
                }),
                other => match warp_kind(other) {
                    Some(mode) => stages.push(StageCfg::Warp {
                        mode,
                        p1: r.anim(live_id!(p1), 0.5),
                        p2: r.anim(live_id!(p2), 0.5),
                    }),
                    None => r.warnings.push(format!(
                        "stage kind '{other}' unknown (feedback/bloom/blur/kaleido/mirror/\
                         chroma/pixelate/swirl/ripple/glitch/posterize/radial_blur/warp_tunnel) \
                         — skipped"
                    )),
                },
            }
            r.obj = outer;
        }

        // -- shader hooks + frame tick --------------------------------------
        // The hooks object must SUBCLASS the engine's draw shader
        // (`shader: draw.DrawVjFxMesh { fx_color: fn()... }`): applying a
        // plain `{...}` would REPLACE the whole shader def and leave it
        // without a vertex function — the one structurally fatal mistake.
        let shader_hooks = {
            let v = r.value(live_id!(shader));
            match v.as_object() {
                Some(obj) => {
                    let has_vertex =
                        !r.vm.bx.heap.value(obj, live_id!(vertex).into(), NoTrap).is_err();
                    if has_vertex {
                        Some(r.vm.bx.heap.new_object_ref(obj))
                    } else {
                        r.warnings.push(
                            "shader: must subclass the engine's draw shader — write \
                             shader: draw.DrawVjFxMesh { fx_color: fn()... } (or the \
                             engine's shader type); ignored"
                                .to_string(),
                        );
                        None
                    }
                }
                None => None,
            }
        };
        let frame_fn = {
            let v = r.value(live_id!(frame));
            v.as_object().map(|obj| r.vm.bx.heap.new_object_ref(obj))
        };
        if frame_fn.is_some() && !matches!(engine, Engine::Emitters(_)) {
            r.warnings
                .push("frame: tick is only run by the emitters engine".to_string());
        }

        // -- dial declarations ----------------------------------------------
        // dials: [{name: "SYNC", bind: "p0", default: 0.3}, ...] — see
        // [`DialDecl`]. Forgiving like everything else: a bad entry warns
        // and is skipped; no block = the engine's default set.
        let dial_objs = r.object_list(live_id!(dials));
        let mut dials = Vec::new();
        for obj in dial_objs {
            if dials.len() >= 4 {
                r.warnings.push("more than 4 dials — extras dropped".to_string());
                break;
            }
            let outer = r.obj;
            r.obj = obj;
            let label = r.string(live_id!(name)).unwrap_or_default();
            let bind = r.string(live_id!(bind)).unwrap_or_default();
            let index = match bind.as_str() {
                "p0" => Some(0usize),
                "p1" => Some(1),
                "p2" => Some(2),
                "p3" => Some(3),
                _ => None,
            };
            let default = r.f32(live_id!(default), 0.5).clamp(0.0, 1.0);
            match index {
                Some(index) if !label.is_empty() => {
                    dials.push(DialDecl { label, index, default })
                }
                _ => r.warnings.push(
                    "dials: each entry needs name: \"…\" and bind: \"p0\"..\"p3\" — skipped"
                        .to_string(),
                ),
            }
            r.obj = outer;
        }
        if dials.is_empty() {
            dials = engine_default_dials(&engine_name);
        }
        let mut p_defaults = [0.0f32; 4];
        for dial in &dials {
            p_defaults[dial.index] = dial.default;
        }
        params = [
            r.anim(live_id!(p0), p_defaults[0]),
            r.anim(live_id!(p1), p_defaults[1]),
            r.anim(live_id!(p2), p_defaults[2]),
            r.anim(live_id!(p3), p_defaults[3]),
        ];

        // -- engage profile -------------------------------------------------
        // engage: "triangle" (default) | "ramp" — how the host rides this
        // doc along the crossfader when it sits in the TRANSITION slot.
        let engage = match r.string(live_id!(engage)).as_deref() {
            None | Some("triangle") => EngageProfile::Triangle,
            Some("ramp") => EngageProfile::Ramp,
            Some(other) => {
                r.warnings
                    .push(format!("engage '{other}' unknown (triangle/ramp)"));
                EngageProfile::Triangle
            }
        };

        let warnings = std::mem::take(&mut r.warnings);
        Ok(EffectDoc {
            name,
            engine,
            warnings,
            engage,
            speed,
            beat_pulse,
            beat_rate,
            bar_beats,
            sway,
            sway_freq,
            twist,
            fog,
            glow,
            content,
            grow,
            grow_beats,
            params,
            dials,
            palette,
            cam,
            input0,
            stages,
            shader_hooks,
            frame_fn,
            fields,
            wind_field,
        })
    }
}
