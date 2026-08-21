//! The conversation on screen: what the user said, what the model is saying
//! back, which tools ran, and how fast the reply is arriving.
//!
//! The model is a process-global so the list widget can read it during draw
//! without threading a scope through. It holds only PRESENTATION state —
//! every real effect goes through the app's own tools, never from here.

pub static CHAT: std::sync::RwLock<ChatData> = std::sync::RwLock::new(ChatData {
    messages: Vec::new(),
    streaming_text: String::new(),
    activity: String::new(),
    activity_shown_at: None,
    activity_clear_pending: false,
    thinking_text: String::new(),
    status: String::new(),
    is_streaming: false,
    last_delta: None,
    rate: RateMeter::new(),
});

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChatRole {
    User,
    Assistant,
    /// Engine-side trouble (a failed eval, an unreachable server). Shown
    /// differently because the user did not say it and the AI did not
    /// either. Every error the transport can see lands here — visible, in
    /// the app's own voice, never a raw provider string in a bubble.
    System,
    /// A compact tool-call chip ("queried: SELECT … → 12 rows"), expandable
    /// to the full arguments/result.
    Tool,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    /// Tool chips only: the full call + result, shown when expanded.
    pub detail: Option<String>,
    /// Tool chips only: the session's call id, so the running chip can be
    /// completed in place when the result arrives.
    pub tool_id: Option<String>,
    pub expanded: bool,
    /// Assistant replies only: how fast this one came out ("78 tok/s"),
    /// pinned small and dim under the text once it landed.
    pub meta: Option<String>,
}

impl ChatMessage {
    fn plain(role: ChatRole, text: String) -> ChatMessage {
        ChatMessage { role, text, detail: None, tool_id: None, expanded: false, meta: None }
    }
}

pub struct ChatData {
    pub messages: Vec<ChatMessage>,
    pub streaming_text: String,
    /// What the agent is doing right now, e.g. "running assets.query…".
    /// Pinned under the streaming reply so a long silent tool call still
    /// moves.
    pub activity: String,
    /// See [`ChatData::set_activity`]: when the current activity appeared,
    /// and whether a clear is waiting for the minimum display time.
    pub activity_shown_at: Option<std::time::Instant>,
    pub activity_clear_pending: bool,
    /// Live reasoning text (the think block so far), for the porthole under
    /// the think dots.
    pub thinking_text: String,
    /// The session-level line an app pins above the transcript ("Qwen ready
    /// · qwen3-27b"). Set by the feed, rendered by the host.
    pub status: String,
    pub is_streaming: bool,
    pub last_delta: Option<std::time::Instant>,
    /// How fast the reply on screen is arriving (see [`RateMeter`]).
    pub rate: RateMeter,
}

// ------------------------------------------------------------------- rate

use std::time::{Duration, Instant};

/// The live rate averages over the last stretch rather than the whole
/// reply: a reader wants to see the CURRENT speed (a box that just picked
/// up three more conversations slows down, and the number should say so),
/// which a since-the-start average would smear away.
const RATE_WINDOW: Duration = Duration::from_secs(2);
/// Nothing arriving for this long means there is no current rate at all —
/// a tool is running, or the model is between rounds. Better to show
/// nothing than a number frozen at whatever the last burst managed.
const RATE_STALE: Duration = Duration::from_millis(2000);
/// Too short a span is noise, not a measurement.
const RATE_MIN_SPAN: f64 = 0.25;
/// Guessing tokens from bytes when nothing counts them for us. Qwen-family
/// tokenizers land near four bytes per token on prose; the guess is always
/// SHOWN as a guess (a leading `~`), never dressed up as a count.
const BYTES_PER_TOKEN: f64 = 4.0;
/// Ring bound. The window trims by time; this only stops a pathologically
/// chatty stream from growing the buffer without limit.
const RATE_MAX_POINTS: usize = 512;
/// How long a freshly shown activity line is held before a clear may take
/// it away (see [`ChatData::set_activity`]).
pub const ACTIVITY_MIN_SHOWN: Duration = Duration::from_millis(700);

