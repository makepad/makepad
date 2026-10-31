//! The BUILD centre tab (run lane): one build of the run manager -- the
//! newest unless the person picks an older one -- with its header (app,
//! goal, state, elapsed, a thin progress bar, Cancel while it builds), its
//! problems (errors first; a click opens `file:line` through
//! `IdeAction::OpenFile`) and its output tail. "logs" in the RUN sidebar
//! shows a running app's output here instead (`show`).
//!
//! The RUN sidebar (run.rs) uses the small parts this file defines
//! (`mod.widgets.Run*`): ide/mod.rs registers this module first.

use super::{IdeAction, Workspace};
use makepad_code_agent::runs::{Goal, JobState, JobView, Level, RunManager, RunningView};
use crate::ui::theme::{palette, vec};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.BuildOutputBase = #(BuildOutput::register_widget(vm))

    // ---- the run lane's small parts (also the RUN sidebar's) -------------

    mod.widgets.RunMono = Label{
        width: Fit
        height: Fit
        padding: 0
        text: ""
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    mod.widgets.RunCaption = mod.widgets.RunMono{
        draw_text +: {
            color: mod.look.dim
            text_style: mod.look.mono_bold{font_size: 7.5}
        }
    }

    mod.widgets.RunRule = View{
        width: Fill
        height: 1
        show_bg: true
        draw_bg +: {
            color: mod.look.hair_hi
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
    }

    mod.widgets.RunSection = View{
        width: Fill
        height: 20
        flow: Right
        spacing: 8
        align: Align{y: 0.5}
        padding: Inset{left: 2 right: 2}
    }

    /// The Code look's key button, small enough for a 20-point row.
    mod.widgets.RunKeyBtn = ButtonFlat{
        height: 18
        padding: Inset{left: 6 right: 6 top: 0 bottom: 0}
        margin: 0
        draw_text +: {
            color: mod.look.text
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono{font_size: 8.0}
        }
        draw_bg +: {
            ink: uniform(mod.look.meta)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 3.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let wash = 0.05 + 0.12 * self.hover + 0.22 * self.down
                let a = clamp(fill * wash + edge * (0.55 + 0.45 * max(self.hover, self.focus)), 0.0, 1.0)
                return vec4(self.ink.rgb * a, a)
            }
        }
    }

    /// A list row: a wash of `color` with a bar on its left when selected.
    mod.widgets.RunRow = View{
        width: Fill
        height: 20
        flow: Right
        spacing: 6
        align: Align{y: 0.5}
        padding: Inset{left: 6 right: 4}
        show_bg: true
        draw_bg +: {
            color: #0000
            pixel: fn() {
                let p = self.pos * self.rect_size
                let bar = (1.0 - step(2.0, p.x)) * step(0.001, self.color.a)
                let c = mix(self.color, mod.look.agent, bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    /// A thin progress bar: `fill` (0..1) of `color` over a faint track.
    mod.widgets.RunBar = View{
        width: Fill
        height: 3
        show_bg: true
        draw_bg +: {
            color: mod.look.agent
            fill: uniform(0.0)
            pixel: fn() {
                let on = 1.0 - step(self.fill, self.pos.x)
                let a = mix(0.10, 0.85, on) * self.color.a
                return vec4(self.color.rgb * a, a)
            }
        }
    }

    mod.widgets.RunLogRow = View{
        width: Fill
        height: 15
        padding: Inset{left: 6}
        log_line := mod.widgets.RunMono{
            width: Fill
            max_lines: 1
            text_overflow: Ellipsis
            draw_text +: {
                color: mod.look.terminal_text
                text_style: mod.look.mono{font_size: 8.0}
            }
        }
    }

    // ---- the tab ---------------------------------------------------------

    mod.widgets.BuildOutput = set_type_default() do mod.widgets.BuildOutputBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 2
        padding: Inset{left: 14 right: 12 top: 8 bottom: 6}
        head := View{
            width: Fill
            height: 22
            flow: Right
            spacing: 10
            align: Align{y: 0.5}
            title := mod.widgets.RunMono{
                text: "nothing built yet"
                draw_text +: {
                    color: mod.look.text_hi
                    text_style: mod.look.mono_bold{font_size: 9.5}
                }
            }
            goal := mod.widgets.RunMono{ draw_text +: { color: mod.look.meta } }
            status := mod.widgets.RunMono{
                width: Fill
                max_lines: 1
                text_overflow: Ellipsis
                draw_text +: { color: mod.look.meta }
            }
            older_btn := mod.widgets.RunKeyBtn{ text: "‹ older" }
            pick := mod.widgets.RunMono{ draw_text +: { color: mod.look.dim text_style: mod.look.mono{font_size: 7.5} } }
            newer_btn := mod.widgets.RunKeyBtn{ text: "newer ›" }
            cancel_btn := mod.widgets.RunKeyBtn{ text: "cancel" visible: false }
            stop_btn := mod.widgets.RunKeyBtn{ text: "stop" visible: false }
        }
        bar := mod.widgets.RunBar{}
        problems_head := mod.widgets.RunSection{
            problems_caption := mod.widgets.RunCaption{ text: "PROBLEMS" }
            mod.widgets.RunRule{}
        }
        problems := PortalList{
            width: Fill
            height: 0
            DiagRow := mod.widgets.RunRow{
                cursor: MouseCursor.Hand
                diag_mark := mod.widgets.RunMono{
                    width: 10
                    draw_text +: { text_style: mod.look.mono_bold{font_size: 8.5} }
                }
                diag_msg := mod.widgets.RunMono{
                    width: Fill
                    max_lines: 1
                    text_overflow: Ellipsis
                }
                diag_loc := mod.widgets.RunMono{
                    draw_text +: {
                        color: mod.look.meta
                        text_style: mod.look.mono{font_size: 8.0}
                    }
                }
            }
        }
        mod.widgets.RunSection{
            output_caption := mod.widgets.RunCaption{ text: "OUTPUT" }
            mod.widgets.RunRule{}
        }
        output := PortalList{
            width: Fill
            height: Fill
            auto_tail: true
            LogRow := mod.widgets.RunLogRow{}
        }
    }
}

/// What the BUILD tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shown {
    /// The newest build, following new ones.
    #[default]
    Newest,
    /// One build the person picked.
    Job(u64),
    /// A running app's output (its registry id: a restart's new run takes over).
    Logs(u64, &'static str),
}

/// The tab's choice, a `Cx` global so the RUN sidebar can set it before it
/// emits `IdeAction::ShowBuild`.
/// The serial counts changes, so the tab redraws on one.
struct ShownGlobal(Shown, u64);

/// Show `what` in the BUILD tab (the caller emits `IdeAction::ShowBuild`).
pub fn show(cx: &mut Cx, what: Shown) {
    // set_global keeps a value already there: install once, then update
    if cx.has_global::<ShownGlobal>() {
        let g = cx.get_global::<ShownGlobal>();
        g.0 = what;
        g.1 += 1;
    } else {
        cx.set_global(ShownGlobal(what, 1));
    }
    SignalToUI::set_ui_signal();
}

fn shown_serial(cx: &Cx) -> u64 {
    cx.get_global_ref::<ShownGlobal>().map_or(0, |s| s.1)
}

pub fn shown(cx: &Cx) -> Shown {
    cx.get_global_ref::<ShownGlobal>().map(|s| s.0).unwrap_or_default()
}

/// Lines of output kept on screen.
const TAIL: usize = 2000;
/// Problem rows shown before the output takes the rest.
const MAX_PROBLEM_ROWS: usize = 14;

#[derive(Script, ScriptHook, Widget)]
pub struct BuildOutput {
    #[deref]
    view: View,
    #[rust]
    seen: u64,
    /// The `show` serial drawn last.
    #[rust]
    seen_shown: u64,
    #[rust]
    hooked: bool,
    /// The build on screen, problems errors first.
    #[rust]
    job: Option<JobView>,
    #[rust]
    run: Option<RunningView>,
    #[rust]
    lines: Vec<String>,
    /// Ticks the elapsed time while a build or the shown app runs.
    #[rust]
    tick: Timer,
}

fn manager() -> &'static RunManager {
    RunManager::global()
}

/// A primary click on a list row (the row's own view action).
pub(crate) fn row_clicked(item: &WidgetRef, actions: &Actions) -> bool {
    actions
        .find_widget_action(item.widget_uid())
        .is_some_and(|a| matches!(a.cast(), ViewAction::FingerUp(f) if !f.cancelled && f.device.is_primary_hit()))
}

pub(crate) fn uptime(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    }
}

/// The goal in words: `build`, `run`, `makepad run`.
pub(crate) fn goal_name(goal: Goal) -> &'static str {
    match goal {
        Goal::Build => "build",
        Goal::Run(mode) => mode.name(),
    }
}

