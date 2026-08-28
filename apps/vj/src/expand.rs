//! The DREAM run's first stage: turning the operator's few words into a
//! full generation brief, on the Asset Server's chat broker.
//!
//! Why not a job? Because there is no expander JOB. The store's queue has
//! twenty generation kinds and none of them is text: `gen_kinds.rs` says so
//! in as many words ("`text` (the prompt expander) and `chat` remain absent
//! entirely"), and the coordinator that turns a job body into a fleet
//! request forwards no expander field at all. The `expand: true` the older
//! VJ pipes have always put in their job bodies is read by NOTHING — those
//! pipes have never expanded a prompt in their lives. The asset UI gets a
//! real expansion only because it hosts the worker and can POST straight at
//! a fleet box; a thin client like this one cannot, and should not — node
//! addresses are deliberately private.
//!
//! What IS reachable is the broker: a session, a message, a stream of
//! deltas, served by whichever box the server picks. So the expansion is a
//! single throwaway conversation with the expander's own instructions in
//! it. Deliberately NOT the VJ chat profile — that one comes with the
//! performer context and a tool surface, and this turn must be one model
//! answering one question, with no tools and nothing remembered.
//!
//! LAW, and the whole reason this file is careful: a failed expansion never
//! costs the operator their run. Every path out of here that has no text
//! returns `None` with a reason, and the caller queues the RAW prompt.

use makepad_asset_client::{
    Api, ApiEndpoints, ChatCreateRequest, ChatEventBodyDto, ChatProviderKind, ChatSendRequest,
    ChatTranscriptRole, HttpLimits,
};
use std::sync::mpsc::{channel, Receiver, Sender};

/// Whole-expansion budget. Past this the run goes ahead on the raw prompt:
/// an operator mid-set would rather have a clip from their own words than a
/// better clip two minutes later.
const DEADLINE_MS: u64 = 45_000;
/// One long-poll of the event stream.
const WAIT_MS: u64 = 4_000;
/// Longest expansion kept (the job body's own prompt cap is 4000).
const MAX_EXPANDED: usize = 1_600;

/// How many expansions may run at once. The operator fires prompts in
/// bursts; each one is a session and a thread, and the broker is shared
/// with the drawer's own chat.
pub const MAX_IN_FLIGHT: usize = 3;

/// The expander's instructions.
///
/// A near-copy of the fleet's own `expand_video.txt` intent, restated here
/// because the broker path cannot select that file: it wraps the turn as
/// chat, so the system prompt has to travel in the message. Kept short and
/// absolute — the model must answer with the prompt and nothing else, or
/// the "expansion" is a paragraph of chat that would be rendered literally.
const INSTRUCTION: &str = "\
You are a prompt expander for a video generation model. Expand the brief \
below into ONE flowing paragraph of 60-100 words describing a single \
continuous shot: name the subject and its materials and colours, the \
setting, the lighting (source, direction, colour temperature), the mood, \
and the camera (angle, distance, lens feel) and how it moves. Keep the \
motion continuous and one-directional. Do not add negative prompts, \
weights, parentheses syntax, resolution numbers, camera-brand names or \
model settings. Do not use lists or headings. Answer with ONLY the \
expanded prompt text — no preamble, no quotes, no explanation.\n\n\
Brief: ";

/// One finished expansion, matched back to its row by `tag`.
pub struct ExpandDone {
    pub tag: u64,
    /// `None` = use the raw prompt (see the law above).
    pub expanded: Option<String>,
    /// Why there is no text, in the row's words.
    pub note: Option<String>,
}

/// Owns the completion channel; the host polls it each tick.
pub struct Expander {
    tx: Sender<ExpandDone>,
    rx: Receiver<ExpandDone>,
    in_flight: usize,
}

impl Default for Expander {
    fn default() -> Self {
        let (tx, rx) = channel();
        Expander { tx, rx, in_flight: 0 }
    }
}

