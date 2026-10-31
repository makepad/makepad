//! The chats in the left sidebar: a "+ new chat" row, then the saved
//! sessions (`makepad_code_agent::session_store` in the config's session folder), newest
//! first, each with its title and when / turns. A pick emits
//! `IdeAction::OpenChat{session}` (the "+ new chat" row `session: None`);
//! sessions open in a centre tab carry the agent's bar and wash.
//!
//! Contract: `refresh(cx)` re-reads the store (the shell calls it on
//! `IdeAction::SessionsChanged`); `set_open(cx, ids)` marks the sessions
//! open in tabs.
//!
//! Keys while the list has the keyboard: Up/Down select, Enter opens,
//! Delete (or Cmd+Backspace) deletes after a confirm. The filter field
//! narrows the list by a fuzzy match on titles; Up/Down/Enter work from it
//! too. A right-click on a session offers Open and Delete session.

use super::{IdeAction, Workspace};
use makepad_code_agent::event::SessionInfo;
use makepad_code_agent::session_store;
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.SessionsPanelBase = #(SessionsPanel::register_widget(vm))

    let Mono = Label{
        width: Fit
        height: Fit
        padding: 0
        text: ""
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    let Rule = View{
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

    let Field = TextInput{
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

    let KeyBtn = ButtonFlat{
        height: 20
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        margin: 0
        draw_text +: {
            color: mod.look.text
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono{font_size: 8.5}
        }
        draw_bg +: {
            ink: uniform(mod.look.meta)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 4.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let wash = 0.05 + 0.12 * self.hover + 0.22 * self.down
                let a = clamp(fill * wash + edge * (0.55 + 0.45 * max(self.hover, self.focus)), 0.0, 1.0)
                return vec4(self.ink.rgb * a, a)
            }
        }
    }

    /// A row's wash; a row whose `bar` is on carries the agent's 2-point bar
    /// on its left (a session open in a tab).
    let RowHead = View{
        width: Fill
        flow: Down
        align: Align{y: 0.5}
        padding: Inset{left: 10 right: 6}
        cursor: MouseCursor.Hand
        grab_key_focus: false
        show_bg: true
        draw_bg +: {
            color: #0000
            bar: uniform(0.0)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let bar = (1.0 - step(2.0, p.x)) * self.bar
                let c = mix(self.color, mod.look.agent, bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    let MenuRow = RowHead{
        height: 22
        flow: Right
        menu_label := Mono{
            width: Fill
            draw_text +: { color: mod.look.text_hi }
        }
        menu_key := Mono{
            draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} }
        }
    }

    mod.widgets.SessionsPanel = set_type_default() do mod.widgets.SessionsPanelBase{
        width: Fill
        height: Fill
        flow: Down
        // The panel's own area holds the keyboard while the list has it.
        show_bg: true
        draw_bg +: { color: #0000 }

        View{
            width: Fill
            height: 28
            flow: Right
            align: Align{y: 0.5}
            padding: Inset{left: 8 right: 6}
            Mono{
                width: 12
                text: "▸"
                draw_text +: { color: mod.look.agent text_style: mod.look.mono_bold{font_size: 9.0} }
            }
            filter := Field{
                width: Fill
                height: Fit
                empty_text: "filter chats  ⌘T new"
            }
        }
        Rule{}

        // Delete asks first.
        confirm_bar := View{
            visible: false
            width: Fill
            height: Fit
            flow: Down
            spacing: 4
            padding: Inset{left: 8 right: 6 top: 6 bottom: 6}
            show_bg: true
            draw_bg +: { color: mod.look.human_wash }
            confirm_text := Mono{
                width: Fill
                draw_text +: { color: mod.look.text_hi }
            }
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 6
                confirm_yes := KeyBtn{ text: "Delete  enter" draw_bg +: { ink: mod.look.bad } }
                confirm_no := KeyBtn{ text: "Cancel  esc" }
            }
        }

        empty := Mono{
            visible: false
            width: Fill
            padding: Inset{left: 12 top: 6 bottom: 4}
            draw_text +: { color: mod.look.dim }
        }

        list := PortalList{
            width: Fill
            height: Fill
            NewRow := View{
                width: Fill
                height: Fit
                padding: Inset{top: 4 bottom: 4}
                row_head := RowHead{
                    height: 24
                    flow: Right
                    new_label := Mono{
                        width: Fill
                        text: "+ new chat"
                        draw_text +: { color: mod.look.agent }
                    }
                    Mono{
                        text: "⌘T"
                        draw_text +: { color: mod.look.dim text_style: mod.look.mono{font_size: 8.0} }
                    }
                }
            }
            SessionRow := View{
                width: Fill
                height: Fit
                row_head := RowHead{
                    height: 36
                    session_title := Mono{
                        max_lines: 1
                        text_overflow: Ellipsis
                        width: Fill
                        draw_text +: {
                            color: mod.look.text
                            text_style: mod.look.mono{font_size: 8.5}
                        }
                    }
                    session_when := Mono{
                        max_lines: 1
                        text_overflow: Ellipsis
                        width: Fill
                        draw_text +: {
                            color: mod.look.meta
                            text_style: mod.look.mono{font_size: 7.5}
                        }
                    }
                }
            }
        }

        // Right-click menu: rows at the pointer over a clear scrim.
        menu := Modal{
            align: Align{x: 0.0 y: 0.0}
            bg_view +: { draw_bg.color: #0000 }
            content +: {
                menu_box := View{
                    width: 200
                    height: Fit
                    flow: Down
                    padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
                    show_bg: true
                    draw_bg +: {
                        color: mod.look.rail
                        pixel: fn() {
                            let p = self.pos * self.rect_size
                            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                            let c0 = self.rect_size * 0.5
                            let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 6.0)
                            let inside = Finish.cover(d, px)
                            let edge = Finish.band(d, 0.0, px, px)
                            var c = vec4(self.color.rgb, 1.0) * inside
                            c = mix(c, vec4(mod.look.agent.rgb, 1.0), edge * 0.6)
                            return vec4(c.rgb * c.a, c.a)
                        }
                    }
                    m_open := MenuRow{ menu_label.text: "Open" menu_key.text: "enter" }
                    m_delete := MenuRow{ menu_label.text: "Delete session" menu_key.text: "del" }
                }
            }
        }
    }
}

/// The list's rows: "+ new chat", then the shown sessions.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    New,
    Session(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct SessionsPanel {
    #[deref]
    view: View,
    #[rust]
    dir: Option<PathBuf>,
    #[rust]
    loaded: bool,
    /// Every saved session, newest first.
    #[rust]
    all: Vec<SessionInfo>,
    /// Indexes into `all` the filter keeps, in order.
    #[rust]
    shown: Vec<usize>,
    #[rust]
    query: String,
    /// The selected row (0 is "+ new chat").
    #[rust]
    sel: usize,
    /// Sessions open in centre tabs.
    #[rust]
    open: Vec<String>,
    /// The session a delete is waiting on.
    #[rust]
    confirm: Option<String>,
    /// The session the right-click menu is about.
    #[rust]
    menu_target: Option<String>,
    #[rust]
    want_list_focus: bool,
    /// What a pick asks of the shell, sent after this panel's own actions
    /// were taken (they are captured here).
    #[rust]
    outbox: Vec<IdeAction>,
}

impl SessionsPanel {
    /// Read the store again (after a chat saved).
    pub fn refresh(&mut self, cx: &mut Cx) {
        if self.dir.is_none() {
            self.dir = Workspace::of(cx).and_then(|w| w.config.session_dir.clone());
        }
        self.loaded = true;
        let keep = self.selected_id();
        self.all = match &self.dir {
            Some(dir) => session_store::list(dir),
            None => Vec::new(),
        };
        self.rank();
        // The selection stays on its session when it is still there.
        if let Some(id) = keep {
            if let Some(i) = self.shown.iter().position(|&i| self.all[i].id == id) {
                self.sel = i + 1;
            }
        }
        self.sel = self.sel.min(self.shown.len());
        self.view.redraw(cx);
    }

    /// Mark the sessions open in centre tabs.
    pub fn set_open(&mut self, cx: &mut Cx, open: &[String]) {
        if self.open != open {
            self.open = open.to_vec();
            self.view.redraw(cx);
        }
    }

    fn ensure_loaded(&mut self, cx: &mut Cx) {
        if !self.loaded && Workspace::of(cx).is_some() {
            self.refresh(cx);
        }
    }

    fn rank(&mut self) {
        let q = self.query.trim();
        let mut hits: Vec<(i32, usize)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, s)| crate::ui::palette::score(q, &s.title).map(|sc| (sc, i)))
            .collect();
        if !q.is_empty() {
            hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.shown = hits.into_iter().map(|(_, i)| i).collect();
    }

    fn rows(&self) -> usize {
        1 + self.shown.len()
    }

    fn row(&self, index: usize) -> Option<Row> {
        match index {
            0 => Some(Row::New),
            i => self.shown.get(i - 1).map(|&s| Row::Session(s)),
        }
    }

    fn selected_id(&self) -> Option<String> {
        match self.row(self.sel)? {
            Row::Session(i) => Some(self.all[i].id.clone()),
            Row::New => None,
        }
    }

    fn emit(&mut self, _cx: &mut Cx, action: IdeAction) {
        self.outbox.push(action);
    }

    fn open_row(&mut self, cx: &mut Cx, index: usize) {
        match self.row(index) {
            Some(Row::New) => self.emit(cx, IdeAction::OpenChat { session: None }),
            Some(Row::Session(i)) => {
                let id = self.all[i].id.clone();
                self.emit(cx, IdeAction::OpenChat { session: Some(id) });
            }
            None => {}
        }
    }

    fn select(&mut self, cx: &mut Cx, sel: usize) {
        self.sel = sel.min(self.rows().saturating_sub(1));
        self.view.portal_list(cx, ids!(list)).keep_index_visible(cx, self.sel);
        self.view.redraw(cx);
    }

    fn ask_delete(&mut self, cx: &mut Cx, id: String) {
        let title = self.all.iter().find(|s| s.id == id).map(|s| s.title.clone()).unwrap_or_default();
        self.view.label(cx, ids!(confirm_text)).set_text(cx, &format!("Delete the chat \"{}\"?", clip(&title, 40)));
        self.confirm = Some(id);
        self.want_list_focus = true;
        self.view.redraw(cx);
    }

    fn commit_delete(&mut self, cx: &mut Cx) {
        let Some(id) = self.confirm.take() else { return };
        if let Some(dir) = &self.dir {
            if let Err(e) = session_store::delete(dir, &id) {
                log!("code: cannot delete session {id}: {e}");
            }
        }
        self.refresh(cx);
    }

    fn cancel_delete(&mut self, cx: &mut Cx) {
        if self.confirm.take().is_some() {
            self.view.redraw(cx);
        }
    }

    fn list_has_focus(&self, cx: &Cx) -> bool {
        let area = self.view.area();
        !area.is_empty() && cx.has_key_focus(area)
    }

    /// Up/Down/Enter (from the list or the filter), Delete, Esc.
    fn key(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        if self.confirm.is_some() {
            match ke.key_code {
                KeyCode::ReturnKey => self.commit_delete(cx),
                KeyCode::Escape => self.cancel_delete(cx),
                _ => return false,
            }
            return true;
        }
        match ke.key_code {
            KeyCode::ArrowDown => self.select(cx, self.sel + 1),
            KeyCode::ArrowUp => self.select(cx, self.sel.saturating_sub(1)),
            KeyCode::ReturnKey => self.open_row(cx, self.sel),
            KeyCode::Delete | KeyCode::Backspace if ke.key_code == KeyCode::Delete || ke.modifiers.is_primary() => {
                match self.selected_id() {
                    Some(id) => self.ask_delete(cx, id),
                    None => return false,
                }
            }
            _ => return false,
        }
        true
    }

    fn open_menu(&mut self, cx: &mut Cx, id: String, at: DVec2) {
        self.menu_target = Some(id);
        let mut boxed = self.view.view(cx, ids!(menu_box));
        script_apply_eval!(cx, boxed, { margin: mod.turtle.Inset{left: #(at.x) top: #(at.y) right: 0.0 bottom: 0.0} });
        self.view.modal(cx, ids!(menu)).open(cx);
    }

    fn menu_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let modal = self.view.modal(cx, ids!(menu));
        if modal.dismissed(actions) {
            self.menu_target = None;
            return;
        }
        let up = |cx: &mut Cx, this: &Self, path: &[LiveId; 1]| {
            this.view.view(cx, path).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit())
        };
        if up(cx, self, ids!(m_open)) {
            modal.close(cx);
            if let Some(id) = self.menu_target.take() {
                self.emit(cx, IdeAction::OpenChat { session: Some(id) });
            }
        } else if up(cx, self, ids!(m_delete)) {
            modal.close(cx);
            if let Some(id) = self.menu_target.take() {
                self.ask_delete(cx, id);
            }
        }
    }

    fn row_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let head = item.view(cx, ids!(row_head));
            if let Some(fd) = head.finger_down(actions) {
                if fd.device.mouse_button().is_some_and(|b| b.is_secondary()) {
                    if let Some(Row::Session(i)) = self.row(index) {
                        self.sel = index;
                        let id = self.all[i].id.clone();
                        self.open_menu(cx, id, fd.abs);
                    }
                    continue;
                }
                if fd.device.is_primary_hit() {
                    self.sel = index;
                    cx.set_key_focus(self.view.area());
                    self.view.redraw(cx);
                }
            }
            if head.finger_up(actions).is_some_and(|f| !f.cancelled && f.is_over && f.device.is_primary_hit()) {
                self.open_row(cx, index);
            }
        }
    }

    fn filter_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let input = self.view.text_input(cx, ids!(filter));
        if let Some(text) = input.changed(actions) {
            self.query = text;
            self.rank();
            // A query picks its best match; an empty one the "+ new chat" row.
            self.sel = if self.query.trim().is_empty() || self.shown.is_empty() { 0 } else { 1 };
            self.view.redraw(cx);
        }
        if let Some(key) = input.key_down_unhandled(actions) {
            self.key(cx, &key);
        }
        if input.returned(actions).is_some() {
            self.open_row(cx, self.sel);
        }
        if input.escaped(actions) && !self.query.is_empty() {
            input.set_text(cx, "");
            self.query.clear();
            self.rank();
            self.view.redraw(cx);
        }
    }

    fn draw_list(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.rows());
        let agent = crate::ui::theme::vec(crate::ui::theme::palette().primary, 1.0);
        while let Some(index) = list.next_visible_item(cx) {
            let Some(row) = self.row(index) else { continue };
            let selected = index == self.sel;
            let (item, open) = match row {
                Row::New => (list.item(cx, index, id!(NewRow)), false),
                Row::Session(i) => {
                    let info = &self.all[i];
                    let item = list.item(cx, index, id!(SessionRow));
                    item.label(cx, ids!(session_title)).set_text(cx, &info.title);
                    let turns = if info.turns == 1 { "1 turn".to_string() } else { format!("{} turns", info.turns) };
                    item.label(cx, ids!(session_when)).set_text(cx, &format!("{}  ·  {turns}", info.when));
                    (item, self.open.iter().any(|o| *o == info.id))
                }
            };
            let alpha = match (selected, open) {
                (true, _) => 0.22,
                (false, true) => 0.12,
                _ => 0.0,
            };
            let tint = Vec4f { x: agent.x, y: agent.y, z: agent.z, w: alpha };
            let bar = if open { 1.0 } else { 0.0 };
            let mut head = item.view(cx, ids!(row_head));
            script_apply_eval!(cx, head, { draw_bg +: { color: #(tint) bar: #(bar) } });
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

impl Widget for SessionsPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_loaded(cx);
        if let Event::KeyDown(ke) = event {
            if self.list_has_focus(cx) {
                self.key(cx, ke);
            }
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !actions.is_empty() {
            self.filter_actions(cx, &actions);
            self.menu_actions(cx, &actions);
            self.row_actions(cx, &actions);
            if self.view.button(cx, ids!(confirm_yes)).clicked(&actions) {
                self.commit_delete(cx);
            }
            if self.view.button(cx, ids!(confirm_no)).clicked(&actions) {
                self.cancel_delete(cx);
            }
        }
        // The shell routes what the rows asked for.
        let uid = self.widget_uid();
        for action in std::mem::take(&mut self.outbox) {
            cx.widget_action(uid, action);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_loaded(cx);
        self.view.view(cx, ids!(confirm_bar)).set_visible(cx, self.confirm.is_some());
        let empty = self.shown.is_empty() && !self.query.trim().is_empty();
        let note = self.view.label(cx, ids!(empty));
        note.set_visible(cx, empty);
        note.set_text(cx, "no chat matches");
        let list_uid = self.view.portal_list(cx, ids!(list)).widget_uid();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if item.widget_uid() == list_uid {
                if let Some(mut list) = item.borrow_mut::<PortalList>() {
                    self.draw_list(cx, &mut list);
                }
            }
        }
        if std::mem::take(&mut self.want_list_focus) {
            cx.set_key_focus(self.view.area());
        }
        DrawStep::done()
    }
}
