# Splash UI-Thread Isolate Changes

## Target

Run each inline `Splash` widget in its own script VM while still creating and mutating native widgets on the UI thread.

The MVP is not a worker-thread sandbox. It is UI-thread VM isolation with sampled wall-clock guards so Splash scripts cannot monopolize the UI loop indefinitely.

## VM Identity

Use one VM id type everywhere:

```rust
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct SplashVmId(u64);

const MAIN_SPLASH_VM_ID: SplashVmId = SplashVmId(0);
```

Rules:

- `SplashVmId(0)` is the existing app VM.
- New Splash isolates allocate ids from `1` upward.
- Widget/script routing carries `SplashVmId`, not a `ScriptVmOwner` enum.
- Unknown app widgets default to `MAIN_SPLASH_VM_ID`.

## Cx Storage

Add isolated VM storage under `CxScriptData`:

```rust
struct IsolatedScriptVms {
    next_id: u64,
    vms: HashMap<SplashVmId, IsolatedSplashVm>,
    widget_owners: HashMap<WidgetUid, SplashVmId>,
}

struct IsolatedSplashVm {
    std: ScriptStd,
    vm: Option<Box<ScriptVmBase>>,
    initialized: bool,
    generation: u64,
    wake_pending: bool,
    consecutive_over_budget: u32,
}
```

Add helpers:

- `Cx::with_script_vm_id(vm_id, f)`: routes id `0` to the app VM, non-zero ids to isolated VMs.
- `Cx::with_splash_vm(id, f)`: non-zero isolate helper.
- subtree owner registration/unregistration for widgets produced by a Splash view.

## Splash Eval

Change `Splash::eval_body` from app-VM eval to isolate eval:

```text
Splash::set_text
  -> allocate or reuse SplashVmId
  -> initialize restricted Splash VM once
  -> eval_with_append_source under wall-clock budget
  -> View::script_from_value in the same VM
  -> register produced widget subtree with that SplashVmId
  -> replace the native View
  -> redraw
```

The restricted Splash VM registers the widget/runtime modules needed for normal widget DSL, but does not inherit host app script globals. Filesystem, process spawning, and arbitrary network remain disabled by default.

## Widget Message Routing

Make widget async routing VM-id-aware.

Update widget-to-script requests:

```rust
struct WidgetToScriptCallRequest {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    // existing callback/function/arg fields
}
```

Dispatch:

- `vm_id == MAIN_SPLASH_VM_ID`: use existing app `cx.with_vm`.
- non-zero `vm_id`: use `cx.with_splash_vm`.

Update script-to-widget async calls the same way:

- pending returns keyed by `(SplashVmId, ScriptThreadId)`;
- `ui` handle injection is per VM;
- `View.render()` callbacks and returned objects apply in the same VM that requested them.

Move `TextInput` callback execution onto the same VM-id-aware queue instead of directly calling `cx.with_vm`.

## UI Loop Pumping

Use the existing UI service points:

- after `Cx::call_event_handler`, pump app tasks, then pump runnable Splash isolates;
- on `Event::Signal`, pump isolates with `wake_pending` or queued work;
- on `TimerEvent`, route script timers by `SplashVmId`;
- on network responses, only route to Splash isolates if network is explicitly enabled.

Scheduling policy:

- round-robin non-zero Splash ids;
- per-isolate soft wall-clock slice;
- aggregate per-frame Splash wall-clock cap;
- one wake request per isolate while work is pending, using `wake_pending` to avoid signal storms.

## Tasks And Async

Each isolate owns its own `ScriptStd`, including `ScriptTasks`.

Add a budgeted task pump:

```rust
fn handle_script_tasks_budgeted(
    host: &mut dyn Any,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    budget: &mut ScriptRunBudget,
) -> ScriptPumpOutcome;
```

Stop pumping when:

- no work progresses;
- the isolate time slice is exhausted;
- the frame-wide Splash time budget is exhausted;
- the isolate hits hard overrun policy.

Do not discard unfinished work on a soft time yield. Leave pending resumes, task queues, and pump-hook work in that isolate's `ScriptStd`.

`widget_async` task hooks must be installed per `ScriptStd` or per `SplashVmId`, not behind one global app-only boolean.

## Timers

Make script timers VM-id-aware:

```rust
struct CxScriptTimer {
    vm_id: SplashVmId,
    id: LiveId,
    repeat: bool,
    timer: Timer,
    callback: ScriptFnRef,
}
```

`std.start_timeout`, `std.start_interval`, and `std.stop_timer` operate on the current VM id. `stop_timer` only removes timers owned by that VM id.

Timer callbacks run through `Cx::with_script_vm_id` under the timer callback wall-clock budget. On repeated interval overruns, stop the timer and report an isolate diagnostic.

## Sampled Wall-Clock Guard

Add a wall-clock budget checked inside `ScriptVm::run_core`, but only sample time every N instructions:

```rust
pub struct ScriptRunBudget {
    pub soft_deadline: Instant,
    pub hard_deadline: Instant,
    pub sample_interval_instructions: u32,
    pub instructions_until_sample: u32,
    pub reason: ScriptBudgetReason,
}
```

Deadline meaning:

- `soft_deadline`: cooperative yield point. The current script thread stays resumable, the isolate is queued for a later signal/frame, and this is not treated as an error.
- `hard_deadline`: runaway failure point. The current script execution is bailed, pending work for that isolate generation is cleared, and a diagnostic is reported.

Interpreter loop behavior:

```text
each opcode/direct value
  -> instructions_until_sample -= 1
  -> if zero:
       instructions_until_sample = sample_interval_instructions
       now = monotonic_time()
       if now >= hard_deadline: hard time-budget failure
       if now >= soft_deadline: soft time-budget yield
```

Do not reuse `ScriptTrapOn::Pause` for budget yield. Add a distinct outcome/trap, for example `TimeBudgetYield`, so async pauses still mean "waiting for external work".

Soft time yield:

- preserve thread state and instruction pointer;
- queue the isolate for a later signal/frame;
- return control to the UI loop without treating it as an error.

Hard time failure:

- bail the current thread;
- clear pending timers/tasks/callbacks for that isolate generation;
- keep the previous valid Splash view if possible;
- show/log a diagnostic.

Native Rust calls cannot be preempted mid-call. For Splash VMs, sample immediately before and after native calls so execution stops after an overlong native call returns.

## Implementation Order

1. Add `SplashVmId`, id `0` main routing, and isolated VM registry storage.
2. Add `Cx::with_script_vm_id` and isolate initialization.
3. Add sampled wall-clock budget support to VM entry points and `run_core`.
4. Move `Splash::eval_body` to isolated VM eval and subtree owner registration.
5. Make widget async requests, returns, `ui`, `View.render()`, and `TextInput` VM-id-aware.
6. Make task hooks and task pumping per VM id and budgeted.
7. Make script timers VM-id-aware and budgeted.
8. Add isolate pumping to event, signal, timer, and optional network paths.
9. Add stale subtree unregistering and isolate generation cleanup.

## Tests

- Two Splash widgets keep independent state and callbacks.
- Splash callback updates only its own subtree.
- `TextInput.on_return` runs in the owning Splash VM.
- `View.render()` callback and result apply in the owning Splash VM.
- Infinite loop yields or is killed by wall-clock policy while the app UI remains responsive.
- Requeueing task is limited by per-isolate pump budget.
- Interval callback cannot run in app VM and is stopped after repeated overruns.
- `stop_timer` from one VM cannot cancel another VM's timer.
- Replacing Splash body invalidates stale callbacks from the previous generation.
