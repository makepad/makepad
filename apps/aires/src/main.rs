pub use makepad_widgets;

mod glyph;
mod login;
mod source;
mod view;

use makepad_shell_kit::tray_app::{DropdownChange, TrayDropdown};
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use view::{Clearing, Inbox, InboxAction, Model};

app_main!(App);

const TRAY: LiveId = live_id!(aires);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                create_on_start: false
                window.title: "aires"
                window.inner_size: vec2(420, 720)
                body +: {inbox := mod.widgets.Inbox{}}
            }
            dropdown_window := Window{
                create_on_start: false
                show_caption_bar: false
                chrome: WindowChrome.None
                window.title: "aires"
                window.dropdown: true
                window.inner_size: vec2(420, 720)
                body +: {drop_inbox := mod.widgets.Inbox{}}
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    source: Option<source::Source>,
    #[rust]
    model: Rc<RefCell<Model>>,
    #[rust(TrayDropdown::new("aires", TRAY, 420.0))]
    dropdown: TrayDropdown,
    /// What the tray shows now: the open count, whether one is new, and the login switch
    #[rust]
    tray_key: Option<(usize, bool, Option<bool>)>,
    /// The questions waiting for the person that have their OS notification (one each)
    #[rust]
    notified: HashSet<String>,
}

impl App {
    fn refresh_tray(&mut self, cx: &mut Cx) {
        let (count, unseen) = {
            let model = self.model.borrow();
            (model.waiting(), model.unseen.len())
        };
        // A question the person has not looked at yet marks the icon until the inbox opens.
        let new = unseen > 0 && !self.dropdown.is_open();
        let login = login::state();
        if self.tray_key == Some((count, new, login)) { return; }
        self.tray_key = Some((count, new, login));
        let mut menu = vec![TrayMenuItem::item(live_id!(open), "Open")];
        if let Some(on) = login { menu.push(TrayMenuItem::check(live_id!(login), "Open at login", on)); }
        menu.push(TrayMenuItem::Line);
        menu.push(TrayMenuItem::item(live_id!(quit), "Quit"));
        cx.set_tray(TrayIcon {
            id: TRAY,
            dropdown: true,
            image: glyph::image(count, new),
            tooltip: match (count, new) {
                (0, _) => "aires: no questions".into(),
                (_, true) => format!("aires: {count} questions wait for you, {unseen} new"),
                _ => format!("aires: {count} questions wait for you"),
            },
            menu,
        });
    }

    /// One OS notification per question that starts waiting for the person, removed when it stops
    /// (answered, closed). A notification has the question's id, so the OS replaces one an
    /// earlier run of the app posted rather than stacking a second.
    fn notify(&mut self, cx: &mut Cx) {
        let model = self.model.borrow();
        let open: HashSet<&str> = model.questions.iter().filter(|q| q.state == makepad_devhub::ask::State::Open).map(|q| q.id.as_str()).collect();
        for q in model.questions.iter().filter(|q| open.contains(q.id.as_str())) {
            if self.notified.insert(q.id.clone()) {
                let about = [&q.why, &q.blocked, &q.decided].into_iter().find(|s| !s.is_empty()).map_or(String::new(), |s| format!(": {s}"));
                cx.post_notification(Notification { id: LiveId::from_str(&q.id), title: q.title.clone(), body: format!("from {}{about}", q.from) });
            }
        }
        self.notified.retain(|id| {
            let keep = open.contains(id.as_str());
            if !keep { cx.remove_notification(LiveId::from_str(id)); }
            keep
        });
    }

    fn content_height(&self) -> f64 {
        if self.model.borrow().questions.is_empty() { 120.0 } else { 0.0 }
    }

    fn open_window(&mut self, cx: &mut Cx) {
        let window = self.ui.window(cx, ids!(dropdown_window));
        self.dropdown.close(cx, &window, "open window");
        cx.show_in_dock(true);
        self.ui.window(cx, ids!(main_window)).show(cx);
        self.ui.redraw(cx);
    }

    fn open(&mut self, cx: &mut Cx) {
        if cx.has_tray_dropdown() {
            let window = self.ui.window(cx, ids!(dropdown_window));
            self.dropdown.open(cx, &window, self.content_height());
            self.refresh_tray(cx);
            self.ui.redraw(cx);
        } else {
            self.open_window(cx);
        }
    }

    /// The inbox closed: what it showed is no longer new.
    fn seen(&mut self, cx: &mut Cx) {
        self.model.borrow_mut().mark_seen();
        self.refresh_tray(cx);
        self.ui.redraw(cx);
    }

