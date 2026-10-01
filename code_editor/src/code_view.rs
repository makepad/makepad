use crate::{
    code_editor::{CodeEditorAction, KeepCursorInView}, decoration::DecorationSet, history::NewGroup,
    makepad_widgets_core::*, selection::Affinity, session::SelectionMode, str::StrExt, text::Position,
    CodeDocument, CodeEditor, CodeSession,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CodeViewBase = set_type_default() do #(CodeView::register_widget(vm)){
        editor +: {
            pad_left_top: vec2(0.0, -0.0)
            height: Fit
            empty_page_at_end: false
            read_only: true
            show_gutter: false
            word_wrap: false
            draw_bg +: { color: #0000 }
        }
        // Find: Cmd/Ctrl+F (or the magnifier) opens this bar over the
        // top-right corner of the code; Esc closes it.
        find_bar: RoundedView{
            width: 320 height: Fit
            flow: Right spacing: 2 align: Align{y: 0.5}
            padding: Inset{left: 4 right: 2 top: 3 bottom: 3}
            show_bg: true
            draw_bg +: {
                color: theme.color_bg_app
                border_size: 1.0
                border_radius: 4.0
                border_color: theme.color_bevel_outset_1
            }
            query := TextInput{width: Fill height: Fit empty_text: "Find"}
            count := Label{
                width: Fit height: Fit padding: Inset{left: 4 right: 4} text: ""
                draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 8}}
            }
            prev := ButtonFlatter{width: 22 height: 22 padding: 0 align: Align{x: 0.5 y: 0.5} text: "↑"}
            next := ButtonFlatter{width: 22 height: 22 padding: 0 align: Align{x: 0.5 y: 0.5} text: "↓"}
            close := ButtonFlatter{width: 22 height: 22 padding: 0 align: Align{x: 0.5 y: 0.5} text: "×"}
        }
        find_open: View{
            width: Fit height: Fit
            open := ButtonFlatterIcon{
                width: 22 height: 22 padding: 0 align: Align{x: 0.5 y: 0.5}
                icon_walk: Walk{width: 11 height: 11}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_search.svg")}
            }
        }
    }

    mod.widgets.CodeView = mod.widgets.CodeViewBase {}
}

/// What a CodeView tells its host.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CodeViewAction {
    /// The document changed (a keystroke, a paste, an undo).
    Changed,
    #[default]
    None,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct CodeView {
    #[uid]
    uid: WidgetUid,
    #[live]
    pub editor: CodeEditor,
    // alright we have to have a session and a document.
    #[rust]
    pub session: Option<CodeSession>,
    #[live(false)]
    keep_cursor_at_end: bool,
    /// Indent width in columns; 4 by default, 2 where the host wants a
    /// lighter indent (the design tweaker's shader view).
    #[live(4)]
    tab_column_count: usize,

    #[live]
    text: ArcStringMut,

    /// The find bar drawn over the code while find is open.
    #[live]
    find_bar: WidgetRef,
    /// The magnifier that opens find, over a scrolling view.
    #[live]
    find_open: WidgetRef,
    #[rust]
    find: Find,
    /// The bar's own draw list, drawn after the code so it sits on top and
    /// nothing in it batches into the code's draw calls; redrawn with the
    /// view, so a recount after an edit shows at once.
    #[rust]
    find_list: Option<DrawList2d>,
}

/// What find is doing in one view.
#[derive(Default)]
struct Find {
    open: bool,
    query: String,
    /// Typing searches from here: where the caret was when find opened.
    anchor: Position,
    /// The current match's start, so a recount after an edit keeps it.
    current_start: Option<Position>,
    /// Whether the magnifier was drawn (a Fit-height snippet gets none).
    show_open: bool,
    /// The query field takes the keyboard once it has been drawn: before its
    /// first draw it has no area to hold the focus.
    focus_pending: bool,
}

/// Every non-overlapping match of `query` in `lines`, as (start, end) per
/// line. Case-insensitive unless the query has a capital; a match starts
/// and ends on grapheme boundaries, so it can always be laid out.
pub fn find_matches(lines: &[String], query: &str) -> Vec<(Position, Position)> {
    let mut matches = Vec::new();
    if query.is_empty() || query.contains('\n') {
        return matches;
    }
    let case_sensitive = query.chars().any(char::is_uppercase);
    let needle = query.as_bytes();
    for (line_index, line) in lines.iter().enumerate() {
        if line.len() < needle.len() {
            continue;
        }
        let bytes = line.as_bytes();
        let mut bounds: Vec<usize> = line.grapheme_indices().map(|(i, _)| i).collect();
        bounds.push(line.len());
        let mut next_free = 0;
        for &start in &bounds {
            if start < next_free {
                continue;
            }
            let end = start + needle.len();
            if end > bytes.len() {
                break;
            }
            let hay = &bytes[start..end];
            let hit = if case_sensitive { hay == needle } else { hay.eq_ignore_ascii_case(needle) };
            if hit && bounds.binary_search(&end).is_ok() {
                matches.push((
                    Position { line_index, byte_index: start },
                    Position { line_index, byte_index: end },
                ));
                next_free = end;
            }
        }
    }
    matches
}

