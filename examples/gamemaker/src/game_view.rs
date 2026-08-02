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
use makepad_widgets::makepad_script::numeric::NumericValue;
use makepad_widgets::widget_async::{CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID};
use makepad_widgets::*;

// The simulation core lives in makepad-game-sim now (M0 stage A extraction,
// game.md): entity/terrain/world data, the fixed-step physics and the spatial
// queries moved there verbatim. This file keeps the host: script isolate,
// verb dispatch, rendering, input devices, agent RPC.
use makepad_game_blocks::{Blocks, BrainKind, CarConfig, CharacterConfig, ControlSource, PlaneConfig};
use makepad_game_sim::{
    collect_touches, step_world, world_raycast, Beam, BodyKind, CallbackSlot, Entity, GameTimer,
    GameWorld, HudAnchor, HudBar, HudSlot, LabelDef, PadState, Part, SaveVal, Shape, SkyConfig,
    Terrain, TERRAIN_ID, TICK_DT,
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



/// Host-side slot table mapping the sim's opaque `CallbackSlot`s to script
/// closures (the sim never holds a ScriptObjectRef — game.md). Entries are
/// tagged with the eval generation that allocated them: a successful eval
/// frees every earlier generation (that world is gone), a failed eval frees
/// exactly the generation it created (the world rolled back to the snapshot,
/// whose slots are older). Point frees happen at replace/cancel/one-shot
/// sites, where the slot index is known-live.
#[derive(Default)]
struct CallbackTable {
    entries: Vec<Option<(u64, ScriptObjectRef)>>,
    free: Vec<u32>,
}

impl CallbackTable {
    fn alloc(&mut self, generation: u64, func: ScriptObjectRef) -> CallbackSlot {
        if let Some(index) = self.free.pop() {
            self.entries[index as usize] = Some((generation, func));
            CallbackSlot(index)
        } else {
            self.entries.push(Some((generation, func)));
            CallbackSlot(self.entries.len() as u32 - 1)
        }
    }

    fn get(&self, slot: CallbackSlot) -> Option<ScriptObjectRef> {
        self.entries
            .get(slot.0 as usize)?
            .as_ref()
            .map(|(_, func)| func.clone())
    }

    fn free(&mut self, slot: CallbackSlot) {
        if let Some(entry) = self.entries.get_mut(slot.0 as usize) {
            if entry.take().is_some() {
                self.free.push(slot.0);
            }
        }
    }

    fn free_generation(&mut self, generation: u64) {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.as_ref().is_some_and(|(g, _)| *g == generation) {
                *entry = None;
                self.free.push(index as u32);
            }
        }
    }

    fn free_generations_before(&mut self, generation: u64) {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.as_ref().is_some_and(|(g, _)| *g < generation) {
                *entry = None;
                self.free.push(index as u32);
            }
        }
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
        let callbacks = self.callbacks.clone();
        let blocks = self.blocks.clone();
        let eval_gen = self.eval_gen_cell.clone();
        let vm_id = self.vm_id;
        cx.with_script_vm_id(vm_id, |vm| {
            let game_type = vm.new_handle_type(id_lut!(game));
            let dispatch_world = world.clone();
            vm.set_handle_call(game_type, move |vm, args, method| {
                game_dispatch(
                    vm,
                    &dispatch_world,
                    &callbacks,
                    &blocks,
                    &eval_gen,
                    args,
                    method,
                )
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

// ── the script API dispatcher ───────────────────────────────────────────
//
// Every `game.<method>(...)` in a game script lands here, synchronously.
// The vocabulary is deliberately small and grows toward GDScript-like power
// method by method — add a match arm, document it in aigame-dsl.md, done.

fn arg(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    let trap = vm.bx.threads.cur().trap.pass();
    vm.bx.heap.vec_value(args, index, trap)
}

/// Optional positional argument: absent → NIL, and — unlike `arg` — probing
/// past the end records no error (a live trap would fail the whole eval).
fn arg_opt(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    let v = vm.bx.heap.vec_value(args, index, NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn arg_f32(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> f32 {
    let v = arg(vm, args, index);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    vm.bx.heap.cast_to_f64(v, ip) as f32
}

fn arg_id(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> u64 {
    arg_f32(vm, args, index) as u64
}

fn arg_string(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> String {
    let v = arg(vm, args, index);
    vm.bx.heap.temp_string_with(|heap, out| {
        heap.cast_to_string(v, out);
        out.to_string()
    })
}

fn value_vec3(vm: &mut ScriptVm, v: ScriptValue) -> Vec3f {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
        NumericValue::Vec3(v) => v,
        NumericValue::F64(f) => vec3f(f as f32, f as f32, f as f32),
        _ => vec3f(0.0, 0.0, 0.0),
    }
}

fn value_color(vm: &mut ScriptVm, v: ScriptValue) -> Vec4f {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
        NumericValue::Color(c) => c,
        NumericValue::Vec4(c) => c,
        _ => vec4(0.8, 0.8, 0.8, 1.0),
    }
}

fn vec3_value(vm: &mut ScriptVm, v: Vec3f) -> ScriptValue {
    NumericValue::Vec3(v).to_script_value_heap(&mut vm.bx.heap, &vm.bx.code)
}

/// Missing option keys come back as error values, not NIL — normalize both
/// to NIL so `is_nil()` means "not provided" (a raw error would NaN every
/// numeric cast downstream).
fn opts_value(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> ScriptValue {
    let v = vm.bx.heap.value(opts, key.into(), NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn fn_ref(vm: &mut ScriptVm, v: ScriptValue) -> Option<ScriptObjectRef> {
    let obj = v.as_object()?;
    Some(vm.bx.heap.new_object_ref(obj))
}

/// Typo guard: a misspelled option (`pich: -0.2`) used to be silently ignored
/// and cost the agent whole test cycles (features.md §2). Every options-taking
/// verb enumerates its object's keys against an allow-list and logs strays.
fn warn_unknown_keys(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    verb: &str,
    opts: ScriptObject,
    allowed: &[LiveId],
) {
    let len = vm.bx.heap.iter_len(opts);
    for index in 0..len {
        let kv = vm.bx.heap.iter_key_value(opts, index, NoTrap);
        let Some(key) = kv.key.as_id() else {
            continue; // positional/vec entry, not a named option
        };
        if !allowed.contains(&key) {
            world
                .borrow_mut()
                .log(format!("game.{verb}: unknown option `{key}` (ignored)"));
        }
    }
}

/// Script `[...]` literals are ScriptArrays; some paths hand us vec-objects.
/// Accept either — a heights list must never silently fall back to noise.
fn list_len(vm: &ScriptVm, v: ScriptValue) -> usize {
    if let Some(a) = v.as_array() {
        vm.bx.heap.array_len(a)
    } else if let Some(o) = v.as_object() {
        vm.bx.heap.vec_len(o)
    } else {
        0
    }
}

fn list_value(vm: &mut ScriptVm, v: ScriptValue, index: usize) -> ScriptValue {
    if let Some(a) = v.as_array() {
        vm.bx.heap.array_index(a, index, NoTrap)
    } else if let Some(o) = v.as_object() {
        vm.bx.heap.vec_value(o, index, NoTrap)
    } else {
        NIL
    }
}


// ── block spawn helpers (libs/game/blocks) ──────────────────────────────

fn opts_f32(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: f32) -> f32 {
    opts_value(vm, opts, key)
        .as_f64()
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn opts_string(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> String {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        return String::new();
    }
    vm.bx.heap.temp_string_with(|heap, out| {
        heap.cast_to_string(v, out);
        out.to_string()
    })
}

fn opts_vec3(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> Option<Vec3f> {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        return None;
    }
    Some(value_vec3(vm, v))
}

/// Read a `[vec3, vec3, ...]` option into a route.
fn read_point_list(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> Vec<Vec3f> {
    let value = opts_value(vm, opts, key);
    let mut points = Vec::new();
    if let Some(array) = value.as_array() {
        let len = vm.bx.heap.array_len(array);
        for i in 0..len {
            let item = vm.bx.heap.array_index_unchecked(array, i);
            if !item.is_nil() {
                points.push(value_vec3(vm, item));
            }
        }
    }
    points
}

fn set_brain(blocks: &Rc<RefCell<Blocks>>, entity: u64, kind: BrainKind) -> ScriptValue {
    let mut blocks = blocks.borrow_mut();
    // Re-issuing a brain on the same entity replaces it, so a hot-reload
    // can't stack two behaviours on one actor.
    blocks.brains.retain(|b| b.entity != entity);
    blocks
        .brains
        .push(makepad_game_blocks::Brain::new(entity, kind));
    ScriptValue::from_f64(entity as f64)
}

/// Shared body for game.car / game.plane: spawn the rigid chassis, then the
/// block that drives it.
fn spawn_block_body(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    opts: ScriptObject,
    default_size: Vec3f,
    tag: &str,
) -> u64 {
    let pos = opts_vec3(vm, opts, id!(pos)).unwrap_or(vec3f(0.0, 2.0, 0.0));
    let size = opts_vec3(vm, opts, id!(size)).unwrap_or(default_size);
    let color = {
        let v = opts_value(vm, opts, id!(color));
        if v.is_nil() {
            vec4(0.85, 0.35, 0.3, 1.0)
        } else {
            value_color(vm, v)
        }
    };
    let tag_opt = {
        let s = opts_string(vm, opts, id!(tag));
        if s.is_empty() {
            tag.to_string()
        } else {
            s
        }
    };
    let mut world = world.borrow_mut();
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Rigid,
        pos,
        half: size * 0.5,
        color,
        tag: tag_opt,
        collide: true,
        gravity_scale: 1.0,
        speed_mult: 1.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        turn_rate: 6.0,
        density: 1.0,
        friction: 0.7,
        restitution: 0.0,
        ..Default::default()
    });
    world.mark_render_dirty();
    id
}

fn spawn_car(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    blocks: &Rc<RefCell<Blocks>>,
    args: ScriptObject,
) -> ScriptValue {
    let opts_val = arg(vm, args, 0);
    let Some(opts) = opts_val.as_object() else {
        return NIL;
    };
    warn_unknown_keys(
        vm,
        world,
        "car",
        opts,
        &[
            id!(pos),
            id!(size),
            id!(color),
            id!(tag),
            id!(player),
            id!(top_speed),
            id!(accel),
            id!(braking),
            id!(grip),
            id!(steer_rate),
            id!(seats),
        ],
    );
    let mut config = CarConfig::default();
    config.top_speed = opts_f32(vm, opts, id!(top_speed), config.top_speed);
    config.accel = opts_f32(vm, opts, id!(accel), config.accel);
    config.braking = opts_f32(vm, opts, id!(braking), config.braking);
    config.grip = opts_f32(vm, opts, id!(grip), config.grip);
    config.steer_rate = opts_f32(vm, opts, id!(steer_rate), config.steer_rate);
    config.seats = opts_f32(vm, opts, id!(seats), 1.0) as u32;
    let player = opts_value(vm, opts, id!(player)).as_bool().unwrap_or(false);
    let id = spawn_block_body(vm, world, opts, vec3f(1.8, 0.8, 3.2), "car");
    let control = if player {
        ControlSource::Player
    } else {
        ControlSource::Script
    };
    blocks
        .borrow_mut()
        .cars
        .push(makepad_game_blocks::Car::new(id, config, control));
    ScriptValue::from_f64(id as f64)
}

fn spawn_plane(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    blocks: &Rc<RefCell<Blocks>>,
    args: ScriptObject,
) -> ScriptValue {
    let opts_val = arg(vm, args, 0);
    let Some(opts) = opts_val.as_object() else {
        return NIL;
    };
    warn_unknown_keys(
        vm,
        world,
        "plane",
        opts,
        &[
            id!(pos),
            id!(size),
            id!(color),
            id!(tag),
            id!(player),
            id!(thrust),
            id!(top_speed),
            id!(lift_speed),
            id!(auto_level),
        ],
    );
    let mut config = PlaneConfig::default();
    config.thrust = opts_f32(vm, opts, id!(thrust), config.thrust);
    config.top_speed = opts_f32(vm, opts, id!(top_speed), config.top_speed);
    config.lift_speed = opts_f32(vm, opts, id!(lift_speed), config.lift_speed);
    config.auto_level = opts_f32(vm, opts, id!(auto_level), config.auto_level);
    let player = opts_value(vm, opts, id!(player)).as_bool().unwrap_or(false);
    let id = spawn_block_body(vm, world, opts, vec3f(2.4, 0.8, 3.2), "plane");
    let control = if player {
        ControlSource::Player
    } else {
        ControlSource::Script
    };
    blocks
        .borrow_mut()
        .planes
        .push(makepad_game_blocks::Plane::new(id, config, control));
    ScriptValue::from_f64(id as f64)
}

fn spawn_character(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    blocks: &Rc<RefCell<Blocks>>,
    args: ScriptObject,
) -> ScriptValue {
    let opts_val = arg(vm, args, 0);
    let Some(opts) = opts_val.as_object() else {
        return NIL;
    };
    warn_unknown_keys(
        vm,
        world,
        "character",
        opts,
        &[
            id!(pos),
            id!(size),
            id!(color),
            id!(tag),
            id!(player),
            id!(model),
            id!(speed),
            id!(jump),
            id!(view),
        ],
    );
    let mut config = CharacterConfig::default();
    config.speed = opts_f32(vm, opts, id!(speed), config.speed);
    config.jump = opts_f32(vm, opts, id!(jump), config.jump);
    if opts_string(vm, opts, id!(view)) == "first" {
        config.view = makepad_game_blocks::character::ViewMode::First;
    }
    let model = {
        let s = opts_string(vm, opts, id!(model));
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    let player = opts_value(vm, opts, id!(player)).as_bool().unwrap_or(false);
    let pos = opts_vec3(vm, opts, id!(pos)).unwrap_or(vec3f(0.0, 2.0, 0.0));
    let size = opts_vec3(vm, opts, id!(size)).unwrap_or(vec3f(0.8, 1.6, 0.8));
    let color = {
        let v = opts_value(vm, opts, id!(color));
        if v.is_nil() {
            vec4(0.29, 0.5, 0.84, 1.0)
        } else {
            value_color(vm, v)
        }
    };
    let tag = {
        let s = opts_string(vm, opts, id!(tag));
        if s.is_empty() {
            "player".to_string()
        } else {
            s
        }
    };
    let id = {
        let mut world = world.borrow_mut();
        world.next_id += 1;
        let id = world.next_id;
        world.push_entity(Entity {
            id,
            kind: BodyKind::Mover,
            pos,
            half: size * 0.5,
            color,
            tag,
            collide: true,
            gravity_scale: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            auto_face: true,
            turn_rate: config.turn_rate,
            density: 1.0,
            ..Default::default()
        });
        world.mark_render_dirty();
        id
    };
    let control = if player {
        ControlSource::Player
    } else {
        ControlSource::Script
    };
    blocks
        .borrow_mut()
        .characters
        .push(makepad_game_blocks::Character::new(
            id, config, control, model,
        ));
    ScriptValue::from_f64(id as f64)
}

fn spawn_entity(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    args: ScriptObject,
    kind: BodyKind,
) -> ScriptValue {
    let opts_val = arg(vm, args, 0);
    let Some(opts) = opts_val.as_object() else {
        return NIL;
    };
    warn_unknown_keys(
        vm,
        world,
        "box/mover/spawn",
        opts,
        &[
            id!(pos),
            id!(size),
            id!(color),
            id!(tag),
            id!(sensor),
            id!(collide),
            id!(body),
            id!(gravity),
            id!(vel),
            id!(life),
            id!(hits),
            id!(glow),
            id!(face),
            id!(rot_y),
            id!(turn_rate),
            id!(shape),
            id!(density),
            id!(friction),
            id!(restitution),
        ],
    );
    let pos_v = opts_value(vm, opts, id!(pos));
    let size_v = opts_value(vm, opts, id!(size));
    let color_v = opts_value(vm, opts, id!(color));
    let tag_v = opts_value(vm, opts, id!(tag));
    let sensor_v = opts_value(vm, opts, id!(sensor));
    let body_v = opts_value(vm, opts, id!(body));
    let gravity_v = opts_value(vm, opts, id!(gravity));
    let vel_v = opts_value(vm, opts, id!(vel));
    let life_v = opts_value(vm, opts, id!(life));
    let hits_v = opts_value(vm, opts, id!(hits));
    let glow_v = opts_value(vm, opts, id!(glow));
    let face_v = opts_value(vm, opts, id!(face));
    let rot_y_v = opts_value(vm, opts, id!(rot_y));
    let collide_v = opts_value(vm, opts, id!(collide));
    let turn_v = opts_value(vm, opts, id!(turn_rate));

    let pos = if pos_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, pos_v) };
    let size = if size_v.is_nil() { vec3f(1.0, 1.0, 1.0) } else { value_vec3(vm, size_v) };
    let color = if color_v.is_nil() { vec4(0.75, 0.75, 0.8, 1.0) } else { value_color(vm, color_v) };
    let tag = if tag_v.is_nil() {
        String::new()
    } else {
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(tag_v, out);
            out.to_string()
        })
    };
    let sensor = sensor_v.as_bool().unwrap_or(false);
    let gravity_scale = if gravity_v.is_nil() {
        1.0
    } else {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        vm.bx.heap.cast_to_f64(gravity_v, ip) as f32
    };

    // `body: "kinematic"` upgrades a box to a script-driven platform.
    let kind = if body_v.is_nil() {
        kind
    } else {
        let body = vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(body_v, out);
            out.to_string()
        });
        match body.as_str() {
            "kinematic" => BodyKind::Kinematic,
            "mover" => BodyKind::Mover,
            // Full box3d dynamics (M1a): stacks, tumbles, impulses. Shared
            // replication tier; collides with statics/kinematics/rigids,
            // not with movers.
            "rigid" => BodyKind::Rigid,
            _ => kind,
        }
    };

    let vel = if vel_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, vel_v) };
    let life = if life_v.is_nil() {
        0.0
    } else {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        (vm.bx.heap.cast_to_f64(life_v, ip) as f32).max(0.0)
    };
    let hits = hits_v.as_bool().unwrap_or(false);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    let glow = if glow_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(glow_v, ip) as f32 };
    // `rot_y:` is the natural spelling for placed geometry (a rotated road
    // slab); `face:` reads better for characters. Same visual yaw either way.
    let yaw = if !rot_y_v.is_nil() {
        vm.bx.heap.cast_to_f64(rot_y_v, ip) as f32
    } else if !face_v.is_nil() {
        vm.bx.heap.cast_to_f64(face_v, ip) as f32
    } else {
        0.0
    };
    let collide = collide_v.as_bool().unwrap_or(true);
    let turn_rate = if turn_v.is_nil() { 7.0 } else { vm.bx.heap.cast_to_f64(turn_v, ip) as f32 };
    // Rigid material params (harmless defaults on every other body kind).
    let f32_or = |vm: &mut ScriptVm, key: LiveId, default: f32| -> f32 {
        let v = opts_value(vm, opts, key);
        if v.is_nil() {
            default
        } else {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            vm.bx.heap.cast_to_f64(v, ip) as f32
        }
    };
    let density = f32_or(vm, id!(density), 1.0).max(0.01);
    let friction = f32_or(vm, id!(friction), 0.6).clamp(0.0, 4.0);
    let restitution = f32_or(vm, id!(restitution), 0.0).clamp(0.0, 1.0);

    let shape_v = opts_value(vm, opts, id!(shape));
    let shape = if shape_v.is_nil() {
        Shape::Box
    } else {
        let name = vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(shape_v, out);
            out.to_string()
        });
        Shape::parse(&name)
    };

    let mut world = world.borrow_mut();
    world.mark_render_dirty();
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind,
        shape,
        pos,
        vel,
        half: vec3f(
            (size.x * 0.5).max(0.01),
            (size.y * 0.5).max(0.01),
            (size.z * 0.5).max(0.01),
        ),
        color,
        tag,
        sensor,
        collide,
        gravity_scale,
        on_floor: false,
        floor_id: 0,
        attached_to: 0,
        attach_offset: vec3f(0.0, 0.0, 0.0),
        attach_ride: false,
        attach_spin: 0.0,
        speed_mult: 1.0,
        life,
        hits,
        hit_wall: 0,
        yaw,
        // Movers face their walk direction like every Godot actor; boxes and
        // platforms hold whatever `face:` gave them.
        auto_face: kind == BodyKind::Mover,
        turn_rate,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        glow,
        orient: Quat::default(),
        density,
        friction,
        restitution,
    });
    ScriptValue::from_f64(id as f64)
}

