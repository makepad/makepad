//! JobStrip — the status bar's view of the app's long operations, and the
//! JobList that its popover shows.
//!
//! The work itself is registered on the process's job board
//! ([`makepad_jobs`], re-exported as `jobs`): a model rebuild, an export, a
//! generation on an AI hub node. The strip reads that board and says what
//! is running in plain words, "Building motor mount · fillets 3/7", over a
//! thin bar, with a × that cancels it.
//!
//! **Quick work never flashes.** A running job shows only once it has run
//! for `show_after` seconds (0.3 by default): a rebuild that answers in a
//! frame or two never puts anything in the status bar.
//!
//! **One line, the newest.** With several jobs the strip shows the newest
//! and counts the rest ("+2"); a press on the line opens the popover, which
//! lists every job with where it runs, its stage, its bar, its time and its
//! cancel, and below them the recent finished and failed ones with the
//! reason in words, a retry where trying again can help, and the next step
//! a result is waiting for ("Place it").
//!
//! **Endings that need the user stay.** A failure stays on the strip for
//! `failure_secs` and in the history after; a result that waits for a next
//! step stays until the user takes it or clears it. A plain success leaves
//! at once: the work is visible in what it changed.
//!
//! The app hears the user's answers to an ended job as [`JobAction`]s
//! (`job_actions`): the next step, a retry. Cancel and clear are the board's
//! own business and need nothing from the app.
//!
//! Main-loop cost: the strip reads the board's sequence number on
//! `Event::Signal` and redraws only when it moved; a draw copies the board
//! (a few jobs). While the popover is open it redraws once a second for the
//! elapsed times; an indeterminate bar moves on the pass clock.

use crate::{
    button::{ButtonWidgetExt, ButtonWidgetRefExt},
    makepad_derive_widget::*,
    makepad_draw::*,
    popover::PopoverWidgetExt,
    progress::{Intent, ProgressBarWidgetRefExt},
    view::View,
    widget::*,
    widget_tree::CxWidgetExt,
};
use makepad_jobs::{board, JobEnd, JobId, JobSnapshot};
use std::collections::HashMap;

/// What the user asked of an ended job, for the app to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum JobAction {
    /// Take the next step the job's result is waiting for (place the mesh).
    Next(JobId),
    /// Run the failed job again.
    Retry(JobId),
}