impl WidgetNode for CodeView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.editor.walk(cx)
    }
    fn area(&self) -> Area {
        self.editor.area()
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.editor.redraw(cx)
    }
    fn set_scroll_pos(&mut self, cx: &mut Cx, v: Vec2d) {
        self.editor.set_scroll_pos(cx, v)
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.editor.find_widgets_from_point(cx, point, found)
    }
    fn visible(&self) -> bool {
        self.editor.visible()
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.editor.set_visible(cx, visible)
    }

    // Selection API - map to code editor document text
    fn selection_text_len(&self) -> usize {
        self.text.as_ref().len()
    }

    fn selection_point_to_char_index(&self, _cx: &Cx, abs: DVec2) -> Option<usize> {
        // Use the editor's pick method for precise character mapping
        let session = self.session.as_ref()?;
        let rect = self.editor.viewport_rect();
        if rect.size.y <= 0.0 {
            return None;
        }
        let ((position, _affinity), _) = self.editor.pick(session, abs);
        let text = self.text.as_ref();
        Some(CodeView::position_to_byte_offset(text, position))
    }

    fn selection_set(&mut self, anchor: usize, cursor: usize) {
        self.lazy_init_session();
        let text = self.text.as_ref().to_string();
        let anchor_pos = CodeView::byte_offset_to_position(&text, anchor);
        let cursor_pos = CodeView::byte_offset_to_position(&text, cursor);
        if let Some(session) = &self.session {
            session.set_selection(
                anchor_pos,
                Affinity::Before,
                SelectionMode::Simple,
                NewGroup::Yes,
            );
            session.move_to(cursor_pos, Affinity::Before, NewGroup::Yes);
        }
        // When receiving external selection, use focus colors even without key focus
        self.editor.set_external_selection_focus_no_redraw(true);
    }

    fn selection_clear(&mut self) {
        // Clear external selection focus when selection is cleared
        self.editor.set_external_selection_focus_no_redraw(false);
        if let Some(session) = &self.session {
            // Extract cursor position, then drop the Ref before mutating
            let pos = {
                let selections = session.selections();
                if selections.is_empty() {
                    return;
                }
                selections[0].cursor.position
            };
            session.set_selection(pos, Affinity::Before, SelectionMode::Simple, NewGroup::Yes);
        }
    }

    fn selection_select_all(&mut self) {
        self.lazy_init_session();
        let text = self.text.as_ref();
        let text_len = text.len();
        let start = Position {
            line_index: 0,
            byte_index: 0,
        };
        let end = CodeView::byte_offset_to_position(text, text_len);
        if let Some(session) = &self.session {
            session.set_selection(
                start,
                Affinity::Before,
                SelectionMode::Simple,
                NewGroup::Yes,
            );
            session.move_to(end, Affinity::Before, NewGroup::Yes);
        }
        // When receiving external selection, use focus colors even without key focus
        self.editor.set_external_selection_focus_no_redraw(true);
    }

    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        let text = self.text.as_ref();
        let start = start.min(text.len());
        let end = end.min(text.len());
        if start >= end {
            return String::new();
        }
        text[start..end].to_string()
    }

    fn selection_get_full_text(&self) -> String {
        self.text.as_ref().to_string()
    }
}

impl CodeView {
    pub fn lazy_init_session(&mut self) {
        if self.session.is_none() {
            let dec = DecorationSet::new();
            let doc = CodeDocument::new(self.text.as_ref().into(), dec);
            self.session = Some(CodeSession::new(doc));
            self.session
                .as_mut()
                .unwrap()
                .set_tab_column_count(self.tab_column_count);
            self.session.as_mut().unwrap().handle_changes();
            if self.keep_cursor_at_end {
                self.session.as_mut().unwrap().set_cursor_at_file_end();
                self.editor.keep_cursor_in_view = KeepCursorInView::Once
            }
        }
    }

    /// Convert a byte offset into the text to a Position (line_index, byte_index).
    fn byte_offset_to_position(text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());
        let mut line_index = 0;
        let mut line_start = 0;
        for (i, ch) in text.char_indices() {
            if i >= offset {
                break;
            }
            if ch == '\n' {
                line_index += 1;
                line_start = i + 1;
            }
        }
        Position {
            line_index,
            byte_index: offset - line_start,
        }
    }

    /// Convert a Position (line_index, byte_index) to an absolute byte offset.
    fn position_to_byte_offset(text: &str, pos: Position) -> usize {
        let mut offset = 0;
        for (i, line) in text.split('\n').enumerate() {
            if i == pos.line_index {
                return offset + pos.byte_index.min(line.len());
            }
            offset += line.len() + 1; // +1 for the '\n'
        }
        // Past end of text
        text.len()
    }
}

