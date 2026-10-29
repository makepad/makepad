//! What opens over the code: completions at the cursor, the docs of the
//! name under the resting mouse, and the instrument picker (every
//! instrument grouped, typing filters, the arrows and Return choose, the
//! one under the selection auditioned by the desk; a menu: a press outside
//! closes it, a row is chosen on release over it).

use super::{ScoreEditor, ScoreEditorAction};
use makepad_code_editor::code_view::CodeView;
use makepad_look::palette::Palette;
use makepad_score_live::instruments::Choice;
use makepad_widgets::badge::advance;
use makepad_widgets::*;

/// Completions at the cursor, the one selected, where they show.
pub type Completing = (makepad_score_live::assist::Completions, usize, DVec2);

fn rgba(c: u32) -> Vec4f {
    Vec4f::from_u32(c)
}

fn alpha(c: u32, a: f32) -> Vec4f {
    let mut v = Vec4f::from_u32(c);
    v.w = a;
    v
}

/// The instrument picker, open on one node.
pub struct Picker {
    pub node: String,
    at: DVec2,
    filter: String,
    pub sel: usize,
    choices: Vec<Choice>,
    current: Option<String>,
    /// Rows drawn: (rect, index into the filtered list).
    rows: Vec<(Rect, usize)>,
    panel: Rect,
    /// The list's scroll in pixels, and its visible rect.
    top: f64,
    list: Rect,
    /// Keep the selection in view on the next draw (keyboard moves).
    reveal: bool,
    mouse: Option<DVec2>,
    /// The press that opened it (its release there does not choose).
    opened_at: Option<DVec2>,
    /// A press inside: a release on the same row chooses it.
    pressed: Option<usize>,
}

impl Picker {
    pub fn filtered(&self) -> Vec<&Choice> {
        let f = self.filter.to_ascii_lowercase();
        self.choices.iter().filter(|c| f.is_empty() || c.label.to_ascii_lowercase().contains(&f) || c.group.to_ascii_lowercase().contains(&f)).collect()
    }

    fn row_at(&self, abs: DVec2) -> Option<usize> {
        self.rows.iter().find(|(r, _)| r.contains(abs) && self.list.contains(abs)).map(|(_, i)| *i)
    }
}

/// A panel that stays inside the view.
fn place_panel(at: DVec2, size: DVec2, clip: Rect) -> Rect {
    let x = at.x.min(clip.pos.x + clip.size.x - size.x - 4.0).max(clip.pos.x + 4.0);
    let below = at.y + size.y <= clip.pos.y + clip.size.y;
    let y = if below { at.y } else { (at.y - size.y - 22.0).max(clip.pos.y + 4.0) };
    Rect { pos: dvec2(x, y), size }
}

impl ScoreEditor {
    /// Open the picker on instrument `node` at `at`.
    pub(super) fn open_picker(&mut self, cx: &mut Cx, node: &str, at: DVec2) {
        let (text, _) = self.live_text(cx).unwrap_or((self.source.clone(), None));
        let doc = makepad_score_live::ScoreDoc::load(&text).ok().or_else(|| makepad_score_live::ScoreDoc::load(&self.source).ok());
        let choices = makepad_score_live::instruments::catalog(doc.as_ref(), &self.available);
        let current = makepad_score_live::instruments::current_key(&text, node);
        let sel = current.as_ref().and_then(|c| choices.iter().position(|x| x.key() == *c)).unwrap_or(0);
        self.picker = Some(Picker {
            node: node.to_string(), at, filter: String::new(), sel, choices, current, rows: Vec::new(), panel: Rect::default(),
            top: 0.0, list: Rect::default(), reveal: true, mouse: None, opened_at: Some(at), pressed: None,
        });
        self.completions = None;
        self.hover = None;
        self.view.redraw(cx);
    }

    fn choose(&mut self, cx: &mut Cx, index: usize) {
        let Some(picker) = self.picker.take() else { return };
        self.hover = None;
        let Some(choice) = picker.filtered().get(index).map(|c| (*c).clone()) else { return };
        let Some((text, _)) = self.live_text(cx) else { return };
        if let Ok(splice) = makepad_score_live::instruments::switch(&text, &picker.node, &choice) {
            cx.widget_action(self.widget_uid(), ScoreEditorAction::Replace { start: splice.start, end: splice.end, text: splice.text });
        }
        self.view.redraw(cx);
    }