/// One call builds a whole heightfield of column boxes — the corpus built
/// ~960 of these by hand in script. Heights come from a flat row-major
/// script array (index z * cells + x, world-y column tops) or, absent that,
/// from built-in terraced value noise seeded by `seed`. Colors: parallel
/// `colors` array, or `color` auto-shaded darker (low) to lighter (high).
fn spawn_terrain(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    opts: ScriptObject,
) -> ScriptValue {
    let f32_opt = |vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: f32| -> f32 {
        let v = opts_value(vm, opts, key);
        if v.is_nil() {
            default
        } else {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            vm.bx.heap.cast_to_f64(v, ip) as f32
        }
    };
    let span = f32_opt(vm, opts, id!(size), 120.0).max(2.0);
    // 384^2 vertices ≈ 147k — engine-generated, so no script instruction cost;
    // the Godot corpus runs 257x257.
    let cells = f32_opt(vm, opts, id!(cells), 24.0).clamp(2.0, 384.0) as usize;
    let base = f32_opt(vm, opts, id!(base), -12.0);
    let amp = f32_opt(vm, opts, id!(amp), 6.0);
    let seed = f32_opt(vm, opts, id!(seed), 1.0) as u64;
    // Noise shaping (engine-side so big worlds don't burn the eval budget):
    // `freq` lattice frequency per cell, `offset` raises the whole field,
    // `step` terrace size (0 = smooth), `min`/`max` clamp, and `plaza`
    // flattens a disc at the origin with a blend ramp — the corpus layout.
    let noise_freq = f32_opt(vm, opts, id!(freq), 0.18).clamp(0.005, 2.0);
    let noise_offset = f32_opt(vm, opts, id!(offset), 0.0);
    let terrace = f32_opt(vm, opts, id!(step), 1.0).max(0.0);
    let clamp_min = f32_opt(vm, opts, id!(min), f32::MIN);
    let clamp_max = f32_opt(vm, opts, id!(max), f32::MAX);
    let plaza_v = opts_value(vm, opts, id!(plaza));
    let plaza = plaza_v.as_object().map(|p| {
        (
            f32_opt(vm, p, id!(r), 20.0),
            f32_opt(vm, p, id!(ramp), 12.0).max(0.01),
            f32_opt(vm, p, id!(h), 0.0),
        )
    });
    let color_v = opts_value(vm, opts, id!(color));
    let base_color = if color_v.is_nil() {
        vec4(0.36, 0.62, 0.32, 1.0)
    } else {
        value_color(vm, color_v)
    };
    let tag_v = opts_value(vm, opts, id!(tag));
    let tag = if tag_v.is_nil() {
        "terrain".to_string()
    } else {
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(tag_v, out);
            out.to_string()
        })
    };

    let heights_v = opts_value(vm, opts, id!(heights));
    let colors_v = opts_value(vm, opts, id!(colors));
    let count = cells * cells;

    // Column tops: script array, or terraced value noise.
    let mut tops = Vec::with_capacity(count);
    let heights_len = list_len(vm, heights_v);
    if heights_len > 0 {
        let len = heights_len.min(count);
        let ip = vm.bx.threads.cur_ref().trap.ip;
        for index in 0..count {
            let top = if index < len {
                let v = list_value(vm, heights_v, index);
                if v.is_err() || v.is_nil() { base } else { vm.bx.heap.cast_to_f64(v, ip) as f32 }
            } else {
                base
            };
            tops.push(top);
        }
    } else {
        // Deterministic terraced value noise (xorshift lattice + bilinear).
        let lattice = |x: i64, z: i64| -> f32 {
            let mut h = seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((x as u64).wrapping_mul(0x2545_F491_4F6C_DD1D))
                .wrapping_add((z as u64).wrapping_mul(0x27D4_EB2F_1656_67C5));
            h ^= h >> 33;
            h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            h ^= h >> 33;
            (h >> 11) as f32 / (1u64 << 53) as f32
        };
        let cell_size = span / (cells - 1).max(1) as f32;
        let origin = -span * 0.5;
        for iz in 0..cells {
            for ix in 0..cells {
                let fx = ix as f32 * noise_freq;
                let fz = iz as f32 * noise_freq;
                let (x0, z0) = (fx.floor() as i64, fz.floor() as i64);
                let (tx, tz) = (fx.fract(), fz.fract());
                let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
                let (sx, sz) = (smooth(tx), smooth(tz));
                let h00 = lattice(x0, z0);
                let h10 = lattice(x0 + 1, z0);
                let h01 = lattice(x0, z0 + 1);
                let h11 = lattice(x0 + 1, z0 + 1);
                let h = h00 + (h10 - h00) * sx + (h01 - h00) * sz
                    + (h00 - h10 - h01 + h11) * sx * sz;
                let mut top = noise_offset + h * amp;
                if let Some((r, ramp, flat_h)) = plaza {
                    let wx = origin + ix as f32 * cell_size;
                    let wz = origin + iz as f32 * cell_size;
                    let d = (wx * wx + wz * wz).sqrt();
                    if d < r {
                        top = flat_h;
                    } else if d < r + ramp {
                        top = flat_h + (top - flat_h) * ((d - r) / ramp);
                    }
                }
                // Terraces: steps a mover can jump up, like the corpus.
                if terrace > 0.0 {
                    top = (top / terrace).floor() * terrace;
                }
                tops.push(top.clamp(clamp_min, clamp_max));
            }
        }
    }

    let (min_top, max_top) = tops.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
        (lo.min(*t), hi.max(*t))
    });
    let colors_len = list_len(vm, colors_v);

    // Height bands: `bands: [{h: 3.6, color: SAND}, ..., {h: 999, color: SNOW}]`
    // — a handful of thresholds instead of a 257x257 colors array. This is how
    // the corpus paints sand/grass/dirt/stone/snowy-mountain terrain.
    let bands_v = opts_value(vm, opts, id!(bands));
    let bands_len = list_len(vm, bands_v);
    let mut bands: Vec<(f32, Vec4f)> = Vec::with_capacity(bands_len);
    for index in 0..bands_len {
        let entry = list_value(vm, bands_v, index);
        if let Some(entry) = entry.as_object() {
            let h_v = opts_value(vm, entry, id!(h));
            let c_v = opts_value(vm, entry, id!(color));
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let h = if h_v.is_nil() { f32::MAX } else { vm.bx.heap.cast_to_f64(h_v, ip) as f32 };
            let c = if c_v.is_nil() { base_color } else { value_color(vm, c_v) };
            bands.push((h, c));
        }
    }
    bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Per-index colors (bands, script array, or auto-shade valleys darker).
    let mut vertex_colors = Vec::with_capacity(count);
    for index in 0..count {
        let color = if !bands.is_empty() {
            bands
                .iter()
                .find(|(h, _)| tops[index] <= *h)
                .map(|(_, c)| *c)
                .unwrap_or_else(|| bands.last().map(|(_, c)| *c).unwrap_or(base_color))
        } else if colors_len > 0 {
            if index < colors_len {
                let v = list_value(vm, colors_v, index);
                if v.is_err() || v.is_nil() { base_color } else { value_color(vm, v) }
            } else {
                base_color
            }
        } else {
            let t = if max_top > min_top { (tops[index] - min_top) / (max_top - min_top) } else { 0.5 };
            let shade = 0.75 + t * 0.4;
            vec4(
                (base_color.x * shade).min(1.0),
                (base_color.y * shade).min(1.0),
                (base_color.z * shade).min(1.0),
                base_color.w,
            )
        };
        vertex_colors.push(color);
    }

    let smooth = opts_value(vm, opts, id!(smooth)).as_bool().unwrap_or(false);
    let water_v = opts_value(vm, opts, id!(water));

    if smooth {
        // Smooth mode: the same heights array becomes VERTEX heights of one
        // triangulated ground mesh (cells = vertices per side) with height
        // lookups for collision — no column entities at all.
        let cell_size = span / (cells - 1).max(1) as f32;
        let mut world = world.borrow_mut();
        let revision = world.terrain.as_ref().map_or(1, |t| t.revision + 1);
        world.terrain = Some(Terrain {
            cells,
            cell_size,
            origin: -span * 0.5,
            heights: tops,
            colors: vertex_colors,
            revision,
        });
        if !water_v.is_nil() {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let level = vm.bx.heap.cast_to_f64(water_v, ip) as f32;
            world.next_id += 1;
            let id = world.next_id;
            // One translucent sensor slab: gameplay touch + the water look.
            world.push_entity(Entity {
                id,
                kind: BodyKind::Static,
                pos: vec3f(0.0, level - 0.05, 0.0),
                vel: vec3f(0.0, 0.0, 0.0),
                half: vec3f(span * 0.5, 0.05, span * 0.5),
                color: vec4(0.25, 0.55, 0.85, 0.6),
                tag: "water".to_string(),
                sensor: true,
                collide: true,
                gravity_scale: 1.0,
                on_floor: false,
                floor_id: 0,
                attached_to: 0,
                attach_offset: vec3f(0.0, 0.0, 0.0),
                attach_ride: false,
                attach_spin: 0.0,
                speed_mult: 1.0,
                life: 0.0,
                hits: false,
                hit_wall: 0,
                yaw: 0.0,
                auto_face: false,
                turn_rate: 7.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                glow: 0.0,
                shape: Shape::Box,
                            orient: Quat::default(),
                density: 1.0,
                friction: 0.6,
                restitution: 0.0,
            });
        }
        return ScriptValue::from_f64(count as f64);
    }

    let cell_size = span / cells as f32;
    let mut spawned = 0usize;
    for iz in 0..cells {
        for ix in 0..cells {
            let index = iz * cells + ix;
            let top = tops[index].max(base + 0.05);
            let color = vertex_colors[index];
            let x = (ix as f32 + 0.5) * cell_size - span * 0.5;
            let z = (iz as f32 + 0.5) * cell_size - span * 0.5;
            let mut world = world.borrow_mut();
            world.next_id += 1;
            let id = world.next_id;
            world.push_entity(Entity {
                id,
                kind: BodyKind::Static,
                pos: vec3f(x, (base + top) * 0.5, z),
                vel: vec3f(0.0, 0.0, 0.0),
                half: vec3f(cell_size * 0.5, ((top - base) * 0.5).max(0.05), cell_size * 0.5),
                color,
                tag: tag.clone(),
                sensor: false,
                collide: true,
                gravity_scale: 1.0,
                on_floor: false,
                floor_id: 0,
                attached_to: 0,
                attach_offset: vec3f(0.0, 0.0, 0.0),
                attach_ride: false,
                attach_spin: 0.0,
                speed_mult: 1.0,
                life: 0.0,
                hits: false,
                hit_wall: 0,
                yaw: 0.0,
                auto_face: false,
                turn_rate: 7.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                glow: 0.0,
                shape: Shape::Box,
                            orient: Quat::default(),
                density: 1.0,
                friction: 0.6,
                restitution: 0.0,
            });
            spawned += 1;
        }
    }
    ScriptValue::from_f64(spawned as f64)
}

