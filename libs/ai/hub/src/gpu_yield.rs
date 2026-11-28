//! The UI comes first on a shared GPU.
//!
//! The window manager (the desktop session's owner) asks the hub for GPU
//! memory when its HAL runs short (`POST /control/yield {"mb", "unload",
//! "reserve_mb"}`). The hub gives it back in the order that costs the user
//! least, and reports what it freed:
//!
//! 1. shrink the chat model's context: its attention KV cache is the part of
//!    the model that scales, so a shorter window frees memory and keeps the
//!    model answering (the session is rebuilt at the smaller window);
//! 2. drop idle side models (speech, vision towers, image models) that load
//!    again on their next request;
//! 3. only when asked to (`"unload": true`, the WM's last step) and still
//!    short: unload the chat model. It loads again lazily on the next chat
//!    request, and only when the UI's reserve stays free.
//!
//! The reserve the WM names is kept for every later load: a model (or a
//! context) that would leave less than the reserve free is not loaded — the
//! request is answered "not enough GPU memory, UI has priority" instead.
//!
//! Everything deciding is plain arithmetic over [`ChatFacts`] / [`Resident`]
//! so it is tested without a GPU; `server.rs` measures and executes.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// Context windows are cut in whole steps of this many tokens, so a run of
/// small requests does not rebuild the session for every few MB.
pub const CONTEXT_STEP: u32 = 4096;

/// A yield never cuts the chat model's window under this: a conversation with
/// a system prompt and a few turns needs it. Short of more, the model goes.
pub const YIELD_FLOOR: u32 = 8192;

/// Kept free on top of the UI's reserve when the hub sizes or cuts its model,
/// so the free memory does not sit exactly on the reserve and every small
/// allocation of the desktop makes it short again.
pub const HEADROOM_MB: u64 = 512;

/// Additional free memory required for relief to grow by even one context step.
pub const RELIEF_HYSTERESIS_MB: u64 = 512;

/// A load's cost beyond the model's arena is never counted above this (a CUDA
/// context and cuBLAS workspace are a few hundred MB).
pub const LOAD_OVERHEAD_MAX_MB: u64 = 1024;

/// After a yield, the chat model does not grow back for this long: a desktop
/// starting or switching is short for seconds, then relieved; growing back at
/// once rebuilt the model twice in four seconds on a node.
pub const RELIEF_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

static LAST_YIELD: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);

/// A yield gave something: relief waits [`RELIEF_COOLDOWN`] from now, and a
/// relief already waiting is superseded.
pub fn note_yield() {
    let mut last = LAST_YIELD.lock().unwrap();
    let epoch = last.map_or(1, |(_, e)| e + 1);
    *last = Some((std::time::Instant::now(), epoch));
}

/// When a relief asked for now may run, and the epoch it belongs to (a
/// later yield changes it: that relief is dropped).
pub fn relief_schedule() -> (std::time::Instant, u64) {
    let now = std::time::Instant::now();
    match *LAST_YIELD.lock().unwrap() {
        Some((at, epoch)) => ((at + RELIEF_COOLDOWN).max(now), epoch),
        None => (now, 0),
    }
}

pub fn relief_current(epoch: u64) -> bool {
    LAST_YIELD.lock().unwrap().map_or(0, |(_, e)| e) == epoch
}

/// A load the GPU budget refused for the UI's sake (`not_enough_message`).
pub fn is_ui_priority_refusal(error: &crate::error::AssetAiError) -> bool {
    error.to_string().contains("UI has priority")
}

/// The least a chat model needs to load at all: its fixed part and the floor
/// window ([`YIELD_FLOOR`]).
pub fn smallest_load_mb(memory: &LlmMemory, unified: bool) -> u64 {
    memory.fixed_mb_for(unified) + u64::from(YIELD_FLOOR) * memory.bytes_per_lane_token() / MB
}

const MB: u64 = 1 << 20;
pub const DEVICE_RESERVE_DEFAULT_MB: u64 = 1024;

pub fn device_reserve_mb() -> u64 {
    std::env::var("MAKEPAD_LLM_VRAM_RESERVE_MB").ok()
        .and_then(|s| s.trim().parse().ok()).unwrap_or(DEVICE_RESERVE_DEFAULT_MB)
}

pub fn effective_reserve_mb(unified: bool) -> u64 {
    if unified { ui_reserve_mb() } else { ui_reserve_mb().max(device_reserve_mb()) }
}

static LOG_SINK: Mutex<Option<fn(&str)>> = Mutex::new(None);

pub fn set_log_sink(sink: fn(&str)) {
    *LOG_SINK.lock().unwrap() = Some(sink);
}

pub(crate) fn emit_log(line: &str) {
    let sink = *LOG_SINK.lock().unwrap();
    if let Some(sink) = sink { sink(line); } else { eprintln!("{line}"); }
}

static UI_RESERVE: Mutex<UiReserveState> = Mutex::new(UiReserveState::new());
/// The largest window the chat session may be built with (0 = no ceiling).
static CONTEXT_CEILING: AtomicU32 = AtomicU32::new(0);
/// Each chat model's own accounting, noted when it loads; the last noted last.
static LLM_MEMORY: Mutex<Vec<LlmMemory>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UiReserveState {
    env_mb: Option<u64>,
    runtime_mb: u64,
    suppressed: bool,
}

fn parse_ui_reserve_env(env: Option<&str>) -> u64 {
    env.and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

impl UiReserveState {
    const fn new() -> Self {
        Self { env_mb: None, runtime_mb: 0, suppressed: false }
    }

    fn effective(&self) -> u64 {
        if self.suppressed { 0 } else { self.runtime_mb.max(self.env_mb.unwrap_or(0)) }
    }

    fn apply_local_use(&mut self, ai_first: Option<bool>, floor_mb: Option<u64>) -> (u64, u64) {
        let prev = self.effective();
        if let Some(suppressed) = ai_first {
            self.suppressed = suppressed;
        }
        if let Some(floor) = floor_mb {
            self.runtime_mb = if floor == 0 { 0 } else { self.runtime_mb.max(floor) };
        }
        (prev, self.effective())
    }

    fn mirror(&self) {
        #[cfg(feature = "llm")]
        makepad_ai_llm::set_ui_reserve_bytes(self.effective().saturating_mul(MB));
    }
}

fn ui_reserve() -> std::sync::MutexGuard<'static, UiReserveState> {
    let mut state = UI_RESERVE.lock().unwrap();
    if state.env_mb.is_none() {
        state.env_mb = Some(parse_ui_reserve_env(std::env::var("MAKEPAD_AI_UI_RESERVE_MB").ok().as_deref()));
        state.mirror();
    }
    state
}

/// The larger of the operator and runtime reserves, suppressed under AI First.
pub fn ui_reserve_mb() -> u64 {
    ui_reserve().effective()
}

/// A yield updates the runtime reserve even while AI First suppresses it.
pub fn set_ui_reserve_mb(mb: u64) {
    let mut state = ui_reserve();
    let prev = state.effective();
    state.runtime_mb = mb;
    let next = state.effective();
    state.mirror();
    drop(state);
    if next != prev {
        crate::control::changed();
    }
}

/// Only an explicit mode change can suppress or restore the reserve.
pub fn apply_local_use_reserve(ai_first: Option<bool>, floor_mb: Option<u64>) -> (u64, u64) {
    let mut state = ui_reserve();
    let (prev, next) = state.apply_local_use(ai_first, floor_mb);
    state.mirror();
    drop(state);
    if next != prev {
        crate::control::changed();
    }
    (prev, next)
}

#[cfg(test)]
pub(crate) struct UiReserveGuardState(UiReserveState);

