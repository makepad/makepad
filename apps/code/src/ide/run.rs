//! The RUN section of the sidebar (run lane): the repo's apps from the
//! shared registry (`makepad-app-registry`) with sources in the workspace,
//! the apps running now, and one line with the current build. The selected
//! app shows its actions (build, run: release build then the binary with
//! `--remote`, makepad run, stop); the build itself opens in the centre
//! BUILD tab (build_output.rs) through `IdeAction::ShowBuild`, also when the
//! agent starts one.
//!
//! The panel is a view of the process-wide run manager (agent/runs.rs) that
//! the agent's build/run tools drive too.
//!
//! Keys (with the panel focused, after a click on a row or Esc in the
//! filter): Up/Down select, b build, r run, j makepad run, s stop. In the
//! filter Up/Down select too and Return runs.

use super::build_output::{self, bar_fill, bar_ink, row_clicked, state_ink, uptime, Shown};
use super::{IdeAction, Workspace};
use makepad_code_agent::runs::{Mode, RunManager, Snapshot, Started};
use crate::ui::theme::{palette, vec};
use makepad_app_registry::AppEntry;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RunPanelBase = #(RunPanel::register_widget(vm))

    let Mono = mod.widgets.RunMono{}
    let KeyBtn = mod.widgets.RunKeyBtn{}

    mod.widgets.RunPanel = set_type_default() do mod.widgets.RunPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 2
        padding: Inset{left: 6 right: 6 top: 4 bottom: 6}

        filter := TextInput{
            width: Fill
            height: Fit
            empty_text: "filter apps"
            draw_text +: {
                color: mod.look.text_hi
                color_hover: uniform(mod.look.text_hi)
                color_focus: uniform(mod.look.text_hi)
                color_down: uniform(mod.look.text_hi)
                color_empty: uniform(mod.look.dim)
                color_empty_hover: uniform(mod.look.dim)
                color_empty_focus: uniform(mod.look.dim)
                text_style: mod.look.mono{font_size: 9.0}
            }
            draw_cursor +: { color: mod.look.agent }
            draw_selection +: {
                color: mod.look.agent_select
                color_hover: mod.look.agent_select
                color_focus: mod.look.agent_select
            }
            draw_bg +: {
                color: #0000
                color_hover: #0000
                color_focus: #0000
                color_empty: #0000
                color_empty_hover: #0000
                color_empty_focus: #0000
                border_size: 0.0
            }
        }
        mod.widgets.RunRule{}
        apps := PortalList{
            width: Fill
            height: Fill
            AppRow := View{
                width: Fill
                height: Fit
                flow: Down
                cursor: MouseCursor.Hand
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
                line := View{
                    width: Fill
                    height: 20
                    flow: Right
                    spacing: 6
                    align: Align{y: 0.5}
                    padding: Inset{left: 8 right: 4}
                    app_label := Mono{
                        width: Fill
                        max_lines: 1
                        text_overflow: Ellipsis
                    }
                    app_state := Mono{
                        draw_text +: {
                            color: mod.look.meta
                            text_style: mod.look.mono{font_size: 7.5}
                        }
                    }
                }
                // The selected row's action strip.
                actions := View{
                    visible: false
                    width: Fill
                    height: 24
                    flow: Right
                    spacing: 4
                    align: Align{y: 0.5}
                    padding: Inset{left: 8 right: 4 bottom: 4}
                    build_btn := KeyBtn{ text: "build" }
                    run_btn := KeyBtn{ text: "run" }
                    jit_btn := KeyBtn{ text: "makepad run" }
                    stop_btn := KeyBtn{ text: "stop" }
                }
            }
        }
        running_head := mod.widgets.RunSection{
            visible: false
            mod.widgets.RunCaption{ text: "RUNNING" }
            mod.widgets.RunRule{}
        }
        running := PortalList{
            width: Fill
            height: 0
            RunRow := mod.widgets.RunRow{
                spacing: 4
                run_name := Mono{
                    width: Fill
                    max_lines: 1
                    text_overflow: Ellipsis
                }
                run_info := Mono{
                    draw_text +: {
                        color: mod.look.meta
                        text_style: mod.look.mono{font_size: 7.5}
                    }
                }
                logs_btn := KeyBtn{ text: "logs" padding: Inset{left: 4 right: 4} }
                restart_btn := KeyBtn{ text: "restart" padding: Inset{left: 4 right: 4} }
                stop_btn := KeyBtn{ text: "■" padding: Inset{left: 5 right: 5} }
            }
        }
        mod.widgets.RunRule{}
        // The current build in one line; a click opens the BUILD tab.
        status := View{
            width: Fill
            height: Fit
            flow: Down
            spacing: 3
            padding: Inset{left: 2 right: 2 top: 4 bottom: 2}
            cursor: MouseCursor.Hand
            show_bg: true
            draw_bg +: { color: #0000 }
            status_line := Mono{
                width: Fill
                max_lines: 1
                text_overflow: Ellipsis
                text: "no build yet"
                draw_text +: { color: mod.look.meta }
            }
            bar := mod.widgets.RunBar{}
        }
        hint := Mono{
            margin: Inset{top: 2}
            text: "↑↓  b build  r run  j makepad run  s stop"
            draw_text +: {
                color: mod.look.dim
                text_style: mod.look.mono{font_size: 7.0}
            }
        }
    }
}