    fn quit(&mut self, cx: &mut Cx) {
        login::pid().clear();
        cx.remove_tray(TRAY);
        cx.quit();
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        login::pid().write();
        {
            let mut model = self.model.borrow_mut();
            model.status = Some("Connecting to master...".into());
            match source::Source::start(&cx.thread_spawner()) {
                Ok(source) => self.source = Some(source),
                Err(error) => model.status = Some(error),
            }
            model.rebuild();
        }
        for path in [ids!(inbox), ids!(drop_inbox)] {
            if let Some(mut inbox) = self.ui.widget(cx, path).borrow_mut::<Inbox>() {
                inbox.model = self.model.clone();
            }
        }
        self.refresh_tray(cx);
        if cx.desktop_has_tray_dropdown() && !source::demo_mode() { cx.show_in_dock(false); } else { self.open_window(cx); }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut changed = false;
        for action in actions {
            let Some(action) = action.as_widget_action() else { continue };
            let answer = match action.cast() {
                InboxAction::Answer(answer) => answer,
                InboxAction::Clear => {
                    let mut model = self.model.borrow_mut();
                    if let Some(by) = cx.injected_by() {
                        log!("aires: clear: not sent: injected input ({by})");
                        model.clear.clearing = Clearing::Idle;
                        model.clear.note = Some(format!("Not cleared: injected input ({by})"));
                        changed = true;
                    } else if model.clear.clearing == Clearing::Confirm {
                        match self.source.as_ref().ok_or_else(|| "answer worker unavailable".to_string()).and_then(|source| source.clear()) {
                            Ok(()) => model.clear.clearing = Clearing::Pending,
                            Err(error) => {
                                log!("aires: clear: {error}");
                                model.clear.clearing = Clearing::Idle;
                                model.clear.note = Some(format!("Not cleared: {error}"));
                            }
                        }
                        changed = true;
                    }
                    continue;
                }
                InboxAction::None => continue,
            };
            let mut model = self.model.borrow_mut();
            if let Some(by) = cx.injected_by() {
                let error = format!("not sent: injected input ({by})");
                log!("aires: {}: {error}", answer.id);
                model.errors.insert(answer.id, error);
            } else if !model.pending.contains_key(&answer.id) {
                model.errors.remove(&answer.id);
                match self.source.as_ref().ok_or_else(|| "answer worker unavailable".to_string()).and_then(|source| source.answer(answer.clone())) {
                    Ok(()) => { model.pending.insert(answer.id.clone(), answer); }
                    Err(error) => {
                        log!("aires: {}: {error}", answer.id);
                        model.errors.insert(answer.id, error);
                    }
                }
            }
            changed = true;
        }
        if changed { self.model.borrow_mut().rebuild(); self.ui.redraw(cx); }
    }
}

impl AppMain for App {
    fn outlives_window_manager() -> bool { true }

    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // the system's appearance, light or dark, as the window system says at start
        let dark = makepad_widgets::makepad_platform::host_dark().unwrap_or(true);
        makepad_widgets::set_base_theme(vm.cx_mut(), if dark { BaseTheme::Dark } else { BaseTheme::Light });
        makepad_widgets::script_mod(vm);
        makepad_shell_kit::eval_default_theme(vm);
        makepad_shell_kit::script_mod(vm);
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Signal => {
                if let Some(source) = &self.source {
                    let (snapshot, results, cleared) = source.take();
                    if snapshot.is_some() || !results.is_empty() || !cleared.is_empty() {
                        let mut model = self.model.borrow_mut();
                        model.update(snapshot, results);
                        for result in cleared { model.cleared(result); }
                        drop(model);
                        self.notify(cx);
                        self.refresh_tray(cx);
                        let window = self.ui.window(cx, ids!(dropdown_window));
                        self.dropdown.fit(cx, &window, self.content_height());
                        self.ui.redraw(cx);
                    }
                }
            }
            Event::TrayMenu(e) if e.tray == TRAY => {
                match e.item {
                    id if id == live_id!(open) => self.open(cx),
                    id if id == live_id!(login) => {
                        if let Some(on) = login::state() {
                            if let Err(error) = login::set(!on) { log!("aires: open at login: {error}"); }
                        }
                    }
                    id if id == live_id!(quit) => return self.quit(cx),
                    _ => {}
                }
                self.refresh_tray(cx);
            }
            Event::NotificationClick(_) => self.open(cx),
            Event::TrayClick(e) if e.tray == TRAY => {
                self.refresh_tray(cx);
                if !cx.has_tray_dropdown() { self.open_window(cx); }
            }
            Event::WindowCloseRequested(e) => {
                let dropdown = self.ui.window(cx, ids!(dropdown_window));
                if dropdown.window_id() == Some(e.window_id) {
                    e.accept_close.set(false);
                    self.dropdown.close(cx, &dropdown, "close requested");
                    self.seen(cx);
                } else if cx.has_tray() {
                    e.accept_close.set(false);
                    self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
                    cx.show_in_dock(false);
                    self.seen(cx);
                } else { return self.quit(cx); }
            }
            Event::Shutdown => login::pid().clear(),
            _ => {}
        }
        let window = self.ui.window(cx, ids!(dropdown_window));
        match self.dropdown.handle_event(cx, event, &window, self.content_height()) {
            Some(DropdownChange::Opened) => self.refresh_tray(cx),
            Some(DropdownChange::Closed) => self.seen(cx),
            None => {}
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
