//! The UI thread publishes its progress. The command waiter compares samples off-thread.
use crate::thread::ui_hang::phase_name;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

#[path = "ui_thread_probe.rs"]
mod thread_probe;
pub use thread_probe::{register_ui_thread, UiThreadProbe};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum UiState {
    #[default]
    NotStarted = 0,
    RunningTurn = 1,
    InPump = 2,
    Exited = 3,
}

pub mod hold {
    pub const FIRST_FRAME: u32 = 1 << 0;
    pub const GPU_FLIGHT: u32 = 1 << 1;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CmdStage {
    #[default]
    Queued = 0,
    Running = 1,
    Waiting = 2,
}

struct CommandLiveInner {
    alive: AtomicBool,
    stage: AtomicU8,
}

#[derive(Clone)]
pub struct CommandLive(Arc<CommandLiveInner>);

/// Dropping the waiting end cancels every copy held by the UI.
#[must_use]
pub struct PendingGuard(Arc<CommandLiveInner>);

impl CommandLive {
    pub fn new() -> (Self, PendingGuard) {
        let live = Self::detached();
        let guard = PendingGuard(Arc::clone(&live.0));
        (live, guard)
    }

    pub fn cancelled() -> Self {
        Self::with_alive(false)
    }

    pub fn detached() -> Self {
        Self::with_alive(true)
    }

    fn with_alive(alive: bool) -> Self {
        Self(Arc::new(CommandLiveInner {
            alive: AtomicBool::new(alive),
            stage: AtomicU8::new(CmdStage::Queued as u8),
        }))
    }

    pub fn is_alive(&self) -> bool {
        self.0.alive.load(Ordering::Acquire)
    }

    pub fn stage(&self) -> CmdStage {
        match self.0.stage.load(Ordering::Acquire) {
            0 => CmdStage::Queued,
            1 => CmdStage::Running,
            2 => CmdStage::Waiting,
            _ => unreachable!(),
        }
    }

    pub fn set_stage(&self, stage: CmdStage) {
        self.0.stage.store(stage as u8, Ordering::Release);
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiSnapshot {
    pub state: UiState,
    pub turns: u64,
    pub repaints: u64,
    pub phase: u32,
    pub holds: u32,
}

pub struct UiProgressState {
    state: AtomicU8,
    turns: AtomicU64,
    repaints: AtomicU64,
    phase: AtomicU32,
    holds: AtomicU32,
}

pub static GLOBAL_UI_PROGRESS: UiProgressState = UiProgressState::new();

impl UiProgressState {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(UiState::NotStarted as u8),
            turns: AtomicU64::new(0),
            repaints: AtomicU64::new(0),
            phase: AtomicU32::new(0),
            holds: AtomicU32::new(0),
        }
    }

    pub fn snapshot(&self) -> UiSnapshot {
        UiSnapshot {
            state: match self.state.load(Ordering::Acquire) {
                0 => UiState::NotStarted,
                1 => UiState::RunningTurn,
                2 => UiState::InPump,
                3 => UiState::Exited,
                _ => unreachable!(),
            },
            turns: self.turns.load(Ordering::Relaxed),
            repaints: self.repaints.load(Ordering::Relaxed),
            phase: self.phase.load(Ordering::Relaxed),
            holds: self.holds.load(Ordering::Relaxed),
        }
    }

    pub fn enter_loop(&'static self) -> UiLoopGuard {
        self.state.store(UiState::RunningTurn as u8, Ordering::Release);
        UiLoopGuard(self)
    }

    pub fn enter_turn(&self) -> UiTurnGuard<'_> {
        self.leave_pump_and_begin_turn();
        UiTurnGuard(self)
    }

    pub fn set_in_pump(&self) {
        self.state.store(UiState::InPump as u8, Ordering::Release);
    }