/// The registry's apps with sources in the checkout.
#[derive(Default)]
struct Apps(Vec<&'static AppEntry>);

impl std::ops::Deref for Apps {
    type Target = Vec<&'static AppEntry>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RunPanel {
    #[deref]
    view: View,
    #[rust]
    started: bool,
    #[rust]
    apps: Apps,
    /// The apps the filter keeps, as indices into `apps`.
    #[rust]
    shown: Vec<usize>,
    #[rust]
    filter: String,
    /// Selected row of `shown`.
    #[rust]
    sel: usize,
    #[rust]
    seen: u64,
    #[rust]
    snap: Snapshot,
    /// The newest build already shown (a newer one, e.g. the agent's,
    /// opens the BUILD tab).
    #[rust]
    newest_shown: u64,
    /// Ticks uptimes while something runs.
    #[rust]
    tick: Timer,
}

fn manager() -> &'static RunManager {
    RunManager::global()
}

/// A file was saved (`rel` relative to the project root): running
/// `makepad run` apps whose package or path dependencies hold it learn of it
/// (their own watcher applies it).
/// A free function because the apps may have been started by the agent
/// while the RUN section was never opened; the shell calls it on
/// `FileEditorAction::Saved`.
pub fn on_saved(cx: &Cx, rel: &str) {
    let Some(ws) = Workspace::of(cx) else { return };
    let m = manager();
    // The manager's paths are relative to the checkout root, which may sit
    // above the project folder.
    let abs = ws.absolute(rel);
    if let Some(rel) = m.root().and_then(|root| abs.strip_prefix(&root).ok().map(|p| p.to_string_lossy().replace('\\', "/"))) {
        m.on_saved(&rel);
    }
}

