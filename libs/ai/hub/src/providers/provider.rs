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

use crate::chat_wire::{ChatMessage, ProviderAvailability, ProviderKind, ServingFacts};

/// Trusted binary tool output, separate from bounded text/JSON history.
#[derive(Clone, Debug)]
pub struct ToolImage {
    pub label: String,
    /// PNG or JPEG bytes. The historical field name is kept for callers.
    pub png: std::sync::Arc<[u8]>,
}
/// The most pixels a tool picture may have a side: a tool fits its pictures within it (and
/// within its byte budget) before they come here.
pub const MAX_TOOL_IMAGE_SIDE: u32 = 1024;

/// Sniff the encoded picture without decoding its pixels.
pub fn tool_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        Some("image/jpeg")
    } else {
        None
    }
}

fn tool_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match tool_image_media_type(bytes)? {
        "image/png" => Some((
            u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
            u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
        )),
        "image/jpeg" => {
            let mut offset = 2;
            loop {
                if *bytes.get(offset)? != 0xff { return None; }
                while *bytes.get(offset)? == 0xff { offset += 1; }
                let marker = *bytes.get(offset)?;
                offset += 1;
                match marker {
                    0x00 | 0xd8 | 0xd9 | 0xda => return None,
                    0x01 | 0xd0..=0xd7 => continue, // standalone markers
                    _ => {}
                }
                let len = u16::from_be_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as usize;
                if len < 2 { return None; }
                let segment = bytes.get(offset..offset.checked_add(len)?)?;
                if matches!(marker, 0xc0 | 0xc1 | 0xc2) {
                    let height = u16::from_be_bytes(segment.get(3..5)?.try_into().ok()?);
                    let width = u16::from_be_bytes(segment.get(5..7)?.try_into().ok()?);
                    let components = *segment.get(7)? as usize;
                    if components == 0 || len != 8 + 3 * components { return None; }
                    return Some((width as u32, height as u32));
                }
                offset += len;
            }
        }
        _ => None,
    }
}

pub fn validate_tool_images(images:&[ToolImage])->Result<(),String>{
    if images.len()>4{return Err("too many tool images".into());}
    let mut bytes=0usize;
    for image in images {
        bytes=bytes.saturating_add(image.png.len());
        if image.label.len()>128 {return Err("invalid tool image".into());}
        let (w,h) = tool_image_dimensions(&image.png).ok_or("invalid tool image: expected PNG or JPEG dimensions")?;
        if w==0 || h==0 || w>MAX_TOOL_IMAGE_SIDE || h>MAX_TOOL_IMAGE_SIDE {return Err(format!("tool image dimensions exceed {MAX_TOOL_IMAGE_SIDE}"));}
    }
    if bytes>8*1024*1024{return Err("tool image byte budget exceeded".into());}Ok(())
}

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
    /// Volatile per-turn context (the game's world manifest, a live status
    /// block). Deliberately NOT part of `system`: the system block heads the
    /// prompt, and a system that changes every turn re-prefills the whole
    /// conversation on every KV-holding provider (aicore §7). Stateless
    /// cloud providers may still fold it into their per-request system; a
    /// KV-sticky provider renders it inside the CURRENT turn instead, where
    /// it extends the prefix rather than invalidating it.
    pub dynamic_context: String,
}

impl TurnInput {
    /// The system block with the volatile layer folded back in — for
    /// STATELESS providers (cloud HTTP, the CLIs), where every request
    /// re-sends everything anyway and prefix stability buys nothing.
    pub fn system_with_dynamic(&self) -> String {
        if self.dynamic_context.is_empty() {
            self.system.clone()
        } else {
            format!("{}\n{}", self.system, self.dynamic_context)
        }
    }

    pub fn new(system: impl Into<String>, messages: Vec<ChatMessage>) -> TurnInput {
        TurnInput {
            system: system.into(),
            messages,
            tools_enabled: true,
            dynamic_context: String::new(),
        }
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
    /// A tool's arguments while they are being written. This is the whole
    /// JSON prefix, possibly incomplete; consumers may preview it but must
    /// wait for `FunctionCall` before executing the tool.
    FunctionCallPartial { call_id: String, name: String, partial_json: String },
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

    /// Attach trusted rendered images to the next continuation. Unsupported
    /// providers explicitly refuse rather than pretending text is vision.
    fn attach_tool_images(&mut self, images: Vec<ToolImage>) -> Result<(), String> {
        if images.is_empty(){Ok(())}else{Err("this chat provider cannot receive rendered tool images".into())}
    }

    /// Resume after a native function call with the exact provider
    /// `call_id` and the encoded tool outcome. Providers that do not
    /// implement native tools leave the default (an error) in place.
    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        let _ = (call_id, output);
        Err("native tool continuation is not supported by this provider".to_string())
    }

