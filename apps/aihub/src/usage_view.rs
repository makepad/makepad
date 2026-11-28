//! The panel's Usage section: per tool (Claude, Codex, Antigravity) a header with the read's
//! state and a Reconnect button that ends the hub's warm CLI and starts a fresh one (for
//! Antigravity, which keeps none, it reads again at once), then one line per account. The
//! account signed in now comes first, then every other account the tool was seen signed in as,
//! the most headroom first, so the person sees which account to switch to. A line names the
//! account in a masked form (never the whole address; Antigravity's CLI names none, so its one
//! line is "Account") and shows each limit (session, the week of all models, Fable's week,
//! Neon's) as a small bar with its share and when it resets; a narrower panel drops words, then
//! reset times, then bars, never wrapping to a second line.
//! A click on a line opens its detail under it (each limit with its reset time, when it was
//! read, its free resets, a week's trace); another click closes it.
//!
//! Codex's signed-in account carries a Reset limits button with its free resets left: a press
//! asks in place ("Use 1 of 3 resets?") before one is spent, and the limits read again at once
//! are the answer. A failed or stuck read says so; numbers from an earlier read are dimmed and
//! dated, never shown as current.

use super::{Hit, HubAppView, ViewState};
use crate::usage::{clock_label, countdown, plan_label, Busy, Limit, Provider, ProviderState, UsageWindow, POLL_SECONDS};
use crate::usage_history::{AccountHistory, Sample, SampleWindow, LOCAL_ACCOUNT};
use makepad_shell_kit::graph::{trace, Span};
use makepad_shell_kit::ui::{cut_top, inset, rect, HAlign, Ico};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;

const HEAD_H: f64 = 22.0;
const LINE_H: f64 = 16.0;
/// An account's line.
const ROW_H: f64 = 20.0;
const METER_LABEL_W: f64 = 52.0;
const PCT_W: f64 = 38.0;
const RESET_W: f64 = 170.0;
/// The account's dot, before its name.
const DOT_W: f64 = 14.0;
/// The account's name keeps at least this much of its line.
const LABEL_MIN: f64 = 64.0;
/// The open/closed chevron that ends a line.
const CHEVRON_W: f64 = 14.0;
/// How long a reset that changed nothing (or failed) says so (s).
const RESET_NOTE_SECS: u64 = 180;

/// The level colours of a used share (Scope's: amber from 80 %, red from 90 %).
fn level(used: f64, tok: &ShellTokens) -> Vec4f {
    if used >= 90.0 {
        vec4(0.90, 0.30, 0.26, 1.0)
    } else if used >= 80.0 {
        vec4(0.93, 0.62, 0.20, 1.0)
    } else {
        tok.controls.selected_color
    }
}

const AVAILABLE: Vec4f = Vec4f { x: 0.30, y: 0.78, z: 0.42, w: 1.0 };
/// Fable's week in the traces (the week of all models is the theme's colour).
const FABLE: Vec4f = Vec4f { x: 0.66, y: 0.52, z: 0.95, w: 1.0 };
/// Antigravity Neon in the traces (distinct from Week blue and Fable purple).
const NEON: Vec4f = Vec4f { x: 0.25, y: 0.75, z: 0.85, w: 1.0 };
/// The traces show the last week.
const TRACE_SECS: u64 = 7 * 86400;
const WARN: Vec4f = Vec4f { x: 0.93, y: 0.62, z: 0.20, w: 1.0 };

/// An account's limit as it stands now: a window whose reset passed is free again.
fn effective(w: Option<&SampleWindow>, now: u64) -> Option<f64> {
    let w = w?;
    if w.reset_at.is_some_and(|at| at <= now) {
        return Some(0.0);
    }
    w.used
}

/// Every reset of the account's last reading passed (none unknown): it is fresh again.
pub(super) fn available(sample: &Sample, now: u64) -> bool {
    let resets: Vec<Option<u64>> = Limit::ALL.iter().filter_map(|l| sample.window(*l)).map(|w| w.reset_at).collect();
    !resets.is_empty() && resets.iter().all(|r| r.is_some_and(|at| at <= now))
}

/// The other accounts in the order to switch to: available ones, then the least used week,
/// then the most recently seen.
pub(super) fn switch_order<'a>(accounts: &'a [AccountHistory], current: Option<&str>, now: u64) -> Vec<&'a AccountHistory> {
    let mut others: Vec<&AccountHistory> = accounts.iter().filter(|a| Some(a.email.as_str()) != current).collect();
    let key = |a: &AccountHistory| {
        let latest = a.latest();
        let free = latest.is_some_and(|s| available(s, now));
        let week = latest.and_then(|s| [s.week.as_ref(), s.fable.as_ref(), s.neon.as_ref()].into_iter().filter_map(|w| effective(w, now)).reduce(f64::max)).unwrap_or(f64::INFINITY);
        (!free, week)
    };
    others.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then(b.last_checked_at.cmp(&a.last_checked_at))
    });
    others
}

/// One limit of an account's line as it stands now.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cell {
    pub limit: Limit,
    /// The used share (0 once its reset passed; None: the CLI did not say).
    pub used: Option<f64>,
    pub reset_at: Option<u64>,
    pub reset_text: Option<String>,
    /// Its reset passed: free again.
    pub free: bool,
}

impl Cell {
    fn new(limit: Limit, used: Option<f64>, reset_at: Option<u64>, reset_text: Option<String>, now: u64) -> Cell {
        let free = reset_at.is_some_and(|at| at <= now);
        Cell { limit, used: if free { Some(0.0) } else { used }, reset_at, reset_text, free }
    }

    fn window(&self) -> UsageWindow {
        UsageWindow { name: String::new(), limit: Some(self.limit), used_percent: self.used, reset_at: self.reset_at, reset_text: self.reset_text.clone() }
    }
}

