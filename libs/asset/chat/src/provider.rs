//! The explicit-provider seam. A provider owns exactly one conversation
//! lane (fleet Qwen, OpenAI Responses, or xAI Grok Responses) and exposes
//! the same tiny, headless, poll-driven surface — no UI Cx, no async
//! runtime, no callbacks. The session engine above is provider-agnostic.
//! Fleet Qwen still streams text and uses the textual tool marker;
//! OpenAI/Grok emit a native [`ProviderEvent::FunctionCall`] and resume
//! through [`ChatProvider::continue_function`].
//!
//! Contract:
//! - One in-flight turn per provider. `begin_turn` while active is an error.
//! - `poll` is non-blocking and may return nothing.
//! - `cancel` is best-effort immediate; after it, the provider is idle and
//!   late events are ignored. The worker is never joined.
//! - `availability` is a live, honest probe — it is what the UI shows, and
//!   what the session engine checks before every send. There is no
//!   fallback path anywhere below this trait.

use crate::wire::{ChatMessage, ProviderAvailability, ProviderKind, ServingFacts};

/// Everything a provider needs for one turn. `system` carries the tool
/// protocol, live capabilities and attachment bindings; `messages` is the
/// bounded conversation so far (user/assistant/tool roles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnInput {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    /// When false, native providers omit the function list. Used by the
    /// broker's `llm.consult` one-shot so the external model cannot nest
    /// another tool-capable session.
    pub tools_enabled: bool,
}

