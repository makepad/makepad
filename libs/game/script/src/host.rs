//! `ScriptHost` — owns a game's isolate, evaluates `game.splash` with
//! last-good rollback, and calls the script's `on_tick`/`on_touch`/timers.
//!
//! Extracted from gamemaker's proven flow, with one deliberate improvement:
//! the rollback snapshot is `GameWorld::clone()` (M1a made the world Clone via
//! a box3d snapshot round-trip) instead of a hand-written 24-field struct.
//! That struct is exactly where the `next_id` rollback bug lived — a clone
//! cannot forget a field.

use crate::callbacks::CallbackTable;
use crate::dispatch::{suggest, verb_table, AudioRequest, Ctx, ModelRequest, VerbFn};
use makepad_game_assets::AssetIndex;
use crate::sandbox::{strip_capabilities, Trust};
use makepad_game_blocks::Blocks;
use makepad_game_sim::*;
use makepad_widgets::*;
use makepad_widgets::widget_async::{CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Script sees the widget prelude, same as gamemaker.
const GAME_PREFIX: &str = "use mod.prelude.widgets.*\n";
const EVAL_INSTRUCTION_LIMIT: usize = 2_000_000;
const TICK_INSTRUCTION_LIMIT: usize = 500_000;

pub struct EvalReport {
    pub generation: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub entities: usize,
}

pub struct ScriptHost {
    pub world: Rc<RefCell<GameWorld>>,
    pub blocks: Rc<RefCell<Blocks>>,
    callbacks: Rc<RefCell<CallbackTable>>,
    audio: Rc<RefCell<Vec<AudioRequest>>>,
    /// Device-local particle requests; drained by the host each frame like
    /// audio. Never simulation state — see makepad_game_sim::particles.
    particles: Rc<RefCell<Vec<ParticleRequest>>>,
    eval_gen: Rc<Cell<u64>>,
    next_tone: Rc<Cell<u64>>,
    next_emitter: Rc<Cell<u64>>,
    /// The stock library. Shared rather than owned because building it probes
    /// ~5,000 GLBs (~1.8 s) — a host builds it once and hands the same index
    /// to every game it loads.
    assets: Rc<Option<AssetIndex>>,
    /// Stock model placements queued by this eval, drained by the host.
    models: Rc<RefCell<Vec<ModelRequest>>>,
    /// Script-declared interactables. Public so the host can ask what the
    /// primary activity button would do and draw the affordance.
    pub interact: Rc<RefCell<crate::interact::InteractSet>>,
    verbs: Rc<HashMap<LiveId, VerbFn>>,
    vm_id: SplashVmId,
    /// Checkpoint identity for streaming eval. gamemaker abuses the widget's
    /// heap address here; a host owns an explicit id so relocating (or running
    /// two games) can't silently fork eval state.
    body_id: usize,
    source: String,
    generation: u64,
    last_error: Option<String>,
    /// Downloaded games get a capability-stripped isolate. Set before the
    /// first eval — that is when the isolate is allocated and stripped.
    trust: Trust,
}

/// Distinct per host instance, so two games in one process never collide.
fn next_body_id() -> usize {
    thread_local! {
        static NEXT: Cell<usize> = const { Cell::new(1) };
    }
    NEXT.with(|n| {
        let v = n.get();
        n.set(v + 1);
        v
    })
}

impl ScriptHost {
    pub fn new() -> Self {
        Self {
            world: Rc::new(RefCell::new(GameWorld::new())),
            blocks: Rc::new(RefCell::new(Blocks::new())),
            callbacks: Rc::new(RefCell::new(CallbackTable::default())),
            audio: Rc::new(RefCell::new(Vec::new())),
            particles: Rc::new(RefCell::new(Vec::new())),
            eval_gen: Rc::new(Cell::new(0)),
            next_tone: Rc::new(Cell::new(0)),
            next_emitter: Rc::new(Cell::new(0)),
            assets: Rc::new(None),
            models: Rc::new(RefCell::new(Vec::new())),
            interact: Rc::new(RefCell::new(crate::interact::InteractSet::default())),
            verbs: Rc::new(verb_table()),
            vm_id: MAIN_SPLASH_VM_ID,
            body_id: next_body_id(),
            source: String::new(),
            generation: 0,
            last_error: None,
            trust: Trust::Local,
        }
    }

    /// A host for a game that arrived from a registry or a peer: its isolate is
    /// capability-stripped when allocated. Untrusted games are untrusted code.
    pub fn new_sandboxed() -> Self {
        Self {
            trust: Trust::Downloaded,
            ..Self::new()
        }
    }

    pub fn trust(&self) -> Trust {
        self.trust
    }

    /// Must be called before the first eval; afterwards the isolate exists and
    /// its capabilities are already fixed.
    pub fn set_trust(&mut self, trust: Trust) -> Result<(), &'static str> {
        if self.vm_id != MAIN_SPLASH_VM_ID {
            return Err("trust is fixed once the isolate has been allocated");
        }
        self.trust = trust;
        Ok(())
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Drain queued audio requests (the host owns the synth, not the sim).
    pub fn take_audio(&self) -> Vec<AudioRequest> {
        std::mem::take(&mut *self.audio.borrow_mut())
    }

    /// Drain this frame's particle requests for the device's ParticleSystem.
    pub fn take_particles(&self) -> Vec<ParticleRequest> {
        std::mem::take(&mut *self.particles.borrow_mut())
    }

    fn ctx(&self) -> Ctx {
        Ctx {
            particles: self.particles.clone(),
            next_emitter: self.next_emitter.clone(),
            world: self.world.clone(),
            blocks: self.blocks.clone(),
            callbacks: self.callbacks.clone(),
            audio: self.audio.clone(),
            eval_gen: self.eval_gen.clone(),
            next_tone: self.next_tone.clone(),
            assets: self.assets.clone(),
            models: self.models.clone(),
            interact: self.interact.clone(),
        }
    }

    /// Hand this host the stock library. Absent by default, and absence is not
    /// an error — a game built from primitives must still run on a machine
    /// that never downloaded the packs, so the asset verbs report a clear
    /// message instead of failing the eval.
    pub fn set_assets(&mut self, assets: Rc<Option<AssetIndex>>) {
        self.assets = assets;
    }

    /// Drain the stock models this eval asked for. The host loads the GLBs,
    /// hands them to the renderer, and spawns colliders for loose props from
    /// the model's own parts (tiles already carry theirs, since a kit knows
    /// its grid pitch before anything is read from disk).
    pub fn take_models(&self) -> Vec<ModelRequest> {
        std::mem::take(&mut *self.models.borrow_mut())
    }

    /// Feed source; a no-op when unchanged so an mtime poll is cheap.
    pub fn set_source(&mut self, cx: &mut Cx, source: &str) -> Option<EvalReport> {
        if self.source == source {
            return None;
        }
        self.source = source.to_string();
        Some(self.eval(cx))
    }

    pub fn eval(&mut self, cx: &mut Cx) -> EvalReport {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            self.vm_id = cx.alloc_splash_vm_with_network(false);
            // Strip before the handle is registered and before any source is
            // evaluated, so a downloaded game never sees the modules at all.
            if self.trust.is_sandboxed() {
                let vm_id = self.vm_id;
                cx.with_script_vm_id(vm_id, |vm| strip_capabilities(vm));
            }
            self.register_handle(cx);
        }
        self.generation += 1;
        self.eval_gen.set(self.generation);

        // Last-good: a full clone, so no field can be forgotten on rollback.
        let snapshot = self.world.borrow().clone();
        let blocks_snapshot = self.blocks.borrow().clone();
        let interact_snapshot = self.interact.borrow().clone();
        self.world.borrow_mut().reset_content();
        self.blocks.borrow_mut().clear();
        // Declarations are script content, so they rebuild with it. Rolled
        // back below alongside world and blocks — three snapshots restored
        // together or a failed edit leaves prompts pointing at dead entities.
        self.interact.borrow_mut().clear();

        // The trailing "\n;" finalizes the stream: eval_with_append_source is
        // a STREAMING parser, so a last statement with no terminator is held
        // back as "possibly incomplete" and silently never runs — and game
        // logic (on_tick) idiomatically sits last in the file.
        let code = format!("{}{}\n;", GAME_PREFIX, self.source);
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "game.splash".to_string(),
            line: 0,
            column: self.body_id,
            code: String::new(),
            values: vec![],
        };

        let vm_id = self.vm_id;
        let errors = cx.with_script_vm_id(vm_id, |vm| {
            vm.bx.captured_errors = Some(Vec::new());
            let _ = vm.with_instruction_limit(EVAL_INSTRUCTION_LIMIT, |vm| {
                vm.eval_with_append_source(script_mod, &code, NIL.into())
            });
            vm.take_errors()
        });

        let generation = self.generation;
        if errors.is_empty() {
            self.last_error = None;
            self.callbacks
                .borrow_mut()
                .free_generations_before(generation);
            self.blocks.borrow_mut().reconcile(&self.world.borrow());
            let entities = self.world.borrow().entities.len();
            EvalReport {
                generation,
                ok: true,
                error: None,
                entities,
            }
        } else {
            // The failed eval's registrations die with it; the snapshot's
            // (older-generation) slots stay live for the rollback.
            self.callbacks.borrow_mut().free_generation(generation);
            let time = snapshot.time;
            *self.world.borrow_mut() = snapshot;
            *self.blocks.borrow_mut() = blocks_snapshot;
            *self.interact.borrow_mut() = interact_snapshot;
            // The rolled-back world keeps ITS clock.
            self.world.borrow_mut().time = time;
            let joined = errors.join("\n");
            self.last_error = Some(joined.clone());
            let entities = self.world.borrow().entities.len();
            EvalReport {
                generation,
                ok: false,
                error: Some(joined),
                entities,
            }
        }
    }

    fn register_handle(&mut self, cx: &mut Cx) {
        let ctx = self.ctx();
        let verbs = self.verbs.clone();
        let world = self.world.clone();
        cx.with_script_vm_id(self.vm_id, |vm| {
            let game_type = vm.new_handle_type(id_lut!(game));
            vm.set_handle_call(game_type, move |vm, args, method| {
                if let Some(verb) = verbs.get(&method) {
                    return verb(vm, &ctx, args);
                }
                // Hard-fail: a silently ignored verb costs whole agent test
                // cycles. Location + did-you-mean, like the VM's own errors.
                let name = format!("{}", method);
                let loc = vm
                    .bx
                    .code
                    .ip_to_loc(vm.bx.threads.cur_ref().trap.ip)
                    .map(|l| format!("{l}: "))
                    .unwrap_or_default();
                let msg = match suggest(&name) {
                    Some(s) => format!("{loc}unknown game verb '{name}'. Did you mean '{s}'?"),
                    None => format!("{loc}unknown game verb '{name}' — game.api() lists every verb"),
                };
                world.borrow_mut().log(msg.clone());
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

    /// One 60Hz step: script on_tick, timers, physics, touch events.
    pub fn tick(&mut self, cx: &mut Cx, dt: f32) {
        let dt64 = dt as f64;
        // ONE cumulative budget for every script call this tick (M0r).
        let mut budget = TICK_INSTRUCTION_LIMIT;

        let on_tick = self.world.borrow().on_tick;
        if let Some(slot) = on_tick {
            // NIL in a positional slot is the host's "build the input object
            // here" marker, same convention gamemaker uses.
            self.call(cx, slot, &mut budget, move |_vm, _world| {
                vec![ScriptValue::from_f64(dt64), NIL]
            });
        }

        // Timers are tick-based; repeats re-arm, fired one-shots are gone.
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
        for timer in due {
            self.call(cx, timer.func, &mut budget, |_, _| vec![]);
            if timer.interval_ticks == 0 {
                self.callbacks.borrow_mut().free(timer.func);
            }
        }

        {
            let mut world = self.world.borrow_mut();
            self.blocks.borrow_mut().pre_step(&mut world);
            step_world(&mut world);
            self.blocks.borrow_mut().post_step(&mut world);
            world.tick += 1;
            world.time += dt64;
            world.pressed.clear();
        }

        let touches = collect_touches(&self.world.borrow());
        let on_touch = self.world.borrow().on_touch;
        if let Some(slot) = on_touch {
            for (a, b) in touches {
                self.call(cx, slot, &mut budget, move |_, _| {
                    vec![ScriptValue::from_f64(a as f64), ScriptValue::from_f64(b as f64)]
                });
            }
        }
    }

    fn call(
        &mut self,
        cx: &mut Cx,
        slot: CallbackSlot,
        budget: &mut usize,
        args: impl FnOnce(&mut ScriptVm, &Rc<RefCell<GameWorld>>) -> Vec<ScriptValue>,
    ) {
        if *budget == 0 {
            return;
        }
        let Some(func) = self.callbacks.borrow().get(slot) else {
            return;
        };
        let world = self.world.clone();
        let allow = *budget;
        let (errors, used) = cx.with_script_vm_id(self.vm_id, |vm| {
            let values = args(vm, &world);
            let args_obj = vm.bx.heap.new_object();
            vm.bx.heap.set_object_storage_vec2(args_obj);
            vm.bx.heap.clear_object_deep(args_obj);
            // A NIL positional slot is the host's "build the input snapshot
            // here" marker: the object must be created inside the isolate, so
            // it cannot be prepared by the caller. Pushing the marker through
            // unresolved is what left `input` nil in every on_tick.
            let mut input = None;
            for value in values {
                let value = if value.is_nil() {
                    let obj = crate::input::build_input_object(vm, &world.borrow());
                    input = Some(obj);
                    obj
                } else {
                    value
                };
                vm.bx.heap.vec_push_unchecked(args_obj, NIL, value);
            }
            vm.bx.captured_errors = Some(Vec::new());
            let _ = vm.with_instruction_limit(allow, |vm| {
                vm.call_with_args_object_with_me(func.as_object().into(), args_obj, NIL)
            });
            vm.release_transient(args_obj.into());
            if let Some(input) = input {
                vm.release_transient(input);
            }
            (vm.take_errors(), vm.last_limit_consumed())
        });
        *budget = budget.saturating_sub(used);
        if !errors.is_empty() {
            self.last_error = Some(errors.join("\n"));
        }
    }
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new()
    }
}