/// One account's line.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Row {
    pub email: String,
    pub plan: Option<String>,
    /// The account signed in now.
    pub current: bool,
    /// Its numbers are the latest read's (else an earlier one's: dimmed and dated).
    pub fresh: bool,
    pub read_at: u64,
    pub cells: Vec<Cell>,
    /// Every reset passed: the account is fresh again.
    pub available: bool,
    pub reset_credits: Option<u32>,
    /// One of its free resets can be spent from here: Codex's account signed in now, with
    /// resets left, its CLI answering.
    pub can_reset: bool,
}

fn sample_cells(sample: &Sample, now: u64) -> Vec<Cell> {
    Limit::ALL.iter().filter_map(|l| sample.window(*l).map(|w| Cell::new(*l, w.used, w.reset_at, w.reset_text.clone(), now))).collect()
}

fn history_row(account: &AccountHistory, sample: &Sample, current: bool, now: u64) -> Row {
    let cells = sample_cells(sample, now);
    Row {
        email: account.email.clone(),
        plan: account.plan.clone(),
        current,
        fresh: false,
        read_at: sample.at,
        available: available(sample, now),
        cells,
        reset_credits: account.reset_credits,
        can_reset: false,
    }
}

/// A tool's lines: the account signed in now first (its latest reading, else the last one it
/// had), then every other account seen, in the order to switch to.
pub(super) fn rows(state: &ProviderState, now: u64) -> Vec<Row> {
    let current = state.latest.account.clone().or_else(|| {
        (state.provider == Provider::Agy && state.latest.stuck.is_none() && state.shown().is_some()).then(|| LOCAL_ACCOUNT.to_owned())
    });
    let mut out = Vec::new();
    if let Some(email) = &current {
        let history = state.accounts.iter().find(|a| &a.email == email);
        if let Some((at, reading)) = state.shown() {
            let fresh = at == state.checked_at && state.latest.error.is_none();
            let cells: Vec<Cell> = Limit::ALL.iter().filter_map(|l| reading.window(*l).map(|w| Cell::new(*l, w.used_percent, w.reset_at, w.reset_text.clone(), now))).collect();
            let signed_in = state.latest.stuck.is_none();
            out.push(Row {
                email: email.clone(),
                plan: reading.plan.clone().or_else(|| history.and_then(|h| h.plan.clone())),
                current: true,
                fresh,
                read_at: at,
                available: !cells.is_empty() && cells.iter().all(|c| c.free),
                cells,
                reset_credits: reading.reset_credits,
                can_reset: state.provider == Provider::Codex && signed_in && reading.reset_credits.is_some_and(|n| n > 0),
            });
        } else if let Some((account, sample)) = history.and_then(|h| h.latest().map(|s| (h, s))) {
            out.push(history_row(account, sample, true, now));
        }
    }
    for account in switch_order(&state.accounts, current.as_deref(), now) {
        if let Some(sample) = account.latest() {
            out.push(history_row(account, sample, false, now));
        }
    }
    out
}

/// An account as its line names it, never its whole address: the start of its name, the
/// digits it ends in (what tells `name1@` from `name2@`) and the start of its domain.
pub(super) fn account_label(email: &str) -> String {
    if email == LOCAL_ACCOUNT {
        return "Account".into();
    }
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    let digits = local.chars().rev().take_while(char::is_ascii_digit).count();
    let stem: Vec<char> = local.chars().take(local.chars().count() - digits).collect();
    let mut label: String = stem.iter().take(3).collect();
    if stem.len() > 3 {
        label.push('\u{2026}');
    }
    let tail: Vec<char> = local.chars().skip(stem.len()).collect();
    if tail.len() > 3 {
        label.push('\u{2026}');
    }
    label.extend(tail.iter().skip(tail.len().saturating_sub(3)));
    if let Some(head) = domain.split('.').next().filter(|d| !d.is_empty()) {
        label.push('@');
        label.extend(head.chars().take(2));
        label.push('\u{2026}');
    }
    label
}

/// A reset as few characters: `45m`, `5h`, `6d` (`units` 2: `2h 13m`, `6d 2h`, a zero second
/// unit left out).
pub(super) fn short_countdown(secs: u64, units: u8) -> String {
    let m = secs.div_ceil(60);
    let (big, small, b, s) = match m {
        m if m < 60 => return format!("{m}m"),
        m if m < 1440 => (m / 60, m % 60, "h", "m"),
        m => (m / 1440, (m % 1440) / 60, "d", "h"),
    };
    if units == 1 || small == 0 { format!("{big}{b}") } else { format!("{big}{b} {small}{s}") }
}

/// How much of each limit a line shows, by its width: the limit's name, bar, share and reset
/// (`Full`), a letter for the name and a shorter reset (`Compact`), bar and share (`Tight`),
/// the share alone (`Bare`). The detail under the line always has everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fit {
    Full,
    Compact,
    Tight,
    Bare,
}

impl Fit {
    const ALL: [Fit; 4] = [Fit::Full, Fit::Compact, Fit::Tight, Fit::Bare];

    /// The widths of a limit's name, bar, share and reset at this fit.
    fn parts(self) -> [f64; 4] {
        match self {
            Fit::Full => [48.0, 46.0, 28.0, 46.0],
            Fit::Compact => [11.0, 34.0, 25.0, 25.0],
            Fit::Tight => [0.0, 30.0, 25.0, 0.0],
            Fit::Bare => [0.0, 0.0, 26.0, 0.0],
        }
    }

    /// One limit's width, the gap after it included.
    fn cell_w(self) -> f64 {
        self.parts().iter().sum::<f64>() + 6.0
    }
}

/// The fullest fit whose limits leave the account's name its minimum width on one line of
/// `row_w`, with `trailing` taken at its end (the reset button).
pub(super) fn fit(row_w: f64, cells: usize, trailing: f64) -> Fit {
    Fit::ALL.into_iter().find(|f| DOT_W + LABEL_MIN + cells as f64 * f.cell_w() + trailing + CHEVRON_W <= row_w).unwrap_or(Fit::Bare)
}

