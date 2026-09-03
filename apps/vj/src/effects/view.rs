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

use super::audio_tex::{bind_audio, AudioBinding};
use super::doc::{EffectDoc, GrowMode, StageCfg};
use super::engines::{CamPose, EmitterInst, Engine, ShaderKind};
use super::engines_charts::DrawVjFxCharts;
use super::engines_city::DrawVjFxCity;
use super::engines_copper::DrawVjFxCopper;
use super::engines_domino::DrawVjFxDomino;
use super::engines_duo::DrawVjFxDuo;
use super::engines_firefly::DrawVjFxFirefly;
use super::engines_flock::DrawVjFxFlock;
use super::engines_forge::DrawVjFxForge;
use super::engines_harmonograph::DrawVjFxHarmono;
use super::engines_jet::DrawVjFxJet;
use super::engines_pipes::DrawVjFxPipes;
use super::engines_raymarch::DrawVjFxRaymarch;
use super::engines_tiles::DrawVjFxTiles;
use super::engines_videomesh::DrawVjFxVideomesh;
use super::expr::Signals;
use super::mesh::FxMesh;
use super::post::{PostChain, PostDraws, ResolvedStage};
use super::shaders::*;
use super::sim::{
    DrawVjFxFluidAdvect, DrawVjFxFluidDiv, DrawVjFxFluidDye, DrawVjFxFluidJacobi,
    DrawVjFxFluidProject, DrawVjFxFluidView, DrawVjFxMeshField, DrawVjFxSimSwarm,
    DrawVjFxSimSwarmDraw, DrawVjFxSimWind, SimDraws, SimFields, SimKind,
};
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
    draw_forge: DrawVjFxForge,
    #[live]
    draw_copper: DrawVjFxCopper,
    #[live]
    draw_tiles: DrawVjFxTiles,
    #[live]
    draw_videomesh: DrawVjFxVideomesh,
    #[live]
    draw_flock: DrawVjFxFlock,
    #[live]
    draw_raymarch: DrawVjFxRaymarch,
    #[live]
    draw_duo: DrawVjFxDuo,
    #[live]
    draw_jet: DrawVjFxJet,
    #[live]
    draw_city: DrawVjFxCity,
    #[live]
    draw_pipes: DrawVjFxPipes,
    #[live]
    draw_charts: DrawVjFxCharts,
    /// THE CONTENT BACKDROP (shaders.rs): one far-plane quad of the channel
    /// video drawn UNDER the sparse-geometry families, so "the video is
    /// playing" reads at a glance instead of surviving only as a tint.
    /// Skipped entirely unless real content is bound AND the family asks
    /// for it — `content: 0` and standalone stay bit-exact classic.
    #[live]
    draw_backdrop: DrawVjFxBackdrop,
    #[live]
    draw_screen: DrawVjFxScreen,
    // Sim-field family (sim.rs): update quads + the stateful consumers.
    #[live]
    draw_mesh_field: DrawVjFxMeshField,
    #[live]
    draw_swarm: DrawVjFxSimSwarmDraw,
    #[live]
    draw_sim_wind: DrawVjFxSimWind,
    #[live]
    draw_sim_swarm: DrawVjFxSimSwarm,
    #[live]
    draw_fluid_advect: DrawVjFxFluidAdvect,
    #[live]
    draw_fluid_div: DrawVjFxFluidDiv,
    #[live]
    draw_fluid_jacobi: DrawVjFxFluidJacobi,
    #[live]
    draw_fluid_project: DrawVjFxFluidProject,
    #[live]
    draw_fluid_dye: DrawVjFxFluidDye,
    #[live]
    draw_fluid_view: DrawVjFxFluidView,
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
    draw_hold: DrawVjFxHold,
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
    /// The content backdrop's own geometry: one clip-space quad, built once.
    #[rust]
    backdrop_geometry: Option<Geometry>,
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
    /// DETERMINISTIC CAPTURE (parity harness — [`VjFxView::set_capture`]):
    /// (fixed dt, frame budget). `None` = the free-running wall clock.
    #[rust]
    capture: Option<(f64, u32)>,
    /// Frames advanced since the last document load in capture mode.
    #[rust]
    capture_frame: u32,
    /// Audio levels fed by the host ([energy, bass, mid, high]); zero until
    /// something feeds them (the contract exists ahead of the wiring).
    #[rust]
    audio: [f32; 4],
    /// THE AUDIO PICTURE the whole engines layer samples (`audio_tex.rs`):
    /// the live spectrogram/waveform texture plus its layout uniforms, fed
    /// by the host every frame. `None` = no analysis running; every shader
    /// helper still returns a legal silent reading.
    #[rust]
    audio_bind: Option<AudioBinding>,

    /// Host overrides for the document's `p0..p3` user params (the effect
    /// slots' general knobs). `None` leaves the document's own — possibly
    /// music-bound — value in charge.
    #[rust]
    user_override: [Option<f32>; 4],
    /// Host multiplier on the document's own clock (the slots' SPD knob).
    #[rust(1.0f32)]
    speed_scale: f32,

    #[rust]
    input0: Option<Texture>,
    /// Texture input 1: deck B, for the two-deck transition engine. Never
    /// substituted by the fallback — an unbound deck samples black.
    #[rust]
    input1: Option<Texture>,
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
    /// HOST-DRIVEN CLOCK (the thumbnail renderer, fx_thumbs.rs): the widget
    /// stops advancing its own clock on `NextFrame` and moves exactly the
    /// document time a [`VjFxView::tick_manual`] call asks for. That lets a
    /// host render a fixed span of document time as fast as the GPU allows
    /// (and identically on any machine) instead of in wall-clock real time.
    #[rust]
    manual_clock: bool,

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
    /// Sim update shaders currently carrying a document's `update:`
    /// subclass (restored to base on the next load, like `hooked_kind`).
    #[rust]
    hooked_sim: Vec<SimKind>,
    /// The document's sim fields (float GPU state — sim.rs).
    #[rust]
    sim: SimFields,
    /// CPU encode cost of the last sim update (uniforms + pass encodes).
    #[rust]
    pub sim_ms: f32,

    /// THE BAKE POOL (thumbnail batching — fx_thumbs.rs): a pass object
    /// executes ONCE per paint, so the classic loop could only capture one
    /// frame per UI beat and a 34-step sheet cost 34 display beats of
    /// idle pacing. Each pooled step owns its own pass/draw-list/target
    /// set; the host swaps one in, encodes one captured frame into it, and
    /// several captured frames ride a single paint. Empty for every normal
    /// host — nothing on the live path touches it.
    #[rust]
    bake_pool: Vec<BakeStep>,
    /// True while a pooled step is swapped in (draw_scene switches its
    /// geometry-upload rule to the per-step version check).
    #[rust]
    bake_active: bool,
    /// The swapped-in step's last uploaded mesh version.
    #[rust]
    bake_geo_version: u64,
    /// The sequential bake path's staged scene (sig, chain input) awaiting
    /// its chain beat — see [`VjFxView::seq_scene_step`].
    #[rust]
    seq_staged: Option<(Signals, Texture)>,
    /// The sequential path's alternate scene target: scene N+1 encodes
    /// into the OTHER texture while step N's chain still reads this one,
    /// so both may share a paint with no ordering assumption at all.
    #[rust]
    seq_color_alt: Option<Texture>,
    /// Monotonic mesh content version: bumped on every dirty regen, so a
    /// pooled step knows whether its own geometry copy is current.
    #[rust]
    mesh_version: u64,
}