    pub fn leave_pump_and_begin_turn(&self) {
        self.state.store(UiState::RunningTurn as u8, Ordering::Release);
        self.turns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_repaint(&self, repaint_id: u64) {
        self.repaints.store(repaint_id, Ordering::Relaxed);
    }

    pub fn swap_phase(&self, id: u32) -> u32 {
        self.phase.swap(id, Ordering::Relaxed)
    }

    pub fn hold(&self, flag: u32) -> HoldGuard<'_> {
        let prev = self.holds.fetch_or(flag, Ordering::Relaxed);
        HoldGuard { state: self, cleared: flag & !prev }
    }

    pub fn set_hold(&self, flag: u32, on: bool) {
        if on {
            self.holds.fetch_or(flag, Ordering::Relaxed);
        } else {
            self.holds.fetch_and(!flag, Ordering::Relaxed);
        }
    }

    pub fn mark_exited(&self) {
        self.state.store(UiState::Exited as u8, Ordering::Release);
    }
}

#[must_use]
pub struct UiLoopGuard(&'static UiProgressState);

impl Drop for UiLoopGuard {
    fn drop(&mut self) {
        self.0.mark_exited();
        if core::ptr::eq(self.0, &GLOBAL_UI_PROGRESS) {
            crate::remote::on_loop_exit();
        }
    }
}

#[must_use]
pub struct UiTurnGuard<'a>(&'a UiProgressState);

impl Drop for UiTurnGuard<'_> {
    fn drop(&mut self) {
        self.0.set_in_pump();
    }
}

#[must_use]
pub struct HoldGuard<'a> {
    state: &'a UiProgressState,
    cleared: u32,
}