/// The limit columns a tool's lines share, so its accounts' bars line up: each limit any of
/// its lines has, in order, at one fit; a line without a limit leaves its column empty.
pub(super) struct Columns {
    pub limits: Vec<Limit>,
    pub fit: Fit,
    /// The reset button's width (0: no line offers one).
    pub reset_w: f64,
}

impl Columns {
    pub(super) fn of(lines: &[Row], row_w: f64, reset_w: f64) -> Columns {
        let limits: Vec<Limit> = Limit::ALL.into_iter().filter(|l| lines.iter().any(|r| r.cells.iter().any(|c| c.limit == *l))).collect();
        let fit = fit(row_w, limits.len(), Self::trailing(reset_w));
        Columns { limits, fit, reset_w }
    }

    /// What the line keeps between its limits and its chevron.
    fn trailing(reset_w: f64) -> f64 {
        if reset_w > 0.0 { reset_w + 8.0 } else { 2.0 }
    }

    /// Where the limits start on a line ending at `right`.
    fn start(&self, right: f64) -> f64 {
        right - CHEVRON_W - Self::trailing(self.reset_w) - self.fit.cell_w() * self.limits.len() as f64 + 6.0
    }
}

/// What a tool's header says about its last read.
pub(super) fn status_text(state: &ProviderState, now: u64) -> (String, bool) {
    match state.busy {
        Busy::Reconnecting => return ("reconnecting\u{2026}".into(), false),
        Busy::Resetting => return ("resetting the limits\u{2026}".into(), false),
        Busy::Reading if state.checked_at == 0 => return ("reading\u{2026}".into(), false),
        _ => {}
    }
    if state.latest.error.is_none() && state.checked_at > 0 {
        return (format!("read {}", ago(now.saturating_sub(state.checked_at))), false);
    }
    if let Some(stuck) = state.latest.stuck {
        return (stuck.label().into(), true);
    }
    let last_ok = state.last_ok.as_ref().map(|(at, _)| *at);
    match last_ok {
        // no reading for more than one poll interval
        Some(at) if state.checked_at.saturating_sub(at) > POLL_SECONDS => (format!("no answer since {}", clock_label(at, now)), true),
        Some(at) => (format!("not read since {}", clock_label(at, now)), true),
        None if state.checked_at == 0 => ("reading\u{2026}".into(), false),
        None => ("not read".into(), true),
    }
}

/// What the person's last Reset limits says for a while, when it changed nothing or failed (a
/// reset that went through says nothing: the refreshed limits show it).
pub(super) fn reset_note(state: &ProviderState, now: u64) -> Option<(String, bool)> {
    let note = state.reset.as_ref().filter(|n| now.saturating_sub(n.at) < RESET_NOTE_SECS)?;
    match &note.outcome {
        Ok(outcome) => outcome.note().map(|(text, warn)| (format!("Reset limits: {text}"), warn)),
        Err(error) => Some((format!("Reset limits: {error}"), true)),
    }
}

fn ago(secs: u64) -> String {
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else {
        format!("{} h ago", secs / 3600)
    }
}

/// A window's reset: `available` once it passed, else its time and the countdown.
pub(super) fn reset_text(w: &UsageWindow, now: u64) -> (String, bool) {
    match w.reset_at {
        Some(at) if at <= now => (format!("available (reset {})", clock_label(at, now)), true),
        Some(at) => (format!("{} \u{b7} in {}", clock_label(at, now), countdown(at - now)), false),
        None => (w.reset_text.as_deref().map(|t| t.trim_start_matches("Resets ").trim_start_matches("resets ").to_string()).unwrap_or_else(|| "reset unknown".into()), false),
    }
}

fn short_name(limit: Limit) -> &'static str {
    match limit {
        Limit::Session => "S",
        Limit::Week => "W",
        Limit::Fable => "F",
        Limit::Neon => "N",
    }
}