fn game_dispatch(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    callbacks: &Rc<RefCell<CallbackTable>>,
    blocks: &Rc<RefCell<Blocks>>,
    eval_gen: &Rc<std::cell::Cell<u64>>,
    args: ScriptObject,
    method: LiveId,
) -> ScriptValue {
    match method {
        x if x == LiveId::from_str("box") || x == live_id!(block) => {
            spawn_entity(vm, world, args, BodyKind::Static)
        }
        x if x == live_id!(mover) => spawn_entity(vm, world, args, BodyKind::Mover),
        // A projectile-flavored mover: same options plus vel/life/hits are
        // typically set. game.spawn({pos, vel, life: 1.5, hits: true, ...}).
        x if x == live_id!(spawn) => spawn_entity(vm, world, args, BodyKind::Mover),
        x if x == live_id!(part) => {
            let owner = arg_id(vm, args, 0);
            let Some(opts) = arg_opt(vm, args, 1).as_object() else {
                return NIL;
            };
            warn_unknown_keys(
                vm,
                world,
                "part",
                opts,
                &[id!(pos), id!(size), id!(color), id!(glow), id!(rot_x), id!(rot_y), id!(rot_z), id!(shape)],
            );
            let pos_v = opts_value(vm, opts, id!(pos));
            let size_v = opts_value(vm, opts, id!(size));
            let color_v = opts_value(vm, opts, id!(color));
            let glow_v = opts_value(vm, opts, id!(glow));
            let rx_v = opts_value(vm, opts, id!(rot_x));
            let ry_v = opts_value(vm, opts, id!(rot_y));
            let rz_v = opts_value(vm, opts, id!(rot_z));
            let offset = if pos_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, pos_v) };
            let size = if size_v.is_nil() { vec3f(0.2, 0.2, 0.2) } else { value_vec3(vm, size_v) };
            let color = if color_v.is_nil() { vec4(0.1, 0.1, 0.12, 1.0) } else { value_color(vm, color_v) };
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let glow = if glow_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(glow_v, ip) as f32 };
            let rot = vec3f(
                if rx_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(rx_v, ip) as f32 },
                if ry_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(ry_v, ip) as f32 },
                if rz_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(rz_v, ip) as f32 },
            );
            let half = vec3f(
                (size.x * 0.5).max(0.005),
                (size.y * 0.5).max(0.005),
                (size.z * 0.5).max(0.005),
            );
            let shape_v = opts_value(vm, opts, id!(shape));
            let shape = if shape_v.is_nil() {
                Shape::Box
            } else {
                let name = vm.bx.heap.temp_string_with(|heap, out| {
                    heap.cast_to_string(shape_v, out);
                    out.to_string()
                });
                Shape::parse(&name)
            };
            let mut world = world.borrow_mut();
            if world.entity(owner).is_none() {
                return NIL;
            }
            world.mark_render_dirty();
            world.next_id += 1;
            let id = world.next_id;
            world.parts.push(Part {
                id,
                owner,
                offset,
                rot,
                half,
                target_offset: offset,
                target_rot: rot,
                target_half: half,
                rate: 9.0,
                color,
                glow,
                shape,
                anim_active: false,
            });
            ScriptValue::from_f64(id as f64)
        }
        // Animate a part: set lerp targets; the engine eases toward them at
        // `rate`/second (Godot's arm-reach used delta*9). Only given keys move.
        x if x == live_id!(move_part) => {
            let pid = arg_id(vm, args, 0);
            let Some(opts) = arg_opt(vm, args, 1).as_object() else {
                return NIL;
            };
            let pos_v = opts_value(vm, opts, id!(pos));
            let size_v = opts_value(vm, opts, id!(size));
            let rx_v = opts_value(vm, opts, id!(rot_x));
            let ry_v = opts_value(vm, opts, id!(rot_y));
            let rz_v = opts_value(vm, opts, id!(rot_z));
            let rate_v = opts_value(vm, opts, id!(rate));
            let pos = if pos_v.is_nil() { None } else { Some(value_vec3(vm, pos_v)) };
            let size = if size_v.is_nil() { None } else { Some(value_vec3(vm, size_v)) };
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let rx = if rx_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(rx_v, ip) as f32) };
            let ry = if ry_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(ry_v, ip) as f32) };
            let rz = if rz_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(rz_v, ip) as f32) };
            let rate = if rate_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(rate_v, ip) as f32) };
            let mut world = world.borrow_mut();
            if let Some(part) = world.parts.iter_mut().find(|p| p.id == pid) {
                if let Some(pos) = pos {
                    part.target_offset = pos;
                }
                if let Some(size) = size {
                    part.target_half = vec3f(
                        (size.x * 0.5).max(0.005),
                        (size.y * 0.5).max(0.005),
                        (size.z * 0.5).max(0.005),
                    );
                }
                if let Some(rx) = rx {
                    part.target_rot.x = rx;
                }
                if let Some(ry) = ry {
                    part.target_rot.y = ry;
                }
                if let Some(rz) = rz {
                    part.target_rot.z = rz;
                }
                if let Some(rate) = rate {
                    part.rate = rate.max(0.1);
                }
                part.anim_active = true;
            }
            // The part leaves the static slab while it animates.
            let owner = world.parts.iter().find(|p| p.id == pid).map(|p| p.owner);
            if let Some(owner) = owner {
                if world.is_static_visual(owner) {
                    world.mark_render_dirty();
                }
            }
            NIL
        }
        // Manual facing: sets the model yaw and takes over from auto-face —
        // vehicles pointing where they drive, the headcrab's riding spin.
        // The takeover is STICKY (walk does not revert it — no silent
        // write-then-revert, features.md Idea 2); `game.face(id)` with no yaw
        // hands facing back to auto.
        x if x == live_id!(face) => {
            let id = arg_id(vm, args, 0);
            let yaw_v = arg_opt(vm, args, 1);
            let yaw = if yaw_v.is_nil() {
                None
            } else {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                Some(vm.bx.heap.cast_to_f64(yaw_v, ip) as f32)
            };
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                match yaw {
                    None => e.auto_face = e.kind == BodyKind::Mover,
                    Some(yaw) => {
                        e.yaw = yaw;
                        e.auto_face = false;
                    }
                }
            }
            NIL
        }
        x if x == live_id!(yaw) => {
            let id = arg_id(vm, args, 0);
            let yaw = world.borrow().entity(id).map(|e| e.yaw).unwrap_or(0.0);
            ScriptValue::from_f64(yaw as f64)
        }
        // Visual model scale (physics box unchanged), eased like Godot's
        // `_model.scale.lerp(target, delta*6)` — CatNap's curl, giant bosses.
        x if x == live_id!(scale) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let s = value_vec3(vm, v);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                // Animating statics render through the dynamic path until the
                // ease settles (see step_world), so drop them from the slab.
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.scale_target = vec3f(s.x.max(0.01), s.y.max(0.01), s.z.max(0.01));
            }
            NIL
        }
        // Emission energy on an entity body or a part (glowing eyes ramp
        // 1.5→5 with AI state in the corpus).
        x if x == live_id!(glow) => {
            let id = arg_id(vm, args, 0);
            let energy = arg_f32(vm, args, 1).max(0.0);
            let mut world = world.borrow_mut();
            // Static entity, or a part on a static owner → slab content changed.
            let part_owner = world.parts.iter().find(|p| p.id == id).map(|p| p.owner);
            if world.is_static_visual(id)
                || part_owner.is_some_and(|o| world.is_static_visual(o))
            {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.glow = energy;
            } else if let Some(p) = world.parts.iter_mut().find(|p| p.id == id) {
                p.glow = energy;
            }
            NIL
        }
        // Sky + fog. game.sky({}) = the Godot game's daylight defaults.
        x if x == live_id!(sky) => {
            let mut config = SkyConfig::default();
            if let Some(opts) = arg_opt(vm, args, 0).as_object() {
                warn_unknown_keys(vm, world, "sky", opts, &[id!(top), id!(horizon), id!(ground), id!(fog)]);
                let top_v = opts_value(vm, opts, id!(top));
                let horizon_v = opts_value(vm, opts, id!(horizon));
                let ground_v = opts_value(vm, opts, id!(ground));
                let fog_v = opts_value(vm, opts, id!(fog));
                if !top_v.is_nil() {
                    config.top = value_color(vm, top_v);
                }
                if !horizon_v.is_nil() {
                    config.horizon = value_color(vm, horizon_v);
                }
                if !ground_v.is_nil() {
                    config.ground = value_color(vm, ground_v);
                    config.ground_bottom = vec4(
                        config.ground.x * 0.45,
                        config.ground.y * 0.55,
                        config.ground.z * 0.45,
                        1.0,
                    );
                }
                if !fog_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    config.fog = (vm.bx.heap.cast_to_f64(fog_v, ip) as f32).clamp(0.0, 0.2);
                }
            }
            {
                let mut world = world.borrow_mut();
                world.sky = Some(config);
                // Fog parameters are baked into the static instance slabs.
                world.mark_render_dirty();
            }
            NIL
        }
        x if x == live_id!(label) => {
            // game.label(id, text)          → the entity's default nametag.
            // game.label(id, text, {height, color, size}) → an EXTRA label,
            // returns a label id for game.label_text updates ("HELP!" bubbles).
            let id = arg_id(vm, args, 0);
            let text = arg_string(vm, args, 1);
            let opts_v = arg_opt(vm, args, 2);
            let mut height = f32::NAN;
            let mut color = vec4(0.0, 0.0, 0.0, 0.0);
            let mut size = 0.0f32;
            let extra = opts_v.as_object().is_some();
            if let Some(opts) = opts_v.as_object() {
                let height_v = opts_value(vm, opts, id!(height));
                let color_v = opts_value(vm, opts, id!(color));
                let size_v = opts_value(vm, opts, id!(size));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !height_v.is_nil() {
                    height = vm.bx.heap.cast_to_f64(height_v, ip) as f32;
                }
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !size_v.is_nil() {
                    size = vm.bx.heap.cast_to_f64(size_v, ip) as f32;
                }
            }
            let mut world = world.borrow_mut();
            if !extra {
                // Default nametag: replace in place (empty text removes).
                world.labels.retain(|l| !(l.owner == id && l.default));
                if !text.is_empty() && world.entity(id).is_some() {
                    world.next_id += 1;
                    let lid = world.next_id;
                    world.labels.push(LabelDef {
                        lid,
                        owner: id,
                        text,
                        height,
                        color,
                        size,
                        default: true,
                    });
                }
                return NIL;
            }
            if text.is_empty() || world.entity(id).is_none() {
                return NIL;
            }
            world.next_id += 1;
            let lid = world.next_id;
            world.labels.push(LabelDef {
                lid,
                owner: id,
                text,
                height,
                color,
                size,
                default: false,
            });
            ScriptValue::from_f64(lid as f64)
        }
        x if x == live_id!(label_text) => {
            let lid = arg_id(vm, args, 0);
            let text = arg_string(vm, args, 1);
            let mut world = world.borrow_mut();
            if text.is_empty() {
                world.labels.retain(|l| l.lid != lid);
            } else if let Some(label) = world.labels.iter_mut().find(|l| l.lid == lid) {
                label.text = text;
            }
            NIL
        }
        x if x == live_id!(terrain) => {
            let Some(opts) = arg(vm, args, 0).as_object() else {
                return NIL;
            };
            spawn_terrain(vm, world, opts)
        }
        x if x == live_id!(ground_y) => {
            // Terrain height at (x, z) — place spawns/goals on engine noise.
            let x = arg_f32(vm, args, 0);
            let z = arg_f32(vm, args, 1);
            let world = world.borrow();
            match world.terrain.as_ref().and_then(|t| t.height_at(x, z)) {
                Some(h) => ScriptValue::from_f64(h as f64),
                None => NIL,
            }
        }
        x if x == live_id!(ground_peak) => {
            // Highest terrain vertex, as vec3 — where the corpus puts the goal.
            let world = world.borrow();
            let Some(t) = world.terrain.as_ref() else {
                return NIL;
            };
            let mut best = (0usize, f32::MIN);
            for (index, h) in t.heights.iter().enumerate() {
                if *h > best.1 {
                    best = (index, *h);
                }
            }
            let ix = (best.0 % t.cells) as f32;
            let iz = (best.0 / t.cells) as f32;
            let pos = vec3f(
                t.origin + ix * t.cell_size,
                best.1,
                t.origin + iz * t.cell_size,
            );
            drop(world);
            vec3_value(vm, pos)
        }
        x if x == live_id!(reset) => {
            // Release this world's callback slots, then wipe. The synth stop
            // moved host-side too (reset_content is pure sim now).
            {
                let mut world = world.borrow_mut();
                let mut callbacks = callbacks.borrow_mut();
                if let Some(slot) = world.on_tick.take() {
                    callbacks.free(slot);
                }
                if let Some(slot) = world.on_touch.take() {
                    callbacks.free(slot);
                }
                for timer in world.timers.drain(..) {
                    callbacks.free(timer.func);
                }
                world.reset_content();
            }
            blocks.borrow_mut().clear();
            crate::synth::stop_all_tones();
            NIL
        }
        x if x == live_id!(gravity) => {
            let g = arg_f32(vm, args, 0);
            world.borrow_mut().gravity = g;
            NIL
        }
        x if x == live_id!(on_tick) => {
            let func = arg(vm, args, 0);
            let slot = fn_ref(vm, func)
                .map(|func| callbacks.borrow_mut().alloc(eval_gen.get(), func));
            let old = std::mem::replace(&mut world.borrow_mut().on_tick, slot);
            if let Some(old) = old {
                // Re-registration replaces: the previous closure is released.
                callbacks.borrow_mut().free(old);
            }
            NIL
        }
        x if x == live_id!(on_touch) => {
            let func = arg(vm, args, 0);
            let slot = fn_ref(vm, func)
                .map(|func| callbacks.borrow_mut().alloc(eval_gen.get(), func));
            let old = std::mem::replace(&mut world.borrow_mut().on_touch, slot);
            if let Some(old) = old {
                callbacks.borrow_mut().free(old);
            }
            NIL
        }
        x if x == live_id!(after) => {
            let secs = arg_f32(vm, args, 0);
            let func = arg(vm, args, 1);
            let func = fn_ref(vm, func);
            let mut world = world.borrow_mut();
            let at_tick = world.tick + (secs.max(0.0) / TICK_DT) as u64;
            if let Some(func) = func {
                let slot = callbacks.borrow_mut().alloc(eval_gen.get(), func);
                world.next_id += 1;
                let id = world.next_id;
                world.timers.push(GameTimer {
                    id,
                    at_tick,
                    interval_ticks: 0,
                    func: slot,
                });
                return ScriptValue::from_f64(id as f64);
            }
            NIL
        }
        // Repeating timer: fires every `secs` until game.cancel(id) or reload.
        x if x == live_id!(every) => {
            let secs = arg_f32(vm, args, 0);
            let func = arg(vm, args, 1);
            let func = fn_ref(vm, func);
            let mut world = world.borrow_mut();
            let interval_ticks = ((secs.max(0.02) / TICK_DT) as u64).max(1);
            let at_tick = world.tick + interval_ticks;
            if let Some(func) = func {
                let slot = callbacks.borrow_mut().alloc(eval_gen.get(), func);
                world.next_id += 1;
                let id = world.next_id;
                world.timers.push(GameTimer {
                    id,
                    at_tick,
                    interval_ticks,
                    func: slot,
                });
                return ScriptValue::from_f64(id as f64);
            }
            NIL
        }
        x if x == live_id!(cancel) => {
            let id = arg_id(vm, args, 0);
            let mut world = world.borrow_mut();
            let mut callbacks = callbacks.borrow_mut();
            world.timers.retain(|t| {
                if t.id == id {
                    // A cancelled timer releases its closure slot.
                    callbacks.free(t.func);
                    false
                } else {
                    true
                }
            });
            NIL
        }
        // ── players (M2 multiplayer) ────────────────────────────────────
        x if x == live_id!(players) => {
            let ids: Vec<u32> = world.borrow().players.ids().iter().map(|p| p.0).collect();
            let array = vm.bx.heap.new_array();
            for id in ids {
                vm.bx
                    .heap
                    .array_push(array, ScriptValue::from_f64(id as f64), NoTrap);
            }
            array.into()
        }
        x if x == live_id!(player_name) => {
            let id = makepad_game_sim::PlayerId(arg_f32(vm, args, 0) as u32);
            let name = world
                .borrow()
                .players
                .get(id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            vm.bx.heap.new_string_from_str(&name)
        }
        x if x == live_id!(player_entity) => {
            let id = makepad_game_sim::PlayerId(arg_f32(vm, args, 0) as u32);
            let entity = world.borrow().players.get(id).map_or(0, |p| p.entity);
            ScriptValue::from_f64(entity as f64)
        }
        x if x == live_id!(player_input) => {
            let id = makepad_game_sim::PlayerId(arg_f32(vm, args, 0) as u32);
            let w = world.borrow();
            let (move_x, move_z) = w.player_move(id);
            let (axis_x, axis_z) = match w.players.get(id) {
                Some(p) if !id.is_local_slot() => p.input.axes(),
                _ => {
                    let key = |name: LiveId| w.held.contains(&name);
                    (
                        ((key(live_id!(right)) as i8 - key(live_id!(left)) as i8) as f64
                            + w.pad.axis_x)
                            .clamp(-1.0, 1.0),
                        ((key(live_id!(down)) as i8 - key(live_id!(up)) as i8) as f64
                            + w.pad.axis_z)
                            .clamp(-1.0, 1.0),
                    )
                }
            };
            let actions = [
                (live_id!(left), id!(left_pressed)),
                (live_id!(right), id!(right_pressed)),
                (live_id!(up), id!(up_pressed)),
                (live_id!(down), id!(down_pressed)),
                (live_id!(jump), id!(jump_pressed)),
                (live_id!(shoot), id!(shoot_pressed)),
                (live_id!(grab), id!(grab_pressed)),
                (live_id!(reset), id!(reset_pressed)),
            ];
            let states: Vec<(LiveId, LiveId, bool, bool)> = actions
                .iter()
                .map(|(a, p)| (*a, *p, w.action_held_for(id, *a), w.action_pressed_for(id, *a)))
                .collect();
            drop(w);
            let obj = vm.bx.heap.new_object();
            vm.bx.heap.set_object_storage_auto(obj);
            let heap = &mut vm.bx.heap;
            for (action, pressed_key, is_held, was_pressed) in states {
                heap.set_value(obj, action.into(), ScriptValue::from_bool(is_held), NoTrap);
                heap.set_value(
                    obj,
                    pressed_key.into(),
                    ScriptValue::from_bool(was_pressed),
                    NoTrap,
                );
            }
            heap.set_value(obj, id!(axis_x).into(), ScriptValue::from_f64(axis_x), NoTrap);
            heap.set_value(obj, id!(axis_z).into(), ScriptValue::from_f64(axis_z), NoTrap);
            heap.set_value(obj, id!(move_x).into(), ScriptValue::from_f64(move_x), NoTrap);
            heap.set_value(obj, id!(move_z).into(), ScriptValue::from_f64(move_z), NoTrap);
            obj.into()
        }
        x if x == live_id!(bot) => {
            let name = arg_string(vm, args, 0);
            let name = if name.is_empty() { "bot".to_string() } else { name };
            let id = world
                .borrow_mut()
                .players
                .add(name, makepad_game_sim::PlayerSource::Bot);
            ScriptValue::from_f64(id.0 as f64)
        }
        x if x == live_id!(on_join) || x == live_id!(on_leave) => {
            let func = arg(vm, args, 0);
            let slot = fn_ref(vm, func)
                .map(|func| callbacks.borrow_mut().alloc(eval_gen.get(), func));
            let joining = method == live_id!(on_join);
            let old = {
                let mut w = world.borrow_mut();
                let target = if joining { &mut w.on_join } else { &mut w.on_leave };
                std::mem::replace(target, slot)
            };
            if let Some(old) = old {
                callbacks.borrow_mut().free(old);
            }
            NIL
        }
        // ── building blocks (libs/game/blocks) ──────────────────────────
        x if x == live_id!(car) => spawn_car(vm, world, blocks, args),
        x if x == live_id!(character) => spawn_character(vm, world, blocks, args),
        x if x == live_id!(plane) => spawn_plane(vm, world, blocks, args),
        x if x == live_id!(drive) => {
            let id = arg_id(vm, args, 0);
            let opts_val = arg(vm, args, 1);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(
                vm,
                world,
                "drive",
                opts,
                &[
                    id!(steer),
                    id!(throttle),
                    id!(brake),
                    id!(handbrake),
                    id!(pitch),
                    id!(roll),
                    id!(move_x),
                    id!(move_z),
                    id!(jump),
                ],
            );
            let read = |vm: &mut ScriptVm, key: LiveId| {
                opts_value(vm, opts, key).as_f64().map(|v| v as f32)
            };
            let (steer, throttle, brake, handbrake, pitch, roll, move_x, move_z) = (
                read(vm, id!(steer)),
                read(vm, id!(throttle)),
                read(vm, id!(brake)),
                read(vm, id!(handbrake)),
                read(vm, id!(pitch)),
                read(vm, id!(roll)),
                read(vm, id!(move_x)),
                read(vm, id!(move_z)),
            );
            let jump = opts_value(vm, opts, id!(jump)).as_bool();
            let found = blocks.borrow_mut().drive(id, |input| {
                if let Some(v) = steer {
                    input.steer = v;
                }
                if let Some(v) = throttle {
                    input.throttle = v;
                }
                if let Some(v) = brake {
                    input.brake = v;
                }
                if let Some(v) = handbrake {
                    input.handbrake = v;
                }
                if let Some(v) = pitch {
                    input.pitch = v;
                }
                if let Some(v) = roll {
                    input.roll = v;
                }
                if let Some(v) = move_x {
                    input.move_x = v;
                }
                if let Some(v) = move_z {
                    input.move_z = v;
                }
                if let Some(v) = jump {
                    input.jump = v;
                    input.jump_pressed = v;
                }
            });
            ScriptValue::from_bool(found)
        }
        x if x == live_id!(autodrive) => {
            let id = arg_id(vm, args, 0);
            let opts_val = arg(vm, args, 1);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(vm, world, "autodrive", opts, &[id!(points), id!(pace)]);
            let points = read_point_list(vm, opts, id!(points));
            let pace = opts_value(vm, opts, id!(pace))
                .as_f64()
                .map(|v| v as f32)
                .unwrap_or(1.0);
            let mut blocks = blocks.borrow_mut();
            if let Some(car) = blocks.car_mut(id) {
                car.route = points;
                car.route_at = 0;
                car.route_pace = pace.clamp(0.0, 1.0);
                car.control = ControlSource::Script;
                return ScriptValue::from_bool(true);
            }
            ScriptValue::from_bool(false)
        }
        x if x == live_id!(speed) => {
            let id = arg_id(vm, args, 0);
            let blocks = blocks.borrow();
            let speed = blocks
                .cars
                .iter()
                .find(|c| c.entity == id)
                .map(|c| c.speed)
                .or_else(|| {
                    blocks
                        .planes
                        .iter()
                        .find(|p| p.entity == id)
                        .map(|p| p.airspeed)
                })
                .unwrap_or_else(|| {
                    world
                        .borrow()
                        .entity(id)
                        .map(|e| (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt())
                        .unwrap_or(0.0)
                });
            ScriptValue::from_f64(speed as f64)
        }
        // ── brains ──────────────────────────────────────────────────────
        x if x == live_id!(wander) => {
            let id = arg_id(vm, args, 0);
            let opts_val = arg(vm, args, 1);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(vm, world, "wander", opts, &[id!(range), id!(speed), id!(pause), id!(home)]);
            let home = opts_vec3(vm, opts, id!(home)).unwrap_or_else(|| {
                world.borrow().entity(id).map(|e| e.pos).unwrap_or_default()
            });
            let range = opts_f32(vm, opts, id!(range), 8.0);
            let speed = opts_f32(vm, opts, id!(speed), 3.0);
            let pause = opts_f32(vm, opts, id!(pause), 0.8);
            set_brain(
                blocks,
                id,
                BrainKind::Wander {
                    home,
                    range,
                    speed,
                    pause,
                },
            )
        }
        x if x == live_id!(chase) => {
            let id = arg_id(vm, args, 0);
            let opts_val = arg(vm, args, 1);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(
                vm,
                world,
                "chase",
                opts,
                &[id!(tag), id!(target), id!(range), id!(catch), id!(speed)],
            );
            let tag = opts_string(vm, opts, id!(tag));
            let target = opts_value(vm, opts, id!(target))
                .as_f64()
                .map(|v| v as u64)
                .unwrap_or(0);
            let range = opts_f32(vm, opts, id!(range), 30.0);
            let catch = opts_f32(vm, opts, id!(catch), 1.5);
            let speed = opts_f32(vm, opts, id!(speed), 5.0);
            set_brain(
                blocks,
                id,
                BrainKind::Chase {
                    tag,
                    target,
                    range,
                    catch,
                    speed,
                },
            )
        }
        x if x == live_id!(patrol) => {
            let id = arg_id(vm, args, 0);
            let opts_val = arg(vm, args, 1);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(vm, world, "patrol", opts, &[id!(points), id!(speed), id!(loop)]);
            let points = read_point_list(vm, opts, id!(points));
            let speed = opts_f32(vm, opts, id!(speed), 4.0);
            let looping = opts_value(vm, opts, id!(loop)).as_bool().unwrap_or(true);
            set_brain(
                blocks,
                id,
                BrainKind::Patrol {
                    points,
                    speed,
                    looping,
                },
            )
        }
        x if x == live_id!(caught) => {
            let id = arg_id(vm, args, 0);
            let caught = blocks
                .borrow()
                .brains
                .iter()
                .find(|b| b.entity == id)
                .map(|b| b.caught)
                .unwrap_or(0);
            ScriptValue::from_f64(caught as f64)
        }
        // ── race kit ────────────────────────────────────────────────────
        x if x == live_id!(spawnpoint) => {
            let opts_val = arg(vm, args, 0);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(vm, world, "spawnpoint", opts, &[id!(pos), id!(yaw)]);
            let pos = opts_vec3(vm, opts, id!(pos)).unwrap_or_default();
            let yaw = opts_f32(vm, opts, id!(yaw), 0.0);
            let slot = blocks.borrow_mut().race.add_spawn(pos, yaw);
            ScriptValue::from_f64(slot as f64)
        }
        x if x == live_id!(checkpoint) => {
            let opts_val = arg(vm, args, 0);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(vm, world, "checkpoint", opts, &[id!(pos), id!(size)]);
            let pos = opts_vec3(vm, opts, id!(pos)).unwrap_or_default();
            let size = opts_vec3(vm, opts, id!(size)).unwrap_or(vec3f(6.0, 4.0, 6.0));
            let index = blocks
                .borrow_mut()
                .race
                .add_checkpoint(pos, size * 0.5, 0);
            ScriptValue::from_f64(index as f64)
        }
        x if x == live_id!(place) => {
            let id = arg_id(vm, args, 0);
            let slot = arg_f32(vm, args, 1) as usize;
            let mut world = world.borrow_mut();
            let ok = blocks.borrow_mut().race.place(&mut world, slot, id);
            ScriptValue::from_bool(ok)
        }
        x if x == live_id!(race) => {
            let opts_val = arg(vm, args, 0);
            let Some(opts) = opts_val.as_object() else {
                return NIL;
            };
            warn_unknown_keys(vm, world, "race", opts, &[id!(laps)]);
            let laps = opts_f32(vm, opts, id!(laps), 3.0) as u32;
            blocks.borrow_mut().race.start(laps);
            NIL
        }
        x if x == live_id!(standings) => {
            let order = blocks.borrow().race.order();
            let array = vm.bx.heap.new_array();
            for standing in order {
                let obj = vm.bx.heap.new_object();
                let heap = &mut vm.bx.heap;
                heap.set_value(
                    obj,
                    id!(entity).into(),
                    ScriptValue::from_f64(standing.entity as f64),
                    NoTrap,
                );
                heap.set_value(
                    obj,
                    id!(lap).into(),
                    ScriptValue::from_f64(standing.lap as f64),
                    NoTrap,
                );
                heap.set_value(
                    obj,
                    id!(checkpoint).into(),
                    ScriptValue::from_f64(standing.checkpoint as f64),
                    NoTrap,
                );
                heap.set_value(
                    obj,
                    id!(finished).into(),
                    ScriptValue::from_bool(standing.finished),
                    NoTrap,
                );
                heap.set_value(
                    obj,
                    id!(score).into(),
                    ScriptValue::from_f64(standing.score as f64),
                    NoTrap,
                );
                vm.bx.heap.array_push(array, obj.into(), NoTrap);
            }
            array.into()
        }
        x if x == live_id!(lap) => {
            let id = arg_id(vm, args, 0);
            let lap = blocks
                .borrow()
                .race
                .standing_of(id)
                .map(|s| s.lap)
                .unwrap_or(0);
            ScriptValue::from_f64(lap as f64)
        }
        x if x == live_id!(rank) => {
            let id = arg_id(vm, args, 0);
            ScriptValue::from_f64(blocks.borrow().race.rank_of(id) as f64)
        }
        x if x == live_id!(finished) => {
            let id = arg_id(vm, args, 0);
            let finished = blocks
                .borrow()
                .race
                .standing_of(id)
                .map(|s| s.finished)
                .unwrap_or(false);
            ScriptValue::from_bool(finished)
        }
        x if x == live_id!(score) => {
            let id = arg_id(vm, args, 0);
            let points = arg_f32(vm, args, 1);
            blocks.borrow_mut().race.add_score(id, points as i32);
            NIL
        }
        x if x == live_id!(score_of) => {
            let id = arg_id(vm, args, 0);
            let score = blocks
                .borrow()
                .race
                .standing_of(id)
                .map(|s| s.score)
                .unwrap_or(0);
            ScriptValue::from_f64(score as f64)
        }
        x if x == live_id!(walk) => {
            let id = arg_id(vm, args, 0);
            let vx = arg_f32(vm, args, 1);
            let vz = arg_f32(vm, args, 2);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                // speed_mult is the engine-side debuff (headcrab on your head):
                // the walking script never has to know.
                e.vel.x = vx * e.speed_mult;
                e.vel.z = vz * e.speed_mult;
            }
            NIL
        }
        x if x == live_id!(speed_mult) => {
            let id = arg_id(vm, args, 0);
            let f = arg_f32(vm, args, 1);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                e.speed_mult = f.clamp(0.0, 10.0);
            }
            NIL
        }
        x if x == live_id!(jump) => {
            let id = arg_id(vm, args, 0);
            let v = arg_f32(vm, args, 1);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                e.vel.y = v;
            }
            NIL
        }
        x if x == live_id!(on_floor) => {
            let id = arg_id(vm, args, 0);
            let on = world.borrow().entity(id).map(|e| e.on_floor).unwrap_or(false);
            ScriptValue::from_bool(on)
        }
        x if x == live_id!(pos) => {
            let id = arg_id(vm, args, 0);
            let pos = world.borrow().entity(id).map(|e| e.pos).unwrap_or_default();
            vec3_value(vm, pos)
        }
        x if x == live_id!(vel) => {
            let id = arg_id(vm, args, 0);
            let vel = world.borrow().entity(id).map(|e| e.vel).unwrap_or_default();
            vec3_value(vm, vel)
        }
        x if x == live_id!(set_pos) || x == live_id!(teleport) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let pos = value_vec3(vm, v);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.pos = pos;
                e.vel = vec3f(0.0, 0.0, 0.0);
            }
            NIL
        }
        x if x == live_id!(set_vel) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let vel = value_vec3(vm, v);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                e.vel = vel;
            }
            NIL
        }
        x if x == live_id!(set_color) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let color = value_color(vm, v);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.color = color;
            }
            NIL
        }
        x if x == live_id!(remove) => {
            let id = arg_id(vm, args, 0);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            world.entities.retain(|e| e.id != id);
            NIL
        }
        x if x == live_id!(tag) => {
            let id = arg_id(vm, args, 0);
            let tag = world
                .borrow()
                .entity(id)
                .map(|e| e.tag.clone())
                .unwrap_or_default();
            vm.bx.heap.new_string_from_str(&tag)
        }
        x if x == live_id!(find) => {
            let tag = arg_string(vm, args, 0);
            let ids: Vec<u64> = world
                .borrow()
                .entities
                .iter()
                .filter(|e| e.tag == tag)
                .map(|e| e.id)
                .collect();
            let array = vm.bx.heap.new_array();
            let trap = vm.bx.threads.cur().trap.pass();
            for id in ids {
                vm.bx
                    .heap
                    .array_push(array, ScriptValue::from_f64(id as f64), trap);
            }
            array.into()
        }
        x if x == live_id!(distance) => {
            // Either argument may be an entity id or a vec3 point — checkpoints
            // are positions, not entities (features.md §15).
            let resolve = |vm: &mut ScriptVm, world: &GameWorld, v: ScriptValue| -> Option<Vec3f> {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
                    NumericValue::Vec3(p) => Some(p),
                    _ => {
                        let id = vm.bx.heap.cast_to_f64(v, ip) as u64;
                        world.entity(id).map(|e| e.pos)
                    }
                }
            };
            let av = arg(vm, args, 0);
            let bv = arg(vm, args, 1);
            let world = world.borrow();
            let d = match (resolve(vm, &world, av), resolve(vm, &world, bv)) {
                (Some(a), Some(b)) => (a - b).length(),
                _ => f32::MAX,
            };
            ScriptValue::from_f64(d as f64)
        }
        x if x == live_id!(held) => {
            let action = arg_string(vm, args, 0);
            let held = world.borrow().action_held(LiveId::from_str(&action));
            ScriptValue::from_bool(held)
        }
        x if x == live_id!(pressed) => {
            let action = arg_string(vm, args, 0);
            let pressed = world.borrow().action_pressed(LiveId::from_str(&action));
            ScriptValue::from_bool(pressed)
        }
        x if x == live_id!(axis) => {
            let neg = arg_string(vm, args, 0);
            let pos = arg_string(vm, args, 1);
            let world = world.borrow();
            let v = world.action_held(LiveId::from_str(&pos)) as i8 as f64
                - world.action_held(LiveId::from_str(&neg)) as i8 as f64;
            ScriptValue::from_f64(v)
        }
        x if x == live_id!(camera) => {
            let opts_val = arg(vm, args, 0);
            if let Some(opts) = opts_val.as_object() {
                warn_unknown_keys(
                    vm,
                    world,
                    "camera",
                    opts,
                    &[
                        id!(target),
                        id!(distance),
                        id!(follow),
                        id!(side),
                        id!(third_person),
                        id!(chase),
                        id!(lag),
                        id!(recenter),
                        id!(speed_tighten),
                        id!(height),
                        id!(boom),
                        id!(pitch),
                        id!(fov),
                    ],
                );
                let target_v = opts_value(vm, opts, id!(target));
                let distance_v = opts_value(vm, opts, id!(distance));
                let follow_v = opts_value(vm, opts, id!(follow));
                let side_v = opts_value(vm, opts, id!(side));
                let mut world = world.borrow_mut();
                if !target_v.is_nil() {
                    let target = {
                        let ip = vm.bx.threads.cur_ref().trap.ip;
                        match NumericValue::from_script_value_heap(&vm.bx.heap, target_v, ip) {
                            NumericValue::Vec3(v) => v,
                            _ => world.cam_target,
                        }
                    };
                    world.cam_target = target;
                }
                if !distance_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    world.cam_distance = vm.bx.heap.cast_to_f64(distance_v, ip) as f32;
                }
                if !follow_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    world.cam_follow = vm.bx.heap.cast_to_f64(follow_v, ip) as u64;
                }
                if !side_v.is_nil() {
                    world.cam_side = side_v.as_bool().unwrap_or(false);
                }
                // Third-person rig: pivot on an entity, drag orbits around it,
                // boom pulls in when geometry is in the way (Godot player cam).
                let third_v = opts_value(vm, opts, id!(third_person));
                let height_v = opts_value(vm, opts, id!(height));
                let boom_v = opts_value(vm, opts, id!(boom));
                let pitch_v = opts_value(vm, opts, id!(pitch));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !third_v.is_nil() {
                    world.cam_third = vm.bx.heap.cast_to_f64(third_v, ip) as u64;
                }
                // Chase rig: third_person's rendering (pivot, boom, occlusion
                // pull-in) plus engine-side ease-behind-the-target, so a
                // racing camera is one line instead of hand-rolled yaw math.
                // chase: 0 stops the easing but leaves the third-person rig
                // on the last chase target for the mouse to orbit.
                let chase_v = opts_value(vm, opts, id!(chase));
                if !chase_v.is_nil() {
                    world.cam_chase = vm.bx.heap.cast_to_f64(chase_v, ip) as u64;
                    if world.cam_chase != 0 {
                        world.cam_third = world.cam_chase;
                    }
                }
                let lag_v = opts_value(vm, opts, id!(lag));
                if !lag_v.is_nil() {
                    world.cam_lag = (vm.bx.heap.cast_to_f64(lag_v, ip) as f32).max(0.05);
                }
                let recenter_v = opts_value(vm, opts, id!(recenter));
                if !recenter_v.is_nil() {
                    world.cam_recenter = (vm.bx.heap.cast_to_f64(recenter_v, ip) as f32).max(0.0);
                }
                let tighten_v = opts_value(vm, opts, id!(speed_tighten));
                if !tighten_v.is_nil() {
                    world.cam_speed_tighten =
                        (vm.bx.heap.cast_to_f64(tighten_v, ip) as f32).max(0.0);
                }
                if !height_v.is_nil() {
                    world.cam_height = vm.bx.heap.cast_to_f64(height_v, ip) as f32;
                }
                if !boom_v.is_nil() {
                    world.cam_boom = (vm.bx.heap.cast_to_f64(boom_v, ip) as f32).max(1.0);
                }
                if !pitch_v.is_nil() {
                    world.cam_pitch_request =
                        Some((vm.bx.heap.cast_to_f64(pitch_v, ip) as f32).clamp(-1.2, 0.25));
                }
                // Wider FOV = faster feel; racing games lerp it with speed.
                let fov_v = opts_value(vm, opts, id!(fov));
                if !fov_v.is_nil() {
                    world.cam_fov = (vm.bx.heap.cast_to_f64(fov_v, ip) as f32).clamp(20.0, 120.0);
                }
            }
            NIL
        }
        x if x == live_id!(text) => {
            // game.text(msg) → the "center" banner (the classic form).
            // game.text(slot, msg, {color, size, anchor}) → ANY named slot:
            // "lap", "best", ... — a race scoreboard is a handful of slots.
            // "center"/"top"/"hint" keep their historical looks and homes.
            let a1 = arg_opt(vm, args, 1);
            let (slot_name, text) = if a1.is_nil() {
                ("center".to_string(), arg_string(vm, args, 0))
            } else {
                (arg_string(vm, args, 0), arg_string(vm, args, 1))
            };
            let mut color = vec4(0.0, 0.0, 0.0, 0.0);
            let mut size = 0.0f32;
            // Slot-name defaults; an explicit anchor: overrides.
            let mut anchor = match slot_name.as_str() {
                "hint" => HudAnchor::TopLeft,
                "top" => HudAnchor::Top,
                _ => HudAnchor::Center,
            };
            if let Some(opts) = arg_opt(vm, args, 2).as_object() {
                warn_unknown_keys(vm, world, "text", opts, &[id!(color), id!(size), id!(anchor)]);
                let color_v = opts_value(vm, opts, id!(color));
                let size_v = opts_value(vm, opts, id!(size));
                let anchor_v = opts_value(vm, opts, id!(anchor));
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !size_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    size = vm.bx.heap.cast_to_f64(size_v, ip) as f32;
                }
                if !anchor_v.is_nil() {
                    let name = vm.bx.heap.temp_string_with(|heap, out| {
                        heap.cast_to_string(anchor_v, out);
                        out.to_string()
                    });
                    anchor = HudAnchor::parse(&name);
                }
            }
            let mut world = world.borrow_mut();
            if text.is_empty() {
                world.hud_slots.retain(|(n, _)| *n != slot_name);
            } else if let Some(slot) = world.hud_slot_mut(&slot_name) {
                slot.text = text;
                slot.color = color;
                slot.size = size;
                slot.anchor = anchor;
            } else {
                world
                    .hud_slots
                    .push((slot_name, HudSlot { text, color, size, anchor }));
            }
            NIL
        }
        x if x == live_id!(rand) => ScriptValue::from_f64(world.borrow_mut().rand()),
        x if x == live_id!(rand_range) => {
            let a = arg_f32(vm, args, 0) as f64;
            let b = arg_f32(vm, args, 1) as f64;
            ScriptValue::from_f64(a + (b - a) * world.borrow_mut().rand())
        }
        x if x == live_id!(cam_yaw) => {
            ScriptValue::from_f64(world.borrow().cam_yaw as f64)
        }
        x if x == live_id!(attach) => {
            let rider = arg_id(vm, args, 0);
            let owner = arg_id(vm, args, 1);
            let extra = arg_opt(vm, args, 2);
            // Third arg is either the legacy vec3 offset, or an options object
            // {pos, mode: "ride", spin}. A vec3 parses as a vec3 first.
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let (offset, ride, spin) =
                match NumericValue::from_script_value_heap(&vm.bx.heap, extra, ip) {
                    NumericValue::Vec3(v) => (v, false, 0.0),
                    _ => {
                        if let Some(opts) = extra.as_object() {
                            let pos_v = opts_value(vm, opts, id!(pos));
                            let mode_v = opts_value(vm, opts, id!(mode));
                            let spin_v = opts_value(vm, opts, id!(spin));
                            let offset = if pos_v.is_nil() {
                                vec3f(0.0, 1.0, 0.0)
                            } else {
                                value_vec3(vm, pos_v)
                            };
                            let ride = if mode_v.is_nil() {
                                false
                            } else {
                                let mode = vm.bx.heap.temp_string_with(|heap, out| {
                                    heap.cast_to_string(mode_v, out);
                                    out.to_string()
                                });
                                mode == "ride"
                            };
                            let spin = if spin_v.is_nil() {
                                0.0
                            } else {
                                let ip = vm.bx.threads.cur_ref().trap.ip;
                                vm.bx.heap.cast_to_f64(spin_v, ip) as f32
                            };
                            (offset, ride, spin)
                        } else {
                            (vec3f(0.0, 1.0, 0.0), false, 0.0)
                        }
                    }
                };
            if let Some(e) = world.borrow_mut().entity_mut(rider) {
                e.attached_to = owner;
                e.attach_offset = offset;
                e.attach_ride = ride;
                e.attach_spin = spin;
                e.vel = vec3f(0.0, 0.0, 0.0);
            }
            NIL
        }
        x if x == live_id!(detach) => {
            let rider = arg_id(vm, args, 0);
            if let Some(e) = world.borrow_mut().entity_mut(rider) {
                e.attached_to = 0;
                e.attach_ride = false;
                e.attach_spin = 0.0;
            }
            NIL
        }
        x if x == live_id!(beam) => {
            let from_v = arg(vm, args, 0);
            let from = value_vec3(vm, from_v);
            let to_v = arg(vm, args, 1);
            let to = value_vec3(vm, to_v);
            let opts_v = arg_opt(vm, args, 2);
            let mut size = 0.12f32;
            let mut color = vec4(0.9, 0.9, 0.95, 1.0);
            let mut glow = 0.0f32;
            if let Some(opts) = opts_v.as_object() {
                let size_v = opts_value(vm, opts, id!(size));
                let color_v = opts_value(vm, opts, id!(color));
                let glow_v = opts_value(vm, opts, id!(glow));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !size_v.is_nil() {
                    size = vm.bx.heap.cast_to_f64(size_v, ip) as f32;
                }
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !glow_v.is_nil() {
                    glow = vm.bx.heap.cast_to_f64(glow_v, ip) as f32;
                }
            }
            world.borrow_mut().beams.push(Beam {
                from,
                to,
                size: size.clamp(0.01, 4.0),
                color,
                glow,
            });
            NIL
        }
        x if x == live_id!(crosshair) => {
            let on = arg(vm, args, 0).as_bool().unwrap_or(true);
            world.borrow_mut().crosshair = on;
            NIL
        }
        x if x == live_id!(sfx) => {
            let name = arg_string(vm, args, 0);
            let pitch_v = arg_opt(vm, args, 1);
            let pitch = if pitch_v.is_nil() {
                1.0
            } else {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                vm.bx.heap.cast_to_f64(pitch_v, ip) as f32
            };
            if !crate::synth::play_named(&name, pitch) {
                // An unknown name is a script bug the agent should hear about.
                world.borrow_mut().log(format!("sfx: unknown sound \"{name}\""));
            }
            NIL
        }
        x if x == live_id!(beep) => {
            let Some(opts) = arg(vm, args, 0).as_object() else {
                return NIL;
            };
            let freq_v = opts_value(vm, opts, id!(freq));
            let to_v = opts_value(vm, opts, id!(to));
            let ms_v = opts_value(vm, opts, id!(ms));
            let wave_v = opts_value(vm, opts, id!(wave));
            let gain_v = opts_value(vm, opts, id!(gain));
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let freq = if freq_v.is_nil() { 440.0 } else { vm.bx.heap.cast_to_f64(freq_v, ip) as f32 };
            let to = if to_v.is_nil() { freq } else { vm.bx.heap.cast_to_f64(to_v, ip) as f32 };
            let ms = if ms_v.is_nil() { 120.0 } else { vm.bx.heap.cast_to_f64(ms_v, ip) as f32 };
            let gain = if gain_v.is_nil() { 0.25 } else { vm.bx.heap.cast_to_f64(gain_v, ip) as f32 };
            let wave = if wave_v.is_nil() {
                crate::synth::Wave::Square
            } else {
                let name = vm.bx.heap.temp_string_with(|heap, out| {
                    heap.cast_to_string(wave_v, out);
                    out.to_string()
                });
                crate::synth::Wave::parse(&name)
            };
            crate::synth::beep(freq, to, ms / 1000.0, wave, gain, 0.0);
            NIL
        }
        x if x == live_id!(jingle) => {
            let notes = arg_string(vm, args, 0);
            let ms_v = arg_opt(vm, args, 1);
            let ms = if ms_v.is_nil() {
                100.0
            } else {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                vm.bx.heap.cast_to_f64(ms_v, ip) as f32
            };
            crate::synth::jingle(&notes, ms / 1000.0, crate::synth::Wave::Triangle, 0.22);
            NIL
        }
        x if x == live_id!(log) => {
            let line = arg_string(vm, args, 0);
            world.borrow_mut().log(line);
            NIL
        }
        x if x == live_id!(time) => ScriptValue::from_f64(world.borrow().time),
        // ── writable camera (the chase-cam API, features.md §1) ─────────
        x if x == live_id!(set_cam_yaw) => {
            let yaw = arg_f32(vm, args, 0);
            world.borrow_mut().cam_yaw_request = Some(yaw);
            NIL
        }
        x if x == live_id!(set_cam_pitch) => {
            let pitch = arg_f32(vm, args, 0).clamp(-1.2, 0.25);
            world.borrow_mut().cam_pitch_request = Some(pitch);
            NIL
        }
        x if x == live_id!(set_cam_dist) => {
            let d = arg_f32(vm, args, 0);
            let mut world = world.borrow_mut();
            if world.cam_third != 0 {
                world.cam_boom = d.clamp(1.0, 60.0);
            } else {
                world.cam_distance = d.clamp(0.5, 120.0);
            }
            NIL
        }
        x if x == live_id!(set_cam_fov) => {
            let fov = arg_f32(vm, args, 0).clamp(20.0, 120.0);
            world.borrow_mut().cam_fov = fov;
            NIL
        }
        x if x == live_id!(cam_pitch) => {
            ScriptValue::from_f64(world.borrow().cam_pitch as f64)
        }
        x if x == live_id!(cam_dist) => {
            let world = world.borrow();
            let d = if world.cam_third != 0 { world.cam_boom } else { world.cam_distance };
            ScriptValue::from_f64(d as f64)
        }
        x if x == live_id!(cam_fov) => {
            ScriptValue::from_f64(world.borrow().cam_fov as f64)
        }
        x if x == live_id!(cam_dragging) => {
            ScriptValue::from_bool(world.borrow().cam_dragging)
        }
        x if x == live_id!(cam_shake) => {
            let amount = arg_f32(vm, args, 0).max(0.0);
            let mut world = world.borrow_mut();
            world.cam_shake = (world.cam_shake + amount).clamp(0.0, 1.5);
            NIL
        }
        // ── spatial queries (features.md §7) ─────────────────────────────
        x if x == live_id!(raycast) => {
            let from_v = arg(vm, args, 0);
            let from = value_vec3(vm, from_v);
            let dir_v = arg(vm, args, 1);
            let dir = value_vec3(vm, dir_v);
            let max = arg_f32(vm, args, 2).max(0.0);
            let hit = world_raycast(&world.borrow(), from, dir, max);
            match hit {
                Some((id, pos, normal, dist)) => {
                    let obj = vm.bx.heap.new_object();
                    vm.bx.heap.set_object_storage_auto(obj);
                    // Terrain reports as hit: -1 (u64::MAX doesn't survive f64).
                    let hit_id = if id == TERRAIN_ID { -1.0 } else { id as f64 };
                    let pos_v = vec3_value(vm, pos);
                    let normal_v = vec3_value(vm, normal);
                    let heap = &mut vm.bx.heap;
                    heap.set_value(obj, id!(hit).into(), ScriptValue::from_f64(hit_id), NoTrap);
                    heap.set_value(obj, id!(pos).into(), pos_v, NoTrap);
                    heap.set_value(obj, id!(normal).into(), normal_v, NoTrap);
                    heap.set_value(obj, id!(dist).into(), ScriptValue::from_f64(dist as f64), NoTrap);
                    obj.into()
                }
                None => NIL,
            }
        }
        x if x == live_id!(overlap_sphere) => {
            let center_v = arg(vm, args, 0);
            let center = value_vec3(vm, center_v);
            let r = arg_f32(vm, args, 1).max(0.0);
            let ids: Vec<u64> = world
                .borrow()
                .entities
                .iter()
                .filter(|e| {
                    // Distance from sphere center to the entity's AABB.
                    let dx = ((center.x - e.pos.x).abs() - e.half.x).max(0.0);
                    let dy = ((center.y - e.pos.y).abs() - e.half.y).max(0.0);
                    let dz = ((center.z - e.pos.z).abs() - e.half.z).max(0.0);
                    dx * dx + dy * dy + dz * dz <= r * r
                })
                .map(|e| e.id)
                .collect();
            let array = vm.bx.heap.new_array();
            let trap = vm.bx.threads.cur().trap.pass();
            for id in ids {
                vm.bx.heap.array_push(array, ScriptValue::from_f64(id as f64), trap);
            }
            array.into()
        }
        x if x == live_id!(ground_normal) => {
            let x = arg_f32(vm, args, 0);
            let z = arg_f32(vm, args, 1);
            let normal = world
                .borrow()
                .terrain
                .as_ref()
                .and_then(|t| t.normal_at(x, z));
            match normal {
                Some(n) => vec3_value(vm, n),
                None => vec3_value(vm, vec3f(0.0, 1.0, 0.0)),
            }
        }
        // ── push (features.md §8): add to velocity, don't overwrite it ──
        x if x == live_id!(push) => {
            let id = arg_id(vm, args, 0);
            let v_raw = arg(vm, args, 1);
            let v = value_vec3(vm, v_raw);
            let mut world = world.borrow_mut();
            let is_rigid = world
                .entity(id)
                .map_or(false, |e| e.kind == BodyKind::Rigid);
            if is_rigid {
                // Rigid: a real mass-scaled impulse (same Δv semantics).
                // Falls back to a velocity write for a body spawned this
                // eval — reconcile seeds the body with entity.vel.
                if !world.dynamics.rigid_impulse(id, v) {
                    if let Some(e) = world.entity_mut(id) {
                        e.vel = e.vel + v;
                    }
                }
            } else if let Some(e) = world.entity_mut(id) {
                e.vel = e.vel + v;
            }
            NIL
        }
        // ── save/load (features.md §9): best laps survive edits ─────────
        x if x == live_id!(save) => {
            let key = arg_string(vm, args, 0);
            let v = arg(vm, args, 1);
            // Strings first: the numeric cast coerces strings to NaN.
            let val = if let Some(text) = vm.bx.heap.string_with(v, |_, s| s.to_string()) {
                SaveVal::Str(text)
            } else {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                SaveVal::Num(vm.bx.heap.cast_to_f64(v, ip))
            };
            let mut world = world.borrow_mut();
            world.save_data.insert(key, val);
            world.save_dirty = true;
            NIL
        }
        x if x == live_id!(load) => {
            let key = arg_string(vm, args, 0);
            let world_ref = world.borrow();
            match world_ref.save_data.get(&key) {
                Some(SaveVal::Num(n)) => ScriptValue::from_f64(*n),
                Some(SaveVal::Str(text)) => {
                    let text = text.clone();
                    drop(world_ref);
                    vm.bx.heap.new_string_from_str(&text)
                }
                None => {
                    drop(world_ref);
                    arg_opt(vm, args, 1) // the default, or NIL
                }
            }
        }
        // ── sustained tones (features.md §10): the car-engine primitive ──
        x if x == live_id!(tone) => {
            let mut freq = 220.0f32;
            let mut gain = 0.15f32;
            let mut wave = crate::synth::Wave::Saw;
            if let Some(opts) = arg_opt(vm, args, 0).as_object() {
                warn_unknown_keys(vm, world, "tone", opts, &[id!(freq), id!(gain), id!(wave)]);
                let freq_v = opts_value(vm, opts, id!(freq));
                let gain_v = opts_value(vm, opts, id!(gain));
                let wave_v = opts_value(vm, opts, id!(wave));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !freq_v.is_nil() {
                    freq = vm.bx.heap.cast_to_f64(freq_v, ip) as f32;
                }
                if !gain_v.is_nil() {
                    gain = vm.bx.heap.cast_to_f64(gain_v, ip) as f32;
                }
                if !wave_v.is_nil() {
                    let name = vm.bx.heap.temp_string_with(|heap, out| {
                        heap.cast_to_string(wave_v, out);
                        out.to_string()
                    });
                    wave = crate::synth::Wave::parse(&name);
                }
            }
            ScriptValue::from_f64(crate::synth::tone(freq, wave, gain) as f64)
        }
        x if x == live_id!(tone_set) => {
            let id = arg_id(vm, args, 0);
            let mut freq = None;
            let mut gain = None;
            if let Some(opts) = arg_opt(vm, args, 1).as_object() {
                warn_unknown_keys(vm, world, "tone_set", opts, &[id!(freq), id!(gain)]);
                let freq_v = opts_value(vm, opts, id!(freq));
                let gain_v = opts_value(vm, opts, id!(gain));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !freq_v.is_nil() {
                    freq = Some(vm.bx.heap.cast_to_f64(freq_v, ip) as f32);
                }
                if !gain_v.is_nil() {
                    gain = Some(vm.bx.heap.cast_to_f64(gain_v, ip) as f32);
                }
            }
            crate::synth::tone_set(id, freq, gain);
            NIL
        }
        x if x == live_id!(tone_stop) => {
            crate::synth::tone_stop(arg_id(vm, args, 0));
            NIL
        }
        // ── HUD extras (features.md §13) ─────────────────────────────────
        x if x == live_id!(format) => {
            let value = arg_f32(vm, args, 0) as f64;
            let decimals = arg_opt(vm, args, 1);
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let decimals = if decimals.is_nil() {
                1
            } else {
                (vm.bx.heap.cast_to_f64(decimals, ip) as usize).min(6)
            };
            let text = format!("{:.*}", decimals, value);
            vm.bx.heap.new_string_from_str(&text)
        }
        x if x == live_id!(bar) => {
            let name = arg_string(vm, args, 0);
            let fraction = arg_f32(vm, args, 1);
            let mut color = vec4(0.4, 0.85, 0.4, 0.9);
            let mut anchor = HudAnchor::BottomLeft;
            if let Some(opts) = arg_opt(vm, args, 2).as_object() {
                warn_unknown_keys(vm, world, "bar", opts, &[id!(color), id!(anchor)]);
                let color_v = opts_value(vm, opts, id!(color));
                let anchor_v = opts_value(vm, opts, id!(anchor));
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !anchor_v.is_nil() {
                    let name = vm.bx.heap.temp_string_with(|heap, out| {
                        heap.cast_to_string(anchor_v, out);
                        out.to_string()
                    });
                    anchor = HudAnchor::parse(&name);
                }
            }
            let mut world = world.borrow_mut();
            world.hud_bars.retain(|b| b.name != name);
            // A negative fraction removes the gauge.
            if fraction >= 0.0 {
                world.hud_bars.push(HudBar {
                    name,
                    fraction: fraction.min(1.0),
                    color,
                    anchor,
                });
            }
            NIL
        }
        x if x == live_id!(api) => {
            // Introspection (features.md Idea 6): dump the whole verb surface
            // so the agent can lint itself without six test cycles.
            let mut world = world.borrow_mut();
            world.log("game.* API:".to_string());
            for (verb, sig) in GAME_API {
                world.log(format!("  game.{verb}{sig}"));
            }
            NIL
        }
        _ => {
            // A typo'd verb must be indistinguishable from any other script
            // error: it fails the eval (last-good keeps the old world) and the
            // text reaches the agent through last_error.txt / the wake-up push.
            // Silence here once cost six blind test cycles (features.md §2).
            // Location + did-you-mean, like the VM's own variable errors.
            let name = format!("{}", method);
            let loc = vm
                .bx
                .code
                .ip_to_loc(vm.bx.threads.cur_ref().trap.ip)
                .map(|l| format!("{l}: "))
                .unwrap_or_default();
            let message = match suggest_verb(&name) {
                Some(s) => {
                    format!("{loc}unknown game verb '{name}'. Did you mean '{s}'?")
                }
                None => format!(
                    "{loc}unknown game verb '{name}' — game.api() lists every verb"
                ),
            };
            world.borrow_mut().log(message.clone());
            if let Some(sink) = vm.bx.captured_errors.as_mut() {
                sink.push(message);
            }
            NIL
        }
    }
}

