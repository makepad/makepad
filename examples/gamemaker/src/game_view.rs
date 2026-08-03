//! GameView — the in-process game engine pane.
//!
//! Hosts the kid's game: evaluates `game.splash` in a dedicated splash isolate
//! (aichat-style incremental re-eval, so the world rebuilds live while the AI
//! streams edits), simulates a small fixed-step AABB world, renders it to an
//! offscreen 3D pass composited into the pane, and answers the agent harness
//! (`tools/ag`) through `.agent/` file RPC: peek captures, scripted input-tape
//! test runs, and an error/log round-trip so the AI can see what went wrong.
//!
//! The physics is deliberately tiny (gravity + axis-separated AABB sweeps —
//! the same vocabulary Godot's CharacterBody gave the AI). It is engine-sized
//! for kid platformers, not a solver.
// TODO(aigame): swap the mini-physics for libs/box3d once the xr Rapier->box3d
// port lands; the script API below is the stable surface.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[cfg(not(headless))]
use makepad_widgets::makepad_platform::event::GamepadState;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use makepad_widgets::widget_async::{CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID};
use makepad_widgets::*;

// The simulation core lives in makepad-game-sim now (M0 stage A extraction,
// game.md): entity/terrain/world data, the fixed-step physics and the spatial
// queries moved there verbatim. This file keeps the host: script isolate,
// verb dispatch, rendering, input devices, agent RPC.
use makepad_game_blocks::Blocks;
use makepad_game_sim::{
    collect_touches, step_world, BodyKind, CallbackSlot, Entity, GameTimer, GameWorld, HudBar,
    HudSlot, LabelDef, PadState, Part, SaveVal, SkyConfig, Terrain, TICK_DT,
};
use makepad_game_render::{
    draw_billboard_labels, draw_hud_overlay, scene_state as render_scene_state, set_pass_camera,
    CameraRig, DrawGameAlpha, DrawGameCube, DrawGameSky, DrawGameTerrain, DrawGameTexture,
    GameDraws, GameRenderer,
};