#[cfg(test)]
pub(crate) fn set_ui_reserve_state_for_test(env_mb: u64, runtime_mb: u64, suppressed: bool) -> UiReserveGuardState {
    let mut state = UI_RESERVE.lock().unwrap();
    let prev = std::mem::replace(&mut *state, UiReserveState { env_mb: Some(env_mb), runtime_mb, suppressed });
    state.mirror();
    UiReserveGuardState(prev)
}

#[cfg(test)]
pub(crate) fn restore_ui_reserve_state_for_test(prev: UiReserveGuardState) {
    let mut state = UI_RESERVE.lock().unwrap();
    *state = prev.0;
    state.mirror();
}

/// The admission reserve a load uses: the hub's own slack or the UI's
/// reserve, whichever is larger.
pub fn admission_reserve_mb(own_mb: u64) -> u64 {
    own_mb.max(ui_reserve_mb())
}

pub fn context_ceiling() -> Option<u32> {
    match CONTEXT_CEILING.load(Ordering::Relaxed) {
        0 => None,
        c => Some(c),
    }
}

pub fn set_context_ceiling(context: Option<u32>) {
    CONTEXT_CEILING.store(context.unwrap_or(0), Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeilingHold {
    None,
    Yield(u32),
    Relief(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettledCeiling {
    pub window: u32,
    pub source: Option<FitSource>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReliefState {
    pub last_grown_to: Option<u32>,
    pub extra_hysteresis_mb: u64,
}

impl ReliefState {
    pub fn hysteresis_mb(&self) -> u64 {
        RELIEF_HYSTERESIS_MB.saturating_add(self.extra_hysteresis_mb)
    }

    pub fn note_yield_shrink(&mut self, from: u32, to: u32, frees_mb: u64) {
        if to < from && self.last_grown_to.take().is_some() {
            self.extra_hysteresis_mb = self.extra_hysteresis_mb.saturating_add(frees_mb.max(RELIEF_HYSTERESIS_MB));
        }
    }

    pub fn note_relief_grow(&mut self, to: u32) {
        self.last_grown_to = Some(to);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

static HOLD: Mutex<CeilingHold> = Mutex::new(CeilingHold::None);
static RELIEF_STATE: Mutex<ReliefState> = Mutex::new(ReliefState { last_grown_to: None, extra_hysteresis_mb: 0 });

pub fn relief_state_snapshot() -> ReliefState {
    *RELIEF_STATE.lock().unwrap()
}

pub fn restore_relief_state(state: ReliefState) {
    *RELIEF_STATE.lock().unwrap() = state;
}

pub fn relief_hysteresis_mb() -> u64 {
    relief_state_snapshot().hysteresis_mb()
}

pub fn note_yield_shrink(from: u32, to: u32, frees_mb: u64) {
    RELIEF_STATE.lock().unwrap().note_yield_shrink(from, to, frees_mb);
}

pub fn reset_relief_state() {
    RELIEF_STATE.lock().unwrap().reset();
}

pub fn clear_pending_hold() {
    *HOLD.lock().unwrap() = CeilingHold::None;
}

/// A yield's cut holds for the reload it queues.
pub fn hold_ceiling(context: u32) {
    let mut hold = HOLD.lock().unwrap();
    set_context_ceiling(Some(context));
    *hold = CeilingHold::Yield(context);
}

/// The next load keeps the cushion used to compute the relief target.
pub fn hold_relief_target(to: u32, configured: u32) {
    let mut hold = HOLD.lock().unwrap();
    set_context_ceiling((to < configured).then_some(to));
    *hold = CeilingHold::Relief(to);
    RELIEF_STATE.lock().unwrap().note_relief_grow(to);
}

pub fn settle_ceiling_detailed(fit: u32, configured: u32) -> SettledCeiling {
    let mut hold = HOLD.lock().unwrap();
    let fit = fit.min(configured);
    let (window, source) = match *hold {
        CeilingHold::Yield(to) if to <= fit => (to, Some(FitSource::YieldHold)),
        CeilingHold::Relief(to) if to <= fit => (to, Some(FitSource::Relief)),
        _ => (fit, None),
    };
    *hold = CeilingHold::None;
    set_context_ceiling((window < configured).then_some(window));
    SettledCeiling { window, source }
}

pub fn settle_ceiling(fit: u32, configured: u32) -> u32 {
    settle_ceiling_detailed(fit, configured).window
}

/// The window a chat session built now gets: what was configured, under the
/// ceiling a yield (or the budget at load) set.
pub fn capped_context(configured: u32) -> u32 {
    context_ceiling().map_or(configured, |c| configured.min(c))
}

/// A loaded chat session's device memory by purpose, from its own arena
/// accounting (`makepad_ai_llm::SessionMemory`) plus what the load cost the
/// card as a whole (driver context, cuBLAS workspace, allocator slack).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LlmMemory {
    pub model: String,
    pub model_max_context: u32,
    pub lanes: u32,
    pub context: u32,
    pub kv: String,
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub kv_bytes_per_token: u64,
    pub mask_bytes_per_token: u64,
    pub state_mb: u64,
    pub activations_mb: u64,
    /// Persistent runtime scratch outside the arena, in MiB.
    pub runtime_scratch_mb: u64,
    /// Free memory before the load minus after it (None without NVML).
    pub load_cost_mb: Option<u64>,
    pub measured: bool,
}

impl LlmMemory {
    #[cfg(feature = "llm")]
    pub fn from_plan(model: String, kv: String, plan: &makepad_ai_llm::SessionMemoryPlan) -> Self {
        let mb = |bytes: usize| (bytes as u64).div_ceil(MB);
        Self {
            model, kv, lanes: plan.lanes.max(1), model_max_context: plan.model_max_context,
            weights_mb: mb(plan.weights), state_mb: mb(plan.state), activations_mb: mb(plan.activations_base),
            runtime_scratch_mb: mb(plan.runtime_scratch),
            kv_bytes_per_token: plan.attention_kv_per_token as u64,
            mask_bytes_per_token: plan.mask_bytes_per_token as u64,
            ..Default::default()
        }
    }

    pub fn bytes_per_lane_token(&self) -> u64 {
        u64::from(self.lanes.max(1)) * (self.kv_bytes_per_token + self.mask_bytes_per_token)
    }

    pub fn mask_mb(&self) -> u64 {
        (u64::from(self.context) * u64::from(self.lanes.max(1)) * self.mask_bytes_per_token).div_ceil(MB)
    }

    pub fn arena_mb(&self) -> u64 {
        self.weights_mb + self.kv_mb + self.state_mb + self.activations_mb
    }

    pub fn accounted_mb(&self) -> u64 {
        self.arena_mb().saturating_add(self.runtime_scratch_mb)
    }

    pub fn overhead_mb_for(&self, unified: bool) -> u64 {
        if !self.measured && !unified {
            return LOAD_OVERHEAD_MAX_MB;
        }
        self.load_cost_mb.map_or(0, |c| c.saturating_sub(self.accounted_mb()).min(LOAD_OVERHEAD_MAX_MB))
    }

    pub fn fixed_mb_for(&self, unified: bool) -> u64 {
        let mask = if self.measured { self.mask_mb() } else { 0 };
        self.weights_mb + self.state_mb + self.activations_mb.saturating_sub(mask) + self.runtime_scratch_mb + self.overhead_mb_for(unified)
    }

    fn matches(&self, model: &str, kv: &str, lanes: u32) -> bool {
        self.model == model && self.kv == kv && self.lanes.max(1) == lanes.max(1)
    }

    pub fn line(&self) -> String {
        format!(
            "{} memory: weights {} MB, KV {} MB ({} tokens {}, {} KiB/token), state {} MB, activations {} MB{}{}",
            self.model,
            self.weights_mb,
            self.kv_mb,
            self.context,
            self.kv,
            self.kv_bytes_per_token / 1024,
            self.state_mb,
            self.activations_mb,
            if self.runtime_scratch_mb > 0 { format!(", scratch {} MB", self.runtime_scratch_mb) } else { String::new() },
            self.load_cost_mb.map_or(String::new(), |c| format!(", whole load {c} MB")),
        )
    }
}

pub fn note_llm_memory(mut memory: LlmMemory) -> LlmMemory {
    let mut all = LLM_MEMORY.lock().unwrap();
    if let Some(index) = all.iter().position(|m| m.matches(&memory.model, &memory.kv, memory.lanes)) {
        let prev = all.remove(index);
        if let (true, Some(prev_cost), Some(curr_cost)) = (prev.measured, prev.load_cost_mb, memory.load_cost_mb) {
            // Filter external allocations against the arena. A lazy backend's
            // planned scratch may still be absent from both measurements.
            let extra = |m: &LlmMemory, cost: u64| {
                cost.saturating_sub(m.arena_mb()).min(m.runtime_scratch_mb.saturating_add(LOAD_OVERHEAD_MAX_MB))
            };
            let shared_extra = extra(&prev, prev_cost).min(extra(&memory, curr_cost));
            memory.load_cost_mb = Some(curr_cost.min(memory.arena_mb().saturating_add(shared_extra)));
        }
    }
    all.push(memory.clone());
    drop(all);
    emit_log(&format!("[gpu-yield] {}", memory.line()));
    crate::control::changed();
    memory
}

pub fn note_llm_plan(plan: LlmMemory) {
    let mut all = LLM_MEMORY.lock().unwrap();
    if !all.iter().any(|m| m.matches(&plan.model, &plan.kv, plan.lanes)) {
        all.push(plan);
    }
}

/// The most recently loaded chat model.
pub fn llm_memory() -> Option<LlmMemory> {
    LLM_MEMORY.lock().unwrap().iter().rev().find(|m| m.measured).cloned()
}

pub fn llm_memory_of(model: &str) -> Option<LlmMemory> {
    LLM_MEMORY.lock().unwrap().iter().rev().find(|m| m.model == model).cloned()
}

pub fn llm_memory_for(model: &str, kv: &str, lanes: u32) -> Option<LlmMemory> {
    LLM_MEMORY.lock().unwrap().iter().find(|m| m.matches(model, kv, lanes)).cloned()
}

pub fn context_fit(memory: &LlmMemory, free_mb: u64, released_mb: u64, unified: bool, min_context: u32) -> Option<u32> {
    fit_context_detailed(memory, Some(free_mb.saturating_add(released_mb)), effective_reserve_mb(unified), unified, None, min_context)
        .ok().map(|d| d.chosen)
}

pub fn context_options(model_max: Option<u32>, fit: Option<u32>, min_context: u32) -> Vec<u32> {
    const RUNGS: &[u32] = &[16_384, 32_768, 49_152, 65_536, 98_304, 131_072, 196_608, 262_144, 524_288, 1_048_576];
    let Some(max) = model_max else { return Vec::new() };
    let mut options: Vec<_> = RUNGS.iter().copied().chain([max]).chain(fit)
        .filter(|n| (min_context..=max).contains(n)).collect();
    options.sort_unstable();
    options.dedup();
    options
}

/// The chat model as the planner sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatFacts {
    pub model: String,
    pub context: u32,
    pub min_context: u32,
    pub kv_bytes_per_token: u64,
    /// What the model holds on the card in all.
    pub resident_mb: u64,
}

/// A resident side model (not the chat model) and what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Resident {
    pub model: String,
    pub mb: u64,
    /// A job runs on it now: it is not dropped.
    pub busy: bool,
    /// A client holds it (`crate::residency::Holds`): it goes after the
    /// others and after a context cut.
    pub held: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    ShrinkContext { model: String, from: u32, to: u32, frees_mb: u64 },
    Unload { model: String, frees_mb: u64 },
    /// A side model a client holds: after the others and a context cut.
    UnloadHeld { model: String, frees_mb: u64 },
    UnloadChat { model: String, frees_mb: u64 },
}

impl Step {
    pub fn frees_mb(&self) -> u64 {
        match self {
            Step::ShrinkContext { frees_mb, .. }
            | Step::Unload { frees_mb, .. }
            | Step::UnloadHeld { frees_mb, .. }
            | Step::UnloadChat { frees_mb, .. } => *frees_mb,
        }
    }

    pub fn line(&self) -> String {
        match self {
            Step::ShrinkContext { model, from, to, frees_mb } => format!("{model} context {from} -> {to} (~{frees_mb} MB)"),
            Step::Unload { model, frees_mb } => format!("unload idle {model} (~{frees_mb} MB)"),
            Step::UnloadHeld { model, frees_mb } => format!("unload held {model} (~{frees_mb} MB)"),
            Step::UnloadChat { model, frees_mb } => format!("unload chat model {model} (~{frees_mb} MB)"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub frees_mb: u64,
    /// MB still missing after every step allowed (0 = the request is met).
    pub short_mb: u64,
}

/// The cheapest steps that free `wanted_mb`: idle side models first (largest
/// first; they load again on their next request and the chat model is not
/// touched), then ONE context cut that frees what is still missing plus
/// [`HEADROOM_MB`] (so a second cut right after is not needed; never under
/// [`YIELD_FLOOR`]), then idle side models a client holds, and the chat model
/// itself only when `allow_unload` and still short.
pub fn plan(chat: Option<&ChatFacts>, residents: &[Resident], wanted_mb: u64, allow_unload: bool) -> Plan {
    let mut out = Plan::default();
    if wanted_mb == 0 {
        return out;
    }
    let mut idle: Vec<&Resident> = residents.iter().filter(|r| !r.busy && r.mb > 0).collect();
    idle.sort_by(|a, b| b.mb.cmp(&a.mb).then_with(|| a.model.cmp(&b.model)));
    let (held, idle): (Vec<&Resident>, Vec<&Resident>) = idle.into_iter().partition(|r| r.held);
    for r in idle {
        if out.frees_mb >= wanted_mb {
            break;
        }
        out.steps.push(Step::Unload { model: r.model.clone(), frees_mb: r.mb });
        out.frees_mb += r.mb;
    }
    if out.frees_mb < wanted_mb {
        if let Some(chat) = chat.filter(|c| c.kv_bytes_per_token > 0) {
            let missing = wanted_mb - out.frees_mb + HEADROOM_MB;
            let tokens = (missing * MB).div_ceil(chat.kv_bytes_per_token);
            let floor = chat.min_context.max(YIELD_FLOOR).min(chat.context);
            let to = (u64::from(chat.context).saturating_sub(tokens) as u32 / CONTEXT_STEP * CONTEXT_STEP).max(floor);
            if to < chat.context {
                let frees_mb = u64::from(chat.context - to) * chat.kv_bytes_per_token / MB;
                out.steps.push(Step::ShrinkContext { model: chat.model.clone(), from: chat.context, to, frees_mb });
                out.frees_mb += frees_mb;
            }
        }
    }
    for r in held {
        if out.frees_mb >= wanted_mb {
            break;
        }
        out.steps.push(Step::UnloadHeld { model: r.model.clone(), frees_mb: r.mb });
        out.frees_mb += r.mb;
    }
    if out.frees_mb < wanted_mb && allow_unload {
        if let Some(chat) = chat {
            // The whole model goes: a context cut before it would only
            // rebuild a session that is dropped right after.
            if let Some(i) = out.steps.iter().position(|s| matches!(s, Step::ShrinkContext { .. })) {
                out.frees_mb -= out.steps.remove(i).frees_mb();
            }
            out.steps.push(Step::UnloadChat { model: chat.model.clone(), frees_mb: chat.resident_mb });
            out.frees_mb += chat.resident_mb;
        }
    }
    out.short_mb = wanted_mb.saturating_sub(out.frees_mb);
    out
}

/// What a yield still has to free when it runs. The UI asked for `asked_mb`
/// more than the device had free then (`free_when_asked`); what came free
/// since (`free_now`, a fresh reading: a live loop gave its kept memory
/// back, a session ended while the yield waited behind it) counts toward
/// it. Without both readings, the whole ask.
pub fn still_wanted_mb(asked_mb: u64, free_when_asked: Option<u64>, free_now: Option<u64>) -> u64 {
    match (free_when_asked, free_now) {
        (Some(then), Some(now)) => (then + asked_mb).saturating_sub(now),
        _ => asked_mb,
    }
}

/// A yield's report: what the UI asked for, what it still needed when the
/// yield ran (`wanted_mb`, [`still_wanted_mb`]), the steps done, what they
/// freed and what is free now. The memory being free already is a plain yes.
pub fn yield_report(asked_mb: u64, allow_unload: bool, wanted_mb: u64, done: &[String], freed_mb: Option<u64>, free_now_mb: Option<u64>, short_mb: u64) -> String {
    let mb = |v: Option<u64>| v.map_or("?".to_string(), |v| v.to_string());
    let asked = format!("yield {asked_mb} MB for the UI{}", if allow_unload { " (unload allowed)" } else { "" });
    if wanted_mb == 0 {
        return format!("{asked}: free already, {} MB free now; nothing to give", mb(free_now_mb));
    }
    let came_free = if wanted_mb < asked_mb { format!("{} MB came free since it was asked; ", asked_mb - wanted_mb) } else { String::new() };
    let mut report = format!(
        "{asked}: {came_free}{}; freed {} MB, {} MB free now",
        if done.is_empty() { "nothing to give".to_string() } else { done.join(", ") },
        mb(freed_mb),
        mb(free_now_mb),
    );
    if short_mb > 0 {
        report.push_str(&format!("; {short_mb} MB short"));
    }
    report
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitSource {
    Fit,
    Override,
    YieldHold,
    Relief,
    UnifiedFloor,
    NoVramProbe,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FitDecision {
    pub model: String,
    pub kv: String,
    pub lanes: u32,
    pub model_max: u32,
    pub override_context: Option<u32>,
    pub free_mb: Option<u64>,
    pub reserve_mb: u64,
    pub headroom_mb: u64,
    pub fixed_mb: u64,
    pub scratch_mb: u64,
    pub kv_bytes_per_token: u64,
    pub mask_bytes_per_token: u64,
    pub bytes_per_lane_token: u64,
    pub chosen: u32,
    pub source: FitSource,
}

impl FitDecision {
    pub fn log_line(&self) -> String {
        let source = match self.source {
            FitSource::Fit => "fit",
            FitSource::Override => "override",
            FitSource::YieldHold => "yield-hold",
            FitSource::Relief => "relief",
            FitSource::UnifiedFloor => "unified-floor",
            FitSource::NoVramProbe => "no-vram-probe",
        };
        let free = self.free_mb.map_or_else(|| "unknown".into(), |mb| format!("{mb} MB"));
        format!("[llm-context] {} ({}, {} {}): window {} (source={source}; model_max={}, free={free}, reserve={} MB + {} MB headroom, fixed={} MB [scratch={} MB], per_token={} B [kv={} + mask={}])",
            self.model, self.kv, self.lanes, if self.lanes == 1 { "lane" } else { "lanes" },
            self.chosen, self.model_max, self.reserve_mb, self.headroom_mb, self.fixed_mb, self.scratch_mb,
            self.bytes_per_lane_token, self.kv_bytes_per_token, self.mask_bytes_per_token)
    }
}

pub fn fit_context_detailed(
    memory: &LlmMemory, free_mb: Option<u64>, reserve_mb: u64, unified: bool,
    explicit_override: Option<u32>, min_context: u32,
) -> Result<FitDecision, String> {
    let model_max = memory.model_max_context.max(min_context);
    let wanted = explicit_override.unwrap_or(model_max).clamp(min_context, model_max);
    let fixed_mb = memory.fixed_mb_for(unified);
    let per_token = memory.bytes_per_lane_token();
    let mut decision = FitDecision {
        model: memory.model.clone(), kv: memory.kv.clone(), lanes: memory.lanes.max(1),
        model_max, override_context: explicit_override, free_mb, reserve_mb, headroom_mb: HEADROOM_MB,
        fixed_mb, scratch_mb: memory.runtime_scratch_mb, kv_bytes_per_token: memory.kv_bytes_per_token, mask_bytes_per_token: memory.mask_bytes_per_token,
        bytes_per_lane_token: per_token, chosen: wanted,
        source: if explicit_override.is_some() { FitSource::Override } else { FitSource::NoVramProbe },
    };
    let Some(free) = free_mb else { return Ok(decision) };
    let kept = reserve_mb.saturating_add(HEADROOM_MB);
    let floor = min_context.max(YIELD_FLOOR.min(wanted));
    let room = free.saturating_sub(kept).saturating_sub(fixed_mb);
    let fits = room.saturating_mul(MB).checked_div(per_token).unwrap_or(u64::from(wanted));
    if fits < u64::from(wanted) {
        let fits = fits as u32;
        let stepped = fits / CONTEXT_STEP * CONTEXT_STEP;
        decision.chosen = if fits >= floor { stepped.max(floor) } else { stepped };
    }
    if decision.chosen < floor {
        if !unified {
            return Err(not_enough_message(&memory.model, free, kept, fixed_mb + u64::from(floor) * per_token / MB));
        }
        decision.chosen = min_context;
        decision.source = FitSource::UnifiedFloor;
    } else if explicit_override.is_none() || decision.chosen < wanted {
        decision.source = FitSource::Fit;
    }
    Ok(decision)
}

pub fn grow_target(memory: &LlmMemory, free_mb: u64, held_mb: u64, reserve_mb: u64, unified: bool, target_upper: u32, loaded: u32, min_context: u32) -> Option<u32> {
    grow_target_with_hysteresis(memory, free_mb, held_mb, reserve_mb, RELIEF_HYSTERESIS_MB, unified, target_upper, loaded, min_context)
}

#[allow(clippy::too_many_arguments)]
pub fn grow_target_with_hysteresis(
    memory: &LlmMemory, free_mb: u64, held_mb: u64, reserve_mb: u64, hysteresis_mb: u64,
    unified: bool, target_upper: u32, loaded: u32, min_context: u32,
) -> Option<u32> {
    let reserve_mb = reserve_mb.saturating_add(hysteresis_mb);
    let fit = fit_context_detailed(memory, Some(free_mb.saturating_add(held_mb)), reserve_mb, unified, Some(target_upper), min_context).ok()?.chosen;
    (fit >= loaded.saturating_add(CONTEXT_STEP)).then_some(fit)
}

/// Whether a model needing `need_mb` may load with `free_mb` free.
pub fn admit(model: &str, free_mb: u64, need_mb: u64, reserve_mb: u64) -> Result<(), String> {
    if free_mb >= need_mb.saturating_add(reserve_mb) {
        Ok(())
    } else {
        Err(not_enough_message(model, free_mb, reserve_mb, need_mb))
    }
}

/// The registry ids that are `model`'s quantised variant (same family, smaller), in
/// the order a job tries them: `<model>-q4`, `<model>-q4-24g`.
pub fn variant_ids(model: &str) -> [String; 2] {
    [format!("{model}-q4"), format!("{model}-q4-24g")]
}

/// Whether a job asking for a model needing `asked_mb` runs its quantised variant
/// (needing `variant_mb`) instead: when `asked` cannot run on this card at all
/// (`asked_mb` None), or does not fit in `room_mb` (free memory plus what idle
/// residents give back) next to the UI's `reserve_mb`. Unknown room (no NVML):
/// the job runs what it asked for.
pub fn use_variant(asked_mb: Option<u64>, variant_mb: u64, room_mb: Option<u64>, reserve_mb: u64) -> bool {
    match (asked_mb, room_mb) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(asked), Some(room)) => asked.saturating_add(reserve_mb) > room && variant_mb < asked,
    }
}

/// What the hub's memory policy did lately, for the panel (`GET /control` "policy").
static POLICY: Mutex<Vec<(std::time::Instant, String)>> = Mutex::new(Vec::new());

/// Say a policy decision: a variant ran for a model that did not fit, or an idle chat
/// model was unloaded for a job. Kept: the last 4.
pub fn note_policy(text: String) {
    eprintln!("[gpu-policy] {text}");
    let mut notes = POLICY.lock().unwrap();
    notes.push((std::time::Instant::now(), text));
    let n = notes.len();
    if n > 4 {
        notes.drain(..n - 4);
    }
}

/// The policy notes, newest first, with their age in ms.
pub fn policy_notes() -> Vec<(u64, String)> {
    let notes = POLICY.lock().unwrap();
    notes.iter().rev().map(|(at, text)| (at.elapsed().as_millis() as u64, text.clone())).collect()
}

pub fn not_enough_message(model: &str, free_mb: u64, reserve_mb: u64, need_mb: u64) -> String {
    format!(
        "not enough GPU memory for {model}, UI has priority: it needs {need_mb} MB, {free_mb} MB are free and the UI keeps {reserve_mb} MB"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 149 picture: Qwen3.8-27B at 49152 tokens of q8 KV (36 KiB/token),
    /// 20.4 GB resident in all.
    fn qwen() -> ChatFacts {
        ChatFacts { model: "qwen3.8-27b".into(), context: 49152, min_context: 2048, kv_bytes_per_token: 36_992, resident_mb: 20_878 }
    }

    fn side(model: &str, mb: u64, busy: bool) -> Resident {
        Resident { model: model.into(), mb, busy, held: false }
    }

    #[test]
    fn a_held_model_goes_after_the_others_and_a_context_cut() {
        let held = Resident { held: true, ..side("flux", 9000, false) };
        let p = plan(Some(&qwen()), &[held.clone(), side("whisper", 1700, false)], 1500, false);
        assert_eq!(p.steps, vec![Step::Unload { model: "whisper".into(), frees_mb: 1700 }], "an unheld model first, though smaller");
        let p = plan(Some(&qwen()), &[held.clone()], 500, false);
        assert!(matches!(p.steps.as_slice(), [Step::ShrinkContext { .. }]), "a context cut before a held model: {:?}", p.steps);
        let p = plan(None, &[held], 3000, false);
        assert_eq!(p.steps, vec![Step::UnloadHeld { model: "flux".into(), frees_mb: 9000 }], "and the held model when the UI still needs it");
        assert_eq!(p.short_mb, 0);
    }

    #[test]
    fn a_model_that_does_not_fit_next_to_the_ui_runs_its_q4_variant() {
        assert_eq!(variant_ids("flux1-schnell"), ["flux1-schnell-q4", "flux1-schnell-q4-24g"]);
        // 149: flux1-schnell 21504 MB, 21872 MB free, the UI keeps 1536 MB; the q4 needs 18432 MB
        assert!(use_variant(Some(21_504), 18_432, Some(21_872), 1_536));
        // a 96 GB card runs what was asked for
        assert!(!use_variant(Some(21_504), 18_432, Some(90_000), 1_536));
        // a model this card cannot hold at all
        assert!(use_variant(None, 18_432, Some(21_872), 1_536));
        // no NVML: no guessing
        assert!(!use_variant(Some(21_504), 18_432, None, 1_536));
    }

    #[test]
    fn policy_notes_keep_the_last_four_newest_first() {
        for i in 0..6 {
            note_policy(format!("note {i}"));
        }
        let notes: Vec<String> = policy_notes().into_iter().map(|(_, t)| t).filter(|t| t.starts_with("note ")).collect();
        assert_eq!(notes, ["note 5", "note 4", "note 3", "note 2"]);
    }

    #[test]
    fn idle_side_models_go_before_the_chat_model_is_touched() {
        // the 149 case: a ~300 MB ask with Whisper idle costs no chat reload at all
        let p = plan(Some(&qwen()), &[side("whisper", 1700, false)], 300, false);
        assert_eq!(p.steps, vec![Step::Unload { model: "whisper".into(), frees_mb: 1700 }]);
    }

    #[test]
    fn a_small_request_cuts_the_context_once_with_headroom() {
        let p = plan(Some(&qwen()), &[], 300, false);
        assert_eq!(p.steps.len(), 1, "{p:?}");
        let Step::ShrinkContext { from, to, frees_mb, .. } = &p.steps[0] else { panic!("{p:?}") };
        assert_eq!((*from, *to % CONTEXT_STEP), (49152, 0));
        // 300 asked + 512 headroom: one cut big enough that the next small ask is not short
        assert!(*frees_mb >= 300 + HEADROOM_MB && *to >= 24576, "{to} frees {frees_mb}");
        assert_eq!(p.short_mb, 0);
    }

    #[test]
    fn the_context_never_goes_under_the_floor() {
        let p = plan(Some(&qwen()), &[side("kokoro", 400, false), side("whisper", 1700, false), side("flux", 9000, true)], 3000, false);
        assert_eq!(p.steps[0], Step::Unload { model: "whisper".into(), frees_mb: 1700 });
        assert_eq!(p.steps[1], Step::Unload { model: "kokoro".into(), frees_mb: 400 });
        let Step::ShrinkContext { to, .. } = &p.steps[2] else { panic!("{p:?}") };
        assert_eq!(*to, YIELD_FLOOR, "never under 8192 (one node went to 2048)");
        assert!(!p.steps.iter().any(|s| matches!(s, Step::Unload { model, .. } if model == "flux")), "busy models stay");
        // a cut from 12288 for 307 MB stays at the floor, it does not go to 2048
        let mut small = qwen();
        small.context = 12288;
        let p = plan(Some(&small), &[], 307, false);
        assert!(matches!(&p.steps[..], [Step::ShrinkContext { to: 8192, .. }]), "{p:?}");
    }

    #[test]
    fn the_chat_model_is_unloaded_only_when_allowed_and_then_instead_of_the_cut() {
        let p = plan(Some(&qwen()), &[], 8000, false);
        assert!(p.short_mb > 0 && !p.steps.iter().any(|s| matches!(s, Step::UnloadChat { .. })));
        let p = plan(Some(&qwen()), &[side("kokoro", 400, false)], 8000, true);
        assert_eq!(p.steps, vec![
            Step::Unload { model: "kokoro".into(), frees_mb: 400 },
            Step::UnloadChat { model: "qwen3.8-27b".into(), frees_mb: 20_878 },
        ]);
        assert_eq!((p.frees_mb, p.short_mb), (21_278, 0));
    }

    #[test]
    fn a_yield_for_memory_that_is_free_already_is_a_plain_yes() {
        // The UI asked for 1024 MB with 10 350 MB free; the yield waited
        // behind a live loop, which ended, and 16 812 MB are free when it runs.
        let wanted = still_wanted_mb(1024, Some(10_350), Some(16_812));
        assert_eq!(wanted, 0, "the memory the UI asked for is free");
        let p = plan(None, &[side("flux2-klein-4b", 15_000, false)], wanted, true);
        assert_eq!(p, Plan::default(), "nothing is unloaded for memory that is free");
        let line = yield_report(1024, false, wanted, &[], Some(0), Some(16_812), p.short_mb);
        assert_eq!(line, "yield 1024 MB for the UI: free already, 16812 MB free now; nothing to give");
        // Part of it came free: only the rest is asked of the models.
        let wanted = still_wanted_mb(4000, Some(1000), Some(3000));
        assert_eq!(wanted, 2000, "what came free since counts");
        let p = plan(None, &[side("kokoro", 400, false)], wanted, false);
        let done: Vec<String> = p.steps.iter().map(Step::line).collect();
        assert_eq!(
            yield_report(4000, false, wanted, &done, Some(400), Some(3400), p.short_mb),
            "yield 4000 MB for the UI: 2000 MB came free since it was asked; unload idle kokoro (~400 MB); freed 400 MB, 3400 MB free now; 1600 MB short"
        );
        // Without a reading at either end, the whole ask stands.
        assert_eq!(still_wanted_mb(1024, None, Some(16_812)), 1024);
        assert_eq!(still_wanted_mb(1024, Some(10_350), None), 1024);
    }

    #[test]
    fn nothing_resident_reports_the_whole_shortfall() {
        let p = plan(None, &[], 1200, true);
        assert_eq!((p.steps.len(), p.frees_mb, p.short_mb), (0, 0, 1200));
        assert_eq!(plan(Some(&qwen()), &[], 0, true), Plan::default());
    }

    /// The tests that note chat-model loads: the accounting is one process-wide list and
    /// `llm_memory()` is its newest entry, so a parallel test's note would come between.
    fn llm_memory_tests() -> std::sync::MutexGuard<'static, ()> {
        crate::llm_context::test_settings()
    }

    #[test]
    fn ui_reserve_state_preserves_operator_floor_across_ai_first_and_syncs_llm_mirror() {
        assert_eq!(parse_ui_reserve_env(Some("1536")), 1536);
        assert_eq!(parse_ui_reserve_env(None), 0);
        assert_eq!(parse_ui_reserve_env(Some("bad")), 0);
        let mut state = UiReserveState { env_mb: Some(2048), ..UiReserveState::new() };
        assert_eq!(state.effective(), 2048);
        assert_eq!(state.apply_local_use(Some(false), Some(1536)), (2048, 2048));
        assert_eq!(state.apply_local_use(Some(true), None), (2048, 0));
        assert_eq!(state.apply_local_use(None, Some(3072)), (0, 0));
        assert_eq!(state.apply_local_use(Some(false), None), (0, 3072));
        assert_eq!(state.apply_local_use(Some(false), Some(0)), (3072, 2048));

        let _guard = llm_memory_tests();
        let prev = set_ui_reserve_state_for_test(2048, 0, false);
        let check = |mb| {
            assert_eq!(ui_reserve_mb(), mb);
            #[cfg(feature = "llm")]
            assert_eq!(makepad_ai_llm::ui_reserve_bytes(), mb * MB);
        };
        check(2048);
        assert_eq!(apply_local_use_reserve(Some(true), None), (2048, 0));
        check(0);
        set_ui_reserve_mb(3072);
        check(0);
        assert_eq!(apply_local_use_reserve(None, Some(1536)), (0, 0));
        check(0);
        assert_eq!(apply_local_use_reserve(Some(false), None), (0, 3072));
        check(3072);
        set_ui_reserve_mb(1536);
        check(2048);
        restore_ui_reserve_state_for_test(prev);
    }

    fn cold_27b() -> LlmMemory {
        LlmMemory {
            model: "27b".into(), model_max_context: 262_144, lanes: 1, kv: "q8".into(),
            weights_mb: 15_702, state_mb: 789, activations_mb: 512,
            kv_bytes_per_token: 36_992, mask_bytes_per_token: 1_024, ..Default::default()
        }
    }

    fn cold_9b() -> LlmMemory {
        LlmMemory {
            model: "9b".into(), weights_mb: 5_800, state_mb: 300, kv_bytes_per_token: 17_408, ..cold_27b()
        }
    }

    #[test]
    fn fit_on_24gb_gpu_27b_q8_cold_and_warm_beats_32k_and_keeps_reserve() {
        let cold = cold_27b();
        let warm = LlmMemory {
            context: 49_152, kv_mb: 1_734, activations_mb: 513, load_cost_mb: Some(19_178), measured: true, ..cold.clone()
        };
        assert_eq!((warm.overhead_mb_for(false), warm.mask_mb()), (440, 48));
        for (memory, expected) in [(&cold, 114_688), (&warm, 131_072)] {
            let d = fit_context_detailed(memory, Some(23_800), 1_024, false, None, 2048).unwrap();
            assert_eq!((d.chosen, d.source, d.chosen % CONTEXT_STEP), (expected, FitSource::Fit, 0));
            assert!(d.fixed_mb + u64::from(d.chosen) * d.bytes_per_lane_token / MB + d.reserve_mb + d.headroom_mb <= 23_800);
        }
    }

    #[test]
    fn fit_on_mac_9b_unified_reaches_full_262144_model_max() {
        for free in [180_000, 96_000] {
            let d = fit_context_detailed(&cold_9b(), Some(free), 0, true, None, 2048).unwrap();
            assert_eq!((d.chosen, d.source), (262_144, FitSource::Fit));
        }
    }

    #[test]
    fn fit_divides_across_multiple_lanes_and_caps_each_at_model_max() {
        for (mut memory, free, reserve, unified, expected) in [
            (cold_27b(), 23_800, 1_024, false, 57_344), (cold_9b(), 180_000, 0, true, 262_144)
        ] {
            memory.lanes = 2;
            assert_eq!(fit_context_detailed(&memory, Some(free), reserve, unified, None, 2048).unwrap().chosen, expected);
        }
    }

    #[test]
    fn explicit_override_caps_below_fit_but_ui_reserve_still_wins() {
        for (cap, chosen, source) in [(65_536, 65_536, FitSource::Override), (262_144, 102_400, FitSource::Fit)] {
            let d = fit_context_detailed(&cold_27b(), Some(23_800), 1_536, false, Some(cap), 2048).unwrap();
            assert_eq!((d.chosen, d.source), (chosen, source));
        }
        assert!(fit_context_detailed(&cold_27b(), Some(19_000), 1536, false, None, 2048).unwrap_err().contains("UI has priority"));
        let d = fit_context_detailed(&cold_9b(), Some(7000), 0, true, None, 2048).unwrap();
        assert_eq!((d.chosen, d.source), (2048, FitSource::UnifiedFloor));
    }

    #[test]
    fn fit_decision_log_line_names_every_input_and_chosen_window() {
        let d = fit_context_detailed(&cold_27b(), Some(23_800), 1024, false, None, 2048).unwrap();
        assert_eq!(d.log_line(), "[llm-context] 27b (q8, 1 lane): window 114688 (source=fit; model_max=262144, free=23800 MB, reserve=1024 MB + 512 MB headroom, fixed=18027 MB [scratch=0 MB], per_token=38016 B [kv=36992 + mask=1024])");
        let d = fit_context_detailed(&cold_27b(), None, 1024, false, None, 2048).unwrap();
        assert_eq!((d.chosen, d.source), (262_144, FitSource::NoVramProbe));
        assert!(d.log_line().contains("free=unknown"));
    }

    #[test]
    fn context_options_builds_dynamic_rungs_up_to_model_max_with_fit() {
        let options = context_options(Some(262_144), Some(114_688), 2048);
        assert_eq!(options, [16_384, 32_768, 49_152, 65_536, 98_304, 114_688, 131_072, 196_608, 262_144]);
        assert!(context_options(None, None, 2048).is_empty());
    }

    #[test]
    fn measured_note_does_not_inherit_cold_plan_overhead() {
        let _guard = llm_memory_tests();
        let cold = cold_27b();
        note_llm_plan(cold.clone());
        note_llm_memory(LlmMemory { measured: true, load_cost_mb: Some(cold.arena_mb() + 440), ..cold.clone() });
        note_llm_plan(cold.clone());
        note_llm_plan(LlmMemory { lanes: 2, ..cold.clone() });
        assert_eq!(llm_memory_for(&cold.model, "q8", 1).unwrap().overhead_mb_for(false), 440);
        assert!(!llm_memory_for(&cold.model, "q8", 2).unwrap().measured);
        assert!(llm_memory_for(&cold.model, "f16", 1).is_none());
    }

    fn mem() -> LlmMemory {
        LlmMemory {
            model: "qwen3.8-27b".into(),
            context: 49152,
            model_max_context: 262_144, lanes: 1, measured: true, mask_bytes_per_token: 0,
            kv: "q8".into(),
            weights_mb: 15_700,
            kv_mb: 1_734,
            kv_bytes_per_token: 36_992,
            state_mb: 700,
            activations_mb: 600,
            runtime_scratch_mb: 0,
            load_cost_mb: Some(20_400),
        }
    }

    fn warm_scratch_memory() -> LlmMemory {
        LlmMemory {
            model: "scratch-chat".into(), kv: "q8".into(), lanes: 1, model_max_context: 262_144,
            context: 69_632, weights_mb: 16_315, kv_mb: 2_457, state_mb: 789, activations_mb: 513,
            runtime_scratch_mb: 321, kv_bytes_per_token: 36_992, mask_bytes_per_token: 1_024,
            load_cost_mb: Some(20_430), measured: true,
        }
    }

    #[test]
    fn runtime_scratch_enters_fixed_without_double_counting_load_cost() {
        let _guard = llm_memory_tests();
        let m = warm_scratch_memory();
        assert_eq!(m.accounted_mb(), 20_395);
        assert_eq!(m.overhead_mb_for(false), 35);
        assert_eq!(m.fixed_mb_for(false), 17_905);
        assert_eq!(m.fixed_mb_for(false) + m.kv_mb + m.mask_mb(), 20_430);
        let d = fit_context_detailed(&m, Some(22_617), 1_536, false, None, 2048).unwrap();
        assert_eq!((d.chosen, d.scratch_mb), (69_632, 321));
        assert!(d.log_line().contains("fixed=17905 MB [scratch=321 MB]"));
        assert!(m.line().contains(", scratch 321 MB, whole load 20430 MB"));
        let reserve = set_ui_reserve_state_for_test(0, 1_536, false);
        let fit = context_fit(&m, 2_187, 20_430, false, 2048);
        restore_ui_reserve_state_for_test(reserve);
        assert_eq!(fit, Some(69_632));

        let arena = m.arena_mb();
        let metal = LlmMemory {
            model: "scratch-lazy".into(), runtime_scratch_mb: 129, load_cost_mb: Some(arena + 4), ..m.clone()
        };
        assert_eq!(metal.overhead_mb_for(true), 0);
        assert_eq!(metal.fixed_mb_for(true), metal.weights_mb + metal.state_mb + metal.activations_mb - metal.mask_mb() + 129);
        for (measured, expected) in [(arena + 4, arena + 4), (arena + 4, arena + 4),
            (arena + 504, arena + 4), (arena - 50, arena - 50)] {
            let recorded = note_llm_memory(LlmMemory { load_cost_mb: Some(measured), ..metal.clone() });
            assert_eq!(recorded.load_cost_mb, Some(expected), "lazy scratch, load measured {measured}");
        }

        let cold = LlmMemory { runtime_scratch_mb: 320, ..cold_27b() };
        assert_eq!(cold.overhead_mb_for(false), LOAD_OVERHEAD_MAX_MB);
        assert_eq!(cold.fixed_mb_for(false), cold.weights_mb + cold.state_mb + cold.activations_mb + 320 + LOAD_OVERHEAD_MAX_MB);

        note_llm_memory(LlmMemory { load_cost_mb: Some(20_395), ..m.clone() });
        let recorded = note_llm_memory(m.clone());
        assert_eq!(recorded.load_cost_mb, Some(20_395));
        assert_eq!(llm_memory_of(&m.model), Some(recorded));
        let clean = LlmMemory { model: "scratch-eager-rebuild".into(), ..m };
        assert_eq!(note_llm_memory(clean.clone()).arena_mb(), 20_074);
        let rebuilt = note_llm_memory(LlmMemory {
            context: 32_768, kv_mb: 1_156, state_mb: 786, load_cost_mb: Some(19_626), ..clean
        });
        assert_eq!(rebuilt.arena_mb(), 18_770);
        assert_eq!((rebuilt.load_cost_mb, rebuilt.overhead_mb_for(false)), (Some(19_126), 35));
        LLM_MEMORY.lock().unwrap().retain(|m| !matches!(m.model.as_str(), "scratch-chat" | "scratch-lazy" | "scratch-eager-rebuild"));
    }

    #[test]
    fn relief_hysteresis_prevents_immediate_bounce_after_yield_at_steady_vram() {
        let reserve = 1_536;
        for kv_bytes_per_token in [36_992, 73_728, 17_408] {
            let mut m = LlmMemory { kv_bytes_per_token, ..warm_scratch_memory() };
            m.kv_mb = (u64::from(m.context) * kv_bytes_per_token).div_ceil(MB);
            let held = m.accounted_mb() + 35;
            m.load_cost_mb = Some(held);
            let slope = m.bytes_per_lane_token();
            let chat = ChatFacts { model: m.model.clone(), context: m.context, min_context: 2048,
                kv_bytes_per_token: slope, resident_mb: held };
            let cut = plan(Some(&chat), &[], 162 + 256, false);
            let [Step::ShrinkContext { from, to, frees_mb, .. }] = cut.steps.as_slice() else {
                panic!("expected a single shrink for {slope} bytes/token: {cut:?}");
            };
            assert_eq!(*from, m.context);
            let free_after = reserve - 162 + frees_mb;
            let held_after = held - frees_mb;
            let grow = |free| grow_target(&m, free, held_after, reserve, false, m.model_max_context, *to, 2048);
            assert_eq!(grow(free_after), None, "steady desktop at {slope} bytes/token");
            let grown = grow(free_after + 700).expect("desktop released enough memory");
            assert!(grown >= to + CONTEXT_STEP);
            let consumed = (u64::from(grown - to) * slope).div_ceil(MB);
            assert!(free_after + 700 - consumed >= reserve + HEADROOM_MB + RELIEF_HYSTERESIS_MB);
        }
    }

    #[test]
    fn relief_state_absorbs_repeating_desktop_bursts_and_converges_without_timers() {
        let m = warm_scratch_memory();
        let reserve = 1_536;
        let mut state = ReliefState::default();
        let mut loaded = m.context;
        let slope = m.bytes_per_lane_token();
        let held = |window| m.fixed_mb_for(false) + (u64::from(window) * slope).div_ceil(MB);
        let mut saw_reversal = false;
        for round in 1..=10 {
            let free = 21_417 - held(loaded);
            if free < reserve {
                assert!(round < 3, "yield in settled round {round}");
                let chat = ChatFacts { model: m.model.clone(), context: loaded, min_context: 2048,
                    kv_bytes_per_token: slope, resident_mb: held(loaded) };
                let cut = plan(Some(&chat), &[], reserve - free + 256, false);
                let [Step::ShrinkContext { from, to, frees_mb, .. }] = cut.steps.as_slice() else {
                    panic!("expected one shrink in round {round}: {cut:?}");
                };
                saw_reversal |= state.last_grown_to.is_some();
                state.note_yield_shrink(*from, *to, *frees_mb);
                loaded = *to;
            }
            assert!(21_417 - held(loaded) >= reserve, "short after yield in round {round}");
            if let Some(to) = grow_target_with_hysteresis(&m, 22_617 - held(loaded), held(loaded),
                reserve, state.hysteresis_mb(), false, m.model_max_context, loaded, 2048) {
                assert!(round < 3, "relief in settled round {round}");
                state.note_relief_grow(to);
                loaded = to;
            }
        }
        assert!(saw_reversal && state.extra_hysteresis_mb >= RELIEF_HYSTERESIS_MB);
        let grown = grow_target_with_hysteresis(&m, 26_617 - held(loaded), held(loaded),
            reserve, state.hysteresis_mb(), false, m.model_max_context, loaded, 2048);
        assert!(grown.is_some_and(|to| to > loaded), "genuine relief can still grow the settled window");
        state.reset();
        assert_eq!((state.hysteresis_mb(), state.last_grown_to), (RELIEF_HYSTERESIS_MB, None));
    }

    #[test]
    fn settle_ceiling_detailed_distinguishes_yield_hold_from_relief_and_resets_on_clear() {
        let _guard = crate::llm_context::test_settings();
        reset_relief_state();
        let upper = 262_144;
        let settle = || settle_ceiling_detailed(73_728, upper);
        let raw = SettledCeiling { window: 73_728, source: None };
        hold_relief_target(61_440, upper);
        assert_eq!(context_ceiling(), Some(61_440));
        assert_eq!(settle(), SettledCeiling { window: 61_440, source: Some(FitSource::Relief) });
        assert_eq!(settle(), raw);
        hold_ceiling(57_344);
        assert_eq!(settle(), SettledCeiling { window: 57_344, source: Some(FitSource::YieldHold) });
        hold_relief_target(61_440, upper);
        clear_pending_hold();
        assert_eq!(settle(), raw);
        reset_relief_state();
        set_context_ceiling(None);
    }

    #[test]
    fn the_context_is_sized_to_what_is_left_after_the_ui_reserve() {
        let m = mem();
        // arena 18734 + overhead 1666 capped at 1024, minus the KV
        assert_eq!(m.fixed_mb_for(false), 18_734 - 1_734 + LOAD_OVERHEAD_MAX_MB);
        // the whole card minus the UI's reserve and headroom: the configured window fits
        assert_eq!(fit_context_detailed(&m, Some(23_000), 1_500, false, Some(49152), 2048).map(|d| d.chosen), Ok(49152));
        // 600 MB over the fixed part, the reserve and the headroom: ~16.6k tokens, a whole step
        let c = fit_context_detailed(&m, Some(m.fixed_mb_for(false) + 1_500 + HEADROOM_MB + 600), 1_500, false, Some(49152), 2048).map(|d| d.chosen).unwrap();
        assert_eq!(c, 16384);
        // not even the floor window: refused, with the UI named
        let e = fit_context_detailed(&m, Some(m.fixed_mb_for(false) + 1_500 + HEADROOM_MB + 200), 1_500, false, Some(49152), 2048).map(|d| d.chosen).unwrap_err();
        assert!(e.contains("UI has priority"), "{e}");
    }

    #[test]
    fn a_yields_cut_holds_for_its_reload_then_the_budget_decides() {
        let _guard = crate::llm_context::test_settings();
        hold_ceiling(8192);
        assert_eq!(settle_ceiling(49152, 49152), 8192, "the reload the yield queued keeps the cut");
        assert_eq!(context_ceiling(), Some(8192));
        assert_eq!(settle_ceiling(49152, 49152), 49152, "a later load goes by the budget");
        assert_eq!(context_ceiling(), None);
        assert_eq!(settle_ceiling(16384, 49152), 16384);
        assert_eq!(context_ceiling(), Some(16384));
        set_context_ceiling(None);
    }

    #[test]
    fn each_chat_model_keeps_its_own_accounting_and_fit() {
        let _serial = llm_memory_tests();
        let mut small = mem();
        small.model = "qwen3.5-4b".into();
        small.weights_mb = 3000;
        small.kv_mb = 544;
        small.kv_bytes_per_token = 17_408;
        small.load_cost_mb = Some(4142);
        note_llm_memory(mem());
        note_llm_memory(small.clone());
        assert_eq!(llm_memory().unwrap().model, "qwen3.5-4b");
        assert_eq!(llm_memory_of("qwen3.8-27b").unwrap().weights_mb, 15_700);
        // the 4B next to a 4K desktop with 6 GB free: the whole 80k window fits
        assert_eq!(fit_context_detailed(&small, Some(6_000 + 1_536 + HEADROOM_MB), 1_536, false, Some(81920), 2048).map(|d| d.chosen), Ok(81920));
        // the 27B with 1.1 GB free while it holds 20.4 GB: ~40k fits, in whole steps
        let f = fit_context_detailed(&mem(), Some(1_100 + 20_400), 1_536, false, Some(81920), 2048).map(|d| d.chosen).unwrap();
        assert!(f % CONTEXT_STEP == 0 && (32768..49152).contains(&f), "{f}");
    }

    #[test]
    fn relief_grows_back_as_far_as_the_budget_allows_in_one_step() {
        let m = mem(); // fixed 18666 MB, 36 KiB/token
        // cut to 8192 (holding fixed + 289 MB); the UI let go of 2 GB: back to the configured 49152
        let held = m.fixed_mb_for(false) + 289;
        assert_eq!(grow_target(&m, 4_800, held, 1_536, false, 49152, 8192, 2048), Some(49152));
        // only ~1 GB over the reserve and headroom: part of the way, in whole steps
        let to = grow_target(&m, 1_536 + HEADROOM_MB + 1_000, held, 1_536, false, 49152, 8192, 2048).unwrap();
        assert!(to % CONTEXT_STEP == 0 && to > 8192 && to < 49152, "{to}");
        // no room for one more step: stays
        assert_eq!(grow_target(&m, 1_536, held, 1_536, false, 49152, 8192, 2048), None);
        assert_eq!(grow_target(&m, 30_000, held, 1_536, false, 49152, 49152, 2048), None, "already configured");
    }

    #[test]
    fn relief_waits_out_the_cooldown_and_a_later_yield_supersedes_it() {
        note_yield();
        let (at, epoch) = relief_schedule();
        assert!(at >= std::time::Instant::now() + RELIEF_COOLDOWN - std::time::Duration::from_secs(1));
        assert!(relief_current(epoch));
        note_yield();
        assert!(!relief_current(epoch), "a yield after the relief was asked drops it");
        let refusal = crate::error::AssetAiError::Unavailable(not_enough_message("q", 1, 2, 3));
        assert!(is_ui_priority_refusal(&refusal));
        assert_eq!(smallest_load_mb(&mem(), false), mem().fixed_mb_for(false) + 8192 * 36_992 / MB);
    }

    #[test]
    fn a_measured_refusals_numbers_fit_now() {
        // Measured on a node: the 27B cut to 16384 measured 'whole load 19711 MB' while the
        // desktop allocated; the hub then wanted 19439 MB for the floor and refused
        // with 21256 MB free next to a 2048 MB reserve+headroom.
        let _serial = llm_memory_tests();
        let first = LlmMemory { model: "q27".into(), model_max_context: 262_144, lanes: 1, measured: true, mask_bytes_per_token: 0, context: 49152, kv: "q8".into(), weights_mb: 15702, kv_mb: 1632,
            kv_bytes_per_token: 34816, state_mb: 891, activations_mb: 513, runtime_scratch_mb: 0, load_cost_mb: Some(19178) };
        let polluted = LlmMemory { context: 16384, kv_mb: 544, state_mb: 823, load_cost_mb: Some(19711), ..first.clone() };
        note_llm_memory(first);
        note_llm_memory(polluted);
        let m = llm_memory_of("q27").unwrap();
        assert_eq!(m.overhead_mb_for(false), 440, "the smaller overhead seen wins");
        assert!(smallest_load_mb(&m, false) < 18_000, "{}", smallest_load_mb(&m, false));
        let fit = fit_context_detailed(&m, Some(21_256), 1_536, false, Some(49152), 2048).map(|d| d.chosen).unwrap();
        assert!(fit >= 32768, "sized to the room, not refused: {fit}");
    }

    #[test]
    fn a_load_that_would_eat_the_ui_reserve_is_refused() {
        assert!(admit("qwen3.5-4b", 6_000, 4_600, 1_500).is_err());
        assert!(admit("qwen3.5-4b", 6_100, 4_600, 1_500).is_ok());
        assert!(admit("qwen3.5-4b", 6_000, 4_600, 0).is_ok());
    }
}
