//! Quick-open (Cmd/Ctrl+P): a field over the window that ranks the repo's
//! files by a fuzzy match on what you type; Enter opens the pick. It looks
//! like the command palette: a chamfered frame, the ▸ glyph, the field and
//! up to twelve rows (file name bright, folder dim, matched characters lit).
//!
//! `open(cx)` shows it with the keyboard in its field; a pick emits
//! `IdeAction::OpenFile` (a `path:42` or `path:42:7` query opens at that
//! line and column); Esc closes it. With nothing typed, the files opened in
//! this session come first, most recent on top; `note_opened` tells it
//! about opens made elsewhere. Closed, it takes no space and no input.

use super::files::{name_then_dir, HitText};
use makepad_code_agent::index::FileMatch;
use super::{IdeAction, Workspace};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.QuickOpenBase = #(QuickOpen::register_widget(vm))

    let Field = TextInput{
        draw_text +: {
            color: mod.look.text_hi
            color_hover: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            color_down: uniform(mod.look.text_hi)
            color_empty: uniform(mod.look.dim)
            color_empty_hover: uniform(mod.look.dim)
            color_empty_focus: uniform(mod.look.dim)
            text_style: mod.look.mono{font_size: 9.5}
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

    let QoRow = View{
        visible: false
        width: Fill
        height: 24
        flow: Right
        align: Align{y: 0.5}
        padding: Inset{left: 12 right: 12}
        cursor: MouseCursor.Hand
        grab_key_focus: false
        show_bg: true
        draw_bg +: {
            color: #0000
            pixel: fn() {
                let p = self.pos * self.rect_size
                // The selected row's wash carries a bar on its left.
                let bar = (1.0 - step(2.0, p.x)) * step(0.001, self.color.a)
                let c = mix(self.color, mod.look.agent, bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
        qo_text := mod.widgets.CodeHitText{
            height: 24
            draw_text +: { text_style: mod.look.mono{font_size: 9.0} }
        }
    }

    mod.widgets.QuickOpen = set_type_default() do mod.widgets.QuickOpenBase{
        // The modal is an overlay of its own: closed, this takes nothing.
        width: Fit
        height: Fit
        modal := Modal{
            align: Align{x: 0.5 y: 0.14}
            bg_view +: { draw_bg.color: #x020306b8 }
            content +: {
                View{
                    width: 620
                    height: Fit
                    flow: Down
                    spacing: 4
                    padding: Inset{left: 8 right: 8 top: 8 bottom: 8}
                    show_bg: true
                    draw_bg +: {
                        color: mod.look.rail
                        pixel: fn() {
                            let p = self.pos * self.rect_size
                            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                            let c0 = self.rect_size * 0.5
                            let d = Finish.sd_chamfer(p, c0, c0 - vec2(0.5, 0.5), 8.0)
                            let inside = Finish.cover(d, px)
                            let edge = Finish.band(d, 0.0, px, px)
                            var c = vec4(self.color.rgb, 1.0) * inside
                            c = mix(c, vec4(mod.look.agent.rgb, 1.0), edge * 0.7)
                            return vec4(c.rgb * c.a, c.a)
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                        Label{
                            width: 12
                            margin: Inset{top: 3 right: 6}
                            text: "▸"
                            draw_text +: { color: mod.look.agent text_style: mod.look.mono_bold{font_size: 9.0} }
                        }
                        qo_input := Field{
                            width: Fill
                            height: Fit
                            empty_text: "open file   (path:line jumps to a line)"
                        }
                        qo_count := Label{
                            margin: Inset{top: 3}
                            draw_text +: { color: mod.look.dim text_style: mod.look.mono{font_size: 8.0} }
                        }
                    }
                    Rule{}
                    View{
                        width: Fill
                        height: Fit
                        flow: Down
                        qo_row_0 := QoRow{}
                        qo_row_1 := QoRow{}
                        qo_row_2 := QoRow{}
                        qo_row_3 := QoRow{}
                        qo_row_4 := QoRow{}
                        qo_row_5 := QoRow{}
                        qo_row_6 := QoRow{}
                        qo_row_7 := QoRow{}
                        qo_row_8 := QoRow{}
                        qo_row_9 := QoRow{}
                        qo_row_10 := QoRow{}
                        qo_row_11 := QoRow{}
                    }
                }
            }
        }
    }
}

fn rows() -> [&'static [LiveId; 1]; 12] {
    [
        ids!(qo_row_0),
        ids!(qo_row_1),
        ids!(qo_row_2),
        ids!(qo_row_3),
        ids!(qo_row_4),
        ids!(qo_row_5),
        ids!(qo_row_6),
        ids!(qo_row_7),
        ids!(qo_row_8),
        ids!(qo_row_9),
        ids!(qo_row_10),
        ids!(qo_row_11),
    ]
}

/// How many ranked files a query keeps (the rows scroll through them).
const LIMIT: usize = 200;
/// How many recently opened files it remembers.
const MRU: usize = 30;

#[derive(Script, ScriptHook, Widget)]
pub struct QuickOpen {
    #[deref]
    view: View,
    /// Opened in this session, most recent first.
    #[rust]
    recent: Vec<String>,
    #[rust]
    query: String,
    #[rust]
    matches: Vec<FileMatch>,
    #[rust]
    sel: usize,
    #[rust]
    first: usize,
    #[rust]
    want_focus: bool,
    #[rust]
    index_version: u64,
}

/// `path:42` or `path:42:7` → the path and the 1-based line and column.
pub fn parse_query(q: &str) -> (&str, Option<u32>, Option<u32>) {
    let q = q.trim();
    let mut parts = q.rsplitn(3, ':');
    let last = parts.next().unwrap_or("");
    let mid = parts.next();
    let head = parts.next();
    match (head, mid, last.parse::<u32>().ok()) {
        (Some(path), Some(line), Some(col)) if line.parse::<u32>().is_ok() => (path, line.parse().ok(), Some(col)),
        (_, Some(_), Some(line)) => {
            let path = &q[..q.rfind(':').unwrap()];
            (path, Some(line), None)
        }
        _ => (q.trim_end_matches(':'), None, None),
    }
}

impl QuickOpen {
    pub fn open(&mut self, cx: &mut Cx) {
        self.query.clear();
        self.sel = 0;
        self.first = 0;
        self.rank(cx);
        self.view.modal(cx, ids!(modal)).open(cx);
        self.view.text_input(cx, ids!(qo_input)).set_text(cx, "");
        self.want_focus = true;
        self.view.redraw(cx);
    }

    pub fn is_open(&self, cx: &mut Cx) -> bool {
        self.view.modal(cx, ids!(modal)).is_open()
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.view.modal(cx, ids!(modal)).close(cx);
        self.view.redraw(cx);
    }

    /// A file was opened somewhere else: it goes on top of the recent list.
    pub fn note_opened(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MRU);
    }

    fn rank(&mut self, cx: &Cx) {
        let Some(ws) = Workspace::of(cx) else {
            self.matches.clear();
            return;
        };
        self.index_version = ws.index.version();
        let (path, _, _) = parse_query(&self.query);
        if path.trim().is_empty() {
            let mut out: Vec<FileMatch> = self
                .recent
                .iter()
                .filter(|p| ws.root.join(p).is_file())
                .map(|p| FileMatch { path: p.clone(), score: 0, hits: Vec::new() })
                .collect();
            let files = ws.index.files();
            let fill = files.iter().filter(|p| !self.recent.contains(p)).take(LIMIT);
            out.extend(fill.map(|p| FileMatch { path: p.clone(), score: 0, hits: Vec::new() }));
            out.truncate(LIMIT);
            self.matches = out;
        } else {
            let mut found = ws.index.search(path, LIMIT);
            // Among equals, a file opened lately wins.
            for m in &mut found {
                if let Some(at) = self.recent.iter().position(|p| *p == m.path) {
                    m.score += 8 - (at as i32).min(8);
                }
            }
            found.sort_by(|a, b| b.score.cmp(&a.score));
            self.matches = found;
        }
        self.sel = self.sel.min(self.matches.len().saturating_sub(1));
    }

    fn pick(&mut self, cx: &mut Cx, index: usize) {
        let Some(m) = self.matches.get(index) else { return };
        let path = m.path.clone();
        let (_, line, col) = parse_query(&self.query);
        self.note_opened(&path);
        self.close(cx);
        cx.widget_action(self.view.widget_uid(), IdeAction::OpenFile { path, line, col });
    }

    fn draw_rows(&mut self, cx: &mut Cx2d) {
        let p = crate::ui::theme::palette();
        let slots = rows();
        if self.sel < self.first {
            self.first = self.sel;
        } else if self.sel >= self.first + slots.len() {
            self.first = self.sel + 1 - slots.len();
        }
        let count = if self.matches.len() >= LIMIT { format!("{}+", LIMIT) } else { self.matches.len().to_string() };
        self.view.label(cx, ids!(qo_count)).set_text(cx, &count);
        for (slot, path) in slots.iter().enumerate() {
            let mut row = self.view.view(cx, *path);
            let index = self.first + slot;
            let Some(m) = self.matches.get(index) else {
                row.set_visible(cx, false);
                continue;
            };
            row.set_visible(cx, true);
            let tint = if index == self.sel { crate::ui::theme::vec(p.primary, 0.22) } else { crate::ui::theme::vec(0, 0.0) };
            script_apply_eval!(cx, row, { draw_bg +: { color: #(tint) } });
            let runs = name_then_dir(
                &m.path,
                &m.hits,
                crate::ui::theme::vec(p.text_hi, 1.0),
                crate::ui::theme::vec(p.meta, 1.0),
                crate::ui::theme::vec(p.primary, 1.0),
            );
            if let Some(mut text) = row.widget(cx, ids!(qo_text)).borrow_mut::<HitText>() {
                text.set_runs(runs);
            }
        }
    }
}

impl Widget for QuickOpen {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let open = self.is_open(cx);
        if open {
            if let Event::Signal = event {
                if Workspace::of(cx).is_some_and(|ws| ws.index.version() != self.index_version) {
                    self.rank(cx);
                    self.view.redraw(cx);
                }
            }
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if actions.is_empty() || !open {
            return;
        }
        let modal = self.view.modal(cx, ids!(modal));
        if modal.dismissed(&actions) {
            return;
        }
        let input = self.view.text_input(cx, ids!(qo_input));
        if let Some(text) = input.changed(&actions) {
            if text != self.query {
                self.query = text;
                self.sel = 0;
                self.first = 0;
                self.rank(cx);
                self.view.redraw(cx);
            }
        }
        if let Some(key) = input.key_down_unhandled(&actions) {
            let last = self.matches.len().saturating_sub(1);
            match key.key_code {
                KeyCode::ArrowDown => self.sel = (self.sel + 1).min(last),
                KeyCode::ArrowUp => self.sel = self.sel.saturating_sub(1),
                KeyCode::PageDown => self.sel = (self.sel + rows().len()).min(last),
                KeyCode::PageUp => self.sel = self.sel.saturating_sub(rows().len()),
                _ => {}
            }
            self.view.redraw(cx);
        }
        if input.escaped(&actions) {
            self.close(cx);
            return;
        }
        if input.returned(&actions).is_some() {
            self.pick(cx, self.sel);
            return;
        }
        for (slot, path) in rows().iter().enumerate() {
            if self.view.view(cx, *path).finger_up(&actions).is_some_and(|f| f.is_over && f.device.is_primary_hit()) {
                self.pick(cx, self.first + slot);
                return;
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.view.modal(cx, ids!(modal)).is_open() {
            self.draw_rows(cx);
        }
        let step = self.view.draw_walk(cx, scope, walk);
        if std::mem::take(&mut self.want_focus) {
            self.view.text_input(cx, ids!(qo_input)).set_key_focus(cx);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_suffixes() {
        assert_eq!(parse_query("src/a.rs"), ("src/a.rs", None, None));
        assert_eq!(parse_query("src/a.rs:42"), ("src/a.rs", Some(42), None));
        assert_eq!(parse_query("src/a.rs:42:7"), ("src/a.rs", Some(42), Some(7)));
        assert_eq!(parse_query("a.rs:"), ("a.rs", None, None));
        assert_eq!(parse_query(" shell "), ("shell", None, None));
    }
}