/// The job answers in this pass's actions.
pub fn job_actions(actions: &Actions) -> impl Iterator<Item = &JobAction> {
    actions.iter().filter_map(|a| a.downcast_ref::<JobAction>())
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.JobListBase = #(JobList::register_widget(vm))
    /** Every job on the board: the running ones with their stage, bar, time
     * and cancel, then the recent finished and failed ones. */
    mod.widgets.JobList = set_type_default() do mod.widgets.JobListBase{
        width: 380
        height: Fit
        flow: Down
        spacing: theme.space_1

        /** the line under the running jobs, above the finished ones */
        history_label: "Finished"
        /** what the list says when the board is empty */
        empty_label: "Nothing has run yet"

        Heading := Label{
            width: Fill
            margin: Inset{top: theme.space_1}
            draw_text +: {
                color: theme.color_text_meta
                text_style: theme.font_regular{font_size: theme.font_size_base - 1.0}
            }
            text: ""
        }

        Row := View{
            width: Fill
            height: Fit
            flow: Down
            spacing: 0
            padding: Inset{top: 2, bottom: 4}

            head := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                align: Align{y: 0.5}

                title := Label{
                    padding: 0
                    margin: 0
                    width: Fill
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: theme.font_size_base}
                    }
                    text: ""
                }
                elapsed := Label{
                    padding: 0
                    margin: 0
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_regular{font_size: theme.font_size_base - 1.0}
                    }
                    text: ""
                }
                next := ButtonFlat{ text: "" }
                retry := ButtonFlat{ text: "Retry" }
                cancel := ButtonFlat{ text: "Cancel" }
                clear := ButtonFlatter{ text: "Clear" }
            }
            detail := Label{
                    padding: 0
                    margin: 0
                width: Fill
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: theme.font_size_base - 1.0}
                }
                text: ""
            }
            failure := Label{
                    padding: 0
                    margin: 0
                width: Fill
                draw_text +: {
                    color: theme.color_error
                    text_style: theme.font_regular{font_size: theme.font_size_base - 1.0}
                }
                text: ""
            }
            bar := ProgressBar{
                width: Fill
                height: 3
                margin: Inset{top: 3}
                draw_bg +: {thickness: 3.0}
            }
        }
    }

    mod.widgets.JobStripBase = #(JobStrip::register_widget(vm))
    /** The status bar's job strip: the newest long operation in words over a
     * thin bar, a count of the others, and a cancel; a press opens the list
     * of every job. Shows nothing until a job has run for `show_after`. */
    mod.widgets.JobStrip = set_type_default() do mod.widgets.JobStripBase{
        width: Fit
        height: Fill
        flow: Right
        align: Align{y: 0.5}

        /** a running job shows once it has run this long, in seconds 0..2 step 0.05 */
        show_after: 0.3
        /** a failure stays on the strip this long, in seconds 0..60 step 1 */
        failure_secs: 10.0
        /** the longest line the strip shows before it shortens it, in characters 16..160 step 1 */
        max_chars: 64

        pop := Popover{
            width: Fit
            height: Fill
            flow: Right
            align: Align{y: 0.5}
            spacing: 2
            trigger: Manual
            placement: TopEnd

            // The job's words over a thin bar drawn in the same face: one
            // control, as wide as its words, that opens the list.
            line := ButtonFlatter{
                height: Fit
                padding: Inset{left: 4, right: 4, top: 4, bottom: 4}
                text: ""
                draw_text +: {
                    text_style: theme.font_regular{font_size: theme.font_size_base - 1.0}
                }
                draw_bg +: {
                    /** below -1.5: no bar; -1: a sweep (unknown); 0..1: how far */
                    progress: instance(-2.0)
                    /** 1 paints the bar in the failure colour */
                    failed: instance(0.0)
                    color_bar: uniform(theme.color_primary)
                    color_track: uniform(theme.color_outline_variant)
                    color_fail: uniform(theme.color_error)
                    /** bar thickness in points 1..4 step 0.5 */
                    bar_height: uniform(2.0)
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        if self.progress > -1.5 {
                            let x = 4.0
                            let w = max(self.rect_size.x - 8.0, 1.0)
                            let y = self.rect_size.y - self.bar_height - 1.0
                            sdf.rect(x, y, w, self.bar_height)
                            sdf.fill(self.color_track)
                            let seg = w * 0.35
                            let t = fract(self.draw_pass.time * 0.7)
                            let head = x - seg + (w + seg) * t
                            let sweep_from = max(head, x)
                            let sweep_to = min(head + seg, x + w)
                            let from = mix(x, sweep_from, step(self.progress, -0.5))
                            let to = mix(x + w * clamp(self.progress, 0.0, 1.0), sweep_to, step(self.progress, -0.5))
                            sdf.rect(from, y, max(to - from, 0.0), self.bar_height)
                            sdf.fill(mix(self.color_bar, self.color_fail, self.failed))
                        }
                        return sdf.result
                    }
                }
            }
            more := ButtonFlatter{
                height: Fit
                padding: Inset{left: 3, right: 3, top: 1, bottom: 1}
                text: ""
                a11y_name: "Show all jobs"
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: theme.font_size_base - 1.0}
                }
            }
            cancel := ButtonFlatter{
                height: Fit
                padding: Inset{left: 3, right: 3, top: 1, bottom: 1}
                text: "×"
                a11y_name: "Cancel job"
            }
            content := View{
                width: Fit
                height: Fit
                list := mod.widgets.JobList{}
            }
        }
    }
}

/// `seconds` as a short time: "4 s", "1:05", "1:02:09".
pub fn job_time(seconds: f64) -> String {
    let s = seconds.max(0.0);
    if s < 10.0 {
        return format!("{:.1} s", s);
    }
    let s = s as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    }
}

