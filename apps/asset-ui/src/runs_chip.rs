//! The global RUNS chip, the panel behind it, and the ONE card every
//! spawned unit of work is drawn as.
//!
//! Three engines put work in flight from this app and, until now, each drew
//! its own progress story: the store's PIPELINE records (a declared graph of
//! stage jobs, `POST /v1/pipelines`), standalone STORE JOBS (a one-shot
//! generate, a vision describe), and the app's own LOCAL engine
//! (`pipeline.rs`, which talks to fleet boxes directly and never touches the
//! job queue). A person who fires something off does not care which of the
//! three caught it — they asked for one thing and want to see it, stop it,
//! and read what was actually sent.
//!
//! So this module owns exactly two things:
//!
//! * a poll thread holding a stateless [`Api`] handle, which reads the
//!   pipeline listing, the details of the runs that are still moving, and
//!   the standalone job listing — and which executes cancels between polls,
//!   so a cancel never waits for a sleep to end;
//! * [`RunCard`] — the card grammar of F1 §5.7, built the SAME way from all
//!   three sources: a title row, ONE aggregate bar, one compact stage strip,
//!   and a fold holding the whole truth (sent prompts, params, declared
//!   bodies, box tags, job ids, attempts, errors).
//!
//! The bar is never computed here: [`aggregate_permille`] is the client
//! crate's one implementation, shared with the server's own derivation, so a
//! local run and a store pipeline drawn side by side cannot disagree about
//! what 61% means. What IS held here is the per-card high-water mark — a
//! stage retry legitimately re-starts one stage's bar, and a bar that goes
//! backwards reads as a bug even when it is honest.

use makepad_asset_client::{
    aggregate_permille, default_stage_weight, Api, ApiEndpoints, HttpLimits, JobDetailDto, JobId,
    JobRowDto, JobStageDto, JobStateDto, PipelineDetailDto, PipelineId, PipelineRowDto,
    PipelineStageDto, PipelineStateDto,
};
use makepad_asset_client::json::Value;
use makepad_widgets::log;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pipeline::{format_clock, stage_display_name, Pipeline, StageState};

/// While the panel is open the numbers are being read, so they are fresh.
const POLL_OPEN: Duration = Duration::from_secs(1);
/// Closed, only the chip is on screen and 5 s is invisible to a person.
const POLL_CLOSED: Duration = Duration::from_secs(5);
/// Pipelines one listing asks for (active AND recently finished, so a run
/// that just ended is still readable where it was being watched).
const PIPELINE_PAGE: u64 = 24;
/// Standalone jobs one listing asks for, per state.
const JOB_PAGE: u64 = 40;
/// Details fetched per poll for runs that are still moving. A finished
/// run's detail is fetched once and kept — it cannot change again.
const DETAIL_BUDGET: usize = 8;
/// Open standalone-job cards whose full record is fetched per poll.
const JOB_DETAIL_BUDGET: usize = 4;
/// The annotation backlog is thousands of pending jobs and it has its own
/// chip (SEARCHABLE) two pixels away. Its RUNNING jobs are live work and do
/// show; its pending pile is counted, named once, and never listed.
const ANNOTATE_KIND: &str = "annotate.asset";
/// Prompt excerpt on the title row (F1 §5.7: the words the PERSON typed).
const EXCERPT: usize = 60;

// ---------------------------------------------------------------------------
// The card model — pure data, built the same way from all three engines
// ---------------------------------------------------------------------------

/// Which spawned unit a card is, and what its × has to talk to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CardKey {
    /// A store pipeline record (`pipe_…`).
    Pipeline(String),
    /// A standalone store job (`job_…`) that no pipeline owns.
    Job(String),
    /// A run of this app's own engine, by run id.
    Local(u64),
    /// A run waiting in this app's queue, by queue position.
    LocalQueued(usize),
}

