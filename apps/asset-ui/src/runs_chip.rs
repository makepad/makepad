//! The RUNS chip, the panel behind it, and the ONE card every spawned unit
//! of work is drawn as.
//!
//! All generation runs in THIS app now (aicore §9): the engine in
//! `pipeline.rs` talks to fleet boxes directly, and the store no longer has
//! a queue to poll. So this module owns [`RunCard`] — the card grammar of
//! F1 §5.7 (a title row, ONE aggregate bar, one compact stage strip, and a
//! fold holding the whole truth: sent prompts, params, box tags, errors) —
//! built from the local engine's runs and its waiting queue.
//!
//! The bar is never computed here: [`aggregate_permille`] is the client
//! crate's one implementation. What IS held here is the per-card high-water
//! mark — a stage retry legitimately re-starts one stage's bar, and a bar
//! that goes backwards reads as a bug even when it is honest.

use makepad_asset_client::{aggregate_permille, default_stage_weight};
use std::collections::HashMap;

use crate::pipeline::{format_clock, stage_display_name, Pipeline, StageState};

/// Longest prompt excerpt a card title carries.
const EXCERPT: usize = 60;

// ---------------------------------------------------------------------------
// The card grammar
// ---------------------------------------------------------------------------

/// Which spawned unit a card is; the panel's fold/cancel state keys on it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CardKey {
    /// A run of this app's own engine, by run id.
    Local(u64),
    /// A run waiting in this app's queue, by queue position.
    LocalQueued(usize),
}

impl CardKey {
    pub fn as_text(&self) -> String {
        match self {
            Self::Local(id) => format!("local:{id}"),
            Self::LocalQueued(index) => format!("queued:{index}"),
        }
    }
}

/// The five states a spawned unit reads as, in the order a list sorts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    Running,
    Queued,
    Done,
    Failed,
    Cancelled,
}

impl CardState {
    /// Running work first, then what is waiting, then what is over.
    fn rank(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Queued => 1,
            Self::Done | Self::Failed | Self::Cancelled => 2,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// The state dot's colour. ONE accent (#3d9bf0) and it belongs to the
    /// thing that is alive; everything else is grey, and only a failure
    /// earns red. The state WORD is always on the card too (row 2), so the
    /// dot is never the only signal.
    pub fn dot(self) -> [f32; 4] {
        match self {
            Self::Running => [0.239, 0.608, 0.941, 1.0],
            Self::Queued => [0.29, 0.32, 0.36, 1.0],
            Self::Done => [0.35, 0.42, 0.48, 1.0],
            Self::Failed => [0.851, 0.345, 0.310, 1.0],
            Self::Cancelled => [0.29, 0.32, 0.36, 1.0],
        }
    }

    /// The one bar's fill. Same hue family as the dot, for the same reason.
    pub fn fill(self) -> [f32; 4] {
        match self {
            Self::Running => [0.239, 0.608, 0.941, 1.0],
            Self::Queued => [0.20, 0.28, 0.36, 1.0],
            Self::Done => [0.173, 0.373, 0.533, 1.0],
            Self::Failed => [0.851, 0.345, 0.310, 1.0],
            Self::Cancelled => [0.29, 0.32, 0.36, 1.0],
        }
    }
}

/// One chip of the stage strip. No bar — row 2 is the only bar on the card.
#[derive(Clone, Debug, PartialEq)]
pub struct StageChip {
    /// `expand · 15s`, `music · 62%`, `publish`.
    pub text: String,
    pub tone: StageTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageTone {
    Pending,
    Running,
    Done,
    Failed,
    /// An `on_fail: skip` expander that failed — the run went on with the
    /// raw prompt, and that is a fact worth its own colour.
    Skipped,
}

impl StageTone {
    pub fn color(self) -> [f32; 4] {
        match self {
            Self::Pending => [0.35, 0.38, 0.42, 1.0],
            Self::Running => [0.239, 0.608, 0.941, 1.0],
            Self::Done => [0.60, 0.64, 0.69, 1.0],
            Self::Failed => [0.851, 0.345, 0.310, 1.0],
            Self::Skipped => [0.85, 0.65, 0.35, 1.0],
        }
    }
}

/// ONE spawned unit of work, however it was spawned. F1 §5.7 verbatim:
/// row 1 title, row 2 the only bar, row 3 the stage strip, row 4 the fold.
#[derive(Clone, Debug, PartialEq)]
pub struct RunCard {
    pub key: CardKey,
    pub state: CardState,
    /// Where this ran: the store, or this app's own engine.
    pub origin: &'static str,
    /// The preset / pipeline label.
    pub label: String,
    /// The words the PERSON typed, quoted and truncated. Never the
    /// expanded prompt — that lives in the fold.
    pub excerpt: String,
    /// `m:ss`, live while running, frozen when terminal. This REPLACES the
    /// "Done in 538.3s" banner.
    pub elapsed: String,
    /// The aggregate, 0..=1000, already high-water clamped.
    pub permille: u16,
    /// The humanized current-stage word, or the failure reason.
    pub status: String,
    /// Empty for a single-stage unit — a heading with nothing under it does
    /// not render, and neither does a strip of one.
    pub stages: Vec<StageChip>,
    /// The whole truth, only when the card is open.
    pub fold: String,
    /// Whether the × is offered (pending / running / publishing).
    pub can_cancel: bool,
    /// Whether this queued LOCAL run can still be moved up the queue.
    pub can_promote: bool,
    pub open: bool,
    /// Newest first inside a state bucket.
    created_ms: u64,
}

impl RunCard {
    pub fn percent(&self) -> u32 {
        (self.permille as u32 + 5) / 10
    }