/// `line` cut to `max` characters, the cut marked.
fn shorten(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Set a child's text only when it changes: a set redraws, and a draw that
/// redraws itself every time never rests.
fn put_text(cx: &mut Cx, w: &WidgetRef, text: &str) {
    if w.text() != text {
        w.set_text(cx, text);
    }
}

fn put_visible(cx: &mut Cx, w: &WidgetRef, visible: bool) {
    if w.visible() != visible {
        w.set_visible(cx, visible);
    }
}

fn put_bar(cx: &mut Cx, w: &WidgetRef, value: f64, intent: Intent) {
    let bar = w.as_progress_bar();
    bar.set_intent(cx, intent);
    let value = if value < 0.0 { -1.0 } else { value.min(1.0) };
    if bar.value() != value {
        bar.set_value(cx, value);
    }
}

/// The bar's value for a job: its fraction, or below zero (a sweep) when it
/// cannot say.
fn bar_value(snap: &JobSnapshot) -> f64 {
    snap.fraction.map_or(-1.0, |f| f as f64)
}

/// The order the list shows jobs in: the running ones newest first, then
/// the ended ones newest first.
fn listed(jobs: &[JobSnapshot]) -> (Vec<&JobSnapshot>, Vec<&JobSnapshot>) {
    let mut running: Vec<&JobSnapshot> = jobs.iter().filter(|j| j.end.is_running()).collect();
    let mut ended: Vec<&JobSnapshot> = jobs.iter().filter(|j| !j.end.is_running()).collect();
    running.sort_by(|a, b| b.id.cmp(&a.id));
    ended.sort_by(|a, b| b.finished.unwrap_or(0.0).total_cmp(&a.finished.unwrap_or(0.0)));
    (running, ended)
}

#[derive(Script, Widget)]
pub struct JobList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    history_label: String,
    #[live]
    empty_label: String,
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    rows: HashMap<JobId, WidgetRef>,
    #[rust]
    headings: HashMap<LiveId, WidgetRef>,
}

impl JobList {
    fn template(&mut self, cx: &mut Cx, name: LiveId) -> Option<WidgetRef> {
        let template = self.templates.get(&name)?;
        let value: ScriptValue = template.as_object().into();
        Some(cx.with_vm(|vm| WidgetRef::script_from_value(vm, value)))
    }

    fn row(&mut self, cx: &mut Cx, id: JobId) -> Option<WidgetRef> {
        if let Some(row) = self.rows.get(&id) {
            return Some(row.clone());
        }
        let row = self.template(cx, live_id!(Row))?;
        cx.widget_tree_insert_child(self.uid, LiveId(id.0), row.clone());
        self.rows.insert(id, row.clone());
        Some(row)
    }

    fn heading(&mut self, cx: &mut Cx, key: LiveId) -> Option<WidgetRef> {
        if let Some(h) = self.headings.get(&key) {
            return Some(h.clone());
        }
        let h = self.template(cx, live_id!(Heading))?;
        cx.widget_tree_insert_child(self.uid, key, h.clone());
        self.headings.insert(key, h.clone());
        Some(h)
    }

