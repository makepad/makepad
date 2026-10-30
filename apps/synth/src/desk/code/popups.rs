//! What opens over the code: completions at the cursor, the docs of the
//! name under the resting mouse (or what a lens control does), and the
//! instrument browser (`makepad_synth_panel::browser`, the one every
//! picker uses), anchored under the card that opened it: it stays open
//! while the person picks, each pick heard in place and written at once.

use super::{ScoreEditor, ScoreEditorAction};
use makepad_code_editor::code_view::CodeView;
use makepad_look::palette::Palette;
use makepad_synth_panel::browser::Browser;
use makepad_widgets::badge::advance;
use makepad_widgets::*;

/// Completions at the cursor, the one selected, where they show.
pub type Completing = (makepad_score_live::assist::Completions, usize, DVec2);

fn rgba(c: u32) -> Vec4f {
    Vec4f::from_u32(c)
}

/// A panel that stays inside the view.
fn place_panel(at: DVec2, size: DVec2, clip: Rect) -> Rect {
    let x = at.x.min(clip.pos.x + clip.size.x - size.x - 4.0).max(clip.pos.x + 4.0);
    let below = at.y + size.y <= clip.pos.y + clip.size.y;
    let y = if below { at.y } else { (at.y - size.y - 22.0).max(clip.pos.y + 4.0) };
    Rect { pos: dvec2(x, y), size }
}

impl ScoreEditor {
    /// Open the browser on instrument `node`, anchored at `at`.
    pub(super) fn open_picker(&mut self, cx: &mut Cx, node: &str, at: DVec2) {
        self.browser.popped = false;
        self.browser.open(node);
        self.browser_at = at;
        self.completions = None;
        self.hover = None;
        self.view.redraw(cx);
    }

    /// The browser has the keys and the mouse while it is open; true when
    /// it took the event.
    pub(super) fn picker_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.browser_tick.is_event(event).is_some() {
            self.browser.tick();
            self.view.redraw(cx);
        }
        let out = self.browser.handle_event(cx, event);
        if let Some(true) = out.pop {
            let node = self.browser.node.clone();
            self.browser.close();
            cx.widget_action(self.widget_uid(), ScoreEditorAction::PopOut(node));
        }
        if out.close {
            // The press that closed it does not reach the code.
            if matches!(event, Event::MouseDown(_)) {
                self.swallow_up = true;
                self.view.redraw(cx);
                return true;
            }
        }
        if out.redraw {
            self.view.redraw(cx);
            self.browser_tick = cx.new_next_frame();
        }
        out.consumed || out.close
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

    pub(super) fn draw_picker(&mut self, cx: &mut Cx2d, _p: &Palette, clip: Rect) {
        if !self.browser.is_open() {
            return;
        }
        let rect = Browser::place(self.browser_at, clip);
        self.browser.layer(cx);
        if self.browser.draw(cx, rect) {
            self.browser_tick = cx.new_next_frame();
        }
    }
}