impl Expander {
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Start one expansion on its own thread. Returns false when the cap is
    /// full — the caller then runs on the raw prompt rather than queueing
    /// behind an expander, because the point of the cap is that the clip
    /// still gets made.
    pub fn start(
        &mut self,
        endpoints: ApiEndpoints,
        token: Option<String>,
        tag: u64,
        prompt: String,
    ) -> bool {
        if self.in_flight >= MAX_IN_FLIGHT {
            return false;
        }
        self.in_flight += 1;
        let tx = self.tx.clone();
        // Named so a stack in a crash report says which thread this is.
        let spawned = std::thread::Builder::new()
            .name("vj-expand".to_string())
            .spawn(move || {
                let (expanded, note) = run_one(endpoints, token, &prompt);
                let _ = tx.send(ExpandDone { tag, expanded, note });
            });
        if spawned.is_err() {
            self.in_flight -= 1;
            return false;
        }
        true
    }

    /// Everything that finished since the last call.
    pub fn drain(&mut self) -> Vec<ExpandDone> {
        let mut out = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            out.push(done);
        }
        out
    }
}

/// One blocking expansion. Never panics, never returns an empty string as
/// if it were an answer.
fn run_one(
    endpoints: ApiEndpoints,
    token: Option<String>,
    prompt: &str,
) -> (Option<String>, Option<String>) {
    let fail = |what: &str| (None, Some(format!("expander {what} — using the prompt as typed")));
    let api = match Api::new(endpoints, HttpLimits::default_v1(), token) {
        Ok(api) => api,
        Err(error) => return fail(&format!("unreachable ({error})")),
    };
    // An ephemeral session, NOT a keyed one: a keyed session would remember
    // every previous expansion and start blending one prompt into the next.
    let create = ChatCreateRequest::new("gen", ChatProviderKind::FleetQwen);
    let session = match api.chat_create(&create) {
        Ok(session) => session,
        Err(error) => return fail(&format!("session failed ({error})")),
    };
    let id = session.session;
    let message = format!("{INSTRUCTION}{prompt}");
    if let Err(error) = api.chat_send(&id, &ChatSendRequest::text(message)) {
        let _ = api.chat_retire(&id);
        return fail(&format!("send failed ({error})"));
    }
    let started = std::time::Instant::now();
    let mut cursor = 0u64;
    // Deltas are the FALLBACK. They carry a thinking model's reasoning as
    // well as its answer — a live run came back with "The user wants me to
    // expand a brief… Let me craft this carefully." and rendered THAT — so
    // the transcript, which the broker returns with thinking stripped, is
    // what the answer is actually read from below.
    let mut text = String::new();
    let outcome = loop {
        if started.elapsed().as_millis() as u64 > DEADLINE_MS {
            break Some("timed out".to_string());
        }
        let page = match api.chat_events(&id, cursor, WAIT_MS, 64) {
            Ok(page) => page,
            // One transport blip is not a failed expansion; the deadline
            // above is what ends this loop.
            Err(_) => continue,
        };
        cursor = page.cursor;
        let mut done = None;
        for event in page.events {
            match event.body {
                ChatEventBodyDto::Delta { text: delta, .. } => text.push_str(&delta),
                ChatEventBodyDto::Done => done = Some(None),
                ChatEventBodyDto::Cancelled => done = Some(Some("was cancelled".to_string())),
                ChatEventBodyDto::Error { code, message } => {
                    done = Some(Some(format!("failed ({code}: {message})")))
                }
                _ => {}
            }
        }
        if let Some(reason) = done {
            break reason;
        }
    };
    // The broker's own transcript: thinking already stripped, one row per
    // turn. Only used when it actually has an assistant row — a provider
    // that returns nothing here still gets the delta stream's version.
    let answer = api
        .chat_transcript(&id)
        .ok()
        .and_then(|rows| {
            rows.iter()
                .rev()
                .find(|row| row.role == ChatTranscriptRole::Assistant)
                .map(|row| row.text.clone())
        })
        .filter(|answer| !answer.trim().is_empty())
        .unwrap_or(text);
    let _ = api.chat_retire(&id);
    let cleaned = clean(&answer);
    match (cleaned, outcome) {
        (Some(text), _) => (Some(text), None),
        (None, Some(reason)) => fail(&reason),
        (None, None) => fail("answered with nothing"),
    }
}