impl HubAppView {
    /// The Usage section from `rest`'s top; returns what is left below it.
    pub(super) fn draw_usage(&mut self, cx: &mut Cx2d, tok: &ShellTokens, fg: Vec4f, mut rest: Rect, st: &ViewState) -> Rect {
        self.rows.clear();
        let (sep, r2) = cut_top(rest, 12.0);
        rest = r2;
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let (head, r2) = cut_top(rest, 24.0);
        rest = r2;
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x - 60.0, head.size.y - 4.0), tok, fg, "Usage");
        let on = st.settings.usage;
        let sw = self.d.toggle_switch(cx, dvec2(head.pos.x + head.size.x - super::SWITCH_W, head.pos.y + 1.0), tok, on, fg);
        self.hits.push((Hit::Usage, inset(sw, -6.0)));
        if !on {
            self.confirm = None;
            let (row, r2) = cut_top(rest, LINE_H);
            rest = r2;
            self.d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Off: the hub reads no account's usage");
            return rest;
        }
        let now = st.now_ms / 1000;
        let mut confirm_shown = false;
        for provider in Provider::ALL {
            let Some(state) = st.usage.iter().find(|s| s.provider == provider) else { continue };
            rest = self.draw_provider(cx, tok, fg, rest, state, now, &mut confirm_shown);
        }
        // a question whose line is gone (the account changed, its resets ran out) is dropped
        if !confirm_shown {
            self.confirm = None;
        }
        rest
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_provider(&mut self, cx: &mut Cx2d, tok: &ShellTokens, fg: Vec4f, mut rest: Rect, state: &ProviderState, now: u64, confirm_shown: &mut bool) -> Rect {
        let dim = darker(fg, 1.4);
        // The header: the tool, the read's state, Reconnect.
        let (row, r2) = cut_top(rest, HEAD_H);
        rest = r2;
        let (status, trouble) = status_text(state, now);
        let button_w = self.d.button_width(cx, tok, false, "Reconnect", tok.font.caption) - 6.0;
        let button = rect(row.pos.x + row.size.x - button_w, row.pos.y + 2.0, button_w, row.size.y - 4.0);
        let hit = Hit::Reconnect(state.provider);
        let busy = matches!(state.busy, Busy::Reconnecting | Busy::Resetting);
        let button_state = if busy { CtrlState::Disabled } else if self.hot == Some(hit) { CtrlState::Hover } else if trouble { CtrlState::Selected } else { CtrlState::Normal };
        self.d.button(cx, button, tok, button_state, None, "Reconnect", tok.font.caption, fg, true);
        if !busy {
            self.hits.push((hit, button));
        }
        let name = state.provider.label();
        let name_w = self.d.measure(cx, true, tok.font.body, name);
        self.d.label(cx, rect(row.pos.x, row.pos.y, name_w, row.size.y), true, tok.font.body, fg, HAlign::Left, name);
        let status_x = row.pos.x + name_w + 10.0;
        self.d.label_elided(cx, rect(status_x, row.pos.y, (button.pos.x - 8.0 - status_x).max(0.0), row.size.y), false, tok.font.caption, if trouble { WARN } else { dim }, HAlign::Left, &status);
        // Why the last read failed (a stuck CLI says so in the header), what the last reset
        // came to when it changed nothing, and a history that did not load or save.
        let read_error = state.latest.error.as_ref().filter(|_| state.latest.stuck.is_none() && state.checked_at > 0 && state.busy != Busy::Reconnecting).map(|e| (e.clone(), true));
        for (text, warn) in [read_error, reset_note(state, now), state.history_error.clone().map(|e| (e, true))].into_iter().flatten() {
            let (row, r2) = cut_top(rest, LINE_H);
            rest = r2;
            self.d.label_elided(cx, rect(row.pos.x + DOT_W, row.pos.y, row.size.x - DOT_W, row.size.y), false, tok.font.caption, if warn { alpha(WARN, 0.85) } else { dim }, HAlign::Left, &text);
        }
        // One line per account, the one signed in now first; a click opens its detail.
        let lines = rows(state, now);
        let reset_w = match lines.iter().find(|l| l.can_reset) {
            Some(l) => self.d.button_width(cx, tok, true, &l.reset_credits.unwrap_or(0).to_string(), tok.font.caption) - 6.0,
            None => 0.0,
        };
        let columns = Columns::of(&lines, rest.size.x, reset_w);
        for line in lines {
            let i = self.rows.len();
            self.rows.push((state.provider, line.email.clone()));
            let (row, r2) = cut_top(rest, ROW_H);
            rest = r2;
            let open = self.expanded.as_ref().is_some_and(|(p, e)| *p == state.provider && *e == line.email);
            *confirm_shown |= self.account_row(cx, tok, fg, row, &line, i, open, state, &columns, now);
            if open {
                let history = state.accounts.iter().find(|a| a.email == line.email);
                rest = self.account_detail(cx, tok, fg, rest, &line, history, now);
            }
        }
        cut_top(rest, 6.0).1
    }

    /// One account on one line: its dot, its masked name (and plan where it fits), its limits as
    /// bars, the reset button (or its question), the chevron. True when it asked the question.
    #[allow(clippy::too_many_arguments)]
    fn account_row(&mut self, cx: &mut Cx2d, tok: &ShellTokens, fg: Vec4f, r: Rect, line: &Row, i: usize, open: bool, state: &ProviderState, columns: &Columns, now: u64) -> bool {
        let hit = Hit::Row(i);
        if open || self.hot == Some(hit) {
            self.d.solid(cx, r, alpha(fg, if open { 0.07 } else { 0.05 }));
        }
        self.hits.push((hit, r));
        let ink = if line.current { fg } else { darker(fg, 1.15) };
        let dim = darker(fg, 1.4);
        let dot = if line.current { tok.controls.selected_color } else if line.available { AVAILABLE } else { alpha(fg, 0.25) };
        self.d.icon_centered(cx, Ico::Dot, rect(r.pos.x, r.pos.y, DOT_W - 4.0, r.size.y), 6.0, dot);
        let right = r.pos.x + r.size.x;
        let chevron = rect(right - CHEVRON_W, r.pos.y, CHEVRON_W, r.size.y);
        self.d.icon_centered(cx, if open { Ico::ChevronDown } else { Ico::ChevronRight }, chevron, 10.0, alpha(fg, 0.45));
        let mut end = chevron.pos.x - 2.0;
        let label_x = r.pos.x + DOT_W;
        let busy = matches!(state.busy, Busy::Resetting | Busy::Reconnecting);
        let asking = line.can_reset && self.confirm.as_ref().is_some_and(|(p, e)| *p == state.provider && *e == line.email);
        let font = tok.font.caption;
        if asking {
            // "Use 1 of 3 resets?  [Cancel] [Use]", in place of the limits
            let n = line.reset_credits.unwrap_or(0);
            for (text, hit, primary) in [("Use", Hit::ResetConfirm(i), true), ("Cancel", Hit::ResetCancel(i), false)] {
                let w = self.d.button_width(cx, tok, false, text, font) - 6.0;
                let b = rect(end - w, r.pos.y + 1.0, w, r.size.y - 2.0);
                let state = if busy { CtrlState::Disabled } else if self.hot == Some(hit) { CtrlState::Hover } else if primary { CtrlState::Selected } else { CtrlState::Normal };
                self.d.button(cx, b, tok, state, None, text, font, fg, true);
                if !busy {
                    self.hits.push((hit, b));
                }
                end = b.pos.x - 4.0;
            }
            let long = format!("Use 1 of {n} reset{}?", if n == 1 { "" } else { "s" });
            let room = (end - 6.0 - (label_x + LABEL_MIN)).max(0.0);
            let question = if self.d.measure(cx, false, font, &long) <= room { long } else { format!("1 of {n}?") };
            let qw = self.d.measure(cx, false, font, &question).min(room);
            self.d.label_elided(cx, rect(end - 6.0 - qw, r.pos.y, qw, r.size.y), false, font, fg, HAlign::Right, &question);
            end -= 12.0 + qw;
        } else {
            if line.can_reset {
                let text = line.reset_credits.unwrap_or(0).to_string();
                let b = rect(end - columns.reset_w, r.pos.y + 1.0, columns.reset_w, r.size.y - 2.0);
                let hit = Hit::Reset(i);
                let state = if busy { CtrlState::Disabled } else if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
                self.d.button(cx, b, tok, state, Some(Ico::Refresh), &text, font, fg, true);
                if !busy {
                    self.hits.push((hit, b));
                }
            }
            let x0 = columns.start(right);
            let cw = columns.fit.cell_w();
            for c in &line.cells {
                let Some(k) = columns.limits.iter().position(|l| *l == c.limit) else { continue };
                self.cell(cx, tok, ink, rect(x0 + k as f64 * cw, r.pos.y, cw - 6.0, r.size.y), c, columns.fit, line.fresh, now);
            }
            end = x0 - 8.0;
        }
        // the name, then the plan where it fits
        let name = account_label(&line.email);
        let room = (end - label_x).max(0.0);
        let name_w = self.d.measure(cx, line.current, font, &name).min(room);
        self.d.label_elided(cx, rect(label_x, r.pos.y, name_w, r.size.y), line.current, font, ink, HAlign::Left, &name);
        if let Some(plan) = &line.plan {
            let plan = plan_label(plan);
            let plan_w = self.d.measure(cx, false, font, &plan);
            if name_w + 8.0 + plan_w <= room {
                self.d.label(cx, rect(label_x + name_w + 8.0, r.pos.y, plan_w, r.size.y), false, font, dim, HAlign::Left, &plan);
            }
        }
        asking
    }

    /// One limit on an account's line: `W [====   ] 52% 6d`, as much of it as `fit` holds.
    #[allow(clippy::too_many_arguments)]
    fn cell(&mut self, cx: &mut Cx2d, tok: &ShellTokens, ink: Vec4f, r: Rect, c: &Cell, fit: Fit, fresh: bool, now: u64) {
        let ink = if fresh { ink } else { darker(ink, 1.3) };
        let dim = darker(ink, 1.4);
        let font = tok.font.caption;
        let [name_w, bar_w, pct_w, reset_w] = fit.parts();
        let mut x = r.pos.x;
        if name_w > 0.0 {
            let name = if fit == Fit::Full { c.limit.label() } else { short_name(c.limit) };
            self.d.label_elided(cx, rect(x, r.pos.y, name_w - 3.0, r.size.y), false, font, dim, HAlign::Left, name);
            x += name_w;
        }
        let used = c.used.unwrap_or(0.0).clamp(0.0, 100.0);
        let color = if c.free { AVAILABLE } else { level(used, tok) };
        if bar_w > 0.0 {
            let bar = rect(x, r.pos.y + r.size.y * 0.5 - 2.5, bar_w, 5.0);
            self.d.solid(cx, bar, alpha(ink, 0.12));
            if used > 0.0 {
                self.d.solid(cx, rect(bar.pos.x, bar.pos.y, (bar_w * used / 100.0).max(2.0), bar.size.y), if fresh { color } else { alpha(color, 0.6) });
            }
            x += bar_w;
        }
        let pct = match c.used {
            Some(p) => format!("{p:.0}%"),
            None => "\u{2014}".into(),
        };
        // a line without bars colours its share
        let pct_color = if bar_w > 0.0 { ink } else if fresh { color } else { alpha(color, 0.7) };
        self.d.label(cx, rect(x, r.pos.y, pct_w, r.size.y), false, font, if c.free { AVAILABLE } else { pct_color }, HAlign::Right, &pct);
        x += pct_w;
        if reset_w > 0.0 {
            let text = match c.reset_at {
                Some(at) if at > now => short_countdown(at - now, if fit == Fit::Full { 2 } else { 1 }),
                _ => String::new(),
            };
            self.d.label_elided(cx, rect(x + 4.0, r.pos.y, reset_w - 4.0, r.size.y), false, font, dim, HAlign::Left, &text);
        }
    }

    /// An open line's detail: each limit with its reset time, when it was read and its free
    /// resets, and its week's trace.
    #[allow(clippy::too_many_arguments)]
    fn account_detail(&mut self, cx: &mut Cx2d, tok: &ShellTokens, fg: Vec4f, mut rest: Rect, line: &Row, history: Option<&AccountHistory>, now: u64) -> Rect {
        let dim = darker(fg, 1.4);
        let inner = |r: Rect| rect(r.pos.x + DOT_W, r.pos.y, r.size.x - DOT_W, r.size.y);
        for c in &line.cells {
            let (row, r2) = cut_top(rest, LINE_H);
            rest = r2;
            self.meter(cx, tok, fg, inner(row), c.limit.label(), &c.window(), now, line.fresh);
        }
        let mut parts = Vec::new();
        if let Some(plan) = &line.plan {
            parts.push(plan_label(plan));
        }
        parts.push(if line.fresh { format!("read {}", clock_label(line.read_at, now)) } else { format!("as read at {}", clock_label(line.read_at, now)) });
        if let Some(n) = line.reset_credits.filter(|n| *n > 0) {
            let s = if n == 1 { "" } else { "s" };
            parts.push(if line.current { format!("{n} free full reset{s} of the limits") } else { format!("{n} free reset{s} (usable once signed in)") });
        }
        let (row, r2) = cut_top(rest, LINE_H);
        rest = r2;
        let row = inner(row);
        self.d.label_elided(cx, rect(row.pos.x + 8.0, row.pos.y, row.size.x - 8.0, row.size.y), false, tok.font.caption, dim, HAlign::Left, &parts.join(" \u{b7} "));
        if let Some(history) = history.filter(|h| h.samples.len() >= 2) {
            let (row, r2) = cut_top(rest, 24.0);
            rest = r2;
            let row = inner(row);
            self.spark(cx, tok, rect(row.pos.x + 8.0 + METER_LABEL_W, row.pos.y + 2.0, row.size.x - 8.0 - METER_LABEL_W, row.size.y - 4.0), &history.samples, now, 1.4);
            self.d.label(cx, rect(row.pos.x + 8.0, row.pos.y, METER_LABEL_W, row.size.y), false, tok.font.caption, darker(dim, 1.2), HAlign::Left, "7 days");
        }
        cut_top(rest, 4.0).1
    }

    /// The last seven days of Week, Fable and Neon readings, or Session for a session-only history.
    fn spark(&mut self, cx: &mut Cx2d, tok: &ShellTokens, plot: Rect, samples: &[Sample], now: u64, width: f32) {
        let span = Span { t0: now.saturating_sub(TRACE_SECS) as f64, t1: now as f64, lo: 0.0, hi: 100.0 };
        self.d.solid(cx, rect(plot.pos.x, plot.pos.y + plot.size.y - 1.0, plot.size.x, 1.0), alpha(tok.popups.text, 0.10));
        let traces: &[_] = if samples.iter().any(|s| s.week.is_some() || s.fable.is_some() || s.neon.is_some()) {
            &[(Limit::Fable, FABLE, 0.0), (Limit::Neon, NEON, 0.18), (Limit::Week, tok.controls.selected_color, 0.18)]
        } else {
            &[(Limit::Session, NEON, 0.18)]
        };
        for &(limit, color, fill) in traces {
            let mut points = samples.iter().filter_map(|s| s.window(limit).map(|w| (s.at as f64, w.used.map_or(f32::NAN, |u| u as f32))));
            trace(&mut self.g, cx, plot, &mut points, span, TRACE_SECS as f64, color, fill, width);
        }
    }

    /// `Week [=====     ] 52%   Nov 23 17:00 · in 6 d 2 h`
    #[allow(clippy::too_many_arguments)]
    fn meter(&mut self, cx: &mut Cx2d, tok: &ShellTokens, fg: Vec4f, row: Rect, label: &str, w: &UsageWindow, now: u64, current: bool) {
        let ink = if current { fg } else { darker(fg, 1.5) };
        let dim = darker(ink, 1.4);
        let x = row.pos.x + 8.0;
        self.d.label(cx, rect(x, row.pos.y, METER_LABEL_W, row.size.y), false, tok.font.caption, dim, HAlign::Left, label);
        let (reset, available) = reset_text(w, now);
        let reset_w = RESET_W.min(row.size.x * 0.42);
        let bar_x = x + METER_LABEL_W;
        let bar_w = (row.pos.x + row.size.x - reset_w - 8.0 - PCT_W - bar_x).max(16.0);
        let used = if available { 0.0 } else { w.used_percent.unwrap_or(0.0).clamp(0.0, 100.0) };
        let bar = rect(bar_x, row.pos.y + row.size.y * 0.5 - 3.0, bar_w, 6.0);
        self.d.solid(cx, bar, alpha(ink, 0.12));
        let color = if available { AVAILABLE } else { level(used, tok) };
        if used > 0.0 {
            self.d.solid(cx, rect(bar.pos.x, bar.pos.y, (bar_w * used / 100.0).max(2.0), bar.size.y), if current { color } else { alpha(color, 0.5) });
        }
        let pct = match (available, w.used_percent) {
            (true, _) => "0%".to_string(),
            (false, Some(p)) => format!("{p:.0}%"),
            (false, None) => "\u{2014}".into(),
        };
        self.d.label(cx, rect(bar_x + bar_w, row.pos.y, PCT_W, row.size.y), false, tok.font.caption, ink, HAlign::Right, &pct);
        self.d.label_elided(cx, rect(row.pos.x + row.size.x - reset_w, row.pos.y, reset_w, row.size.y), false, tok.font.caption, if available { AVAILABLE } else { dim }, HAlign::Right, &reset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Reading, ResetNote, ResetOutcome, Stuck, UsageWindow};
    use std::sync::Arc;

    fn week(used: f64, reset_at: u64) -> Reading {
        Reading {
            account: Some("user@example.com".into()),
            windows: vec![UsageWindow { name: "7-day".into(), limit: Some(Limit::Week), used_percent: Some(used), reset_at: Some(reset_at), reset_text: None }],
            ..Default::default()
        }
    }

    /// A read that fails says so with the time of the last good one; past one poll interval
    /// it is "no answer since"; a CLI at a login says it is stuck.
    #[test]
    fn a_failed_or_stuck_read_says_so_and_dates_the_last_numbers() {
        let t0 = 1_800_000_000;
        let mut s = ProviderState { provider: Provider::Codex, checked_at: t0, latest: week(40.0, t0 + 3600), last_ok: Some((t0, week(40.0, t0 + 3600))), ..Default::default() };
        assert_eq!(status_text(&s, t0 + 120), ("read 2 min ago".into(), false));
        s.latest = Reading::failed(Some("user@example.com".into()), "timed out");
        s.checked_at = t0 + POLL_SECONDS;
        assert!(status_text(&s, t0 + POLL_SECONDS).0.starts_with("not read since "));
        s.checked_at = t0 + 2 * POLL_SECONDS;
        let (text, trouble) = status_text(&s, t0 + 2 * POLL_SECONDS);
        assert!(text.starts_with("no answer since ") && trouble, "{text}");
        s.latest.stuck = Some(Stuck::Login);
        assert_eq!(status_text(&s, t0 + 2 * POLL_SECONDS).0, "stuck: waiting for login");
        s.busy = Busy::Reconnecting;
        assert_eq!(status_text(&s, t0).0, "reconnecting\u{2026}");
    }

    fn account(email: &str, week: f64, reset_at: u64, checked: u64) -> AccountHistory {
        let w = SampleWindow { used: Some(week), reset_at: Some(reset_at), reset_text: None };
        AccountHistory { email: email.into(), last_checked_at: checked, samples: vec![Sample { at: checked, week: Some(w), ..Default::default() }], ..Default::default() }
    }

    /// The accounts to switch to: one whose reset passed is available and comes first, then
    /// the least used week; the account signed in now is not among them.
    #[test]
    fn the_account_with_the_most_headroom_comes_first() {
        let now = 1_800_000_000;
        let mut accounts = vec![
            account("a@example.com", 30.0, now + 3600, now - 600),
            account("b@example.com", 99.0, now - 60, now - 86400),
            account("c@example.com", 10.0, now + 7200, now - 1200),
            account("d@example.com", 5.0, now + 7200, now - 1800),
        ];
        let order: Vec<&str> = switch_order(&accounts, Some("d@example.com"), now).iter().map(|a| a.email.as_str()).collect();
        assert_eq!(order, ["b@example.com", "c@example.com", "a@example.com"], "b's week reset a minute ago: free again");
        assert!(available(accounts[1].latest().unwrap(), now) && !available(accounts[0].latest().unwrap(), now));
        accounts[2].samples[0].neon = Some(SampleWindow { used: Some(60.0), reset_at: Some(now + 3600), reset_text: None });
        let order: Vec<&str> = switch_order(&accounts, Some("d@example.com"), now).into_iter().map(|a| a.email.as_str()).collect();
        assert_eq!(order, ["b@example.com", "a@example.com", "c@example.com"], "c's tighter Neon limit reduces its headroom");
    }

    /// Once a window's reset passed it shows as available, so the account has headroom.
    #[test]
    fn a_passed_reset_shows_available() {
        let w = week(95.0, 1_800_000_000).windows.remove(0);
        assert!(reset_text(&w, 1_800_000_000 + 1).1, "after the reset");
        let (text, available) = reset_text(&w, 1_800_000_000 - 2 * 3600 - 13 * 60);
        assert!(!available && text.ends_with("in 2 h 13 min"), "{text}");
    }

    /// Each account is one line: the one signed in now first with its latest numbers, then
    /// the others to switch to. Only Codex's signed-in account, with resets left and its CLI
    /// answering, offers Reset limits; a failed read keeps its line, dimmed, without it.
    #[test]
    fn one_line_per_account_and_the_reset_only_where_it_can_be_spent() {
        let now = 1_800_000_000;
        let mut reading = week(40.0, now + 3600);
        reading.account = Some("name2@example.com".into());
        reading.reset_credits = Some(3);
        reading.plan = Some("pro".into());
        let accounts = vec![account("name1@example.com", 80.0, now - 60, now - 7200), account("name2@example.com", 35.0, now + 3600, now - 600), account("name3@example.com", 20.0, now + 9000, now - 900)];
        let mut s = ProviderState { provider: Provider::Codex, checked_at: now - 30, latest: reading.clone(), last_ok: Some((now - 30, reading.clone())), accounts: Arc::new(accounts), ..Default::default() };
        let lines = rows(&s, now);
        let names: Vec<&str> = lines.iter().map(|r| r.email.as_str()).collect();
        assert_eq!(names, ["name2@example.com", "name1@example.com", "name3@example.com"], "the signed-in account, then the one whose week reset");
        let me = &lines[0];
        assert!(me.current && me.fresh && me.can_reset && me.reset_credits == Some(3));
        assert_eq!((me.cells.len(), me.cells[0].used, me.plan.as_deref()), (1, Some(40.0), Some("pro")), "the latest reading's numbers, not the history's 35");
        assert!(lines[1].available && lines[1].cells[0].free && lines[1].cells[0].used == Some(0.0), "a passed reset is free again");
        assert!(lines[1..].iter().all(|r| !r.current && !r.can_reset));
        assert_eq!(Columns::of(&lines, 528.0, 30.0).limits, [Limit::Week], "only the limits the lines have take a column");

        // a failed read: the same line from the last good reading, dimmed, no reset offered
        s.latest = Reading::failed(Some("name2@example.com".into()), "timed out");
        s.checked_at = now;
        let me = &rows(&s, now)[0];
        assert!(me.current && !me.fresh && me.read_at == now - 30);
        // the CLI ended, no resets left, or Claude: nothing to press
        s.latest = Reading { stuck: Some(Stuck::Ended), ..reading.clone() };
        assert!(!rows(&s, now)[0].can_reset);
        s.latest = Reading { reset_credits: Some(0), ..reading.clone() };
        s.last_ok = None;
        s.checked_at = now;
        assert!(!rows(&s, now)[0].can_reset);
        s.latest = reading.clone();
        s.provider = Provider::Claude;
        assert!(!rows(&s, now)[0].can_reset);
        // signed out: no line claims to be the account now
        s.latest = Reading::failed(None, "Not signed in");
        s.last_ok = None;
        assert!(rows(&s, now).iter().all(|r| !r.current) && rows(&s, now).len() == 3);
    }

    #[test]
    fn agy_anonymous_row_shows_current_dimmed_and_signed_out_states() {
        let at = 1_800_000_000;
        let good = Reading {
            plan: Some("Google AI Ultra".into()),
            windows: vec![UsageWindow { limit: Some(Limit::Neon), used_percent: Some(35.0), reset_at: Some(at + 3600), ..Default::default() }],
            ..Default::default()
        };
        let mut history = crate::usage_history::History::open(None, Provider::Agy);
        history.observe(&good, at);
        let mut state = ProviderState { provider: Provider::Agy, accounts: Arc::new(history.accounts), ..Default::default() };
        let lines = rows(&state, at);
        assert_eq!(lines.len(), 1, "startup restores the one saved account");
        assert!(!lines[0].current && !lines[0].fresh, "the saved row is historical before the first read");

        state.checked_at = at;
        state.latest = good.clone();
        state.last_ok = Some((at, good));
        let lines = rows(&state, at);
        assert_eq!(lines.len(), 1, "the live anonymous account and its history share one row");
        let row = &lines[0];
        assert_eq!(row.email, LOCAL_ACCOUNT, "the row uses the saved local key");
        assert_eq!(account_label(&row.email), "Account", "the anonymous row has a readable label");
        assert!(row.current && row.fresh && !row.can_reset, "a fresh Antigravity row is current with no reset action");
        assert_eq!(row.plan.as_deref(), Some("Google AI Ultra"), "the live row shows its plan");
        assert_eq!(row.cells[0].used, Some(35.0), "the live Neon bar shows the reported share");
        assert_eq!(short_name(Limit::Neon), "N", "Neon's compact column label is N");
        for row_w in [460.0 - 32.0, 560.0 - 32.0] {
            let columns = Columns::of(&lines, row_w, 0.0);
            assert_eq!(columns.limits, [Limit::Neon], "only Neon takes a column");
            let used = DOT_W + LABEL_MIN + columns.fit.cell_w() + Columns::trailing(columns.reset_w) + CHEVRON_W;
            assert!(used <= row_w, "the Neon row needs {used} px within {row_w} px");
        }

        state.checked_at += POLL_SECONDS;
        state.latest = Reading::failed(None, "timed out");
        let lines = rows(&state, state.checked_at);
        assert_eq!(lines.len(), 1, "a timeout keeps the anonymous row");
        let stale = &lines[0];
        assert!(stale.current && !stale.fresh && stale.read_at == at, "the timeout dims and dates the last successful read");
        assert_eq!(stale.cells, row.cells, "the stale row keeps the last good limits");

        state.latest.stuck = Some(Stuck::Login);
        let lines = rows(&state, state.checked_at);
        assert_eq!(lines.len(), 1, "signing out keeps one history row");
        assert!(!lines[0].current && !lines[0].fresh && lines[0].read_at == at, "the signed-out row shows when history was read");
        assert_eq!(lines[0].cells, row.cells, "the signed-out row loads the same Neon window from history");
        assert_eq!(status_text(&state, state.checked_at), ("stuck: waiting for login".into(), true), "the header asks for login");

        let missing = ProviderState { provider: Provider::Agy, checked_at: at, latest: Reading::failed(None, "CLI missing"), ..Default::default() };
        assert!(rows(&missing, at).is_empty(), "a missing CLI with no history has no row");
        assert_eq!(status_text(&missing, at), ("not read".into(), true), "the header reports a missing first reading");
    }

    /// A line names its account without the whole address, and accounts that differ only in
    /// their trailing digits or their domain stay apart.
    #[test]
    fn an_account_is_named_masked_and_distinct() {
        assert_eq!(account_label("name4@example.io"), "nam\u{2026}4@ex\u{2026}");
        assert_eq!(account_label("jo@mail.example.com"), "jo@ma\u{2026}");
        assert_eq!(account_label("1234567@example.com"), "\u{2026}567@ex\u{2026}");
        let emails = ["name1@example.com", "name2@example.com", "name12@example.com", "name1@other.org", "alice@example.com", "bob@example.com"];
        let labels: Vec<String> = emails.iter().map(|e| account_label(e)).collect();
        for (email, label) in emails.iter().zip(&labels) {
            let (local, domain) = email.split_once('@').unwrap();
            assert!(!label.contains(email) && !(label.contains(local) && label.contains(domain)), "{label}");
        }
        let mut distinct = labels.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), labels.len(), "{labels:?}");
    }

    /// A line never wraps: narrower panels drop words, reset times and bars in turn, and what
    /// is drawn fits the line (Claude's three limits in the tray dropdown keep their bars and
    /// resets; Codex's two beside its reset button get the full words in the window).
    #[test]
    fn a_line_fits_its_width_by_dropping_detail() {
        let reset_button = 34.0;
        let dropdown = 460.0 - 32.0;
        let window = 560.0 - 32.0;
        assert_eq!(fit(dropdown, 3, 0.0), Fit::Compact);
        assert_eq!(fit(window, 2, reset_button), Fit::Full);
        assert_eq!(fit(280.0, 3, 0.0), Fit::Tight);
        assert_eq!(fit(200.0, 3, 0.0), Fit::Bare);
        for w in [180.0, 240.0, 300.0, 360.0, 428.0, 528.0, 700.0] {
            for (cells, trailing) in [(1, 0.0), (2, reset_button), (3, 0.0)] {
                let f = fit(w, cells, trailing);
                let used = DOT_W + LABEL_MIN + cells as f64 * f.cell_w() + trailing + CHEVRON_W;
                assert!(used <= w || f == Fit::Bare, "{w}: {f:?} takes {used}");
                assert!(f.parts().iter().sum::<f64>() < f.cell_w());
            }
        }
        assert_eq!(short_countdown(45 * 60, 1), "45m");
        assert_eq!(short_countdown(5 * 3600 + 59 * 60, 1), "5h");
        assert_eq!(short_countdown(2 * 3600 + 13 * 60, 2), "2h 13m");
        assert_eq!(short_countdown(6 * 86400 + 2 * 3600 + 5, 2), "6d 2h");
        assert_eq!(short_countdown(5 * 86400, 2), "5d");
    }

    /// A reset that went through says nothing (the bars show it); one that changed nothing or
    /// failed says so under the header for a few minutes.
    #[test]
    fn a_reset_that_changed_nothing_says_so_for_a_while() {
        let now = 1_800_000_000;
        let note = |outcome| ProviderState { provider: Provider::Codex, reset: Some(ResetNote { at: now, account: "user@example.com".into(), outcome }), ..Default::default() };
        assert_eq!(reset_note(&note(Ok(ResetOutcome::Reset)), now), None);
        assert_eq!(reset_note(&note(Ok(ResetOutcome::AlreadyRedeemed)), now), None);
        assert_eq!(reset_note(&note(Ok(ResetOutcome::NothingToReset)), now).map(|n| n.1), Some(false));
        let (text, warn) = reset_note(&note(Err("Codex refused the reset".into())), now + 60).unwrap();
        assert!(warn && text.contains("refused"));
        assert_eq!(reset_note(&note(Ok(ResetOutcome::NoCredit)), now + RESET_NOTE_SECS), None, "gone after a while");
        let mut s = note(Ok(ResetOutcome::Reset));
        s.busy = Busy::Resetting;
        assert_eq!(status_text(&s, now).0, "resetting the limits\u{2026}");
    }
}