/// The ink of a build's state line.
pub(crate) fn state_ink(state: &JobState) -> Vec4f {
    match state {
        JobState::Failed(_) => vec(palette().error, 1.0),
        JobState::Succeeded => vec(palette().ok, 1.0),
        _ => vec(palette().meta, 1.0),
    }
}

pub(crate) fn bar_ink(state: &JobState) -> Vec4f {
    match state {
        JobState::Failed(_) => vec(palette().error, 1.0),
        JobState::Succeeded => vec(palette().ok, 1.0),
        _ => vec(palette().primary, 1.0),
    }
}

/// A build's bar fill: crates done of all, full once it ended.
pub(crate) fn bar_fill(job: &JobView) -> f64 {
    if job.state.is_done() {
        1.0
    } else if job.progress.total > 0 {
        job.progress.done as f64 / job.progress.total as f64
    } else {
        0.0
    }
}

impl BuildOutput {
    /// The checkout-relative `path` as the workspace's (project-relative) one.
    fn project_relative(cx: &Cx, path: &str) -> String {
        let (Some(ws), Some(root)) = (Workspace::of(cx), manager().root()) else { return path.to_string() };
        ws.relative(&root.join(path)).unwrap_or_else(|| path.to_string())
    }

    /// The jobs the picker steps through, oldest first, and the shown one's index.
    fn pick(&self, cx: &Cx) -> (Vec<u64>, Option<usize>) {
        let ids: Vec<u64> = manager().snapshot().jobs.iter().map(|j| j.id).collect();
        let at = match shown(cx) {
            Shown::Job(id) => ids.iter().position(|j| *j == id),
            Shown::Newest => ids.len().checked_sub(1),
            Shown::Logs(..) => None,
        };
        (ids, at)
    }

