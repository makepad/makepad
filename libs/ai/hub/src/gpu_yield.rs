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

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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

/// A load's cost beyond the model's arena is never counted above this (a CUDA
/// context and cuBLAS workspace are a few hundred MB).
pub const LOAD_OVERHEAD_MAX_MB: u64 = 1024;

/// After a yield, the chat model does not grow back for this long: a desktop
/// starting or switching is short for seconds, then relieved; growing back at
/// once rebuilt the model twice in four seconds on 149.
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
pub fn smallest_load_mb(memory: &LlmMemory) -> u64 {
    memory.fixed_mb() + u64::from(YIELD_FLOOR) * memory.kv_bytes_per_token / MB
}

const MB: u64 = 1 << 20;

/// MB of free GPU memory the UI keeps (0 = none asked for).
static UI_RESERVE_MB: AtomicU64 = AtomicU64::new(0);
/// The largest window the chat session may be built with (0 = no ceiling).
static CONTEXT_CEILING: AtomicU32 = AtomicU32::new(0);
/// Each chat model's own accounting, noted when it loads; the last noted last.
static LLM_MEMORY: Mutex<Vec<LlmMemory>> = Mutex::new(Vec::new());

/// What the UI keeps free: the last yield's `reserve_mb`, else
/// `MAKEPAD_AI_UI_RESERVE_MB` (so a hub sized for a desktop keeps the room
/// from its first load, before the WM ever asks), else 0.
pub fn ui_reserve_mb() -> u64 {
    match UI_RESERVE_MB.load(Ordering::Relaxed) {
        0 => {
            static ENV: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            let mb = *ENV.get_or_init(|| {
                let mb = std::env::var("MAKEPAD_AI_UI_RESERVE_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
                #[cfg(feature = "llm")]
                makepad_ai_llm::set_ui_reserve_bytes(mb * MB);
                mb
            });
            mb
        }
        mb => mb,
    }
}

/// The reserve the UI asked for; every later model load keeps it free.
pub fn set_ui_reserve_mb(mb: u64) {
    UI_RESERVE_MB.store(mb, Ordering::Relaxed);
    #[cfg(feature = "llm")]
    makepad_ai_llm::set_ui_reserve_bytes(mb * MB);
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

/// A yield's cut holds for the reload it queues: the UI asked for that room
/// and may not have taken it yet when the model comes back.
static HOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A yield cut the window to `context`; the next load keeps it even when the
/// budget would allow more.
pub fn hold_ceiling(context: u32) {
    set_context_ceiling(Some(context));
    HOLD.store(true, Ordering::Relaxed);
}

/// The window a load may use, given what fits next to the UI's reserve
/// (`fit`): the fit, under a yield's held cut once; later loads go by the
/// budget alone. Sets the ceiling accordingly.
pub fn settle_ceiling(fit: u32, configured: u32) -> u32 {
    let held = HOLD.swap(false, Ordering::Relaxed);
    let cap = if held { context_ceiling().map_or(fit, |c| c.min(fit)) } else { fit };
    set_context_ceiling((cap < configured).then_some(cap));
    cap.min(configured)
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
    pub context: u32,
    pub kv: String,
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub kv_bytes_per_token: u64,
    pub state_mb: u64,
    pub activations_mb: u64,
    /// Free memory before the load minus after it (None without NVML).
    pub load_cost_mb: Option<u64>,
}

impl LlmMemory {
    /// Everything the load cost that does not scale with the context.
    /// The model's own arena: weights, KV, state, activations.
    pub fn arena_mb(&self) -> u64 {
        self.weights_mb + self.kv_mb + self.state_mb + self.activations_mb
    }

    /// What the load cost beyond the arena (CUDA context, cuBLAS workspace,
    /// allocator slack), at most [`LOAD_OVERHEAD_MAX_MB`]: the free-memory
    /// difference around a load also counts whatever the desktop allocated
    /// meanwhile (149: 2129 MB 'overhead' at one load, 34 at another).
    pub fn overhead_mb(&self) -> u64 {
        self.load_cost_mb.map_or(0, |c| c.saturating_sub(self.arena_mb())).min(LOAD_OVERHEAD_MAX_MB)
    }

    /// Everything the load costs that does not scale with the context.
    pub fn fixed_mb(&self) -> u64 {
        self.arena_mb() - self.kv_mb + self.overhead_mb()
    }

    pub fn line(&self) -> String {
        format!(
            "{} memory: weights {} MB, KV {} MB ({} tokens {}, {} KiB/token), state {} MB, activations {} MB{}",
            self.model,
            self.weights_mb,
            self.kv_mb,
            self.context,
            self.kv,
            self.kv_bytes_per_token / 1024,
            self.state_mb,
            self.activations_mb,
            self.load_cost_mb.map_or(String::new(), |c| format!(", whole load {c} MB")),
        )
    }
}

pub fn note_llm_memory(memory: LlmMemory) {
    eprintln!("[gpu-yield] {}", memory.line());
    let mut memory = memory;
    let mut all = LLM_MEMORY.lock().unwrap();
    // The overhead is the smallest seen: a larger reading is the desktop
    // allocating during that load, not the model.
    if let Some(prev) = all.iter().find(|m| m.model == memory.model) {
        let overhead = memory.overhead_mb().min(prev.overhead_mb());
        memory.load_cost_mb = Some(memory.arena_mb() + overhead);
    }
    all.retain(|m| m.model != memory.model);
    all.push(memory);
    drop(all);
    crate::control::changed();
}

/// The chat model loaded last.
pub fn llm_memory() -> Option<LlmMemory> {
    LLM_MEMORY.lock().unwrap().last().cloned()
}

/// A chat model's accounting from its last load (None until it was loaded once).
pub fn llm_memory_of(model: &str) -> Option<LlmMemory> {
    LLM_MEMORY.lock().unwrap().iter().find(|m| m.model == model).cloned()
}

/// The largest window `memory`'s model could be built with now, next to the
/// UI's reserve: `free_mb` free plus `released_mb` that loading it would give
/// back (itself or the chat model it replaces). None when not even the
/// minimum fits.
pub fn context_fit(memory: &LlmMemory, free_mb: u64, released_mb: u64, context_max: u32, min_context: u32) -> Option<u32> {
    fit_context(memory, free_mb + released_mb, ui_reserve_mb(), context_max, min_context).ok()
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    ShrinkContext { model: String, from: u32, to: u32, frees_mb: u64 },
    Unload { model: String, frees_mb: u64 },
    UnloadChat { model: String, frees_mb: u64 },
}

impl Step {
    pub fn frees_mb(&self) -> u64 {
        match self {
            Step::ShrinkContext { frees_mb, .. } | Step::Unload { frees_mb, .. } | Step::UnloadChat { frees_mb, .. } => *frees_mb,
        }
    }

    pub fn line(&self) -> String {
        match self {
            Step::ShrinkContext { model, from, to, frees_mb } => format!("{model} context {from} -> {to} (~{frees_mb} MB)"),
            Step::Unload { model, frees_mb } => format!("unload idle {model} (~{frees_mb} MB)"),
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
/// [`YIELD_FLOOR`]), and the chat model itself only when `allow_unload` and
/// still short.
pub fn plan(chat: Option<&ChatFacts>, residents: &[Resident], wanted_mb: u64, allow_unload: bool) -> Plan {
    let mut out = Plan::default();
    if wanted_mb == 0 {
        return out;
    }
    let mut idle: Vec<&Resident> = residents.iter().filter(|r| !r.busy && r.mb > 0).collect();
    idle.sort_by(|a, b| b.mb.cmp(&a.mb).then_with(|| a.model.cmp(&b.model)));
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

/// The window a chat session may be built with when `free_mb` is free
/// before its load: what fits after the model's fixed cost and the UI's
/// reserve, up to `wanted`, in whole [`CONTEXT_STEP`]s. `Err` when not even
/// `min_context` fits: the UI has priority, the model is not loaded.
pub fn fit_context(memory: &LlmMemory, free_mb: u64, reserve_mb: u64, wanted: u32, min_context: u32) -> Result<u32, String> {
    let fixed = memory.fixed_mb();
    let reserve_mb = if reserve_mb > 0 { reserve_mb + HEADROOM_MB } else { 0 };
    let min_context = min_context.max(YIELD_FLOOR.min(wanted));
    let room_mb = free_mb.saturating_sub(reserve_mb).saturating_sub(fixed);
    let fits = if memory.kv_bytes_per_token == 0 {
        u64::from(wanted)
    } else {
        room_mb * MB / memory.kv_bytes_per_token
    };
    let context = if fits >= u64::from(wanted) {
        wanted
    } else {
        let fits = fits as u32;
        let stepped = fits / CONTEXT_STEP * CONTEXT_STEP;
        if stepped < min_context && fits >= min_context { min_context } else { stepped }
    };
    if context < min_context {
        return Err(not_enough_message(&memory.model, free_mb, reserve_mb, fixed + u64::from(min_context) * memory.kv_bytes_per_token / MB));
    }
    Ok(context)
}

/// The window to grow a loaded chat model back to once the UI's shortage is
/// over: what fits with `free_mb` free plus the `held_mb` its own rebuild
/// gives back, up to `configured`; None unless that is at least one
/// [`CONTEXT_STEP`] more than `loaded`.
pub fn grow_target(memory: &LlmMemory, free_mb: u64, held_mb: u64, reserve_mb: u64, configured: u32, loaded: u32, min_context: u32) -> Option<u32> {
    let fit = fit_context(memory, free_mb + held_mb, reserve_mb, configured, min_context).ok()?;
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
        Resident { model: model.into(), mb, busy }
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
        assert_eq!(*to, YIELD_FLOOR, "never under 8192 (149 went to 2048)");
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
    fn nothing_resident_reports_the_whole_shortfall() {
        let p = plan(None, &[], 1200, true);
        assert_eq!((p.steps.len(), p.frees_mb, p.short_mb), (0, 0, 1200));
        assert_eq!(plan(Some(&qwen()), &[], 0, true), Plan::default());
    }

    fn mem() -> LlmMemory {
        LlmMemory {
            model: "qwen3.8-27b".into(),
            context: 49152,
            kv: "q8".into(),
            weights_mb: 15_700,
            kv_mb: 1_734,
            kv_bytes_per_token: 36_992,
            state_mb: 700,
            activations_mb: 600,
            load_cost_mb: Some(20_400),
        }
    }

    #[test]
    fn the_context_is_sized_to_what_is_left_after_the_ui_reserve() {
        let m = mem();
        // arena 18734 + overhead 1666 capped at 1024, minus the KV
        assert_eq!(m.fixed_mb(), 18_734 - 1_734 + LOAD_OVERHEAD_MAX_MB);
        // the whole card minus the UI's reserve and headroom: the configured window fits
        assert_eq!(fit_context(&m, 23_000, 1_500, 49152, 2048), Ok(49152));
        // 600 MB over the fixed part, the reserve and the headroom: ~16.6k tokens, a whole step
        let c = fit_context(&m, m.fixed_mb() + 1_500 + HEADROOM_MB + 600, 1_500, 49152, 2048).unwrap();
        assert_eq!(c, 16384);
        // not even the floor window: refused, with the UI named
        let e = fit_context(&m, m.fixed_mb() + 1_500 + HEADROOM_MB + 200, 1_500, 49152, 2048).unwrap_err();
        assert!(e.contains("UI has priority"), "{e}");
    }

    #[test]
    fn a_yields_cut_holds_for_its_reload_then_the_budget_decides() {
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
        assert_eq!(fit_context(&small, 6_000 + 1_536 + HEADROOM_MB, 1_536, 81920, 2048), Ok(81920));
        // the 27B with 1.1 GB free while it holds 20.4 GB: ~40k fits, in whole steps
        let f = fit_context(&mem(), 1_100 + 20_400, 1_536, 81920, 2048).unwrap();
        assert!(f % CONTEXT_STEP == 0 && (32768..49152).contains(&f), "{f}");
    }

    #[test]
    fn relief_grows_back_as_far_as_the_budget_allows_in_one_step() {
        let m = mem(); // fixed 18666 MB, 36 KiB/token
        // cut to 8192 (holding fixed + 289 MB); the UI let go of 2 GB: back to the configured 49152
        let held = m.fixed_mb() + 289;
        assert_eq!(grow_target(&m, 4_800, held, 1_536, 49152, 8192, 2048), Some(49152));
        // only ~1 GB over the reserve and headroom: part of the way, in whole steps
        let to = grow_target(&m, 1_536 + HEADROOM_MB + 1_000, held, 1_536, 49152, 8192, 2048).unwrap();
        assert!(to % CONTEXT_STEP == 0 && to > 8192 && to < 49152, "{to}");
        // no room for one more step: stays
        assert_eq!(grow_target(&m, 1_536, held, 1_536, 49152, 8192, 2048), None);
        assert_eq!(grow_target(&m, 30_000, held, 1_536, 49152, 49152, 2048), None, "already configured");
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
        assert_eq!(smallest_load_mb(&mem()), mem().fixed_mb() + 8192 * 36_992 / MB);
    }

    #[test]
    fn the_149_refusal_numbers_fit_now() {
        // 149, 15:00: the 27B cut to 16384 measured 'whole load 19711 MB' while the
        // desktop allocated; the hub then wanted 19439 MB for the floor and refused
        // with 21256 MB free next to a 2048 MB reserve+headroom.
        let first = LlmMemory { model: "q27".into(), context: 49152, kv: "q8".into(), weights_mb: 15702, kv_mb: 1632,
            kv_bytes_per_token: 34816, state_mb: 891, activations_mb: 513, load_cost_mb: Some(19178) };
        let polluted = LlmMemory { context: 16384, kv_mb: 544, state_mb: 823, load_cost_mb: Some(19711), ..first.clone() };
        note_llm_memory(first);
        note_llm_memory(polluted);
        let m = llm_memory_of("q27").unwrap();
        assert_eq!(m.overhead_mb(), 440, "the smaller overhead seen wins");
        assert!(smallest_load_mb(&m) < 18_000, "{}", smallest_load_mb(&m));
        let fit = fit_context(&m, 21_256, 1_536, 49152, 2048).unwrap();
        assert!(fit >= 32768, "sized to the room, not refused: {fit}");
    }

    #[test]
    fn a_load_that_would_eat_the_ui_reserve_is_refused() {
        assert!(admit("qwen3.5-4b", 6_000, 4_600, 1_500).is_err());
        assert!(admit("qwen3.5-4b", 6_100, 4_600, 1_500).is_ok());
        assert!(admit("qwen3.5-4b", 6_000, 4_600, 0).is_ok());
    }
}