// The five game draw shaders (DrawGameTexture/Cube/Alpha/Sky/Terrain) and the
// whole scene/HUD render path moved to makepad-game-render (M0 stage B) —
// main.rs registers makepad_game_render::script_mod before this module's.
script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.GameViewBase = #(GameView::register_widget(vm))
    mod.widgets.GameView = set_type_default() do mod.widgets.GameViewBase{
        width: Fill
        height: Fill
        draw_hud +: {
            text_style: theme.font_bold{font_size: 22}
            color: #xffffffee
        }
        draw_label +: {
            text_style: theme.font_bold{font_size: 11}
            color: #xffffffdd
        }
        draw_dot +: {
            color: #xffffffb8
        }
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_alpha +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_terrain +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

const EVAL_INSTRUCTION_LIMIT: usize = 2_000_000;
const TICK_INSTRUCTION_LIMIT: usize = 500_000;
const AGENT_POLL_TICKS: u64 = 15;
const PEEK_SNAPS: usize = 4;
const PEEK_SNAP_GAP_TICKS: u64 = 18;

const GAME_PREFIX: &str = "use mod.prelude.widgets.*\n";

/// PERF: one profiler reporting window (~2s of frames). Everything is main-
/// thread wall-clock μs. "script" is on_tick + timers + on_touch calls into
/// the isolate, "physics" is step_world + touch collection, "house" is the
/// agent poll / save / log flush, "scene" is the 3D pass encode and "overlay"
/// the HUD/nametag 2D pass. The worst_* fields catch single-frame spikes —
/// stutter lives there, not in the averages.
#[derive(Default)]
struct PerfWindow {
    draws: u64,
    ticks: u64,
    script_us: u64,
    physics_us: u64,
    house_us: u64,
    scene_us: u64,
    worst_scene_us: u64,
    slab_us: u64,
    slab_rebuilds: u64,
    overlay_us: u64,
    worst_tick_us: u64,
    gap_ms_sum: f64,
    worst_gap_ms: f64,
    gaps: u64,
    static_instances: u64,
    dyn_instances: u64,
}

fn perf_us(t0: std::time::Instant) -> u64 {
    t0.elapsed().as_micros() as u64
}



/// Serialized form of the save map (micro_serde has no HashMap support).
#[derive(SerJson, DeJson, Default)]
struct SaveFile {
    nums: Vec<(String, f64)>,
    strs: Vec<(String, String)>,
}

/// One frame-indexed scripted input event (same shape as the Godot tapes).
#[derive(SerJson, DeJson, Clone, Default)]
pub struct TapeEvent {
    pub f: u64,
    pub press: Option<String>,
    pub release: Option<String>,
}

#[derive(SerJson, DeJson, Clone, Default)]
pub struct Tape {
    pub events: Vec<TapeEvent>,
    pub probe: Vec<String>,
}

#[derive(SerJson, DeJson, Clone, Default)]
struct TestRequest {
    frames: u64,
    tape: String,
    every: u64,
}

struct TestRun {
    frame: u64,
    frames: u64,
    capture_every: u64,
    tape: Tape,
    probe_lines: Vec<String>,
    captures: usize,
}

struct PeekRun {
    snaps_left: usize,
    next_at_tick: u64,
}



// Host-side slot table mapping the sim's opaque `CallbackSlot`s to script
// closures. It lived here character-for-character; it belongs to the shared
// binding layer, so this file uses that one.
use makepad_game_script::callbacks::CallbackTable;
use makepad_game_script::dispatch::{AudioRequest, Ctx as GameCtx, ToneWave};

/// Script tone handle -> live synth voice. Aliased because the `Script` derive
/// only accepts simple field type paths.
type ToneVoices = std::collections::HashMap<u64, u64>;

fn synth_wave(wave: ToneWave) -> crate::synth::Wave {
    match wave {
        ToneWave::Sine => crate::synth::Wave::Sine,
        ToneWave::Square => crate::synth::Wave::Square,
        ToneWave::Saw => crate::synth::Wave::Saw,
        ToneWave::Triangle => crate::synth::Wave::Triangle,
        ToneWave::Noise => crate::synth::Wave::Noise,
    }
}

/// Snapshot taken before a re-eval so a broken script never replaces a
/// working world ("last good" semantics).
struct WorldSnapshot {
    entities: Vec<Entity>,
    /// Restored on rollback so post-rollback spawns can't collide with (or
    /// sort under) surviving ids — the sorted-by-id invariant depends on it.
    next_id: u64,
    parts: Vec<Part>,
    labels: Vec<LabelDef>,
    terrain: Option<Terrain>,
    sky: Option<SkyConfig>,
    gravity: f32,
    on_tick: Option<CallbackSlot>,
    on_touch: Option<CallbackSlot>,
    timers: Vec<GameTimer>,
    hud_slots: Vec<(String, HudSlot)>,
    hud_bars: Vec<HudBar>,
    crosshair: bool,
    cam_target: Vec3f,
    cam_distance: f32,
    cam_follow: u64,
    cam_side: bool,
    cam_third: u64,
    cam_height: f32,
    cam_boom: f32,
    cam_chase: u64,
    cam_lag: f32,
    cam_recenter: f32,
    cam_speed_tighten: f32,
    cam_fov: f32,
    time: f64,
    /// Blocks travel with the world: a rolled-back world whose cars stayed
    /// behind would drive entities that no longer exist.
    blocks: Blocks,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct GameView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawGameTexture,
    #[live]
    draw_cube: DrawGameCube,
    #[live]
    draw_alpha: DrawGameAlpha,
    #[live]
    draw_sky: DrawGameSky,
    #[live]
    draw_terrain: DrawGameTerrain,
    #[live]
    draw_hud: DrawText,
    #[live]
    draw_label: DrawText,
    /// The crosshair dot (and any future flat overlay quads).
    #[live]
    draw_dot: DrawColor,
    #[live(vec4(0.03, 0.045, 0.075, 1.0))]
    clear_color: Vec4f,
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
    // PERF: per-phase frame profiler, always collected (a few Instant reads
    // per frame). F3 toggles the on-screen overlay, `ag perf` fetches the
    // report, AIGAME_PERF=1 also prints it to stderr every window.
    #[rust(std::env::var_os("AIGAME_PERF").is_some())]
    perf_log: bool,
    #[rust]
    perf: PerfWindow,
    #[rust]
    perf_report: String,
    #[rust(false)]
    perf_overlay: bool,
    #[rust(false)]
    perf_want_file: bool,
    // PerfGraph widget channel for the physics step (registered on first use).
    #[rust]
    perf_physics_ch: Option<PerfChannel>,
    /// GPU-side caches (shape geometries, static slabs, terrain mesh) —
    /// makepad-game-render owns the whole scene pass now.
    #[rust]
    renderer: GameRenderer,
    #[rust(Rc::new(RefCell::new(GameWorld::new())))]
    world: Rc<RefCell<GameWorld>>,
    /// Slot table for script callbacks (see CallbackTable). Shared with the
    /// dispatch closure the same way `world` is.
    #[rust]
    callbacks: Rc<RefCell<CallbackTable>>,
    /// Engine-side building blocks (cars, characters, brains, race kit).
    /// Shared with the dispatch closure like `world`; snapshotted beside it so
    /// a failed eval rolls both back together.
    #[rust]
    blocks: Rc<RefCell<Blocks>>,
    /// Current eval generation, mirrored for the dispatch closure so runtime
    /// callback registrations are tagged with the world that made them.
    #[rust]
    eval_gen_cell: Rc<std::cell::Cell<u64>>,
    /// Audio is not a sim concern: the verb table queues requests here and the
    /// host drains them into its synth (game_script cannot depend on an
    /// example's synth module).
    #[rust]
    audio: Rc<RefCell<Vec<AudioRequest>>>,
    /// Next sustained-tone handle. `tone()` must return an id synchronously,
    /// which a drained queue cannot do, so ids are minted script-side and this
    /// host maps them onto real synth voices.
    #[rust]
    next_tone: Rc<std::cell::Cell<u64>>,
    /// Script tone id -> synth voice id.
    #[rust]
    tone_voices: ToneVoices,
    /// Particle requests. gamemaker has no particle renderer (Arcade owns
    /// the pretty pass), so these are drained and dropped — the verbs still
    /// work, they just draw nothing here.
    #[rust]
    particles: Rc<RefCell<Vec<makepad_game_sim::ParticleRequest>>>,
    #[rust]
    next_emitter: Rc<std::cell::Cell<u64>>,
    #[rust]
    vm_id: SplashVmId,
    #[rust]
    body: String,
    #[rust]
    eval_generation: u64,
    #[rust]
    last_eval_ok: bool,
    /// Error text of the last failed eval (all classes the isolate can
    /// produce — parse, runtime, pod, and shader-compiler errors all flow
    /// through the same captured-error sink). None while the eval is clean.
    #[rust]
    last_eval_error: Option<String>,
    /// Where the current game lives; `.agent/` goes under it.
    #[rust]
    project_dir: Option<PathBuf>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time_accum: f64,
    #[rust]
    last_time: Option<f64>,
    // Orbit camera: the AUTHORITATIVE pose lives on GameWorld (orbit_yaw/
    // orbit_pitch/chase_hold — M0r ownership consolidation); the widget owns
    // only device-input accumulation below.
    #[rust]
    orbit_last_abs: Option<DVec2>,
    /// Buffered game.log lines (see append_log/flush_log_to_disk).
    #[rust]
    log_buf: String,
    /// .agent dir mtime at the last RPC poll — unchanged mtime means no new
    /// request files, so the poll is one stat instead of three exists().
    #[rust]
    agent_dir_mtime: Option<std::time::SystemTime>,
    /// Cumulative script-instruction budget left this tick (on_tick + timers
    /// + on_touch all draw from ONE pool of TICK_INSTRUCTION_LIMIT).
    #[rust]
    tick_budget_left: usize,
    #[rust]
    budget_exhausted_logged: bool,
    /// Mouse orbit delta accumulated between ticks, handed to the script as
    /// input.look_dx/look_dy (drag detection for chase cams).
    #[rust]
    look_accum: DVec2,
    /// Pane rect from the last draw, for mouse hit checks (raw mouse events,
    /// not the finger-hit system — same pattern as XrCamera's desktop orbit).
    #[rust]
    view_rect: Rect,
    #[rust]
    test_run: Option<TestRun>,
    #[rust]
    peek_run: Option<PeekRun>,
    /// Previous-tick gamepad button state, for press-edge detection.
    /// (The headless backend has no game input; the poll fn is stubbed there.)
    #[cfg(not(headless))]
    #[rust]
    pad_jump_prev: bool,
    #[cfg(not(headless))]
    #[rust]
    pad_shoot_prev: bool,
    #[cfg(not(headless))]
    #[rust]
    pad_grab_prev: bool,
    #[cfg(not(headless))]
    #[rust]
    pad_reset_prev: bool,
    /// Right analog stick, deadzoned, in mouse-drag pixel convention
    /// (x: drag right+, y: drag down+ — which this orbit reads as look up,
    /// so stick-up = look-up). Rotates the camera exactly like a drag.
    #[rust]
    pad_look: DVec2,
}

impl GameView {
    // ── setup ───────────────────────────────────────────────────────────

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        // NOTE: no world reset here — the startup eval may already have built
        // the world before the first draw. eval_body owns resets.
        self.next_frame = cx.new_next_frame();
    }

    pub fn set_project_dir(&mut self, dir: PathBuf) {
        // game.save/game.load live beside the app's other per-game state.
        let save_path = dir.join(".gamemaker").join("save.json");
        {
            let mut world = self.world.borrow_mut();
            world.save_data.clear();
            world.save_dirty = false;
            if let Ok(json) = std::fs::read_to_string(&save_path) {
                if let Ok(file) = SaveFile::deserialize_json(&json) {
                    for (k, v) in file.nums {
                        world.save_data.insert(k, SaveVal::Num(v));
                    }
                    for (k, v) in file.strs {
                        world.save_data.insert(k, SaveVal::Str(v));
                    }
                }
            }
            world.save_path = Some(save_path);
        }
        self.project_dir = Some(dir);
    }

    /// Write the save map through to disk (called from the tick, debounced to
    /// once a second so a per-tick game.save can't thrash the disk).
    fn flush_save(&mut self) {
        let (path, file) = {
            let mut world = self.world.borrow_mut();
            if !world.save_dirty {
                return;
            }
            world.save_dirty = false;
            let Some(path) = world.save_path.clone() else {
                return;
            };
            let mut file = SaveFile::default();
            for (k, v) in world.save_data.iter() {
                match v {
                    SaveVal::Num(n) => file.nums.push((k.clone(), *n)),
                    SaveVal::Str(s) => file.strs.push((k.clone(), s.clone())),
                }
            }
            // Deterministic file contents (HashMap order isn't).
            file.nums.sort_by(|a, b| a.0.cmp(&b.0));
            file.strs.sort_by(|a, b| a.0.cmp(&b.0));
            (path, file)
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, file.serialize_json());
    }

    fn agent_dir(&self) -> Option<PathBuf> {
        Some(self.project_dir.as_ref()?.join(".agent"))
    }

    // ── script isolate ──────────────────────────────────────────────────

    fn self_id(&self) -> usize {
        self as *const Self as usize
    }

    /// Feed (possibly streaming) game source. Evaluates incrementally like the
    /// Splash widget; a failed eval rolls the world back to the last good one
    /// and reports errors to `.agent/` for the AI.
    pub fn set_source(&mut self, cx: &mut Cx, source: &str) {
        if self.body == source {
            return;
        }
        self.body = source.to_string();
        self.eval_body(cx);
        self.redraw(cx);
    }

    #[allow(dead_code)]
    pub fn last_eval_ok(&self) -> bool {
        self.last_eval_ok
    }

    /// The last failed eval's error text, for the app to push back into the
    /// agent conversation. None when the current eval is clean.
    pub fn last_eval_error(&self) -> Option<&str> {
        self.last_eval_error.as_deref()
    }

    fn eval_body(&mut self, cx: &mut Cx) {
        if self.body.is_empty() {
            return;
        }
        if self.vm_id == MAIN_SPLASH_VM_ID {
            self.vm_id = cx.alloc_splash_vm_with_network(false);
            self.register_game_handle(cx);
        }
        self.eval_generation += 1;
        // Runtime registrations (dispatch) tag slots with this generation.
        self.eval_gen_cell.set(self.eval_generation);

        // Last-good: keep a copy of the world; the eval rebuilds from scratch.
        let snapshot = {
            let mut world = self.world.borrow_mut();
            let snapshot = WorldSnapshot {
                next_id: world.next_id,
                entities: std::mem::take(&mut world.entities),
                parts: std::mem::take(&mut world.parts),
                labels: std::mem::take(&mut world.labels),
                terrain: world.terrain.take(),
                sky: world.sky.take(),
                gravity: world.gravity,
                on_tick: world.on_tick,
                on_touch: world.on_touch,
                timers: std::mem::take(&mut world.timers),
                hud_slots: std::mem::take(&mut world.hud_slots),
                hud_bars: std::mem::take(&mut world.hud_bars),
                crosshair: world.crosshair,
                cam_target: world.cam_target,
                cam_distance: world.cam_distance,
                cam_follow: world.cam_follow,
                cam_side: world.cam_side,
                cam_third: world.cam_third,
                cam_height: world.cam_height,
                cam_boom: world.cam_boom,
                cam_chase: world.cam_chase,
                cam_lag: world.cam_lag,
                cam_recenter: world.cam_recenter,
                cam_speed_tighten: world.cam_speed_tighten,
                cam_fov: world.cam_fov,
                time: world.time,
                blocks: std::mem::take(&mut *self.blocks.borrow_mut()),
            };
            world.reset_content();
            snapshot
        };
        // reset_content is pure sim now: the host stops sustained audio
        // alongside it (a rebuilt world must never inherit a stuck hum).
        crate::synth::stop_all_tones();
        self.tone_voices.clear();
        // Anything the dying world queued dies with it.
        self.audio.borrow_mut().clear();

        let self_id = self.self_id();
        // The trailing "\n;" finalizes the stream: eval_with_append_source is a
        // STREAMING parser, so a last statement with no terminator is held back
        // as "possibly incomplete" and silently never runs — and game logic
        // (`on_tick`/`on_touch`) idiomatically sits last in the file. aichat
        // never hits this because its wrapper auto-closes with `}`. The empty
        // statement is harmless after any complete file. (bugs.md, my-game-5.)
        let code = format!("{}{}\n;", GAME_PREFIX, self.body);
        // Errors print `file:(row + line):col` — with `file: "game.splash"` and
        // `line: 0`, the 0-based parse row exactly cancels the one-line
        // GAME_PREFIX, so reported lines are REAL 1-based game.splash lines.
        // The widget identity keys the body via `column` (never printed; the
        // (file, line, column) tuple is only checkpoint identity).
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "game.splash".to_string(),
            line: 0,
            column: self_id,
            code: String::new(),
            values: vec![],
        };

        let vm_id = self.vm_id;
        let eval_t0 = std::time::Instant::now();
        let errors = cx.with_script_vm_id(vm_id, |vm| {
            // Install the captured-error sink: run_core drains errors as they
            // occur (and streaming evals silence them); the sink is the only
            // reliable way to get them back for the AI.
            vm.bx.captured_errors = Some(Vec::new());
            let _ = vm.with_instruction_limit(EVAL_INSTRUCTION_LIMIT, |vm| {
                vm.eval_with_append_source(script_mod, &code, NIL.into())
            });
            vm.take_errors()
        });
        // Evals are splash exec too — a hot-reload hitch shows as a script
        // spike on the PerfGraph.
        cx.perf_monitor
            .add(PERF_CHANNEL_SCRIPT, perf_us(eval_t0));

        let generation = self.eval_generation;
        if errors.is_empty() {
            self.last_eval_ok = true;
            self.last_eval_error = None;
            // The old world is gone for good: release every callback slot
            // earlier generations registered (the snapshot is dropped below).
            self.callbacks
                .borrow_mut()
                .free_generations_before(generation);
            let count = self.world.borrow().entities.len();
            self.append_log(&format!("eval #{generation}: ok, {count} entities"));
            self.write_agent_file("last_error.txt", "");
        } else {
            self.last_eval_ok = false;
            // The failed eval's registrations die with it; the snapshot's
            // (older-generation) slots stay live for the rollback below.
            self.callbacks.borrow_mut().free_generation(generation);
            // Roll back so the kid keeps the world that worked.
            {
                let mut world = self.world.borrow_mut();
                *self.blocks.borrow_mut() = snapshot.blocks;
                world.entities = snapshot.entities;
                world.next_id = snapshot.next_id;
                world.parts = snapshot.parts;
                world.labels = snapshot.labels;
                world.terrain = snapshot.terrain;
                world.sky = snapshot.sky;
                world.gravity = snapshot.gravity;
                world.on_tick = snapshot.on_tick;
                world.on_touch = snapshot.on_touch;
                world.timers = snapshot.timers;
                world.hud_slots = snapshot.hud_slots;
                world.hud_bars = snapshot.hud_bars;
                world.crosshair = snapshot.crosshair;
                world.cam_target = snapshot.cam_target;
                world.cam_distance = snapshot.cam_distance;
                world.cam_follow = snapshot.cam_follow;
                world.cam_side = snapshot.cam_side;
                world.cam_third = snapshot.cam_third;
                world.cam_height = snapshot.cam_height;
                world.cam_boom = snapshot.cam_boom;
                world.cam_chase = snapshot.cam_chase;
                world.cam_lag = snapshot.cam_lag;
                world.cam_recenter = snapshot.cam_recenter;
                world.cam_speed_tighten = snapshot.cam_speed_tighten;
                world.cam_fov = snapshot.cam_fov;
                // The rolled-back world keeps ITS clock (reset_content zeroed it).
                world.time = snapshot.time;
            }
            let joined = errors.join("\n");
            self.append_log(&format!("eval #{generation}: FAILED\n{joined}"));
            self.write_agent_file("last_error.txt", &format!("eval #{generation}\n{joined}\n"));
            self.last_eval_error = Some(joined);
        }
        // Top-level audio (a startup jingle) queued during eval: play it now
        // rather than holding it to the next tick. A failed eval's queue was
        // already discarded with its world.
        if self.last_eval_ok {
            self.drain_audio();
        } else {
            self.audio.borrow_mut().clear();
        }
        // Eval boundaries flush immediately — `ag logs`/`ag errors` right
        // after an edit must see the report, not a 1s-stale file.
        self.flush_log_to_disk();
        if self.last_eval_ok {
            cx.widget_action(self.uid, GameViewAction::EvalOk { generation });
        } else {
            cx.widget_action(
                self.uid,
                GameViewAction::EvalFailed {
                    generation,
                    error: self.last_eval_error.clone().unwrap_or_default(),
                },
            );
        }
    }

    /// Register the synchronous `game` native handle into this view's isolate.
    fn register_game_handle(&mut self, cx: &mut Cx) {
        let world = self.world.clone();
        // The verb table replaces the 84-arm `x if x == live_id!(..)` chain
        // this file used to carry: one hash lookup per call, and one
        // implementation shared with Arcade instead of two that drift.
        let ctx = GameCtx {
            world: self.world.clone(),
            blocks: self.blocks.clone(),
            callbacks: self.callbacks.clone(),
            audio: self.audio.clone(),
            eval_gen: self.eval_gen_cell.clone(),
            next_tone: self.next_tone.clone(),
            // Particles are device-local and gamemaker has no particle
            // renderer, so its queue is drained and dropped each eval.
            particles: self.particles.clone(),
            next_emitter: self.next_emitter.clone(),
        };
        let verbs = makepad_game_script::dispatch::verb_table();
        let vm_id = self.vm_id;
        cx.with_script_vm_id(vm_id, |vm| {
            let game_type = vm.new_handle_type(id_lut!(game));
            let dispatch_world = world.clone();
            vm.set_handle_call(game_type, move |vm, args, method| {
                if let Some(verb) = verbs.get(&method) {
                    return verb(vm, &ctx, args);
                }
                // Hard-fail an unknown verb: silently ignoring one costs whole
                // agent test cycles. Location + did-you-mean, like VM errors.
                let name = format!("{}", method);
                let loc = vm
                    .bx
                    .code
                    .ip_to_loc(vm.bx.threads.cur_ref().trap.ip)
                    .map(|l| format!("{l}: "))
                    .unwrap_or_default();
                let msg = match makepad_game_script::dispatch::suggest(&name) {
                    Some(s) => format!("{loc}unknown game verb '{name}'. Did you mean '{s}'?"),
                    None => {
                        format!("{loc}unknown game verb '{name}' — game.api() lists every verb")
                    }
                };
                dispatch_world.borrow_mut().log(msg.clone());
                if let Some(sink) = vm.bx.captured_errors.as_mut() {
                    sink.push(msg);
                }
                NIL
            });
            struct GameHandleGc;
            impl ScriptHandleGc for GameHandleGc {
                fn gc(&mut self) {}
            }
            let handle = vm.bx.heap.new_handle(game_type, Box::new(GameHandleGc));
            vm.set_injected_global(id!(game), handle.into());
        });
    }

    // ── logs / agent files ──────────────────────────────────────────────

    /// Buffered: lines land in memory and hit disk via flush_log_to_disk —
    /// once a second from the tick, immediately on eval/error boundaries.
    /// (The old per-line open/append was file I/O on the 60Hz tick path.)
    fn append_log(&mut self, line: &str) {
        use std::fmt::Write;
        let tick = self.world.borrow().tick;
        let _ = writeln!(self.log_buf, "[t{tick}] {line}");
    }

    fn flush_log_to_disk(&mut self) {
        if self.log_buf.is_empty() {
            return;
        }
        let Some(dir) = self.agent_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("game.log"))
        {
            let _ = file.write_all(self.log_buf.as_bytes());
        }
        self.log_buf.clear();
    }

    fn write_agent_file(&self, name: &str, contents: &str) {
        let Some(dir) = self.agent_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(name), contents);
    }

    fn state_report(&self) -> String {
        let world = self.world.borrow();
        let mut out = String::new();
        use std::fmt::Write;
        let _ = writeln!(out, "tick={} entities={}", world.tick, world.entities.len());
        for e in &world.entities {
            if e.kind == BodyKind::Static && e.tag.is_empty() {
                continue;
            }
            let _ = writeln!(
                out,
                "{} tag={} kind={} pos=({:.2},{:.2},{:.2}) vel=({:.2},{:.2},{:.2}) floor={}",
                e.id,
                if e.tag.is_empty() { "-" } else { &e.tag },
                match e.kind {
                    BodyKind::Static => "static",
                    BodyKind::Kinematic => "kinematic",
                    BodyKind::Mover => "mover",
                    BodyKind::Rigid => "rigid",
                },
                e.pos.x, e.pos.y, e.pos.z,
                e.vel.x, e.vel.y, e.vel.z,
                e.on_floor,
            );
        }
        out
    }

    // ── agent RPC (peek / test) ─────────────────────────────────────────

    fn poll_agent_requests(&mut self, cx: &mut Cx) {
        let Some(dir) = self.agent_dir() else { return };

        // Cheap gate: request files land as direct children of .agent/, so an
        // unchanged dir mtime means nothing new — one stat instead of three
        // exists() every poll. (Our own writes bump it too; that just costs
        // one full check on the next poll.)
        let mtime = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime == self.agent_dir_mtime {
            return;
        }
        self.agent_dir_mtime = mtime;

        // `ag perf`: answer with the NEXT completed profiler window (≤2s away)
        // so the numbers are a fresh whole window, not a stale one.
        let perf_request = dir.join("perf_request");
        if perf_request.exists() {
            let _ = std::fs::remove_file(&perf_request);
            self.perf_want_file = true;
        }

        let peek_request = dir.join("peek_request");
        if peek_request.exists() && self.peek_run.is_none() {
            let _ = std::fs::remove_file(&peek_request);
            let live = dir.join("live");
            let _ = std::fs::remove_dir_all(&live);
            let _ = std::fs::create_dir_all(&live);
            self.write_agent_file("live/state.txt", &self.state_report());
            let tick = self.world.borrow().tick;
            self.peek_run = Some(PeekRun {
                snaps_left: PEEK_SNAPS,
                next_at_tick: tick,
            });
        }

        let test_request = dir.join("test_request");
        if test_request.exists() && self.test_run.is_none() {
            let request = std::fs::read_to_string(&test_request)
                .ok()
                .and_then(|s| TestRequest::deserialize_json(&s).ok())
                .unwrap_or_default();
            let _ = std::fs::remove_file(&test_request);

            let tape = if request.tape.is_empty() {
                Tape::default()
            } else {
                let path = self
                    .project_dir
                    .as_ref()
                    .map(|p| p.join(&request.tape))
                    .unwrap_or_else(|| PathBuf::from(&request.tape));
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| Tape::deserialize_json(&s).ok())
                    .unwrap_or_default()
            };

            let cap = dir.join("cap");
            let _ = std::fs::remove_dir_all(&cap);
            let _ = std::fs::create_dir_all(&cap);

            // Restart the game so the run is repeatable from spawn state.
            self.reeval_for_test(cx);
            // Deterministic starting camera: canonical pins per mode, set ONCE.
            // From here the angles move only via SCRIPT writes (deterministic
            // under tapes — the mouse is inert during a test), so chase cams
            // are tape-testable instead of being silently pinned every frame
            // (bugs.md BUG 3: set_cam_yaw appeared clobbered because every
            // probe ran inside a test).
            {
                let mut world = self.world.borrow_mut();
                let yaw = if world.cam_third != 0 { 0.0 } else { 0.6 };
                world.orbit_yaw = yaw;
                world.orbit_pitch = -0.35;
                // A drag just before test start must not leak mouse authority
                // into the (mouse-inert) test — chase rigs stay deterministic.
                world.chase_hold = 0.0;
            }
            self.test_run = Some(TestRun {
                frame: 0,
                frames: request.frames.max(1),
                capture_every: request.every.max(1),
                tape,
                probe_lines: Vec::new(),
                captures: 0,
            });
        }
    }

    fn reeval_for_test(&mut self, cx: &mut Cx) {
        // Force a re-eval of the current body: rebuilds entities at spawn.
        let body = std::mem::take(&mut self.body);
        self.set_source(cx, &body);
    }

    fn tick_test_run(&mut self, cx: &mut Cx) -> bool {
        let Some(dir) = self.agent_dir() else {
            self.test_run = None;
            return false;
        };
        let Some(mut run) = self.test_run.take() else {
            return false;
        };

        // Scripted input for this frame.
        {
            let mut world = self.world.borrow_mut();
            for event in &run.tape.events {
                if event.f != run.frame {
                    continue;
                }
                if let Some(action) = &event.press {
                    let action = LiveId::from_str(action);
                    if world.held.insert(action) {
                        world.pressed.insert(action);
                    }
                }
                if let Some(action) = &event.release {
                    let action = LiveId::from_str(action);
                    world.held.remove(&action);
                }
            }
        }

        if run.frame % run.capture_every == 0 {
            cx.capture_next_frame_to_file(dir.join(format!("cap/f{:06}.png", run.frame)));
            run.captures += 1;
        }
        if run.frame % 15 == 0 {
            let world = self.world.borrow();
            for name in &run.tape.probe {
                let found = world.entities.iter().find(|e| &e.tag == name);
                if let Some(e) = found {
                    run.probe_lines.push(format!(
                        "[probe] f={} {} pos=({:.1},{:.1},{:.1}) vel=({:.1},{:.1},{:.1}) floor={}",
                        run.frame, name, e.pos.x, e.pos.y, e.pos.z,
                        e.vel.x, e.vel.y, e.vel.z, e.on_floor
                    ));
                } else {
                    run.probe_lines.push(format!("[probe] f={} {} MISSING", run.frame, name));
                }
            }
        }

        run.frame += 1;
        if run.frame >= run.frames {
            // Release everything the tape held down.
            {
                let mut world = self.world.borrow_mut();
                world.held.clear();
            }
            self.write_agent_file("probe.txt", &(run.probe_lines.join("\n") + "\n"));
            self.write_agent_file(
                "test_done",
                &format!("frames={} captures={}\n", run.frames, run.captures),
            );
            self.test_run = None;
        } else {
            self.test_run = Some(run);
        }
        true
    }

    fn tick_peek_run(&mut self, cx: &mut Cx) {
        let Some(dir) = self.agent_dir() else {
            self.peek_run = None;
            return;
        };
        let Some(mut run) = self.peek_run.take() else {
            return;
        };
        let tick = self.world.borrow().tick;
        if tick >= run.next_at_tick {
            let index = PEEK_SNAPS - run.snaps_left;
            cx.capture_next_frame_to_file(dir.join(format!("live/f{:04}.png", index)));
            run.snaps_left -= 1;
            run.next_at_tick = tick + PEEK_SNAP_GAP_TICKS;
        }
        if run.snaps_left > 0 {
            self.peek_run = Some(run);
        } else {
            // The last PNG lands a frame or two later; `ag` polls for the file.
            self.write_agent_file("live/done", "ok");
        }
    }

    // ── the fixed-step tick ─────────────────────────────────────────────

    /// Poll the most active gamepad into the world's PadState. Stick is
    /// analog (deadzone 0.22), dpad digital; A = jump, X = shoot — the same
    /// bindings AgentEye taught the Godot games. Merged with the keyboard at
    /// read time, never written into `held`.
    #[cfg(not(headless))]
    fn poll_gamepad(&mut self, cx: &mut Cx) {
        let mut best: Option<GamepadState> = None;
        let mut best_score = 0.0f32;
        for state in cx.game_input_states() {
            let GameInputState::Gamepad(pad) = state else {
                continue;
            };
            let score = pad.left_stick.x.abs() as f32
                + pad.left_stick.y.abs() as f32
                + pad.right_stick.x.abs() as f32
                + pad.right_stick.y.abs() as f32
                + pad.dpad_up
                + pad.dpad_down
                + pad.dpad_left
                + pad.dpad_right
                + pad.a
                + pad.x;
            if best.is_none() || score > best_score {
                best_score = score;
                best = Some(pad.clone());
            }
        }
        let (jump, shoot, grab, reset, pad_state) = if let Some(pad) = best {
            const DEADZONE: f64 = 0.22;
            let stick_x = pad.left_stick.x as f64;
            // Stick up = forward = negative axis_z (axis_z is down-minus-up).
            let stick_z = -(pad.left_stick.y as f64);
            let mut axis_x = if stick_x.abs() > DEADZONE { stick_x } else { 0.0 };
            let mut axis_z = if stick_z.abs() > DEADZONE { stick_z } else { 0.0 };
            // Right stick = camera, deadzone rescaled so motion starts at
            // zero. Stick up (+y) maps to look-up (see pad_look docs).
            let dz = |v: f64| {
                if v.abs() > DEADZONE {
                    (v.abs() - DEADZONE) / (1.0 - DEADZONE) * v.signum()
                } else {
                    0.0
                }
            };
            self.pad_look = dvec2(
                dz(pad.right_stick.x as f64),
                dz(pad.right_stick.y as f64),
            );
            axis_x += (pad.dpad_right > 0.5) as i8 as f64 - (pad.dpad_left > 0.5) as i8 as f64;
            axis_z += (pad.dpad_down > 0.5) as i8 as f64 - (pad.dpad_up > 0.5) as i8 as f64;
            let jump = pad.a > 0.5;
            let shoot = pad.x > 0.5;
            let grab = pad.b > 0.5;
            let reset = pad.y > 0.5;
            (
                jump,
                shoot,
                grab,
                reset,
                PadState {
                    axis_x: axis_x.clamp(-1.0, 1.0),
                    axis_z: axis_z.clamp(-1.0, 1.0),
                    jump,
                    jump_pressed: jump && !self.pad_jump_prev,
                    shoot,
                    shoot_pressed: shoot && !self.pad_shoot_prev,
                    grab,
                    grab_pressed: grab && !self.pad_grab_prev,
                    reset,
                    reset_pressed: reset && !self.pad_reset_prev,
                },
            )
        } else {
            self.pad_look = dvec2(0.0, 0.0);
            (false, false, false, false, PadState::default())
        };
        self.pad_jump_prev = jump;
        self.pad_shoot_prev = shoot;
        self.pad_grab_prev = grab;
        self.pad_reset_prev = reset;
        self.world.borrow_mut().pad = pad_state;
    }

    #[cfg(headless)]
    fn poll_gamepad(&mut self, _cx: &mut Cx) {}

    fn perf_physics_channel(&mut self, cx: &mut Cx) -> PerfChannel {
        *self
            .perf_physics_ch
            .get_or_insert_with(|| cx.perf_monitor.channel("physics", 0x58ffd0))
    }

    /// The local player's control intent, in the shape blocks consume. Camera
    /// relative for walking (what the kid means by "left" is screen-left) and
    /// raw axes for steering, matching the input object scripts already see.
    fn block_player_input(&self, world: &GameWorld) -> makepad_game_blocks::DriveInput {
        let held = |name: LiveId| world.action_held(name);
        let axis_x = ((held(live_id!(right)) as i8 - held(live_id!(left)) as i8) as f64
            + world.pad.axis_x)
            .clamp(-1.0, 1.0) as f32;
        let axis_z = ((held(live_id!(down)) as i8 - held(live_id!(up)) as i8) as f64
            + world.pad.axis_z)
            .clamp(-1.0, 1.0) as f32;
        let yaw = world.cam_yaw;
        let (sin_yaw, cos_yaw) = makepad_game_sim::math::sincos(yaw);
        makepad_game_blocks::DriveInput {
            steer: axis_x,
            throttle: -axis_z,
            brake: if held(live_id!(grab)) { 1.0 } else { 0.0 },
            handbrake: if held(live_id!(shoot)) { 1.0 } else { 0.0 },
            move_x: axis_x * cos_yaw - axis_z * sin_yaw,
            move_z: axis_x * sin_yaw + axis_z * cos_yaw,
            jump: held(live_id!(jump)),
            jump_pressed: world.action_pressed(live_id!(jump)),
            pitch: -axis_z,
            roll: axis_x,
        }
    }

    fn run_tick(&mut self, cx: &mut Cx) {
        let tick_t0 = std::time::Instant::now();
        let in_test = self.tick_test_run(cx);
        // ONE cumulative script budget per tick: on_tick, every timer and
        // every touch event share it (each call used to get a fresh 500k).
        self.tick_budget_left = TICK_INSTRUCTION_LIMIT;
        self.budget_exhausted_logged = false;
        // Scripts steer relative to the camera ("run where the camera looks"),
        // so the EFFECTIVE yaw must be visible world state: the orbit yaw, or
        // 0 for the fixed side-on camera (where raw axes are already correct).
        // Tape runs pin it to 0 — repeatability must not depend on where the
        // kid happened to leave the camera.
        // Right stick rotates the camera exactly like a mouse drag, engine-
        // default: same 0.01 rad/px orbit through pseudo-pixels (~2.6 rad/s
        // at full deflection), same look_dx/dy for scripts, same chase-cam
        // authority below. Applied BEFORE script writes, like real mouse
        // events, so a script set_cam_yaw still wins its tick.
        let stick_look = if in_test { dvec2(0.0, 0.0) } else { self.pad_look };
        let stick_active = stick_look.x != 0.0 || stick_look.y != 0.0;
        {
            let mut world = self.world.borrow_mut();
            if stick_active {
                let px = stick_look * (260.0 * TICK_DT as f64);
                world.orbit_yaw -= px.x as f32 * 0.01;
                world.orbit_pitch =
                    (world.orbit_pitch + px.y as f32 * 0.01).clamp(-1.45, 1.45);
                self.look_accum += px;
            }
            // Script camera writes (game.set_cam_yaw/pitch, camera({pitch}))
            // apply to the live rig here — the SAME state the mouse writes,
            // so script and kid share one camera and never fork.
            if let Some(pitch) = world.cam_pitch_request.take() {
                world.orbit_pitch = pitch;
            }
            let mut script_wrote_yaw = false;
            if let Some(yaw) = world.cam_yaw_request.take() {
                world.orbit_yaw = yaw;
                script_wrote_yaw = true;
            }
            // Chase rig: ease the orbit yaw to sit behind the target.
            // Authority, in order: a script set_cam_yaw sticks untouched for
            // the tick it lands; the mouse owns the orbit while dragging and
            // for cam_recenter seconds after (peek-around while driving);
            // otherwise the rig eases with time-constant cam_lag, tightening
            // with target speed. The camera yaw convention is mirror-handed
            // vs entity yaw (cam forward is (sin y, -cos y), entities face
            // atan2(-vx,-vz)), so "behind the target" is -e.yaw, NOT
            // e.yaw + PI — my-game-5's hand-rolled heading+PI never rendered
            // (BUG 3) so the sign error there went unnoticed.
            if world.cam_chase != 0 {
                let dragging =
                    !in_test && (self.orbit_last_abs.is_some() || stick_active);
                if dragging {
                    world.chase_hold = world.cam_recenter;
                } else if world.chase_hold > 0.0 {
                    world.chase_hold -= TICK_DT;
                } else if !script_wrote_yaw {
                    // Copy out of the entity borrow before writing the rig —
                    // same values, same float expression order as before.
                    if let Some((desired, speed)) = world.entity(world.cam_chase).map(|e| {
                        (-e.yaw, (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt())
                    }) {
                        let rate = (1.0 / world.cam_lag.max(0.05))
                            * (1.0 + speed * world.cam_speed_tighten.max(0.0));
                        let mut d = desired - world.orbit_yaw;
                        while d > std::f32::consts::PI {
                            d -= std::f32::consts::TAU;
                        }
                        while d < -std::f32::consts::PI {
                            d += std::f32::consts::TAU;
                        }
                        world.orbit_yaw += d * (rate * TICK_DT).min(1.0);
                    }
                }
            }
            // Beams are immediate-mode: whatever on_tick re-issues below
            // survives to render; everything else vanishes right here.
            world.beams.clear();
            world.cam_yaw = if world.cam_side { 0.0 } else { world.orbit_yaw };
            // Mirror the rest of the camera pose for script reads, and hand
            // over the drag deltas (zeroed under tapes for determinism).
            // Tests no longer pin per-frame: the test START pins the orbit
            // once and the mouse is inert, so script camera writes stick and
            // stay deterministic.
            world.cam_pitch = world.orbit_pitch;
            world.cam_dragging =
                !in_test && (self.orbit_last_abs.is_some() || stick_active);
            let look = std::mem::take(&mut self.look_accum);
            world.look_dx = if in_test { 0.0 } else { look.x };
            world.look_dy = if in_test { 0.0 } else { look.y };
            // Camera shake decays over ~half a second.
            world.cam_shake = (world.cam_shake - world.cam_shake * 3.0 * TICK_DT as f32
                - 0.01 * TICK_DT as f32)
                .max(0.0);
        }
        if in_test {
            // Tapes own the input during a test; a bumped stick must not
            // contaminate a repeatable run.
            self.world.borrow_mut().pad = PadState::default();
        } else {
            self.poll_gamepad(cx);
        }

        // Call the script tick with (dt, input-snapshot) — input as a plain
        // object so the hot path costs no cross-boundary calls.
        let (on_tick, input_snapshot) = {
            let world = self.world.borrow();
            (world.on_tick, self.input_snapshot(&world))
        };
        let on_tick = on_tick.and_then(|slot| self.callbacks.borrow().get(slot));
        if let Some(on_tick) = on_tick {
            let t0 = std::time::Instant::now();
            self.call_script_fn2(cx, on_tick, ScriptValue::from_f64(TICK_DT as f64), input_snapshot);
            let us = perf_us(t0);
            self.perf.script_us += us;
            cx.perf_monitor.add(PERF_CHANNEL_SCRIPT, us);
        }

        // Timers. Repeating ones (game.every) re-arm after firing; a fired
        // one-shot is gone. game.cancel removed its id before we got here.
        let due: Vec<GameTimer> = {
            let mut world = self.world.borrow_mut();
            let now = world.tick;
            let (due, rest): (Vec<_>, Vec<_>) =
                world.timers.drain(..).partition(|t| t.at_tick <= now);
            world.timers = rest;
            for timer in &due {
                if timer.interval_ticks > 0 {
                    let mut again = timer.clone();
                    again.at_tick = now + timer.interval_ticks;
                    world.timers.push(again);
                }
            }
            due
        };
        if !due.is_empty() {
            let t0 = std::time::Instant::now();
            for timer in due {
                let func = self.callbacks.borrow().get(timer.func);
                if let Some(func) = func {
                    self.call_script_fn0(cx, func);
                }
                if timer.interval_ticks == 0 {
                    // A fired one-shot releases its slot (repeats re-armed above).
                    self.callbacks.borrow_mut().free(timer.func);
                }
            }
            let us = perf_us(t0);
            self.perf.script_us += us;
            cx.perf_monitor.add(PERF_CHANNEL_SCRIPT, us);
        }

        // Physics + sensors.
        let t_phys = std::time::Instant::now();
        let touch_events = {
            let mut world = self.world.borrow_mut();
            let mut blocks = self.blocks.borrow_mut();
            // Blocks drive BEFORE the sim (their decisions land this tick) and
            // observe AFTER it (laps/standings see final positions).
            if !blocks.is_empty() {
                blocks.player_input = self.block_player_input(&world);
                blocks.pre_step(&mut world);
            }
            step_world(&mut world);
            world.tick += 1;
            world.time += TICK_DT as f64;
            world.pressed.clear();
            if !blocks.is_empty() {
                blocks.post_step(&mut world);
            }
            collect_touches(&world)
        };
        {
            let us = perf_us(t_phys);
            self.perf.physics_us += us;
            let ch = self.perf_physics_channel(cx);
            cx.perf_monitor.add(ch, us);
        }
        let on_touch = self.world.borrow().on_touch;
        let on_touch = on_touch.and_then(|slot| self.callbacks.borrow().get(slot));
        if let Some(on_touch) = on_touch {
            let t0 = std::time::Instant::now();
            for (a, b) in touch_events {
                let args = self.make_touch_args(cx, a, b);
                if let Some((av, bv)) = args {
                    self.call_script_fn2(cx, on_touch.clone(), av, bv);
                }
            }
            let us = perf_us(t0);
            self.perf.script_us += us;
            cx.perf_monitor.add(PERF_CHANNEL_SCRIPT, us);
        }

        let t_house = std::time::Instant::now();
        // Audio verbs queue; the host owns the synth (the binding layer cannot
        // depend on this example's module).
        self.drain_audio();
        // Agent RPC.
        if self.world.borrow().tick % AGENT_POLL_TICKS == 0 {
            self.poll_agent_requests(cx);
        }
        // Persist game.save data at most once a second.
        if self.world.borrow().tick % 60 == 0 {
            self.flush_save();
        }
        self.tick_peek_run(cx);
        self.flush_log();
        // Disk flush at most once a second (or when a chatty game piles up).
        if self.world.borrow().tick % 60 == 0 || self.log_buf.len() > 16384 {
            self.flush_log_to_disk();
        }
        self.perf.house_us += perf_us(t_house);
        self.perf.ticks += 1;
        self.perf.worst_tick_us = self.perf.worst_tick_us.max(perf_us(tick_t0));
        let _ = in_test;
    }

    /// Play what the verb table queued. Sustained tones carry script-side
    /// handles, so the mapping to real synth voices lives here.
    fn drain_audio(&mut self) {
        // Drop queued particle requests: no particle renderer here, and an
        // undrained queue would grow for the life of the game.
        self.particles.borrow_mut().clear();
        let requests = std::mem::take(&mut *self.audio.borrow_mut());
        for request in requests {
            match request {
                AudioRequest::Sfx { name, pitch } => {
                    if !crate::synth::play_named(&name, pitch) {
                        // An unknown name is a script bug the agent should hear.
                        self.world
                            .borrow_mut()
                            .log(format!("sfx: unknown sound \"{name}\""));
                    }
                }
                // Positional one-shot: gamemaker's synth is 2D, so the
                // listener distance becomes a gain scale (audio3d owns the
                // math so every host attenuates the same way).
                AudioRequest::SfxAt { name, pitch, at, range } => {
                    let (pos, yaw) = {
                        let w = self.world.borrow();
                        (w.cam_target, w.cam_yaw)
                    };
                    let listener = makepad_game_script::audio3d::Listener::from_yaw(pos, yaw);
                    let placed = makepad_game_script::audio3d::place(&listener, at, range);
                    if placed.gain > 0.01 && !crate::synth::play_named(&name, pitch) {
                        self.world
                            .borrow_mut()
                            .log(format!("sfx_at: unknown sound \"{name}\""));
                    }
                }
                AudioRequest::Beep { freq, to, ms, wave, gain } => {
                    crate::synth::beep(freq, to, ms / 1000.0, synth_wave(wave), gain, 0.0);
                }
                AudioRequest::Jingle { notes, ms } => {
                    crate::synth::jingle(
                        &notes,
                        ms / 1000.0,
                        crate::synth::Wave::Triangle,
                        0.22,
                    );
                }
                AudioRequest::Tone { id, freq, wave, gain } => {
                    let voice = crate::synth::tone(freq, synth_wave(wave), gain);
                    self.tone_voices.insert(id, voice);
                }
                AudioRequest::ToneSet { id, freq, gain } => {
                    if let Some(voice) = self.tone_voices.get(&id) {
                        crate::synth::tone_set(*voice, freq, gain);
                    }
                }
                AudioRequest::ToneStop { id } => {
                    if let Some(voice) = self.tone_voices.remove(&id) {
                        crate::synth::tone_stop(voice);
                    }
                }
                AudioRequest::StopAllTones => {
                    crate::synth::stop_all_tones();
                    self.tone_voices.clear();
                }
            }
        }
    }

    fn input_snapshot(&self, world: &GameWorld) -> ScriptValue {
        // Built fresh per tick inside the isolate.
        let _ = world;
        NIL // replaced in call_script_fn2 via build_input_object
    }

    fn build_input_object(vm: &mut ScriptVm, world: &GameWorld) -> ScriptValue {
        let obj = vm.bx.heap.new_object();
        vm.bx.heap.set_object_storage_auto(obj);
        let trap = NoTrap;
        let key = |world: &GameWorld, name: LiveId| world.held.contains(&name);
        // Keyboard digital + gamepad analog, clamped — either device just works.
        let axis = ((key(world, live_id!(right)) as i8 - key(world, live_id!(left)) as i8) as f64
            + world.pad.axis_x)
            .clamp(-1.0, 1.0);
        let axis_z = ((key(world, live_id!(down)) as i8 - key(world, live_id!(up)) as i8) as f64
            + world.pad.axis_z)
            .clamp(-1.0, 1.0);
        let heap = &mut vm.bx.heap;
        heap.set_value(obj, id!(left).into(), ScriptValue::from_bool(world.action_held(live_id!(left))), trap);
        heap.set_value(obj, id!(right).into(), ScriptValue::from_bool(world.action_held(live_id!(right))), trap);
        heap.set_value(obj, id!(up).into(), ScriptValue::from_bool(world.action_held(live_id!(up))), trap);
        heap.set_value(obj, id!(down).into(), ScriptValue::from_bool(world.action_held(live_id!(down))), trap);
        heap.set_value(obj, id!(jump).into(), ScriptValue::from_bool(world.action_held(live_id!(jump))), trap);
        heap.set_value(obj, id!(jump_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(jump))), trap);
        heap.set_value(obj, id!(shoot).into(), ScriptValue::from_bool(world.action_held(live_id!(shoot))), trap);
        heap.set_value(obj, id!(shoot_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(shoot))), trap);
        heap.set_value(obj, id!(grab).into(), ScriptValue::from_bool(world.action_held(live_id!(grab))), trap);
        heap.set_value(obj, id!(grab_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(grab))), trap);
        heap.set_value(obj, id!(reset).into(), ScriptValue::from_bool(world.action_held(live_id!(reset))), trap);
        heap.set_value(obj, id!(reset_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(reset))), trap);
        heap.set_value(obj, id!(back).into(), ScriptValue::from_bool(world.action_held(live_id!(back))), trap);
        heap.set_value(obj, id!(back_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(back))), trap);
        // Mouse-drag deltas this tick (0 while not orbiting / under tapes):
        // chase cams use these to yield to the kid's hand.
        heap.set_value(obj, id!(look_dx).into(), ScriptValue::from_f64(world.look_dx), trap);
        heap.set_value(obj, id!(look_dy).into(), ScriptValue::from_f64(world.look_dy), trap);
        heap.set_value(obj, id!(axis_x).into(), ScriptValue::from_f64(axis), trap);
        heap.set_value(obj, id!(axis_z).into(), ScriptValue::from_f64(axis_z), trap);
        // Camera-relative movement: what the kid MEANS by "left" is screen-left.
        // Camera basis on the ground plane: forward = (sin y, -cos y), right =
        // (cos y, sin y) — so rotate the raw axes by +yaw. Scripts should walk
        // with these; the raw axes stay for side-scrollers and custom schemes.
        let yaw = world.cam_yaw;
        let move_x = axis * yaw.cos() as f64 - axis_z * yaw.sin() as f64;
        let move_z = axis * yaw.sin() as f64 + axis_z * yaw.cos() as f64;
        heap.set_value(obj, id!(move_x).into(), ScriptValue::from_f64(move_x), trap);
        heap.set_value(obj, id!(move_z).into(), ScriptValue::from_f64(move_z), trap);
        obj.into()
    }

    fn make_touch_args(&self, _cx: &mut Cx, a: u64, b: u64) -> Option<(ScriptValue, ScriptValue)> {
        Some((ScriptValue::from_f64(a as f64), ScriptValue::from_f64(b as f64)))
    }

    fn call_script_fn0(&mut self, cx: &mut Cx, func: ScriptObjectRef) {
        self.call_script(cx, func, &[]);
    }

    fn call_script_fn2(
        &mut self,
        cx: &mut Cx,
        func: ScriptObjectRef,
        a: ScriptValue,
        b: ScriptValue,
    ) {
        self.call_script(cx, func, &[a, b]);
    }

    fn call_script(&mut self, cx: &mut Cx, func: ScriptObjectRef, args: &[ScriptValue]) {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            return;
        }
        if self.tick_budget_left == 0 {
            if !self.budget_exhausted_logged {
                self.budget_exhausted_logged = true;
                self.append_log("tick script budget exhausted — remaining callbacks skipped this tick");
            }
            return;
        }
        let budget = self.tick_budget_left;
        let world = self.world.clone();
        let vm_id = self.vm_id;
        let (errors, consumed) = cx.with_script_vm_id(vm_id, |vm| {
            let args_obj = vm.bx.heap.new_object();
            vm.bx.heap.set_object_storage_vec2(args_obj);
            vm.bx.heap.clear_object_deep(args_obj);
            // Host transients: unchecked pushes keep args/input releasable after
            // the call (a checked store would tag them escaped). If the script
            // retains either, release_transient no-ops and GC owns them.
            let mut input_val = None;
            for value in args {
                // NIL positional slots become the fresh input snapshot.
                let value = if value.is_nil() {
                    let input = Self::build_input_object(vm, &world.borrow());
                    input_val = Some(input);
                    input
                } else {
                    *value
                };
                vm.bx.heap.vec_push_unchecked(args_obj, NIL, value);
            }
            vm.bx.captured_errors = Some(Vec::new());
            let _ = vm.with_instruction_limit(budget, |vm| {
                vm.call_with_args_object_with_me(func.as_object().into(), args_obj, NIL)
            });
            let consumed = vm.last_limit_consumed();
            vm.release_transient(args_obj.into());
            if let Some(input) = input_val {
                vm.release_transient(input);
            }
            (vm.take_errors(), consumed)
        });
        self.tick_budget_left = self.tick_budget_left.saturating_sub(consumed);
        if !errors.is_empty() {
            let joined = errors.join("\n");
            self.append_log(&format!("script error:\n{joined}"));
            self.flush_log_to_disk();
            self.write_agent_file("last_error.txt", &format!("runtime\n{joined}\n"));
            // Push to the app, which decides whether to wake the agent — a
            // runtime error the kid just hit is invisible to the AI otherwise.
            cx.widget_action(
                self.uid,
                GameViewAction::RuntimeError {
                    generation: self.eval_generation,
                    error: joined,
                },
            );
        }
    }

    fn flush_log(&mut self) {
        let pending: Vec<String> = std::mem::take(&mut self.world.borrow_mut().log_pending);
        for line in pending {
            self.append_log(&line);
        }
    }

    // ── camera / render ─────────────────────────────────────────────────

    fn scene_state(&self, rect: Rect, time: f64) -> Option<SceneState3D> {
        let world = self.world.borrow();
        render_scene_state(
            &world,
            rect,
            time,
            &CameraRig {
                yaw: world.orbit_yaw,
                pitch: world.orbit_pitch,
                in_test: self.test_run.is_some(),
            },
        )
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        let t0 = std::time::Instant::now();
        let world = self.world.clone();
        let world = world.borrow();
        let mut draws = GameDraws {
            cube: &mut self.draw_cube,
            alpha: &mut self.draw_alpha,
            sky: &mut self.draw_sky,
            terrain: &mut self.draw_terrain,
            // Legacy blob tier: gamemaker is the tape/parity oracle, so it
            // keeps the shadows it has always drawn. Arcade opts in.
            shadow: None,
        };
        let stats = self
            .renderer
            .draw_scene(cx, &mut self.draw_list, &mut draws, &world, scene_state);
        drop(world);
        self.perf.slab_us += stats.slab_us;
        self.perf.slab_rebuilds += stats.slab_rebuilds;
        self.perf.static_instances = stats.static_instances;
        self.perf.dyn_instances = stats.dyn_instances;
        let us = perf_us(t0);
        self.perf.scene_us += us;
        self.perf.worst_scene_us = self.perf.worst_scene_us.max(us);
        self.perf.draws += 1;
        if self.perf.draws >= 120 {
            self.finish_perf_window();
        }
    }

    /// Close the ~2s profiler window: format the report, hand it to whoever
    /// asked (F3 overlay, `ag perf`, AIGAME_PERF=1 stderr), start the next.
    fn finish_perf_window(&mut self) {
        let w = std::mem::take(&mut self.perf);
        let draws = w.draws.max(1);
        let ticks = w.ticks.max(1);
        let fps = if w.gaps > 0 && w.gap_ms_sum > 0.0 {
            1000.0 / (w.gap_ms_sum / w.gaps as f64)
        } else {
            0.0
        };
        let (entities, parts, labels, timers) = {
            let world = self.world.borrow();
            (world.entities.len(), world.parts.len(), world.labels.len(), world.timers.len())
        };
        self.perf_report = format!(
            "fps {:.1}  worst gap {:.1}ms\n\
             tick: script {}us  physics {}us  house {}us  worst {}us\n\
             scene: {}us avg  {}us worst  slab {} rebuilds {}us\n\
             overlay: {}us   inst {} static + {} dyn\n\
             entities {}  parts {}  labels {}  timers {}",
            fps,
            w.worst_gap_ms,
            w.script_us / ticks,
            w.physics_us / ticks,
            w.house_us / ticks,
            w.worst_tick_us,
            w.scene_us / draws,
            w.worst_scene_us,
            w.slab_rebuilds,
            w.slab_us,
            w.overlay_us / draws,
            w.static_instances,
            w.dyn_instances,
            entities,
            parts,
            labels,
            timers,
        );
        if self.perf_log {
            eprintln!("[aigame-perf] {}", self.perf_report.replace('\n', " | "));
        }
        if self.perf_want_file {
            self.perf_want_file = false;
            self.write_agent_file("perf.txt", &self.perf_report.clone());
        }
    }

}