impl CardKey {
    pub fn as_text(&self) -> String {
        match self {
            Self::Pipeline(id) => id.clone(),
            Self::Job(id) => id.clone(),
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

/// The word a person reads for a kind of work while it is happening.
fn kind_word(kind: &str) -> &'static str {
    match kind {
        "text.expand" => "expanding",
        "image.generate" | "image.edit" | "image.control" | "image.inpaint" => "rendering",
        "image.upscale" | "video.enhance" => "enhancing",
        "video.generate" => "rendering video",
        "music.generate" => "composing",
        "mesh.generate" => "meshing",
        "annotate.asset" | "vision.describe" => "describing",
        _ => "running",
    }
}

/// The short name a stage chip carries: the last word of the kind, which is
/// what the person picked ("image", "music", "expand").
fn stage_word(name: &str, kind: &str) -> String {
    if !name.is_empty() {
        return name.to_string();
    }
    match kind.split_once('.') {
        Some((domain, _)) => domain.to_string(),
        None => kind.to_string(),
    }
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

/// What a failure says on the card itself. Readable WITHOUT unfolding is the
/// whole rule — everything else diagnostic is folded, this is not.
fn failure_reason(body: &Value, outcome: &str) -> String {
    for key in ["error", "message", "reason", "detail"] {
        if let Some(text) = body.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return crate::store_views::truncate(text.trim(), 120);
            }
        }
    }
    if let Some(text) = body.as_str() {
        if !text.is_empty() {
            return crate::store_views::truncate(text.trim(), 120);
        }
    }
    if outcome.is_empty() {
        "no reason recorded".to_string()
    } else {
        outcome.to_string()
    }
}

/// A declared (not yet sent) body, as a person reads it: one `key: value`
/// per line, nested values as their compact document.
fn declared_lines(body: &Value) -> String {
    let mut out = String::new();
    if let Value::Obj(pairs) = body {
        for (key, value) in pairs {
            let rendered = match value.as_str() {
                Some(text) => text.to_string(),
                None => value.to_json(),
            };
            out.push_str(&format!("{key}: {rendered}\n"));
        }
        if !out.is_empty() {
            out.pop();
            return out;
        }
    }
    body.to_json()
}

/// One worker stage record, the way the RUNS inspect rows already write it.
fn record_block(record: &JobStageDto) -> String {
    let mut out = String::new();
    let at = [record.model.as_str(), record.at.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" @ ");
    if !at.is_empty() {
        out.push_str(&format!("MODEL\n{at}\n\n"));
    }
    if !record.prompt.is_empty() {
        out.push_str(&format!("PROMPT SENT\n{}\n\n", record.prompt));
    }
    if !record.params.is_empty() {
        out.push_str(&format!("PARAMS\n{}\n\n", record.params));
    }
    if !record.output.is_empty() {
        out.push_str(&format!("ANSWERED\n{}\n\n", record.output));
    }
    out
}

// ---------------------------------------------------------------------------
// Cards from a store pipeline
// ---------------------------------------------------------------------------

fn stage_chip(stage: &PipelineStageDto) -> StageChip {
    let name = stage_word(&stage.name, &stage.kind);
    let (tone, tail) = if stage.skipped {
        (StageTone::Skipped, " (raw)".to_string())
    } else {
        match stage.state {
            JobStateDto::Pending => (StageTone::Pending, String::new()),
            JobStateDto::Running => {
                let permille = stage.progress.as_ref().map(|p| p.permille).unwrap_or(0);
                (StageTone::Running, format!(" \u{b7} {}%", (permille as u32 + 5) / 10))
            }
            JobStateDto::Succeeded => (StageTone::Done, String::new()),
            JobStateDto::Failed => (StageTone::Failed, String::new()),
            JobStateDto::Cancelled => (StageTone::Pending, String::new()),
        }
    };
    StageChip { text: format!("{name}{tail}"), tone }
}

/// The humanized line under the bar, and whether it is a failure.
fn pipeline_status(detail: &PipelineDetailDto) -> String {
    if let Some(stage) = detail
        .stages
        .iter()
        .find(|s| s.state == JobStateDto::Failed && !s.skipped)
    {
        // The recorded error first; failing that, the last thing the worker
        // said it was doing, which is where it died. "no reason recorded"
        // is the honest last resort, never a guess.
        let reason = match &stage.result {
            Some(result) => failure_reason(&result.body, &result.outcome),
            None => match stage.progress.as_ref().map(|p| p.note.trim()) {
                Some(note) if !note.is_empty() => format!("died at \u{201c}{note}\u{201d}"),
                _ => "no reason recorded".to_string(),
            },
        };
        return format!("failed at {} \u{2014} {reason}", stage_word(&stage.name, &stage.kind));
    }
    match detail.state {
        PipelineStateDto::Succeeded => "done".to_string(),
        PipelineStateDto::Cancelled => "cancelled".to_string(),
        PipelineStateDto::Failed => "failed".to_string(),
        PipelineStateDto::Running => {
            let Some(stage) = detail
                .stages
                .iter()
                .find(|s| !s.state.is_terminal())
            else {
                return "finishing".to_string();
            };
            let attempt = (stage.attempts > 1)
                .then(|| format!("retrying (attempt {}) \u{b7} ", stage.attempts))
                .unwrap_or_default();
            match stage.state {
                JobStateDto::Pending => format!("{attempt}queued"),
                _ => match stage.progress.as_ref().map(|p| p.note.clone()) {
                    // The worker's own note IS the explanation for a flat
                    // bar ("queued behind 1 run", "waiting for vram"); it is
                    // already written for a person, so it is not rewritten.
                    Some(note) if !note.is_empty() => format!("{attempt}{note}"),
                    _ => format!("{attempt}{}", kind_word(&stage.kind)),
                },
            }
        }
    }
}

fn pipeline_fold(detail: &PipelineDetailDto) -> String {
    let mut out = String::new();
    out.push_str(&format!("PROMPT\n{}\n", detail.prompt));
    for stage in &detail.stages {
        out.push_str(&format!(
            "\n\u{2500}\u{2500} {} \u{b7} {} \u{b7} {}{} \u{b7} weight {}\n",
            stage.seq + 1,
            stage.name,
            stage.kind,
            if stage.skipped {
                " \u{b7} skipped".to_string()
            } else {
                format!(" \u{b7} {}", stage.state.as_str())
            },
            stage.weight,
        ));
        out.push_str(&format!("JOB\n{}\n\n", stage.job));
        if stage.attempts > 1 {
            out.push_str(&format!("ATTEMPTS\n{}\n\n", stage.attempts));
        }
        if stage.records.is_empty() {
            match &stage.declared {
                Some(body) => out.push_str(&format!(
                    "not sent yet \u{2014} this stage has not started\nDECLARED\n{}\n\n",
                    declared_lines(body)
                )),
                None => out.push_str("not sent yet \u{2014} this stage has not started\n\n"),
            }
        } else {
            for record in &stage.records {
                out.push_str(&record_block(record));
            }
        }
        if let Some(result) = &stage.result {
            if result.outcome != "succeeded" {
                out.push_str(&format!(
                    "RESULT\n{} \u{2014} {}\n\n",
                    result.outcome,
                    failure_reason(&result.body, &result.outcome)
                ));
            }
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn pipeline_card(
    row: &PipelineRowDto,
    detail: Option<&PipelineDetailDto>,
    now_ms: u64,
    open: bool,
) -> RunCard {
    let state = match row.state {
        PipelineStateDto::Running => CardState::Running,
        PipelineStateDto::Succeeded => CardState::Done,
        PipelineStateDto::Failed => CardState::Failed,
        PipelineStateDto::Cancelled => CardState::Cancelled,
    };
    // A run whose every stage is still pending has not started; saying
    // "running" over an untouched bar is the "shows up once claimed" lie
    // this whole design exists to remove.
    let state = match detail {
        Some(detail)
            if state == CardState::Running
                && detail.stages.iter().all(|s| s.state == JobStateDto::Pending) =>
        {
            CardState::Queued
        }
        _ => state,
    };
    let end_ms = row.finished_ms.filter(|ms| *ms > 0).unwrap_or(now_ms);
    let stages = match detail {
        Some(detail) if detail.stages.len() > 1 => {
            detail.stages.iter().map(stage_chip).collect()
        }
        _ => Vec::new(),
    };
    let status = match detail {
        Some(detail) => pipeline_status(detail),
        None if row.note.is_empty() => row.state.as_str().to_string(),
        None => row.note.clone(),
    };
    RunCard {
        key: CardKey::Pipeline(row.pipeline.to_string()),
        state,
        origin: "STORE",
        label: row.title.clone(),
        excerpt: excerpt(row.prompt.as_deref().unwrap_or("")),
        elapsed: format_clock(end_ms.saturating_sub(row.created_ms) as f64 / 1000.0),
        permille: row.permille,
        status,
        stages,
        fold: match (open, detail) {
            (true, Some(detail)) => pipeline_fold(detail),
            (true, None) => "reading the run\u{2026}".to_string(),
            (false, _) => String::new(),
        },
        can_cancel: !state.is_terminal(),
        can_promote: false,
        open,
        created_ms: row.created_ms,
    }
}

// ---------------------------------------------------------------------------
// Cards from a standalone store job
// ---------------------------------------------------------------------------

fn job_card(row: &JobRowDto, detail: Option<&JobDetailDto>, now_ms: u64, open: bool) -> RunCard {
    let state = match row.state {
        JobStateDto::Pending => CardState::Queued,
        JobStateDto::Running => CardState::Running,
        JobStateDto::Succeeded => CardState::Done,
        JobStateDto::Failed => CardState::Failed,
        JobStateDto::Cancelled => CardState::Cancelled,
    };
    let permille = match state {
        CardState::Done => 1000,
        CardState::Queued => 0,
        _ => row.progress.as_ref().map(|p| p.permille).unwrap_or(0),
    };
    let status = match (state, row.progress.as_ref()) {
        (CardState::Queued, _) => "queued".to_string(),
        (CardState::Done, _) => "done".to_string(),
        (CardState::Cancelled, _) => "cancelled".to_string(),
        (CardState::Failed, _) => match detail.and_then(|d| d.result.as_ref()) {
            Some(result) => format!(
                "failed \u{2014} {}",
                failure_reason(&result.body, &result.outcome)
            ),
            None => "failed".to_string(),
        },
        (CardState::Running, Some(progress)) if !progress.note.is_empty() => progress.note.clone(),
        (CardState::Running, _) => kind_word(&row.kind).to_string(),
    };
    let mut fold = String::new();
    if open {
        fold.push_str(&format!(
            "JOB\n{}\n\nKIND\n{} \u{b7} namespace {}\n\n",
            row.job, row.kind, row.namespace
        ));
        match detail {
            Some(detail) if !detail.status.stages.is_empty() => {
                for record in &detail.status.stages {
                    fold.push_str(&record_block(record));
                }
            }
            _ => match row.prompt.as_deref() {
                Some(prompt) if !prompt.is_empty() => {
                    fold.push_str(&format!("PROMPT\n{prompt}\n\n"));
                }
                _ => fold.push_str("not sent yet \u{2014} this job has not started\n\n"),
            },
        }
        if let Some(detail) = detail {
            if detail.attempts.len() > 1 {
                fold.push_str(&format!("ATTEMPTS\n{}\n\n", detail.attempts.len()));
            }
            if let Some(result) = &detail.result {
                if result.outcome != "succeeded" {
                    fold.push_str(&format!(
                        "RESULT\n{} \u{2014} {}\n\n",
                        result.outcome,
                        failure_reason(&result.body, &result.outcome)
                    ));
                }
            }
        }
        while fold.ends_with('\n') {
            fold.pop();
        }
    }
    RunCard {
        key: CardKey::Job(row.job.to_string()),
        state,
        origin: "STORE",
        label: row.kind.clone(),
        excerpt: excerpt(row.prompt.as_deref().unwrap_or("")),
        elapsed: format_clock(now_ms.saturating_sub(row.created_ms) as f64 / 1000.0),
        permille,
        status,
        // A standalone job is one stage; a strip of one is decoration.
        stages: Vec::new(),
        fold,
        can_cancel: !state.is_terminal(),
        can_promote: false,
        open,
        created_ms: row.created_ms,
    }
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
// The poll thread
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Session {
    endpoints: ApiEndpoints,
    token: String,
}

/// What one poll saw, as one atomic picture. Partial pictures make a list
/// flicker between two truths; there is never a reason to draw one.
struct Snapshot {
    pipelines: Vec<PipelineRowDto>,
    details: HashMap<String, PipelineDetailDto>,
    jobs: Vec<JobRowDto>,
    job_details: HashMap<String, JobDetailDto>,
    annotate_pending: usize,
}

enum Msg {
    Snapshot(Box<Snapshot>),
    Error(String),
}

enum Cmd {
    CancelPipeline(PipelineId),
    CancelJob(JobId),
    /// Read the server NOW instead of finishing the nap — sent when the
    /// event feed says a declared run just ended, so a finished card stops
    /// claiming to be running within a round trip rather than a poll.
    ReadNow,
}

/// Everything the app knows about work in flight anywhere.
pub struct RunsChip {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    rx: Option<Receiver<Msg>>,
    cmd_tx: Option<Sender<Cmd>>,
    /// 1 s while the panel is open, 5 s while only the chip is on screen.
    fast: Arc<AtomicBool>,
    /// The cards the person has unfolded. Shared with the poll thread,
    /// which fetches the extra reads an OPEN card needs and nothing more.
    open: Arc<Mutex<Vec<CardKey>>>,
    pipelines: Vec<PipelineRowDto>,
    details: HashMap<String, PipelineDetailDto>,
    jobs: Vec<JobRowDto>,
    job_details: HashMap<String, JobDetailDto>,
    /// Pending `annotate.asset` jobs — counted, named once in the panel,
    /// never listed. SEARCHABLE is that backlog's chip.
    pub annotate_pending: usize,
    pub error: Option<String>,
    /// Per-card high-water mark. A stage retry honestly restarts one stage's
    /// bar; a bar that walks backwards reads as a broken app.
    high_water: HashMap<CardKey, u16>,
    pub panel_open: bool,
}

impl Default for RunsChip {
    fn default() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            rx: None,
            cmd_tx: None,
            fast: Arc::new(AtomicBool::new(false)),
            open: Arc::new(Mutex::new(Vec::new())),
            pipelines: Vec::new(),
            details: HashMap::new(),
            jobs: Vec::new(),
            job_details: HashMap::new(),
            annotate_pending: 0,
            error: None,
            high_water: HashMap::new(),
            panel_open: false,
        }
    }
}

impl Drop for RunsChip {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl RunsChip {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Start reading the server this process is talking to. Idempotent.
    pub fn start(&mut self, endpoints: ApiEndpoints, token: String) {
        if self.running() {
            return;
        }
        let session = Session { endpoints, token };
        let (tx, rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        self.rx = Some(rx);
        self.cmd_tx = Some(cmd_tx);
        self.stop = Arc::new(AtomicBool::new(false));
        let stop = self.stop.clone();
        let fast = self.fast.clone();
        let open = self.open.clone();
        if let Ok(thread) = std::thread::Builder::new()
            .name("runs-poll".into())
            .spawn(move || poll_loop(session, tx, cmd_rx, stop, fast, open))
        {
            self.thread = Some(thread);
        }
    }

    /// Opening the panel makes the numbers a person is reading fresh.
    pub fn set_panel_open(&mut self, open: bool) {
        self.panel_open = open;
        self.fast.store(open, Ordering::Release);
    }

    pub fn is_open(&self, key: &CardKey) -> bool {
        self.open
            .lock()
            .map(|open| open.contains(key))
            .unwrap_or(false)
    }

    /// Unfold / refold one card.
    pub fn toggle_open(&mut self, key: &CardKey) {
        if let Ok(mut open) = self.open.lock() {
            match open.iter().position(|held| held == key) {
                Some(at) => {
                    open.remove(at);
                }
                None => open.push(key.clone()),
            }
        }
    }

    /// Drain the poll thread. True when something on screen changed.
    pub fn poll(&mut self) -> bool {
        if self.rx.is_none() {
            return false;
        }
        let mut changed = false;
        loop {
            let msg = self.rx.as_ref().map(|rx| rx.try_recv());
            match msg {
                Some(Ok(Msg::Snapshot(snapshot))) => {
                    self.pipelines = snapshot.pipelines;
                    self.details = snapshot.details;
                    self.jobs = snapshot.jobs;
                    self.job_details = snapshot.job_details;
                    self.annotate_pending = snapshot.annotate_pending;
                    self.error = None;
                    changed = true;
                }
                Some(Ok(Msg::Error(error))) => {
                    if self.error.as_deref() != Some(error.as_str()) {
                        log!("runs: {error}");
                        self.error = Some(error);
                        changed = true;
                    }
                }
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }

    /// Read the server now. The event feed knows a run ended before this
    /// poll would: `pipeline.finished` is the ONE end-of-run signal (a
    /// publish is per-asset and coincidental, and a failed run publishes
    /// nothing at all), so a card settles on the event, not on the clock.
    pub fn wake(&self) -> bool {
        self.cmd_tx
            .as_ref()
            .is_some_and(|tx| tx.send(Cmd::ReadNow).is_ok())
    }

    /// Stop one spawned unit. LOCAL keys are the app's own business and are
    /// refused here — main.rs routes those to the engine's own cancel.
    pub fn cancel(&self, key: &CardKey) -> bool {
        let Some(tx) = &self.cmd_tx else { return false };
        match key {
            CardKey::Pipeline(id) => match PipelineId::parse(id) {
                Some(id) => tx.send(Cmd::CancelPipeline(id)).is_ok(),
                None => false,
            },
            CardKey::Job(id) => match JobId::parse(id) {
                Some(id) => tx.send(Cmd::CancelJob(id)).is_ok(),
                None => false,
            },
            CardKey::Local(_) | CardKey::LocalQueued(_) => false,
        }
    }

    /// Every spawned unit, one card each, in the one order: what is running,
    /// what is waiting, what is over — newest first inside each.
    pub fn cards(&mut self, local: &[LocalRun], queued: &[LocalQueued]) -> Vec<RunCard> {
        let now_ms = now_ms();
        let open = self.open.lock().map(|open| open.clone()).unwrap_or_default();
        let is_open = |key: &CardKey| open.contains(key);
        let mut cards = Vec::new();

        // Store pipelines. A stage job of a pipeline must never ALSO draw as
        // a standalone job, so the stage job ids are collected here.
        let mut pipeline_jobs: Vec<String> = Vec::new();
        for row in &self.pipelines {
            let id = row.pipeline.to_string();
            let detail = self.details.get(&id);
            if let Some(detail) = detail {
                pipeline_jobs.extend(detail.stages.iter().map(|s| s.job.to_string()));
            }
            let key = CardKey::Pipeline(id);
            cards.push(pipeline_card(row, detail, now_ms, is_open(&key)));
        }
        for row in &self.jobs {
            let id = row.job.to_string();
            if pipeline_jobs.contains(&id) {
                continue;
            }
            let key = CardKey::Job(id.clone());
            cards.push(job_card(row, self.job_details.get(&id), now_ms, is_open(&key)));
        }
        for run in local {
            let key = CardKey::Local(run.id);
            cards.push(local_card(run, is_open(&key)));
        }
        for entry in queued {
            let key = CardKey::LocalQueued(entry.index);
            cards.push(queued_card(entry, is_open(&key)));
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn poll_loop(
    session: Session,
    tx: Sender<Msg>,
    cmd_rx: Receiver<Cmd>,
    stop: Arc<AtomicBool>,
    fast: Arc<AtomicBool>,
    open: Arc<Mutex<Vec<CardKey>>>,
) {
    let api = match Api::new(
        session.endpoints,
        HttpLimits::default_v1(),
        Some(session.token),
    ) {
        Ok(api) => api,
        Err(error) => {
            let _ = tx.send(Msg::Error(format!("connect: {error}")));
            return;
        }
    };
    // Finished runs cannot change again, so their detail is read once.
    let mut details: HashMap<String, PipelineDetailDto> = HashMap::new();
    while !stop.load(Ordering::Acquire) {
        match read_once(&api, &open, &mut details) {
            Ok(snapshot) => {
                let _ = tx.send(Msg::Snapshot(Box::new(snapshot)));
            }
            Err(error) => {
                let _ = tx.send(Msg::Error(error));
            }
        }
        // The sleep IS the command wait: a cancel pressed a tenth of a
        // second into a five-second nap goes out now, not in 4.9 s.
        let until = if fast.load(Ordering::Acquire) { POLL_OPEN } else { POLL_CLOSED };
        let deadline = std::time::Instant::now() + until;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || stop.load(Ordering::Acquire) {
                break;
            }
            match cmd_rx.recv_timeout(left.min(Duration::from_millis(250))) {
                Ok(Cmd::CancelPipeline(id)) => match api.cancel_pipeline(&id) {
                    Ok(answer) => log!(
                        "runs: cancelled {} stage jobs of {id} ({})",
                        answer.cancelled,
                        answer.state.as_str()
                    ),
                    Err(error) => {
                        let _ = tx.send(Msg::Error(format!("cancel {id}: {error}")));
                    }
                },
                Ok(Cmd::CancelJob(id)) => match api.cancel_job(&id) {
                    Ok(count) => log!("runs: cancelled {count} job(s) for {id}"),
                    Err(error) => {
                        let _ = tx.send(Msg::Error(format!("cancel {id}: {error}")));
                    }
                },
                Ok(Cmd::ReadNow) => {}
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // A cancel changes the picture: redraw from the server, now.
            break;
        }
    }
}

fn read_once(
    api: &Api,
    open: &Arc<Mutex<Vec<CardKey>>>,
    kept: &mut HashMap<String, PipelineDetailDto>,
) -> Result<Snapshot, String> {
    let open_keys = open.lock().map(|open| open.clone()).unwrap_or_default();
    let pipelines = api
        .list_pipelines(None, false, PIPELINE_PAGE)
        .map_err(|error| format!("pipelines: {error}"))?;
    // Details: every run still moving (its stage strip and fold change under
    // it), plus any finished run this session has not read yet. Runs already
    // read and finished are kept, never re-read.
    let mut details = HashMap::new();
    let mut budget = DETAIL_BUDGET;
    for row in &pipelines {
        let id = row.pipeline.to_string();
        let terminal = row.state.is_terminal();
        if terminal {
            if let Some(held) = kept.get(&id) {
                details.insert(id, held.clone());
                continue;
            }
        }
        if budget == 0 {
            continue;
        }
        budget -= 1;
        match api.pipeline_detail(&row.pipeline) {
            Ok(detail) => {
                if terminal {
                    kept.insert(id.clone(), detail.clone());
                }
                details.insert(id, detail);
            }
            Err(error) => return Err(format!("pipeline {id}: {error}")),
        }
    }
    kept.retain(|id, _| details.contains_key(id));

    let mut jobs = Vec::new();
    let mut annotate_pending = 0usize;
    for state in [JobStateDto::Running, JobStateDto::Pending] {
        let page = api
            .list_jobs(None, None, Some(state), JOB_PAGE)
            .map_err(|error| format!("jobs: {error}"))?;
        for row in page {
            // The annotation backlog is thousands of rows with a chip of its
            // own; counting it here and listing it would make this panel a
            // second, worse version of that one.
            if row.kind == ANNOTATE_KIND && row.state == JobStateDto::Pending {
                annotate_pending += 1;
                continue;
            }
            jobs.push(row);
        }
    }

    let mut job_details = HashMap::new();
    for key in open_keys.iter().take(JOB_DETAIL_BUDGET) {
        let CardKey::Job(id) = key else { continue };
        let Some(job) = JobId::parse(id) else { continue };
        if let Ok(detail) = api.job_detail(&job) {
            job_details.insert(id.clone(), detail);
        }
    }

    Ok(Snapshot { pipelines, details, jobs, job_details, annotate_pending })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_client::{
        JobProgressDto, JobResultDto, PipelineStageDto, StageOnFailDto,
    };

    fn pipe_id(byte: u8) -> PipelineId {
        PipelineId([byte; 16])
    }

    fn job_id(byte: u8) -> JobId {
        JobId([byte; 16])
    }

    fn stage(
        seq: u32,
        name: &str,
        kind: &str,
        state: JobStateDto,
        weight: u16,
        permille: u16,
    ) -> PipelineStageDto {
        PipelineStageDto {
            name: name.to_string(),
            seq,
            job: job_id(seq as u8 + 1),
            kind: kind.to_string(),
            state,
            skipped: false,
            weight,
            on_fail: StageOnFailDto::Fail,
            attempts: 1,
            progress: (permille > 0).then(|| JobProgressDto {
                permille,
                note: String::new(),
                updated_ms: None,
            }),
            declared: None,
            records: Vec::new(),
            result: None,
        }
    }

    fn detail(stages: Vec<PipelineStageDto>, state: PipelineStateDto) -> PipelineDetailDto {
        PipelineDetailDto {
            pipeline: pipe_id(9),
            namespace: "gen".into(),
            title: "DREAM".into(),
            state,
            permille: aggregate_permille(stages.iter().map(|s| (s.weight, s.done_permille()))),
            enqueued_by: None,
            created_ms: 1_000,
            prompt: "80s new wave about leaving the city".into(),
            current_stage: None,
            finished_ms: None,
            stages,
        }
    }

    fn row(state: PipelineStateDto, permille: u16, created_ms: u64) -> PipelineRowDto {
        PipelineRowDto {
            pipeline: pipe_id(9),
            namespace: "gen".into(),
            title: "DREAM".into(),
            state,
            permille,
            stages: 3,
            enqueued_by: None,
            created_ms,
            prompt: Some("80s new wave about leaving the city".into()),
            current_stage: Some("image".into()),
            note: String::new(),
            finished_ms: None,
        }
    }

    fn job_row(state: JobStateDto, kind: &str, created_ms: u64, permille: u16) -> JobRowDto {
        JobRowDto {
            job: job_id(200),
            namespace: "gen".into(),
            kind: kind.to_string(),
            state,
            enqueued_by: None,
            created_ms,
            prompt: Some("a rain-slick highway".into()),
            progress: (permille > 0).then(|| JobProgressDto {
                permille,
                note: "@.165 denoise 40%".into(),
                updated_ms: None,
            }),
        }
    }

    /// The chip is the ONE line that says how much work is in flight, and it
    /// is never blank: "nothing is running" is an answer a person acts on.
    #[test]
    fn the_chip_says_how_many_and_how_far_or_that_it_is_idle() {
        assert_eq!(chip_text(&[]), "RUNS \u{b7} idle");

        let mut running = pipeline_card(&row(PipelineStateDto::Running, 600, 10), None, 20, false);
        running.state = CardState::Running;
        let mut other = running.clone();
        other.key = CardKey::Job("job_b".into());
        other.permille = 200;
        assert_eq!(
            chip_text(&[running.clone(), other.clone()]),
            "RUNS \u{b7} 2 running \u{b7} 40%"
        );

        // Queued work is still work the person fired off: it is counted, and
        // it drags the percent down honestly rather than being hidden.
        let mut waiting = other.clone();
        waiting.key = CardKey::LocalQueued(0);
        waiting.state = CardState::Queued;
        waiting.permille = 0;
        assert_eq!(
            chip_text(&[running.clone(), waiting.clone()]),
            "RUNS \u{b7} 1 running \u{b7} 1 queued \u{b7} 30%"
        );
        assert_eq!(chip_text(&[waiting.clone()]), "RUNS \u{b7} 1 queued");

        // Finished cards stay readable in the panel but are not "in flight".
        let mut done = running;
        done.state = CardState::Done;
        assert_eq!(chip_text(&[done]), "RUNS \u{b7} idle");
    }

    /// A pipeline draws as ONE card: one aggregate bar, one chip per stage
    /// with its own percent as TEXT, and no second bar anywhere.
    #[test]
    fn a_pipeline_is_one_card_with_one_bar_and_a_strip() {
        let detail = detail(
            vec![
                stage(0, "expand", "text.expand", JobStateDto::Succeeded, 5, 1000),
                stage(1, "image", "image.generate", JobStateDto::Running, 15, 500),
                stage(2, "video", "video.generate", JobStateDto::Pending, 70, 0),
            ],
            PipelineStateDto::Running,
        );
        let card = pipeline_card(&row(PipelineStateDto::Running, detail.permille, 1_000), Some(&detail), 61_000, false);

        // The bar is the client crate's one formula, not a second one here.
        assert_eq!(card.permille, 138);
        assert_eq!(card.percent(), 14);
        assert_eq!(card.state, CardState::Running);
        assert_eq!(card.excerpt, "\u{201c}80s new wave about leaving the city\u{201d}");
        assert_eq!(card.elapsed, "1:00");
        assert!(card.can_cancel);
        // Three chips, one style, the running one carrying its own percent.
        let strip: Vec<&str> = card.stages.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(strip, ["expand", "image \u{b7} 50%", "video"]);
        assert_eq!(
            card.stages.iter().map(|c| c.tone).collect::<Vec<_>>(),
            [StageTone::Done, StageTone::Running, StageTone::Pending]
        );
        // Closed, the card carries no fold: a closed list stays a list.
        assert!(card.fold.is_empty());
        assert_eq!(card.status, "rendering");
    }

    /// A run that has been declared but not claimed reads QUEUED, and its
    /// fold already says what each stage WILL be sent.
    #[test]
    fn a_spawned_run_is_readable_before_anything_is_claimed() {
        let mut stages = vec![
            stage(0, "expand", "text.expand", JobStateDto::Pending, 5, 0),
            stage(1, "image", "image.generate", JobStateDto::Pending, 15, 0),
        ];
        stages[1].declared = Some(makepad_asset_client::json::obj(vec![
            ("prompt", makepad_asset_client::json::s("a rain-slick highway")),
            ("steps", Value::Int(28)),
        ]));
        let detail = detail(stages, PipelineStateDto::Running);
        let card = pipeline_card(
            &row(PipelineStateDto::Running, 0, 1_000),
            Some(&detail),
            1_500,
            true,
        );
        assert_eq!(card.state, CardState::Queued, "nothing claimed yet");
        assert_eq!(card.permille, 0);
        assert_eq!(card.status, "queued");
        assert!(card.fold.contains("not sent yet \u{2014} this stage has not started"));
        assert!(card.fold.contains("prompt: a rain-slick highway"));
        assert!(card.fold.contains("steps: 28"));
        // Never on the face of the card — only in the fold.
        assert!(!card.status.contains("job_"));
        assert!(card.fold.contains("job_"));
    }

    /// A failure is readable without unfolding: that is the whole rule for
    /// what is primary text and what is folded.
    #[test]
    fn a_failure_says_where_and_why_without_unfolding() {
        let mut stages = vec![
            stage(0, "image", "image.generate", JobStateDto::Succeeded, 15, 1000),
            stage(1, "publish", "image.upscale", JobStateDto::Failed, 25, 730),
        ];
        stages[1].result = Some(JobResultDto {
            outcome: "failed".into(),
            attempt: 1,
            recorded_ms: 5,
            body: makepad_asset_client::json::obj(vec![(
                "error",
                makepad_asset_client::json::s("annotation refused"),
            )]),
        });
        let detail = detail(stages, PipelineStateDto::Failed);
        let card = pipeline_card(
            &row(PipelineStateDto::Failed, detail.permille, 1_000),
            Some(&detail),
            2_000,
            false,
        );
        assert_eq!(card.state, CardState::Failed);
        assert_eq!(card.status, "failed at publish \u{2014} annotation refused");
        assert!(!card.can_cancel, "a finished run has nothing to stop");
        // The bar FREEZES where it died — it is not filled to 100%.
        assert!(card.permille < 1000, "{} should be frozen", card.permille);

        // A worker that died without recording WHY still leaves the last
        // thing it said it was doing. That is where it died, and it is a
        // better answer than a shrug.
        let mut mute = detail.clone();
        mute.stages[1].result = None;
        mute.stages[1].progress = Some(JobProgressDto {
            permille: 730,
            note: "@.217 publishing".into(),
            updated_ms: None,
        });
        let card = pipeline_card(
            &row(PipelineStateDto::Failed, mute.permille, 1_000),
            Some(&mute),
            2_000,
            false,
        );
        assert_eq!(
            card.status,
            "failed at publish \u{2014} died at \u{201c}@.217 publishing\u{201d}"
        );

        // And when there is nothing at all to say, it says exactly that.
        mute.stages[1].progress = None;
        let card = pipeline_card(
            &row(PipelineStateDto::Failed, mute.permille, 1_000),
            Some(&mute),
            2_000,
            false,
        );
        assert_eq!(card.status, "failed at publish \u{2014} no reason recorded");
    }

    /// One list, one order: running, then waiting, then over — newest first
    /// inside each — whichever engine spawned it.
    #[test]
    fn every_engine_lands_in_one_list_in_one_order() {
        let mut chip = RunsChip::default();
        chip.pipelines = vec![
            row(PipelineStateDto::Succeeded, 1000, 5_000),
            {
                let mut r = row(PipelineStateDto::Running, 400, 9_000);
                r.pipeline = pipe_id(3);
                r
            },
        ];
        chip.jobs = vec![job_row(JobStateDto::Running, "image.generate", 8_000, 300)];
        let cards = chip.cards(&[], &[LocalQueued { index: 0, label: "MESH", prompt: "a lamp" }]);

        let order: Vec<(&str, CardState)> = cards
            .iter()
            .map(|card| (card.label.as_str(), card.state))
            .collect();
        assert_eq!(
            order,
            [
                ("DREAM", CardState::Running),
                ("image.generate", CardState::Running),
                ("MESH", CardState::Queued),
                ("DREAM", CardState::Done),
            ]
        );
        // A standalone job card has no strip — a strip of one is decoration.
        assert!(cards[1].stages.is_empty());
        assert_eq!(cards[2].origin, "LOCAL");
        assert!(cards[2].can_cancel, "a queued run is cancellable at once");
    }

    /// A stage retry legitimately restarts one stage's bar. The aggregate
    /// still may not walk backwards.
    #[test]
    fn the_aggregate_never_walks_backwards() {
        let mut chip = RunsChip::default();
        chip.pipelines = vec![row(PipelineStateDto::Running, 610, 1_000)];
        assert_eq!(chip.cards(&[], &[])[0].permille, 610);
        chip.pipelines = vec![row(PipelineStateDto::Running, 240, 1_000)];
        assert_eq!(
            chip.cards(&[], &[])[0].permille,
            610,
            "held at the high-water mark while the stage retries"
        );
        chip.pipelines = vec![row(PipelineStateDto::Running, 800, 1_000)];
        assert_eq!(chip.cards(&[], &[])[0].permille, 800);

        // A run that leaves the list takes its mark with it, so a later run
        // reusing nothing of it starts honestly at zero.
        chip.pipelines.clear();
        assert!(chip.cards(&[], &[]).is_empty());
        assert!(chip.high_water.is_empty());
    }

    /// A pipeline's stage jobs are its own. They must never ALSO draw as
    /// standalone jobs — that is the double-rendering this design removes.
    #[test]
    fn a_pipelines_stage_job_never_draws_twice() {
        let mut chip = RunsChip::default();
        let detail = detail(
            vec![
                stage(0, "expand", "text.expand", JobStateDto::Succeeded, 5, 1000),
                stage(1, "image", "image.generate", JobStateDto::Running, 15, 500),
            ],
            PipelineStateDto::Running,
        );
        let stage_job = detail.stages[1].job;
        chip.details.insert(pipe_id(9).to_string(), detail);
        chip.pipelines = vec![row(PipelineStateDto::Running, 138, 1_000)];
        chip.jobs = vec![
            {
                let mut row = job_row(JobStateDto::Running, "image.generate", 900, 500);
                row.job = stage_job;
                row
            },
            job_row(JobStateDto::Running, "vision.describe", 800, 100),
        ];
        let cards = chip.cards(&[], &[]);
        assert_eq!(cards.len(), 2, "the stage job is the pipeline's, not its own card");
        assert!(cards.iter().any(|card| card.label == "DREAM"));
        assert!(cards.iter().any(|card| card.label == "vision.describe"));
    }

    /// The local engine's stages weigh the SAME as the store's, because they
    /// read the same table — a local card's 61% means what a store card's
    /// 61% means.
    #[test]
    fn local_stages_weigh_from_the_one_shared_table() {
        assert_eq!(local_stage_weight("text"), 5);
        assert_eq!(local_stage_weight("image"), 15);
        assert_eq!(local_stage_weight("video"), 70);
        assert_eq!(local_stage_weight("music"), 60);
        assert_eq!(local_stage_weight("mesh"), 40);
        // An unmapped domain is the server's neutral weight, never a guess
        // invented here.
        assert_eq!(
            local_stage_weight("wardrobe"),
            makepad_asset_client::NEUTRAL_STAGE_WEIGHT
        );
    }
}