impl TurnInput {
    pub fn new(system: impl Into<String>, messages: Vec<ChatMessage>) -> TurnInput {
        TurnInput { system: system.into(), messages, tools_enabled: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    /// Streaming assistant text, in order. Native Responses providers emit
    /// one `Delta` with the full visible text before `FunctionCall` or
    /// `Done`.
    Delta(String),
    /// Job/load progress (prefill, token count, queued). Not assistant text.
    Status { note: String, permille: u16 },
    /// Presentation-only serving facts for the deltas that follow (how many
    /// tokens the box has generated this round, lane contention when it
    /// advertises lanes). Providers that know none of it never emit it, and
    /// no consumer may depend on it arriving.
    Serving(ServingFacts),
    /// A native function call. `arguments` is the raw JSON string; the
    /// session parses it to an object and remains the security boundary.
    FunctionCall { call_id: String, name: String, arguments: String },
    /// The turn finished; `text` is the full assistant reply (providers
    /// that only streamed deltas pass the accumulated text).
    Done { text: String },
    /// The turn failed. The provider is idle afterwards.
    Error(String),
}

pub trait ChatProvider {
    fn kind(&self) -> ProviderKind;

    /// Live probe. Called before every send and whenever the UI wants a
    /// status row; must be cheap enough for that.
    fn availability(&mut self) -> ProviderAvailability;

    /// Start one turn. Err is a bounded, user-showable reason.
    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String>;

    /// Drain pending events. Non-blocking.
    fn poll(&mut self) -> Vec<ProviderEvent>;

    /// Abort the in-flight turn, if any.
    fn cancel(&mut self);

    /// Resume after a native function call with the exact provider
    /// `call_id` and the encoded tool outcome. Providers that do not
    /// implement native tools leave the default (an error) in place.
    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        let _ = (call_id, output);
        Err("native tool continuation is not supported by this provider".to_string())
    }

    /// Abandon the conversation state deliberately: any in-flight turn is
    /// cancelled AND any pending native function call is dropped, so the
    /// NEXT `begin_turn` starts a fresh provider conversation over whatever
    /// history the session sends. This is how a turn that ends on an
    /// unanswerable tool round (`world.new_level` hands the player to
    /// another game) leaves a native provider usable — plain `cancel`
    /// deliberately preserves the pending call so an accidental abort
    /// fails closed instead. Textual providers have no such state; the
    /// default is `cancel`.
    fn reset_conversation(&mut self) {
        self.cancel();
    }
}

// ------------------------------------------------------------------ threaded

/// Runs any [`ChatProvider`] on its own worker thread, so the single-threaded
/// broker actor NEVER blocks on provider HTTP. The fleet provider's
/// `begin_turn` rides out flaky-LAN connect windows with ~18 s of backoff —
/// inline on the actor that starved every other session's events and
/// tool-result routes for the whole ride (play-session-1 entry 13's third
/// act), and each poll's connect timeout stuttered the pump. Behind this
/// wrapper the actor's `begin_turn` is a channel send and `poll` a drain.
///
/// Failures from the async `begin_turn` surface as
/// [`ProviderEvent::Error`] through `poll`, which the session engine
/// already routes to an honest turn error. `availability` answers from the
/// last known probe and refreshes in the background — the actor's create
/// path does its own factory probe, so a slightly stale row here only
/// affects the status display between refreshes.
pub struct ThreadedProvider {
    kind: ProviderKind,
    cmd_tx: std::sync::mpsc::Sender<ThreadedCmd>,
    msg_rx: std::sync::mpsc::Receiver<ThreadedMsg>,
    last_availability: ProviderAvailability,
    probe_in_flight: bool,
    /// Non-availability messages drained while answering `availability`,
    /// held for the next `poll`.
    pending: Vec<ThreadedMsg>,
}

enum ThreadedCmd {
    Begin(TurnInput),
    ContinueFunction { call_id: String, output: String },
    Cancel,
    Reset,
    Probe,
    Shutdown,
}

enum ThreadedMsg {
    Events(Vec<ProviderEvent>),
    BeginFailed(String),
    ContinueFailed(String),
    Availability(ProviderAvailability),
}

impl ThreadedProvider {
    /// Wrap `inner`, seeding the availability row (callers usually just
    /// probed it while deciding to open the provider at all).
    pub fn spawn<P: ChatProvider + Send + 'static>(
        mut inner: P,
        seed_availability: ProviderAvailability,
    ) -> ThreadedProvider {
        let kind = inner.kind();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<ThreadedCmd>();
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<ThreadedMsg>();
        std::thread::Builder::new()
            .name("chat-provider".into())
            .spawn(move || loop {
                use std::sync::mpsc::RecvTimeoutError;
                match cmd_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(ThreadedCmd::Begin(input)) => {
                        if let Err(e) = inner.begin_turn(&input) {
                            let _ = msg_tx.send(ThreadedMsg::BeginFailed(e));
                        }
                    }
                    Ok(ThreadedCmd::ContinueFunction { call_id, output }) => {
                        if let Err(e) = inner.continue_function(&call_id, &output) {
                            let _ = msg_tx.send(ThreadedMsg::ContinueFailed(e));
                        }
                    }
                    Ok(ThreadedCmd::Cancel) => inner.cancel(),
                    Ok(ThreadedCmd::Reset) => inner.reset_conversation(),
                    Ok(ThreadedCmd::Probe) => {
                        let _ = msg_tx.send(ThreadedMsg::Availability(inner.availability()));
                    }
                    Ok(ThreadedCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                        inner.cancel();
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
                let events = inner.poll();
                if !events.is_empty() {
                    let _ = msg_tx.send(ThreadedMsg::Events(events));
                }
            })
            .expect("spawn chat provider worker");
        ThreadedProvider {
            kind,
            cmd_tx,
            msg_rx,
            last_availability: seed_availability,
            probe_in_flight: false,
            pending: Vec::new(),
        }
    }

    fn route(
        msg: ThreadedMsg,
        out: &mut Vec<ProviderEvent>,
        last_availability: &mut ProviderAvailability,
        probe_in_flight: &mut bool,
    ) {
        match msg {
            ThreadedMsg::Events(events) => out.extend(events),
            ThreadedMsg::BeginFailed(e) => {
                out.push(ProviderEvent::Error(format!("turn request failed: {e}")))
            }
            ThreadedMsg::ContinueFailed(e) => {
                out.push(ProviderEvent::Error(format!("tool continuation failed: {e}")))
            }
            ThreadedMsg::Availability(a) => {
                *last_availability = a;
                *probe_in_flight = false;
            }
        }
    }
}

impl Drop for ThreadedProvider {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(ThreadedCmd::Shutdown);
    }
}