    fn refresh(&mut self, cx: &mut Cx) {
        let m = manager();
        self.seen = m.generation();
        let snap = m.snapshot();
        // The shown job or run, falling back to the newest build.
        let mut what = shown(cx);
        if let Shown::Logs(id, app) = what {
            let live = snap.running.iter().any(|r| r.id == id);
            what = if live {
                what
            } else {
                match snap.running.iter().rev().find(|r| r.app.id == app) {
                    Some(r) => Shown::Logs(r.id, app),
                    None => Shown::Newest,
                }
            };
        }
        if let Shown::Job(id) = what {
            if !snap.jobs.iter().any(|j| j.id == id) {
                what = Shown::Newest;
            }
        }
        if what != shown(cx) {
            show(cx, what);
        }
        self.seen_shown = shown_serial(cx);
        self.job = None;
        self.run = None;
        match what {
            Shown::Logs(id, _) => {
                self.run = snap.running.iter().find(|r| r.id == id).cloned();
                self.lines = m.run_log(id, TAIL);
            }
            Shown::Job(id) => self.job = snap.jobs.iter().find(|j| j.id == id).cloned(),
            Shown::Newest => self.job = snap.jobs.last().cloned(),
        }
        if let Some(job) = &mut self.job {
            // errors first, each level in the compiler's order
            job.diags.sort_by_key(|d| d.level != Level::Error);
            self.lines = m.job_log(job.id, TAIL);
        } else if self.run.is_none() {
            self.lines.clear();
        }

        let (title, goal, status, ink, fill, bar, busy) = match (&self.job, &self.run) {
            (Some(j), _) => {
                let title = if j.target == j.app.package { j.app.id.to_string() } else { format!("{} ({})", j.app.id, j.target) };
                (title, goal_name(j.goal).to_string(), j.status_line(), state_ink(&j.state), bar_fill(j), bar_ink(&j.state), !j.state.is_done())
            }
            (None, Some(r)) => {
                let status = match (&r.exited, &r.building) {
                    (None, Some(stage)) => format!("pid {}  {stage}", r.pid),
                    (None, None) => format!("pid {}  up {}", r.pid, uptime(r.uptime)),
                    (Some(how), _) => format!("pid {}  {how}", r.pid),
                };
                let ink = if r.exited.is_none() { vec(palette().ok, 1.0) } else { vec(palette().meta, 1.0) };
                (r.app.id.to_string(), format!("{} output", r.mode.name()), status, ink, 0.0, vec(palette().primary, 0.0), false)
            }
            (None, None) => ("nothing built yet".to_string(), String::new(), String::new(), vec(palette().meta, 1.0), 0.0, vec(palette().primary, 1.0), false),
        };
        self.view.label(cx, ids!(title)).set_text(cx, &title);
        self.view.label(cx, ids!(goal)).set_text(cx, &goal);
        let mut status_label = self.view.label(cx, ids!(status));
        status_label.set_text(cx, &status);
        script_apply_eval!(cx, status_label, { draw_text +: { color: #(ink) } });
        let mut bar_view = self.view.view(cx, ids!(bar));
        script_apply_eval!(cx, bar_view, { draw_bg +: { fill: #(fill) color: #(bar) } });
        self.view.button(cx, ids!(cancel_btn)).set_visible(cx, busy);
        let stoppable = self.run.as_ref().is_some_and(|r| r.exited.is_none());
        self.view.button(cx, ids!(stop_btn)).set_visible(cx, stoppable);

        let (ids, at) = self.pick(cx);
        let picking = ids.len() > 1 && self.job.is_some();
        self.view.button(cx, ids!(older_btn)).set_visible(cx, picking && at.is_some_and(|i| i > 0));
        self.view.button(cx, ids!(newer_btn)).set_visible(cx, picking && at.is_some_and(|i| i + 1 < ids.len()));
        let pick_text = match at {
            Some(i) if picking => format!("{}/{}", i + 1, ids.len()),
            _ => String::new(),
        };
        self.view.label(cx, ids!(pick)).set_text(cx, &pick_text);

        let rows = self.job.as_ref().map_or(0, |j| j.diags.len());
        self.view.view(cx, ids!(problems_head)).set_visible(cx, self.job.is_some());
        let (e, w) = self.job.as_ref().map(|j| (j.errors(), j.warnings())).unwrap_or((0, 0));
        let caption = if e + w > 0 { format!("PROBLEMS  {e} errors  {w} warnings") } else { "PROBLEMS  none".to_string() };
        self.view.label(cx, ids!(problems_caption)).set_text(cx, &caption);
        let h = (rows.min(MAX_PROBLEM_ROWS) * 20) as f64;
        let mut problems = self.view.portal_list(cx, ids!(problems));
        script_apply_eval!(cx, problems, { height: #(h) });
        let out = if self.run.is_some() { "OUTPUT" } else { "BUILD OUTPUT" };
        self.view.label(cx, ids!(output_caption)).set_text(cx, out);

        let live = busy || stoppable;
        if live && self.tick.is_empty() {
            self.tick = cx.start_interval(1.0);
        } else if !live && !self.tick.is_empty() {
            cx.stop_timer(self.tick);
            self.tick = Timer::empty();
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.view.button(cx, ids!(cancel_btn)).clicked(actions) {
            if let Some(j) = &self.job {
                manager().cancel(j.id);
            }
        }
        if self.view.button(cx, ids!(stop_btn)).clicked(actions) {
            if let Some(r) = &self.run {
                let _ = manager().stop(r.id);
            }
        }
        let step: isize = if self.view.button(cx, ids!(older_btn)).clicked(actions) {
            -1
        } else if self.view.button(cx, ids!(newer_btn)).clicked(actions) {
            1
        } else {
            0
        };
        if step != 0 {
            let (ids, at) = self.pick(cx);
            if let Some(at) = at {
                let next = (at as isize + step).clamp(0, ids.len() as isize - 1) as usize;
                show(cx, if next + 1 == ids.len() { Shown::Newest } else { Shown::Job(ids[next]) });
            }
            self.view.redraw(cx);
        }
        let problems = self.view.portal_list(cx, ids!(problems));
        for (index, item) in problems.items_with_actions(actions) {
            if !row_clicked(&item, actions) {
                continue;
            }
            let Some(d) = self.job.as_ref().and_then(|j| j.diags.get(index)) else { continue };
            if let Some(path) = &d.path {
                let path = Self::project_relative(cx, path);
                cx.widget_action(self.view.widget_uid(), IdeAction::OpenFile { path, line: Some(d.line), col: Some(d.col) });
            }
        }
    }

    fn draw_problems(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let diags = self.job.as_ref().map(|j| j.diags.clone()).unwrap_or_default();
        list.set_item_range(cx, 0, diags.len());
        while let Some(index) = list.next_visible_item(cx) {
            let Some(d) = diags.get(index) else { continue };
            let row = list.item(cx, index, id!(DiagRow));
            let (mark, ink) = match d.level {
                Level::Error => ("✗", vec(palette().error, 1.0)),
                Level::Warning => ("!", vec(palette().warning, 1.0)),
            };
            let mut m = row.label(cx, ids!(diag_mark));
            m.set_text(cx, mark);
            script_apply_eval!(cx, m, { draw_text +: { color: #(ink) } });
            row.label(cx, ids!(diag_msg)).set_text(cx, &d.message);
            row.label(cx, ids!(diag_loc)).set_text(cx, &d.location());
            row.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_output(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.lines.len());
        while let Some(index) = list.next_visible_item(cx) {
            let Some(line) = self.lines.get(index) else { continue };
            let row = list.item(cx, index, id!(LogRow));
            row.label(cx, ids!(log_line)).set_text(cx, line);
            row.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for BuildOutput {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Signal = event {
            if self.hooked && (manager().generation() != self.seen || shown_serial(cx) != self.seen_shown) {
                self.view.redraw(cx);
            }
        }
        if self.tick.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !actions.is_empty() {
            self.handle_actions(cx, &actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.hooked {
            self.hooked = true;
            // the RUN sidebar set the manager up; a tab restored first does it here
            if let Some(root) = Workspace::of(cx).map(|w| w.root.clone()) {
                manager().set_root(&root);
            }
            manager().on_change(std::sync::Arc::new(|| SignalToUI::set_ui_signal()));
        }
        self.refresh(cx);
        let problems = self.view.portal_list(cx, ids!(problems)).widget_uid();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let uid = item.widget_uid();
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                if uid == problems {
                    self.draw_problems(cx, &mut list);
                } else {
                    self.draw_output(cx, &mut list);
                }
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptimes_read_short() {
        use std::time::Duration;
        assert_eq!(uptime(Duration::from_secs(42)), "42s");
        assert_eq!(uptime(Duration::from_secs(125)), "2m05s");
        assert_eq!(uptime(Duration::from_secs(7300)), "2h01m");
    }
}