// The `game.*` verb table lives in makepad-game-script now: this file used to
// carry its own 84-arm dispatch chain plus the arg/option helpers, the spawn
// builders and the GAME_API table, all duplicated. register_game_handle binds
// the shared table instead, so gamemaker and Arcade cannot drift apart.

// ── widget plumbing ─────────────────────────────────────────────────────

/// Eval/runtime status pushed to the app so it can wake the agent — errors
/// must reach the AI that edits the game, not just wait in `.agent/` for a
/// poll. Carries every error class the isolate produces (parse, runtime,
/// pod, shader-compiler) with file:line text intact.
#[derive(Clone, Debug, Default)]
pub enum GameViewAction {
    EvalOk {
        #[allow(dead_code)]
        generation: u64,
    },
    EvalFailed {
        #[allow(dead_code)]
        generation: u64,
        #[allow(dead_code)]
        error: String,
    },
    RuntimeError {
        generation: u64,
        error: String,
    },
    #[default]
    None,
}

impl WidgetNode for GameView {
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

impl Widget for GameView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            if time > last {
                let gap_ms = (time - last) * 1000.0;
                self.perf.gap_ms_sum += gap_ms;
                self.perf.worst_gap_ms = self.perf.worst_gap_ms.max(gap_ms);
                self.perf.gaps += 1;
            }
            self.time_accum += (time - last).min(0.25);
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                self.run_tick(cx);
                ticked = true;
            }
            if ticked {
                self.area.redraw(cx);
            }
            self.next_frame = cx.new_next_frame();
        }

        // F4 toggles the engine's text overlay (phase averages + world
        // counts); F3 is the app-level PerfGraph widget.
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::F4 && !ke.is_repeat {
                self.perf_overlay = !self.perf_overlay;
                self.area.redraw(cx);
            }
        }

        // Keyboard -> named actions. Test runs feed the tape instead.
        if self.test_run.is_none() {
            match event {
                Event::KeyDown(ke) if !ke.is_repeat => {
                    if let Some(action) = key_to_action(ke.key_code) {
                        let mut world = self.world.borrow_mut();
                        if world.held.insert(action) {
                            world.pressed.insert(action);
                        }
                    }
                }
                Event::KeyUp(ke) => {
                    if let Some(action) = key_to_action(ke.key_code) {
                        self.world.borrow_mut().held.remove(&action);
                    }
                }
                _ => {}
            }
        }

        // Mouse orbit + wheel zoom on the pane. Raw mouse events with a rect
        // check, NOT event.hits(): the composited-pass quad doesn't take part
        // in finger capture the way plain widgets do, and this is the exact
        // pattern XrCamera's desktop orbit uses.
        match event {
            // Mouse orbit is inert during a tape test — determinism now relies
            // on this (test start pins the orbit once; only script writes may
            // move it afterwards).
            Event::MouseDown(me)
                if self.test_run.is_none()
                    && self.view_rect.contains(me.abs)
                    && me.button.is_primary() =>
            {
                self.orbit_last_abs = Some(me.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Event::MouseMove(me) => {
                if let Some(last) = self.orbit_last_abs {
                    let delta = me.abs - last;
                    {
                        let mut world = self.world.borrow_mut();
                        world.orbit_yaw -= delta.x as f32 * 0.01;
                        world.orbit_pitch =
                            (world.orbit_pitch + delta.y as f32 * 0.01).clamp(-1.45, 1.45);
                    }
                    // Scripts see this as input.look_dx/look_dy next tick.
                    self.look_accum += delta;
                    self.orbit_last_abs = Some(me.abs);
                    self.area.redraw(cx);
                } else if self.view_rect.contains(me.abs) {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Event::MouseUp(me) if me.button.is_primary() => {
                self.orbit_last_abs = None;
            }
            Event::Scroll(se) if self.test_run.is_none() && self.view_rect.contains(se.abs) => {
                let scroll_axis = if se.scroll.y.abs() > f64::EPSILON {
                    se.scroll.y
                } else {
                    se.scroll.x
                };
                if scroll_axis.abs() > f64::EPSILON {
                    let factor = if scroll_axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
                    let mut world = self.world.borrow_mut();
                    if world.cam_third != 0 {
                        // Third-person: the wheel zooms the boom in and out.
                        world.cam_boom = (world.cam_boom * factor as f32).clamp(2.0, 60.0);
                    } else {
                        world.cam_distance = (world.cam_distance * factor).clamp(2.0, 120.0);
                    }
                    drop(world);
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        self.view_rect = rect;
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        if let Some(scene_state) = self.scene_state(rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_scene(cx3d, scene_state);
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);

        let t_overlay = std::time::Instant::now();
        // HUD + gauges + crosshair (moved to makepad-game-render::hud).
        {
            let (slots, bars, crosshair) = {
                let world = self.world.borrow();
                (world.hud_slots.clone(), world.hud_bars.clone(), world.crosshair)
            };
            draw_hud_overlay(
                cx,
                rect,
                &mut self.draw_hud,
                &mut self.draw_dot,
                &slots,
                &bars,
                crosshair,
            );
        }

        // Billboard nametags: project each labeled entity into the pane and
        // draw in the 2D overlay — always camera-facing and never hidden by
        // geometry, like the Godot Label3D (billboard + no_depth_test).
        let labels: Vec<(Vec3f, String, Vec4f, f32)> = {
            let world = self.world.borrow();
            world
                .labels
                .iter()
                .filter_map(|label| {
                    world.entity(label.owner).map(|e| {
                        let height = if label.height.is_nan() {
                            e.half.y + 0.7
                        } else {
                            label.height
                        };
                        (
                            e.pos + vec3f(0.0, height, 0.0),
                            label.text.clone(),
                            label.color,
                            label.size,
                        )
                    })
                })
                .collect()
        };
        if !labels.is_empty() {
            if let Some(scene) = self.scene_state(rect, cx.time()) {
                draw_billboard_labels(cx, rect, &scene, &mut self.draw_label, &labels);
            }
        }
        self.perf.overlay_us += perf_us(t_overlay);

        // F3 profiler overlay: the last completed ~2s window, top-right.
        if self.perf_overlay {
            let report = if self.perf_report.is_empty() {
                "perf: collecting...".to_string()
            } else {
                self.perf_report.clone()
            };
            let lines: Vec<&str> = report.lines().collect();
            let pad = 8.0f64;
            let line_h = 13.0f64;
            let w = 350.0f64;
            let h = lines.len() as f64 * line_h + pad * 2.0;
            let x = rect.pos.x + rect.size.x - w - 10.0;
            let y = rect.pos.y + 10.0;
            self.draw_dot.color = vec4(0.04, 0.05, 0.09, 0.78);
            self.draw_dot.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, y),
                    size: dvec2(w, h),
                },
            );
            self.draw_dot.color = vec4(1.0, 1.0, 1.0, 0.9);
            self.draw_hud.text_style.font_size = 8.0;
            self.draw_hud.color = vec4(0.65, 1.0, 0.75, 0.95);
            for (i, line) in lines.iter().enumerate() {
                self.draw_hud
                    .draw_abs(cx, dvec2(x + pad, y + pad + i as f64 * line_h), line);
            }
            self.draw_hud.text_style.font_size = 22.0;
            self.draw_hud.color = vec4(1.0, 1.0, 1.0, 0.93);
        }
        DrawStep::done()
    }
}

fn key_to_action(key_code: KeyCode) -> Option<LiveId> {
    match key_code {
        KeyCode::ArrowLeft | KeyCode::KeyA => Some(live_id!(left)),
        KeyCode::ArrowRight | KeyCode::KeyD => Some(live_id!(right)),
        KeyCode::ArrowUp | KeyCode::KeyW => Some(live_id!(up)),
        KeyCode::ArrowDown | KeyCode::KeyS => Some(live_id!(down)),
        KeyCode::Space | KeyCode::ReturnKey => Some(live_id!(jump)),
        KeyCode::KeyF => Some(live_id!(shoot)),
        KeyCode::KeyG => Some(live_id!(grab)),
        KeyCode::KeyR => Some(live_id!(reset)),
        KeyCode::KeyC => Some(live_id!(back)),
        _ => None,
    }
}