/// The indices of `apps` whose id, label or package holds every word of
/// `filter` (any case).
fn filter_apps(apps: &[&AppEntry], filter: &str) -> Vec<usize> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    apps.iter()
        .enumerate()
        .filter(|(_, a)| {
            let hay = format!("{} {} {}", a.id, a.label, a.package).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

/// A mode in a few letters for the 260-point column.
fn mode_short(mode: Mode) -> &'static str {
    match mode {
        Mode::Native => "native",
        Mode::MakepadRun => "jit",
    }
}

impl RunPanel {
    pub fn focus(&mut self, cx: &mut Cx) {
        self.view.text_input(cx, ids!(filter)).take_key_focus(cx);
    }

    /// First use: the manager's root from the workspace, the app list, and
    /// a wake from the manager's threads.
    fn start(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        let Some(root) = Workspace::of(cx).map(|w| w.root.clone()) else { return };
        self.started = true;
        let m = manager();
        m.set_root(&root);
        m.on_change(Arc::new(|| SignalToUI::set_ui_signal()));
        self.apps = Apps(m.apps());
        self.shown = filter_apps(&self.apps, &self.filter);
        self.newest_shown = m.snapshot().jobs.last().map_or(0, |j| j.id);
    }

    fn selected(&self) -> Option<&'static AppEntry> {
        self.shown.get(self.sel).map(|i| self.apps[*i])
    }

    fn select(&mut self, cx: &mut Cx, sel: usize) {
        self.sel = sel.min(self.shown.len().saturating_sub(1));
        let list = self.view.portal_list(cx, ids!(apps));
        let first = list.first_id();
        if self.sel < first {
            list.set_first_id(self.sel);
        } else if self.sel > first + 6 {
            list.set_first_id(self.sel - 6);
        }
        self.view.redraw(cx);
    }

    /// Open the BUILD tab on `what`.
    fn show_build(&mut self, cx: &mut Cx, what: Shown) {
        // every build up to the newest has been offered now
        let newest = manager().snapshot().jobs.last().map_or(0, |j| j.id);
        self.newest_shown = self.newest_shown.max(newest);
        build_output::show(cx, what);
        cx.widget_action(self.view.widget_uid(), IdeAction::ShowBuild);
    }

    fn build(&mut self, cx: &mut Cx, app: &'static AppEntry) {
        match manager().build(app) {
            Ok(_) => self.show_build(cx, Shown::Newest),
            Err(e) => log!("code: build {}: {e}", app.id),
        }
        self.view.redraw(cx);
    }

    fn run(&mut self, cx: &mut Cx, app: &'static AppEntry, mode: Mode) {
        match manager().run(app, mode) {
            Ok(Started::Job(_)) => self.show_build(cx, Shown::Newest),
            Ok(Started::Running(id)) => self.show_build(cx, Shown::Logs(id, app.id)),
            Err(e) => log!("code: run {}: {e}", app.id),
        }
        self.view.redraw(cx);
    }

    fn stop(&mut self, cx: &mut Cx, app: &'static AppEntry) {
        let m = manager();
        let snap = m.snapshot();
        for j in snap.jobs.iter().filter(|j| j.app.id == app.id && !j.state.is_done()) {
            m.cancel(j.id);
        }
        for r in snap.running.iter().filter(|r| r.app.id == app.id && r.exited.is_none()) {
            let _ = m.stop(r.id);
        }
        self.view.redraw(cx);
    }

    fn keys(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        if ke.modifiers.any() {
            return;
        }
        match ke.key_code {
            KeyCode::ArrowUp => self.select(cx, self.sel.saturating_sub(1)),
            KeyCode::ArrowDown => self.select(cx, self.sel + 1),
            KeyCode::KeyB | KeyCode::KeyR | KeyCode::KeyJ | KeyCode::KeyS => {
                let Some(app) = self.selected() else { return };
                match ke.key_code {
                    KeyCode::KeyB => self.build(cx, app),
                    KeyCode::KeyR => self.run(cx, app, Mode::Native),
                    KeyCode::KeyJ => self.run(cx, app, Mode::MakepadRun),
                    _ => self.stop(cx, app),
                }
            }
            _ => {}
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let filter = self.view.text_input(cx, ids!(filter));
        if let Some(text) = filter.changed(actions) {
            self.filter = text;
            self.shown = filter_apps(&self.apps, &self.filter);
            self.select(cx, 0);
        }
        if let Some(ke) = filter.key_down_unhandled(actions) {
            match ke.key_code {
                KeyCode::ArrowUp => self.select(cx, self.sel.saturating_sub(1)),
                KeyCode::ArrowDown => self.select(cx, self.sel + 1),
                _ => {}
            }
        }
        if filter.returned(actions).is_some() {
            if let Some(app) = self.selected() {
                self.run(cx, app, Mode::Native);
            }
        }
        if filter.escaped(actions) {
            cx.set_key_focus(self.view.area());
        }

        let apps = self.view.portal_list(cx, ids!(apps));
        for (index, item) in apps.items_with_actions(actions) {
            let Some(app) = self.shown.get(index).map(|i| self.apps[*i]) else { continue };
            if item.button(cx, ids!(build_btn)).clicked(actions) {
                self.build(cx, app);
            } else if item.button(cx, ids!(run_btn)).clicked(actions) {
                self.run(cx, app, Mode::Native);
            } else if item.button(cx, ids!(jit_btn)).clicked(actions) {
                self.run(cx, app, Mode::MakepadRun);
            } else if item.button(cx, ids!(stop_btn)).clicked(actions) {
                self.stop(cx, app);
            } else if row_clicked(&item, actions) {
                self.sel = index;
                cx.set_key_focus(self.view.area());
                self.view.redraw(cx);
            }
        }

        if self.view.view(cx, ids!(status)).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
            self.show_build(cx, Shown::Newest);
        }

        let running = self.view.portal_list(cx, ids!(running));
        for (index, item) in running.items_with_actions(actions) {
            // After the apps: the agent's commands (stop only).
            if let Some(sh) = index.checked_sub(self.snap.running.len()).and_then(|i| self.snap.shells.get(i)).cloned() {
                if item.button(cx, ids!(stop_btn)).clicked(actions) {
                    if sh.exit.is_some() {
                        manager().dismiss_shell(sh.id);
                    } else {
                        let _ = manager().stop_shell(sh.id, "from the RUN panel");
                    }
                }
                self.view.redraw(cx);
                continue;
            }
            let Some(r) = self.snap.running.get(index).cloned() else { continue };
            let m = manager();
            if item.button(cx, ids!(stop_btn)).clicked(actions) {
                let _ = m.stop(r.id);
            } else if item.button(cx, ids!(restart_btn)).clicked(actions) {
                let _ = m.restart(r.id);
            } else if item.button(cx, ids!(logs_btn)).clicked(actions) {
                self.show_build(cx, Shown::Logs(r.id, r.app.id));
            }
            self.view.redraw(cx);
        }
    }

    /// Labels, the bar and the running list's size from the manager's state.
    fn refresh(&mut self, cx: &mut Cx) {
        let m = manager();
        self.seen = m.generation();
        self.snap = m.snapshot();
        let job = self.snap.jobs.last().cloned();
        let (line, ink, fill, bar) = match &job {
            Some(j) => (format!("{}  {}", j.app.id, j.status_line()), state_ink(&j.state), bar_fill(j), bar_ink(&j.state)),
            None => ("no build yet".to_string(), vec(palette().meta, 1.0), 0.0, vec(palette().primary, 1.0)),
        };
        let mut status = self.view.label(cx, ids!(status_line));
        status.set_text(cx, &line);
        script_apply_eval!(cx, status, { draw_text +: { color: #(ink) } });
        let mut bar_view = self.view.view(cx, ids!(bar));
        script_apply_eval!(cx, bar_view, { draw_bg +: { fill: #(fill) color: #(bar) } });

        // The running list takes the rows it needs, up to five: the apps,
        // then the agent's shell commands.
        let n = self.snap.running.len() + self.snap.shells.len();
        self.view.view(cx, ids!(running_head)).set_visible(cx, n > 0);
        let h = (n.min(5) * 20) as f64;
        let mut running = self.view.portal_list(cx, ids!(running));
        script_apply_eval!(cx, running, { height: #(h) });
        let live = self.snap.running.iter().any(|r| r.exited.is_none()) || self.snap.shells.iter().any(|s| s.exit.is_none());
        if live && self.tick.is_empty() {
            self.tick = cx.start_interval(1.0);
        } else if !live && !self.tick.is_empty() {
            cx.stop_timer(self.tick);
            self.tick = Timer::empty();
        }
    }

    fn draw_apps(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.shown.len());
        let m = manager();
        while let Some(index) = list.next_visible_item(cx) {
            let Some(app) = self.shown.get(index).map(|i| self.apps[*i]) else { continue };
            let mut row = list.item(cx, index, id!(AppRow));
            let selected = index == self.sel;
            let tint = if selected { vec(palette().primary, 0.12) } else { vec(0, 0.0) };
            script_apply_eval!(cx, row, { draw_bg +: { color: #(tint) } });
            row.view(cx, ids!(actions)).set_visible(cx, selected);
            row.label(cx, ids!(app_label)).set_text(cx, app.label);
            let running = self.snap.running.iter().any(|r| r.app.id == app.id && r.exited.is_none());
            let building = self.snap.jobs.iter().any(|j| j.app.id == app.id && !j.state.is_done());
            let (state, ink) = if building {
                ("building", vec(palette().info, 1.0))
            } else if running {
                ("● running", vec(palette().ok, 1.0))
            } else if m.is_built(app) {
                ("built", vec(palette().meta, 1.0))
            } else {
                ("", vec(palette().meta, 1.0))
            };
            let mut state_label = row.label(cx, ids!(app_state));
            state_label.set_text(cx, state);
            script_apply_eval!(cx, state_label, { draw_text +: { color: #(ink) } });
            row.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_running(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let apps = self.snap.running.len();
        list.set_item_range(cx, 0, apps + self.snap.shells.len());
        let shown = build_output::shown(cx);
        while let Some(index) = list.next_visible_item(cx) {
            if let Some(sh) = index.checked_sub(apps).and_then(|i| self.snap.shells.get(i)).cloned() {
                let mut row = list.item(cx, index, id!(RunRow));
                script_apply_eval!(cx, row, { draw_bg +: { color: #(vec(0, 0.0)) } });
                let (name_ink, info) = match &sh.exit {
                    None => (vec(palette().warning, 1.0), format!("run {} {}", sh.id, uptime(sh.elapsed))),
                    Some(exit) => (vec(palette().meta, 1.0), format!("run {} {}", sh.id, exit.text())),
                };
                let mut name = row.label(cx, ids!(run_name));
                name.set_text(cx, &format!("$ {}", makepad_code_agent::runs::cut_command(&sh.command, 60)));
                script_apply_eval!(cx, name, { draw_text +: { color: #(name_ink) } });
                row.label(cx, ids!(run_info)).set_text(cx, &info);
                row.button(cx, ids!(logs_btn)).set_visible(cx, false);
                row.button(cx, ids!(restart_btn)).set_visible(cx, false);
                row.button(cx, ids!(stop_btn)).set_text(cx, if sh.exit.is_some() { "×" } else { "■" });
                row.draw_all(cx, &mut Scope::empty());
                continue;
            }
            let Some(r) = self.snap.running.get(index).cloned() else { continue };
            let mut row = list.item(cx, index, id!(RunRow));
            row.button(cx, ids!(logs_btn)).set_visible(cx, true);
            row.button(cx, ids!(restart_btn)).set_visible(cx, true);
            let on_screen = matches!(shown, Shown::Logs(id, _) if id == r.id);
            let tint = if on_screen { vec(palette().primary, 0.12) } else { vec(0, 0.0) };
            script_apply_eval!(cx, row, { draw_bg +: { color: #(tint) } });
            let (name_ink, info) = match (&r.exited, &r.building) {
                (None, Some(stage)) => (vec(palette().meta, 1.0), format!("{} {stage}", mode_short(r.mode))),
                (None, None) => (vec(palette().ok, 1.0), format!("{} {}", mode_short(r.mode), uptime(r.uptime))),
                (Some(how), _) => (vec(palette().meta, 1.0), format!("{} {how}", mode_short(r.mode))),
            };
            let mut name = row.label(cx, ids!(run_name));
            name.set_text(cx, r.app.label);
            script_apply_eval!(cx, name, { draw_text +: { color: #(name_ink) } });
            row.label(cx, ids!(run_info)).set_text(cx, &info);
            // an exited run's stop clears it from the list
            row.button(cx, ids!(stop_btn)).set_text(cx, if r.exited.is_some() { "×" } else { "■" });
            row.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for RunPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::Signal if self.started => {
                let m = manager();
                if m.generation() != self.seen {
                    self.view.redraw(cx);
                }
                // A build started elsewhere (the agent) opens the BUILD tab.
                if let Some(newest) = m.snapshot().jobs.last().map(|j| j.id) {
                    if newest > self.newest_shown {
                        self.show_build(cx, Shown::Newest);
                    }
                }
            }
            Event::KeyDown(ke) if cx.has_key_focus(self.view.area()) => self.keys(cx, ke),
            _ => {}
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
        self.start(cx);
        self.refresh(cx);
        let apps = self.view.portal_list(cx, ids!(apps)).widget_uid();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let uid = item.widget_uid();
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                if uid == apps {
                    self.draw_apps(cx, &mut list);
                } else {
                    self.draw_running(cx, &mut list);
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
    fn the_filter_keeps_apps_holding_every_word() {
        let apps: Vec<&AppEntry> = makepad_app_registry::apps().iter().collect();
        let calc = filter_apps(&apps, "calc");
        assert!(calc.iter().any(|i| apps[*i].id == "calculator"));
        assert!(calc.iter().all(|i| format!("{} {} {}", apps[*i].id, apps[*i].label, apps[*i].package).to_lowercase().contains("calc")));
        assert_eq!(filter_apps(&apps, "").len(), apps.len());
        assert!(filter_apps(&apps, "app calculator").iter().any(|i| apps[*i].id == "calculator"));
        assert!(filter_apps(&apps, "zzzz-nothing").is_empty());
    }
}