/// Tokens-per-second for the reply currently on screen.
///
/// Deltas are `partial_text` diffs at the broker's poll cadence — one
/// delta is not one token and its byte length is not a token count — so
/// the honest input is the serving box's own generated-token counter,
/// forwarded on the delta. When the service does not send one (an older
/// service, or a device-local agent lane) the meter estimates from bytes
/// and marks the readout `~`.
///
/// The clock starts at the FIRST delta and every rate is a difference
/// between two samples, so time spent waiting for the first token never
/// enters the number.
pub struct RateMeter {
    /// `(arrival, cumulative tokens)`, trimmed to [`RATE_WINDOW`].
    points: Vec<(Instant, f64)>,
    /// The first sample of this reply, kept past the window for the
    /// whole-message average.
    first: Option<(Instant, f64)>,
    cum: f64,
    /// The service counts tokens for us — the readout is exact.
    exact: bool,
    /// Last cumulative count seen. It restarts at 0 every provider round,
    /// so a DECREASE is a new round, not a gap.
    last_gen: Option<u32>,
    /// `(lanes_active, slots_total)` as the serving box last advertised.
    lanes: Option<(u32, u32)>,
    /// Hidden reasoning tokens so far; the reply is still "thinking" while
    /// the service reports think progress but no visible tokens yet.
    think: Option<u32>,
    visible: Option<u32>,
}

impl Default for RateMeter {
    fn default() -> RateMeter {
        RateMeter::new()
    }
}

impl RateMeter {
    pub const fn new() -> RateMeter {
        RateMeter {
            points: Vec::new(),
            first: None,
            cum: 0.0,
            exact: false,
            last_gen: None,
            lanes: None,
            think: None,
            visible: None,
        }
    }

    /// A new reply segment starts (a turn, or the text after a tool call).
    pub fn reset(&mut self) {
        self.points.clear();
        self.first = None;
        self.cum = 0.0;
        self.exact = false;
        self.last_gen = None;
        self.think = None;
        self.visible = None;
        // Lane facts survive: they describe the box, not the segment.
    }

    /// One delta arrived: `bytes` of text, plus the serving box's
    /// cumulative token count and lane contention when it sent them.
    pub fn record(
        &mut self,
        now: Instant,
        bytes: usize,
        gen_tokens: Option<u32>,
        lanes: Option<(u32, u32)>,
        think: Option<u32>,
        visible: Option<u32>,
    ) {
        if lanes.is_some() {
            self.lanes = lanes;
        }
        if think.is_some() {
            self.think = think;
        }
        if visible.is_some() {
            self.visible = visible;
        }
        let tokens = match gen_tokens {
            Some(gen) => {
                let step = match self.last_gen {
                    Some(prev) if gen >= prev => (gen - prev) as f64,
                    // First count of the segment, or the round restarted.
                    _ => gen as f64,
                };
                self.last_gen = Some(gen);
                self.exact = true;
                step
            }
            // Once a real count exists we never mix guesses into it: the
            // count is cumulative, so bytes that arrived without one are
            // already inside the next count.
            None if self.exact => 0.0,
            None => bytes as f64 / BYTES_PER_TOKEN,
        };
        self.cum += tokens;
        let point = (now, self.cum);
        if self.first.is_none() {
            self.first = Some(point);
        }
        self.points.push(point);
        let cutoff = now.checked_sub(RATE_WINDOW);
        while self.points.len() > 2 {
            let too_old = cutoff.is_some_and(|c| self.points[1].0 < c);
            if !too_old && self.points.len() <= RATE_MAX_POINTS {
                break;
            }
            self.points.remove(0);
        }
    }

    /// Current speed, or `None` while there is nothing honest to say.
    pub fn live(&self, now: Instant) -> Option<f64> {
        let first = *self.points.first()?;
        let last = *self.points.last()?;
        if now.duration_since(last.0) > RATE_STALE {
            return None;
        }
        Self::between(first, last)
    }

    /// Whole-message average, measured from the first delta.
    pub fn average(&self) -> Option<f64> {
        Self::between(self.first?, *self.points.last()?)
    }

    fn between(a: (Instant, f64), b: (Instant, f64)) -> Option<f64> {
        let span = b.0.duration_since(a.0).as_secs_f64();
        (span >= RATE_MIN_SPAN).then(|| (b.1 - a.1) / span)
    }

    /// The readout itself: `78 tok/s`, `~78 tok/s` when the tokens were
    /// estimated, and the box's lane contention appended when it runs more
    /// than one lane (a single-lane box has nothing to say about
    /// contention, so it says nothing).
    pub fn label(&self, rate: f64) -> String {
        // The number is always the GPU's real generation rate (thinking
        // included — the user's question is "how fast is the card").
        // Thinking is a STATE annotation, not a separate rate.
        let mut out = format!("{}{rate:.0} tok/s", if self.exact { "" } else { "~" });
        if self.visible.is_none() && self.think.is_some() {
            out.push_str(" · thinking");
        }
        if let Some((active, total)) = self.lanes {
            if total > 1 {
                out.push_str(&format!(" · {active}/{total} lanes"));
            }
        }
        out
    }