impl CodeView {
    pub fn find_is_open(&self) -> bool {
        self.find.open
    }

    /// Open find with the selection (one line of it) as the query, the
    /// keyboard in the query field.
    pub fn open_find(&mut self, cx: &mut Cx) {
        self.lazy_init_session();
        let session = self.session.as_ref().unwrap();
        let selection = session.selections()[session.last_added_selection_index().unwrap_or(0)];
        let (start, end) = (selection.start(), selection.end());
        self.find.anchor = start;
        if start.line_index == end.line_index && start.byte_index < end.byte_index {
            let document = session.document();
            let selected = document.as_text().as_lines()[start.line_index]
                .get(start.byte_index..end.byte_index)
                .map(str::to_string);
            if let Some(selected) = selected {
                self.find.query = selected;
            }
        }
        self.find.open = true;
        self.find_bar.text_input(cx, ids!(query)).set_text(cx, &self.find.query);
        self.find.focus_pending = true;
        self.find.current_start = None;
        self.refresh_find();
        self.jump_to_current(cx);
        self.redraw(cx);
    }

    /// Close find and hand the keyboard back to the code.
    pub fn close_find(&mut self, cx: &mut Cx) {
        self.find.open = false;
        self.editor.find_matches.clear();
        self.editor.find_current = None;
        self.editor.set_key_focus(cx);
        self.redraw(cx);
    }

    /// Recount the matches in the document as it is now, keeping the current
    /// one where it was (or the next one after it).
    fn refresh_find(&mut self) {
        let Some(session) = self.session.as_ref() else { return };
        let matches = find_matches(session.document().as_text().as_lines(), &self.find.query);
        let from = self.find.current_start.unwrap_or(self.find.anchor);
        let current = if matches.is_empty() {
            None
        } else {
            Some(matches.iter().position(|m| m.0 >= from).unwrap_or(0))
        };
        self.find.current_start = current.map(|i| matches[i].0);
        self.editor.find_matches = matches;
        self.editor.find_current = current;
    }

    /// Step to the next (+1) or previous (-1) match, wrapping.
    fn step_find(&mut self, cx: &mut Cx, step: isize) {
        let count = self.editor.find_matches.len();
        if count == 0 {
            return;
        }
        let current = match self.editor.find_current {
            Some(current) => (current as isize + step).rem_euclid(count as isize) as usize,
            None => 0,
        };
        self.editor.find_current = Some(current);
        self.find.current_start = Some(self.editor.find_matches[current].0);
        self.jump_to_current(cx);
    }

    /// Select the current match and scroll it into view.
    fn jump_to_current(&mut self, cx: &mut Cx) {
        let Some(current) = self.editor.find_current else {
            self.redraw(cx);
            return;
        };
        let (start, end) = self.editor.find_matches[current];
        let session = self.session.as_mut().unwrap();
        self.editor.set_selection_and_scroll(cx, start, end, session);
    }

    fn find_count_text(&self) -> String {
        if self.find.query.is_empty() {
            return String::new();
        }
        match self.editor.find_current {
            Some(current) => format!("{} of {}", current + 1, self.editor.find_matches.len()),
            None => "No results".to_string(),
        }
    }