    /// The host removed already-seen rows from its rolling context. This
    /// happens between provider requests and preserves the conversation.
    fn history_pruned(&mut self, _removed: usize) {}

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

    /// The model calls the session's tools natively, through an MCP server
    /// the host handed the provider, inside one turn: the session teaches
    /// no text tool protocol and extracts no calls from the reply.
    fn tools_over_mcp(&self) -> bool {
        false
    }

    /// The model reasons on its own channel (a native reasoner): the session
    /// never tells it to write its reasoning out in `<think>` tags (Claude's
    /// API refuses that as reasoning extraction). Only the fleet's own Qwen
    /// reasons in text.
    fn reasons_natively(&self) -> bool {
        self.kind() != ProviderKind::FleetQwen
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
    reasons_natively: bool,
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
    HistoryPruned(usize),
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
        let reasons_natively = inner.reasons_natively();
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
                    Ok(ThreadedCmd::HistoryPruned(removed)) => inner.history_pruned(removed),
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
            reasons_natively,
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

    fn reasons_natively(&self) -> bool {
        self.reasons_natively
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

    fn history_pruned(&mut self, removed: usize) {
        let _ = self.cmd_tx.send(ThreadedCmd::HistoryPruned(removed));
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
    use crate::chat_wire::ProviderAvailability;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    #[test]
    fn tool_pictures_accept_png_and_jpeg_and_check_dimensions() {
        let encode = |width: u16, height: u16, progressive| {
            let mut bytes = Vec::new();
            let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, 85);
            encoder.set_progressive(progressive);
            encoder.encode(&[20, 80, 140].repeat(width as usize * height as usize), width, height, jpeg_encoder::ColorType::Rgb).unwrap();
            bytes
        };
        let png = crate::testpattern::encode_png_rgb8(&[20, 80, 140], 1, 1).unwrap();
        assert_eq!(tool_image_media_type(&png), Some("image/png"), "PNG signature is recognised");
        let png = ToolImage { label: "png".into(), png: png.into() };
        assert!(validate_tool_images(&[png]).is_ok(), "existing PNG delivery remains valid");
        for progressive in [false, true] {
            let jpeg = encode(7, 3, progressive);
            assert_eq!(tool_image_media_type(&jpeg), Some("image/jpeg"), "JPEG signature is recognised");
            assert_eq!(tool_image_dimensions(&jpeg), Some((7, 3)), "SOF width and height are read in the right order");
            let image = ToolImage { label: "jpeg".into(), png: jpeg.into() };
            assert!(validate_tool_images(std::slice::from_ref(&image)).is_ok(), "baseline and progressive JPEG are accepted");
            let message = super::super::claude::image_message("inspect", &[image]);
            assert!(message.contains("image/jpeg"), "Claude labels the attached JPEG correctly");
        }
        let mut extended = encode(7, 3, false);
        let sof = extended.windows(2).position(|p| p == [0xff, 0xc0]).unwrap();
        extended[sof + 1] = 0xc1;
        assert_eq!(tool_image_dimensions(&extended), Some((7, 3)), "extended sequential SOF1 is supported");
        assert!(tool_image_dimensions(&extended[..sof + 5]).is_none(), "truncated SOF is rejected without a panic");
        assert!(tool_image_dimensions(&[0xff, 0xd8, 0xff, 0xe0, 0, 0]).is_none(), "zero-length marker cannot loop");
        for (width, height) in [(1025, 1), (1, 1025)] {
            let image = ToolImage { label: "oversize".into(), png: encode(width, height, false).into() };
            let error = validate_tool_images(&[image]).unwrap_err();
            assert!(error.contains("dimensions"), "either side over the limit is rejected");
        }
        assert_eq!(tool_image_media_type(b"not a picture"), None, "unknown formats are rejected");
    }

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