    pub fn fraction(&self) -> f32 {
        (self.permille as f32 / 1000.0).clamp(0.0, 1.0)
    }

    /// `RUNS · 3 running · 61%` needs to know which cards are still work.
    pub fn is_active(&self) -> bool {
        !self.state.is_terminal()
    }
}

/// A run of this app's own engine, lent to the card builder.
pub struct LocalRun<'a> {
    pub id: u64,
    pub label: &'a str,
    pub prompt: &'a str,
    pub created_ms: u64,
    pub pipeline: &'a Pipeline,
}

/// A run waiting in this app's queue — spawned, visible, cancellable, but
/// nothing has been sent yet.
pub struct LocalQueued<'a> {
    pub index: usize,
    pub label: &'a str,
    pub prompt: &'a str,
}


// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

fn excerpt(prompt: &str) -> String {
    let one_line = prompt.replace('\n', " ");
    let trimmed = one_line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    format!("\u{201c}{}\u{201d}", crate::store_views::truncate(trimmed, EXCERPT))
}


/// The job-kind spelling a local stage domain corresponds to, so the ONE
/// shared weight table (`default_stage_weight`) covers local runs too and a
/// local card's bar cannot mean something different from a store card's.
pub fn local_stage_kind(domain: &str) -> &'static str {
    match domain {
        "text" => "text.expand",
        "image" => "image.generate",
        "edit" => "image.edit",
        "inpaint" => "image.inpaint",
        "control" => "image.control",
        "upscale" => "image.upscale",
        "video" => "video.generate",
        "enhance" => "video.enhance",
        "music" | "speech" | "sfx" => "music.generate",
        "mesh" | "splat" | "paint" | "rig" => "mesh.generate",
        _ => "",
    }
}

fn local_stage_weight(domain: &str) -> u16 {
    default_stage_weight(local_stage_kind(domain))
}

// ---------------------------------------------------------------------------
// Cards from this app's own engine
// ---------------------------------------------------------------------------

/// The local engine's per-stage share of the bar, in the same 0..=1000 the
/// store speaks — so `aggregate_permille` can weigh them together.
///
/// A FAILED stage keeps the fraction it died at. The old rendering filled it
/// to 1.0 and drew "100% · FAILED" beside it, which is the exact lie this
/// design exists to remove.
fn local_stage_permille(state: &StageState, progress: f64) -> u16 {
    match state {
        StageState::Done | StageState::AwaitingChoice => 1000,
        _ => (progress.clamp(0.0, 1.0) * 1000.0) as u16,
    }
}

fn local_chip(pipeline: &Pipeline, index: usize) -> StageChip {
    let stage = &pipeline.stages[index];
    let name = stage.domain.clone();
    let (tone, tail) = match &stage.state {
        StageState::Waiting => (StageTone::Pending, String::new()),
        StageState::Failed(_) => (StageTone::Failed, String::new()),
        StageState::Done | StageState::AwaitingChoice => (
            StageTone::Done,
            match (stage.started, stage.finished) {
                (Some(t0), Some(t1)) => format!(" \u{b7} {}", format_clock((t1 - t0).as_secs_f64())),
                _ => String::new(),
            },
        ),
        _ => (
            StageTone::Running,
            format!(" \u{b7} {}%", (stage.progress.clamp(0.0, 1.0) * 100.0).round() as u32),
        ),
    };
    StageChip { text: format!("{name}{tail}"), tone }
}

