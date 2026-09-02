//! What a chat panel draws: the engine's transcript and status, as plain
//! data. The engine writes it, the panel reads it; nothing here has a side
//! effect. It sits outside the `engine` feature on purpose — a panel crate
//! links only this and the wire, and a host that owns the engine hands the
//! panel a reference to its state each frame.

use crate::wire::ToolOutcome;

/// Where the conversation is right now.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Status {
    #[default]
    Idle,
    /// The model is coming up: weights streaming, prefix prefilling.
    Loading { phase: String, fraction: f64 },
    /// A turn is in flight and the model has not started writing yet.
    Thinking,
    /// The model is writing.
    Streaming,
    /// A tool is running and the model waits for it.
    WaitingForTool,
    /// Something the person must read; the next send clears it.
    Error(String),
}

impl Status {
    pub fn is_busy(&self) -> bool {
        matches!(self, Status::Thinking | Status::Streaming | Status::WaitingForTool)
    }
}

/// Which model answers. Slugs, so the choice can be persisted and shown
/// without the hub's types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderChoice {
    /// The machine's own model through the hub election.
    Local,
    /// A cloud or CLI provider by its hub slug (`claude-cli`, `claude-api`, `openai`, …).
    Cloud(String),
}

impl ProviderChoice {
    pub fn slug(&self) -> String {
        match self {
            ProviderChoice::Local => "local".to_string(),
            ProviderChoice::Cloud(s) => s.clone(),
        }
    }

    pub fn from_slug(s: &str) -> ProviderChoice {
        match s.trim() {
            "" | "local" => ProviderChoice::Local,
            other => ProviderChoice::Cloud(other.to_string()),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, ProviderChoice::Local)
    }
}

/// One provider the person may pick, as the chip's menu lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderRow {
    pub choice: ProviderChoice,
    /// `Local · Qwen3.5 9B`, `Claude (CLI)`.
    pub label: String,
    /// Why it cannot be picked right now (locked out, not installed);
    /// `None` when selectable.
    pub unavailable: Option<String>,
}

/// One connected (or known) app, for the apps row.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceInfo {
    pub id: String,
    pub label: String,
    /// Its port is up and its tools are in the table.
    pub connected: bool,
    /// The host can bring it up (`os.launch`) when it is not connected.
    pub launchable: bool,
    pub tool_count: usize,
}

/// A tool card's life.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolStatus {
    /// Destructive: parked until the person clicks Run or Cancel.
    Confirm,
    Running { note: String, permille: u16 },
    Done { outcome: ToolOutcome, note: String, text: String },
}

impl ToolStatus {
    pub fn is_done(&self) -> bool {
        matches!(self, ToolStatus::Done { .. })
    }
}

/// One tool call in the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolEntry {
    pub call_id: String,
    pub service: String,
    pub service_label: String,
    /// Short name (`plan`).
    pub tool: String,
    /// What the card says: `Route · plan  Dam → Utrecht`.
    pub title: String,
    /// The argument JSON, for the expanded view.
    pub args: String,
    pub status: ToolStatus,
    /// Draw the service's live preview under the card.
    pub preview: bool,
    /// The person opened the details.
    pub expanded: bool,
}

/// One line of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User { text: String },
    /// `streaming` while deltas still arrive; the panel draws the landed
    /// text with markup and the streaming text plain.
    Assistant { text: String, streaming: bool },
    Tool(ToolEntry),
    /// The engine talking: "Route connected", an error, a refusal.
    System { text: String },
}

/// Everything the panel draws. `generation` bumps on every change so a
/// panel can skip work when nothing moved.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EngineState {
    pub entries: Vec<Entry>,
    pub status: Status,
    /// Tokens per second of the current or last answer, when known.
    pub rate: Option<f32>,
    /// The model's own reasoning text while it thinks, bounded; empty
    /// when the provider shows none.
    pub thinking: String,
    pub provider: ProviderChoice,
    pub provider_label: String,
    pub providers: Vec<ProviderRow>,
    pub local_only: bool,
    pub services: Vec<ServiceInfo>,
    pub generation: u64,
}

impl Default for ProviderChoice {
    fn default() -> Self {
        ProviderChoice::Local
    }
}

/// The most transcript entries kept. A conversation is ephemeral; a
/// transcript that grows without limit is a leak with a scrollbar.
pub const MAX_ENTRIES: usize = 600;
/// Bytes of reasoning text shown at once.
pub const MAX_THINKING_BYTES: usize = 400;

impl EngineState {
    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            let cut = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..cut);
        }
        self.generation += 1;
    }

    pub fn touch(&mut self) {
        self.generation += 1;
    }

    pub fn tool_mut(&mut self, call_id: &str) -> Option<&mut ToolEntry> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool(t) if t.call_id == call_id => Some(t),
            _ => None,
        })
    }

    /// The assistant entry still being written, if any.
    pub fn streaming_mut(&mut self) -> Option<&mut String> {
        match self.entries.last_mut() {
            Some(Entry::Assistant { text, streaming: true }) => Some(text),
            _ => None,
        }
    }

    pub fn service(&self, id: &str) -> Option<&ServiceInfo> {
        self.services.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transcript_is_bounded_and_generation_moves() {
        let mut s = EngineState::default();
        for i in 0..(MAX_ENTRIES + 5) {
            s.push(Entry::User { text: i.to_string() });
        }
        assert_eq!(s.entries.len(), MAX_ENTRIES);
        assert_eq!(s.generation, (MAX_ENTRIES + 5) as u64);
        assert!(matches!(&s.entries[0], Entry::User { text } if text == "5"));
    }

    #[test]
    fn tool_lookup_finds_the_latest_card_with_that_id() {
        let mut s = EngineState::default();
        let card = |id: &str| {
            Entry::Tool(ToolEntry {
                call_id: id.into(),
                service: "route".into(),
                service_label: "Route".into(),
                tool: "plan".into(),
                title: "Route · plan".into(),
                args: "{}".into(),
                status: ToolStatus::Running { note: String::new(), permille: 0 },
                preview: true,
                expanded: false,
            })
        };
        s.push(card("a"));
        s.push(card("b"));
        assert!(s.tool_mut("b").is_some());
        assert!(s.tool_mut("zzz").is_none());
        assert!(s.streaming_mut().is_none());
        s.push(Entry::Assistant { text: "hel".into(), streaming: true });
        s.streaming_mut().unwrap().push_str("lo");
        assert!(matches!(s.entries.last(), Some(Entry::Assistant { text, .. }) if text == "hello"));
    }

    #[test]
    fn provider_slugs_round_trip() {
        assert_eq!(ProviderChoice::from_slug("local"), ProviderChoice::Local);
        assert_eq!(ProviderChoice::from_slug(""), ProviderChoice::Local);
        assert_eq!(ProviderChoice::from_slug("claude-cli").slug(), "claude-cli");
    }
}