/// One pooled bake step: everything the offscreen render of ONE captured
/// frame must own so that S captured frames can be encoded into one paint
/// without aliasing each other — pass + draw list (instance buffers),
/// render targets, a post chain instance (its passes execute once per
/// paint too), the per-step animated fallback input (rewritten per tick:
/// a shared texture would make every step sample the LAST tick's
/// pattern), and a per-step geometry for engines that regen per frame
/// (one shared GPU buffer would leave every step drawing the last step's
/// mesh).
///
/// ORDERING LAW: pass execution order among siblings follows pass-pool id
/// order, which recycles — so within one paint, a step's CHAIN passes may
/// execute before its SCENE pass. Scene encodes and chain runs of the
/// same step therefore go in DIFFERENT paints (the host alternates
/// scene/chain beats for staged documents); a stage-less document's blit
/// reads the scene texture from the sheet pass, which as the passes'
/// PARENT always executes after them, so it may blit in the same paint.
struct BakeStep {
    pass: DrawPass,
    draw_list: DrawList,
    color: Texture,
    depth: Texture,
    post: PostChain,
    fallback: Option<Texture>,
    fallback_data: Vec<u32>,
    geometry: Option<Geometry>,
    geometry_version: u64,
    /// Captured at scene encode, replayed at the deferred chain run:
    /// (this step's signal vector, the chain's input texture).
    staged: Option<(Signals, Texture)>,
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
        // Pooled bake steps follow the document too: their chains carry the
        // same stage list, their geometry copies are stale by definition.
        // ONLY for documents that will actually batch: reconfiguring twelve
        // chains for a sequential document would churn the pass-id free
        // list for nothing — and sibling pass execution order follows pass
        // ids, so churn is never free (see the ordering law on `BakeStep`).
        if !self.bake_pool.is_empty() && !Self::doc_is_sequential(&doc) {
            let mut pool = std::mem::take(&mut self.bake_pool);
            for step in &mut pool {
                step.post.configure(cx, &doc.stages);
                step.geometry_version = 0;
                step.staged = None;
            }
            self.bake_pool = pool;
        } else {
            for step in &mut self.bake_pool {
                step.geometry_version = 0;
                step.staged = None;
            }
        }
        // Sim fields rebuilt for the new field list (sim.rs).
        self.sim.configure(cx, &doc.fields);
        // A previous document's hook overrides must not leak into this one:
        // restore that shader family to its registered base first.
        if let Some(kind) = self.hooked_kind.take() {
            self.apply_shader_value(cx, kind, None);
        }
        for kind in std::mem::take(&mut self.hooked_sim) {
            self.apply_sim_update(cx, kind, None);
        }
        // Apply the document's shader hooks (a SUBCLASS object of the base
        // shader — validated at parse): recompiles the family's draw shader
        // with the doc's fn overrides.
        if let Some(hooks) = &doc.shader_hooks {
            let value: ScriptValue = hooks.as_object().into();
            let kind = Self::effective_shader_kind(&doc);
            self.apply_shader_value(cx, kind, Some(value));
            self.hooked_kind = Some(kind);
        }
        // Field `update:` subclasses recompile the kind's update shader.
        for (kind, hook) in doc
            .fields
            .iter()
            .filter_map(|f| f.update_hook.as_ref().map(|h| (f.kind, h.as_object())))
            .collect::<Vec<_>>()
        {
            let value: ScriptValue = hook.into();
            self.apply_sim_update(cx, kind, Some(value));
            if !self.hooked_sim.contains(&kind) {
                self.hooked_sim.push(kind);
            }
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
        self.capture_frame = 0;
        // In capture mode the BEAT restarts with the document too. It
        // otherwise free-runs across the whole app session, which would
        // make a grab depend on how many documents were shown before it —
        // the one thing a deterministic capture must not do.
        if self.capture.is_some() {
            self.beat_pos = 0.0;
        }
        self.tick_error = None;
        self.seq_staged = None;
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
                        ShaderKind::Forge => live_id!(DrawVjFxForge),
                        ShaderKind::Copper => live_id!(DrawVjFxCopper),
                        ShaderKind::Tiles => live_id!(DrawVjFxTiles),
                        ShaderKind::VideoMesh => live_id!(DrawVjFxVideomesh),
                        ShaderKind::Flock => live_id!(DrawVjFxFlock),
                        ShaderKind::Raymarch => live_id!(DrawVjFxRaymarch),
                        ShaderKind::Duo => live_id!(DrawVjFxDuo),
                        ShaderKind::Jet => live_id!(DrawVjFxJet),
                        ShaderKind::City => live_id!(DrawVjFxCity),
                        ShaderKind::Pipes => live_id!(DrawVjFxPipes),
                        ShaderKind::Charts => live_id!(DrawVjFxCharts),
                        ShaderKind::MeshField => live_id!(DrawVjFxMeshField),
                        ShaderKind::SimSwarm => live_id!(DrawVjFxSimSwarmDraw),
                        ShaderKind::FluidView => live_id!(DrawVjFxFluidView),
                        ShaderKind::Screen => live_id!(DrawVjFxScreen),
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
                ShaderKind::Forge => {
                    self.draw_forge.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Copper => {
                    self.draw_copper.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Tiles => {
                    self.draw_tiles.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::VideoMesh => {
                    self.draw_videomesh.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Flock => {
                    self.draw_flock.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Raymarch => {
                    self.draw_raymarch.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Duo => {
                    self.draw_duo.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Jet => {
                    self.draw_jet.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::City => {
                    self.draw_city.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Pipes => {
                    self.draw_pipes.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Charts => {
                    self.draw_charts.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::MeshField => {
                    self.draw_mesh_field.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::SimSwarm => {
                    self.draw_swarm.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::FluidView => {
                    self.draw_fluid_view.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                ShaderKind::Screen => {
                    self.draw_screen.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
            }
        });
    }

    /// The shader family a document actually draws with: the engine's kind,
    /// except mesh-family docs that opt into a wind SIM FIELD
    /// (`wind_field:`), which draw with the field-sampling mesh shader.
    fn effective_shader_kind(doc: &EffectDoc) -> ShaderKind {
        let kind = doc.engine.shader_kind();
        if kind == ShaderKind::Mesh && doc.wind_field.is_some() {
            ShaderKind::MeshField
        } else {
            kind
        }
    }

    /// THE CONTENT BACKDROP TABLE — how much of the channel video a family
    /// shows behind itself at `content: 1` (0 = no backdrop at all).
    ///
    /// A family earns a backdrop when its classic look is BRIGHT SPARSE
    /// GEOMETRY over a near-black field: no amount of tinting a few
    /// thousand thin triangles can read as a picture, so the picture goes
    /// behind them and the geometry plays over it (the murmuration and the
    /// stock tape shipped this structure first and are the two families a
    /// viewer could always name the clip in). Families that already fill
    /// the frame with surface — terrain, tube, city, marcher, tiles, the
    /// jet's range, the fluid dye, the transition decks — drape or project
    /// the video instead and take NO backdrop; a second copy behind them
    /// would only be invisible or muddy.
    fn backdrop_level(kind: ShaderKind) -> f32 {
        match kind {
            ShaderKind::Particles => 0.55,
            ShaderKind::Emitters => 0.55,
            ShaderKind::Ribbon => 0.50,
            // l-system / grass / metaballs (and the wind-field twin).
            ShaderKind::Mesh | ShaderKind::MeshField => 0.45,
            ShaderKind::SimSwarm => 0.55,
            ShaderKind::Firefly => 0.45,
            ShaderKind::Harmono => 0.50,
            ShaderKind::Pipes => 0.50,
            ShaderKind::Forge => 0.50,
            ShaderKind::Copper => 0.40,
            ShaderKind::Domino => 0.45,
            // Frame-filling families: they drape/project the video
            // themselves, or already own a backdrop quad of their own
            // (Flock, Charts), or are content-native (Tiles, Duo, Fluid).
            ShaderKind::Terrain
            | ShaderKind::Tunnel
            | ShaderKind::Jet
            | ShaderKind::City
            | ShaderKind::Raymarch
            | ShaderKind::Tiles
            | ShaderKind::VideoMesh
            | ShaderKind::Flock
            | ShaderKind::Charts
            | ShaderKind::Duo
            | ShaderKind::Screen
            | ShaderKind::FluidView => 0.0,
        }
    }

    /// THE CLIP-SPACE QUAD, built once and shared by every fullscreen
    /// draw (the content backdrop and the `screen` family's own pass). It
    /// lives in clip space, so no camera and no pass size can invalidate
    /// it; uv runs (0,0) top-left.
    fn clip_quad_id(&mut self, cx: &mut Cx) -> GeometryId {
        if self.backdrop_geometry.is_none() {
            let mut quad = FxMesh::default();
            let n0 = vec3f(0.0, 0.0, 1.0);
            let a = quad.push_vert(vec3f(-1.0, -1.0, 0.0), 0.0, n0, 0.0, vec2f(0.0, 1.0), 0.0, 0.0);
            let b = quad.push_vert(vec3f(1.0, -1.0, 0.0), 1.0, n0, 0.0, vec2f(1.0, 1.0), 0.0, 0.0);
            let c = quad.push_vert(vec3f(1.0, 1.0, 0.0), 2.0, n0, 0.0, vec2f(1.0, 0.0), 0.0, 0.0);
            let d = quad.push_vert(vec3f(-1.0, 1.0, 0.0), 3.0, n0, 0.0, vec2f(0.0, 0.0), 0.0, 0.0);
            quad.push_quad(a, b, c, d);
            let g = Geometry::new(cx);
            quad.upload_clone(cx, &g);
            self.backdrop_geometry = Some(g);
        }
        self.backdrop_geometry.as_ref().expect("clip quad").geometry_id()
    }

    /// Apply a field's `update:` subclass onto that kind's update shader;
    /// `None` restores the registered base (mirror of `apply_shader_value`).
    fn apply_sim_update(&mut self, cx: &mut Cx, kind: SimKind, value: Option<ScriptValue>) {
        cx.with_vm(|vm| {
            let value = match value {
                Some(v) => v,
                None => {
                    let name = match kind {
                        SimKind::Wind => live_id!(DrawVjFxSimWind),
                        SimKind::Swarm => live_id!(DrawVjFxSimSwarm),
                        // The fluid's subclassable pass is the advect step
                        // (its force/motion character).
                        SimKind::Fluid => live_id!(DrawVjFxFluidAdvect),
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
                SimKind::Wind => {
                    self.draw_sim_wind.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                SimKind::Swarm => {
                    self.draw_sim_swarm.script_apply(vm, &Apply::Eval, &mut scope, value)
                }
                SimKind::Fluid => {
                    self.draw_fluid_advect.script_apply(vm, &Apply::Eval, &mut scope, value)
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

    /// The loaded document's declared dials (name + p-index + default) —
    /// the host labels its knobs from these. Empty = no meaningful levers.
    pub fn dials(&self) -> &[super::doc::DialDecl] {
        self.doc.as_ref().map(|d| d.dials.as_slice()).unwrap_or(&[])
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

    /// THE AUDIO PICTURE (see `audio_tex.rs`): the live spectrogram +
    /// waveform texture, its layout uniforms, and the same analysis as the
    /// four binding levels. One call feeds both the shader side and the
    /// document-binding side, so a host can never wire up half of it.
    pub fn set_audio(&mut self, binding: Option<AudioBinding>) {
        if let Some(b) = &binding {
            self.audio = b.levels;
        }
        self.audio_bind = binding;
    }

    /// Texture input 0 (the "extra slot" — a still, or the channel's main
    /// content in effect-pass mode).
    pub fn set_input_texture(&mut self, index: usize, texture: Option<Texture>) {
        match index {
            0 => self.input0 = texture,
            1 => self.input1 = texture,
            _ => {}
        }
    }

    /// True when the loaded document is a two-deck transition — the host
    /// should feed deck A into input 0 and deck B into input 1, and drive
    /// `p3` with the crossfader position (not the triangle). The duo engine
    /// always is; a videomesh doc opts in with `decks: 2`.
    pub fn wants_deck_inputs(&self) -> bool {
        match self.doc.as_ref().map(|d| &d.engine) {
            Some(Engine::Duo(_)) => true,
            Some(Engine::VideoMesh(e)) => e.cfg.decks == 2,
            _ => false,
        }
    }

    /// A screen-engine doc: its whole look is a function of the content on
    /// input 0, so a preview bake must feed it a structured picture.
    pub fn is_screen_engine(&self) -> bool {
        matches!(self.doc.as_ref().map(|d| &d.engine), Some(Engine::Screen))
    }

    /// True when the doc declared `engage: "ramp"` — an overlay/key
    /// transition that stays fully applied at the fader's B end instead of
    /// handing back the plain deck (see [`super::doc::EngageProfile`]).
    pub fn engage_ramp(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.engage == super::doc::EngageProfile::Ramp)
    }

    /// Host overrides for the document's `p0..p3` user params; `None` per
    /// entry keeps the document's binding.
    pub fn set_user_override(&mut self, over: [Option<f32>; 4]) {
        self.user_override = over;
    }

    /// Host multiplier on the document's clock (1.0 = the document's own
    /// `speed`). Clamped to a sane performance range.
    pub fn set_speed_scale(&mut self, scale: f32) {
        self.speed_scale = scale.clamp(0.05, 20.0);
    }

    /// One user param for this frame: already resolved into the signal
    /// vector by `signals()` (host override, else the doc's binding).
    fn user_value(&self, index: usize, sig: &Signals) -> f32 {
        sig.0[Signals::P0 + index]
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

    /// Switch the clock to host control (see `manual_clock`). While on, the
    /// widget's own `NextFrame` tick advances nothing; the host calls
    /// [`VjFxView::tick_manual`] once per frame it wants rendered.
    pub fn set_manual_clock(&mut self, on: bool) {
        self.manual_clock = on;
        self.last_time = None;
    }

    /// Advance the document clock by `dt` SECONDS OF WALL TIME the host is
    /// pretending passed (the document's own `speed` and the host's
    /// `speed_scale` still scale it, exactly as the real clock does), then
    /// run the per-frame script tick. One call = one frame the host is about
    /// to draw. No-op unless [`VjFxView::set_manual_clock`] is on.
    pub fn tick_manual(&mut self, cx: &mut Cx, dt: f64) {
        if !self.manual_clock || self.paused {
            return;
        }
        if let Some(doc) = &self.doc {
            let speed = doc.speed as f64 * self.speed_scale as f64;
            self.local_time += dt * speed;
            self.frame_dt = (dt * speed) as f32;
        }
        if !self.host_beat {
            self.beat_pos += dt * self.bpm / 60.0;
        }
        self.run_frame_tick(cx);
    }

    /// True when the picture depends on the ITERATION HISTORY of the frames
    /// before it — sim fields carry GPU state, feedback stages re-project
    /// their own last output, hold stages latch a frame and show it back,
    /// emitters accumulate what the frame tick spawned. A host stepping a
    /// span of document time must feed those in small steps; everything
    /// else is a pure function of the clock and can be jumped straight to
    /// the moment it wants.
    pub fn needs_stepped_time(&self) -> bool {
        self.doc.as_ref().is_some_and(|doc| {
            !doc.fields.is_empty()
                || doc.frame_fn.is_some()
                || matches!(doc.engine, Engine::Emitters(_))
                || doc.stages.iter().any(|s| {
                    matches!(s, StageCfg::Feedback { .. } | StageCfg::Hold { .. })
                })
        })
    }

    /// DETERMINISTIC CAPTURE MODE — the parity instrument.
    ///
    /// With a capture set, the widget ignores the wall clock: every frame
    /// after a document load advances the document by exactly `dt`
    /// seconds, and after `frames` of them the widget FREEZES (stops
    /// advancing and stops asking for frames), so whatever the grab lands
    /// on is a pure function of the document plus (dt, frames). That is
    /// what makes a before/after render comparison meaningful across
    /// machines and load: feedback stages, CPU sims and the emitters
    /// script tick all see the same frame sequence every run.
    ///
    /// `None` restores the free-running wall clock (the live default).
    pub fn set_capture(&mut self, capture: Option<(f64, u32)>) {
        self.capture = capture;
        self.capture_frame = 0;
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
        // p0..p3 as signals, so any binding can route a dial ("0.4+p1*1.2").
        // Host override (a touched knob) wins; else the doc's own p binding,
        // evaluated against the BASE signals — a p-param's own binding sees
        // the other p's as 0, never itself (no cycles by construction).
        let base = Signals(s);
        for i in 0..4 {
            s[Signals::P0 + i] = self.user_override[i].unwrap_or_else(|| {
                doc.map(|d| d.params[i].value(&base)).unwrap_or(0.0)
            });
        }
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
            Engine::Forge(e) => {
                let r = e.cfg.radius.clamp(1.0, 40.0);
                (vec3f(0.0, 1.1, 0.0), r * 2.15, r * 0.95)
            }
            Engine::Copper(e) => (
                vec3f(0.0, 0.0, 0.0),
                e.cfg.width.clamp(2.0, 80.0) * 0.85,
                e.cfg.span.clamp(0.5, 40.0) * 0.30,
            ),
            Engine::Flock(e) => {
                let b = if e.cfg.bound.is_finite() { e.cfg.bound.clamp(1.0, 40.0) } else { 6.0 };
                (vec3f(0.0, 0.0, 0.0), b * 2.0, b * 0.32)
            }
            Engine::Emitters(_) => (vec3f(0.0, 2.0, 0.0), 16.0, 4.0),
            // Mountain-jet: heightmap-style corridor framing (presets pin
            // cam_orbit: 0.0 so yaw stays 0 = a fixed camera); the jet
            // itself is view-space and always framed.
            Engine::MountainJet(e) => (
                vec3f(0.0, e.cfg.height * 0.35, -e.cfg.size * 0.22),
                e.cfg.size * 0.40,
                e.cfg.height * 1.15 + 1.4,
            ),
            Engine::Pipes(e) => {
                let ext = e.extent.clamp(0.5, 40.0);
                (vec3f(0.0, 0.0, 0.0), ext * 2.6, ext * 0.85)
            }
            // Stockcharts: frontal terminal framing (presets pin
            // cam_orbit: 0.0 for a locked screen, or a whisper for drift).
            Engine::Charts(e) => (
                vec3f(0.0, 0.0, 0.0),
                e.cfg.width.clamp(2.0, 60.0) * 0.72,
                e.cfg.height.clamp(1.0, 40.0) * 0.12,
            ),
            Engine::Swarm(e) => {
                let b = e.cfg.bound.clamp(1.0, 60.0);
                (vec3f(0.0, 0.0, 0.0), b * 2.0, b * 0.4)
            }
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
        let t0 = crate::clock::Instant::now();
        let (spawns, errors, used) = cx.with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            // Input snapshot: what the tick can see.
            let input = vm.bx.heap.new_object();
            let fields: [(LiveId, f64); 12] = [
                (live_id!(time), sig.0[Signals::TIME] as f64),
                (live_id!(dt), sig.0[Signals::DT] as f64),
                (live_id!(beat), sig.0[Signals::BEAT] as f64),
                (live_id!(phase), sig.0[Signals::PHASE] as f64),
                (live_id!(bar), sig.0[Signals::BAR] as f64),
                (live_id!(pulse), sig.0[Signals::PULSE] as f64),
                (live_id!(energy), sig.0[Signals::ENERGY] as f64),
                (live_id!(emitters), emitter_count as f64),
                // The resolved user params (dials): fx.p0..fx.p3, so the
                // tick can dial spawn rate/size like shaders read self.user.
                (live_id!(p0), sig.0[Signals::P0] as f64),
                (live_id!(p1), sig.0[Signals::P1] as f64),
                (live_id!(p2), sig.0[Signals::P2] as f64),
                (live_id!(p3), sig.0[Signals::P3] as f64),
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
                    // (mix, bands, stagger, trigger level) — post.rs reads
                    // the trigger as a rising edge, the rest as uniforms.
                    StageCfg::Hold { mix, bands, stagger, trigger, .. } => [
                        mix.value(sig).clamp(0.0, 1.0),
                        bands.value(sig).clamp(1.0, 256.0),
                        stagger.value(sig).clamp(0.0, 1.0),
                        trigger.value(sig),
                    ],
                };
                ResolvedStage { p }
            })
            .collect()
    }

    /// The `screen` family's own scene pass: one clip-space quad through
    /// the document's `fx_color`, into the scene colour texture the stage
    /// chain reads. Only reached when the document declared a `shader:`
    /// block (see the call site); the classic screen path has no pass.
    fn draw_screen_pass(&mut self, cx: &mut Cx2d, pass_rect: Rect, sig: &Signals, src: &Texture) {
        let quad_id = self.clip_quad_id(cx.cx);
        let (palette, fog_z) = match self.doc.as_ref() {
            Some(doc) => (
                doc.palette,
                // Same law as every family: the strength is host-gated to
                // 0 without REAL content, so a look can never accidentally
                // key off the animated fallback pattern.
                if self.input0.is_some() {
                    doc.content.value(sig).clamp(0.0, 1.0)
                } else {
                    0.0
                },
            ),
            None => return,
        };
        let doc_fog = self.doc.as_ref().map(|d| d.fog.value(sig)).unwrap_or(0.0);
        let doc_glow = self.doc.as_ref().map(|d| d.glow.value(sig)).unwrap_or(1.0);
        self.pass.set_size(cx.cx, pass_rect.size);
        self.pass.set_color_texture(
            cx.cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(palette[0]),
        );
        self.pass.set_depth_texture(
            cx.cx,
            &self.depth_texture,
            DrawPassClearDepth::ClearWith(1.0),
        );
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        self.pass.set_size(cx.cx, pass_rect.size);
        // THE VIEWPORT still has to be published even though this quad
        // lives in clip space and needs no camera: without it the pass
        // keeps a stale viewport and the frame renders into a strip.
        fx_set_pass_camera(
            cx.cx,
            &self.pass,
            &SceneState3D {
                time: self.local_time,
                camera_pos: vec3f(0.0, 0.0, 1.0),
                view: Mat4f::identity(),
                projection: Mat4f::identity(),
                viewport_rect: pass_rect,
            },
        );
        let cx3d = &mut Cx3d::new(cx.cx);
        // A DRAW LIST inside the pass: without it the instance lands in the
        // PARENT's current draw list and paints over the window instead of
        // into the pass texture (it did, and the frame came out black with
        // the effect smeared across the title bar).
        self.draw_list.begin_always(cx3d);
        cx3d.cx.draw_lists[self.draw_list.id()].draw_list_uniforms.view_transform =
            Mat4f::identity();
        let has_content = self.input0.is_some();
        let audio_bind = self.audio_bind.clone();
        let d = &mut self.draw_screen;
        // Same audio binding as every mesh engine gets (audio_tex.rs).
        if let Some(ab) = &audio_bind {
            bind_audio(cx3d.cx, &mut d.draw_vars, ab);
        }
        d.time_beat = vec4(
            sig.0[Signals::TIME],
            sig.0[Signals::BEAT],
            sig.0[Signals::PHASE],
            sig.0[Signals::PULSE],
        );
        d.sig = vec4(
            sig.0[Signals::BAR],
            sig.0[Signals::BPM],
            sig.0[Signals::ENERGY],
            sig.0[Signals::DT],
        );
        d.user = vec4(
            sig.0[Signals::P0],
            sig.0[Signals::P1],
            sig.0[Signals::P2],
            sig.0[Signals::P3],
        );
        d.col_bg = palette[0];
        d.col_a = palette[1];
        d.col_b = palette[2];
        d.col_c = palette[3];
        d.fog = vec4(doc_fog, doc_glow, fog_z, 0.0);
        d.draw_vars.set_texture(0, src);
        d.draw_vars.set_uniform(
            cx3d.cx,
            live_id!(has_content),
            &[if has_content { 1.0 } else { 0.0 }],
        );
        d.draw_vars.geometry_id = Some(quad_id);
        if d.draw_vars.can_instance() {
            let new_area = cx3d.add_instance(&d.draw_vars);
            d.draw_vars.area = cx3d.update_area_refs(d.draw_vars.area, new_area);
        }
        self.draw_list.end(cx3d);
        cx.end_pass(&self.pass);
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
        let t0 = crate::clock::Instant::now();
        let dirty = doc.engine.regen(self.frame_dt, time, phase, &mut self.mesh);
        if dirty {
            self.regen_ms = t0.elapsed().as_secs_f32() * 1000.0;
            self.mesh_version = self.mesh_version.wrapping_add(1);
            self.note_mesh_bounds();
        }
        // Upload when the mesh changed — and, on a pooled bake step, when
        // THIS step's geometry copy has never seen the current mesh (its
        // `geometry` was swapped in by bake_begin, so the upload below
        // lands in the step's own buffer, never a shared one).
        let upload = if self.bake_active {
            self.bake_geo_version != self.mesh_version
        } else {
            dirty
        };
        if upload {
            let geometry = self.geometry.get_or_insert_with(|| Geometry::new(cx.cx));
            if self.bake_active {
                // Pooled steps each carry their own geometry copy, and the
                // SAME mesh uploads into several of them. Recycle-upload
                // MOVES the mesh vectors into the geometry (handing back
                // the geometry's previous — empty — ones), so every step
                // after the first would upload nothing: one good cell,
                // twenty-nine black ones, and a vertex_buffer-None spam in
                // the log. Clone-upload keeps the CPU mesh authoritative.
                self.mesh.upload_clone(cx.cx, geometry);
            } else {
                self.mesh.upload_recycle(cx.cx, geometry);
            }
            self.bake_geo_version = self.mesh_version;
        }
        let Some(geometry) = &self.geometry else { return };
        let geometry_id = geometry.geometry_id();

        let backdrop_geometry_id = Some(self.clip_quad_id(cx.cx));

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

        // CONTENT COUPLING GATE: true only when REAL host content sits in
        // input0 (channel video in effect-pass mode). The animated fallback
        // pattern and a doc-requested sim field both gate to false, so a
        // coupling can never leak the test pattern into the standalone look.
        let has_content = {
            let field_request = self
                .doc
                .as_ref()
                .and_then(|d| d.input0.as_deref())
                .is_some_and(|s| s.starts_with("field:"));
            self.input0.is_some() && !field_request
        };
        let (eng_u, user, anim, fog, palette) = {
            let doc = self.doc.as_ref().unwrap();
            (
                doc.engine.uniforms(),
                vec4(
                    sig.0[Signals::P0],
                    sig.0[Signals::P1],
                    sig.0[Signals::P2],
                    sig.0[Signals::P3],
                ),
                vec4(
                    doc.sway.value(sig),
                    doc.sway_freq.value(sig),
                    growth,
                    doc.twist.value(sig),
                ),
                vec4(
                    doc.fog.value(sig).max(0.0),
                    doc.glow.value(sig),
                    // fog.z = the doc's `content` strength, pre-gated: 0
                    // means "exactly the classic standalone look" by law.
                    if has_content {
                        doc.content.value(sig).clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                    0.0,
                ),
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

        // Texture input 0: a doc-requested sim field wins ("field:<name>"),
        // then the host's texture, then the animated fallback.
        let field_input = self
            .doc
            .as_ref()
            .and_then(|d| d.input0.as_deref())
            .and_then(|s| s.strip_prefix("field:"))
            .and_then(|n| self.sim.texture(n));
        let input0 = match field_input.or_else(|| self.input0.clone()) {
            Some(tex) => Some(tex),
            None => Some(self.update_fallback_input(cx.cx, time, pulse)),
        };
        // Sim-field consumers: bind state textures + mapping uniforms
        // before the draw list opens.
        let (wind_field, state_field) = {
            let doc = self.doc.as_ref().unwrap();
            (
                doc.wind_field.clone(),
                match &doc.engine {
                    Engine::Swarm(e) => Some(e.cfg.state_field.clone()),
                    _ => None,
                },
            )
        };
        if let Some(wf) = &wind_field {
            if let Some(tex) = self.sim.texture(wf) {
                self.draw_mesh_field.draw_vars.set_texture(0, &tex);
            }
            if let Some(u) = self.sim.wind_uniform(wf) {
                self.draw_mesh_field.draw_vars.set_uniform(cx.cx, live_id!(u_wind), &u);
            }
        }
        if let Some(sf) = &state_field {
            if let Some(tex) = self.sim.texture(sf) {
                self.draw_swarm.draw_vars.set_texture(0, &tex);
            }
        }
        // Lifted out of `self` before the draw calls borrow their own
        // fields: the macro below cannot re-borrow `self` while it holds
        // `&mut self.draw_<family>`.
        let audio_bind = self.audio_bind.clone();
        let doc = self.doc.as_ref().unwrap();
        let cx3d = &mut Cx3d::new(cx.cx);
        self.draw_list.begin_always(cx3d);
        cx3d.cx.draw_lists[self.draw_list.id()].draw_list_uniforms.view_transform =
            Mat4f::identity();

        macro_rules! draw_engine {
            // Default: input0 (real content, else the animated fallback)
            // lands on texture slot 0 — the family's `tex0` — and the raw
            // `has_content` gate uniform is published (both no-ops for
            // shaders that declare neither).
            ($draw:expr) => {{
                let d = $draw;
                if let Some(tex) = &input0 {
                    d.draw_vars.set_texture(0, tex);
                }
                draw_engine!(@common d)
            }};
            // `notex`: the arm manages its own texture slots (duo decks,
            // sim consumers whose state texture owns slot 0).
            ($draw:expr, notex) => {{
                let d = $draw;
                draw_engine!(@common d)
            }};
            (@common $d:expr) => {{
                let d = $d;
                // THE AUDIO PICTURE, bound BY NAME on every engine (see
                // audio_tex.rs): whichever slot the family declared
                // `audio_tex` in gets the texture, and the three layout
                // uniforms ride along. A family that declares none is a
                // no-op — that is what keeps this one line uniform.
                if let Some(ab) = &audio_bind {
                    bind_audio(cx3d.cx, &mut d.draw_vars, ab);
                }
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
                d.draw_vars.set_uniform(
                    cx3d.cx,
                    live_id!(has_content),
                    &[if has_content { 1.0 } else { 0.0 }],
                );
                d.draw_vars.geometry_id = Some(geometry_id);
                if d.draw_vars.can_instance() {
                    let new_area = cx3d.add_instance(&d.draw_vars);
                    d.draw_vars.area = cx3d.update_area_refs(d.draw_vars.area, new_area);
                }
            }};
        }
        let kind = Self::effective_shader_kind(doc);

        // THE CONTENT BACKDROP, drawn FIRST so every engine paints over it.
        // Three gates, all of which must hold: real content bound, a
        // non-zero `content` strength, and a family that asked for one.
        // Skipping the draw is what makes `content: 0` bit-exact classic.
        let bd_level = Self::backdrop_level(kind);
        if has_content && fog.z > 0.001 && bd_level > 0.0 {
            if let (Some(tex), Some(bg_id)) = (&input0, backdrop_geometry_id) {
                let d = &mut self.draw_backdrop;
                d.draw_vars.set_texture(0, tex);
                d.time_beat = time_beat;
                d.sig = sig_v;
                d.col_bg = palette[0];
                // fog.w = the family's dim; fog.z = pre-gated strength.
                d.fog = vec4(fog.x, fog.y, fog.z, bd_level);
                d.draw_vars.geometry_id = Some(bg_id);
                if d.draw_vars.can_instance() {
                    let new_area = cx3d.add_instance(&d.draw_vars);
                    d.draw_vars.area = cx3d.update_area_refs(d.draw_vars.area, new_area);
                }
            }
        }

        match kind {
            ShaderKind::Mesh => draw_engine!(&mut self.draw_mesh),
            ShaderKind::MeshField => {
                // wind_tex owns slot 0; content rides slot 1 (`tex0`).
                if let Some(tex) = &input0 {
                    self.draw_mesh_field.draw_vars.set_texture(1, tex);
                }
                draw_engine!(&mut self.draw_mesh_field, notex)
            }
            ShaderKind::SimSwarm => {
                // state_tex owns slot 0; content rides slot 1 (`tex0`).
                if let Some(tex) = &input0 {
                    self.draw_swarm.draw_vars.set_texture(1, tex);
                }
                draw_engine!(&mut self.draw_swarm, notex)
            }
            // Fluid and screen never reach the mesh scene pass (draw_walk
            // routes them through the fluid view shader / the screen quad
            // pass, both of which run before this one is even begun).
            ShaderKind::FluidView | ShaderKind::Screen => {}
            ShaderKind::Terrain => draw_engine!(&mut self.draw_terrain),
            ShaderKind::Ribbon => draw_engine!(&mut self.draw_ribbon),
            ShaderKind::Tunnel => draw_engine!(&mut self.draw_tunnel),
            ShaderKind::Firefly => draw_engine!(&mut self.draw_firefly),
            ShaderKind::Harmono => draw_engine!(&mut self.draw_harmono),
            ShaderKind::Forge => draw_engine!(&mut self.draw_forge),
            ShaderKind::Copper => draw_engine!(&mut self.draw_copper),
            ShaderKind::Domino => {
                // Content projection footprint: the layout's xz half-extent
                // (the shader maps world xz over it to sample input0).
                if let Engine::Domino(e) = &doc.engine {
                    self.draw_domino.draw_vars.set_uniform(
                        cx3d.cx,
                        live_id!(u_area),
                        &[e.extent.clamp(1.0, 200.0)],
                    );
                }
                draw_engine!(&mut self.draw_domino)
            }
            ShaderKind::Tiles => {
                // The tiles-only instance block (`extrude` today): both of
                // the shared engine uniform lanes are already full.
                if let Engine::Tiles(e) = &doc.engine {
                    self.draw_tiles.tile = e.extra();
                }
                draw_engine!(&mut self.draw_tiles)
            }
            ShaderKind::VideoMesh => {
                // tex1 = deck B for the two-deck transition variant; an
                // unbound deck samples empty, never the animated fallback
                // (input0 still rides the default macro arm on slot 0).
                match &self.input1 {
                    Some(tex) => self.draw_videomesh.draw_vars.set_texture(1, tex),
                    None => self.draw_videomesh.draw_vars.empty_texture(1),
                }
                draw_engine!(&mut self.draw_videomesh)
            }
            ShaderKind::Flock => draw_engine!(&mut self.draw_flock),
            ShaderKind::Raymarch => {
                // The marcher builds its rays in-shader; it needs only the
                // viewport aspect (the pass camera is ignored).
                self.draw_raymarch.rm = vec4(aspect, 0.0, 0.0, 0.0);
                draw_engine!(&mut self.draw_raymarch)
            }
            ShaderKind::Duo => {
                // Deck A in 0, deck B in 1; a missing deck samples empty
                // (never the animated fallback — a transition must show
                // the real decks or nothing).
                match &self.input0 {
                    Some(tex) => self.draw_duo.draw_vars.set_texture(0, tex),
                    None => self.draw_duo.draw_vars.empty_texture(0),
                }
                match &self.input1 {
                    Some(tex) => self.draw_duo.draw_vars.set_texture(1, tex),
                    None => self.draw_duo.draw_vars.empty_texture(1),
                }
                self.draw_duo.rm = vec4(aspect, 0.0, 0.0, 0.0);
                draw_engine!(&mut self.draw_duo, notex)
            }
            ShaderKind::Jet => draw_engine!(&mut self.draw_jet),
            ShaderKind::City => draw_engine!(&mut self.draw_city),
            ShaderKind::Pipes => draw_engine!(&mut self.draw_pipes),
            ShaderKind::Charts => draw_engine!(&mut self.draw_charts),
            ShaderKind::Particles => draw_engine!(&mut self.draw_particles),
            ShaderKind::Emitters => {
                if let Engine::Emitters(e) = &doc.engine {
                    let d = &mut self.draw_emitter;
                    if let Some(ab) = &audio_bind {
                        bind_audio(cx3d.cx, &mut d.draw_vars, ab);
                    }
                    d.time_beat = time_beat;
                    d.sig = sig_v;
                    d.fog = fog;
                    if let Some(tex) = &input0 {
                        d.draw_vars.set_texture(0, tex);
                    }
                    d.draw_vars.set_uniform(
                        cx3d.cx,
                        live_id!(has_content),
                        &[if has_content { 1.0 } else { 0.0 }],
                    );
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
    // A direct camera write: the web backend uploads a pass block only when
    // its generation moved.
    cx.passes[pass.draw_pass_id()].mark_pass_uniforms_dirty();
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
        if self.next_frame.is_event(event).is_some() && self.live && !self.manual_clock {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            let mut dt = (time - last).clamp(0.0, 0.1);
            // Deterministic capture: fixed step for a fixed budget, then
            // freeze (no clock, no redraw) so the grab is reproducible.
            if let Some((fixed, frames)) = self.capture {
                if self.capture_frame >= frames {
                    return;
                }
                self.capture_frame += 1;
                dt = fixed;
            }
            if !self.paused {
                if let Some(doc) = &self.doc {
                    let speed = doc.speed as f64 * self.speed_scale as f64;
                    self.local_time += dt * speed;
                    self.frame_dt = (dt * speed) as f32;
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
        let is_fluid = matches!(self.doc.as_ref().map(|d| &d.engine), Some(Engine::Fluid(_)));

        // -- sim fields (updated BEFORE anything samples them) -------------
        if !self.sim.is_empty() {
            let palette = self.doc.as_ref().map(|d| d.palette).unwrap_or_default();
            self.sim.update(
                cx,
                &sig,
                &palette,
                &mut SimDraws {
                    wind: &mut self.draw_sim_wind,
                    swarm: &mut self.draw_sim_swarm,
                    fluid_advect: &mut self.draw_fluid_advect,
                    fluid_div: &mut self.draw_fluid_div,
                    fluid_jacobi: &mut self.draw_fluid_jacobi,
                    fluid_project: &mut self.draw_fluid_project,
                    fluid_dye: &mut self.draw_fluid_dye,
                },
            );
            self.sim_ms = self.sim.last_ms();
        }

        let chain_input = self.render_scene(cx, pass_rect, &sig);
        let final_texture = self.render_chain(cx, pass_rect, &sig, chain_input);
        self.out_texture = Some(final_texture.clone());

        // -- present -------------------------------------------------------
        if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            self.draw_present.draw_vars.set_texture(0, &final_texture);
            self.draw_present.draw_abs(cx, rect);
            self.area = self.draw_present.area();
            if !is_screen && !is_fluid {
                cx.set_pass_area(&self.pass, self.area);
            }
        }
        DrawStep::done()
    }
}

impl VjFxView {
    /// The offscreen SCENE HALF of one frame: the engine's own pass (or the
    /// screen family's quad pass / the fluid view pass), producing the
    /// texture the stage chain reads. Extracted from `draw_walk` verbatim so
    /// the bake seam can encode it into a pooled step.
    fn render_scene(&mut self, cx: &mut Cx2d, pass_rect: Rect, sig: &Signals) -> Texture {
        let sig = *sig;
        let is_screen = matches!(self.doc.as_ref().map(|d| &d.engine), Some(Engine::Screen));
        let is_fluid = matches!(self.doc.as_ref().map(|d| &d.engine), Some(Engine::Fluid(_)));
        // -- scene pass (skipped by the fullscreen `screen` engine) --------
        if is_screen {
            let src = match self.input0.clone() {
                Some(tex) => tex,
                None => {
                    let t = sig.0[Signals::TIME];
                    let p = sig.0[Signals::PULSE];
                    self.update_fallback_input(cx.cx, t, p)
                }
            };
            // THE FULLSCREEN FAMILY. Classic path: input0 goes STRAIGHT
            // into the stage chain, no scene pass at all — that is what the
            // shipped `screen` presets are, and routing them through a blit
            // would resample the input before the chain ever saw it. A
            // document that declares its own `shader:` opts into a real
            // pass instead: one clip-space quad whose whole fragment is the
            // doc's `fx_color` (shaders.rs `DrawVjFxScreen`), with the
            // stage chain still running on top.
            if self.doc.as_ref().is_some_and(|d| d.shader_hooks.is_some()) {
                self.draw_screen_pass(cx, pass_rect, &sig, &src);
                self.color_texture.clone()
            } else {
                src
            }
        } else if is_fluid {
            // Fluid: the scene pass IS the field's view shader (dye through
            // the palette over an optionally dye-warped input0).
            let input0 = match self.input0.clone() {
                Some(tex) => tex,
                None => {
                    let t = sig.0[Signals::TIME];
                    let p = sig.0[Signals::PULSE];
                    self.update_fallback_input(cx.cx, t, p)
                }
            };
            let (name, view_par, palette) = {
                let doc = self.doc.as_ref().unwrap();
                let name = match &doc.engine {
                    Engine::Fluid(e) => e.cfg.field.clone(),
                    _ => String::new(),
                };
                let glow = doc.glow.value(&sig).max(0.0);
                let warp = self.user_value(0, &sig);
                // CONTENT COUPLING: with real channel video bound, the
                // doc's `content` strength floors the texmix dial — the
                // clip shows through the dye wash by default. Standalone
                // (fallback input) keeps the dial's classic behavior.
                let content = if self.input0.is_some() {
                    doc.content.value(&sig).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let texmix = self.user_value(1, &sig).clamp(0.0, 1.0).max(content);
                (name, [glow, warp, texmix, 0.0], doc.palette)
            };
            // The fluid family's view pass is its own encode, so it takes
            // the same audio binding here rather than through the engines
            // macro (audio_tex.rs).
            if let Some(ab) = self.audio_bind.clone() {
                bind_audio(cx, &mut self.draw_fluid_view.draw_vars, &ab);
            }
            match self.sim.fluid_view(
                cx,
                &name,
                pass_rect.size,
                &mut self.draw_fluid_view,
                &input0,
                view_par,
                &palette,
            ) {
                Some(tex) => tex,
                None => self.color_texture.clone(),
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
            // This scene pass CONSUMES whatever sim state was updated this
            // frame (wind/swarm textures, `field:` inputs): parent each
            // field's final update pass under it so the graph — never
            // pass-pool id order — puts the sim ahead of its consumer.
            self.sim.link_consumer(cx, self.pass.draw_pass_id());
            cx.begin_pass(&self.pass, None);
            self.pass.set_size(cx, pass_rect.size);
            self.draw_scene(cx, pass_rect, &sig);
            cx.end_pass(&self.pass);
            self.color_texture.clone()
        }
    }

    /// The CHAIN HALF: the document's stage list over the scene output. A
    /// stage-less document hands the input straight through (no pass at
    /// all). Extracted from `draw_walk` verbatim for the bake seam.
    fn render_chain(
        &mut self,
        cx: &mut Cx2d,
        pass_rect: Rect,
        sig: &Signals,
        chain_input: Texture,
    ) -> Texture {
        if self.post.is_empty() {
            return chain_input;
        }
        let resolved = self.resolve_stages(sig);
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
                hold: &mut self.draw_hold,
            },
        )
    }

    /// Mesh bounds captured on the first non-empty regen (l-system
    /// auto-framing reads them).
    fn note_mesh_bounds(&mut self) {
        if self.bounds.is_some() || self.mesh.verts.is_empty() {
            return;
        }
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

    // -- THE BAKE SEAM (fx_thumbs.rs — see `BakeStep`) ---------------------

    /// Grow the bake pool to `n` steps. Pool passes keep their camera
    /// matrix exactly like the view's own pass.
    ///
    /// A step created here configures its chain from the CURRENT document
    /// straight away (batched documents only — the same gate as
    /// `set_effect_source`'s pool reconfigure). Job startup loads the
    /// document BEFORE the pool exists on a lane's first job, so without
    /// this that job's pooled steps carried an empty `PostChain::new()`
    /// and baked the raw scene with no stages — a subtly wrong sheet,
    /// cached forever under the revision key.
    pub fn bake_pool_ensure(&mut self, cx: &mut Cx, n: usize) {
        while self.bake_pool.len() < n {
            let pass = DrawPass::new_with_name(cx, "vjfx_bake_step");
            cx.passes[pass.draw_pass_id()].keep_camera_matrix = true;
            let color = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
            );
            let depth = Texture::new_with_format(
                cx,
                TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true },
            );
            let mut post = PostChain::new();
            if let Some(doc) = &self.doc {
                if !Self::doc_is_sequential(doc) {
                    post.configure(cx, &doc.stages);
                }
            }
            self.bake_pool.push(BakeStep {
                pass,
                draw_list: DrawList::new(cx),
                color,
                depth,
                post,
                fallback: None,
                fallback_data: Vec::new(),
                geometry: None,
                geometry_version: 0,
                staged: None,
            });
        }
    }

    /// Release the pool's MEMORY without releasing its IDENTITY: textures,
    /// geometry copies and staged frames drop (the render-target bytes hand
    /// back; fresh Auto-sized handles cost nothing until used), while the
    /// pass, draw-list and chain OBJECTS live for the widget's lifetime.
    /// Destroying them would push their pass ids into the recycler, and
    /// sibling pass EXECUTION ORDER follows pass ids — the first build of
    /// this pool freed and re-allocated on every idle/active edge, which
    /// scrambled the id bands the sequential documents' own chain and sim
    /// passes depend on and corrupted their sheets (alternating feedback
    /// trails, black fluid bakes). Identity is permanent BY LAW.
    pub fn bake_pool_release_memory(&mut self, cx: &mut Cx) {
        for step in &mut self.bake_pool {
            step.color = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
            );
            step.depth = Texture::new_with_format(
                cx,
                TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true },
            );
            step.fallback = None;
            step.fallback_data = Vec::new();
            step.geometry = None;
            step.geometry_version = 0;
            step.staged = None;
        }
    }

    /// A document whose picture carries GPU state from frame to frame —
    /// sim fields, a feedback or hold stage — must run its passes once per
    /// paint in sequence.
    fn doc_is_sequential(doc: &EffectDoc) -> bool {
        !doc.fields.is_empty()
            || doc
                .stages
                .iter()
                .any(|s| matches!(s, StageCfg::Feedback { .. } | StageCfg::Hold { .. }))
    }

    /// How many capture steps of this DOCUMENT may share one paint. A
    /// sequential document (see [`Self::doc_is_sequential`]) batches at 1 —
    /// exactly the classic loop. Everything else batches up to the pool.
    pub fn bake_batch_limit(&self) -> usize {
        if self.bake_pool.is_empty() {
            return 1;
        }
        if self.doc.as_ref().is_some_and(Self::doc_is_sequential) {
            1
        } else {
            self.bake_pool.len()
        }
    }

    /// True when this document's batched steps need their chain run in the
    /// NEXT paint (see the ordering law on [`BakeStep`]): pass execution
    /// order among siblings is not encode order, so a chain encoded in the
    /// same paint as its scene may execute first and read a stale scene.
    /// Stage-less documents have no chain passes and blit straight from
    /// the scene texture, which the sheet pass (their parent) always reads
    /// after they ran.
    pub fn bake_needs_chain_beat(&self) -> bool {
        !self.post.is_empty()
    }

    /// CPU-only preroll advance for batchable documents: run the engine
    /// regen exactly as a drawn preroll frame would (same dt sequence,
    /// same state), skipping the render whose pixels nobody ever saw.
    /// Never used for sequential documents — their preroll draws feed
    /// feedback/sim state and keep the classic one-draw-per-paint path.
    pub fn bake_advance_only(&mut self) {
        let sig = self.signals();
        let time = sig.0[Signals::TIME];
        let phase = sig.0[Signals::PHASE];
        let Some(doc) = self.doc.as_mut() else { return };
        let dirty = doc.engine.regen(self.frame_dt, time, phase, &mut self.mesh);
        if dirty {
            self.mesh_version = self.mesh_version.wrapping_add(1);
            self.note_mesh_bounds();
        }
    }

    /// SEQUENTIAL TWO-BEAT SEAM — the same ordering law as the pooled
    /// steps, applied to the view's OWN pass set for documents that must
    /// step one frame per paint (feedback/hold/sim state). The scene (and
    /// the sim update feeding it) encodes in one paint; the chain runs in
    /// the NEXT, reading a scene that has settled — because sibling pass
    /// execution order follows recycled pass ids, a chain encoded beside
    /// its scene may execute first and read a stale frame (live this is a
    /// one-frame lag nobody sees; in a sheet bake it is cells that
    /// alternate between two time streams, which is a flickering tile).
    pub fn seq_scene_step(&mut self, cx: &mut Cx2d) {
        if self.doc.is_none() {
            return;
        }
        self.ensure_initialized(cx.cx);
        // PING-PONG the scene target: with alternating textures, this
        // paint's chain (reading LAST step's scene) and this scene encode
        // never touch the same texture, so the host may put a chain run
        // and the next scene into ONE paint — full one-step-per-paint
        // throughput with zero pass-order assumptions. Sim-field documents
        // must not pipeline (their scene IS the sim state) — the host
        // checks [`VjFxView::bake_seq_pipeline`] before sharing a paint,
        // but the ping-pong itself is always harmless.
        let alt = self.seq_color_alt.get_or_insert_with(|| {
            Texture::new_with_format(
                cx.cx,
                TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
            )
        });
        std::mem::swap(alt, &mut self.color_texture);
        let pass_rect =
            Rect { pos: dvec2(0.0, 0.0), size: self.slot_size.unwrap_or(SLOT_PASS) };
        let sig = self.signals();
        // Sim fields advance with the scene beat (their state IS the frame).
        if !self.sim.is_empty() {
            let palette = self.doc.as_ref().map(|d| d.palette).unwrap_or_default();
            self.sim.update(
                cx,
                &sig,
                &palette,
                &mut SimDraws {
                    wind: &mut self.draw_sim_wind,
                    swarm: &mut self.draw_sim_swarm,
                    fluid_advect: &mut self.draw_fluid_advect,
                    fluid_div: &mut self.draw_fluid_div,
                    fluid_jacobi: &mut self.draw_fluid_jacobi,
                    fluid_project: &mut self.draw_fluid_project,
                    fluid_dye: &mut self.draw_fluid_dye,
                },
            );
            self.sim_ms = self.sim.last_ms();
        }
        let chain_input = self.render_scene(cx, pass_rect, &sig);
        self.seq_staged = Some((sig, chain_input));
    }

    /// True when a sequential document's chain beat may SHARE a paint
    /// with the next scene encode: the ping-pong makes the textures
    /// disjoint, EXCEPT for sim-field documents, whose chain input is the
    /// sim's own state texture — the next scene's sim update would rewrite
    /// it under the chain. Those keep strict alternation.
    pub fn bake_seq_pipeline(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.fields.is_empty())
    }

    /// The deferred chain half of [`Self::seq_scene_step`]. For a
    /// stage-less document this is free (the scene texture comes straight
    /// back) and safe in the SAME paint; staged documents call it in the
    /// next. Returns the finished frame.
    pub fn seq_chain_step(&mut self, cx: &mut Cx2d) -> Option<Texture> {
        let (sig, chain_input) = self.seq_staged.take()?;
        let pass_rect =
            Rect { pos: dvec2(0.0, 0.0), size: self.slot_size.unwrap_or(SLOT_PASS) };
        let out = self.render_chain(cx, pass_rect, &sig, chain_input);
        self.out_texture = Some(out.clone());
        Some(out)
    }

    /// Swap pooled step `i`'s identity in (or back out — symmetric).
    fn bake_swap(&mut self, i: usize) {
        let mut pool = std::mem::take(&mut self.bake_pool);
        {
            let step = &mut pool[i];
            std::mem::swap(&mut step.pass, &mut self.pass);
            std::mem::swap(&mut step.draw_list, &mut self.draw_list);
            std::mem::swap(&mut step.color, &mut self.color_texture);
            std::mem::swap(&mut step.depth, &mut self.depth_texture);
            std::mem::swap(&mut step.post, &mut self.post);
            std::mem::swap(&mut step.fallback, &mut self.fallback_input);
            std::mem::swap(&mut step.fallback_data, &mut self.fallback_data);
            std::mem::swap(&mut step.geometry, &mut self.geometry);
            std::mem::swap(&mut step.geometry_version, &mut self.bake_geo_version);
        }
        self.bake_pool = pool;
    }

    /// Encode one captured frame's SCENE into pooled step `i` (the host
    /// has already advanced the clock with [`VjFxView::tick_manual`]).
    /// The step's signals and chain input are staged for the chain run.
    pub fn bake_scene_step(&mut self, cx: &mut Cx2d, i: usize) {
        if self.doc.is_none() || i >= self.bake_pool.len() {
            return;
        }
        self.ensure_initialized(cx.cx);
        let pass_rect =
            Rect { pos: dvec2(0.0, 0.0), size: self.slot_size.unwrap_or(SLOT_PASS) };
        self.bake_swap(i);
        self.bake_active = true;
        let sig = self.signals();
        let chain_input = self.render_scene(cx, pass_rect, &sig);
        self.bake_active = false;
        self.bake_swap(i);
        self.bake_pool[i].staged = Some((sig, chain_input));
    }

    /// Run pooled step `i`'s chain over its staged scene and hand back the
    /// finished frame. For a stage-less document this is free (the staged
    /// scene texture comes straight back) and safe in the SAME paint as
    /// the scene; staged documents call it in the NEXT paint.
    pub fn bake_chain_step(&mut self, cx: &mut Cx2d, i: usize) -> Option<Texture> {
        if self.doc.is_none() || i >= self.bake_pool.len() {
            return None;
        }
        let (sig, chain_input) = self.bake_pool[i].staged.take()?;
        let pass_rect =
            Rect { pos: dvec2(0.0, 0.0), size: self.slot_size.unwrap_or(SLOT_PASS) };
        self.bake_swap(i);
        self.bake_active = true;
        let out = self.render_chain(cx, pass_rect, &sig, chain_input);
        self.bake_active = false;
        self.bake_swap(i);
        Some(out)
    }
}