impl Drop for HoldGuard<'_> {
    fn drop(&mut self) {
        if self.cleared != 0 {
            self.state.holds.fetch_and(!self.cleared, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadExecState {
    #[default]
    Unknown,
    RunningOrRunnable,
    UninterruptibleIo,
    Sleeping,
    Stopped,
    Exited,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadExecSample {
    pub state: ThreadExecState,
    pub cpu_ns: Option<u64>,
    pub run_delay_ns: Option<u64>,
}

impl ThreadExecSample {
    pub const fn unknown() -> Self {
        Self { state: ThreadExecState::Unknown, cpu_ns: None, run_delay_ns: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JitWaitSample {
    pub busy: bool,
    pub parked: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchSample {
    pub ui: UiSnapshot,
    pub stage: CmdStage,
    pub thread: ThreadExecSample,
    pub jit: JitWaitSample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    Turn,
    Frame,
    Reload,
    Grab { not_before_ms: Option<u64> },
}

#[derive(Clone, Copy, Debug)]
pub struct WatchConfig {
    pub probe_interval_ms: u64,
    pub stall_confirm_probes: u32,
    pub slow_log_first_ms: u64,
    pub slow_log_every_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            probe_interval_ms: 200,
            stall_confirm_probes: 10,
            slow_log_first_ms: 5_000,
            slow_log_every_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchAction {
    Continue,
    Rewake,
    LogSlow(String),
    Stalled(String),
}

pub struct ProgressWatch {
    what: &'static str,
    kind: WaitKind,
    cfg: WatchConfig,
    started_ms: u64,
    initial_ui: UiSnapshot,
    initial_thread: ThreadExecSample,
    last: WatchSample,
    quiet_probes: u32,
    rewaked: bool,
    last_log_ms: Option<u64>,
}

fn ns_delta(now: Option<u64>, before: Option<u64>) -> u64 {
    match (now, before) {
        (Some(now), Some(before)) => now.saturating_sub(before),
        _ => 0,
    }
}

fn hold_name(holds: u32) -> &'static str {
    match holds {
        0 => "none",
        hold::FIRST_FRAME => "first-frame hold",
        hold::GPU_FLIGHT => "gpu in flight",
        both if both == hold::FIRST_FRAME | hold::GPU_FLIGHT => "first-frame hold and gpu in flight",
        _ => "unknown hold",
    }
}

fn progress_reason(sample: &WatchSample, last: &WatchSample) -> Option<&'static str> {
    if sample.stage != last.stage {
        Some("stage advanced")
    } else if sample.ui.repaints > last.ui.repaints {
        Some("repaints advancing")
    } else if sample.ui.turns > last.ui.turns {
        Some("event loop turning")
    } else if ns_delta(sample.thread.cpu_ns, last.thread.cpu_ns) > 0 {
        Some("ui cpu active")
    } else if ns_delta(sample.thread.run_delay_ns, last.thread.run_delay_ns) > 0
        || sample.thread.state == ThreadExecState::RunningOrRunnable
    {
        Some("ui runnable (CPU starved)")
    } else if sample.thread.state == ThreadExecState::UninterruptibleIo {
        Some("ui in kernel I/O wait")
    } else if sample.jit.busy {
        Some("relax jit compiling")
    } else if sample.ui.holds != 0 {
        Some(hold_name(sample.ui.holds))
    } else if sample.thread.state == ThreadExecState::Unknown
        && matches!(sample.ui.state, UiState::RunningTurn | UiState::NotStarted)
    {
        Some("ui thread state unknown")
    } else {
        None
    }
}

impl ProgressWatch {
    pub fn new(
        what: &'static str,
        kind: WaitKind,
        cfg: WatchConfig,
        started_ms: u64,
        initial: WatchSample,
    ) -> Self {
        Self {
            what,
            kind,
            cfg,
            started_ms,
            initial_ui: initial.ui,
            initial_thread: initial.thread,
            last: initial,
            quiet_probes: 0,
            rewaked: false,
            last_log_ms: None,
        }
    }

    pub fn step(&mut self, now_ms: u64, sample: WatchSample) -> WatchAction {
        if sample.ui.state == UiState::Exited {
            return WatchAction::Stalled("app event loop exited".into());
        }
        if sample.thread.state == ThreadExecState::Exited {
            return WatchAction::Stalled("UI thread exited".into());
        }
        if let Some(reason) = &sample.jit.parked {
            return WatchAction::Stalled(format!("UI thread parked in Relax: {reason}"));
        }
        if sample.thread.state == ThreadExecState::Stopped {
            return WatchAction::Stalled("UI thread stopped".into());
        }

        let last = core::mem::replace(&mut self.last, sample);
        let sample = &self.last;
        if matches!(self.kind, WaitKind::Grab { not_before_ms: Some(nb) } if now_ms < nb) {
            self.quiet_probes = 0;
            self.rewaked = false;
            return WatchAction::Continue;
        }
        if sample.stage == CmdStage::Queued {
            if sample.ui.turns >= self.initial_ui.turns.saturating_add(20) {
                return WatchAction::Stalled("event loop turning without polling remote control channel".into());
            }
            if sample.ui.state == UiState::InPump && sample.ui.turns == last.ui.turns {
                return self.quiet_pump("UI asleep in pump: undelivered wake");
            }
        }

        // Timer turns can keep the pump active without producing the requested frame or reload.
        if sample.stage == CmdStage::Waiting && sample.ui.state == UiState::InPump
            && sample.ui.holds == 0 && !sample.jit.busy
            && sample.ui.repaints == last.ui.repaints && sample.stage == last.stage
        {
            match self.kind {
                WaitKind::Frame | WaitKind::Grab { .. } => {
                    return self.quiet_pump("event loop idle without producing a frame");
                }
                WaitKind::Reload => {
                    return self.quiet_pump("live-edit reload did not arrive while UI and JIT were idle");
                }
                WaitKind::Turn => {}
            }
        }

        if let Some(why) = progress_reason(sample, &last) {
            self.quiet_probes = 0;
            self.rewaked = false;
            return self.slow_action(now_ms, why);
        } else {
            self.quiet_probes = self.quiet_probes.saturating_add(1);
            if self.quiet_probes >= self.cfg.stall_confirm_probes {
                return WatchAction::Stalled(format!(
                    "UI thread blocked or deadlocked (phase={} thread={})",
                    phase_name(sample.ui.phase),
                    format!("{:?}", sample.thread.state).to_ascii_lowercase(),
                ));
            }
        }
        WatchAction::Continue
    }

    // A grab handed to the encoder no longer depends on a UI turn or repaint.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn waiting_for_encoder(&mut self, now_ms: u64, ui: UiSnapshot) -> WatchAction {
        self.last.ui = ui;
        self.slow_action(now_ms, "encoding grab")
    }

    fn slow_action(&mut self, now_ms: u64, why: &str) -> WatchAction {
        let due = match self.last_log_ms {
            Some(prev) => now_ms.saturating_sub(prev) >= self.cfg.slow_log_every_ms,
            None => now_ms.saturating_sub(self.started_ms) >= self.cfg.slow_log_first_ms,
        };
        if due {
            self.last_log_ms = Some(now_ms);
            WatchAction::LogSlow(self.slow_line(now_ms, why))
        } else {
            WatchAction::Continue
        }
    }

    fn quiet_pump(&mut self, reason: &'static str) -> WatchAction {
        self.quiet_probes = self.quiet_probes.saturating_add(1);
        if !self.rewaked {
            self.rewaked = true;
            WatchAction::Rewake
        } else if self.quiet_probes >= self.cfg.stall_confirm_probes {
            WatchAction::Stalled(reason.into())
        } else {
            WatchAction::Continue
        }
    }

    fn slow_line(&self, now_ms: u64, why: &str) -> String {
        let sample = &self.last;
        let stage = match sample.stage {
            CmdStage::Queued => "queued",
            CmdStage::Running => "running",
            CmdStage::Waiting => "waiting",
        };
        let state = match sample.ui.state {
            UiState::NotStarted => "not-started",
            UiState::RunningTurn => "running-turn",
            UiState::InPump => "in-pump",
            UiState::Exited => "exited",
        };
        format!(
            "remote: {} waiting {:.1}s (stage={stage} state={state} phase={} holds={} why=\"{why}\" turns=+{} repaints=+{} ui_cpu=+{}ms run_wait=+{}ms)",
            self.what,
            now_ms.saturating_sub(self.started_ms) as f64 / 1000.0,
            phase_name(sample.ui.phase),
            hold_name(sample.ui.holds),
            sample.ui.turns.saturating_sub(self.initial_ui.turns),
            sample.ui.repaints.saturating_sub(self.initial_ui.repaints),
            ns_delta(sample.thread.cpu_ns, self.initial_thread.cpu_ns) / 1_000_000,
            ns_delta(sample.thread.run_delay_ns, self.initial_thread.run_delay_ns) / 1_000_000,
        )
    }

    pub fn logged_slow(&self) -> bool {
        self.last_log_ms.is_some()
    }

    pub fn summary_on_complete(&self, now_ms: u64, final_ui: UiSnapshot) -> String {
        format!(
            "remote: {} completed after {:.1}s (turns=+{} repaints=+{})",
            self.what,
            now_ms.saturating_sub(self.started_ms) as f64 / 1000.0,
            final_ui.turns.saturating_sub(self.initial_ui.turns),
            final_ui.repaints.saturating_sub(self.initial_ui.repaints),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::UiPhase;

    fn sample(state: UiState, stage: CmdStage) -> WatchSample {
        WatchSample {
            ui: UiSnapshot { state, ..UiSnapshot::default() },
            stage,
            thread: ThreadExecSample {
                state: ThreadExecState::Sleeping,
                cpu_ns: Some(10_000),
                run_delay_ns: None,
            },
            jit: JitWaitSample::default(),
        }
    }

    fn watch(kind: WaitKind, initial: &WatchSample) -> ProgressWatch {
        ProgressWatch::new("click", kind, WatchConfig::default(), 0, initial.clone())
    }

    // Every probe must keep waiting. The first diagnostic is the only action at 5 s.
    fn assert_busy(
        watch: &mut ProgressWatch,
        sample: &mut WatchSample,
        until_ms: u64,
        why: &str,
        mut advance: impl FnMut(&mut WatchSample, u64),
    ) -> String {
        let mut line = String::new();
        for now_ms in (200..=until_ms).step_by(200) {
            advance(sample, now_ms);
            let action = watch.step(now_ms, sample.clone());
            if now_ms == 5_000 {
                let WatchAction::LogSlow(log) = action else {
                    panic!("expected slow log at {now_ms} ms, got {action:?}");
                };
                assert!(log.contains(&format!("why=\"{why}\"")), "{log}");
                line = log;
            } else {
                assert_eq!(action, WatchAction::Continue, "unexpected action at {now_ms} ms");
            }
        }
        assert!(watch.logged_slow());
        line
    }

    #[test]
    fn busy_ui_thread_past_old_five_second_limit_applies_click_and_logs_once() {
        let (live, _pending) = CommandLive::new();
        let mut sample = sample(UiState::RunningTurn, live.stage());
        sample.ui.phase = UiPhase::FrameBuild as u32;
        let mut watch = watch(WaitKind::Frame, &sample);
        let log = assert_busy(&mut watch, &mut sample, 12_000, "ui cpu active", |s, ms| {
            s.thread.cpu_ns = Some(10_000 + ms * 1_000_000);
        });
        assert!(log.contains("phase=frame-build"), "{log}");
        assert!(log.contains("ui_cpu=+5000ms"), "{log}");
        assert!(live.is_alive(), "the delayed click must still be applicable");
        live.set_stage(CmdStage::Running);
        sample.stage = live.stage();
        sample.ui.repaints += 1;
        assert_eq!(watch.step(12_200, sample), WatchAction::Continue);
    }

    #[test]
    fn cpu_starved_runnable_ui_thread_past_five_seconds_keeps_waiting() {
        for state in [ThreadExecState::RunningOrRunnable, ThreadExecState::Sleeping] {
            let mut sample = sample(UiState::RunningTurn, CmdStage::Queued);
            sample.thread.state = state;
            sample.thread.run_delay_ns = Some(1_000);
            let mut watch = watch(WaitKind::Turn, &sample);
            let log = assert_busy(&mut watch, &mut sample, 8_000, "ui runnable (CPU starved)", |s, ms| {
                s.thread.run_delay_ns = Some(1_000 + ms * 1_000_000);
            });
            assert!(log.contains("ui_cpu=+0ms run_wait=+5000ms"), "{log}");
        }
    }

    #[test]
    fn cpu_starved_runnable_without_run_delay_keeps_waiting() {
        let mut sample = sample(UiState::RunningTurn, CmdStage::Queued);
        sample.thread.state = ThreadExecState::RunningOrRunnable;
        let mut watch = watch(WaitKind::Turn, &sample);
        assert_busy(&mut watch, &mut sample, 8_000, "ui runnable (CPU starved)", |_, _| {});
    }

    #[test]
    fn unknown_thread_state_with_flat_cpu_never_false_stalls() {
        for state in [UiState::RunningTurn, UiState::NotStarted] {
            for cpu_ns in [Some(10_000), None] {
                let mut sample = sample(state, CmdStage::Queued);
                sample.thread = ThreadExecSample { cpu_ns, ..ThreadExecSample::unknown() };
                let mut watch = watch(WaitKind::Turn, &sample);
                assert_busy(&mut watch, &mut sample, 8_000, "ui thread state unknown", |_, _| {});
            }
        }
    }

    #[test]
    fn deadlocked_ui_thread_fails_with_blocked_reason_and_cancels_command() {
        let (live, guard) = CommandLive::new();
        let mut sample = sample(UiState::RunningTurn, live.stage());
        sample.ui.phase = 21; // MouseDown event ID + 1
        sample.thread.run_delay_ns = Some(0);
        let mut watch = watch(WaitKind::Turn, &sample);
        for probe in 1..10 {
            assert_eq!(watch.step(probe * 200, sample.clone()), WatchAction::Continue);
        }
        let WatchAction::Stalled(reason) = watch.step(2_000, sample) else {
            panic!("a sleeping UI thread must stall on probe 10");
        };
        for detail in ["blocked or deadlocked", "phase=MouseDown", "thread=sleeping"] {
            assert!(reason.contains(detail), "{reason}");
        }
        drop(guard);
        assert!(!live.is_alive());
    }

    #[test]
    fn relax_parked_ui_thread_fails_immediately_with_park_reason() {
        let mut sample = sample(UiState::RunningTurn, CmdStage::Queued);
        let mut watch = watch(WaitKind::Turn, &sample);
        sample.jit.parked = Some("P1 compile error at foo.rs:42".into());
        assert_eq!(watch.step(200, sample), WatchAction::Stalled(
            "UI thread parked in Relax: P1 compile error at foo.rs:42".into()
        ));
    }

    #[test]
    fn asleep_in_pump_with_queued_command_rewakes_once_then_fails_undelivered_wake() {
        let mut sample = sample(UiState::InPump, CmdStage::Queued);
        let mut watch = watch(WaitKind::Turn, &sample);
        assert_eq!(watch.step(200, sample.clone()), WatchAction::Rewake);
        sample.ui.turns += 1;
        assert_eq!(watch.step(400, sample.clone()), WatchAction::Continue);
        assert_eq!(watch.step(600, sample.clone()), WatchAction::Rewake);
        for probe in 2..10 {
            assert_eq!(watch.step(400 + probe * 200, sample.clone()), WatchAction::Continue);
        }
        let WatchAction::Stalled(reason) = watch.step(2_400, sample) else {
            panic!("ten quiet probes must confirm the missed wake");
        };
        assert!(reason.contains("asleep in pump") && reason.contains("undelivered wake"), "{reason}");
    }

    #[test]
    fn wait_frame_idle_without_repaint_fails_as_frame_stall() {
        for kind in [WaitKind::Frame, WaitKind::Grab { not_before_ms: None }, WaitKind::Reload] {
            let mut sample = sample(UiState::InPump, CmdStage::Waiting);
            let mut watch = watch(kind, &sample);
            for probe in 1..=10 {
                sample.ui.turns += 1;
                let action = watch.step(probe * 200, sample.clone());
                match probe {
                    1 => assert_eq!(action, WatchAction::Rewake),
                    10 => {
                        let WatchAction::Stalled(reason) = action else {
                            panic!("timer turns alone cannot satisfy {kind:?}");
                        };
                        let expected = if kind == WaitKind::Reload {
                            "live-edit reload did not arrive while UI and JIT were idle"
                        } else {
                            "without producing a frame"
                        };
                        assert!(reason.contains(expected), "{reason}");
                    }
                    _ => assert_eq!(action, WatchAction::Continue),
                }
            }
        }
    }

    #[test]
    fn wait_frame_during_first_frame_hold_and_gpu_flight_keeps_waiting() {
        for (flag, why) in [(hold::FIRST_FRAME, "first-frame hold"), (hold::GPU_FLIGHT, "gpu in flight")] {
            let mut sample = sample(UiState::InPump, CmdStage::Waiting);
            sample.ui.holds = flag;
            let mut watch = watch(WaitKind::Frame, &sample);
            assert_busy(&mut watch, &mut sample, 8_000, why, |_, _| {});
        }
    }

    #[test]
    fn exited_event_loop_fails_immediately() {
        let mut sample = sample(UiState::RunningTurn, CmdStage::Queued);
        let mut watch = watch(WaitKind::Turn, &sample);
        sample.thread.state = ThreadExecState::Exited;
        assert_eq!(watch.step(200, sample.clone()), WatchAction::Stalled("UI thread exited".into()));
        sample.ui.state = UiState::Exited;
        assert_eq!(watch.step(200, sample), WatchAction::Stalled("app event loop exited".into()));
    }

    #[test]
    fn ui_progress_state_guards_command_live_and_queue_drain_stall() {
        static UI: UiProgressState = UiProgressState::new();
        assert_eq!(UI.snapshot(), UiSnapshot::default());
        let loop_guard = UI.enter_loop();
        assert_eq!(UI.snapshot().state, UiState::RunningTurn);
        {
            let _turn = UI.enter_turn();
            let _outer = UI.hold(hold::FIRST_FRAME);
            {
                let _inner = UI.hold(hold::FIRST_FRAME | hold::GPU_FLIGHT);
                assert_eq!(UI.snapshot().holds, hold::FIRST_FRAME | hold::GPU_FLIGHT);
            }
            assert_eq!(UI.snapshot().holds, hold::FIRST_FRAME);
            assert_eq!(UI.swap_phase(21), 0);
            UI.note_repaint(7);
        }
        assert_eq!(UI.snapshot(), UiSnapshot { state: UiState::InPump, turns: 1, repaints: 7, phase: 21, holds: 0 });
        UI.set_hold(hold::GPU_FLIGHT, true);
        assert_eq!(UI.snapshot().holds, hold::GPU_FLIGHT);
        UI.set_hold(hold::GPU_FLIGHT, false);
        UI.leave_pump_and_begin_turn();
        assert_eq!(UI.snapshot().turns, 2);
        assert_eq!(UI.snapshot().state, UiState::RunningTurn);
        assert_eq!(UI.snapshot().holds, 0);
        drop(loop_guard);
        assert_eq!(UI.snapshot().state, UiState::Exited);

        assert!(!CommandLive::cancelled().is_alive());
        assert!(CommandLive::detached().is_alive());
        let (live, guard) = CommandLive::new();
        let copy = live.clone();
        assert_eq!(copy.stage(), CmdStage::Queued);
        for stage in [CmdStage::Running, CmdStage::Waiting] {
            live.set_stage(stage);
            assert_eq!(copy.stage(), stage);
        }
        drop(guard);
        assert!(!copy.is_alive());

        let mut sample = sample(UiState::InPump, CmdStage::Queued);
        sample.ui.turns = 3;
        let kind = WaitKind::Grab { not_before_ms: Some(6_000) };
        let mut watch = watch(kind, &sample);
        for now_ms in (200..6_000).step_by(200) {
            assert_eq!(watch.step(now_ms, sample.clone()), WatchAction::Continue);
        }
        assert!(!watch.logged_slow());
        assert_eq!(watch.step(6_000, sample.clone()), WatchAction::Rewake);

        sample.ui.turns += 19;
        sample.ui.repaints += 9;
        assert!(matches!(watch.step(6_200, sample.clone()), WatchAction::LogSlow(_)));
        assert_eq!(watch.summary_on_complete(6_200, sample.ui),
            "remote: click completed after 6.2s (turns=+19 repaints=+9)");
        sample.thread.state = ThreadExecState::RunningOrRunnable;
        sample.ui.state = UiState::RunningTurn;
        assert_eq!(watch.step(36_199, sample.clone()), WatchAction::Continue);
        assert!(matches!(watch.step(36_200, sample.clone()), WatchAction::LogSlow(_)));
        sample.ui.turns += 1;
        assert_eq!(watch.step(36_400, sample.clone()), WatchAction::Stalled(
            "event loop turning without polling remote control channel".into()
        ));
        sample.thread.state = ThreadExecState::Stopped;
        assert_eq!(watch.step(36_400, sample), WatchAction::Stalled("UI thread stopped".into()));
    }
}