/// Strip the model's thinking and any wrapper the instruction asked it not
/// to add, then bound the length. Returns `None` for anything that is not a
/// usable prompt, so an empty or think-only answer falls back to the raw
/// prompt instead of generating from an empty string.
fn clean(raw: &str) -> Option<String> {
    // ONE LINE, no control characters. This is not cosmetic: the prompt is
    // published as the asset's annotation, and the store refuses control
    // characters there — a brief with a newline in it rendered a picture
    // that then died at publish with "annotation control chars", losing the
    // whole run at the last step.
    let mut text: String = raw
        .chars()
        .map(|c| if c.is_control() || c.is_whitespace() { ' ' } else { c })
        .collect();
    while text.contains("  ") {
        text = text.replace("  ", " ");
    }
    // Thinking models emit <think>…</think>; a broker that already strips
    // it changes nothing here.
    while let Some(start) = text.find("<think>") {
        match text[start..].find("</think>") {
            Some(end) => {
                let end = start + end + "</think>".len();
                text.replace_range(start..end, "");
            }
            // Unclosed: everything after it is thinking.
            None => text.truncate(start),
        }
    }
    let mut text = text.trim().to_string();
    // A model that ignored "no quotes" wraps the whole answer in them.
    for quote in ['"', '\''] {
        if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
            text = text[1..text.len() - 1].trim().to_string();
        }
    }
    if text.len() > MAX_EXPANDED {
        // The largest char boundary that fits. Slicing straight at
        // MAX_EXPANDED would panic the moment a multi-byte character
        // straddled it — and model prose is full of em dashes and curly
        // quotes, so that is a matter of when, not if.
        let boundary = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= MAX_EXPANDED)
            .last()
            .unwrap_or(0);
        // Cut at the last sentence that fits, so the brief never ends
        // mid-clause (a truncated clause reads as a different instruction).
        let cut = text[..boundary].rfind(". ").map(|i| i + 1).unwrap_or(boundary);
        text.truncate(cut);
        text = text.trim().to_string();
    }
    // Too short to be an expansion: the raw prompt is at least the
    // operator's own words.
    (text.chars().count() >= 24).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_and_quotes_come_off_and_junk_is_refused() {
        let expanded = clean(
            "<think>the user wants a fish</think>\"A chrome koi drifts through \
             a flooded cathedral, lit from above by cold shafts of light.\"",
        )
        .expect("a usable prompt");
        assert!(expanded.starts_with("A chrome koi"), "{expanded}");
        assert!(!expanded.ends_with('"'), "{expanded}");
        // Nothing but thinking is not an answer.
        assert_eq!(clean("<think>hmm</think>"), None);
        assert_eq!(clean("   "), None);
        // Too terse to be an expansion.
        assert_eq!(clean("a fish"), None);
        // An unclosed think block does not leak into the prompt.
        assert_eq!(clean("<think>still going and going and going"), None);
    }

    /// The cut must survive multi-byte prose. `String::truncate` and a
    /// bare slice both take BYTE indices, and an expansion that lands a
    /// curly quote across the limit would otherwise panic the app.
    #[test]
    fn a_long_expansion_full_of_punctuation_does_not_panic() {
        // "…" is three bytes, so the byte limit lands mid-character for
        // most repeat counts.
        for pad in 0..8 {
            let long = format!("{}{}", "x".repeat(pad), "an em—dash and a “quote” ".repeat(200));
            let cut = clean(&long).expect("still usable");
            assert!(cut.len() <= MAX_EXPANDED, "{}", cut.len());
            // Whatever came back is still valid UTF-8 prose, not a shard.
            assert!(cut.chars().count() > 0);
        }
    }

    /// The bug that killed a real run at the LAST step: a brief with a
    /// newline in it renders fine and then fails to publish, because the
    /// store refuses control characters in an annotation.
    #[test]
    fn a_brief_is_flattened_to_one_line_with_no_control_characters() {
        let messy = "A chrome koi drifts\n\nthrough a flooded cathedral,\tlit from above.";
        let cleaned = clean(messy).expect("a usable prompt");
        assert!(!cleaned.chars().any(char::is_control), "{cleaned:?}");
        assert!(!cleaned.contains("  "), "{cleaned:?}");
        assert!(cleaned.starts_with("A chrome koi drifts through"), "{cleaned:?}");
    }

    #[test]
    fn a_long_expansion_is_cut_on_a_sentence() {
        let long = format!("{} Final clause that will not fit.", "A ".repeat(1_200));
        let cut = clean(&long).expect("still usable");
        assert!(cut.len() <= MAX_EXPANDED, "{}", cut.len());
        assert!(!cut.contains("Final clause"));
    }
}