fn local_status(pipeline: &Pipeline) -> String {
    if let Some((index, error)) = pipeline.stages.iter().enumerate().find_map(|(i, s)| match &s.state
    {
        StageState::Failed(error) => Some((i, error.clone())),
        _ => None,
    }) {
        return format!(
            "failed at {} \u{2014} {}",
            pipeline.stages[index].domain,
            crate::store_views::truncate(error.trim(), 120)
        );
    }
    if !pipeline.is_running() {
        return "done".to_string();
    }
    let stage = &pipeline.stages[pipeline.current.min(pipeline.stages.len() - 1)];
    match &stage.state {
        StageState::Waiting if !stage.detail.is_empty() => stage.detail.clone(),
        StageState::Waiting => "queued".to_string(),
        StageState::FanOut | StageState::AwaitingChoice => stage.detail.clone(),
        StageState::Submitting => "submitting".to_string(),
        StageState::Polling if !stage.detail.is_empty() => stage.detail.clone(),
        StageState::Polling if !stage.service_state.is_empty() => stage.service_state.clone(),
        StageState::Polling => "rendering".to_string(),
        StageState::Fetching => "fetching artifacts".to_string(),
        StageState::Done => "done".to_string(),
        StageState::Failed(error) => error.clone(),
    }
}

/// The fold of a local run: every stage's inspect block, the SAME text the
/// RUNS surface's opened stage rows already show (`store_views::stage_detail`
/// is the one implementation), with the routing reasoning this engine knows
/// and nothing else does.
fn local_fold(run: &LocalRun) -> String {
    let mut out = format!("PROMPT\n{}\n", run.prompt);
    for (index, stage) in run.pipeline.stages.iter().enumerate() {
        out.push_str(&format!(
            "\n\u{2500}\u{2500} {} \u{b7} {} \u{b7} {}\n",
            index + 1,
            stage_display_name(&stage.domain),
            match &stage.state {
                StageState::Failed(_) => "failed",
                StageState::Done => "succeeded",
                StageState::Waiting => "pending",
                _ => "running",
            }
        ));
        if !stage.box_url.is_empty() {
            out.push_str(&format!(
                "MODEL\n{} @ {}\n\n",
                stage.model,
                stage.box_url.trim_start_matches("http://")
            ));
        }
        if !stage.reason.is_empty() {
            out.push_str(&format!("ROUTED\n{}\n\n", stage.reason));
        }
        out.push_str(&crate::store_views::stage_detail(stage));
        out.push_str("\n\n");
        if let StageState::Failed(error) = &stage.state {
            out.push_str(&format!("ERROR\n{error}\n\n"));
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn local_card(run: &LocalRun, open: bool) -> RunCard {
    let failed = run
        .pipeline
        .stages
        .iter()
        .any(|s| matches!(s.state, StageState::Failed(_)));
    let state = if failed {
        CardState::Failed
    } else if run.pipeline.is_running() {
        if run
            .pipeline
            .stages
            .iter()
            .all(|s| s.state == StageState::Waiting)
        {
            CardState::Queued
        } else {
            CardState::Running
        }
    } else {
        CardState::Done
    };
    let permille = aggregate_permille(run.pipeline.stages.iter().map(|stage| {
        (
            local_stage_weight(&stage.domain),
            local_stage_permille(&stage.state, stage.progress),
        )
    }));
    let elapsed: f64 = run
        .pipeline
        .stages
        .iter()
        .filter_map(|s| match (s.started, s.finished) {
            (Some(t0), Some(t1)) => Some((t1 - t0).as_secs_f64()),
            (Some(t0), None) => Some(t0.elapsed().as_secs_f64()),
            _ => None,
        })
        .sum();
    RunCard {
        key: CardKey::Local(run.id),
        state,
        origin: "LOCAL",
        label: run.label.to_string(),
        excerpt: excerpt(run.prompt),
        elapsed: format_clock(elapsed),
        permille,
        status: local_status(run.pipeline),
        stages: if run.pipeline.stages.len() > 1 {
            (0..run.pipeline.stages.len())
                .map(|index| local_chip(run.pipeline, index))
                .collect()
        } else {
            Vec::new()
        },
        fold: if open { local_fold(run) } else { String::new() },
        can_cancel: run.pipeline.is_running(),
        can_promote: false,
        open,
        created_ms: run.created_ms,
    }
}

fn queued_card(queued: &LocalQueued, open: bool) -> RunCard {
    RunCard {
        key: CardKey::LocalQueued(queued.index),
        state: CardState::Queued,
        origin: "LOCAL",
        label: queued.label.to_string(),
        excerpt: excerpt(queued.prompt),
        elapsed: String::new(),
        permille: 0,
        status: format!("waiting for a free slot \u{b7} #{}", queued.index + 1),
        stages: Vec::new(),
        fold: if open {
            format!(
                "not sent yet \u{2014} this run has not started\n\nPROMPT\n{}",
                queued.prompt
            )
        } else {
            String::new()
        },
        can_cancel: true,
        can_promote: queued.index > 0,
        open,
        created_ms: 0,
    }
}

// ---------------------------------------------------------------------------
// The chip line
// ---------------------------------------------------------------------------

/// The header chip beside SEARCHABLE. Always present: "nothing is running"
/// is itself an answer, and a chip that disappears cannot be clicked to see
/// what just finished.
pub fn chip_text(cards: &[RunCard]) -> String {
    let running = cards
        .iter()
        .filter(|card| card.state == CardState::Running)
        .count();
    let queued = cards
        .iter()
        .filter(|card| card.state == CardState::Queued)
        .count();
    if running == 0 && queued == 0 {
        return "RUNS \u{b7} idle".to_string();
    }
    // The percent covers everything still owed, queued runs included at 0 —
    // "how far is the work I fired off", not "how far is the busiest box".
    let permille = aggregate_permille(
        cards
            .iter()
            .filter(|card| card.is_active())
            .map(|card| (1u16, card.permille)),
    );
    if running == 0 {
        return format!("RUNS \u{b7} {queued} queued");
    }
    let mut line = format!("RUNS \u{b7} {running} running");
    if queued > 0 {
        line.push_str(&format!(" \u{b7} {queued} queued"));
    }
    line.push_str(&format!(" \u{b7} {}%", (permille as u32 + 5) / 10));
    line
}


// ---------------------------------------------------------------------------
// The chip state
// ---------------------------------------------------------------------------

/// Everything the app knows about work in flight — all of it local now.
#[derive(Default)]
pub struct RunsChip {
    /// The cards the person has unfolded.
    open: Vec<CardKey>,
    /// Per-card high-water mark. A stage retry honestly restarts one
    /// stage's bar; a bar that walks backwards reads as a broken app.
    high_water: HashMap<CardKey, u16>,
    pub panel_open: bool,
}

impl RunsChip {
    pub fn set_panel_open(&mut self, open: bool) {
        self.panel_open = open;
    }

    pub fn is_open(&self, key: &CardKey) -> bool {
        self.open.contains(key)
    }

    /// Unfold / refold one card.
    pub fn toggle_open(&mut self, key: &CardKey) {
        match self.open.iter().position(|held| held == key) {
            Some(at) => {
                self.open.remove(at);
            }
            None => self.open.push(key.clone()),
        }
    }

    /// Every spawned unit, one card each, in the one order: what is running,
    /// what is waiting, what is over — newest first inside each.
    pub fn cards(&mut self, local: &[LocalRun], queued: &[LocalQueued]) -> Vec<RunCard> {
        let mut cards = Vec::new();
        for run in local {
            let key = CardKey::Local(run.id);
            cards.push(local_card(run, self.is_open(&key)));
        }
        for entry in queued {
            let key = CardKey::LocalQueued(entry.index);
            cards.push(queued_card(entry, self.is_open(&key)));
        }

        // The high-water mark, held per card: a stage that retries restarts
        // its own bar honestly, and the aggregate must still not walk back.
        for card in &mut cards {
            let seen = self.high_water.entry(card.key.clone()).or_insert(0);
            *seen = (*seen).max(card.permille);
            card.permille = *seen;
        }
        let live: Vec<CardKey> = cards.iter().map(|card| card.key.clone()).collect();
        self.high_water.retain(|key, _| live.contains(key));

        cards.sort_by(|a, b| {
            a.state
                .rank()
                .cmp(&b.state.rank())
                .then(b.created_ms.cmp(&a.created_ms))
        });
        cards
    }
}