    /// Write one row from its job.
    fn dress(cx: &mut Cx, row: &WidgetRef, snap: &JobSnapshot, now: f64) {
        put_text(cx, &row.widget(cx, ids!(head.title)), &snap.title);
        put_text(cx, &row.widget(cx, ids!(head.elapsed)), &job_time(snap.elapsed(now)));
        let running = snap.end.is_running();
        let mut detail = snap.machine.clone();
        let mut failure = String::new();
        match &snap.end {
            JobEnd::Running => {
                let stage = if snap.cancelling { "cancelling".to_string() } else { snap.stage_words() };
                if !stage.is_empty() {
                    detail.push_str(" · ");
                    detail.push_str(&stage);
                }
                if let Some(f) = snap.fraction.filter(|_| !snap.cancelling) {
                    detail.push_str(&format!(" · {:.0}%", f * 100.0));
                }
            }
            JobEnd::Done { next } => {
                detail.push_str(if next.is_some() { " · ready" } else { " · done" });
            }
            JobEnd::Failed { reason, .. } => failure = reason.clone(),
            JobEnd::Cancelled => {
                detail.push_str(" · cancelled");
                if let Some(latency) = snap.cancel_latency {
                    detail.push_str(&format!(", stopped in {:.0} ms", latency * 1000.0));
                }
            }
        }
        put_text(cx, &row.widget(cx, ids!(detail)), &detail);
        let fail = row.widget(cx, ids!(failure));
        put_text(cx, &fail, &failure);
        put_visible(cx, &fail, !failure.is_empty());
        let next = row.widget(cx, ids!(head.next));
        match &snap.end {
            JobEnd::Done { next: Some(step) } if !snap.dismissed => {
                put_text(cx, &next, step);
                put_visible(cx, &next, true);
            }
            _ => put_visible(cx, &next, false),
        }
        put_visible(cx, &row.widget(cx, ids!(head.retry)), matches!(snap.end, JobEnd::Failed { retry: true, .. }) && !snap.dismissed);
        put_visible(cx, &row.widget(cx, ids!(head.cancel)), running && snap.cancellable && !snap.cancelling);
        put_visible(cx, &row.widget(cx, ids!(head.clear)), !running && !snap.dismissed);
        let bar = row.widget(cx, ids!(bar));
        put_visible(cx, &bar, running);
        if running {
            put_bar(cx, &bar, bar_value(snap), Intent::Primary);
        }
    }
}

impl ScriptHook for JobList {
    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if apply.is_reload() {
            self.templates.clear();
            self.rows.clear();
            self.headings.clear();
        }
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, value: ScriptValue) {
        if apply.is_eval() {
            return;
        }
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let (Some(id), Some(template)) = (kv.key.as_id(), kv.value.as_object()) {
                        self.templates.insert(id, vm.new_object_ref(template));
                    }
                }
            });
        }
    }
}