/// The full verb surface, for `game.api()` dumps and typo suggestions.
/// Keep in sync with `game_dispatch` and splashgame.md.
const GAME_API: &[(&str, &str)] = &[
    // ── building blocks: the engine drives these; script just configures ──
    ("car", "({pos, size, color, tag, player, top_speed, accel, braking, grip, steer_rate, seats}) -> id — raycast vehicle, engine-driven"),
    ("character", "({pos, size, color, tag, player, model, speed, jump, view}) -> id — walker with idle/walk/run blending"),
    ("plane", "({pos, size, color, tag, player, thrust, top_speed, lift_speed, auto_level}) -> id — arcade flight"),
    ("drive", "(id, {steer, throttle, brake, handbrake, pitch, roll, move_x, move_z, jump}) — set control intent (AI/script)"),
    ("autodrive", "(id, {points: [vec3], pace}) — waypoint driver for a car"),
    ("speed", "(id) -> forward speed (car), airspeed (plane), planar speed (anything else)"),
    ("wander", "(id, {home, range, speed, pause}) — amble near home"),
    ("chase", "(id, {tag, target, range, catch, speed}) — hunt the nearest tagged entity"),
    ("patrol", "(id, {points: [vec3], speed, loop}) — walk a route"),
    ("caught", "(id) -> entity a chase brain caught this tick (0 = none)"),
    ("spawnpoint", "({pos, yaw}) -> slot — start grid position"),
    ("checkpoint", "({pos, size}) -> index — race gate; must be crossed in order"),
    ("place", "(id, slot) — put an entity on a spawnpoint and enter it in the race"),
    ("race", "({laps}) — (re)start lap tracking over the declared checkpoints"),
    ("standings", "() -> [{entity, lap, checkpoint, finished, score}] leader first"),
    ("lap", "(id) -> laps completed"),
    ("rank", "(id) -> 1-based position"),
    ("finished", "(id) -> did this racer finish"),
    ("score", "(id, points) — add to a racer's score"),
    ("score_of", "(id) -> score"),
    ("players", "() -> [player ids]"),
    ("player_name", "(player) -> name"),
    ("player_entity", "(player) -> entity id (0 = none)"),
    ("player_input", "(player) -> input object"),
    ("bot", "(name) -> player id"),
    ("on_join", "(fn(player))"),
    ("on_leave", "(fn(player))"),
    ("box", "({pos, size, color, tag, sensor, collide, body, gravity, vel, life, hits, glow, face|rot_y, turn_rate, shape, density, friction, restitution})"),
    ("block", " — alias of game.box"),
    ("mover", " — same options as box (kinematic character, turn_rate default 7)"),
    ("spawn", " — same options as box (dynamic body; vel, life, hits)"),
    ("part", "(owner, {pos, size, color, glow, rot_x, rot_y, rot_z, shape})"),
    ("terrain", "({size, cells, smooth, water, seed, freq, offset, amp, step, min, max, plaza: [{r, ramp, h}], bands: [{h, color}], heights, colors, color, base, tag})"),
    ("label", "(id, text, {height, color, size}?)"),
    ("label_text", "(label_id, text)"),
    ("on_tick", "(|dt, input| ...)"),
    ("on_touch", "(|a, b| ...)"),
    ("walk", "(id, vx, vz)"),
    ("jump", "(id, v)"),
    ("on_floor", "(id)"),
    ("pos", "(id)"),
    ("vel", "(id)"),
    ("set_pos", "(id, v)"),
    ("teleport", "(id, v)"),
    ("set_vel", "(id, v)"),
    ("push", "(id, v)"),
    ("face", "(id, yaw?) — no yaw resumes auto-facing"),
    ("yaw", "(id)"),
    ("find", "(tag)"),
    ("tag", "(id)"),
    ("distance", "(a, b) — ids or vec3 points"),
    ("remove", "(id)"),
    ("attach", "(id, owner, offset | {pos, mode, spin})"),
    ("detach", "(id)"),
    ("speed_mult", "(id, f)"),
    ("raycast", "(from, dir, max)"),
    ("overlap_sphere", "(pos, r)"),
    ("ground_y", "(x, z)"),
    ("ground_normal", "(x, z)"),
    ("ground_peak", "()"),
    ("gravity", "(g)"),
    ("camera", "({third_person|chase|follow, lag, recenter, speed_tighten, height, boom, pitch, distance, side, fov, target})"),
    ("set_cam_yaw", "(a)"),
    ("set_cam_pitch", "(p)"),
    ("set_cam_dist", "(d)"),
    ("set_cam_fov", "(f)"),
    ("cam_yaw", "()"),
    ("cam_pitch", "()"),
    ("cam_dist", "()"),
    ("cam_fov", "()"),
    ("cam_dragging", "()"),
    ("cam_shake", "(amount)"),
    ("sky", "({top, horizon, ground, fog})"),
    ("set_color", "(id, c)"),
    ("glow", "(id, e)"),
    ("scale", "(id, s)"),
    ("move_part", "(part, {pos, rot_x, rot_y, rot_z, size, rate})"),
    ("beam", "(from, to, {size, color, glow})"),
    ("text", "(msg | slot, msg, {color, size, anchor})"),
    ("bar", "(name, fraction, {color, anchor})"),
    ("crosshair", "(bool)"),
    ("format", "(x, decimals)"),
    ("sfx", "(name, pitch?)"),
    ("beep", "({freq, to, ms, wave, gain})"),
    ("jingle", "(notes, ms_per_note?)"),
    ("tone", "({freq, wave, gain})"),
    ("tone_set", "(tone_id, {freq, gain})"),
    ("tone_stop", "(tone_id)"),
    ("save", "(key, value)"),
    ("load", "(key, default)"),
    ("after", "(secs, fn)"),
    ("every", "(secs, fn)"),
    ("cancel", "(timer_id)"),
    ("held", "(action)"),
    ("pressed", "(action)"),
    ("axis", "(neg, pos)"),
    ("rand", "()"),
    ("rand_range", "(a, b)"),
    ("log", "(msg)"),
    ("time", "()"),
    ("reset", "()"),
    ("api", "()"),
];

/// Nearest verb by edit distance (≤2), for typo suggestions.
fn suggest_verb(name: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for (verb, _) in GAME_API {
        let d = edit_distance(name, verb);
        if d <= 2 && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, verb));
        }
    }
    best.map(|(_, v)| v)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}


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