    pub fn live_label(&self, now: Instant) -> Option<String> {
        self.live(now).map(|rate| self.label(rate))
    }

    pub fn final_label(&self) -> Option<String> {
        self.average().map(|rate| self.label(rate))
    }
}

impl ChatData {
    pub fn push(role: ChatRole, text: impl Into<String>) {
        if let Ok(mut data) = CHAT.write() {
            data.messages.push(ChatMessage::plain(role, text.into()));
        }
    }

    pub fn begin_stream() {
        if let Ok(mut data) = CHAT.write() {
            data.streaming_text.clear();
            data.is_streaming = true;
            data.rate.reset();
        }
    }

    pub fn push_delta(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            data.streaming_text.push_str(text);
            let now = std::time::Instant::now();
            // This lane (a device-local agent) hands us text, never a token
            // count, so the rate is an estimate and says so.
            data.rate.record(now, text.len(), None, None, None, None);
            data.last_delta = Some(now);
        }
    }

    /// One streaming delta as it arrived from the chat service: `bytes` of
    /// RAW model text (thinking and tool lines included — all of it was
    /// generated), with the service's own cumulative token count and lane
    /// contention when it sends them. Separate from the visible-text
    /// update because the visible text is re-derived, not append-only.
    pub fn note_delta(
        bytes: usize,
        gen_tokens: Option<u32>,
        lanes: Option<(u32, u32)>,
        think: Option<u32>,
        visible: Option<u32>,
    ) {
        if let Ok(mut data) = CHAT.write() {
            data.rate.record(std::time::Instant::now(), bytes, gen_tokens, lanes, think, visible);
        }
    }

    /// Replace the streaming bubble wholesale. The broker session re-derives
    /// the VISIBLE text after every delta (thinking and tool lines are
    /// stripped), so the shown prefix is not append-only.
    pub fn set_stream_text(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            if data.streaming_text != text {
                data.streaming_text.clear();
                data.streaming_text.push_str(text);
                data.last_delta = Some(std::time::Instant::now());
            }
        }
    }

    /// A tool call started: land any visible text said before it as an
    /// assistant message, then add the running chip.
    pub fn push_tool(id: &str, title: impl Into<String>, detail: impl Into<String>) {
        if let Ok(mut data) = CHAT.write() {
            let said = std::mem::take(&mut data.streaming_text);
            if !said.trim().is_empty() {
                let meta = data.rate.final_label();
                let mut msg = ChatMessage::plain(ChatRole::Assistant, said);
                msg.meta = meta;
                data.messages.push(msg);
            }
            // The next round is a new generation on a new job; its speed is
            // its own.
            data.rate.reset();
            data.messages.push(ChatMessage {
                role: ChatRole::Tool,
                text: title.into(),
                detail: Some(detail.into()),
                tool_id: Some(id.to_string()),
                expanded: false,
                meta: None,
            });
        }
    }

    /// The result arrived: complete the chip in place (matched by call id).
    pub fn finish_tool(id: &str, title: impl Into<String>, detail_suffix: &str) {
        if let Ok(mut data) = CHAT.write() {
            if let Some(msg) = data
                .messages
                .iter_mut()
                .rev()
                .find(|m| m.tool_id.as_deref() == Some(id))
            {
                msg.text = title.into();
                if let Some(detail) = &mut msg.detail {
                    detail.push_str(detail_suffix);
                }
            }
        }
    }

    /// Toggle one tool chip open/closed (list index).
    pub fn toggle_tool(index: usize) {
        if let Ok(mut data) = CHAT.write() {
            if let Some(msg) = data.messages.get_mut(index) {
                if msg.role == ChatRole::Tool {
                    msg.expanded = !msg.expanded;
                }
            }
        }
    }

    /// Land the streamed reply as a message. Returns the number of items the
    /// list should scroll to.
    pub fn end_stream() -> usize {
        let Ok(mut data) = CHAT.write() else { return 0 };
        let text = std::mem::take(&mut data.streaming_text);
        if !text.trim().is_empty() {
            let meta = data.rate.final_label();
            let mut msg = ChatMessage::plain(ChatRole::Assistant, text);
            msg.meta = meta;
            data.messages.push(msg);
        }
        data.rate.reset();
        data.is_streaming = false;
        data.activity.clear();
        data.activity_shown_at = None;
        data.activity_clear_pending = false;
        data.thinking_text.clear();
        data.messages.len()
    }

    /// The one-line readout for the status area while a reply streams:
    /// `78 tok/s`, plus lane contention when the box runs more than one
    /// lane. `None` when nothing is arriving.
    pub fn live_rate_label() -> Option<String> {
        let data = CHAT.read().ok()?;
        data.is_streaming
            .then(|| data.rate.live_label(std::time::Instant::now()))
            .flatten()
    }

    /// The live reasoning tail, for the porthole under the think dots.
    pub fn set_thinking_text(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            if data.thinking_text != text {
                data.thinking_text = text.to_string();
            }
        }
    }

    /// Minimum time a just-shown activity stays on screen. Clears inside
    /// the window are DEFERRED (applied on the next set/draw after it), so
    /// the "thinking" chip never flashes in and out within a fraction of a
    /// second — the flicker reads as a glitch, not a status.
    pub fn set_activity(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            let now = std::time::Instant::now();
            if text.is_empty() {
                // A clear only lands once the current status had its beat.
                match data.activity_shown_at {
                    Some(at) if now.duration_since(at) < ACTIVITY_MIN_SHOWN => {
                        data.activity_clear_pending = true;
                    }
                    _ => {
                        data.activity.clear();
                        data.activity_shown_at = None;
                        data.activity_clear_pending = false;
                    }
                }
            } else {
                if data.activity != text {
                    data.activity = text.to_string();
                }
                if data.activity_shown_at.is_none() {
                    data.activity_shown_at = Some(now);
                }
                data.activity_clear_pending = false;
            }
        }
    }

    pub fn activity() -> String {
        CHAT.read().map(|d| d.activity.clone()).unwrap_or_default()
    }

    pub fn set_status(text: impl Into<String>) {
        if let Ok(mut data) = CHAT.write() {
            data.status = text.into();
        }
    }

    pub fn status() -> String {
        CHAT.read().map(|d| d.status.clone()).unwrap_or_default()
    }

    pub fn is_streaming() -> bool {
        CHAT.read().map(|d| d.is_streaming).unwrap_or(false)
    }

    pub fn item_count() -> usize {
        match CHAT.read() {
            Ok(data) => data.messages.len() + data.is_streaming as usize,
            Err(_) => 0,
        }
    }

    /// Wipe the conversation (the Clear control). The session itself is
    /// retired by the feed; this is only what is on screen.
    pub fn clear() {
        if let Ok(mut data) = CHAT.write() {
            data.messages.clear();
            data.streaming_text.clear();
            data.activity.clear();
            data.activity_shown_at = None;
            data.activity_clear_pending = false;
            data.thinking_text.clear();
            data.is_streaming = false;
            data.last_delta = None;
            data.rate.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chat model is a process-global; tests must not interleave.
    pub static CHAT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear() {
        ChatData::clear();
        ChatData::set_status("");
    }

    #[test]
    fn a_turn_streams_then_lands_as_one_message() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::push(ChatRole::User, "make it rain");
        ChatData::begin_stream();
        assert_eq!(ChatData::item_count(), 2, "the streaming bubble counts");

        ChatData::push_delta("Adding ");
        ChatData::push_delta("rain!");
        assert_eq!(CHAT.read().unwrap().streaming_text, "Adding rain!");

        let count = ChatData::end_stream();
        assert_eq!(count, 2);
        let data = CHAT.read().unwrap();
        assert!(!data.is_streaming);
        assert_eq!(data.messages[1].role, ChatRole::Assistant);
        assert_eq!(data.messages[1].text, "Adding rain!");
        drop(data);
        clear();
    }

    #[test]
    fn an_empty_reply_does_not_leave_a_blank_bubble() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::begin_stream();
        ChatData::push_delta("   \n ");
        ChatData::end_stream();
        assert!(
            CHAT.read().unwrap().messages.is_empty(),
            "whitespace-only replies are dropped"
        );
        clear();
    }

    /// A synthetic stream at a known speed must read back as that speed —
    /// from the service's token counts, not from how many deltas arrived.
    #[test]
    fn the_rate_reads_the_services_token_count_not_the_delta_count() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        // 20 tokens per poll, one poll every 200ms = 100 tok/s. The deltas
        // are fat and few, exactly as a partial_text diff stream is.
        for i in 0..10u32 {
            meter.record(
                t0 + Duration::from_millis(200 * i as u64),
                80,
                Some(20 * (i + 1)),
                None,
                None,
                None,
            );
        }
        let now = t0 + Duration::from_millis(200 * 9);
        let live = meter.live(now).expect("a live rate");
        assert!((live - 100.0).abs() < 1.0, "{live}");
        assert_eq!(meter.label(live), "100 tok/s", "an exact count shows no ~");
        let avg = meter.average().expect("an average");
        assert!((avg - 100.0).abs() < 1.0, "{avg}");
    }

    /// Waiting for the first token is not slow generation: the clock starts
    /// at the first delta, so a long prefill never drags the number down.
    #[test]
    fn first_token_latency_stays_out_of_the_rate() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        // Ten seconds of silence, then 100 tokens in one second.
        meter.record(t0 + Duration::from_secs(10), 40, Some(10), None, None, None);
        meter.record(t0 + Duration::from_millis(10_500), 200, Some(60), None, None, None);
        meter.record(t0 + Duration::from_secs(11), 200, Some(110), None, None, None);
        let avg = meter.average().expect("an average");
        assert!((avg - 100.0).abs() < 1.0, "{avg}");
    }

    /// Each tool round is a new job on the box, so its counter restarts at
    /// zero. A restart is a restart, never a negative burst.
    #[test]
    fn a_restarted_round_counter_does_not_break_the_rate() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        meter.record(t0, 40, Some(50), None, None, None);
        meter.record(t0 + Duration::from_secs(1), 40, Some(100), None, None, None);
        // New round: the count drops back to 10.
        meter.record(t0 + Duration::from_secs(2), 40, Some(10), None, None, None);
        let live = meter.live(t0 + Duration::from_secs(2)).expect("a rate");
        assert!(live > 0.0, "a restart must not make the rate negative: {live}");
    }

    /// Without a service-side count the meter estimates from bytes — and
    /// SAYS it estimated.
    #[test]
    fn an_estimated_rate_is_marked_as_one() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        for i in 0..5u64 {
            // 40 bytes every 100ms ~= 10 tokens per 100ms = 100 tok/s.
            meter.record(t0 + Duration::from_millis(100 * i), 40, None, None, None, None);
        }
        let live = meter.live(t0 + Duration::from_millis(400)).expect("a rate");
        assert!((live - 100.0).abs() < 5.0, "{live}");
        assert_eq!(meter.label(live), "~100 tok/s");
    }

    /// A stalled stream (a tool is running) has no current rate at all.
    #[test]
    fn a_stalled_stream_reports_no_live_rate() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        meter.record(t0, 40, Some(10), None, None, None);
        meter.record(t0 + Duration::from_millis(500), 40, Some(60), None, None, None);
        assert!(meter.live(t0 + Duration::from_millis(600)).is_some());
        assert!(
            meter.live(t0 + Duration::from_secs(30)).is_none(),
            "a frozen number is worse than none"
        );
        // The whole-message average survives the stall: it is a fact about
        // the reply, not about now.
        assert!(meter.average().is_some());
    }

    /// Lane contention is shown only when there is contention to show: a
    /// box with one lane (or one that advertises none) says nothing.
    #[test]
    fn lanes_show_only_when_the_box_has_more_than_one() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        meter.record(t0, 40, Some(10), None, None, None);
        meter.record(t0 + Duration::from_secs(1), 40, Some(110), None, None, None);
        assert_eq!(meter.final_label().unwrap(), "100 tok/s");

        let mut meter = RateMeter::new();
        meter.record(t0, 40, Some(10), Some((1, 1)), None, None);
        meter.record(t0 + Duration::from_secs(1), 40, Some(110), Some((1, 1)), None, None);
        assert_eq!(meter.final_label().unwrap(), "100 tok/s", "1/1 is not news");

        let mut meter = RateMeter::new();
        meter.record(t0, 40, Some(10), Some((3, 4)), None, None);
        meter.record(t0 + Duration::from_secs(1), 40, Some(110), Some((3, 4)), None, None);
        assert_eq!(meter.final_label().unwrap(), "100 tok/s · 3/4 lanes");
    }

    /// While the model reasons in its hidden think block the meter says so
    /// instead of showing a rate for text nobody can see; the first visible
    /// token flips it back to a number.
    #[test]
    fn thinking_reads_as_thinking_not_a_rate() {
        let mut meter = RateMeter::new();
        let t0 = Instant::now();
        meter.record(t0, 40, Some(10), None, Some(10), None);
        meter.record(t0 + Duration::from_secs(1), 40, Some(24), None, Some(24), None);
        let live = meter.live_label(t0 + Duration::from_secs(1)).unwrap();
        assert!(live.ends_with("thinking"), "GPU rate + state: {live}");
        meter.record(t0 + Duration::from_secs(2), 40, Some(110), None, Some(24), Some(4));
        let label = meter.final_label().unwrap();
        assert!(label.ends_with("tok/s"), "visible text drops the annotation: {label}");
    }

    /// The finished reply keeps its own number; the next segment starts
    /// from scratch.
    #[test]
    fn the_landed_message_pins_its_own_average() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::begin_stream();
        let t0 = Instant::now();
        {
            let mut data = CHAT.write().unwrap();
            data.rate.record(t0, 40, Some(10), None, None, None);
            data.rate.record(t0 + Duration::from_secs(1), 40, Some(110), None, None, None);
            data.streaming_text.push_str("done");
        }
        ChatData::end_stream();
        let data = CHAT.read().unwrap();
        assert_eq!(data.messages[0].meta.as_deref(), Some("100 tok/s"));
        assert!(data.rate.average().is_none(), "the meter starts clean");
        drop(data);
        clear();
    }

    #[test]
    fn errors_are_injected_as_system_messages() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::push(ChatRole::System, "game.splash:4: unknown verb 'wobble'");
        let data = CHAT.read().unwrap();
        assert_eq!(data.messages[0].role, ChatRole::System);
        assert!(data.messages[0].text.contains("wobble"));
        drop(data);
        clear();
    }

    #[test]
    fn activity_rides_along_with_the_streaming_reply() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::begin_stream();
        ChatData::set_activity("Changing the game");
        assert_eq!(CHAT.read().unwrap().activity, "Changing the game");
        // Completing the turn clears it: a finished reply must not keep
        // claiming the AI is still working.
        ChatData::push_delta("Done!");
        ChatData::end_stream();
        assert!(CHAT.read().unwrap().activity.is_empty());
        clear();
    }

    /// A tool chip is completed in place by call id, and the visible text
    /// said before it lands as its own message.
    #[test]
    fn a_tool_chip_completes_in_place() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::begin_stream();
        ChatData::set_stream_text("Looking that up.");
        ChatData::push_tool("call-1", "queried: SELECT 1 — running…", "args: {}\n");
        ChatData::finish_tool("call-1", "queried → 3 rows", "\nname\n…");
        let data = CHAT.read().unwrap();
        assert_eq!(data.messages[0].role, ChatRole::Assistant);
        assert_eq!(data.messages[1].role, ChatRole::Tool);
        assert_eq!(data.messages[1].text, "queried → 3 rows");
        assert!(data.messages[1].detail.as_deref().unwrap().contains("name"));
        drop(data);
        clear();
    }

    /// A status that appeared a moment ago does not blink out again: the
    /// clear is deferred past the minimum display time, because a chip that
    /// flashes in and out reads as a glitch.
    #[test]
    fn a_just_shown_activity_survives_an_immediate_clear() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::set_activity("thinking…");
        ChatData::set_activity("");
        {
            let data = CHAT.read().unwrap();
            assert_eq!(data.activity, "thinking…", "the clear must wait its turn");
            assert!(data.activity_clear_pending);
        }
        // Past the window, a clear lands immediately.
        std::thread::sleep(ACTIVITY_MIN_SHOWN + Duration::from_millis(20));
        ChatData::set_activity("");
        {
            let data = CHAT.read().unwrap();
            assert!(data.activity.is_empty());
            assert!(!data.activity_clear_pending);
        }
        clear();
    }

    /// Clear wipes what is on screen, including a half-streamed reply.
    #[test]
    fn clear_wipes_the_conversation() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::push(ChatRole::User, "hi");
        ChatData::begin_stream();
        ChatData::set_stream_text("hel");
        ChatData::clear();
        let data = CHAT.read().unwrap();
        assert!(data.messages.is_empty());
        assert!(data.streaming_text.is_empty());
        assert!(!data.is_streaming);
        drop(data);
        clear();
    }
}
