//! Conservative, bounded reports from live provider terminals. A terminal is
//! untrusted text, not an account-wide availability protocol. Percentages,
//! elapsed reset times, user input and generic process activity are not proof
//! of a limit or its recovery. The UI should say "Limit reported".
use crate::usage::UsageProvider;
use std::collections::BTreeMap;

const MAX_LANES: usize = 32;
const MAX_TAIL_BYTES: usize = 8192;
const MAX_LINES: usize = 80;
const MAX_LINE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivitySignal {
    #[default]
    None,
    /// A new provider-owned work event, not just a live process or keystroke.
    Working,
    /// Completed recovery with observed provider readiness. A login click,
    /// browser prompt or restored session UUID alone must still use None.
    RecoveryComplete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitReport {
    pub lane: String,
    pub provider: UsageProvider,
    pub observed_at: u64,
    pub excerpt: String,
    /// The CLI's own wording/timezone; never interpreted as an expiry timer.
    pub reset_text: Option<String>,
}
impl LimitReport {
    pub fn label(&self) -> &'static str { "Limit reported" }
}

struct SeenLane {
    provider: UsageProvider,
    generation: u64,
    sequence: u64,
    observed_at: u64,
    lines: Vec<String>,
    report: Option<LimitReport>,
}

#[derive(Default)]
pub struct UsageStalls {
    pub revision: u64,
    lanes: BTreeMap<String, SeenLane>,
}

impl UsageStalls {
    /// Feed recent bottom-of-scrollback rows, never a manually scrolled
    /// viewport. generation identifies the actual provider/PTY session;
    /// sequence is a monotonic sample counter within that generation.
    ///
    /// First samples and replacement sessions establish a baseline. This
    /// prevents restored history from becoming a fresh limit report. An
    /// existing report survives replacement until observed work/recovery.
    /// Returns whether the visible reports changed, not whether text changed.
    pub fn ingest_terminal(&mut self, lane: &str, provider: UsageProvider, generation: u64,
        sequence: u64, tail: &str, now: u64, signal: ActivitySignal) -> bool
    {
        if lane.is_empty() || lane.len() > 96 || lane.chars().any(char::is_control) { return false; }
        if !self.lanes.contains_key(lane) && self.lanes.len() >= MAX_LANES { return false; }
        let lines = normalize_tail(tail);
        let Some(seen) = self.lanes.get_mut(lane) else {
            self.lanes.insert(lane.to_owned(), SeenLane { provider, generation, sequence, observed_at: now, lines, report: None });
            return false;
        };
        if now < seen.observed_at || (generation == seen.generation && sequence <= seen.sequence) { return false; }
        let before = seen.report.clone();
        if provider != seen.provider {
            seen.report = None;
        }
        if provider != seen.provider || generation != seen.generation {
            seen.provider = provider; seen.generation = generation; seen.sequence = sequence;
            seen.observed_at = now; seen.lines = lines;
            if signal != ActivitySignal::None { seen.report = None; }
        } else {
            let mut previous = BTreeMap::<&str, usize>::new();
            for line in &seen.lines { *previous.entry(line).or_default() += 1; }
            let mut occurrences = BTreeMap::<&str, usize>::new();
            let mut context = TextContext::default();
            let mut last_evidence = None;
            for (index, line) in lines.iter().enumerate() {
                let count = occurrences.entry(line).or_default(); *count += 1;
                let fresh = *count > previous.get(line.as_str()).copied().unwrap_or(0);
                let evidence = classify_line(provider, line, &mut context);
                if !fresh {
                    // A repaint of an older limit above already-observed work
                    // is historical. Do not manufacture a fresh block from a
                    // changed wrapping/reset label earlier in the transcript.
                    if evidence == Some(Evidence::Working) && matches!(last_evidence, Some(Some(_))) { last_evidence = None; }
                    continue;
                }
                if let Some(Evidence::Limit) = evidence {
                    let reset_text = reset_text(line).or_else(|| lines.get(index + 1).filter(|next| is_reset_line(next)).and_then(|next| reset_text(next)));
                    last_evidence = Some(Some(LimitReport { lane: lane.to_owned(), provider, observed_at: now, excerpt: bounded_prefix(line, 360), reset_text }));
                } else if evidence == Some(Evidence::Working) { last_evidence = Some(None); }
            }
            // A fresh limit line wins over a generic host signal in the same
            // sample. Within the terminal, later work can supersede a limit.
            if let Some(report) = last_evidence { seen.report = report; }
            else if signal != ActivitySignal::None { seen.report = None; }
            seen.sequence = sequence; seen.observed_at = now; seen.lines = lines;
        }
        let changed = before != seen.report;
        if changed { self.revision = self.revision.saturating_add(1); }
        changed
    }

    pub fn report(&self, lane: &str) -> Option<&LimitReport> { self.lanes.get(lane)?.report.as_ref() }
    /// Only this provider's affected active lanes. No quota percentage input
    /// exists here; another provider cannot make this provider's pill red.
    pub fn reports(&self, provider: UsageProvider) -> Vec<&LimitReport> {
        self.lanes.values().filter(|lane| lane.provider == provider).filter_map(|lane| lane.report.as_ref()).collect()
    }
    pub fn retain_lanes(&mut self, active: &[&str]) -> bool {
        let mut changed = false;
        self.lanes.retain(|id, lane| {
            let keep = active.iter().take(MAX_LANES).any(|active| *active == id);
            if !keep && lane.report.is_some() { changed = true; }
            keep
        });
        if changed { self.revision = self.revision.saturating_add(1); }
        changed
    }
    /// Project switches discard terminal baselines as well as visible reports.
    pub fn reset(&mut self) {
        self.lanes.clear(); self.revision = self.revision.saturating_add(1);
    }
}

#[derive(Default)]
struct TextContext { fenced: bool, user: bool, tool: bool }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Evidence { Limit, Working }

fn classify_line(provider: UsageProvider, line: &str, context: &mut TextContext) -> Option<Evidence> {
    let text = line.trim();
    if text.starts_with("```") || text.starts_with("~~~") { context.fenced = !context.fenced; return None; }
    if context.fenced || text.is_empty() { return None; }
    // User prompt bodies, quoted text, diffs, JSON and source/grep locations
    // cannot be interpreted as provider status lines.
    if text.starts_with(['❯', '›', '>', '$']) || text.starts_with("user:") || text.starts_with("You:") {
        context.user = true; context.tool = false; return None;
    }
    if text.starts_with(['"', '\'', '`', '{', '[', '+', '-', '|']) { return None; }
    let claude_marker = text.starts_with('⏺');
    let codex_marker = text.starts_with('•') || text.starts_with('◦');
    let error_marker = text.starts_with('■') || text.starts_with('⚠');
    let spinner = text.starts_with(['✻', '✽', '✢', '✳', '✶', '✺', '·']);
    let child_output = text.starts_with('⎿') || text.starts_with('└') || text.starts_with('│');
    let body = text.trim_start_matches(['⏺', '•', '◦', '■', '⚠', '✻', '✽', '✢', '✳', '✶', '✺', '·', '⎿']).trim();
    let lower = body.to_lowercase().replace('’', "'");
    let tool = match provider {
        UsageProvider::Claude => claude_marker && ["bash(", "read(", "write(", "edit(", "grep(", "glob(", "agent(", "task(", "webfetch(", "websearch("].iter().any(|prefix| lower.starts_with(prefix)),
        UsageProvider::Codex => codex_marker && ["ran ", "explored", "edited ", "added ", "deleted ", "updated plan"].iter().any(|prefix| lower.starts_with(prefix)),
    };
    if tool {
        context.user = false; context.tool = true;
        return Some(Evidence::Working);
    }
    if claude_marker || codex_marker || error_marker || spinner {
        context.user = false; context.tool = false;
    }
    // Claude's direct response error may use ⎿ immediately after a user
    // prompt. That native hard-limit/reset form is distinct from the same
    // text under an observed Bash/Grep/Read tool-output block.
    if provider == UsageProvider::Claude && text.starts_with('⎿') && !context.tool && hard_limit(provider, &lower) && reset_text(body).is_some() {
        context.user = false;
        return Some(Evidence::Limit);
    }
    if context.user || context.tool || (child_output && !text.starts_with('⎿')) { return None; }
    // Work indicators require the provider's native interrupt affordance or
    // its tool marker. Arbitrary prose, shell output and typed input do not
    // clear a report, nor do auth prompts or an unchanged idle prompt.
    if ((provider == UsageProvider::Codex && codex_marker) || (provider == UsageProvider::Claude && spinner))
        && lower.contains("esc to interrupt") && !lower.contains("limit") && !lower.contains("login")
    { return Some(Evidence::Working); }
    if hard_limit(provider, &lower) { Some(Evidence::Limit) } else { None }
}

fn hard_limit(provider: UsageProvider, line: &str) -> bool {
    // These strings are warnings or continued work, not an actual stop.
    if ["close to", "approaching", "finishing up", "a little extra", "now using", "has reset", "available again", "example", "quoted", "println!", "assert", "=>"].iter().any(|word| line.contains(word)) { return false; }
    let line = line.strip_prefix("error: ").unwrap_or(line);
    match provider {
        UsageProvider::Codex => ["you've hit your usage limit", "you have reached your usage limit", "you've reached your usage limit", "usage limit reached", "you're out of credits", "you are out of credits"].iter().any(|prefix| status_prefix(line, prefix)),
        UsageProvider::Claude => ["you've hit your limit", "you've hit your usage limit", "you've hit your weekly limit", "you've hit your weekly usage limit", "you've hit your monthly spend limit", "you're out of extra usage", "you're out of usage credits", "usage limit reached"].iter().any(|prefix| status_prefix(line, prefix)),
    }
}

fn status_prefix(line: &str, prefix: &str) -> bool {
    let Some(rest) = line.strip_prefix(prefix) else { return false; };
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with(['.', '!', '·', '—', '–', ':'])
        || ["for ", "resets ", "reset at ", "try again "].iter().any(|suffix| rest.starts_with(suffix))
}
fn is_reset_line(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.starts_with("resets ") || lower.starts_with("reset at ") || lower.starts_with("try again at ")
}
fn reset_text(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    ["resets ", "reset at ", "try again at ", "try again in ", "continuing automatically at "].iter()
        .filter_map(|marker| lower.find(marker)).min().map(|start| bounded_prefix(&line[start..], 180))
}

fn bounded_prefix(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) { end -= 1; }
    text[..end].to_owned()
}
fn normalize_tail(text: &str) -> Vec<String> {
    let mut start = text.len().saturating_sub(MAX_TAIL_BYTES);
    while !text.is_char_boundary(start) { start += 1; }
    let mut clean = String::with_capacity(text.len().min(MAX_TAIL_BYTES));
    let mut escape = 0u8;
    for ch in text[start..].chars() {
        match escape {
            1 => { escape = match ch { '[' => 2, ']' => 3, _ => 0 }; }
            2 => { if ('@'..='~').contains(&ch) { escape = 0; } }
            3 => { if ch == '\u{7}' { escape = 0; } else if ch == '\u{1b}' { escape = 4; } }
            4 => { escape = if ch == '\\' { 0 } else { 3 }; }
            _ => {
                if ch == '\u{1b}' { escape = 1; }
                else if ch == '\n' || ch == '\r' { clean.push('\n'); }
                else if ch == '\t' { clean.push(' '); }
                else if !ch.is_control() { clean.push(ch); }
            }
        }
    }
    let mut lines: Vec<_> = clean.lines().rev().take(MAX_LINES).map(|line| bounded_prefix(line.trim_end(), MAX_LINE_BYTES)).collect();
    lines.reverse(); lines
}