impl Widget for JobList {
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::List;
        makepad_platform::a11y::set_string(&mut node.name, "Jobs");
        node.set_size = self.rows.len() as u32;
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        for (_, row) in self.rows.iter() {
            let row_uid = row.widget_uid();
            cx.group_widget_actions(uid, row_uid, |cx| row.handle_event(cx, event, scope));
        }
        for heading in self.headings.values() {
            heading.handle_event(cx, event, scope);
        }
        // The rows' presses arrive as this pass's actions (a pointer's or an
        // assistive client's alike).
        let Event::Actions(actions) = event else { return };
        let rows: Vec<(JobId, WidgetRef)> = self.rows.iter().map(|(id, row)| (*id, row.clone())).collect();
        for (id, row) in rows {
            if row.button(cx, ids!(head.cancel)).clicked(actions) {
                board().cancel(id);
            }
            if row.button(cx, ids!(head.clear)).clicked(actions) {
                board().dismiss(id);
            }
            if row.button(cx, ids!(head.next)).clicked(actions) {
                board().dismiss(id);
                cx.action(JobAction::Next(id));
            }
            if row.button(cx, ids!(head.retry)).clicked(actions) {
                board().dismiss(id);
                cx.action(JobAction::Retry(id));
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let jobs = board().snapshot();
        let now = makepad_jobs::now();
        // Rows of jobs the board no longer holds go.
        self.rows.retain(|id, _| jobs.iter().any(|j| j.id == *id));
        cx.begin_turtle(walk, self.layout);
        let (running, ended) = listed(&jobs);
        if jobs.is_empty() {
            let text = self.empty_label.clone();
            if let Some(h) = self.heading(cx.cx.cx, live_id!(empty)) {
                h.set_text(cx.cx.cx, &text);
                h.draw_all(cx, scope);
            }
        }
        for snap in &running {
            if let Some(row) = self.row(cx.cx.cx, snap.id) {
                Self::dress(cx.cx.cx, &row, snap, now);
                row.draw_all(cx, scope);
            }
        }
        if !ended.is_empty() {
            let text = self.history_label.clone();
            if let Some(h) = self.heading(cx.cx.cx, live_id!(history)) {
                h.set_text(cx.cx.cx, &text);
                h.draw_all(cx, scope);
            }
        }
        for snap in &ended {
            if let Some(row) = self.row(cx.cx.cx, snap.id) {
                Self::dress(cx.cx.cx, &row, snap, now);
                row.draw_all(cx, scope);
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

/// What the strip is showing, from the last draw.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    job: Option<JobId>,
    others: usize,
}

#[derive(Script, ScriptHook, Widget)]
pub struct JobStrip {
    #[deref]
    view: View,
    /// A running job shows once it has run this long (seconds).
    #[live(0.3)]
    show_after: f64,
    /// A failure stays on the strip this long (seconds).
    #[live(10.0)]
    failure_secs: f64,
    /// The longest line before it is shortened (characters).
    #[live(64usize)]
    max_chars: usize,
    #[rust]
    seen_seq: u64,
    #[rust]
    shown: Shown,
    /// Wakes the strip when a waiting job is due to show or a failure to leave.
    #[rust]
    due_timer: Timer,
    #[rust]
    due_at: Option<f64>,
    /// Ticks the elapsed times while the list is open.
    #[rust]
    tick_timer: Timer,
    /// The line's bar as last written: (progress, failed).
    #[rust]
    line_bar: Option<(f32, f32)>,
}

impl JobStrip {
    /// The job the strip shows, how many others it counts, and when the
    /// picture changes next without the board changing (a job coming due,
    /// a failure leaving).
    fn pick(&self, jobs: &[JobSnapshot], now: f64) -> (Shown, Option<f64>) {
        let mut due: Option<f64> = None;
        let mut soon = |t: f64| due = Some(due.map_or(t, |d: f64| d.min(t)));
        let mut eligible: Vec<&JobSnapshot> = Vec::new();
        // Jobs the app shows elsewhere are counted, never the line.
        let mut elsewhere = 0usize;
        for job in jobs {
            if job.shown_elsewhere {
                if job.end.is_running() && now >= job.started + self.show_after {
                    elsewhere += 1;
                }
                continue;
            }
            match &job.end {
                JobEnd::Running => {
                    let at = job.started + self.show_after;
                    if now >= at {
                        eligible.push(job);
                    } else {
                        soon(at);
                    }
                }
                JobEnd::Failed { .. } if !job.dismissed => {
                    let until = job.finished.unwrap_or(now) + self.failure_secs;
                    if now < until {
                        eligible.push(job);
                        soon(until);
                    }
                }
                JobEnd::Done { next: Some(_) } if !job.dismissed => eligible.push(job),
                _ => {}
            }
        }
        // The newest running job wins; with none running, the newest ending.
        let newest = eligible
            .iter()
            .filter(|j| j.end.is_running())
            .max_by_key(|j| j.id)
            .or_else(|| eligible.iter().max_by(|a, b| a.finished.unwrap_or(0.0).total_cmp(&b.finished.unwrap_or(0.0))))
            .map(|j| j.id);
        let others = if newest.is_some() { eligible.len() - 1 + elsewhere } else { 0 };
        (Shown { job: newest, others }, due)
    }

    fn arm_due(&mut self, cx: &mut Cx, due: Option<f64>, now: f64) {
        match due {
            Some(at) if self.due_at != Some(at) => {
                cx.stop_timer(self.due_timer);
                self.due_timer = cx.start_timeout((at - now).max(0.0) + 0.001);
                self.due_at = Some(at);
            }
            None if self.due_at.is_some() => {
                cx.stop_timer(self.due_timer);
                self.due_timer = Timer::empty();
                self.due_at = None;
            }
            _ => {}
        }
    }

    /// The job on the strip now, for a host (or a test) that asks.
    pub fn shown_job(&self) -> Option<JobId> {
        self.shown.job
    }

    /// Write the line's bar when it changed: (progress, failed), progress
    /// below -1.5 for none and -1 for a sweep.
    fn put_line_bar(&mut self, cx: &mut Cx, line: &WidgetRef, bar: (f32, f32)) {
        let bar = (if bar.0 >= 0.0 { bar.0.min(1.0) } else if bar.0 > -1.5 { -1.0 } else { -2.0 }, bar.1);
        if self.line_bar == Some(bar) {
            return;
        }
        self.line_bar = Some(bar);
        let (progress, failed) = (bar.0 as f64, bar.1 as f64);
        let mut line = line.clone();
        script_apply_eval!(cx, line, { draw_bg +: { progress: #(progress) failed: #(failed) } });
        line.redraw(cx);
    }

    /// Redraw the strip and, when it is open, the list.
    fn refresh(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        let pop = self.view.popover(cx, ids!(pop));
        if pop.is_open() {
            pop.content().redraw(cx);
        }
    }

    fn list_open(&self, cx: &mut Cx) -> bool {
        self.view.popover(cx, ids!(pop)).is_open()
    }
}

impl Widget for JobStrip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.due_timer.is_event(event).is_some() {
            self.due_at = None;
            self.refresh(cx);
        }
        if self.tick_timer.is_event(event).is_some() {
            if self.list_open(cx) {
                self.tick_timer = cx.start_timeout(1.0);
                self.refresh(cx);
            } else {
                self.tick_timer = Timer::empty();
            }
        }
        if let Event::Signal = event {
            let seq = board().seq();
            if seq != self.seen_seq {
                self.seen_seq = seq;
                self.refresh(cx);
            }
        }
        self.view.handle_event(cx, event, scope);
        // Presses arrive as this pass's actions (a pointer's or an assistive client's alike).
        let Event::Actions(actions) = event else { return };
        let pop = self.view.popover(cx, ids!(pop));
        let opens = self.view.button(cx, ids!(pop.line)).clicked(actions) || self.view.button(cx, ids!(pop.more)).clicked(actions);
        if opens {
            pop.toggle(cx);
        }
        if pop.opened(actions) {
            cx.stop_timer(self.tick_timer);
            self.tick_timer = cx.start_timeout(1.0);
        }
        if self.view.button(cx, ids!(pop.cancel)).clicked(actions) {
            if let Some(id) = self.shown.job {
                let running = board().get(id).is_some_and(|j| j.end.is_running());
                if running {
                    board().cancel(id);
                } else {
                    board().dismiss(id);
                }
            }
        }
        if job_actions(actions).next().is_some() || pop.opened(actions) || pop.closed(actions) || pop.dismissed(actions) {
            self.refresh(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let jobs = board().snapshot();
        let now = makepad_jobs::now();
        let (shown, due) = self.pick(&jobs, now);
        self.arm_due(cx.cx.cx, due, now);
        self.shown = shown.clone();
        let open = self.list_open(cx.cx.cx);
        let job = shown.job.and_then(|id| jobs.iter().find(|j| j.id == id));
        // Nothing to say: the strip takes no room (the list stays reachable
        // while it is open).
        let visible = job.is_some() || open;
        let pop = self.view.widget(cx, ids!(pop));
        let line_w = self.view.widget(cx, ids!(pop.line));
        let cancel_w = self.view.widget(cx, ids!(pop.cancel));
        let more_w = self.view.widget(cx, ids!(pop.more));
        let cx1 = &mut *cx.cx.cx;
        put_visible(cx1, &pop, visible);
        if let Some(job) = job {
            put_text(cx1, &line_w, &shorten(&job.line(), self.max_chars));
            let bar = match &job.end {
                JobEnd::Running => (bar_value(job) as f32, 0.0),
                JobEnd::Failed { .. } => (1.0, 1.0),
                _ => (-2.0, 0.0),
            };
            self.put_line_bar(cx1, &line_w, bar);
            put_visible(cx1, &cancel_w, !job.end.is_running() || (job.cancellable && !job.cancelling));
            put_visible(cx1, &more_w, shown.others > 0);
            put_text(cx1, &more_w, &format!("+{}", shown.others));
        } else if open {
            put_text(cx1, &line_w, "Jobs");
            self.put_line_bar(cx1, &line_w, (-2.0, 0.0));
            put_visible(cx1, &cancel_w, false);
            put_visible(cx1, &more_w, false);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Status;
        makepad_platform::a11y::set_string(&mut node.name, "Jobs");
    }
}

impl JobStripRef {
    /// The job on the strip now.
    pub fn shown_job(&self) -> Option<JobId> {
        self.borrow().and_then(|inner| inner.shown_job())
    }
}