    fn handle_find_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.find.open {
            let actions = cx.capture_actions(|cx| self.find_bar.handle_event(cx, event, scope));
            if actions.is_empty() {
                return;
            }
            let query = self.find_bar.text_input(cx, ids!(query));
            if let Some(text) = query.changed(&actions) {
                self.find.query = text;
                self.find.current_start = None;
                self.refresh_find();
                self.jump_to_current(cx);
            }
            if let Some((_, modifiers)) = query.returned(&actions) {
                self.step_find(cx, if modifiers.shift { -1 } else { 1 });
                // A single-line field lets go of the keyboard on Enter; the
                // next Enter should step again.
                if let Some(mut input) = query.borrow_mut() {
                    input.take_key_focus(cx);
                }
            }
            if self.find_bar.button(cx, ids!(next)).clicked(&actions) {
                self.step_find(cx, 1);
            }
            if self.find_bar.button(cx, ids!(prev)).clicked(&actions) {
                self.step_find(cx, -1);
            }
            if query.escaped(&actions) || self.find_bar.button(cx, ids!(close)).clicked(&actions) {
                self.close_find(cx);
            }
            if let Some(TextInputAction::KeyDownUnhandled(KeyEvent { key_code: KeyCode::KeyF, modifiers, .. })) =
                actions.filter_widget_actions_cast::<TextInputAction>(query.widget_uid()).last()
            {
                if modifiers.is_primary() {
                    if let Some(mut input) = query.borrow_mut() {
                        input.select_all(cx);
                    }
                }
            }
        } else if self.find.show_open {
            let actions = cx.capture_actions(|cx| self.find_open.handle_event(cx, event, scope));
            if self.find_open.button(cx, ids!(open)).clicked(&actions) {
                self.open_find(cx);
            }
        }
    }

    /// The bar (or the magnifier) over the top-right corner of the code.
    fn draw_find(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) {
        self.find.show_open = !walk.height.is_fit();
        if !self.find.open && !self.find.show_open {
            return;
        }
        let rect = self.editor.area().rect(cx);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        // Clear of the vertical scroll bar.
        let right = rect.pos.x + rect.size.x - 14.0;
        let top = rect.pos.y + 6.0;
        let mut list = self.find_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_always(cx);
        if self.find.open {
            let count = self.find_count_text();
            self.find_bar.widget(cx, ids!(count)).set_text(cx, &count);
            let mut bar_walk = self.find_bar.walk(cx);
            let width = match bar_walk.width {
                Size::Fixed(width) => width,
                _ => 320.0,
            }
            .min((rect.size.x - 24.0).max(120.0));
            bar_walk.width = Size::Fixed(width);
            bar_walk.abs_pos = Some(dvec2(right - width, top));
            cx.widget_tree_insert_child(self.uid, live_id!(find_bar), self.find_bar.clone());
            let _ = self.find_bar.draw_walk(cx, scope, bar_walk);
            if std::mem::take(&mut self.find.focus_pending) {
                if let Some(mut input) = self.find_bar.text_input(cx, ids!(query)).borrow_mut() {
                    input.take_key_focus(cx);
                    input.select_all(cx);
                }
            }
        } else {
            let mut open_walk = self.find_open.walk(cx);
            open_walk.abs_pos = Some(dvec2(right - 22.0, top));
            cx.widget_tree_insert_child(self.uid, live_id!(find_open), self.find_open.clone());
            let _ = self.find_open.draw_walk(cx, scope, open_walk);
        }
        list.end(cx);
        self.find_list = Some(list);
    }
}

impl Widget for CodeView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // alright so.
        self.lazy_init_session();
        // alright we have a scope, and an id, so now we can properly draw the editor.
        if self.find.open {
            self.refresh_find();
        }
        let session = self.session.as_mut().unwrap();

        self.editor.draw_walk_editor(cx, session, walk);
        self.draw_find(cx, scope, walk);

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.lazy_init_session();
        // The bar sits over the code, so it sees a press first.
        self.handle_find_event(cx, event, scope);
        let uid = self.uid;
        let session = self.session.as_mut().unwrap();
        let mut open_find = false;
        let mut changed = false;
        for action in self
            .editor
            .handle_event(cx, event, &mut Scope::empty(), session)
        {
            session.handle_changes();
            match action {
                CodeEditorAction::TextDidChange => {
                    changed = true;
                    cx.widget_action(uid, CodeViewAction::Changed);
                }
                CodeEditorAction::UnhandledKeyDown(KeyEvent {
                    key_code: KeyCode::KeyF,
                    modifiers,
                    ..
                }) if modifiers.is_primary() && !modifiers.shift && !modifiers.alt => open_find = true,
                _ => {}
            }
        }
        if changed && self.find.open {
            self.refresh_find();
        }
        if open_find {
            self.open_find(cx);
        }
    }

    fn text(&self) -> String {
        // The document is the truth once the view is editable: what the
        // person typed, not what was handed in.
        match &self.session {
            Some(session) => session.document().as_text().to_string(),
            None => self.text.as_ref().to_string(),
        }
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text.as_ref() != v {
            self.text.as_mut_empty().push_str(v);
            self.session = None;
            self.redraw(cx);
        }
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn find_is_case_insensitive_until_the_query_has_a_capital() {
        let text = lines("let Glow = glow\nGLOW glowglow");
        let starts = |query| find_matches(&text, query).iter().map(|m| (m.0.line_index, m.0.byte_index)).collect::<Vec<_>>();
        assert_eq!(starts("glow"), vec![(0, 4), (0, 11), (1, 0), (1, 5), (1, 9)]);
        assert_eq!(starts("Glow"), vec![(0, 4)]);
        assert_eq!(starts(""), vec![]);
    }

    #[test]
    fn find_matches_do_not_overlap_and_end_on_graphemes() {
        let text = lines("aaaa é");
        assert_eq!(find_matches(&text, "aa").len(), 2);
        let accent = find_matches(&text, "é");
        assert_eq!(accent.len(), 1);
        assert_eq!((accent[0].0.byte_index, accent[0].1.byte_index), (5, 7));
    }
}