impl ChatProvider for ThreadedProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn availability(&mut self) -> ProviderAvailability {
        // Drain any pending answer first, then keep one refresh in flight.
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                ThreadedMsg::Availability(a) => {
                    self.last_availability = a;
                    self.probe_in_flight = false;
                }
                // Events observed during an availability check are NOT
                // dropped: push them back through a local buffer is
                // impossible on a Receiver, so hold them in the pending
                // vec below.
                other => self.pending.push(other),
            }
        }
        if !self.probe_in_flight {
            self.probe_in_flight = self.cmd_tx.send(ThreadedCmd::Probe).is_ok();
        }
        self.last_availability.clone()
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        self.cmd_tx
            .send(ThreadedCmd::Begin(input.clone()))
            .map_err(|_| "provider worker is gone".to_string())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let mut out = Vec::new();
        for msg in self.pending.drain(..).collect::<Vec<_>>() {
            Self::route(msg, &mut out, &mut self.last_availability, &mut self.probe_in_flight);
        }
        while let Ok(msg) = self.msg_rx.try_recv() {
            Self::route(msg, &mut out, &mut self.last_availability, &mut self.probe_in_flight);
        }
        out
    }

    fn cancel(&mut self) {
        let _ = self.cmd_tx.send(ThreadedCmd::Cancel);
    }

    fn reset_conversation(&mut self) {
        let _ = self.cmd_tx.send(ThreadedCmd::Reset);
    }

    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        self.cmd_tx
            .send(ThreadedCmd::ContinueFunction {
                call_id: call_id.to_string(),
                output: output.to_string(),
            })
            .map_err(|_| "provider worker is gone".to_string())
    }
}

#[cfg(test)]
mod threaded_tests {
    use super::*;
    use crate::wire::ProviderAvailability;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    /// A provider whose begin_turn BLOCKS like the fleet's flaky-LAN
    /// backoff, scripted through channels.
    struct SlowProvider {
        gate: Receiver<Result<Vec<ProviderEvent>, String>>,
        staged: Vec<ProviderEvent>,
    }

    impl ChatProvider for SlowProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::FleetQwen
        }
        fn availability(&mut self) -> ProviderAvailability {
            ProviderAvailability::Available { model: "test".into(), detail: String::new() }
        }
        fn begin_turn(&mut self, _input: &TurnInput) -> Result<(), String> {
            // Blocks until the test opens the gate — the actor-starvation
            // shape in miniature.
            match self.gate.recv().expect("gate") {
                Ok(events) => {
                    self.staged = events;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        fn poll(&mut self) -> Vec<ProviderEvent> {
            std::mem::take(&mut self.staged)
        }
        fn cancel(&mut self) {}
    }

    fn wrapped() -> (ThreadedProvider, Sender<Result<Vec<ProviderEvent>, String>>) {
        let (tx, rx) = channel();
        let inner = SlowProvider { gate: rx, staged: Vec::new() };
        let seed = ProviderAvailability::Available { model: "test".into(), detail: String::new() };
        (ThreadedProvider::spawn(inner, seed), tx)
    }

    #[test]
    fn begin_turn_never_blocks_the_caller_and_events_flow() {
        let (mut p, gate) = wrapped();
        let input = TurnInput::new("sys", Vec::new());
        let t0 = Instant::now();
        p.begin_turn(&input).expect("begin");
        assert!(
            t0.elapsed() < Duration::from_millis(200),
            "begin_turn blocked the caller: {:?}",
            t0.elapsed()
        );
        // The worker is still stuck in begin_turn; polls stay empty and fast.
        assert!(p.poll().is_empty());
        // Open the gate with a turn's worth of events.
        gate.send(Ok(vec![
            ProviderEvent::Delta("hello".into()),
            ProviderEvent::Done { text: "hello".into() },
        ]))
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = Vec::new();
        while Instant::now() < deadline && got.len() < 2 {
            got.extend(p.poll());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.len(), 2, "events must arrive through poll: {got:?}");
    }

    #[test]
    fn a_failed_begin_surfaces_as_an_error_event() {
        let (mut p, gate) = wrapped();
        p.begin_turn(&TurnInput::new("sys", Vec::new())).expect("begin queues");
        gate.send(Err("fleet is down".into())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let events = p.poll();
            if let Some(ProviderEvent::Error(msg)) = events.first() {
                assert!(msg.contains("fleet is down"), "{msg}");
                break;
            }
            assert!(Instant::now() < deadline, "no error event arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