    /// The picker has the keys and the mouse while it is open; true when
    /// it took the event.
    pub(super) fn picker_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Some(picker) = self.picker.as_mut() else { return false };
        match event {
            Event::KeyDown(k) => {
                let n = picker.filtered().len();
                picker.reveal = true;
                match k.key_code {
                    KeyCode::Escape => self.picker = None,
                    KeyCode::ArrowDown => picker.sel = (picker.sel + 1).min(n.saturating_sub(1)),
                    KeyCode::ArrowUp => picker.sel = picker.sel.saturating_sub(1),
                    KeyCode::PageDown => picker.sel = (picker.sel + 10).min(n.saturating_sub(1)),
                    KeyCode::PageUp => picker.sel = picker.sel.saturating_sub(10),
                    KeyCode::Home => picker.sel = 0,
                    KeyCode::End => picker.sel = n.saturating_sub(1),
                    KeyCode::ReturnKey | KeyCode::Tab => {
                        let sel = picker.sel;
                        self.choose(cx, sel);
                    }
                    KeyCode::Backspace => {
                        picker.filter.pop();
                        picker.sel = 0;
                        picker.top = 0.0;
                    }
                    _ => {}
                }
            }
            Event::TextInput(t) => {
                picker.filter.push_str(&t.input);
                picker.sel = 0;
                picker.top = 0.0;
            }
            Event::MouseDown(e) => {
                if picker.panel.contains(e.abs) {
                    picker.pressed = picker.row_at(e.abs);
                    picker.opened_at = None;
                } else {
                    self.picker = None;
                    self.swallow_up = true;
                }
            }
            Event::MouseUp(e) => {
                let over = picker.row_at(e.abs);
                let opener = picker.opened_at.take().is_some_and(|at| (at - e.abs).length() < 16.0);
                let chosen = match (over, picker.pressed.take()) {
                    (Some(row), Some(pressed)) => (row == pressed).then_some(row),
                    (Some(row), None) if !opener => Some(row),
                    _ => None,
                };
                if let Some(row) = chosen {
                    self.choose(cx, row);
                }
            }
            Event::MouseMove(e) => {
                picker.mouse = Some(e.abs);
                match picker.row_at(e.abs) {
                    Some(index) if picker.sel != index => picker.sel = index,
                    _ => return true,
                }
            }
            Event::Scroll(e) if picker.panel.contains(e.abs) => {
                picker.top += e.scroll.y;
                picker.mouse = Some(e.abs);
                picker.reveal = false;
            }
            _ => return false,
        }
        self.view.redraw(cx);
        true
    }

    /// Recompute the completions at the cursor (after an edit).
    pub(super) fn refresh_completions(&mut self, cx: &mut Cx) {
        let Some((text, Some(cursor))) = self.live_text(cx) else { return };
        match makepad_score_live::assist::complete(&text, cursor, &self.available) {
            Some(c) => {
                let at = self.cursor_point(cx).unwrap_or_default();
                self.completions = Some((c, 0, at));
            }
            None => self.completions = None,
        }
        self.view.redraw(cx);
    }

    /// Take the selected completion: the word is replaced in the editor.
    fn accept_completion(&mut self, cx: &mut Cx) {
        let Some((c, sel, _)) = self.completions.take() else { return };
        let Some(item) = c.items.get(sel) else { return };
        let code = self.view.widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            if let Some(session) = view.session.as_mut() {
                use makepad_code_editor::history::NewGroup;
                use makepad_code_editor::selection::Affinity;
                use makepad_code_editor::session::SelectionMode;
                let text = session.document().as_text().to_string();
                let pos = |at: usize| {
                    let before = &text[..at.min(text.len())];
                    makepad_code_editor::text::Position { line_index: before.matches('\n').count(), byte_index: at - before.rfind('\n').map_or(0, |nl| nl + 1) }
                };
                session.set_selection(pos(c.start), Affinity::Before, SelectionMode::Simple, NewGroup::Yes);
                session.move_to(pos(c.end), Affinity::Before, NewGroup::No);
                session.insert(item.insert.as_str().into());
                session.handle_changes();
            }
            view.redraw(cx);
        }
        self.view.redraw(cx);
    }

    /// The navigation keys while completions show; true when taken.
    pub(super) fn completion_keys(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let (Some((c, sel, _)), Event::KeyDown(k)) = (self.completions.as_mut(), event) else { return false };
        match k.key_code {
            KeyCode::ArrowDown => *sel = (*sel + 1).min(c.items.len().saturating_sub(1)),
            KeyCode::ArrowUp => *sel = sel.saturating_sub(1),
            KeyCode::Tab | KeyCode::ReturnKey => {
                self.accept_completion(cx);
                return true;
            }
            KeyCode::Escape => self.completions = None,
            _ => return false,
        }
        self.view.redraw(cx);
        true
    }

    pub(super) fn draw_note(&mut self, cx: &mut Cx2d, p: &Palette, at: DVec2, text: &str, clip: Rect) {
        let lines: Vec<&str> = text.lines().collect();
        let w = lines.iter().map(|l| advance(&self.draw_label, cx, l)).fold(0.0, f64::max) + 18.0;
        let rect = place_panel(at, dvec2(w.min(clip.size.x - 8.0), 12.0 + lines.len() as f64 * 13.0), clip);
        self.box_(cx, rect, rgba(p.menu), rgba(p.border_hi), 5.0);
        self.draw_label.color = rgba(p.text);
        for (i, line) in lines.iter().enumerate() {
            self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 9.0, rect.pos.y + 6.0 + i as f64 * 13.0), line);
        }
    }

    pub(super) fn draw_completions(&mut self, cx: &mut Cx2d, p: &Palette, clip: Rect) {
        let Some((c, sel, at)) = self.completions.clone() else { return };
        const ROW: f64 = 17.0;
        const SHOW: usize = 10;
        let first = sel.saturating_sub(SHOW - 1);
        let items: Vec<_> = c.items.iter().enumerate().skip(first).take(SHOW).collect();
        let w = items.iter().map(|(_, i)| advance(&self.draw_value, cx, &i.label)).fold(120.0, f64::max) + 24.0;
        let detail = c.items.get(sel).map(|i| i.detail.clone()).unwrap_or_default();
        let dw = if detail.is_empty() { 0.0 } else { advance(&self.draw_label, cx, &detail).min(420.0) + 18.0 };
        let rect = place_panel(at, dvec2(w, items.len() as f64 * ROW + 6.0), clip);
        self.box_(cx, rect, rgba(p.menu), rgba(p.border_hi), 5.0);
        for (row, (index, item)) in items.iter().enumerate() {
            let y = rect.pos.y + 3.0 + row as f64 * ROW;
            if *index == sel {
                self.box_(cx, Rect { pos: dvec2(rect.pos.x + 3.0, y), size: dvec2(rect.size.x - 6.0, ROW) }, rgba(p.accent_wash), rgba(p.accent_wash), 3.0);
            }
            self.draw_value.color = rgba(if *index == sel { p.text_bright } else { p.text });
            self.draw_value.draw_abs(cx, dvec2(rect.pos.x + 9.0, y + 2.5), &item.label);
        }
        if dw > 0.0 {
            let d = Rect { pos: dvec2(rect.pos.x + rect.size.x + 4.0, rect.pos.y), size: dvec2(dw, 22.0) };
            self.box_(cx, d, rgba(p.panel), rgba(p.line), 5.0);
            self.draw_label.color = rgba(p.secondary);
            self.draw_label.draw_abs(cx, dvec2(d.pos.x + 9.0, d.pos.y + 6.0), &detail);
        }
    }

    pub(super) fn draw_picker(&mut self, cx: &mut Cx2d, p: &Palette, clip: Rect) {
        let Some(mut picker) = self.picker.take() else { return };
        const ROW: f64 = 18.0;
        const SHOW: f64 = 16.0;
        let filtered: Vec<(usize, Choice)> = picker.filtered().into_iter().cloned().enumerate().collect();
        let sel = picker.sel.min(filtered.len().saturating_sub(1));
        // Every line: a group's header where the group changes, then rows.
        let mut lines: Vec<Option<usize>> = Vec::new();
        let mut last_group = "";
        for (index, c) in &filtered {
            if c.group != last_group {
                lines.push(None);
                last_group = c.group;
            }
            lines.push(Some(*index));
        }
        let total = lines.len().max(1) as f64 * ROW;
        let view_h = total.min(SHOW * ROW);
        let rect = place_panel(picker.at, dvec2(300.0, 32.0 + view_h + 28.0), clip);
        let list = Rect { pos: dvec2(rect.pos.x, rect.pos.y + 30.0), size: dvec2(rect.size.x, view_h) };
        if picker.reveal {
            if let Some(line) = lines.iter().position(|l| *l == Some(sel)) {
                let y = line as f64 * ROW;
                if y < picker.top + ROW {
                    picker.top = (y - ROW).max(0.0);
                } else if y + ROW > picker.top + view_h {
                    picker.top = y + ROW - view_h;
                }
            }
            picker.reveal = false;
        }
        picker.top = picker.top.clamp(0.0, (total - view_h).max(0.0));
        picker.panel = rect;
        picker.list = list;
        picker.rows.clear();
        self.box_(cx, rect, rgba(p.menu), rgba(p.border_hi), 6.0);
        // The head: what it switches, or the filter typed.
        let head = if picker.filter.is_empty() { format!("{}  ·  switch to…", picker.node) } else { format!("{}  ·  {}", picker.node, picker.filter) };
        self.draw_name.color = rgba(p.text_bright);
        self.draw_name.draw_abs(cx, dvec2(rect.pos.x + 12.0, rect.pos.y + 9.0), &head);
        self.fill(cx, Rect { pos: dvec2(rect.pos.x + 1.0, rect.pos.y + 28.0), size: dvec2(rect.size.x - 2.0, 1.0) }, rgba(p.line));
        let hover_at = picker.mouse.filter(|m| list.contains(*m));
        cx.begin_turtle(Walk::abs_rect(list), Layout::default().with_clip(true, true));
        let first = (picker.top / ROW).floor() as usize;
        for (line, entry) in lines.iter().enumerate().skip(first).take(SHOW as usize + 2) {
            let y = list.pos.y + line as f64 * ROW - picker.top;
            match entry {
                None => {
                    let group = lines[line + 1..].iter().flatten().next().and_then(|i| filtered.get(*i)).map_or("", |(_, c)| c.group);
                    self.draw_caps.color = rgba(p.muted);
                    self.draw_caps.draw_abs(cx, dvec2(rect.pos.x + 12.0, y + 6.0), &group.to_uppercase());
                }
                Some(index) => {
                    let Some((_, c)) = filtered.get(*index) else { continue };
                    let row = Rect { pos: dvec2(rect.pos.x + 4.0, y), size: dvec2(rect.size.x - 8.0, ROW) };
                    if hover_at.is_some_and(|m| row.contains(m)) {
                        picker.sel = *index;
                    }
                    let selected = *index == picker.sel;
                    if selected {
                        self.box_(cx, row, rgba(p.accent_wash), rgba(p.accent_wash), 3.0);
                    }
                    let current = picker.current.as_deref() == Some(c.key().as_str());
                    if current {
                        self.fill(cx, Rect { pos: dvec2(row.pos.x + 5.0, y + 5.0), size: dvec2(2.0, ROW - 10.0) }, rgba(p.accent));
                    }
                    self.draw_value.color = rgba(if current { p.accent } else if selected { p.text_bright } else { p.text });
                    self.draw_value.draw_abs(cx, dvec2(row.pos.x + 14.0, y + 3.0), &c.label);
                    picker.rows.push((row, *index));
                }
            }
        }
        cx.end_turtle();
        if filtered.is_empty() {
            self.draw_label.color = rgba(p.muted);
            self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 12.0, list.pos.y + 4.0), "nothing matches");
        }
        // What the selected one is.
        self.fill(cx, Rect { pos: dvec2(rect.pos.x + 1.0, rect.pos.y + rect.size.y - 26.0), size: dvec2(rect.size.x - 2.0, 1.0) }, alpha(p.line, 1.0));
        if let Some((_, c)) = filtered.get(picker.sel) {
            self.draw_label.color = rgba(p.secondary);
            let detail: String = c.detail.chars().take(48).collect();
            self.draw_label.draw_abs(cx, dvec2(rect.pos.x + 12.0, rect.pos.y + rect.size.y - 19.0), &detail);
        }
        self.picker = Some(picker);
    }
}
