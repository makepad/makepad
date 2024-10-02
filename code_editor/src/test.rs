use {
    makepad_code_editor::{
        code_editor::*,
        state::{CodeDocument, CodeSession},
    },
    makepad_widgets::*,
st::{cell::RefCell, rc::Rc},
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_code_editor::code_editor::CodeEditor;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <CodeEditor> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    code_editor.draw(&mut cx, &mut self.state.session);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        if let Some(mut code_editor) = self.ui.get_code_editor(id!(code_editor)).borrow_mut() {
            code_editor.handle_event(cx, event, &mut self.state.session);
        }
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_code_editor::code_editor::live_design(cx);
    }
}

struct State {
    session: CodeSession,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: CodeSession::new(Rc::new(RefCell::new(CodeDocument::new(
                include_str!("state.rs").into(),
            )))),
        }
    }
}

app_main!(App);
use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => {
                    ChangeKind::Insert(range.start(), text.slice(range))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{
        line::Wrapped,
        selection::Affinity,
        state::{Block, CodeSession},
        str::StrExt,
        token::TokenKind,
        Line, Point, Selection, Token,
    },
    makepad_widgets::*,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    TokenColors = {{TokenColors}} {
        unknown: #808080,
        branch_keyword: #C485BE,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        whitespace: #6E6E6E,
    }

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        
            width: Fill,
            height: Fill,
            margin: 0,
        
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
    #[live]
    draw_text: DrawText,
    #[live]
    token_colors: TokenColors,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start: usize,
    #[rust]
    end: usize,
}

impl LiveHook for CodeEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, CodeEditor)
    }
}

impl Widget for CodeEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }

    fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        //let uid = self.widget_uid();
        /*self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });*/
        //self.handle_event
    }

    fn walk(&self) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CodeEditorRef(WidgetRef);

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, session: &mut CodeSession) {
        let walk = self.draw_state.get().unwrap();

        self.scroll_bars.begin(cx, walk, Layout::default());

        self.viewport_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();

        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        session.handle_changes();
        session.set_wrap_column(Some(
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        ));
        self.start = session.find_first_line_ending_after_y(scroll_pos.y / self.cell_size.y);
        self.end = session.find_first_line_starting_after_y(
            (scroll_pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );

        self.draw_text(cx, session);
        self.draw_selections(cx, session);
        cx.turtle_mut().set_used(
            session.width() * self.cell_size.x,
            session.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if session.update_folds() {
            cx.redraw_all();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, session: &mut CodeSession) {
        session.handle_changes();
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.fold();
                cx.redraw_all();
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.unfold();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_left(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_right(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_up(!shift);
                cx.redraw_all();
            }

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_down(!shift);
                cx.redraw_all();
            }
            Hit::TextInput(TextInputEvent { ref input, .. }) => {
                session.insert(input.into());
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                session.enter();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::RBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.indent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::LBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.outdent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                session.delete();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.backspace();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: false, .. },
                ..
            }) => {
                session.undo();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: true, .. },
                ..
            }) => {
                session.redo();
                cx.redraw_all();
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                cx.set_key_focus(self.scroll_bars.area());
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    if alt {
                        session.add_cursor(cursor, affinity);
                    } else {
                        session.set_cursor(cursor, affinity);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    session.move_to(cursor, affinity);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut y = 0.0;
        session.blocks(
            0,
            session.document().borrow().text().as_lines().len(),
            |blocks| {
                for block in blocks {
                    match block {
                        Block::Line { line, .. } => {
                            self.draw_text.font_scale = line.scale();
                            let mut token_iter = line.tokens().iter().copied();
                            let mut token_slot = token_iter.next();
                            let mut column = 0;
                            for wrapped in line.wrappeds() {
                                match wrapped {
                                    Wrapped::Text {
                                        is_inlay: false,
                                        mut text,
                                    } => {
                                        while !text.is_empty() {
                                            let token = match token_slot {
                                                Some(token) => {
                                                    if text.len() < token.len {
                                                        token_slot = Some(Token {
                                                            len: token.len - text.len(),
                                                            kind: token.kind,
                                                        });
                                                        Token {
                                                            len: text.len(),
                                                            kind: token.kind,
                                                        }
                                                    } else {
                                                        token_slot = token_iter.next();
                                                        token
                                                    }
                                                }
                                                None => Token {
                                                    len: text.len(),
                                                    kind: TokenKind::Unknown,
                                                },
                                            };
                                            let (text_0, text_1) = text.split_at(token.len);
                                            text = text_1;
                                            self.draw_text.color = match token.kind {
                                                TokenKind::Unknown => self.token_colors.unknown,
                                                TokenKind::BranchKeyword => {
                                                    self.token_colors.branch_keyword
                                                }
                                                TokenKind::Identifier => {
                                                    self.token_colors.identifier
                                                }
                                                TokenKind::LoopKeyword => {
                                                    self.token_colors.loop_keyword
                                                }
                                                TokenKind::Number => self.token_colors.number,
                                                TokenKind::OtherKeyword => {
                                                    self.token_colors.other_keyword
                                                }
                                                TokenKind::Punctuator => {
                                                    self.token_colors.punctuator
                                                }
                                                TokenKind::Whitespace => {
                                                    self.token_colors.whitespace
                                                }
                                            };
                                            self.draw_text.draw_abs(
                                                cx,
                                                DVec2 {
                                                    x: line.column_to_x(column),
                                                    y,
                                                } * self.cell_size
                                                    + self.viewport_rect.pos,
                                                text_0,
                                            );
                                            column += text_0
                                                .column_count(session.settings().tab_column_count);
                                        }
                                    }
                                    Wrapped::Text {
                                        is_inlay: true,
                                        text,
                                    } => {
                                        self.draw_text.draw_abs(
                                            cx,
                                            DVec2 {
                                                x: line.column_to_x(column),
                                                y,
                                            } * self.cell_size
                                                + self.viewport_rect.pos,
                                            text,
                                        );
                                        column +=
                                            text.column_count(session.settings().tab_column_count);
                                    }
                                    Wrapped::Widget(widget) => {
                                        column += widget.column_count;
                                    }
                                    Wrapped::Wrap => {
                                        column = line.wrap_indent_column_count();
                                        y += line.scale();
                                    }
                                }
                            }
                            y += line.scale();
                        }
                        Block::Widget(widget) => {
                            y += widget.height;
                        }
                    }
                }
            },
        );
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, session: &CodeSession) {
        let mut active_selection = None;
        let mut selections = session.selections().iter();
        while selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.end().line < self.start)
        {
            selections.next().unwrap();
        }
        if selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.start().line < self.start)
        {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelections {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, session)
    }

    fn pick(&self, session: &CodeSession, point: DVec2) -> Option<(Point, Affinity)> {
        let point = (point - self.viewport_rect.pos) / self.cell_size;
        let mut line = session.find_first_line_ending_after_y(point.y);
        let mut y = session.line(line, |line| line.y());
        session.blocks(line, line + 1, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        let next_byte = byte + grapheme.len();
                                        let next_column = column
                                            + grapheme
                                                .column_count(session.settings().tab_column_count);
                                        let next_y = y + line_ref.scale();
                                        let x = line_ref.column_to_x(column);
                                        let next_x = line_ref.column_to_x(next_column);
                                        let mid_x = (x + next_x) / 2.0;
                                        if (y..=next_y).contains(&point.y) {
                                            if (x..=mid_x).contains(&point.x) {
                                                return Some((
                                                    Point { line, byte },
                                                    Affinity::After,
                                                ));
                                            }
                                            if (mid_x..=next_x).contains(&point.x) {
                                                return Some((
                                                    Point {
                                                        line,
                                                        byte: next_byte,
                                                    },
                                                    Affinity::Before,
                                                ));
                                            }
                                        }
                                        byte = next_byte;
                                        column = next_column;
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    let next_column = column
                                        + text.column_count(session.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    if (y..=next_y).contains(&point.y)
                                        && (x..=next_x).contains(&point.x)
                                    {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = next_column;
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    let next_y = y + line_ref.scale();
                                    if (y..=next_y).contains(&point.y) {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y = next_y;
                                }
                            }
                        }
                        let next_y = y + line_ref.scale();
                        if (y..=y + next_y).contains(&point.y) {
                            return Some((Point { line, byte }, Affinity::After));
                        }
                        line += 1;
                        y = next_y;
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        let next_y = y + line_ref.height();
                        if (y..=next_y).contains(&point.y) {
                            return Some((Point { line, byte: 0 }, Affinity::Before));
                        }
                        y = next_y;
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
            None
        })
    }
}

struct DrawSelections<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelections<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut line = self.code_editor.start;
        let mut y = session.line(line, |line| line.y());
        session.blocks(self.code_editor.start, self.code_editor.end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        self.handle_event(cx, line, line_ref, byte, Affinity::Before, y, column);
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::After,
                                            y,
                                            column,
                                        );
                                        byte += grapheme.len();
                                        column += grapheme
                                            .column_count(session.settings().tab_column_count);
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::Before,
                                            y,
                                            column,
                                        );
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    column +=
                                        text.column_count(session.settings().tab_column_count);
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    if self.active_selection.is_some() {
                                        self.draw_selection(cx, line_ref, y, column);
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y += line_ref.scale();
                                }
                            }
                        }
                        self.handle_event(cx, line, line_ref, byte, Affinity::After, y, column);
                        column += 1;
                        if self.active_selection.is_some() {
                            self.draw_selection(cx, line_ref, y, column);
                        }
                        line += 1;
                        y += line_ref.scale();
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        y += line_ref.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line: usize,
        line_ref: Line<'_>,
        byte: usize,
        affinity: Affinity,
        y: f64,
        column: usize,
    ) {
        let point = Point { line, byte };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == point && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line_ref, y, column);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == point && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: line_ref.column_to_x(column),
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, line: Line<'_>, y: f64, column: usize) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: line.column_to_x(column) - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column: usize) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: line.column_to_x(column),
                    y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    whitespace: Vec4,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
            Self {
                line_count: 0,
                byte_count: self.byte_count - other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count - other.line_count,
                byte_count: self.byte_count,
            }
        }
    }
}

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
use crate::{state::SessionId, Change, Selection, Text};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    current_edit: Option<(SessionId, EditKind)>,
    undos: Vec<(Vec<Selection>, Vec<Change>)>,
    redos: Vec<(Vec<Selection>, Vec<Change>)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit = None;
    }

    pub fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        inverted_changes: Vec<Change>,
    ) {
        if self
            .current_edit
            .map_or(false, |current_edit| current_edit == (origin_id, kind))
        {
            self.undos.last_mut().unwrap().1.extend(inverted_changes);
        } else {
            self.current_edit = Some((origin_id, kind));
            self.undos.push((selections.to_vec(), inverted_changes));
        }
        self.redos.clear();
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, mut inverted_changes)) = self.undos.pop() {
            self.current_edit = None;
            let mut changes = Vec::new();
            inverted_changes.reverse();
            for inverted_change in inverted_changes.iter().cloned() {
                let change = inverted_change.clone().invert(&text);
                text.apply_change(inverted_change);
                changes.push(change);
            }
            self.redos.push((selections.clone(), changes.clone()));
            Some((selections, inverted_changes))
        } else {
            None
        }
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, changes)) = self.redos.pop() {
            self.current_edit = None;
            for change in changes.iter().cloned() {
                text.apply_change(change);
            }
            self.undos.push((selections.clone(), changes.clone()));
            Some((selections, changes))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EditGroup {
    pub selections: Vec<Selection>,
    pub changes: Vec<Change>,
}
use crate::widgets::{BlockWidget, InlineWidget};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Widget(BlockWidget),
}
pub trait IteratorExt: Iterator {
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>;
}

impl<T> IteratorExt for T
where
    T: Iterator,
{
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>,
    {
        Merge {
            prev_item: None,
            iter: self,
            f,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Merge<I, F>
where
    I: Iterator,
{
    prev_item: Option<I::Item>,
    iter: I,
    f: F,
}

impl<I, F> Iterator for Merge<I, F>
where
    I: Iterator,
    F: FnMut(I::Item, I::Item) -> Result<I::Item, (I::Item, I::Item)>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev_item.take(), self.iter.next()) {
                (Some(prev_item), Some(item)) => match (self.f)(prev_item, item) {
                    Ok(merged_item) => {
                        self.prev_item = Some(merged_item);
                        continue;
                    }
                    Err((prev_item, item)) => {
                        self.prev_item = Some(item);
                        break Some(prev_item);
                    }
                },
                (None, Some(item)) => {
                    self.prev_item = Some(item);
                    continue;
                }
                (Some(prev_item), None) => break Some(prev_item),
                (None, None) => break None,
            }
        }
    }
}
pub use makepad_widgets;
use makepad_widgets::*;

pub mod change;
pub mod char;
pub mod code_editor;
pub mod extent;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod line;
pub mod move_ops;
pub mod point;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    change::Change,
    code_editor::CodeEditor,
    extent::Extent,
    history::History,
    line::Line,
    point::Point,
    range::Range,
    selection::Selection,
    settings::Settings,
    state::{CodeDocument, CodeSession},
    text::Text,
    token::Token,
    tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
use {
    crate::{
        inlays::InlineInlay, selection::Affinity, str::StrExt, widgets::InlineWidget,
        wrap::WrapData, Token,
    },
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_data.unwrap().wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn byte_and_affinity_to_row_and_column(
        &self,
        byte: usize,
        affinity: Affinity,
        tab_column_count: usize,
    ) -> (usize, usize) {
        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if current_byte == byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte == byte && affinity == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if current_byte == byte && affinity == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    column += text.column_count(tab_column_count);
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    row += 1;
                    column = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte == byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_and_column_to_byte_and_affinity(
        &self,
        row: usize,
        column: usize,
        tab_width: usize,
    ) -> (usize, Affinity) {
        let mut current_row = 0;
        let mut current_column = 0;
        let mut byte = 0;
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_width);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte += grapheme.len();
                        current_column = next_column;
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column + text.column_count(tab_width);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                Wrapped::Widget(widget) => {
                    current_column += widget.column_count;
                }
                Wrapped::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.wrap_indent_column_count();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn wrap_indent_column_count(self) -> usize {
        self.wrap_data.unwrap().indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            position: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) position: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.position);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.position += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) position: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    mid = mid.min(position - self.position);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.position += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.position += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
mod app;

fn main() {
    app::app_main();
}
use crate::{selection::Affinity, str::StrExt, Point, CodeSession};

pub fn move_left(lines: &[String], point: Point) -> Point {
    if !is_at_start_of_line(point) {
        return move_to_prev_grapheme(lines, point);
    }
    if !is_at_first_line(point) {
        return move_to_end_of_prev_line(lines, point);
    }
    point
}

pub fn move_right(lines: &[String], point: Point) -> Point {
    if !is_at_end_of_line(lines, point) {
        return move_to_next_grapheme(lines, point);
    }
    if !is_at_last_line(lines, point) {
        return move_to_start_of_next_line(point);
    }
    point
}

pub fn move_up(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_first_row_of_line(session, point, affinity) {
        return move_to_prev_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_first_line(point) {
        return move_to_last_row_of_prev_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

pub fn move_down(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_last_row_of_line(session, point, affinity) {
        return move_to_next_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_last_line(session.document().borrow().text().as_lines(), point) {
        return move_to_first_row_of_next_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

fn is_at_first_line(point: Point) -> bool {
    point.line == 0
}

fn is_at_last_line(lines: &[String], point: Point) -> bool {
    point.line == lines.len()
}

fn is_at_start_of_line(point: Point) -> bool {
    point.byte == 0
}

fn is_at_end_of_line(lines: &[String], point: Point) -> bool {
    point.byte == lines[point.line].len()
}

fn is_at_first_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(lines: &[String], point: Point) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], point: Point) -> Point {
    let line = &lines[point.line];
    Point {
        line: point.line,
        byte: line[point.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| point.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], point: Point) -> Point {
    let prev_line = point.line - 1;
    Point {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(point: Point) -> Point {
    Point {
        line: point.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_next_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row + 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_last_row_of_prev_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line - 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}

fn move_to_first_row_of_next_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line + 1, |next_line| {
            let (byte, affinity) = next_line.row_and_column_to_byte_and_affinity(
                0,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line + 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}
use {
    crate::{
        change::{ChangeKind, Drift},
        Change, Extent,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.extent() + (self - point),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Extent, Point};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}
use {
    crate::{Change, Extent, Point, Range},
    std::ops,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Point {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Point {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Extent {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line..self.cursor.line + 1
        } else {
            self.cursor.line..if self.anchor.byte == 0 {
                self.anchor.line
            } else {
                self.anchor.line + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Point, Affinity, Option<usize>) -> (Point, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change),
            cursor: self.cursor.apply_change(change),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub use_soft_tabs: bool,
    pub tab_column_count: usize,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_soft_tabs: true,
            tab_column_count: 4,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
use {
    crate::{
        change::{ChangeKind, Drift},
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::Affinity,
        str::StrExt,
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        Change, Extent, History, Line, Point, Range, Selection, Settings, Text, Token, Tokenizer,
    },
    std::{
        cell::RefCell,
        cmp,
        collections::{HashMap, HashSet},
        iter, mem,
        rc::Rc,
        slice::Iter,
        sync::{
            atomic,
            atomic::AtomicUsize,
            mpsc,
            mpsc::{Receiver, Sender},
        },
    },
};

#[derive(Debug)]
pub struct CodeSession {
    id: SessionId,
    settings: Rc<Settings>,
    document: Rc<RefCell<CodeDocument>>,
    wrap_column: Option<usize>,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
    change_receiver: Receiver<(Option<Vec<Selection>>, Vec<Change>)>,
}

impl CodeSession {
    pub fn new(document: Rc<RefCell<CodeDocument>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (change_sender, change_receiver) = mpsc::channel();
        let line_count = document.borrow().text.as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wrap_data: (0..line_count).map(|_| None).collect(),
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
            change_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .change_senders
            .insert(session.id, change_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().text.as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().text.as_lines().len() - 1;
        let mut y = self.line(index, |line| line.y() + line.height());
        self.blocks(index, index, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: true,
                        line,
                    } => y += line.height(),
                    Block::Widget(widget) => y += widget.height,
                    _ => unreachable!(),
                }
            }
        });
        y
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Rc<RefCell<CodeDocument>> {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document.borrow();
        f(Line {
            y: self.y.get(line).copied(),
            column_count: self.column_count[line],
            fold_column: self.fold_column[line],
            scale: self.scale[line],
            text: &document.text.as_lines()[line],
            tokens: &document.tokens[line],
            inline_inlays: &document.inline_inlays[line],
            wrap_data: self.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        f(Lines {
            y: self.y[start_line.min(self.y.len())..end_line.min(self.y.len())].iter(),
            column_count: self.column_count[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
            text: document.text.as_lines()[start_line..end_line].iter(),
            tokens: document.tokens[start_line..end_line].iter(),
            inline_inlays: document.inline_inlays[start_line..end_line].iter(),
            wrap_data: self.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        let mut block_inlays = document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < start_line)
        {
            block_inlays.next();
        }
        self.lines(start_line, end_line, |lines| {
            f(Blocks {
                lines,
                block_inlays,
                position: start_line,
            })
        })
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .indentation()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.fold_column[line] =
                    self.settings.fold_level * self.settings.indent_column_count;
                self.unfolding_lines.remove(&line);
                self.folding_lines.insert(line);
            }
        }
    }

    pub fn unfold(&mut self) {
        for line in self.folding_lines.drain() {
            self.unfolding_lines.insert(line);
        }
        for line in self.folded_lines.drain() {
            self.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&mut self) -> bool {
        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let mut new_folding_lines = HashSet::new();
        for &line in &self.folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.1 + 0.001 {
                self.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, cursor: Point, affinity: Affinity) {
        self.selections.clear();
        self.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        });
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn add_cursor(&mut self, cursor: Point, affinity: Affinity) {
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        };
        self.pending_selection_index = Some(
            match self.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return cmp::Ordering::Less;
                }
                if selection.start() >= cursor {
                    return cmp::Ordering::Greater;
                }
                cmp::Ordering::Equal
            }) {
                Ok(index) => {
                    self.selections[index] = selection;
                    index
                }
                Err(index) => {
                    self.selections.insert(index, selection);
                    index
                }
            },
        );
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, cursor: Point, affinity: Affinity) {
        let mut pending_selection_index = self.pending_selection_index.unwrap();
        self.selections[pending_selection_index] = Selection {
            cursor,
            affinity,
            ..self.selections[pending_selection_index]
        };
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !self.selections[prev_selection_index]
                .should_merge(self.selections[pending_selection_index])
            {
                break;
            }
            self.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < self.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !self.selections[pending_selection_index]
                .should_merge(self.selections[next_selection_index])
            {
                break;
            }
            self.selections.remove(next_selection_index);
        }
        self.pending_selection_index = Some(pending_selection_index);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_left(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_right(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_up(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_down(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn insert(&mut self, text: Text) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Insert, &self.selections, |_, _, _| {
                (Extent::zero(), Some(text.clone()), None)
            });
    }

    pub fn enter(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Insert,
            &self.selections,
            |line, index, _| {
                (
                    if line[..index].chars().all(|char| char.is_whitespace()) {
                        Extent {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Extent::zero()
                    },
                    Some(Text::newline()),
                    if line[..index]
                        .chars()
                        .rev()
                        .find_map(|char| {
                            if char.is_opening_delimiter() {
                                return Some(true);
                            }
                            if char.is_closing_delimiter() {
                                return Some(false);
                            }
                            None
                        })
                        .unwrap_or(false)
                        && line[index..]
                            .chars()
                            .find_map(|char| {
                                if char.is_closing_delimiter() {
                                    return Some(true);
                                }
                                if !char.is_whitespace() {
                                    return Some(false);
                                }
                                None
                            })
                            .unwrap_or(false)
                    {
                        Some(Text::newline())
                    } else {
                        None
                    },
                )
            },
        );
    }

    pub fn indent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Indent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        (indentation_column_count + self.settings.indent_column_count)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Outdent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        indentation_column_count.saturating_sub(1)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn delete(&mut self) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Delete, &self.selections, |_, _, _| {
                (Extent::zero(), None, None)
            });
    }

    pub fn backspace(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            |line, index, is_empty| {
                (
                    if is_empty {
                        if index == 0 {
                            Extent {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Extent {
                                line_count: 0,
                                byte_count: line.graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Extent::zero()
                    },
                    None,
                    None,
                )
            },
        );
    }

    pub fn undo(&mut self) {
        self.document.borrow_mut().undo(self.id);
    }

    pub fn redo(&mut self) {
        self.document.borrow_mut().redo(self.id);
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().text.as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.y);
        self.blocks(start, end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line { is_inlay, line } => {
                        if !is_inlay {
                            ys.push(y);
                        }
                        y += line.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        ys.push(y);
        self.y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, changes)) = self.change_receiver.try_recv() {
            self.apply_changes(selections, &changes);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        self.line(index, |line| {
            for wrapped in line.wrappeds() {
                match wrapped {
                    Wrapped::Text { text, .. } => {
                        column += text
                            .column_count(self.settings.tab_column_count);
                    }
                    Wrapped::Widget(widget) => {
                        column += widget.column_count;
                    }
                    Wrapped::Wrap => {
                        column_count = column_count.max(column);
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        });
        self.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.wrap_data[line] = Some(wrap_data);
        self.y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(&CodeSession, Selection) -> Selection,
    ) {
        let mut selections = mem::take(&mut self.selections);
        for selection in &mut selections {
            *selection = f(&self, *selection);
            if reset_anchor {
                *selection = selection.reset_anchor();
            }
        }
        self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
                self.selections[current_selection_index] = merged_selection;
                self.selections.remove(next_selection_index);
                if let Some(pending_selection_index) = self.pending_selection_index.as_mut() {
                    if next_selection_index < *pending_selection_index {
                        *pending_selection_index -= 1;
                    }
                }
            } else {
                current_selection_index += 1;
            }
        }
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_changes(&mut self, selections: Option<Vec<Selection>>, changes: &[Change]) {
        for change in changes {
            match &change.kind {
                ChangeKind::Insert(point, text) => {
                    self.column_count[point.line] = None;
                    self.wrap_data[point.line] = None;
                    let line_count = text.extent().line_count;
                    if line_count > 0 {
                        let line = point.line + 1;
                        self.y.truncate(line);
                        self.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                ChangeKind::Delete(range) => {
                    self.column_count[range.start().line] = None;
                    self.wrap_data[range.start().line] = None;
                    let line_count = range.extent().line_count;
                    if line_count > 0 {
                        let start_line = range.start().line + 1;
                        let end_line = start_line + line_count;
                        self.y.truncate(start_line);
                        self.column_count.drain(start_line..end_line);
                        self.fold_column.drain(start_line..end_line);
                        self.scale.drain(start_line..end_line);
                        self.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for change in changes {
                for selection in &mut self.selections {
                    *selection = selection.apply_change(&change);
                }
            }
        }
        self.update_y();
    }
}

impl Drop for CodeSession {
    fn drop(&mut self) {
        self.document.borrow_mut().change_senders.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_data: Iter<'a, Option<WrapData>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct CodeDocument {
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    history: History,
    tokenizer: Tokenizer,
    change_senders: HashMap<SessionId, Sender<(Option<Vec<Selection>>, Vec<Change>)>>,
}

impl CodeDocument {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            text,
            tokens,
            inline_inlays: (0..line_count)
                .map(|line| {
                    if line % 5 == 0 {
                        [
                            (20, InlineInlay::Text("XXX".into())),
                            (40, InlineInlay::Text("XXX".into())),
                            (60, InlineInlay::Text("XXX".into())),
                            (80, InlineInlay::Text("XXX".into())),
                        ]
                        .into()
                    } else {
                        Vec::new()
                    }
                })
                .collect(),
            block_inlays: Vec::new(),
            history: History::new(),
            tokenizer: Tokenizer::new(line_count),
            change_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.text, &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&String, usize, bool) -> (Extent, Option<Text>, Option<Text>),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        let mut point = Point::zero();
        let mut prev_range_end = Point::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, range.extent())),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            let (delete_extent, insert_text_before, insert_text_after) = f(
                &self.text.as_lines()[point.line],
                point.byte,
                range.is_empty(),
            );
            if delete_extent != Extent::zero() {
                if delete_extent.line_count == 0 {
                    point.byte -= delete_extent.byte_count;
                } else {
                    point.line -= delete_extent.line_count;
                    point.byte = self.text.as_lines()[point.line].len() - delete_extent.byte_count;
                }
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, delete_extent)),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_before) = insert_text_before {
                let extent = insert_text_before.extent();
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Insert(point, insert_text_before),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_after) = insert_text_after {
                let extent = insert_text_after.extent();
                let change = Change {
                    drift: Drift::After,
                    kind: ChangeKind::Insert(point, insert_text_after),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            prev_range_end = range.end();
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut changes, &mut inverted_changes, &mut f);
            }
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.text.as_lines()[line]);
        if delete_byte_count > 0 {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Delete(Range::from_start_and_extent(
                    Point { line, byte },
                    Extent {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                )),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
        if !insert_text.is_empty() {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Insert(Point { line, byte }, insert_text.into()),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.undo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn redo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.redo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn apply_changes(
        &mut self,
        origin_id: SessionId,
        selections: Option<Vec<Selection>>,
        changes: &[Change],
    ) {
        for change in changes {
            self.apply_change_to_tokens(change);
            self.apply_change_to_inline_inlays(change);
            self.tokenizer.apply_change(change);
        }
        self.tokenizer.update(&self.text, &mut self.tokens);
        for (&session_id, change_sender) in &self.change_senders {
            if session_id == origin_id {
                change_sender
                    .send((selections.clone(), changes.to_vec()))
                    .unwrap();
            } else {
                change_sender
                    .send((
                        None,
                        changes
                            .iter()
                            .cloned()
                            .map(|change| Change {
                                drift: Drift::Before,
                                ..change
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line].len());
                if byte != point.byte {
                    let token = self.tokens[point.line][index];
                    let mid = point.byte - byte;
                    self.tokens[point.line][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.extent().line_count == 0 {
                    self.tokens[point.line]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][index..].iter().copied());
                    self.tokens.splice(point.line..point.line + 1, tokens);
                }
            }
            ChangeKind::Delete(range) => {
                let mut byte = 0;
                let mut start = self.tokens[range.start().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.start().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.start().line].len());
                if byte != range.start().byte {
                    let token = self.tokens[range.start().line][start];
                    let mid = range.start().byte - byte;
                    self.tokens[range.start().line][start] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start += 1;
                    self.tokens[range.start().line].insert(
                        start,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end = self.tokens[range.end().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.end().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.end().line].len());
                if byte != range.end().byte {
                    let token = self.tokens[range.end().line][end];
                    let mid = range.end().byte - byte;
                    self.tokens[range.end().line][end] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end += 1;
                    self.tokens[range.end().line].insert(
                        end,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if range.start().line == range.end().line {
                    self.tokens[range.start().line].drain(start..end);
                } else {
                    let mut tokens = self.tokens[range.start().line][..start]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[range.end().line][end..].iter().copied());
                    self.tokens
                        .splice(range.start().line..range.end().line + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.extent().byte_count, inline_inlay)
                            }),
                    );
                    self.inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = self.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(self.inline_inlays[range.start().line].len());
                let end = self.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(self.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    self.inline_inlays[range.start().line].drain(start..end);
                    for (byte, _) in &mut self.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[range.start().line]
                        .drain(..start)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(self.inline_inlays[range.end().line].drain(end..).map(
                        |(byte, inline_inlay)| {
                            (
                                range.start().byte + byte - range.end().byte.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    self.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}

fn reindent(
    string: &str,
    use_soft_tabs: bool,
    tab_column_count: usize,
    f: impl FnOnce(usize) -> usize,
) -> (usize, usize, String) {
    let indentation = string.indentation().unwrap_or("");
    let indentation_column_count = indentation.column_count(tab_column_count);
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(
        new_indentation_column_count,
        use_soft_tabs,
        tab_column_count,
    );
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize, use_soft_tabs: bool, tab_column_count: usize) -> String {
    let tab_count;
    let space_count;
    if use_soft_tabs {
        tab_count = 0;
        space_count = column_count;
    } else {
        tab_count = column_count / tab_column_count;
        space_count = column_count % tab_column_count;
    }
    let tabs = iter::repeat("\t").take(tab_count);
    let spaces = iter::repeat(" ").take(space_count);
    tabs.chain(spaces).collect()
}
use crate::char::CharExt;

pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indentation(&self) -> Option<&str>;
    fn longest_common_prefix(&self, other: &str) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indentation(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn longest_common_prefix(&self, other: &str) -> &str {
        &self[..self
            .char_indices()
            .zip(other.chars())
            .find(|((_, char_0), char_1)| char_0 == char_1)
            .map(|((index, _), _)| index)
            .unwrap_or_else(|| self.len().min(other.len()))]
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
    }

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices {
            graphemes: self.graphemes(),
            start: self.as_ptr() as usize,
        }
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    string: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut end = 1;
        while !self.string.is_char_boundary(end) {
            end += 1;
        }
        let (grapheme, string) = self.string.split_at(end);
        self.string = string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut start = self.string.len() - 1;
        while !self.string.is_char_boundary(start) {
            start -= 1;
        }
        let (string, grapheme) = self.string.split_at(start);
        self.string = string;
        Some(grapheme)
    }
}

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
    start: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
use {
    crate::{change, Change, Extent, Point, Range},
    std::{io, io::BufRead, iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn newline() -> Self {
        Self {
            lines: vec![String::new(), String::new()],
        }
    }

    pub fn from_buf_reader<R>(reader: R) -> io::Result<Self>
    where
        R: BufRead,
    {
        Ok(Self {
            lines: reader.lines().collect::<Result<_, _>>()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.extent() == Extent::zero()
    }

    pub fn extent(&self) -> Extent {
        Extent {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line][range.start().byte..range.end().byte].to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines.push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
        }
        Text { lines }
    }

    pub fn insert(&mut self, point: Point, mut text: Self) {
        if text.extent().line_count == 0 {
            self.lines[point.line]
                .replace_range(point.byte..point.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line][..point.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line][point.byte..]);
            self.lines.splice(point.line..point.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, range: Range) {
        if range.start().line == range.end().line {
            self.lines[range.start().line].replace_range(range.start().byte..range.end().byte, "");
        } else {
            let mut line = self.lines[range.start().line][..range.start().byte].to_string();
            line.push_str(&self.lines[range.end().line][range.end().byte..]);
            self.lines
                .splice(range.start().line..range.end().line + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change.kind {
            change::ChangeKind::Insert(point, additional_text) => {
                self.insert(point, additional_text)
            }
            change::ChangeKind::Delete(range) => self.delete(range),
        }
    }

    pub fn into_line_count(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        Self {
            lines: string.lines().map(|string| string.to_owned()).collect(),
        }
    }
}

impl From<&String> for Text {
    fn from(string: &String) -> Self {
        string.as_str().into()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Whitespace,
}
use crate::{change::ChangeKind, token::TokenKind, Change, Text, Token};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
}

impl Tokenizer {
    pub fn new(line_count: usize) -> Self {
        Self {
            state: (0..line_count).map(|_| None).collect(),
        }
    }

    pub fn apply_change(&mut self, change: &Change) {
        match &change.kind {
            ChangeKind::Insert(point, text) => {
                self.state[point.line] = None;
                let line_count = text.extent().line_count;
                if line_count > 0 {
                    let line = point.line + 1;
                    self.state.splice(line..line, (0..line_count).map(|_| None));
                }
            }
            ChangeKind::Delete(range) => {
                self.state[range.start().line] = None;
                let line_count = range.extent().line_count;
                if line_count > 0 {
                    let start_line = range.start().line + 1;
                    let end_line = start_line + line_count;
                    self.state.drain(start_line..end_line);
                }
            }
        }
    }

    pub fn update(&mut self, text: &Text, tokens: &mut [Vec<Token>]) {
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut new_tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => new_tokens.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    tokens[line] = new_tokens;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<Token>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('!', '=', _)
            | ('%', '=', _)
            | ('&', '&', _)
            | ('&', '=', _)
            | ('*', '=', _)
            | ('+', '=', _)
            | ('-', '=', _)
            | ('-', '>', _)
            | ('.', '.', _)
            | ('/', '=', _)
            | (':', ':', _)
            | ('<', '<', _)
            | ('<', '=', _)
            | ('=', '=', _)
            | ('=', '>', _)
            | ('>', '=', _)
            | ('>', '>', _)
            | ('^', '=', _)
            | ('|', '=', _)
            | ('|', '|', _) => {
                cursor.skip(2);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            ('.', char, _) if char.is_digit(10) => self.number(cursor),
            ('!', _, _)
            | ('#', _, _)
            | ('$', _, _)
            | ('%', _, _)
            | ('&', _, _)
            | ('*', _, _)
            | ('+', _, _)
            | (',', _, _)
            | ('-', _, _)
            | ('.', _, _)
            | ('/', _, _)
            | (':', _, _)
            | (';', _, _)
            | ('<', _, _)
            | ('=', _, _)
            | ('>', _, _)
            | ('?', _, _)
            | ('@', _, _)
            | ('^', _, _)
            | ('_', _, _)
            | ('|', _, _) => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            (char, _, _) if char.is_identifier_start() => self.identifier_or_keyword(cursor),
            (char, _, _) if char.is_digit(10) => self.number(cursor),
            (char, _, _) if char.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index;
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_identifier_continue()) {}
        let end = cursor.index;

        (
            State::Initial(InitialState),
            match &cursor.string[start..end] {
                "else" | "if" | "match" | "return" => TokenKind::BranchKeyword,
                "break" | "continue" | "for" | "loop" | "while" => TokenKind::LoopKeyword,
                "Self" | "as" | "async" | "await" | "const" | "crate" | "dyn" | "enum"
                | "extern" | "false" | "fn" | "impl" | "in" | "let" | "mod" | "move" | "mut"
                | "pub" | "ref" | "self" | "static" | "struct" | "super" | "trait" | "true"
                | "type" | "unsafe" | "use" | "where" => TokenKind::OtherKeyword,
                _ => TokenKind::Identifier,
            },
        )
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    _ => {
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                }
            }
        };
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    string: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Cursor { string, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.string[self.index..].chars().nth(index).unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index = self.string[self.index..]
            .char_indices()
            .nth(count)
            .map_or(self.string.len(), |(index, _)| self.index + index);
    }

    fn skip_if<P>(&mut self, predicate: P) -> bool
    where
        P: FnOnce(char) -> bool,
    {
        if predicate(self.peek(0)) {
            self.skip(1);
            true
        } else {
            false
        }
    }

    fn skip_exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(0) == '-' {
            self.skip(1);
        }
        self.skip_digits(10)
    }

    fn skip_digits(&mut self, radix: u32) -> bool {
        let mut has_skip_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                char if char.is_digit(radix) => {
                    self.skip(1);
                    has_skip_digits = true;
                }
                _ => break,
            }
        }
        has_skip_digits
    }

    fn skip_suffix(&mut self) -> bool {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if(|char| char.is_identifier_continue()) {}
            return true;
        }
        false
    }
}

pub trait CharExt {
    fn is_identifier_start(self) -> bool;
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub height: f64,
}
use crate::{char::CharExt, line::Inline, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize, tab_column_count: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .indentation()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count(tab_column_count))
        .sum();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            Inline::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte = 0;
    let mut column = 0;
    let mut wraps = Vec::new();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if column + column_count > wrap_column {
                        column = indent_column_count;
                        wraps.push(byte);
                    }
                    column += column_count;
                    byte += string.len();
                }
            }
            Inline::Widget(widget) => {
                if column + widget.column_count > wrap_column {
                    column = indent_column_count;
                    wraps.push(byte);
                }
                column += widget.column_count;
                byte += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}
use {
    makepad_code_editor::{
        code_editor::*,
        state::{CodeDocument, CodeSession},
    },
    makepad_widgets::*,
    std::{cell::RefCell, rc::Rc},
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_code_editor::code_editor::CodeEditor;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <CodeEditor> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    code_editor.draw(&mut cx, &mut self.state.session);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        if let Some(mut code_editor) = self.ui.get_code_editor(id!(code_editor)).borrow_mut() {
            code_editor.handle_event(cx, event, &mut self.state.session);
        }
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_code_editor::code_editor::live_design(cx);
    }
}

struct State {
    session: CodeSession,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: CodeSession::new(Rc::new(RefCell::new(CodeDocument::new(
                include_str!("state.rs").into(),
            )))),
        }
    }
}

app_main!(App);
use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => {
                    ChangeKind::Insert(range.start(), text.slice(range))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{
        line::Wrapped,
        selection::Affinity,
        state::{Block, CodeSession},
        str::StrExt,
        token::TokenKind,
        Line, Point, Selection, Token,
    },
    makepad_widgets::*,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    TokenColors = {{TokenColors}} {
        unknown: #808080,
        branch_keyword: #C485BE,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        whitespace: #6E6E6E,
    }

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        
            width: Fill,
            height: Fill,
            margin: 0,
        
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
    #[live]
    draw_text: DrawText,
    #[live]
    token_colors: TokenColors,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start: usize,
    #[rust]
    end: usize,
}

impl LiveHook for CodeEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, CodeEditor)
    }
}

impl Widget for CodeEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }

    fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        //let uid = self.widget_uid();
        /*self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });*/
        //self.handle_event
    }

    fn walk(&self) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CodeEditorRef(WidgetRef);

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, session: &mut CodeSession) {
        let walk = self.draw_state.get().unwrap();

        self.scroll_bars.begin(cx, walk, Layout::default());

        self.viewport_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();

        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        session.handle_changes();
        session.set_wrap_column(Some(
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        ));
        self.start = session.find_first_line_ending_after_y(scroll_pos.y / self.cell_size.y);
        self.end = session.find_first_line_starting_after_y(
            (scroll_pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );

        self.draw_text(cx, session);
        self.draw_selections(cx, session);
        cx.turtle_mut().set_used(
            session.width() * self.cell_size.x,
            session.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if session.update_folds() {
            cx.redraw_all();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, session: &mut CodeSession) {
        session.handle_changes();
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.fold();
                cx.redraw_all();
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.unfold();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_left(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_right(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_up(!shift);
                cx.redraw_all();
            }

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_down(!shift);
                cx.redraw_all();
            }
            Hit::TextInput(TextInputEvent { ref input, .. }) => {
                session.insert(input.into());
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                session.enter();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::RBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.indent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::LBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.outdent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                session.delete();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.backspace();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: false, .. },
                ..
            }) => {
                session.undo();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: true, .. },
                ..
            }) => {
                session.redo();
                cx.redraw_all();
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                cx.set_key_focus(self.scroll_bars.area());
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    if alt {
                        session.add_cursor(cursor, affinity);
                    } else {
                        session.set_cursor(cursor, affinity);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    session.move_to(cursor, affinity);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut y = 0.0;
        session.blocks(
            0,
            session.document().borrow().text().as_lines().len(),
            |blocks| {
                for block in blocks {
                    match block {
                        Block::Line { line, .. } => {
                            self.draw_text.font_scale = line.scale();
                            let mut token_iter = line.tokens().iter().copied();
                            let mut token_slot = token_iter.next();
                            let mut column = 0;
                            for wrapped in line.wrappeds() {
                                match wrapped {
                                    Wrapped::Text {
                                        is_inlay: false,
                                        mut text,
                                    } => {
                                        while !text.is_empty() {
                                            let token = match token_slot {
                                                Some(token) => {
                                                    if text.len() < token.len {
                                                        token_slot = Some(Token {
                                                            len: token.len - text.len(),
                                                            kind: token.kind,
                                                        });
                                                        Token {
                                                            len: text.len(),
                                                            kind: token.kind,
                                                        }
                                                    } else {
                                                        token_slot = token_iter.next();
                                                        token
                                                    }
                                                }
                                                None => Token {
                                                    len: text.len(),
                                                    kind: TokenKind::Unknown,
                                                },
                                            };
                                            let (text_0, text_1) = text.split_at(token.len);
                                            text = text_1;
                                            self.draw_text.color = match token.kind {
                                                TokenKind::Unknown => self.token_colors.unknown,
                                                TokenKind::BranchKeyword => {
                                                    self.token_colors.branch_keyword
                                                }
                                                TokenKind::Identifier => {
                                                    self.token_colors.identifier
                                                }
                                                TokenKind::LoopKeyword => {
                                                    self.token_colors.loop_keyword
                                                }
                                                TokenKind::Number => self.token_colors.number,
                                                TokenKind::OtherKeyword => {
                                                    self.token_colors.other_keyword
                                                }
                                                TokenKind::Punctuator => {
                                                    self.token_colors.punctuator
                                                }
                                                TokenKind::Whitespace => {
                                                    self.token_colors.whitespace
                                                }
                                            };
                                            self.draw_text.draw_abs(
                                                cx,
                                                DVec2 {
                                                    x: line.column_to_x(column),
                                                    y,
                                                } * self.cell_size
                                                    + self.viewport_rect.pos,
                                                text_0,
                                            );
                                            column += text_0
                                                .column_count(session.settings().tab_column_count);
                                        }
                                    }
                                    Wrapped::Text {
                                        is_inlay: true,
                                        text,
                                    } => {
                                        self.draw_text.draw_abs(
                                            cx,
                                            DVec2 {
                                                x: line.column_to_x(column),
                                                y,
                                            } * self.cell_size
                                                + self.viewport_rect.pos,
                                            text,
                                        );
                                        column +=
                                            text.column_count(session.settings().tab_column_count);
                                    }
                                    Wrapped::Widget(widget) => {
                                        column += widget.column_count;
                                    }
                                    Wrapped::Wrap => {
                                        column = line.wrap_indent_column_count();
                                        y += line.scale();
                                    }
                                }
                            }
                            y += line.scale();
                        }
                        Block::Widget(widget) => {
                            y += widget.height;
                        }
                    }
                }
            },
        );
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, session: &CodeSession) {
        let mut active_selection = None;
        let mut selections = session.selections().iter();
        while selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.end().line < self.start)
        {
            selections.next().unwrap();
        }
        if selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.start().line < self.start)
        {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelections {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, session)
    }

    fn pick(&self, session: &CodeSession, point: DVec2) -> Option<(Point, Affinity)> {
        let point = (point - self.viewport_rect.pos) / self.cell_size;
        let mut line = session.find_first_line_ending_after_y(point.y);
        let mut y = session.line(line, |line| line.y());
        session.blocks(line, line + 1, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        let next_byte = byte + grapheme.len();
                                        let next_column = column
                                            + grapheme
                                                .column_count(session.settings().tab_column_count);
                                        let next_y = y + line_ref.scale();
                                        let x = line_ref.column_to_x(column);
                                        let next_x = line_ref.column_to_x(next_column);
                                        let mid_x = (x + next_x) / 2.0;
                                        if (y..=next_y).contains(&point.y) {
                                            if (x..=mid_x).contains(&point.x) {
                                                return Some((
                                                    Point { line, byte },
                                                    Affinity::After,
                                                ));
                                            }
                                            if (mid_x..=next_x).contains(&point.x) {
                                                return Some((
                                                    Point {
                                                        line,
                                                        byte: next_byte,
                                                    },
                                                    Affinity::Before,
                                                ));
                                            }
                                        }
                                        byte = next_byte;
                                        column = next_column;
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    let next_column = column
                                        + text.column_count(session.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    if (y..=next_y).contains(&point.y)
                                        && (x..=next_x).contains(&point.x)
                                    {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = next_column;
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    let next_y = y + line_ref.scale();
                                    if (y..=next_y).contains(&point.y) {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y = next_y;
                                }
                            }
                        }
                        let next_y = y + line_ref.scale();
                        if (y..=y + next_y).contains(&point.y) {
                            return Some((Point { line, byte }, Affinity::After));
                        }
                        line += 1;
                        y = next_y;
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        let next_y = y + line_ref.height();
                        if (y..=next_y).contains(&point.y) {
                            return Some((Point { line, byte: 0 }, Affinity::Before));
                        }
                        y = next_y;
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
            None
        })
    }
}

struct DrawSelections<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelections<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut line = self.code_editor.start;
        let mut y = session.line(line, |line| line.y());
        session.blocks(self.code_editor.start, self.code_editor.end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        self.handle_event(cx, line, line_ref, byte, Affinity::Before, y, column);
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::After,
                                            y,
                                            column,
                                        );
                                        byte += grapheme.len();
                                        column += grapheme
                                            .column_count(session.settings().tab_column_count);
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::Before,
                                            y,
                                            column,
                                        );
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    column +=
                                        text.column_count(session.settings().tab_column_count);
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    if self.active_selection.is_some() {
                                        self.draw_selection(cx, line_ref, y, column);
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y += line_ref.scale();
                                }
                            }
                        }
                        self.handle_event(cx, line, line_ref, byte, Affinity::After, y, column);
                        column += 1;
                        if self.active_selection.is_some() {
                            self.draw_selection(cx, line_ref, y, column);
                        }
                        line += 1;
                        y += line_ref.scale();
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        y += line_ref.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line: usize,
        line_ref: Line<'_>,
        byte: usize,
        affinity: Affinity,
        y: f64,
        column: usize,
    ) {
        let point = Point { line, byte };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == point && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line_ref, y, column);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == point && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: line_ref.column_to_x(column),
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, line: Line<'_>, y: f64, column: usize) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: line.column_to_x(column) - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column: usize) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: line.column_to_x(column),
                    y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    whitespace: Vec4,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
            Self {
                line_count: 0,
                byte_count: self.byte_count - other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count - other.line_count,
                byte_count: self.byte_count,
            }
        }
    }
}

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
use crate::{state::SessionId, Change, Selection, Text};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    current_edit: Option<(SessionId, EditKind)>,
    undos: Vec<(Vec<Selection>, Vec<Change>)>,
    redos: Vec<(Vec<Selection>, Vec<Change>)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit = None;
    }

    pub fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        inverted_changes: Vec<Change>,
    ) {
        if self
            .current_edit
            .map_or(false, |current_edit| current_edit == (origin_id, kind))
        {
            self.undos.last_mut().unwrap().1.extend(inverted_changes);
        } else {
            self.current_edit = Some((origin_id, kind));
            self.undos.push((selections.to_vec(), inverted_changes));
        }
        self.redos.clear();
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, mut inverted_changes)) = self.undos.pop() {
            self.current_edit = None;
            let mut changes = Vec::new();
            inverted_changes.reverse();
            for inverted_change in inverted_changes.iter().cloned() {
                let change = inverted_change.clone().invert(&text);
                text.apply_change(inverted_change);
                changes.push(change);
            }
            self.redos.push((selections.clone(), changes.clone()));
            Some((selections, inverted_changes))
        } else {
            None
        }
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, changes)) = self.redos.pop() {
            self.current_edit = None;
            for change in changes.iter().cloned() {
                text.apply_change(change);
            }
            self.undos.push((selections.clone(), changes.clone()));
            Some((selections, changes))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EditGroup {
    pub selections: Vec<Selection>,
    pub changes: Vec<Change>,
}
use crate::widgets::{BlockWidget, InlineWidget};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Widget(BlockWidget),
}
pub trait IteratorExt: Iterator {
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>;
}

impl<T> IteratorExt for T
where
    T: Iterator,
{
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>,
    {
        Merge {
            prev_item: None,
            iter: self,
            f,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Merge<I, F>
where
    I: Iterator,
{
    prev_item: Option<I::Item>,
    iter: I,
    f: F,
}

impl<I, F> Iterator for Merge<I, F>
where
    I: Iterator,
    F: FnMut(I::Item, I::Item) -> Result<I::Item, (I::Item, I::Item)>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev_item.take(), self.iter.next()) {
                (Some(prev_item), Some(item)) => match (self.f)(prev_item, item) {
                    Ok(merged_item) => {
                        self.prev_item = Some(merged_item);
                        continue;
                    }
                    Err((prev_item, item)) => {
                        self.prev_item = Some(item);
                        break Some(prev_item);
                    }
                },
                (None, Some(item)) => {
                    self.prev_item = Some(item);
                    continue;
                }
                (Some(prev_item), None) => break Some(prev_item),
                (None, None) => break None,
            }
        }
    }
}
pub use makepad_widgets;
use makepad_widgets::*;

pub mod change;
pub mod char;
pub mod code_editor;
pub mod extent;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod line;
pub mod move_ops;
pub mod point;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    change::Change,
    code_editor::CodeEditor,
    extent::Extent,
    history::History,
    line::Line,
    point::Point,
    range::Range,
    selection::Selection,
    settings::Settings,
    state::{CodeDocument, CodeSession},
    text::Text,
    token::Token,
    tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
use {
    crate::{
        inlays::InlineInlay, selection::Affinity, str::StrExt, widgets::InlineWidget,
        wrap::WrapData, Token,
    },
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_data.unwrap().wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn byte_and_affinity_to_row_and_column(
        &self,
        byte: usize,
        affinity: Affinity,
        tab_column_count: usize,
    ) -> (usize, usize) {
        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if current_byte == byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte == byte && affinity == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if current_byte == byte && affinity == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    column += text.column_count(tab_column_count);
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    row += 1;
                    column = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte == byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_and_column_to_byte_and_affinity(
        &self,
        row: usize,
        column: usize,
        tab_width: usize,
    ) -> (usize, Affinity) {
        let mut current_row = 0;
        let mut current_column = 0;
        let mut byte = 0;
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_width);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte += grapheme.len();
                        current_column = next_column;
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column + text.column_count(tab_width);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                Wrapped::Widget(widget) => {
                    current_column += widget.column_count;
                }
                Wrapped::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.wrap_indent_column_count();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn wrap_indent_column_count(self) -> usize {
        self.wrap_data.unwrap().indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            position: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) position: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.position);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.position += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) position: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    mid = mid.min(position - self.position);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.position += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.position += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
mod app;

fn main() {
    app::app_main();
}
use crate::{selection::Affinity, str::StrExt, Point, CodeSession};

pub fn move_left(lines: &[String], point: Point) -> Point {
    if !is_at_start_of_line(point) {
        return move_to_prev_grapheme(lines, point);
    }
    if !is_at_first_line(point) {
        return move_to_end_of_prev_line(lines, point);
    }
    point
}

pub fn move_right(lines: &[String], point: Point) -> Point {
    if !is_at_end_of_line(lines, point) {
        return move_to_next_grapheme(lines, point);
    }
    if !is_at_last_line(lines, point) {
        return move_to_start_of_next_line(point);
    }
    point
}

pub fn move_up(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_first_row_of_line(session, point, affinity) {
        return move_to_prev_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_first_line(point) {
        return move_to_last_row_of_prev_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

pub fn move_down(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_last_row_of_line(session, point, affinity) {
        return move_to_next_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_last_line(session.document().borrow().text().as_lines(), point) {
        return move_to_first_row_of_next_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

fn is_at_first_line(point: Point) -> bool {
    point.line == 0
}

fn is_at_last_line(lines: &[String], point: Point) -> bool {
    point.line == lines.len()
}

fn is_at_start_of_line(point: Point) -> bool {
    point.byte == 0
}

fn is_at_end_of_line(lines: &[String], point: Point) -> bool {
    point.byte == lines[point.line].len()
}

fn is_at_first_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(lines: &[String], point: Point) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], point: Point) -> Point {
    let line = &lines[point.line];
    Point {
        line: point.line,
        byte: line[point.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| point.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], point: Point) -> Point {
    let prev_line = point.line - 1;
    Point {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(point: Point) -> Point {
    Point {
        line: point.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_next_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row + 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_last_row_of_prev_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line - 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}

fn move_to_first_row_of_next_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line + 1, |next_line| {
            let (byte, affinity) = next_line.row_and_column_to_byte_and_affinity(
                0,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line + 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}
use {
    crate::{
        change::{ChangeKind, Drift},
        Change, Extent,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.extent() + (self - point),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Extent, Point};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}
use {
    crate::{Change, Extent, Point, Range},
    std::ops,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Point {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Point {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Extent {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line..self.cursor.line + 1
        } else {
            self.cursor.line..if self.anchor.byte == 0 {
                self.anchor.line
            } else {
                self.anchor.line + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Point, Affinity, Option<usize>) -> (Point, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change),
            cursor: self.cursor.apply_change(change),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub use_soft_tabs: bool,
    pub tab_column_count: usize,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_soft_tabs: true,
            tab_column_count: 4,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
use {
    crate::{
        change::{ChangeKind, Drift},
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::Affinity,
        str::StrExt,
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        Change, Extent, History, Line, Point, Range, Selection, Settings, Text, Token, Tokenizer,
    },
    std::{
        cell::RefCell,
        cmp,
        collections::{HashMap, HashSet},
        iter, mem,
        rc::Rc,
        slice::Iter,
        sync::{
            atomic,
            atomic::AtomicUsize,
            mpsc,
            mpsc::{Receiver, Sender},
        },
    },
};

#[derive(Debug)]
pub struct CodeSession {
    id: SessionId,
    settings: Rc<Settings>,
    document: Rc<RefCell<CodeDocument>>,
    wrap_column: Option<usize>,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
    change_receiver: Receiver<(Option<Vec<Selection>>, Vec<Change>)>,
}

impl CodeSession {
    pub fn new(document: Rc<RefCell<CodeDocument>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (change_sender, change_receiver) = mpsc::channel();
        let line_count = document.borrow().text.as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wrap_data: (0..line_count).map(|_| None).collect(),
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
            change_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .change_senders
            .insert(session.id, change_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().text.as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().text.as_lines().len() - 1;
        let mut y = self.line(index, |line| line.y() + line.height());
        self.blocks(index, index, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: true,
                        line,
                    } => y += line.height(),
                    Block::Widget(widget) => y += widget.height,
                    _ => unreachable!(),
                }
            }
        });
        y
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Rc<RefCell<CodeDocument>> {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document.borrow();
        f(Line {
            y: self.y.get(line).copied(),
            column_count: self.column_count[line],
            fold_column: self.fold_column[line],
            scale: self.scale[line],
            text: &document.text.as_lines()[line],
            tokens: &document.tokens[line],
            inline_inlays: &document.inline_inlays[line],
            wrap_data: self.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        f(Lines {
            y: self.y[start_line.min(self.y.len())..end_line.min(self.y.len())].iter(),
            column_count: self.column_count[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
            text: document.text.as_lines()[start_line..end_line].iter(),
            tokens: document.tokens[start_line..end_line].iter(),
            inline_inlays: document.inline_inlays[start_line..end_line].iter(),
            wrap_data: self.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        let mut block_inlays = document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < start_line)
        {
            block_inlays.next();
        }
        self.lines(start_line, end_line, |lines| {
            f(Blocks {
                lines,
                block_inlays,
                position: start_line,
            })
        })
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .indentation()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.fold_column[line] =
                    self.settings.fold_level * self.settings.indent_column_count;
                self.unfolding_lines.remove(&line);
                self.folding_lines.insert(line);
            }
        }
    }

    pub fn unfold(&mut self) {
        for line in self.folding_lines.drain() {
            self.unfolding_lines.insert(line);
        }
        for line in self.folded_lines.drain() {
            self.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&mut self) -> bool {
        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let mut new_folding_lines = HashSet::new();
        for &line in &self.folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.1 + 0.001 {
                self.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, cursor: Point, affinity: Affinity) {
        self.selections.clear();
        self.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        });
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn add_cursor(&mut self, cursor: Point, affinity: Affinity) {
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        };
        self.pending_selection_index = Some(
            match self.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return cmp::Ordering::Less;
                }
                if selection.start() >= cursor {
                    return cmp::Ordering::Greater;
                }
                cmp::Ordering::Equal
            }) {
                Ok(index) => {
                    self.selections[index] = selection;
                    index
                }
                Err(index) => {
                    self.selections.insert(index, selection);
                    index
                }
            },
        );
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, cursor: Point, affinity: Affinity) {
        let mut pending_selection_index = self.pending_selection_index.unwrap();
        self.selections[pending_selection_index] = Selection {
            cursor,
            affinity,
            ..self.selections[pending_selection_index]
        };
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !self.selections[prev_selection_index]
                .should_merge(self.selections[pending_selection_index])
            {
                break;
            }
            self.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < self.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !self.selections[pending_selection_index]
                .should_merge(self.selections[next_selection_index])
            {
                break;
            }
            self.selections.remove(next_selection_index);
        }
        self.pending_selection_index = Some(pending_selection_index);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_left(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_right(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_up(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_down(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn insert(&mut self, text: Text) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Insert, &self.selections, |_, _, _| {
                (Extent::zero(), Some(text.clone()), None)
            });
    }

    pub fn enter(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Insert,
            &self.selections,
            |line, index, _| {
                (
                    if line[..index].chars().all(|char| char.is_whitespace()) {
                        Extent {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Extent::zero()
                    },
                    Some(Text::newline()),
                    if line[..index]
                        .chars()
                        .rev()
                        .find_map(|char| {
                            if char.is_opening_delimiter() {
                                return Some(true);
                            }
                            if char.is_closing_delimiter() {
                                return Some(false);
                            }
                            None
                        })
                        .unwrap_or(false)
                        && line[index..]
                            .chars()
                            .find_map(|char| {
                                if char.is_closing_delimiter() {
                                    return Some(true);
                                }
                                if !char.is_whitespace() {
                                    return Some(false);
                                }
                                None
                            })
                            .unwrap_or(false)
                    {
                        Some(Text::newline())
                    } else {
                        None
                    },
                )
            },
        );
    }

    pub fn indent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Indent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        (indentation_column_count + self.settings.indent_column_count)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Outdent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        indentation_column_count.saturating_sub(1)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn delete(&mut self) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Delete, &self.selections, |_, _, _| {
                (Extent::zero(), None, None)
            });
    }

    pub fn backspace(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            |line, index, is_empty| {
                (
                    if is_empty {
                        if index == 0 {
                            Extent {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Extent {
                                line_count: 0,
                                byte_count: line.graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Extent::zero()
                    },
                    None,
                    None,
                )
            },
        );
    }

    pub fn undo(&mut self) {
        self.document.borrow_mut().undo(self.id);
    }

    pub fn redo(&mut self) {
        self.document.borrow_mut().redo(self.id);
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().text.as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.y);
        self.blocks(start, end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line { is_inlay, line } => {
                        if !is_inlay {
                            ys.push(y);
                        }
                        y += line.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        ys.push(y);
        self.y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, changes)) = self.change_receiver.try_recv() {
            self.apply_changes(selections, &changes);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        self.line(index, |line| {
            for wrapped in line.wrappeds() {
                match wrapped {
                    Wrapped::Text { text, .. } => {
                        column += text
                            .column_count(self.settings.tab_column_count);
                    }
                    Wrapped::Widget(widget) => {
                        column += widget.column_count;
                    }
                    Wrapped::Wrap => {
                        column_count = column_count.max(column);
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        });
        self.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.wrap_data[line] = Some(wrap_data);
        self.y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(&CodeSession, Selection) -> Selection,
    ) {
        let mut selections = mem::take(&mut self.selections);
        for selection in &mut selections {
            *selection = f(&self, *selection);
            if reset_anchor {
                *selection = selection.reset_anchor();
            }
        }
        self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
                self.selections[current_selection_index] = merged_selection;
                self.selections.remove(next_selection_index);
                if let Some(pending_selection_index) = self.pending_selection_index.as_mut() {
                    if next_selection_index < *pending_selection_index {
                        *pending_selection_index -= 1;
                    }
                }
            } else {
                current_selection_index += 1;
            }
        }
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_changes(&mut self, selections: Option<Vec<Selection>>, changes: &[Change]) {
        for change in changes {
            match &change.kind {
                ChangeKind::Insert(point, text) => {
                    self.column_count[point.line] = None;
                    self.wrap_data[point.line] = None;
                    let line_count = text.extent().line_count;
                    if line_count > 0 {
                        let line = point.line + 1;
                        self.y.truncate(line);
                        self.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                ChangeKind::Delete(range) => {
                    self.column_count[range.start().line] = None;
                    self.wrap_data[range.start().line] = None;
                    let line_count = range.extent().line_count;
                    if line_count > 0 {
                        let start_line = range.start().line + 1;
                        let end_line = start_line + line_count;
                        self.y.truncate(start_line);
                        self.column_count.drain(start_line..end_line);
                        self.fold_column.drain(start_line..end_line);
                        self.scale.drain(start_line..end_line);
                        self.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for change in changes {
                for selection in &mut self.selections {
                    *selection = selection.apply_change(&change);
                }
            }
        }
        self.update_y();
    }
}

impl Drop for CodeSession {
    fn drop(&mut self) {
        self.document.borrow_mut().change_senders.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_data: Iter<'a, Option<WrapData>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct CodeDocument {
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    history: History,
    tokenizer: Tokenizer,
    change_senders: HashMap<SessionId, Sender<(Option<Vec<Selection>>, Vec<Change>)>>,
}

impl CodeDocument {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            text,
            tokens,
            inline_inlays: (0..line_count)
                .map(|line| {
                    if line % 5 == 0 {
                        [
                            (20, InlineInlay::Text("XXX".into())),
                            (40, InlineInlay::Text("XXX".into())),
                            (60, InlineInlay::Text("XXX".into())),
                            (80, InlineInlay::Text("XXX".into())),
                        ]
                        .into()
                    } else {
                        Vec::new()
                    }
                })
                .collect(),
            block_inlays: Vec::new(),
            history: History::new(),
            tokenizer: Tokenizer::new(line_count),
            change_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.text, &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&String, usize, bool) -> (Extent, Option<Text>, Option<Text>),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        let mut point = Point::zero();
        let mut prev_range_end = Point::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, range.extent())),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            let (delete_extent, insert_text_before, insert_text_after) = f(
                &self.text.as_lines()[point.line],
                point.byte,
                range.is_empty(),
            );
            if delete_extent != Extent::zero() {
                if delete_extent.line_count == 0 {
                    point.byte -= delete_extent.byte_count;
                } else {
                    point.line -= delete_extent.line_count;
                    point.byte = self.text.as_lines()[point.line].len() - delete_extent.byte_count;
                }
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, delete_extent)),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_before) = insert_text_before {
                let extent = insert_text_before.extent();
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Insert(point, insert_text_before),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_after) = insert_text_after {
                let extent = insert_text_after.extent();
                let change = Change {
                    drift: Drift::After,
                    kind: ChangeKind::Insert(point, insert_text_after),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            prev_range_end = range.end();
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut changes, &mut inverted_changes, &mut f);
            }
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.text.as_lines()[line]);
        if delete_byte_count > 0 {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Delete(Range::from_start_and_extent(
                    Point { line, byte },
                    Extent {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                )),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
        if !insert_text.is_empty() {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Insert(Point { line, byte }, insert_text.into()),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.undo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn redo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.redo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn apply_changes(
        &mut self,
        origin_id: SessionId,
        selections: Option<Vec<Selection>>,
        changes: &[Change],
    ) {
        for change in changes {
            self.apply_change_to_tokens(change);
            self.apply_change_to_inline_inlays(change);
            self.tokenizer.apply_change(change);
        }
        self.tokenizer.update(&self.text, &mut self.tokens);
        for (&session_id, change_sender) in &self.change_senders {
            if session_id == origin_id {
                change_sender
                    .send((selections.clone(), changes.to_vec()))
                    .unwrap();
            } else {
                change_sender
                    .send((
                        None,
                        changes
                            .iter()
                            .cloned()
                            .map(|change| Change {
                                drift: Drift::Before,
                                ..change
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line].len());
                if byte != point.byte {
                    let token = self.tokens[point.line][index];
                    let mid = point.byte - byte;
                    self.tokens[point.line][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.extent().line_count == 0 {
                    self.tokens[point.line]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][index..].iter().copied());
                    self.tokens.splice(point.line..point.line + 1, tokens);
                }
            }
            ChangeKind::Delete(range) => {
                let mut byte = 0;
                let mut start = self.tokens[range.start().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.start().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.start().line].len());
                if byte != range.start().byte {
                    let token = self.tokens[range.start().line][start];
                    let mid = range.start().byte - byte;
                    self.tokens[range.start().line][start] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start += 1;
                    self.tokens[range.start().line].insert(
                        start,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end = self.tokens[range.end().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.end().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.end().line].len());
                if byte != range.end().byte {
                    let token = self.tokens[range.end().line][end];
                    let mid = range.end().byte - byte;
                    self.tokens[range.end().line][end] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end += 1;
                    self.tokens[range.end().line].insert(
                        end,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if range.start().line == range.end().line {
                    self.tokens[range.start().line].drain(start..end);
                } else {
                    let mut tokens = self.tokens[range.start().line][..start]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[range.end().line][end..].iter().copied());
                    self.tokens
                        .splice(range.start().line..range.end().line + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.extent().byte_count, inline_inlay)
                            }),
                    );
                    self.inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = self.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(self.inline_inlays[range.start().line].len());
                let end = self.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(self.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    self.inline_inlays[range.start().line].drain(start..end);
                    for (byte, _) in &mut self.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[range.start().line]
                        .drain(..start)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(self.inline_inlays[range.end().line].drain(end..).map(
                        |(byte, inline_inlay)| {
                            (
                                range.start().byte + byte - range.end().byte.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    self.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}

fn reindent(
    string: &str,
    use_soft_tabs: bool,
    tab_column_count: usize,
    f: impl FnOnce(usize) -> usize,
) -> (usize, usize, String) {
    let indentation = string.indentation().unwrap_or("");
    let indentation_column_count = indentation.column_count(tab_column_count);
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(
        new_indentation_column_count,
        use_soft_tabs,
        tab_column_count,
    );
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize, use_soft_tabs: bool, tab_column_count: usize) -> String {
    let tab_count;
    let space_count;
    if use_soft_tabs {
        tab_count = 0;
        space_count = column_count;
    } else {
        tab_count = column_count / tab_column_count;
        space_count = column_count % tab_column_count;
    }
    let tabs = iter::repeat("\t").take(tab_count);
    let spaces = iter::repeat(" ").take(space_count);
    tabs.chain(spaces).collect()
}
use crate::char::CharExt;

pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indentation(&self) -> Option<&str>;
    fn longest_common_prefix(&self, other: &str) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indentation(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn longest_common_prefix(&self, other: &str) -> &str {
        &self[..self
            .char_indices()
            .zip(other.chars())
            .find(|((_, char_0), char_1)| char_0 == char_1)
            .map(|((index, _), _)| index)
            .unwrap_or_else(|| self.len().min(other.len()))]
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
    }

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices {
            graphemes: self.graphemes(),
            start: self.as_ptr() as usize,
        }
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    string: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut end = 1;
        while !self.string.is_char_boundary(end) {
            end += 1;
        }
        let (grapheme, string) = self.string.split_at(end);
        self.string = string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut start = self.string.len() - 1;
        while !self.string.is_char_boundary(start) {
            start -= 1;
        }
        let (string, grapheme) = self.string.split_at(start);
        self.string = string;
        Some(grapheme)
    }
}

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
    start: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
use {
    crate::{change, Change, Extent, Point, Range},
    std::{io, io::BufRead, iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn newline() -> Self {
        Self {
            lines: vec![String::new(), String::new()],
        }
    }

    pub fn from_buf_reader<R>(reader: R) -> io::Result<Self>
    where
        R: BufRead,
    {
        Ok(Self {
            lines: reader.lines().collect::<Result<_, _>>()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.extent() == Extent::zero()
    }

    pub fn extent(&self) -> Extent {
        Extent {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line][range.start().byte..range.end().byte].to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines.push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
        }
        Text { lines }
    }

    pub fn insert(&mut self, point: Point, mut text: Self) {
        if text.extent().line_count == 0 {
            self.lines[point.line]
                .replace_range(point.byte..point.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line][..point.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line][point.byte..]);
            self.lines.splice(point.line..point.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, range: Range) {
        if range.start().line == range.end().line {
            self.lines[range.start().line].replace_range(range.start().byte..range.end().byte, "");
        } else {
            let mut line = self.lines[range.start().line][..range.start().byte].to_string();
            line.push_str(&self.lines[range.end().line][range.end().byte..]);
            self.lines
                .splice(range.start().line..range.end().line + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change.kind {
            change::ChangeKind::Insert(point, additional_text) => {
                self.insert(point, additional_text)
            }
            change::ChangeKind::Delete(range) => self.delete(range),
        }
    }

    pub fn into_line_count(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        Self {
            lines: string.lines().map(|string| string.to_owned()).collect(),
        }
    }
}

impl From<&String> for Text {
    fn from(string: &String) -> Self {
        string.as_str().into()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Whitespace,
}
use crate::{change::ChangeKind, token::TokenKind, Change, Text, Token};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
}

impl Tokenizer {
    pub fn new(line_count: usize) -> Self {
        Self {
            state: (0..line_count).map(|_| None).collect(),
        }
    }

    pub fn apply_change(&mut self, change: &Change) {
        match &change.kind {
            ChangeKind::Insert(point, text) => {
                self.state[point.line] = None;
                let line_count = text.extent().line_count;
                if line_count > 0 {
                    let line = point.line + 1;
                    self.state.splice(line..line, (0..line_count).map(|_| None));
                }
            }
            ChangeKind::Delete(range) => {
                self.state[range.start().line] = None;
                let line_count = range.extent().line_count;
                if line_count > 0 {
                    let start_line = range.start().line + 1;
                    let end_line = start_line + line_count;
                    self.state.drain(start_line..end_line);
                }
            }
        }
    }

    pub fn update(&mut self, text: &Text, tokens: &mut [Vec<Token>]) {
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut new_tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => new_tokens.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    tokens[line] = new_tokens;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<Token>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('!', '=', _)
            | ('%', '=', _)
            | ('&', '&', _)
            | ('&', '=', _)
            | ('*', '=', _)
            | ('+', '=', _)
            | ('-', '=', _)
            | ('-', '>', _)
            | ('.', '.', _)
            | ('/', '=', _)
            | (':', ':', _)
            | ('<', '<', _)
            | ('<', '=', _)
            | ('=', '=', _)
            | ('=', '>', _)
            | ('>', '=', _)
            | ('>', '>', _)
            | ('^', '=', _)
            | ('|', '=', _)
            | ('|', '|', _) => {
                cursor.skip(2);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            ('.', char, _) if char.is_digit(10) => self.number(cursor),
            ('!', _, _)
            | ('#', _, _)
            | ('$', _, _)
            | ('%', _, _)
            | ('&', _, _)
            | ('*', _, _)
            | ('+', _, _)
            | (',', _, _)
            | ('-', _, _)
            | ('.', _, _)
            | ('/', _, _)
            | (':', _, _)
            | (';', _, _)
            | ('<', _, _)
            | ('=', _, _)
            | ('>', _, _)
            | ('?', _, _)
            | ('@', _, _)
            | ('^', _, _)
            | ('_', _, _)
            | ('|', _, _) => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            (char, _, _) if char.is_identifier_start() => self.identifier_or_keyword(cursor),
            (char, _, _) if char.is_digit(10) => self.number(cursor),
            (char, _, _) if char.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index;
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_identifier_continue()) {}
        let end = cursor.index;

        (
            State::Initial(InitialState),
            match &cursor.string[start..end] {
                "else" | "if" | "match" | "return" => TokenKind::BranchKeyword,
                "break" | "continue" | "for" | "loop" | "while" => TokenKind::LoopKeyword,
                "Self" | "as" | "async" | "await" | "const" | "crate" | "dyn" | "enum"
                | "extern" | "false" | "fn" | "impl" | "in" | "let" | "mod" | "move" | "mut"
                | "pub" | "ref" | "self" | "static" | "struct" | "super" | "trait" | "true"
                | "type" | "unsafe" | "use" | "where" => TokenKind::OtherKeyword,
                _ => TokenKind::Identifier,
            },
        )
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    _ => {
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                }
            }
        };
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    string: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Cursor { string, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.string[self.index..].chars().nth(index).unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index = self.string[self.index..]
            .char_indices()
            .nth(count)
            .map_or(self.string.len(), |(index, _)| self.index + index);
    }

    fn skip_if<P>(&mut self, predicate: P) -> bool
    where
        P: FnOnce(char) -> bool,
    {
        if predicate(self.peek(0)) {
            self.skip(1);
            true
        } else {
            false
        }
    }

    fn skip_exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(0) == '-' {
            self.skip(1);
        }
        self.skip_digits(10)
    }

    fn skip_digits(&mut self, radix: u32) -> bool {
        let mut has_skip_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                char if char.is_digit(radix) => {
                    self.skip(1);
                    has_skip_digits = true;
                }
                _ => break,
            }
        }
        has_skip_digits
    }

    fn skip_suffix(&mut self) -> bool {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if(|char| char.is_identifier_continue()) {}
            return true;
        }
        false
    }
}

pub trait CharExt {
    fn is_identifier_start(self) -> bool;
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub height: f64,
}
use crate::{char::CharExt, line::Inline, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize, tab_column_count: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .indentation()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count(tab_column_count))
        .sum();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            Inline::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte = 0;
    let mut column = 0;
    let mut wraps = Vec::new();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if column + column_count > wrap_column {
                        column = indent_column_count;
                        wraps.push(byte);
                    }
                    column += column_count;
                    byte += string.len();
                }
            }
            Inline::Widget(widget) => {
                if column + widget.column_count > wrap_column {
                    column = indent_column_count;
                    wraps.push(byte);
                }
                column += widget.column_count;
                byte += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}
use {
    makepad_code_editor::{
        code_editor::*,
        state::{CodeDocument, CodeSession},
    },
    makepad_widgets::*,
    std::{cell::RefCell, rc::Rc},
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_code_editor::code_editor::CodeEditor;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <CodeEditor> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    code_editor.draw(&mut cx, &mut self.state.session);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        if let Some(mut code_editor) = self.ui.get_code_editor(id!(code_editor)).borrow_mut() {
            code_editor.handle_event(cx, event, &mut self.state.session);
        }
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_code_editor::code_editor::live_design(cx);
    }
}

struct State {
    session: CodeSession,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: CodeSession::new(Rc::new(RefCell::new(CodeDocument::new(
                include_str!("state.rs").into(),
            )))),
        }
    }
}

app_main!(App);
use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => {
                    ChangeKind::Insert(range.start(), text.slice(range))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{
        line::Wrapped,
        selection::Affinity,
        state::{Block, CodeSession},
        str::StrExt,
        token::TokenKind,
        Line, Point, Selection, Token,
    },
    makepad_widgets::*,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    TokenColors = {{TokenColors}} {
        unknown: #808080,
        branch_keyword: #C485BE,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        whitespace: #6E6E6E,
    }

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        
            width: Fill,
            height: Fill,
            margin: 0,
        
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
    #[live]
    draw_text: DrawText,
    #[live]
    token_colors: TokenColors,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start: usize,
    #[rust]
    end: usize,
}

impl LiveHook for CodeEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, CodeEditor)
    }
}

impl Widget for CodeEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }

    fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        //let uid = self.widget_uid();
        /*self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });*/
        //self.handle_event
    }

    fn walk(&self) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CodeEditorRef(WidgetRef);

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, session: &mut CodeSession) {
        let walk = self.draw_state.get().unwrap();

        self.scroll_bars.begin(cx, walk, Layout::default());

        self.viewport_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();

        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        session.handle_changes();
        session.set_wrap_column(Some(
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        ));
        self.start = session.find_first_line_ending_after_y(scroll_pos.y / self.cell_size.y);
        self.end = session.find_first_line_starting_after_y(
            (scroll_pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );

        self.draw_text(cx, session);
        self.draw_selections(cx, session);
        cx.turtle_mut().set_used(
            session.width() * self.cell_size.x,
            session.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if session.update_folds() {
            cx.redraw_all();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, session: &mut CodeSession) {
        session.handle_changes();
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.fold();
                cx.redraw_all();
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.unfold();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_left(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_right(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_up(!shift);
                cx.redraw_all();
            }

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_down(!shift);
                cx.redraw_all();
            }
            Hit::TextInput(TextInputEvent { ref input, .. }) => {
                session.insert(input.into());
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                session.enter();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::RBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.indent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::LBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.outdent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                session.delete();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.backspace();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: false, .. },
                ..
            }) => {
                session.undo();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: true, .. },
                ..
            }) => {
                session.redo();
                cx.redraw_all();
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                cx.set_key_focus(self.scroll_bars.area());
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    if alt {
                        session.add_cursor(cursor, affinity);
                    } else {
                        session.set_cursor(cursor, affinity);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    session.move_to(cursor, affinity);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut y = 0.0;
        session.blocks(
            0,
            session.document().borrow().text().as_lines().len(),
            |blocks| {
                for block in blocks {
                    match block {
                        Block::Line { line, .. } => {
                            self.draw_text.font_scale = line.scale();
                            let mut token_iter = line.tokens().iter().copied();
                            let mut token_slot = token_iter.next();
                            let mut column = 0;
                            for wrapped in line.wrappeds() {
                                match wrapped {
                                    Wrapped::Text {
                                        is_inlay: false,
                                        mut text,
                                    } => {
                                        while !text.is_empty() {
                                            let token = match token_slot {
                                                Some(token) => {
                                                    if text.len() < token.len {
                                                        token_slot = Some(Token {
                                                            len: token.len - text.len(),
                                                            kind: token.kind,
                                                        });
                                                        Token {
                                                            len: text.len(),
                                                            kind: token.kind,
                                                        }
                                                    } else {
                                                        token_slot = token_iter.next();
                                                        token
                                                    }
                                                }
                                                None => Token {
                                                    len: text.len(),
                                                    kind: TokenKind::Unknown,
                                                },
                                            };
                                            let (text_0, text_1) = text.split_at(token.len);
                                            text = text_1;
                                            self.draw_text.color = match token.kind {
                                                TokenKind::Unknown => self.token_colors.unknown,
                                                TokenKind::BranchKeyword => {
                                                    self.token_colors.branch_keyword
                                                }
                                                TokenKind::Identifier => {
                                                    self.token_colors.identifier
                                                }
                                                TokenKind::LoopKeyword => {
                                                    self.token_colors.loop_keyword
                                                }
                                                TokenKind::Number => self.token_colors.number,
                                                TokenKind::OtherKeyword => {
                                                    self.token_colors.other_keyword
                                                }
                                                TokenKind::Punctuator => {
                                                    self.token_colors.punctuator
                                                }
                                                TokenKind::Whitespace => {
                                                    self.token_colors.whitespace
                                                }
                                            };
                                            self.draw_text.draw_abs(
                                                cx,
                                                DVec2 {
                                                    x: line.column_to_x(column),
                                                    y,
                                                } * self.cell_size
                                                    + self.viewport_rect.pos,
                                                text_0,
                                            );
                                            column += text_0
                                                .column_count(session.settings().tab_column_count);
                                        }
                                    }
                                    Wrapped::Text {
                                        is_inlay: true,
                                        text,
                                    } => {
                                        self.draw_text.draw_abs(
                                            cx,
                                            DVec2 {
                                                x: line.column_to_x(column),
                                                y,
                                            } * self.cell_size
                                                + self.viewport_rect.pos,
                                            text,
                                        );
                                        column +=
                                            text.column_count(session.settings().tab_column_count);
                                    }
                                    Wrapped::Widget(widget) => {
                                        column += widget.column_count;
                                    }
                                    Wrapped::Wrap => {
                                        column = line.wrap_indent_column_count();
                                        y += line.scale();
                                    }
                                }
                            }
                            y += line.scale();
                        }
                        Block::Widget(widget) => {
                            y += widget.height;
                        }
                    }
                }
            },
        );
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, session: &CodeSession) {
        let mut active_selection = None;
        let mut selections = session.selections().iter();
        while selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.end().line < self.start)
        {
            selections.next().unwrap();
        }
        if selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.start().line < self.start)
        {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelections {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, session)
    }

    fn pick(&self, session: &CodeSession, point: DVec2) -> Option<(Point, Affinity)> {
        let point = (point - self.viewport_rect.pos) / self.cell_size;
        let mut line = session.find_first_line_ending_after_y(point.y);
        let mut y = session.line(line, |line| line.y());
        session.blocks(line, line + 1, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        let next_byte = byte + grapheme.len();
                                        let next_column = column
                                            + grapheme
                                                .column_count(session.settings().tab_column_count);
                                        let next_y = y + line_ref.scale();
                                        let x = line_ref.column_to_x(column);
                                        let next_x = line_ref.column_to_x(next_column);
                                        let mid_x = (x + next_x) / 2.0;
                                        if (y..=next_y).contains(&point.y) {
                                            if (x..=mid_x).contains(&point.x) {
                                                return Some((
                                                    Point { line, byte },
                                                    Affinity::After,
                                                ));
                                            }
                                            if (mid_x..=next_x).contains(&point.x) {
                                                return Some((
                                                    Point {
                                                        line,
                                                        byte: next_byte,
                                                    },
                                                    Affinity::Before,
                                                ));
                                            }
                                        }
                                        byte = next_byte;
                                        column = next_column;
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    let next_column = column
                                        + text.column_count(session.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    if (y..=next_y).contains(&point.y)
                                        && (x..=next_x).contains(&point.x)
                                    {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = next_column;
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    let next_y = y + line_ref.scale();
                                    if (y..=next_y).contains(&point.y) {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y = next_y;
                                }
                            }
                        }
                        let next_y = y + line_ref.scale();
                        if (y..=y + next_y).contains(&point.y) {
                            return Some((Point { line, byte }, Affinity::After));
                        }
                        line += 1;
                        y = next_y;
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        let next_y = y + line_ref.height();
                        if (y..=next_y).contains(&point.y) {
                            return Some((Point { line, byte: 0 }, Affinity::Before));
                        }
                        y = next_y;
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
            None
        })
    }
}

struct DrawSelections<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelections<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut line = self.code_editor.start;
        let mut y = session.line(line, |line| line.y());
        session.blocks(self.code_editor.start, self.code_editor.end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        self.handle_event(cx, line, line_ref, byte, Affinity::Before, y, column);
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::After,
                                            y,
                                            column,
                                        );
                                        byte += grapheme.len();
                                        column += grapheme
                                            .column_count(session.settings().tab_column_count);
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::Before,
                                            y,
                                            column,
                                        );
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    column +=
                                        text.column_count(session.settings().tab_column_count);
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    if self.active_selection.is_some() {
                                        self.draw_selection(cx, line_ref, y, column);
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y += line_ref.scale();
                                }
                            }
                        }
                        self.handle_event(cx, line, line_ref, byte, Affinity::After, y, column);
                        column += 1;
                        if self.active_selection.is_some() {
                            self.draw_selection(cx, line_ref, y, column);
                        }
                        line += 1;
                        y += line_ref.scale();
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        y += line_ref.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line: usize,
        line_ref: Line<'_>,
        byte: usize,
        affinity: Affinity,
        y: f64,
        column: usize,
    ) {
        let point = Point { line, byte };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == point && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line_ref, y, column);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == point && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: line_ref.column_to_x(column),
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, line: Line<'_>, y: f64, column: usize) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: line.column_to_x(column) - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column: usize) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: line.column_to_x(column),
                    y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    whitespace: Vec4,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
            Self {
                line_count: 0,
                byte_count: self.byte_count - other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count - other.line_count,
                byte_count: self.byte_count,
            }
        }
    }
}

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
use crate::{state::SessionId, Change, Selection, Text};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    current_edit: Option<(SessionId, EditKind)>,
    undos: Vec<(Vec<Selection>, Vec<Change>)>,
    redos: Vec<(Vec<Selection>, Vec<Change>)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit = None;
    }

    pub fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        inverted_changes: Vec<Change>,
    ) {
        if self
            .current_edit
            .map_or(false, |current_edit| current_edit == (origin_id, kind))
        {
            self.undos.last_mut().unwrap().1.extend(inverted_changes);
        } else {
            self.current_edit = Some((origin_id, kind));
            self.undos.push((selections.to_vec(), inverted_changes));
        }
        self.redos.clear();
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, mut inverted_changes)) = self.undos.pop() {
            self.current_edit = None;
            let mut changes = Vec::new();
            inverted_changes.reverse();
            for inverted_change in inverted_changes.iter().cloned() {
                let change = inverted_change.clone().invert(&text);
                text.apply_change(inverted_change);
                changes.push(change);
            }
            self.redos.push((selections.clone(), changes.clone()));
            Some((selections, inverted_changes))
        } else {
            None
        }
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, changes)) = self.redos.pop() {
            self.current_edit = None;
            for change in changes.iter().cloned() {
                text.apply_change(change);
            }
            self.undos.push((selections.clone(), changes.clone()));
            Some((selections, changes))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EditGroup {
    pub selections: Vec<Selection>,
    pub changes: Vec<Change>,
}
use crate::widgets::{BlockWidget, InlineWidget};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Widget(BlockWidget),
}
pub trait IteratorExt: Iterator {
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>;
}

impl<T> IteratorExt for T
where
    T: Iterator,
{
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>,
    {
        Merge {
            prev_item: None,
            iter: self,
            f,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Merge<I, F>
where
    I: Iterator,
{
    prev_item: Option<I::Item>,
    iter: I,
    f: F,
}

impl<I, F> Iterator for Merge<I, F>
where
    I: Iterator,
    F: FnMut(I::Item, I::Item) -> Result<I::Item, (I::Item, I::Item)>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev_item.take(), self.iter.next()) {
                (Some(prev_item), Some(item)) => match (self.f)(prev_item, item) {
                    Ok(merged_item) => {
                        self.prev_item = Some(merged_item);
                        continue;
                    }
                    Err((prev_item, item)) => {
                        self.prev_item = Some(item);
                        break Some(prev_item);
                    }
                },
                (None, Some(item)) => {
                    self.prev_item = Some(item);
                    continue;
                }
                (Some(prev_item), None) => break Some(prev_item),
                (None, None) => break None,
            }
        }
    }
}
pub use makepad_widgets;
use makepad_widgets::*;

pub mod change;
pub mod char;
pub mod code_editor;
pub mod extent;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod line;
pub mod move_ops;
pub mod point;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    change::Change,
    code_editor::CodeEditor,
    extent::Extent,
    history::History,
    line::Line,
    point::Point,
    range::Range,
    selection::Selection,
    settings::Settings,
    state::{CodeDocument, CodeSession},
    text::Text,
    token::Token,
    tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
use {
    crate::{
        inlays::InlineInlay, selection::Affinity, str::StrExt, widgets::InlineWidget,
        wrap::WrapData, Token,
    },
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_data.unwrap().wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn byte_and_affinity_to_row_and_column(
        &self,
        byte: usize,
        affinity: Affinity,
        tab_column_count: usize,
    ) -> (usize, usize) {
        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if current_byte == byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte == byte && affinity == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if current_byte == byte && affinity == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    column += text.column_count(tab_column_count);
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    row += 1;
                    column = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte == byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_and_column_to_byte_and_affinity(
        &self,
        row: usize,
        column: usize,
        tab_width: usize,
    ) -> (usize, Affinity) {
        let mut current_row = 0;
        let mut current_column = 0;
        let mut byte = 0;
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_width);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte += grapheme.len();
                        current_column = next_column;
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column + text.column_count(tab_width);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                Wrapped::Widget(widget) => {
                    current_column += widget.column_count;
                }
                Wrapped::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.wrap_indent_column_count();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn wrap_indent_column_count(self) -> usize {
        self.wrap_data.unwrap().indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            position: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) position: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.position);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.position += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) position: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    mid = mid.min(position - self.position);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.position += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.position += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
mod app;

fn main() {
    app::app_main();
}
use crate::{selection::Affinity, str::StrExt, Point, CodeSession};

pub fn move_left(lines: &[String], point: Point) -> Point {
    if !is_at_start_of_line(point) {
        return move_to_prev_grapheme(lines, point);
    }
    if !is_at_first_line(point) {
        return move_to_end_of_prev_line(lines, point);
    }
    point
}

pub fn move_right(lines: &[String], point: Point) -> Point {
    if !is_at_end_of_line(lines, point) {
        return move_to_next_grapheme(lines, point);
    }
    if !is_at_last_line(lines, point) {
        return move_to_start_of_next_line(point);
    }
    point
}

pub fn move_up(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_first_row_of_line(session, point, affinity) {
        return move_to_prev_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_first_line(point) {
        return move_to_last_row_of_prev_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

pub fn move_down(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_last_row_of_line(session, point, affinity) {
        return move_to_next_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_last_line(session.document().borrow().text().as_lines(), point) {
        return move_to_first_row_of_next_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

fn is_at_first_line(point: Point) -> bool {
    point.line == 0
}

fn is_at_last_line(lines: &[String], point: Point) -> bool {
    point.line == lines.len()
}

fn is_at_start_of_line(point: Point) -> bool {
    point.byte == 0
}

fn is_at_end_of_line(lines: &[String], point: Point) -> bool {
    point.byte == lines[point.line].len()
}

fn is_at_first_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(lines: &[String], point: Point) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], point: Point) -> Point {
    let line = &lines[point.line];
    Point {
        line: point.line,
        byte: line[point.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| point.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], point: Point) -> Point {
    let prev_line = point.line - 1;
    Point {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(point: Point) -> Point {
    Point {
        line: point.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_next_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row + 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_last_row_of_prev_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line - 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}

fn move_to_first_row_of_next_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line + 1, |next_line| {
            let (byte, affinity) = next_line.row_and_column_to_byte_and_affinity(
                0,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line + 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}
use {
    crate::{
        change::{ChangeKind, Drift},
        Change, Extent,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.extent() + (self - point),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Extent, Point};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}
use {
    crate::{Change, Extent, Point, Range},
    std::ops,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Point {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Point {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Extent {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line..self.cursor.line + 1
        } else {
            self.cursor.line..if self.anchor.byte == 0 {
                self.anchor.line
            } else {
                self.anchor.line + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Point, Affinity, Option<usize>) -> (Point, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change),
            cursor: self.cursor.apply_change(change),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub use_soft_tabs: bool,
    pub tab_column_count: usize,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_soft_tabs: true,
            tab_column_count: 4,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
use {
    crate::{
        change::{ChangeKind, Drift},
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::Affinity,
        str::StrExt,
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        Change, Extent, History, Line, Point, Range, Selection, Settings, Text, Token, Tokenizer,
    },
    std::{
        cell::RefCell,
        cmp,
        collections::{HashMap, HashSet},
        iter, mem,
        rc::Rc,
        slice::Iter,
        sync::{
            atomic,
            atomic::AtomicUsize,
            mpsc,
            mpsc::{Receiver, Sender},
        },
    },
};

#[derive(Debug)]
pub struct CodeSession {
    id: SessionId,
    settings: Rc<Settings>,
    document: Rc<RefCell<CodeDocument>>,
    wrap_column: Option<usize>,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
    change_receiver: Receiver<(Option<Vec<Selection>>, Vec<Change>)>,
}

impl CodeSession {
    pub fn new(document: Rc<RefCell<CodeDocument>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (change_sender, change_receiver) = mpsc::channel();
        let line_count = document.borrow().text.as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wrap_data: (0..line_count).map(|_| None).collect(),
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
            change_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .change_senders
            .insert(session.id, change_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().text.as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().text.as_lines().len() - 1;
        let mut y = self.line(index, |line| line.y() + line.height());
        self.blocks(index, index, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: true,
                        line,
                    } => y += line.height(),
                    Block::Widget(widget) => y += widget.height,
                    _ => unreachable!(),
                }
            }
        });
        y
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Rc<RefCell<CodeDocument>> {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document.borrow();
        f(Line {
            y: self.y.get(line).copied(),
            column_count: self.column_count[line],
            fold_column: self.fold_column[line],
            scale: self.scale[line],
            text: &document.text.as_lines()[line],
            tokens: &document.tokens[line],
            inline_inlays: &document.inline_inlays[line],
            wrap_data: self.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        f(Lines {
            y: self.y[start_line.min(self.y.len())..end_line.min(self.y.len())].iter(),
            column_count: self.column_count[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
            text: document.text.as_lines()[start_line..end_line].iter(),
            tokens: document.tokens[start_line..end_line].iter(),
            inline_inlays: document.inline_inlays[start_line..end_line].iter(),
            wrap_data: self.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        let mut block_inlays = document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < start_line)
        {
            block_inlays.next();
        }
        self.lines(start_line, end_line, |lines| {
            f(Blocks {
                lines,
                block_inlays,
                position: start_line,
            })
        })
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .indentation()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.fold_column[line] =
                    self.settings.fold_level * self.settings.indent_column_count;
                self.unfolding_lines.remove(&line);
                self.folding_lines.insert(line);
            }
        }
    }

    pub fn unfold(&mut self) {
        for line in self.folding_lines.drain() {
            self.unfolding_lines.insert(line);
        }
        for line in self.folded_lines.drain() {
            self.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&mut self) -> bool {
        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let mut new_folding_lines = HashSet::new();
        for &line in &self.folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.1 + 0.001 {
                self.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, cursor: Point, affinity: Affinity) {
        self.selections.clear();
        self.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        });
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn add_cursor(&mut self, cursor: Point, affinity: Affinity) {
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        };
        self.pending_selection_index = Some(
            match self.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return cmp::Ordering::Less;
                }
                if selection.start() >= cursor {
                    return cmp::Ordering::Greater;
                }
                cmp::Ordering::Equal
            }) {
                Ok(index) => {
                    self.selections[index] = selection;
                    index
                }
                Err(index) => {
                    self.selections.insert(index, selection);
                    index
                }
            },
        );
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, cursor: Point, affinity: Affinity) {
        let mut pending_selection_index = self.pending_selection_index.unwrap();
        self.selections[pending_selection_index] = Selection {
            cursor,
            affinity,
            ..self.selections[pending_selection_index]
        };
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !self.selections[prev_selection_index]
                .should_merge(self.selections[pending_selection_index])
            {
                break;
            }
            self.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < self.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !self.selections[pending_selection_index]
                .should_merge(self.selections[next_selection_index])
            {
                break;
            }
            self.selections.remove(next_selection_index);
        }
        self.pending_selection_index = Some(pending_selection_index);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_left(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_right(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_up(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_down(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn insert(&mut self, text: Text) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Insert, &self.selections, |_, _, _| {
                (Extent::zero(), Some(text.clone()), None)
            });
    }

    pub fn enter(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Insert,
            &self.selections,
            |line, index, _| {
                (
                    if line[..index].chars().all(|char| char.is_whitespace()) {
                        Extent {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Extent::zero()
                    },
                    Some(Text::newline()),
                    if line[..index]
                        .chars()
                        .rev()
                        .find_map(|char| {
                            if char.is_opening_delimiter() {
                                return Some(true);
                            }
                            if char.is_closing_delimiter() {
                                return Some(false);
                            }
                            None
                        })
                        .unwrap_or(false)
                        && line[index..]
                            .chars()
                            .find_map(|char| {
                                if char.is_closing_delimiter() {
                                    return Some(true);
                                }
                                if !char.is_whitespace() {
                                    return Some(false);
                                }
                                None
                            })
                            .unwrap_or(false)
                    {
                        Some(Text::newline())
                    } else {
                        None
                    },
                )
            },
        );
    }

    pub fn indent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Indent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        (indentation_column_count + self.settings.indent_column_count)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Outdent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        indentation_column_count.saturating_sub(1)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn delete(&mut self) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Delete, &self.selections, |_, _, _| {
                (Extent::zero(), None, None)
            });
    }

    pub fn backspace(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            |line, index, is_empty| {
                (
                    if is_empty {
                        if index == 0 {
                            Extent {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Extent {
                                line_count: 0,
                                byte_count: line.graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Extent::zero()
                    },
                    None,
                    None,
                )
            },
        );
    }

    pub fn undo(&mut self) {
        self.document.borrow_mut().undo(self.id);
    }

    pub fn redo(&mut self) {
        self.document.borrow_mut().redo(self.id);
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().text.as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.y);
        self.blocks(start, end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line { is_inlay, line } => {
                        if !is_inlay {
                            ys.push(y);
                        }
                        y += line.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        ys.push(y);
        self.y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, changes)) = self.change_receiver.try_recv() {
            self.apply_changes(selections, &changes);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        self.line(index, |line| {
            for wrapped in line.wrappeds() {
                match wrapped {
                    Wrapped::Text { text, .. } => {
                        column += text
                            .column_count(self.settings.tab_column_count);
                    }
                    Wrapped::Widget(widget) => {
                        column += widget.column_count;
                    }
                    Wrapped::Wrap => {
                        column_count = column_count.max(column);
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        });
        self.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.wrap_data[line] = Some(wrap_data);
        self.y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(&CodeSession, Selection) -> Selection,
    ) {
        let mut selections = mem::take(&mut self.selections);
        for selection in &mut selections {
            *selection = f(&self, *selection);
            if reset_anchor {
                *selection = selection.reset_anchor();
            }
        }
        self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
                self.selections[current_selection_index] = merged_selection;
                self.selections.remove(next_selection_index);
                if let Some(pending_selection_index) = self.pending_selection_index.as_mut() {
                    if next_selection_index < *pending_selection_index {
                        *pending_selection_index -= 1;
                    }
                }
            } else {
                current_selection_index += 1;
            }
        }
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_changes(&mut self, selections: Option<Vec<Selection>>, changes: &[Change]) {
        for change in changes {
            match &change.kind {
                ChangeKind::Insert(point, text) => {
                    self.column_count[point.line] = None;
                    self.wrap_data[point.line] = None;
                    let line_count = text.extent().line_count;
                    if line_count > 0 {
                        let line = point.line + 1;
                        self.y.truncate(line);
                        self.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                ChangeKind::Delete(range) => {
                    self.column_count[range.start().line] = None;
                    self.wrap_data[range.start().line] = None;
                    let line_count = range.extent().line_count;
                    if line_count > 0 {
                        let start_line = range.start().line + 1;
                        let end_line = start_line + line_count;
                        self.y.truncate(start_line);
                        self.column_count.drain(start_line..end_line);
                        self.fold_column.drain(start_line..end_line);
                        self.scale.drain(start_line..end_line);
                        self.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for change in changes {
                for selection in &mut self.selections {
                    *selection = selection.apply_change(&change);
                }
            }
        }
        self.update_y();
    }
}

impl Drop for CodeSession {
    fn drop(&mut self) {
        self.document.borrow_mut().change_senders.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_data: Iter<'a, Option<WrapData>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct CodeDocument {
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    history: History,
    tokenizer: Tokenizer,
    change_senders: HashMap<SessionId, Sender<(Option<Vec<Selection>>, Vec<Change>)>>,
}

impl CodeDocument {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            text,
            tokens,
            inline_inlays: (0..line_count)
                .map(|line| {
                    if line % 5 == 0 {
                        [
                            (20, InlineInlay::Text("XXX".into())),
                            (40, InlineInlay::Text("XXX".into())),
                            (60, InlineInlay::Text("XXX".into())),
                            (80, InlineInlay::Text("XXX".into())),
                        ]
                        .into()
                    } else {
                        Vec::new()
                    }
                })
                .collect(),
            block_inlays: Vec::new(),
            history: History::new(),
            tokenizer: Tokenizer::new(line_count),
            change_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.text, &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&String, usize, bool) -> (Extent, Option<Text>, Option<Text>),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        let mut point = Point::zero();
        let mut prev_range_end = Point::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, range.extent())),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            let (delete_extent, insert_text_before, insert_text_after) = f(
                &self.text.as_lines()[point.line],
                point.byte,
                range.is_empty(),
            );
            if delete_extent != Extent::zero() {
                if delete_extent.line_count == 0 {
                    point.byte -= delete_extent.byte_count;
                } else {
                    point.line -= delete_extent.line_count;
                    point.byte = self.text.as_lines()[point.line].len() - delete_extent.byte_count;
                }
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, delete_extent)),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_before) = insert_text_before {
                let extent = insert_text_before.extent();
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Insert(point, insert_text_before),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_after) = insert_text_after {
                let extent = insert_text_after.extent();
                let change = Change {
                    drift: Drift::After,
                    kind: ChangeKind::Insert(point, insert_text_after),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            prev_range_end = range.end();
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut changes, &mut inverted_changes, &mut f);
            }
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.text.as_lines()[line]);
        if delete_byte_count > 0 {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Delete(Range::from_start_and_extent(
                    Point { line, byte },
                    Extent {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                )),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
        if !insert_text.is_empty() {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Insert(Point { line, byte }, insert_text.into()),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.undo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn redo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.redo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn apply_changes(
        &mut self,
        origin_id: SessionId,
        selections: Option<Vec<Selection>>,
        changes: &[Change],
    ) {
        for change in changes {
            self.apply_change_to_tokens(change);
            self.apply_change_to_inline_inlays(change);
            self.tokenizer.apply_change(change);
        }
        self.tokenizer.update(&self.text, &mut self.tokens);
        for (&session_id, change_sender) in &self.change_senders {
            if session_id == origin_id {
                change_sender
                    .send((selections.clone(), changes.to_vec()))
                    .unwrap();
            } else {
                change_sender
                    .send((
                        None,
                        changes
                            .iter()
                            .cloned()
                            .map(|change| Change {
                                drift: Drift::Before,
                                ..change
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line].len());
                if byte != point.byte {
                    let token = self.tokens[point.line][index];
                    let mid = point.byte - byte;
                    self.tokens[point.line][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.extent().line_count == 0 {
                    self.tokens[point.line]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][index..].iter().copied());
                    self.tokens.splice(point.line..point.line + 1, tokens);
                }
            }
            ChangeKind::Delete(range) => {
                let mut byte = 0;
                let mut start = self.tokens[range.start().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.start().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.start().line].len());
                if byte != range.start().byte {
                    let token = self.tokens[range.start().line][start];
                    let mid = range.start().byte - byte;
                    self.tokens[range.start().line][start] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start += 1;
                    self.tokens[range.start().line].insert(
                        start,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end = self.tokens[range.end().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.end().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.end().line].len());
                if byte != range.end().byte {
                    let token = self.tokens[range.end().line][end];
                    let mid = range.end().byte - byte;
                    self.tokens[range.end().line][end] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end += 1;
                    self.tokens[range.end().line].insert(
                        end,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if range.start().line == range.end().line {
                    self.tokens[range.start().line].drain(start..end);
                } else {
                    let mut tokens = self.tokens[range.start().line][..start]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[range.end().line][end..].iter().copied());
                    self.tokens
                        .splice(range.start().line..range.end().line + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.extent().byte_count, inline_inlay)
                            }),
                    );
                    self.inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = self.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(self.inline_inlays[range.start().line].len());
                let end = self.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(self.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    self.inline_inlays[range.start().line].drain(start..end);
                    for (byte, _) in &mut self.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[range.start().line]
                        .drain(..start)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(self.inline_inlays[range.end().line].drain(end..).map(
                        |(byte, inline_inlay)| {
                            (
                                range.start().byte + byte - range.end().byte.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    self.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}

fn reindent(
    string: &str,
    use_soft_tabs: bool,
    tab_column_count: usize,
    f: impl FnOnce(usize) -> usize,
) -> (usize, usize, String) {
    let indentation = string.indentation().unwrap_or("");
    let indentation_column_count = indentation.column_count(tab_column_count);
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(
        new_indentation_column_count,
        use_soft_tabs,
        tab_column_count,
    );
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize, use_soft_tabs: bool, tab_column_count: usize) -> String {
    let tab_count;
    let space_count;
    if use_soft_tabs {
        tab_count = 0;
        space_count = column_count;
    } else {
        tab_count = column_count / tab_column_count;
        space_count = column_count % tab_column_count;
    }
    let tabs = iter::repeat("\t").take(tab_count);
    let spaces = iter::repeat(" ").take(space_count);
    tabs.chain(spaces).collect()
}
use crate::char::CharExt;

pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indentation(&self) -> Option<&str>;
    fn longest_common_prefix(&self, other: &str) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indentation(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn longest_common_prefix(&self, other: &str) -> &str {
        &self[..self
            .char_indices()
            .zip(other.chars())
            .find(|((_, char_0), char_1)| char_0 == char_1)
            .map(|((index, _), _)| index)
            .unwrap_or_else(|| self.len().min(other.len()))]
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
    }

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices {
            graphemes: self.graphemes(),
            start: self.as_ptr() as usize,
        }
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    string: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut end = 1;
        while !self.string.is_char_boundary(end) {
            end += 1;
        }
        let (grapheme, string) = self.string.split_at(end);
        self.string = string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut start = self.string.len() - 1;
        while !self.string.is_char_boundary(start) {
            start -= 1;
        }
        let (string, grapheme) = self.string.split_at(start);
        self.string = string;
        Some(grapheme)
    }
}

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
    start: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
use {
    crate::{change, Change, Extent, Point, Range},
    std::{io, io::BufRead, iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn newline() -> Self {
        Self {
            lines: vec![String::new(), String::new()],
        }
    }

    pub fn from_buf_reader<R>(reader: R) -> io::Result<Self>
    where
        R: BufRead,
    {
        Ok(Self {
            lines: reader.lines().collect::<Result<_, _>>()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.extent() == Extent::zero()
    }

    pub fn extent(&self) -> Extent {
        Extent {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line][range.start().byte..range.end().byte].to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines.push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
        }
        Text { lines }
    }

    pub fn insert(&mut self, point: Point, mut text: Self) {
        if text.extent().line_count == 0 {
            self.lines[point.line]
                .replace_range(point.byte..point.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line][..point.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line][point.byte..]);
            self.lines.splice(point.line..point.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, range: Range) {
        if range.start().line == range.end().line {
            self.lines[range.start().line].replace_range(range.start().byte..range.end().byte, "");
        } else {
            let mut line = self.lines[range.start().line][..range.start().byte].to_string();
            line.push_str(&self.lines[range.end().line][range.end().byte..]);
            self.lines
                .splice(range.start().line..range.end().line + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change.kind {
            change::ChangeKind::Insert(point, additional_text) => {
                self.insert(point, additional_text)
            }
            change::ChangeKind::Delete(range) => self.delete(range),
        }
    }

    pub fn into_line_count(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        Self {
            lines: string.lines().map(|string| string.to_owned()).collect(),
        }
    }
}

impl From<&String> for Text {
    fn from(string: &String) -> Self {
        string.as_str().into()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Whitespace,
}
use crate::{change::ChangeKind, token::TokenKind, Change, Text, Token};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
}

impl Tokenizer {
    pub fn new(line_count: usize) -> Self {
        Self {
            state: (0..line_count).map(|_| None).collect(),
        }
    }

    pub fn apply_change(&mut self, change: &Change) {
        match &change.kind {
            ChangeKind::Insert(point, text) => {
                self.state[point.line] = None;
                let line_count = text.extent().line_count;
                if line_count > 0 {
                    let line = point.line + 1;
                    self.state.splice(line..line, (0..line_count).map(|_| None));
                }
            }
            ChangeKind::Delete(range) => {
                self.state[range.start().line] = None;
                let line_count = range.extent().line_count;
                if line_count > 0 {
                    let start_line = range.start().line + 1;
                    let end_line = start_line + line_count;
                    self.state.drain(start_line..end_line);
                }
            }
        }
    }

    pub fn update(&mut self, text: &Text, tokens: &mut [Vec<Token>]) {
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut new_tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => new_tokens.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    tokens[line] = new_tokens;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<Token>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('!', '=', _)
            | ('%', '=', _)
            | ('&', '&', _)
            | ('&', '=', _)
            | ('*', '=', _)
            | ('+', '=', _)
            | ('-', '=', _)
            | ('-', '>', _)
            | ('.', '.', _)
            | ('/', '=', _)
            | (':', ':', _)
            | ('<', '<', _)
            | ('<', '=', _)
            | ('=', '=', _)
            | ('=', '>', _)
            | ('>', '=', _)
            | ('>', '>', _)
            | ('^', '=', _)
            | ('|', '=', _)
            | ('|', '|', _) => {
                cursor.skip(2);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            ('.', char, _) if char.is_digit(10) => self.number(cursor),
            ('!', _, _)
            | ('#', _, _)
            | ('$', _, _)
            | ('%', _, _)
            | ('&', _, _)
            | ('*', _, _)
            | ('+', _, _)
            | (',', _, _)
            | ('-', _, _)
            | ('.', _, _)
            | ('/', _, _)
            | (':', _, _)
            | (';', _, _)
            | ('<', _, _)
            | ('=', _, _)
            | ('>', _, _)
            | ('?', _, _)
            | ('@', _, _)
            | ('^', _, _)
            | ('_', _, _)
            | ('|', _, _) => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            (char, _, _) if char.is_identifier_start() => self.identifier_or_keyword(cursor),
            (char, _, _) if char.is_digit(10) => self.number(cursor),
            (char, _, _) if char.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index;
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_identifier_continue()) {}
        let end = cursor.index;

        (
            State::Initial(InitialState),
            match &cursor.string[start..end] {
                "else" | "if" | "match" | "return" => TokenKind::BranchKeyword,
                "break" | "continue" | "for" | "loop" | "while" => TokenKind::LoopKeyword,
                "Self" | "as" | "async" | "await" | "const" | "crate" | "dyn" | "enum"
                | "extern" | "false" | "fn" | "impl" | "in" | "let" | "mod" | "move" | "mut"
                | "pub" | "ref" | "self" | "static" | "struct" | "super" | "trait" | "true"
                | "type" | "unsafe" | "use" | "where" => TokenKind::OtherKeyword,
                _ => TokenKind::Identifier,
            },
        )
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    _ => {
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                }
            }
        };
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    string: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Cursor { string, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.string[self.index..].chars().nth(index).unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index = self.string[self.index..]
            .char_indices()
            .nth(count)
            .map_or(self.string.len(), |(index, _)| self.index + index);
    }

    fn skip_if<P>(&mut self, predicate: P) -> bool
    where
        P: FnOnce(char) -> bool,
    {
        if predicate(self.peek(0)) {
            self.skip(1);
            true
        } else {
            false
        }
    }

    fn skip_exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(0) == '-' {
            self.skip(1);
        }
        self.skip_digits(10)
    }

    fn skip_digits(&mut self, radix: u32) -> bool {
        let mut has_skip_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                char if char.is_digit(radix) => {
                    self.skip(1);
                    has_skip_digits = true;
                }
                _ => break,
            }
        }
        has_skip_digits
    }

    fn skip_suffix(&mut self) -> bool {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if(|char| char.is_identifier_continue()) {}
            return true;
        }
        false
    }
}

pub trait CharExt {
    fn is_identifier_start(self) -> bool;
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub height: f64,
}
use crate::{char::CharExt, line::Inline, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize, tab_column_count: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .indentation()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count(tab_column_count))
        .sum();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            Inline::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte = 0;
    let mut column = 0;
    let mut wraps = Vec::new();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if column + column_count > wrap_column {
                        column = indent_column_count;
                        wraps.push(byte);
                    }
                    column += column_count;
                    byte += string.len();
                }
            }
            Inline::Widget(widget) => {
                if column + widget.column_count > wrap_column {
                    column = indent_column_count;
                    wraps.push(byte);
                }
                column += widget.column_count;
                byte += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}
use {
    makepad_code_editor::{
        code_editor::*,
        state::{CodeDocument, CodeSession},
    },
    makepad_widgets::*,
    std::{cell::RefCell, rc::Rc},
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_code_editor::code_editor::CodeEditor;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <CodeEditor> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    code_editor.draw(&mut cx, &mut self.state.session);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        if let Some(mut code_editor) = self.ui.get_code_editor(id!(code_editor)).borrow_mut() {
            code_editor.handle_event(cx, event, &mut self.state.session);
        }
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_code_editor::code_editor::live_design(cx);
    }
}

struct State {
    session: CodeSession,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: CodeSession::new(Rc::new(RefCell::new(CodeDocument::new(
                include_str!("state.rs").into(),
            )))),
        }
    }
}

app_main!(App);
use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => {
                    ChangeKind::Insert(range.start(), text.slice(range))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{
        line::Wrapped,
        selection::Affinity,
        state::{Block, CodeSession},
        str::StrExt,
        token::TokenKind,
        Line, Point, Selection, Token,
    },
    makepad_widgets::*,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    TokenColors = {{TokenColors}} {
        unknown: #808080,
        branch_keyword: #C485BE,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        whitespace: #6E6E6E,
    }

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        
            width: Fill,
            height: Fill,
            margin: 0,
        
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
    #[live]
    draw_text: DrawText,
    #[live]
    token_colors: TokenColors,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start: usize,
    #[rust]
    end: usize,
}

impl LiveHook for CodeEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, CodeEditor)
    }
}

impl Widget for CodeEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }

    fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        //let uid = self.widget_uid();
        /*self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });*/
        //self.handle_event
    }

    fn walk(&self) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CodeEditorRef(WidgetRef);

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, session: &mut CodeSession) {
        let walk = self.draw_state.get().unwrap();

        self.scroll_bars.begin(cx, walk, Layout::default());

        self.viewport_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();

        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        session.handle_changes();
        session.set_wrap_column(Some(
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        ));
        self.start = session.find_first_line_ending_after_y(scroll_pos.y / self.cell_size.y);
        self.end = session.find_first_line_starting_after_y(
            (scroll_pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );

        self.draw_text(cx, session);
        self.draw_selections(cx, session);
        cx.turtle_mut().set_used(
            session.width() * self.cell_size.x,
            session.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if session.update_folds() {
            cx.redraw_all();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, session: &mut CodeSession) {
        session.handle_changes();
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.fold();
                cx.redraw_all();
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.unfold();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_left(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_right(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_up(!shift);
                cx.redraw_all();
            }

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_down(!shift);
                cx.redraw_all();
            }
            Hit::TextInput(TextInputEvent { ref input, .. }) => {
                session.insert(input.into());
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                session.enter();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::RBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.indent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::LBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.outdent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                session.delete();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.backspace();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: false, .. },
                ..
            }) => {
                session.undo();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: true, .. },
                ..
            }) => {
                session.redo();
                cx.redraw_all();
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                cx.set_key_focus(self.scroll_bars.area());
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    if alt {
                        session.add_cursor(cursor, affinity);
                    } else {
                        session.set_cursor(cursor, affinity);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    session.move_to(cursor, affinity);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut y = 0.0;
        session.blocks(
            0,
            session.document().borrow().text().as_lines().len(),
            |blocks| {
                for block in blocks {
                    match block {
                        Block::Line { line, .. } => {
                            self.draw_text.font_scale = line.scale();
                            let mut token_iter = line.tokens().iter().copied();
                            let mut token_slot = token_iter.next();
                            let mut column = 0;
                            for wrapped in line.wrappeds() {
                                match wrapped {
                                    Wrapped::Text {
                                        is_inlay: false,
                                        mut text,
                                    } => {
                                        while !text.is_empty() {
                                            let token = match token_slot {
                                                Some(token) => {
                                                    if text.len() < token.len {
                                                        token_slot = Some(Token {
                                                            len: token.len - text.len(),
                                                            kind: token.kind,
                                                        });
                                                        Token {
                                                            len: text.len(),
                                                            kind: token.kind,
                                                        }
                                                    } else {
                                                        token_slot = token_iter.next();
                                                        token
                                                    }
                                                }
                                                None => Token {
                                                    len: text.len(),
                                                    kind: TokenKind::Unknown,
                                                },
                                            };
                                            let (text_0, text_1) = text.split_at(token.len);
                                            text = text_1;
                                            self.draw_text.color = match token.kind {
                                                TokenKind::Unknown => self.token_colors.unknown,
                                                TokenKind::BranchKeyword => {
                                                    self.token_colors.branch_keyword
                                                }
                                                TokenKind::Identifier => {
                                                    self.token_colors.identifier
                                                }
                                                TokenKind::LoopKeyword => {
                                                    self.token_colors.loop_keyword
                                                }
                                                TokenKind::Number => self.token_colors.number,
                                                TokenKind::OtherKeyword => {
                                                    self.token_colors.other_keyword
                                                }
                                                TokenKind::Punctuator => {
                                                    self.token_colors.punctuator
                                                }
                                                TokenKind::Whitespace => {
                                                    self.token_colors.whitespace
                                                }
                                            };
                                            self.draw_text.draw_abs(
                                                cx,
                                                DVec2 {
                                                    x: line.column_to_x(column),
                                                    y,
                                                } * self.cell_size
                                                    + self.viewport_rect.pos,
                                                text_0,
                                            );
                                            column += text_0
                                                .column_count(session.settings().tab_column_count);
                                        }
                                    }
                                    Wrapped::Text {
                                        is_inlay: true,
                                        text,
                                    } => {
                                        self.draw_text.draw_abs(
                                            cx,
                                            DVec2 {
                                                x: line.column_to_x(column),
                                                y,
                                            } * self.cell_size
                                                + self.viewport_rect.pos,
                                            text,
                                        );
                                        column +=
                                            text.column_count(session.settings().tab_column_count);
                                    }
                                    Wrapped::Widget(widget) => {
                                        column += widget.column_count;
                                    }
                                    Wrapped::Wrap => {
                                        column = line.wrap_indent_column_count();
                                        y += line.scale();
                                    }
                                }
                            }
                            y += line.scale();
                        }
                        Block::Widget(widget) => {
                            y += widget.height;
                        }
                    }
                }
            },
        );
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, session: &CodeSession) {
        let mut active_selection = None;
        let mut selections = session.selections().iter();
        while selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.end().line < self.start)
        {
            selections.next().unwrap();
        }
        if selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.start().line < self.start)
        {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelections {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, session)
    }

    fn pick(&self, session: &CodeSession, point: DVec2) -> Option<(Point, Affinity)> {
        let point = (point - self.viewport_rect.pos) / self.cell_size;
        let mut line = session.find_first_line_ending_after_y(point.y);
        let mut y = session.line(line, |line| line.y());
        session.blocks(line, line + 1, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        let next_byte = byte + grapheme.len();
                                        let next_column = column
                                            + grapheme
                                                .column_count(session.settings().tab_column_count);
                                        let next_y = y + line_ref.scale();
                                        let x = line_ref.column_to_x(column);
                                        let next_x = line_ref.column_to_x(next_column);
                                        let mid_x = (x + next_x) / 2.0;
                                        if (y..=next_y).contains(&point.y) {
                                            if (x..=mid_x).contains(&point.x) {
                                                return Some((
                                                    Point { line, byte },
                                                    Affinity::After,
                                                ));
                                            }
                                            if (mid_x..=next_x).contains(&point.x) {
                                                return Some((
                                                    Point {
                                                        line,
                                                        byte: next_byte,
                                                    },
                                                    Affinity::Before,
                                                ));
                                            }
                                        }
                                        byte = next_byte;
                                        column = next_column;
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    let next_column = column
                                        + text.column_count(session.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    if (y..=next_y).contains(&point.y)
                                        && (x..=next_x).contains(&point.x)
                                    {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = next_column;
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    let next_y = y + line_ref.scale();
                                    if (y..=next_y).contains(&point.y) {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y = next_y;
                                }
                            }
                        }
                        let next_y = y + line_ref.scale();
                        if (y..=y + next_y).contains(&point.y) {
                            return Some((Point { line, byte }, Affinity::After));
                        }
                        line += 1;
                        y = next_y;
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        let next_y = y + line_ref.height();
                        if (y..=next_y).contains(&point.y) {
                            return Some((Point { line, byte: 0 }, Affinity::Before));
                        }
                        y = next_y;
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
            None
        })
    }
}

struct DrawSelections<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelections<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut line = self.code_editor.start;
        let mut y = session.line(line, |line| line.y());
        session.blocks(self.code_editor.start, self.code_editor.end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        self.handle_event(cx, line, line_ref, byte, Affinity::Before, y, column);
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::After,
                                            y,
                                            column,
                                        );
                                        byte += grapheme.len();
                                        column += grapheme
                                            .column_count(session.settings().tab_column_count);
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::Before,
                                            y,
                                            column,
                                        );
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    column +=
                                        text.column_count(session.settings().tab_column_count);
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    if self.active_selection.is_some() {
                                        self.draw_selection(cx, line_ref, y, column);
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y += line_ref.scale();
                                }
                            }
                        }
                        self.handle_event(cx, line, line_ref, byte, Affinity::After, y, column);
                        column += 1;
                        if self.active_selection.is_some() {
                            self.draw_selection(cx, line_ref, y, column);
                        }
                        line += 1;
                        y += line_ref.scale();
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        y += line_ref.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line: usize,
        line_ref: Line<'_>,
        byte: usize,
        affinity: Affinity,
        y: f64,
        column: usize,
    ) {
        let point = Point { line, byte };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == point && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line_ref, y, column);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == point && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: line_ref.column_to_x(column),
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, line: Line<'_>, y: f64, column: usize) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: line.column_to_x(column) - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column: usize) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: line.column_to_x(column),
                    y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    whitespace: Vec4,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
            Self {
                line_count: 0,
                byte_count: self.byte_count - other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count - other.line_count,
                byte_count: self.byte_count,
            }
        }
    }
}

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
use crate::{state::SessionId, Change, Selection, Text};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    current_edit: Option<(SessionId, EditKind)>,
    undos: Vec<(Vec<Selection>, Vec<Change>)>,
    redos: Vec<(Vec<Selection>, Vec<Change>)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit = None;
    }

    pub fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        inverted_changes: Vec<Change>,
    ) {
        if self
            .current_edit
            .map_or(false, |current_edit| current_edit == (origin_id, kind))
        {
            self.undos.last_mut().unwrap().1.extend(inverted_changes);
        } else {
            self.current_edit = Some((origin_id, kind));
            self.undos.push((selections.to_vec(), inverted_changes));
        }
        self.redos.clear();
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, mut inverted_changes)) = self.undos.pop() {
            self.current_edit = None;
            let mut changes = Vec::new();
            inverted_changes.reverse();
            for inverted_change in inverted_changes.iter().cloned() {
                let change = inverted_change.clone().invert(&text);
                text.apply_change(inverted_change);
                changes.push(change);
            }
            self.redos.push((selections.clone(), changes.clone()));
            Some((selections, inverted_changes))
        } else {
            None
        }
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, changes)) = self.redos.pop() {
            self.current_edit = None;
            for change in changes.iter().cloned() {
                text.apply_change(change);
            }
            self.undos.push((selections.clone(), changes.clone()));
            Some((selections, changes))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EditGroup {
    pub selections: Vec<Selection>,
    pub changes: Vec<Change>,
}
use crate::widgets::{BlockWidget, InlineWidget};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Widget(BlockWidget),
}
pub trait IteratorExt: Iterator {
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>;
}

impl<T> IteratorExt for T
where
    T: Iterator,
{
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>,
    {
        Merge {
            prev_item: None,
            iter: self,
            f,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Merge<I, F>
where
    I: Iterator,
{
    prev_item: Option<I::Item>,
    iter: I,
    f: F,
}

impl<I, F> Iterator for Merge<I, F>
where
    I: Iterator,
    F: FnMut(I::Item, I::Item) -> Result<I::Item, (I::Item, I::Item)>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev_item.take(), self.iter.next()) {
                (Some(prev_item), Some(item)) => match (self.f)(prev_item, item) {
                    Ok(merged_item) => {
                        self.prev_item = Some(merged_item);
                        continue;
                    }
                    Err((prev_item, item)) => {
                        self.prev_item = Some(item);
                        break Some(prev_item);
                    }
                },
                (None, Some(item)) => {
                    self.prev_item = Some(item);
                    continue;
                }
                (Some(prev_item), None) => break Some(prev_item),
                (None, None) => break None,
            }
        }
    }
}
pub use makepad_widgets;
use makepad_widgets::*;

pub mod change;
pub mod char;
pub mod code_editor;
pub mod extent;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod line;
pub mod move_ops;
pub mod point;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    change::Change,
    code_editor::CodeEditor,
    extent::Extent,
    history::History,
    line::Line,
    point::Point,
    range::Range,
    selection::Selection,
    settings::Settings,
    state::{CodeDocument, CodeSession},
    text::Text,
    token::Token,
    tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
use {
    crate::{
        inlays::InlineInlay, selection::Affinity, str::StrExt, widgets::InlineWidget,
        wrap::WrapData, Token,
    },
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_data.unwrap().wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn byte_and_affinity_to_row_and_column(
        &self,
        byte: usize,
        affinity: Affinity,
        tab_column_count: usize,
    ) -> (usize, usize) {
        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if current_byte == byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte == byte && affinity == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if current_byte == byte && affinity == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    column += text.column_count(tab_column_count);
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    row += 1;
                    column = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte == byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_and_column_to_byte_and_affinity(
        &self,
        row: usize,
        column: usize,
        tab_width: usize,
    ) -> (usize, Affinity) {
        let mut current_row = 0;
        let mut current_column = 0;
        let mut byte = 0;
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_width);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte += grapheme.len();
                        current_column = next_column;
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column + text.column_count(tab_width);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                Wrapped::Widget(widget) => {
                    current_column += widget.column_count;
                }
                Wrapped::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.wrap_indent_column_count();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn wrap_indent_column_count(self) -> usize {
        self.wrap_data.unwrap().indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            position: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) position: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.position);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.position += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) position: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    mid = mid.min(position - self.position);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.position += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.position += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
mod app;

fn main() {
    app::app_main();
}
use crate::{selection::Affinity, str::StrExt, Point, CodeSession};

pub fn move_left(lines: &[String], point: Point) -> Point {
    if !is_at_start_of_line(point) {
        return move_to_prev_grapheme(lines, point);
    }
    if !is_at_first_line(point) {
        return move_to_end_of_prev_line(lines, point);
    }
    point
}

pub fn move_right(lines: &[String], point: Point) -> Point {
    if !is_at_end_of_line(lines, point) {
        return move_to_next_grapheme(lines, point);
    }
    if !is_at_last_line(lines, point) {
        return move_to_start_of_next_line(point);
    }
    point
}

pub fn move_up(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_first_row_of_line(session, point, affinity) {
        return move_to_prev_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_first_line(point) {
        return move_to_last_row_of_prev_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

pub fn move_down(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_last_row_of_line(session, point, affinity) {
        return move_to_next_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_last_line(session.document().borrow().text().as_lines(), point) {
        return move_to_first_row_of_next_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

fn is_at_first_line(point: Point) -> bool {
    point.line == 0
}

fn is_at_last_line(lines: &[String], point: Point) -> bool {
    point.line == lines.len()
}

fn is_at_start_of_line(point: Point) -> bool {
    point.byte == 0
}

fn is_at_end_of_line(lines: &[String], point: Point) -> bool {
    point.byte == lines[point.line].len()
}

fn is_at_first_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(lines: &[String], point: Point) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], point: Point) -> Point {
    let line = &lines[point.line];
    Point {
        line: point.line,
        byte: line[point.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| point.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], point: Point) -> Point {
    let prev_line = point.line - 1;
    Point {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(point: Point) -> Point {
    Point {
        line: point.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_next_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row + 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_last_row_of_prev_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line - 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}

fn move_to_first_row_of_next_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line + 1, |next_line| {
            let (byte, affinity) = next_line.row_and_column_to_byte_and_affinity(
                0,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line + 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}
use {
    crate::{
        change::{ChangeKind, Drift},
        Change, Extent,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.extent() + (self - point),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Extent, Point};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}
use {
    crate::{Change, Extent, Point, Range},
    std::ops,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Point {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Point {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Extent {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line..self.cursor.line + 1
        } else {
            self.cursor.line..if self.anchor.byte == 0 {
                self.anchor.line
            } else {
                self.anchor.line + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Point, Affinity, Option<usize>) -> (Point, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change),
            cursor: self.cursor.apply_change(change),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub use_soft_tabs: bool,
    pub tab_column_count: usize,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_soft_tabs: true,
            tab_column_count: 4,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
use {
    crate::{
        change::{ChangeKind, Drift},
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::Affinity,
        str::StrExt,
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        Change, Extent, History, Line, Point, Range, Selection, Settings, Text, Token, Tokenizer,
    },
    std::{
        cell::RefCell,
        cmp,
        collections::{HashMap, HashSet},
        iter, mem,
        rc::Rc,
        slice::Iter,
        sync::{
            atomic,
            atomic::AtomicUsize,
            mpsc,
            mpsc::{Receiver, Sender},
        },
    },
};

#[derive(Debug)]
pub struct CodeSession {
    id: SessionId,
    settings: Rc<Settings>,
    document: Rc<RefCell<CodeDocument>>,
    wrap_column: Option<usize>,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
    change_receiver: Receiver<(Option<Vec<Selection>>, Vec<Change>)>,
}

impl CodeSession {
    pub fn new(document: Rc<RefCell<CodeDocument>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (change_sender, change_receiver) = mpsc::channel();
        let line_count = document.borrow().text.as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wrap_data: (0..line_count).map(|_| None).collect(),
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
            change_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .change_senders
            .insert(session.id, change_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().text.as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().text.as_lines().len() - 1;
        let mut y = self.line(index, |line| line.y() + line.height());
        self.blocks(index, index, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: true,
                        line,
                    } => y += line.height(),
                    Block::Widget(widget) => y += widget.height,
                    _ => unreachable!(),
                }
            }
        });
        y
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Rc<RefCell<CodeDocument>> {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document.borrow();
        f(Line {
            y: self.y.get(line).copied(),
            column_count: self.column_count[line],
            fold_column: self.fold_column[line],
            scale: self.scale[line],
            text: &document.text.as_lines()[line],
            tokens: &document.tokens[line],
            inline_inlays: &document.inline_inlays[line],
            wrap_data: self.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        f(Lines {
            y: self.y[start_line.min(self.y.len())..end_line.min(self.y.len())].iter(),
            column_count: self.column_count[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
            text: document.text.as_lines()[start_line..end_line].iter(),
            tokens: document.tokens[start_line..end_line].iter(),
            inline_inlays: document.inline_inlays[start_line..end_line].iter(),
            wrap_data: self.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        let mut block_inlays = document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < start_line)
        {
            block_inlays.next();
        }
        self.lines(start_line, end_line, |lines| {
            f(Blocks {
                lines,
                block_inlays,
                position: start_line,
            })
        })
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .indentation()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.fold_column[line] =
                    self.settings.fold_level * self.settings.indent_column_count;
                self.unfolding_lines.remove(&line);
                self.folding_lines.insert(line);
            }
        }
    }

    pub fn unfold(&mut self) {
        for line in self.folding_lines.drain() {
            self.unfolding_lines.insert(line);
        }
        for line in self.folded_lines.drain() {
            self.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&mut self) -> bool {
        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let mut new_folding_lines = HashSet::new();
        for &line in &self.folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.1 + 0.001 {
                self.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, cursor: Point, affinity: Affinity) {
        self.selections.clear();
        self.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        });
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn add_cursor(&mut self, cursor: Point, affinity: Affinity) {
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        };
        self.pending_selection_index = Some(
            match self.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return cmp::Ordering::Less;
                }
                if selection.start() >= cursor {
                    return cmp::Ordering::Greater;
                }
                cmp::Ordering::Equal
            }) {
                Ok(index) => {
                    self.selections[index] = selection;
                    index
                }
                Err(index) => {
                    self.selections.insert(index, selection);
                    index
                }
            },
        );
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, cursor: Point, affinity: Affinity) {
        let mut pending_selection_index = self.pending_selection_index.unwrap();
        self.selections[pending_selection_index] = Selection {
            cursor,
            affinity,
            ..self.selections[pending_selection_index]
        };
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !self.selections[prev_selection_index]
                .should_merge(self.selections[pending_selection_index])
            {
                break;
            }
            self.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < self.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !self.selections[pending_selection_index]
                .should_merge(self.selections[next_selection_index])
            {
                break;
            }
            self.selections.remove(next_selection_index);
        }
        self.pending_selection_index = Some(pending_selection_index);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_left(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_right(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_up(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_down(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn insert(&mut self, text: Text) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Insert, &self.selections, |_, _, _| {
                (Extent::zero(), Some(text.clone()), None)
            });
    }

    pub fn enter(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Insert,
            &self.selections,
            |line, index, _| {
                (
                    if line[..index].chars().all(|char| char.is_whitespace()) {
                        Extent {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Extent::zero()
                    },
                    Some(Text::newline()),
                    if line[..index]
                        .chars()
                        .rev()
                        .find_map(|char| {
                            if char.is_opening_delimiter() {
                                return Some(true);
                            }
                            if char.is_closing_delimiter() {
                                return Some(false);
                            }
                            None
                        })
                        .unwrap_or(false)
                        && line[index..]
                            .chars()
                            .find_map(|char| {
                                if char.is_closing_delimiter() {
                                    return Some(true);
                                }
                                if !char.is_whitespace() {
                                    return Some(false);
                                }
                                None
                            })
                            .unwrap_or(false)
                    {
                        Some(Text::newline())
                    } else {
                        None
                    },
                )
            },
        );
    }

    pub fn indent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Indent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        (indentation_column_count + self.settings.indent_column_count)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Outdent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        indentation_column_count.saturating_sub(1)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn delete(&mut self) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Delete, &self.selections, |_, _, _| {
                (Extent::zero(), None, None)
            });
    }

    pub fn backspace(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            |line, index, is_empty| {
                (
                    if is_empty {
                        if index == 0 {
                            Extent {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Extent {
                                line_count: 0,
                                byte_count: line.graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Extent::zero()
                    },
                    None,
                    None,
                )
            },
        );
    }

    pub fn undo(&mut self) {
        self.document.borrow_mut().undo(self.id);
    }

    pub fn redo(&mut self) {
        self.document.borrow_mut().redo(self.id);
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().text.as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.y);
        self.blocks(start, end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line { is_inlay, line } => {
                        if !is_inlay {
                            ys.push(y);
                        }
                        y += line.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        ys.push(y);
        self.y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, changes)) = self.change_receiver.try_recv() {
            self.apply_changes(selections, &changes);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        self.line(index, |line| {
            for wrapped in line.wrappeds() {
                match wrapped {
                    Wrapped::Text { text, .. } => {
                        column += text
                            .column_count(self.settings.tab_column_count);
                    }
                    Wrapped::Widget(widget) => {
                        column += widget.column_count;
                    }
                    Wrapped::Wrap => {
                        column_count = column_count.max(column);
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        });
        self.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.wrap_data[line] = Some(wrap_data);
        self.y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(&CodeSession, Selection) -> Selection,
    ) {
        let mut selections = mem::take(&mut self.selections);
        for selection in &mut selections {
            *selection = f(&self, *selection);
            if reset_anchor {
                *selection = selection.reset_anchor();
            }
        }
        self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
                self.selections[current_selection_index] = merged_selection;
                self.selections.remove(next_selection_index);
                if let Some(pending_selection_index) = self.pending_selection_index.as_mut() {
                    if next_selection_index < *pending_selection_index {
                        *pending_selection_index -= 1;
                    }
                }
            } else {
                current_selection_index += 1;
            }
        }
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_changes(&mut self, selections: Option<Vec<Selection>>, changes: &[Change]) {
        for change in changes {
            match &change.kind {
                ChangeKind::Insert(point, text) => {
                    self.column_count[point.line] = None;
                    self.wrap_data[point.line] = None;
                    let line_count = text.extent().line_count;
                    if line_count > 0 {
                        let line = point.line + 1;
                        self.y.truncate(line);
                        self.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                ChangeKind::Delete(range) => {
                    self.column_count[range.start().line] = None;
                    self.wrap_data[range.start().line] = None;
                    let line_count = range.extent().line_count;
                    if line_count > 0 {
                        let start_line = range.start().line + 1;
                        let end_line = start_line + line_count;
                        self.y.truncate(start_line);
                        self.column_count.drain(start_line..end_line);
                        self.fold_column.drain(start_line..end_line);
                        self.scale.drain(start_line..end_line);
                        self.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for change in changes {
                for selection in &mut self.selections {
                    *selection = selection.apply_change(&change);
                }
            }
        }
        self.update_y();
    }
}

impl Drop for CodeSession {
    fn drop(&mut self) {
        self.document.borrow_mut().change_senders.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_data: Iter<'a, Option<WrapData>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct CodeDocument {
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    history: History,
    tokenizer: Tokenizer,
    change_senders: HashMap<SessionId, Sender<(Option<Vec<Selection>>, Vec<Change>)>>,
}

impl CodeDocument {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            text,
            tokens,
            inline_inlays: (0..line_count)
                .map(|line| {
                    if line % 5 == 0 {
                        [
                            (20, InlineInlay::Text("XXX".into())),
                            (40, InlineInlay::Text("XXX".into())),
                            (60, InlineInlay::Text("XXX".into())),
                            (80, InlineInlay::Text("XXX".into())),
                        ]
                        .into()
                    } else {
                        Vec::new()
                    }
                })
                .collect(),
            block_inlays: Vec::new(),
            history: History::new(),
            tokenizer: Tokenizer::new(line_count),
            change_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.text, &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&String, usize, bool) -> (Extent, Option<Text>, Option<Text>),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        let mut point = Point::zero();
        let mut prev_range_end = Point::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, range.extent())),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            let (delete_extent, insert_text_before, insert_text_after) = f(
                &self.text.as_lines()[point.line],
                point.byte,
                range.is_empty(),
            );
            if delete_extent != Extent::zero() {
                if delete_extent.line_count == 0 {
                    point.byte -= delete_extent.byte_count;
                } else {
                    point.line -= delete_extent.line_count;
                    point.byte = self.text.as_lines()[point.line].len() - delete_extent.byte_count;
                }
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, delete_extent)),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_before) = insert_text_before {
                let extent = insert_text_before.extent();
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Insert(point, insert_text_before),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_after) = insert_text_after {
                let extent = insert_text_after.extent();
                let change = Change {
                    drift: Drift::After,
                    kind: ChangeKind::Insert(point, insert_text_after),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            prev_range_end = range.end();
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut changes, &mut inverted_changes, &mut f);
            }
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.text.as_lines()[line]);
        if delete_byte_count > 0 {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Delete(Range::from_start_and_extent(
                    Point { line, byte },
                    Extent {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                )),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
        if !insert_text.is_empty() {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Insert(Point { line, byte }, insert_text.into()),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.undo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn redo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.redo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn apply_changes(
        &mut self,
        origin_id: SessionId,
        selections: Option<Vec<Selection>>,
        changes: &[Change],
    ) {
        for change in changes {
            self.apply_change_to_tokens(change);
            self.apply_change_to_inline_inlays(change);
            self.tokenizer.apply_change(change);
        }
        self.tokenizer.update(&self.text, &mut self.tokens);
        for (&session_id, change_sender) in &self.change_senders {
            if session_id == origin_id {
                change_sender
                    .send((selections.clone(), changes.to_vec()))
                    .unwrap();
            } else {
                change_sender
                    .send((
                        None,
                        changes
                            .iter()
                            .cloned()
                            .map(|change| Change {
                                drift: Drift::Before,
                                ..change
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line].len());
                if byte != point.byte {
                    let token = self.tokens[point.line][index];
                    let mid = point.byte - byte;
                    self.tokens[point.line][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.extent().line_count == 0 {
                    self.tokens[point.line]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][index..].iter().copied());
                    self.tokens.splice(point.line..point.line + 1, tokens);
                }
            }
            ChangeKind::Delete(range) => {
                let mut byte = 0;
                let mut start = self.tokens[range.start().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.start().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.start().line].len());
                if byte != range.start().byte {
                    let token = self.tokens[range.start().line][start];
                    let mid = range.start().byte - byte;
                    self.tokens[range.start().line][start] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start += 1;
                    self.tokens[range.start().line].insert(
                        start,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end = self.tokens[range.end().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.end().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.end().line].len());
                if byte != range.end().byte {
                    let token = self.tokens[range.end().line][end];
                    let mid = range.end().byte - byte;
                    self.tokens[range.end().line][end] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end += 1;
                    self.tokens[range.end().line].insert(
                        end,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if range.start().line == range.end().line {
                    self.tokens[range.start().line].drain(start..end);
                } else {
                    let mut tokens = self.tokens[range.start().line][..start]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[range.end().line][end..].iter().copied());
                    self.tokens
                        .splice(range.start().line..range.end().line + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.extent().byte_count, inline_inlay)
                            }),
                    );
                    self.inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = self.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(self.inline_inlays[range.start().line].len());
                let end = self.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(self.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    self.inline_inlays[range.start().line].drain(start..end);
                    for (byte, _) in &mut self.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[range.start().line]
                        .drain(..start)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(self.inline_inlays[range.end().line].drain(end..).map(
                        |(byte, inline_inlay)| {
                            (
                                range.start().byte + byte - range.end().byte.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    self.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}

fn reindent(
    string: &str,
    use_soft_tabs: bool,
    tab_column_count: usize,
    f: impl FnOnce(usize) -> usize,
) -> (usize, usize, String) {
    let indentation = string.indentation().unwrap_or("");
    let indentation_column_count = indentation.column_count(tab_column_count);
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(
        new_indentation_column_count,
        use_soft_tabs,
        tab_column_count,
    );
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize, use_soft_tabs: bool, tab_column_count: usize) -> String {
    let tab_count;
    let space_count;
    if use_soft_tabs {
        tab_count = 0;
        space_count = column_count;
    } else {
        tab_count = column_count / tab_column_count;
        space_count = column_count % tab_column_count;
    }
    let tabs = iter::repeat("\t").take(tab_count);
    let spaces = iter::repeat(" ").take(space_count);
    tabs.chain(spaces).collect()
}
use crate::char::CharExt;

pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indentation(&self) -> Option<&str>;
    fn longest_common_prefix(&self, other: &str) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indentation(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn longest_common_prefix(&self, other: &str) -> &str {
        &self[..self
            .char_indices()
            .zip(other.chars())
            .find(|((_, char_0), char_1)| char_0 == char_1)
            .map(|((index, _), _)| index)
            .unwrap_or_else(|| self.len().min(other.len()))]
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
    }

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices {
            graphemes: self.graphemes(),
            start: self.as_ptr() as usize,
        }
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    string: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut end = 1;
        while !self.string.is_char_boundary(end) {
            end += 1;
        }
        let (grapheme, string) = self.string.split_at(end);
        self.string = string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut start = self.string.len() - 1;
        while !self.string.is_char_boundary(start) {
            start -= 1;
        }
        let (string, grapheme) = self.string.split_at(start);
        self.string = string;
        Some(grapheme)
    }
}

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
    start: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
use {
    crate::{change, Change, Extent, Point, Range},
    std::{io, io::BufRead, iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn newline() -> Self {
        Self {
            lines: vec![String::new(), String::new()],
        }
    }

    pub fn from_buf_reader<R>(reader: R) -> io::Result<Self>
    where
        R: BufRead,
    {
        Ok(Self {
            lines: reader.lines().collect::<Result<_, _>>()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.extent() == Extent::zero()
    }

    pub fn extent(&self) -> Extent {
        Extent {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line][range.start().byte..range.end().byte].to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines.push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
        }
        Text { lines }
    }

    pub fn insert(&mut self, point: Point, mut text: Self) {
        if text.extent().line_count == 0 {
            self.lines[point.line]
                .replace_range(point.byte..point.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line][..point.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line][point.byte..]);
            self.lines.splice(point.line..point.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, range: Range) {
        if range.start().line == range.end().line {
            self.lines[range.start().line].replace_range(range.start().byte..range.end().byte, "");
        } else {
            let mut line = self.lines[range.start().line][..range.start().byte].to_string();
            line.push_str(&self.lines[range.end().line][range.end().byte..]);
            self.lines
                .splice(range.start().line..range.end().line + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change.kind {
            change::ChangeKind::Insert(point, additional_text) => {
                self.insert(point, additional_text)
            }
            change::ChangeKind::Delete(range) => self.delete(range),
        }
    }

    pub fn into_line_count(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        Self {
            lines: string.lines().map(|string| string.to_owned()).collect(),
        }
    }
}

impl From<&String> for Text {
    fn from(string: &String) -> Self {
        string.as_str().into()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Whitespace,
}
use crate::{change::ChangeKind, token::TokenKind, Change, Text, Token};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
}

impl Tokenizer {
    pub fn new(line_count: usize) -> Self {
        Self {
            state: (0..line_count).map(|_| None).collect(),
        }
    }

    pub fn apply_change(&mut self, change: &Change) {
        match &change.kind {
            ChangeKind::Insert(point, text) => {
                self.state[point.line] = None;
                let line_count = text.extent().line_count;
                if line_count > 0 {
                    let line = point.line + 1;
                    self.state.splice(line..line, (0..line_count).map(|_| None));
                }
            }
            ChangeKind::Delete(range) => {
                self.state[range.start().line] = None;
                let line_count = range.extent().line_count;
                if line_count > 0 {
                    let start_line = range.start().line + 1;
                    let end_line = start_line + line_count;
                    self.state.drain(start_line..end_line);
                }
            }
        }
    }

    pub fn update(&mut self, text: &Text, tokens: &mut [Vec<Token>]) {
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut new_tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => new_tokens.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    tokens[line] = new_tokens;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<Token>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('!', '=', _)
            | ('%', '=', _)
            | ('&', '&', _)
            | ('&', '=', _)
            | ('*', '=', _)
            | ('+', '=', _)
            | ('-', '=', _)
            | ('-', '>', _)
            | ('.', '.', _)
            | ('/', '=', _)
            | (':', ':', _)
            | ('<', '<', _)
            | ('<', '=', _)
            | ('=', '=', _)
            | ('=', '>', _)
            | ('>', '=', _)
            | ('>', '>', _)
            | ('^', '=', _)
            | ('|', '=', _)
            | ('|', '|', _) => {
                cursor.skip(2);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            ('.', char, _) if char.is_digit(10) => self.number(cursor),
            ('!', _, _)
            | ('#', _, _)
            | ('$', _, _)
            | ('%', _, _)
            | ('&', _, _)
            | ('*', _, _)
            | ('+', _, _)
            | (',', _, _)
            | ('-', _, _)
            | ('.', _, _)
            | ('/', _, _)
            | (':', _, _)
            | (';', _, _)
            | ('<', _, _)
            | ('=', _, _)
            | ('>', _, _)
            | ('?', _, _)
            | ('@', _, _)
            | ('^', _, _)
            | ('_', _, _)
            | ('|', _, _) => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            (char, _, _) if char.is_identifier_start() => self.identifier_or_keyword(cursor),
            (char, _, _) if char.is_digit(10) => self.number(cursor),
            (char, _, _) if char.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index;
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_identifier_continue()) {}
        let end = cursor.index;

        (
            State::Initial(InitialState),
            match &cursor.string[start..end] {
                "else" | "if" | "match" | "return" => TokenKind::BranchKeyword,
                "break" | "continue" | "for" | "loop" | "while" => TokenKind::LoopKeyword,
                "Self" | "as" | "async" | "await" | "const" | "crate" | "dyn" | "enum"
                | "extern" | "false" | "fn" | "impl" | "in" | "let" | "mod" | "move" | "mut"
                | "pub" | "ref" | "self" | "static" | "struct" | "super" | "trait" | "true"
                | "type" | "unsafe" | "use" | "where" => TokenKind::OtherKeyword,
                _ => TokenKind::Identifier,
            },
        )
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    _ => {
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                }
            }
        };
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    string: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Cursor { string, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.string[self.index..].chars().nth(index).unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index = self.string[self.index..]
            .char_indices()
            .nth(count)
            .map_or(self.string.len(), |(index, _)| self.index + index);
    }

    fn skip_if<P>(&mut self, predicate: P) -> bool
    where
        P: FnOnce(char) -> bool,
    {
        if predicate(self.peek(0)) {
            self.skip(1);
            true
        } else {
            false
        }
    }

    fn skip_exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(0) == '-' {
            self.skip(1);
        }
        self.skip_digits(10)
    }

    fn skip_digits(&mut self, radix: u32) -> bool {
        let mut has_skip_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                char if char.is_digit(radix) => {
                    self.skip(1);
                    has_skip_digits = true;
                }
                _ => break,
            }
        }
        has_skip_digits
    }

    fn skip_suffix(&mut self) -> bool {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if(|char| char.is_identifier_continue()) {}
            return true;
        }
        false
    }
}

pub trait CharExt {
    fn is_identifier_start(self) -> bool;
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub height: f64,
}
use crate::{char::CharExt, line::Inline, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize, tab_column_count: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .indentation()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count(tab_column_count))
        .sum();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            Inline::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte = 0;
    let mut column = 0;
    let mut wraps = Vec::new();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if column + column_count > wrap_column {
                        column = indent_column_count;
                        wraps.push(byte);
                    }
                    column += column_count;
                    byte += string.len();
                }
            }
            Inline::Widget(widget) => {
                if column + widget.column_count > wrap_column {
                    column = indent_column_count;
                    wraps.push(byte);
                }
                column += widget.column_count;
                byte += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}
use {
    makepad_code_editor::{
        code_editor::*,
        state::{CodeDocument, CodeSession},
    },
    makepad_widgets::*,
    std::{cell::RefCell, rc::Rc},
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_code_editor::code_editor::CodeEditor;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <CodeEditor> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    code_editor.draw(&mut cx, &mut self.state.session);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        if let Some(mut code_editor) = self.ui.get_code_editor(id!(code_editor)).borrow_mut() {
            code_editor.handle_event(cx, event, &mut self.state.session);
        }
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_code_editor::code_editor::live_design(cx);
    }
}

struct State {
    session: CodeSession,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: CodeSession::new(Rc::new(RefCell::new(CodeDocument::new(
                include_str!("state.rs").into(),
            )))),
        }
    }
}

app_main!(App);
use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => {
                    ChangeKind::Insert(range.start(), text.slice(range))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
pub trait CharExt {
    fn is_opening_delimiter(self) -> bool;
    fn is_closing_delimiter(self) -> bool;
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn is_opening_delimiter(self) -> bool {
        match self {
            '(' | '[' | '{' => true,
            _ => false,
        }
    }

    fn is_closing_delimiter(self) -> bool {
        match self {
            ')' | ']' | '}' => true,
            _ => false,
        }
    }

    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{
        line::Wrapped,
        selection::Affinity,
        state::{Block, CodeSession},
        str::StrExt,
        token::TokenKind,
        Line, Point, Selection, Token,
    },
    makepad_widgets::*,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

    TokenColors = {{TokenColors}} {
        unknown: #808080,
        branch_keyword: #C485BE,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        whitespace: #6E6E6E,
    }

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0

        fn vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped - self.rect_pos) / self.rect_size;
            return self.camera_projection * (self.camera_view * (
                self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
            ));
        }

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.rect_pos + self.pos * self.rect_size);
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.gloop(self.gloopiness);
            }
            return sdf.fill(#08f8);
        }
    }

    CodeEditor = {{CodeEditor}} {
        
            width: Fill,
            height: Fill,
            margin: 0,
        
        draw_text: {
            draw_depth: 0.0,
            text_style: <FONT_CODE> {}
        }
        draw_selection: {
            draw_depth: 1.0,
        }
        draw_cursor: {
            draw_depth: 2.0,
            color: #C0C0C0,
        }
    }
}

#[derive(Live)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
    #[live]
    draw_text: DrawText,
    #[live]
    token_colors: TokenColors,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start: usize,
    #[rust]
    end: usize,
}

impl LiveHook for CodeEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, CodeEditor)
    }
}

impl Widget for CodeEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }

    fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        //let uid = self.widget_uid();
        /*self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });*/
        //self.handle_event
    }

    fn walk(&self) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CodeEditorRef(WidgetRef);

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d, session: &mut CodeSession) {
        let walk = self.draw_state.get().unwrap();

        self.scroll_bars.begin(cx, walk, Layout::default());

        self.viewport_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();

        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        session.handle_changes();
        session.set_wrap_column(Some(
            (self.viewport_rect.size.x / self.cell_size.x) as usize,
        ));
        self.start = session.find_first_line_ending_after_y(scroll_pos.y / self.cell_size.y);
        self.end = session.find_first_line_starting_after_y(
            (scroll_pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );

        self.draw_text(cx, session);
        self.draw_selections(cx, session);
        cx.turtle_mut().set_used(
            session.width() * self.cell_size.x,
            session.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if session.update_folds() {
            cx.redraw_all();
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, session: &mut CodeSession) {
        session.handle_changes();
        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.fold();
                cx.redraw_all();
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.unfold();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_left(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_right(!shift);
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_up(!shift);
                cx.redraw_all();
            }

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_down(!shift);
                cx.redraw_all();
            }
            Hit::TextInput(TextInputEvent { ref input, .. }) => {
                session.insert(input.into());
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                session.enter();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::RBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.indent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::LBracket,
                modifiers: KeyModifiers { logo: true, .. },
                ..
            }) => {
                session.outdent();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                session.delete();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.backspace();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: false, .. },
                ..
            }) => {
                session.undo();
                cx.redraw_all();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: KeyModifiers { logo: true, shift: true, .. },
                ..
            }) => {
                session.redo();
                cx.redraw_all();
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                cx.set_key_focus(self.scroll_bars.area());
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    if alt {
                        session.add_cursor(cursor, affinity);
                    } else {
                        session.set_cursor(cursor, affinity);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                if let Some((cursor, affinity)) = self.pick(session, abs) {
                    session.move_to(cursor, affinity);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut y = 0.0;
        session.blocks(
            0,
            session.document().borrow().text().as_lines().len(),
            |blocks| {
                for block in blocks {
                    match block {
                        Block::Line { line, .. } => {
                            self.draw_text.font_scale = line.scale();
                            let mut token_iter = line.tokens().iter().copied();
                            let mut token_slot = token_iter.next();
                            let mut column = 0;
                            for wrapped in line.wrappeds() {
                                match wrapped {
                                    Wrapped::Text {
                                        is_inlay: false,
                                        mut text,
                                    } => {
                                        while !text.is_empty() {
                                            let token = match token_slot {
                                                Some(token) => {
                                                    if text.len() < token.len {
                                                        token_slot = Some(Token {
                                                            len: token.len - text.len(),
                                                            kind: token.kind,
                                                        });
                                                        Token {
                                                            len: text.len(),
                                                            kind: token.kind,
                                                        }
                                                    } else {
                                                        token_slot = token_iter.next();
                                                        token
                                                    }
                                                }
                                                None => Token {
                                                    len: text.len(),
                                                    kind: TokenKind::Unknown,
                                                },
                                            };
                                            let (text_0, text_1) = text.split_at(token.len);
                                            text = text_1;
                                            self.draw_text.color = match token.kind {
                                                TokenKind::Unknown => self.token_colors.unknown,
                                                TokenKind::BranchKeyword => {
                                                    self.token_colors.branch_keyword
                                                }
                                                TokenKind::Identifier => {
                                                    self.token_colors.identifier
                                                }
                                                TokenKind::LoopKeyword => {
                                                    self.token_colors.loop_keyword
                                                }
                                                TokenKind::Number => self.token_colors.number,
                                                TokenKind::OtherKeyword => {
                                                    self.token_colors.other_keyword
                                                }
                                                TokenKind::Punctuator => {
                                                    self.token_colors.punctuator
                                                }
                                                TokenKind::Whitespace => {
                                                    self.token_colors.whitespace
                                                }
                                            };
                                            self.draw_text.draw_abs(
                                                cx,
                                                DVec2 {
                                                    x: line.column_to_x(column),
                                                    y,
                                                } * self.cell_size
                                                    + self.viewport_rect.pos,
                                                text_0,
                                            );
                                            column += text_0
                                                .column_count(session.settings().tab_column_count);
                                        }
                                    }
                                    Wrapped::Text {
                                        is_inlay: true,
                                        text,
                                    } => {
                                        self.draw_text.draw_abs(
                                            cx,
                                            DVec2 {
                                                x: line.column_to_x(column),
                                                y,
                                            } * self.cell_size
                                                + self.viewport_rect.pos,
                                            text,
                                        );
                                        column +=
                                            text.column_count(session.settings().tab_column_count);
                                    }
                                    Wrapped::Widget(widget) => {
                                        column += widget.column_count;
                                    }
                                    Wrapped::Wrap => {
                                        column = line.wrap_indent_column_count();
                                        y += line.scale();
                                    }
                                }
                            }
                            y += line.scale();
                        }
                        Block::Widget(widget) => {
                            y += widget.height;
                        }
                    }
                }
            },
        );
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, session: &CodeSession) {
        let mut active_selection = None;
        let mut selections = session.selections().iter();
        while selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.end().line < self.start)
        {
            selections.next().unwrap();
        }
        if selections
            .as_slice()
            .first()
            .map_or(false, |selection| selection.start().line < self.start)
        {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelections {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, session)
    }

    fn pick(&self, session: &CodeSession, point: DVec2) -> Option<(Point, Affinity)> {
        let point = (point - self.viewport_rect.pos) / self.cell_size;
        let mut line = session.find_first_line_ending_after_y(point.y);
        let mut y = session.line(line, |line| line.y());
        session.blocks(line, line + 1, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        let next_byte = byte + grapheme.len();
                                        let next_column = column
                                            + grapheme
                                                .column_count(session.settings().tab_column_count);
                                        let next_y = y + line_ref.scale();
                                        let x = line_ref.column_to_x(column);
                                        let next_x = line_ref.column_to_x(next_column);
                                        let mid_x = (x + next_x) / 2.0;
                                        if (y..=next_y).contains(&point.y) {
                                            if (x..=mid_x).contains(&point.x) {
                                                return Some((
                                                    Point { line, byte },
                                                    Affinity::After,
                                                ));
                                            }
                                            if (mid_x..=next_x).contains(&point.x) {
                                                return Some((
                                                    Point {
                                                        line,
                                                        byte: next_byte,
                                                    },
                                                    Affinity::Before,
                                                ));
                                            }
                                        }
                                        byte = next_byte;
                                        column = next_column;
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    let next_column = column
                                        + text.column_count(session.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    if (y..=next_y).contains(&point.y)
                                        && (x..=next_x).contains(&point.x)
                                    {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = next_column;
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    let next_y = y + line_ref.scale();
                                    if (y..=next_y).contains(&point.y) {
                                        return Some((Point { line, byte }, Affinity::Before));
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y = next_y;
                                }
                            }
                        }
                        let next_y = y + line_ref.scale();
                        if (y..=y + next_y).contains(&point.y) {
                            return Some((Point { line, byte }, Affinity::After));
                        }
                        line += 1;
                        y = next_y;
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        let next_y = y + line_ref.height();
                        if (y..=next_y).contains(&point.y) {
                            return Some((Point { line, byte: 0 }, Affinity::Before));
                        }
                        y = next_y;
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
            None
        })
    }
}

struct DrawSelections<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelections<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d, session: &CodeSession) {
        let mut line = self.code_editor.start;
        let mut y = session.line(line, |line| line.y());
        session.blocks(self.code_editor.start, self.code_editor.end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: false,
                        line: line_ref,
                    } => {
                        let mut byte = 0;
                        let mut column = 0;
                        self.handle_event(cx, line, line_ref, byte, Affinity::Before, y, column);
                        for wrapped in line_ref.wrappeds() {
                            match wrapped {
                                Wrapped::Text {
                                    is_inlay: false,
                                    text,
                                } => {
                                    for grapheme in text.graphemes() {
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::After,
                                            y,
                                            column,
                                        );
                                        byte += grapheme.len();
                                        column += grapheme
                                            .column_count(session.settings().tab_column_count);
                                        self.handle_event(
                                            cx,
                                            line,
                                            line_ref,
                                            byte,
                                            Affinity::Before,
                                            y,
                                            column,
                                        );
                                    }
                                }
                                Wrapped::Text {
                                    is_inlay: true,
                                    text,
                                } => {
                                    column +=
                                        text.column_count(session.settings().tab_column_count);
                                }
                                Wrapped::Widget(widget) => {
                                    column += widget.column_count;
                                }
                                Wrapped::Wrap => {
                                    if self.active_selection.is_some() {
                                        self.draw_selection(cx, line_ref, y, column);
                                    }
                                    column = line_ref.wrap_indent_column_count();
                                    y += line_ref.scale();
                                }
                            }
                        }
                        self.handle_event(cx, line, line_ref, byte, Affinity::After, y, column);
                        column += 1;
                        if self.active_selection.is_some() {
                            self.draw_selection(cx, line_ref, y, column);
                        }
                        line += 1;
                        y += line_ref.scale();
                    }
                    Block::Line {
                        is_inlay: true,
                        line: line_ref,
                    } => {
                        y += line_ref.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line: usize,
        line_ref: Line<'_>,
        byte: usize,
        affinity: Affinity,
        y: f64,
        column: usize,
    ) {
        let point = Point { line, byte };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == point && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line_ref, y, column);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == point && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor == point && selection.affinity == affinity {
                self.draw_cursor(cx, line_ref, y, column);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection,
                    start_x: line_ref.column_to_x(column),
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, line: Line<'_>, y: f64, column: usize) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: line.column_to_x(column) - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, line: Line<'_>, y: f64, column: usize) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: line.column_to_x(column),
                    y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

#[derive(Live, LiveHook)]
struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    whitespace: Vec4,
}

#[derive(Live, LiveHook)]
#[repr(C)]
struct DrawSelection {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    prev_x: f32,
    #[live]
    prev_w: f32,
    #[live]
    next_x: f32,
    #[live]
    next_w: f32,
    #[rust]
    prev_prev_rect: Option<Rect>,
    #[rust]
    prev_rect: Option<Rect>,
}

impl DrawSelection {
    fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
            Self {
                line_count: 0,
                byte_count: self.byte_count - other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count - other.line_count,
                byte_count: self.byte_count,
            }
        }
    }
}

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
use crate::{state::SessionId, Change, Selection, Text};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct History {
    current_edit: Option<(SessionId, EditKind)>,
    undos: Vec<(Vec<Selection>, Vec<Change>)>,
    redos: Vec<(Vec<Selection>, Vec<Change>)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_new_edit_group(&mut self) {
        self.current_edit = None;
    }

    pub fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        inverted_changes: Vec<Change>,
    ) {
        if self
            .current_edit
            .map_or(false, |current_edit| current_edit == (origin_id, kind))
        {
            self.undos.last_mut().unwrap().1.extend(inverted_changes);
        } else {
            self.current_edit = Some((origin_id, kind));
            self.undos.push((selections.to_vec(), inverted_changes));
        }
        self.redos.clear();
    }

    pub fn undo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, mut inverted_changes)) = self.undos.pop() {
            self.current_edit = None;
            let mut changes = Vec::new();
            inverted_changes.reverse();
            for inverted_change in inverted_changes.iter().cloned() {
                let change = inverted_change.clone().invert(&text);
                text.apply_change(inverted_change);
                changes.push(change);
            }
            self.redos.push((selections.clone(), changes.clone()));
            Some((selections, inverted_changes))
        } else {
            None
        }
    }

    pub fn redo(&mut self, text: &mut Text) -> Option<(Vec<Selection>, Vec<Change>)> {
        if let Some((selections, changes)) = self.redos.pop() {
            self.current_edit = None;
            for change in changes.iter().cloned() {
                text.apply_change(change);
            }
            self.undos.push((selections.clone(), changes.clone()));
            Some((selections, changes))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditKind {
    Insert,
    Delete,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EditGroup {
    pub selections: Vec<Selection>,
    pub changes: Vec<Change>,
}
use crate::widgets::{BlockWidget, InlineWidget};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Widget(BlockWidget),
}
pub trait IteratorExt: Iterator {
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>;
}

impl<T> IteratorExt for T
where
    T: Iterator,
{
    fn merge<F>(self, f: F) -> Merge<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item, Self::Item) -> Result<Self::Item, (Self::Item, Self::Item)>,
    {
        Merge {
            prev_item: None,
            iter: self,
            f,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Merge<I, F>
where
    I: Iterator,
{
    prev_item: Option<I::Item>,
    iter: I,
    f: F,
}

impl<I, F> Iterator for Merge<I, F>
where
    I: Iterator,
    F: FnMut(I::Item, I::Item) -> Result<I::Item, (I::Item, I::Item)>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.prev_item.take(), self.iter.next()) {
                (Some(prev_item), Some(item)) => match (self.f)(prev_item, item) {
                    Ok(merged_item) => {
                        self.prev_item = Some(merged_item);
                        continue;
                    }
                    Err((prev_item, item)) => {
                        self.prev_item = Some(item);
                        break Some(prev_item);
                    }
                },
                (None, Some(item)) => {
                    self.prev_item = Some(item);
                    continue;
                }
                (Some(prev_item), None) => break Some(prev_item),
                (None, None) => break None,
            }
        }
    }
}
pub use makepad_widgets;
use makepad_widgets::*;

pub mod change;
pub mod char;
pub mod code_editor;
pub mod extent;
pub mod history;
pub mod inlays;
pub mod iter;
pub mod line;
pub mod move_ops;
pub mod point;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;
pub mod widgets;
pub mod wrap;

pub use self::{
    change::Change,
    code_editor::CodeEditor,
    extent::Extent,
    history::History,
    line::Line,
    point::Point,
    range::Range,
    selection::Selection,
    settings::Settings,
    state::{CodeDocument, CodeSession},
    text::Text,
    token::Token,
    tokenizer::Tokenizer,
};

pub fn live_design(cx: &mut Cx) {
    crate::code_editor::live_design(cx);
}
use {
    crate::{
        inlays::InlineInlay, selection::Affinity, str::StrExt, widgets::InlineWidget,
        wrap::WrapData, Token,
    },
    std::slice::Iter,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    pub y: Option<f64>,
    pub column_count: Option<usize>,
    pub fold_column: usize,
    pub scale: f64,
    pub text: &'a str,
    pub tokens: &'a [Token],
    pub inline_inlays: &'a [(usize, InlineInlay)],
    pub wrap_data: Option<&'a WrapData>,
}

impl<'a> Line<'a> {
    pub fn y(&self) -> f64 {
        self.y.unwrap()
    }

    pub fn row_count(&self) -> usize {
        self.wrap_data.unwrap().wraps.len() + 1
    }

    pub fn column_count(&self) -> usize {
        self.column_count.unwrap()
    }

    pub fn height(&self) -> f64 {
        self.row_count() as f64 * self.scale
    }

    pub fn width(&self) -> f64 {
        self.column_to_x(self.column_count())
    }

    pub fn byte_and_affinity_to_row_and_column(
        &self,
        byte: usize,
        affinity: Affinity,
        tab_column_count: usize,
    ) -> (usize, usize) {
        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if current_byte == byte && affinity == Affinity::Before {
            return (row, column);
        }
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        if current_byte == byte && affinity == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if current_byte == byte && affinity == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    column += text.column_count(tab_column_count);
                }
                Wrapped::Widget(widget) => {
                    column += widget.column_count;
                }
                Wrapped::Wrap => {
                    row += 1;
                    column = self.wrap_indent_column_count();
                }
            }
        }
        if current_byte == byte && affinity == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_and_column_to_byte_and_affinity(
        &self,
        row: usize,
        column: usize,
        tab_width: usize,
    ) -> (usize, Affinity) {
        let mut current_row = 0;
        let mut current_column = 0;
        let mut byte = 0;
        for wrapped in self.wrappeds() {
            match wrapped {
                Wrapped::Text {
                    is_inlay: false,
                    text,
                } => {
                    for grapheme in text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_width);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte += grapheme.len();
                        current_column = next_column;
                    }
                }
                Wrapped::Text {
                    is_inlay: true,
                    text,
                } => {
                    let next_column = current_column + text.column_count(tab_width);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                Wrapped::Widget(widget) => {
                    current_column += widget.column_count;
                }
                Wrapped::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.wrap_indent_column_count();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold = column.min(self.fold_column);
        let column_count_after_fold = column - column_count_before_fold;
        column_count_before_fold as f64 + column_count_after_fold as f64 * self.scale
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn wrap_indent_column_count(self) -> usize {
        self.wrap_data.unwrap().indent_column_count
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn tokens(&self) -> &[Token] {
        self.tokens
    }

    pub fn inlines(&self) -> Inlines<'a> {
        Inlines {
            text: self.text,
            inline_inlays: self.inline_inlays.iter(),
            position: 0,
        }
    }

    pub fn wrappeds(&self) -> Wrappeds<'a> {
        let mut inlines = self.inlines();
        Wrappeds {
            inline: inlines.next(),
            inlines,
            wraps: self.wrap_data.unwrap().wraps.iter(),
            position: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inlines<'a> {
    pub(super) text: &'a str,
    pub(super) inline_inlays: Iter<'a, (usize, InlineInlay)>,
    pub(super) position: usize,
}

impl<'a> Iterator for Inlines<'a> {
    type Item = Inline<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .inline_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position == self.position)
        {
            let (_, inline_inlay) = self.inline_inlays.next().unwrap();
            return Some(match *inline_inlay {
                InlineInlay::Text(ref text) => Inline::Text {
                    is_inlay: true,
                    text,
                },
                InlineInlay::Widget(widget) => Inline::Widget(widget),
            });
        }
        if self.text.is_empty() {
            return None;
        }
        let mut mid = self.text.len();
        if let Some(&(byte, _)) = self.inline_inlays.as_slice().first() {
            mid = mid.min(byte - self.position);
        }
        let (text_0, text_1) = self.text.split_at(mid);
        self.text = text_1;
        self.position += text_0.len();
        Some(Inline::Text {
            is_inlay: false,
            text: text_0,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inline<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
}

#[derive(Clone, Debug)]
pub struct Wrappeds<'a> {
    pub(super) inline: Option<Inline<'a>>,
    pub(super) inlines: Inlines<'a>,
    pub(super) wraps: Iter<'a, usize>,
    pub(super) position: usize,
}

impl<'a> Iterator for Wrappeds<'a> {
    type Item = Wrapped<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .wraps
            .as_slice()
            .first()
            .map_or(false, |&position| position == self.position)
        {
            self.wraps.next();
            return Some(Wrapped::Wrap);
        }
        Some(match self.inline.take()? {
            Inline::Text { is_inlay, text } => {
                let mut mid = text.len();
                if let Some(&position) = self.wraps.as_slice().first() {
                    mid = mid.min(position - self.position);
                }
                let text = if mid < text.len() {
                    let (text_0, text_1) = text.split_at(mid);
                    self.inline = Some(Inline::Text {
                        is_inlay,
                        text: text_1,
                    });
                    text_0
                } else {
                    self.inline = self.inlines.next();
                    text
                };
                self.position += text.len();
                Wrapped::Text { is_inlay, text }
            }
            Inline::Widget(widget) => {
                self.position += 1;
                Wrapped::Widget(widget)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wrapped<'a> {
    Text { is_inlay: bool, text: &'a str },
    Widget(InlineWidget),
    Wrap,
}
mod app;

fn main() {
    app::app_main();
}
use crate::{selection::Affinity, str::StrExt, Point, CodeSession};

pub fn move_left(lines: &[String], point: Point) -> Point {
    if !is_at_start_of_line(point) {
        return move_to_prev_grapheme(lines, point);
    }
    if !is_at_first_line(point) {
        return move_to_end_of_prev_line(lines, point);
    }
    point
}

pub fn move_right(lines: &[String], point: Point) -> Point {
    if !is_at_end_of_line(lines, point) {
        return move_to_next_grapheme(lines, point);
    }
    if !is_at_last_line(lines, point) {
        return move_to_start_of_next_line(point);
    }
    point
}

pub fn move_up(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_first_row_of_line(session, point, affinity) {
        return move_to_prev_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_first_line(point) {
        return move_to_last_row_of_prev_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

pub fn move_down(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_last_row_of_line(session, point, affinity) {
        return move_to_next_row_of_line(session, point, affinity, preferred_column);
    }
    if !is_at_last_line(session.document().borrow().text().as_lines(), point) {
        return move_to_first_row_of_next_line(session, point, affinity, preferred_column);
    }
    (point, affinity, preferred_column)
}

fn is_at_first_line(point: Point) -> bool {
    point.line == 0
}

fn is_at_last_line(lines: &[String], point: Point) -> bool {
    point.line == lines.len()
}

fn is_at_start_of_line(point: Point) -> bool {
    point.byte == 0
}

fn is_at_end_of_line(lines: &[String], point: Point) -> bool {
    point.byte == lines[point.line].len()
}

fn is_at_first_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(session: &CodeSession, point: Point, affinity: Affinity) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(lines: &[String], point: Point) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], point: Point) -> Point {
    let line = &lines[point.line];
    Point {
        line: point.line,
        byte: line[point.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| point.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], point: Point) -> Point {
    let prev_line = point.line - 1;
    Point {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(point: Point) -> Point {
    Point {
        line: point.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_next_row_of_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row + 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

fn move_to_last_row_of_prev_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line - 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}

fn move_to_first_row_of_next_line(
    session: &CodeSession,
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line + 1, |next_line| {
            let (byte, affinity) = next_line.row_and_column_to_byte_and_affinity(
                0,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line + 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}
use {
    crate::{
        change::{ChangeKind, Drift},
        Change, Extent,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.extent() + (self - point),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Extent, Point};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}
use {
    crate::{Change, Extent, Point, Range},
    std::ops,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Point {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Point {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Extent {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line..self.cursor.line + 1
        } else {
            self.cursor.line..if self.anchor.byte == 0 {
                self.anchor.line
            } else {
                self.anchor.line + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Point, Affinity, Option<usize>) -> (Point, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change),
            cursor: self.cursor.apply_change(change),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub use_soft_tabs: bool,
    pub tab_column_count: usize,
    pub indent_column_count: usize,
    pub fold_level: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_soft_tabs: true,
            tab_column_count: 4,
            indent_column_count: 4,
            fold_level: 2,
        }
    }
}
use {
    crate::{
        change::{ChangeKind, Drift},
        char::CharExt,
        history::EditKind,
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        line::Wrapped,
        move_ops,
        selection::Affinity,
        str::StrExt,
        token::TokenKind,
        widgets::BlockWidget,
        wrap,
        wrap::WrapData,
        Change, Extent, History, Line, Point, Range, Selection, Settings, Text, Token, Tokenizer,
    },
    std::{
        cell::RefCell,
        cmp,
        collections::{HashMap, HashSet},
        iter, mem,
        rc::Rc,
        slice::Iter,
        sync::{
            atomic,
            atomic::AtomicUsize,
            mpsc,
            mpsc::{Receiver, Sender},
        },
    },
};

#[derive(Debug)]
pub struct CodeSession {
    id: SessionId,
    settings: Rc<Settings>,
    document: Rc<RefCell<CodeDocument>>,
    wrap_column: Option<usize>,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_data: Vec<Option<WrapData>>,
    folding_lines: HashSet<usize>,
    folded_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
    change_receiver: Receiver<(Option<Vec<Selection>>, Vec<Change>)>,
}

impl CodeSession {
    pub fn new(document: Rc<RefCell<CodeDocument>>) -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);

        let (change_sender, change_receiver) = mpsc::channel();
        let line_count = document.borrow().text.as_lines().len();
        let mut session = Self {
            id: SessionId(ID.fetch_add(1, atomic::Ordering::AcqRel)),
            settings: Rc::new(Settings::default()),
            document,
            wrap_column: None,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            wrap_data: (0..line_count).map(|_| None).collect(),
            folding_lines: HashSet::new(),
            folded_lines: HashSet::new(),
            unfolding_lines: HashSet::new(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
            change_receiver,
        };
        for line in 0..line_count {
            session.update_wrap_data(line);
        }
        session.update_y();
        session
            .document
            .borrow_mut()
            .change_senders
            .insert(session.id, change_sender);
        session
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn width(&self) -> f64 {
        self.lines(0, self.document.borrow().text.as_lines().len(), |lines| {
            let mut width: f64 = 0.0;
            for line in lines {
                width = width.max(line.width());
            }
            width
        })
    }

    pub fn height(&self) -> f64 {
        let index = self.document.borrow().text.as_lines().len() - 1;
        let mut y = self.line(index, |line| line.y() + line.height());
        self.blocks(index, index, |blocks| {
            for block in blocks {
                match block {
                    Block::Line {
                        is_inlay: true,
                        line,
                    } => y += line.height(),
                    Block::Widget(widget) => y += widget.height,
                    _ => unreachable!(),
                }
            }
        });
        y
    }

    pub fn settings(&self) -> &Rc<Settings> {
        &self.settings
    }

    pub fn document(&self) -> &Rc<RefCell<CodeDocument>> {
        &self.document
    }

    pub fn wrap_column(&self) -> Option<usize> {
        self.wrap_column
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line<T>(&self, line: usize, f: impl FnOnce(Line<'_>) -> T) -> T {
        let document = self.document.borrow();
        f(Line {
            y: self.y.get(line).copied(),
            column_count: self.column_count[line],
            fold_column: self.fold_column[line],
            scale: self.scale[line],
            text: &document.text.as_lines()[line],
            tokens: &document.tokens[line],
            inline_inlays: &document.inline_inlays[line],
            wrap_data: self.wrap_data[line].as_ref(),
        })
    }

    pub fn lines<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Lines<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        f(Lines {
            y: self.y[start_line.min(self.y.len())..end_line.min(self.y.len())].iter(),
            column_count: self.column_count[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
            text: document.text.as_lines()[start_line..end_line].iter(),
            tokens: document.tokens[start_line..end_line].iter(),
            inline_inlays: document.inline_inlays[start_line..end_line].iter(),
            wrap_data: self.wrap_data[start_line..end_line].iter(),
        })
    }

    pub fn blocks<T>(
        &self,
        start_line: usize,
        end_line: usize,
        f: impl FnOnce(Blocks<'_>) -> T,
    ) -> T {
        let document = self.document.borrow();
        let mut block_inlays = document.block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(position, _)| position < start_line)
        {
            block_inlays.next();
        }
        self.lines(start_line, end_line, |lines| {
            f(Blocks {
                lines,
                block_inlays,
                position: start_line,
            })
        })
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn set_wrap_column(&mut self, wrap_column: Option<usize>) {
        if self.wrap_column == wrap_column {
            return;
        }
        self.wrap_column = wrap_column;
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            self.update_wrap_data(line);
        }
        self.update_y();
    }

    pub fn fold(&mut self) {
        let document = self.document.borrow();
        let lines = document.text.as_lines();
        for line in 0..lines.len() {
            let indent_level = lines[line]
                .indentation()
                .unwrap_or("")
                .column_count(self.settings.tab_column_count)
                / self.settings.indent_column_count;
            if indent_level >= self.settings.fold_level && !self.folded_lines.contains(&line) {
                self.fold_column[line] =
                    self.settings.fold_level * self.settings.indent_column_count;
                self.unfolding_lines.remove(&line);
                self.folding_lines.insert(line);
            }
        }
    }

    pub fn unfold(&mut self) {
        for line in self.folding_lines.drain() {
            self.unfolding_lines.insert(line);
        }
        for line in self.folded_lines.drain() {
            self.unfolding_lines.insert(line);
        }
    }

    pub fn update_folds(&mut self) -> bool {
        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let mut new_folding_lines = HashSet::new();
        for &line in &self.folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.1 + 0.001 {
                self.scale[line] = 0.1;
                self.folded_lines.insert(line);
            } else {
                new_folding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.folding_lines = new_folding_lines;
        let mut new_unfolding_lines = HashSet::new();
        for &line in &self.unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.y.truncate(line + 1);
        }
        self.unfolding_lines = new_unfolding_lines;
        self.update_y();
        true
    }

    pub fn set_cursor(&mut self, cursor: Point, affinity: Affinity) {
        self.selections.clear();
        self.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        });
        self.pending_selection_index = Some(0);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn add_cursor(&mut self, cursor: Point, affinity: Affinity) {
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
            preferred_column: None,
        };
        self.pending_selection_index = Some(
            match self.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return cmp::Ordering::Less;
                }
                if selection.start() >= cursor {
                    return cmp::Ordering::Greater;
                }
                cmp::Ordering::Equal
            }) {
                Ok(index) => {
                    self.selections[index] = selection;
                    index
                }
                Err(index) => {
                    self.selections.insert(index, selection);
                    index
                }
            },
        );
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_to(&mut self, cursor: Point, affinity: Affinity) {
        let mut pending_selection_index = self.pending_selection_index.unwrap();
        self.selections[pending_selection_index] = Selection {
            cursor,
            affinity,
            ..self.selections[pending_selection_index]
        };
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !self.selections[prev_selection_index]
                .should_merge(self.selections[pending_selection_index])
            {
                break;
            }
            self.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < self.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !self.selections[pending_selection_index]
                .should_merge(self.selections[next_selection_index])
            {
                break;
            }
            self.selections.remove(next_selection_index);
        }
        self.pending_selection_index = Some(pending_selection_index);
        self.document.borrow_mut().force_new_edit_group();
    }

    pub fn move_left(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_left(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_right(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, _, _| {
                (
                    move_ops::move_right(session.document.borrow().text.as_lines(), cursor),
                    Affinity::Before,
                    None,
                )
            })
        });
    }

    pub fn move_up(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_up(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn move_down(&mut self, reset_anchor: bool) {
        self.modify_selections(reset_anchor, |session, selection| {
            selection.update_cursor(|cursor, affinity, preferred_column| {
                move_ops::move_down(session, cursor, affinity, preferred_column)
            })
        });
    }

    pub fn insert(&mut self, text: Text) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Insert, &self.selections, |_, _, _| {
                (Extent::zero(), Some(text.clone()), None)
            });
    }

    pub fn enter(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Insert,
            &self.selections,
            |line, index, _| {
                (
                    if line[..index].chars().all(|char| char.is_whitespace()) {
                        Extent {
                            line_count: 0,
                            byte_count: index,
                        }
                    } else {
                        Extent::zero()
                    },
                    Some(Text::newline()),
                    if line[..index]
                        .chars()
                        .rev()
                        .find_map(|char| {
                            if char.is_opening_delimiter() {
                                return Some(true);
                            }
                            if char.is_closing_delimiter() {
                                return Some(false);
                            }
                            None
                        })
                        .unwrap_or(false)
                        && line[index..]
                            .chars()
                            .find_map(|char| {
                                if char.is_closing_delimiter() {
                                    return Some(true);
                                }
                                if !char.is_whitespace() {
                                    return Some(false);
                                }
                                None
                            })
                            .unwrap_or(false)
                    {
                        Some(Text::newline())
                    } else {
                        None
                    },
                )
            },
        );
    }

    pub fn indent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Indent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        (indentation_column_count + self.settings.indent_column_count)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn outdent(&mut self) {
        self.document.borrow_mut().edit_lines(
            self.id,
            EditKind::Outdent,
            &self.selections,
            |line| {
                reindent(
                    line,
                    self.settings.use_soft_tabs,
                    self.settings.tab_column_count,
                    |indentation_column_count| {
                        indentation_column_count.saturating_sub(1)
                            / self.settings.indent_column_count
                            * self.settings.indent_column_count
                    },
                )
            },
        );
    }

    pub fn delete(&mut self) {
        self.document
            .borrow_mut()
            .edit(self.id, EditKind::Delete, &self.selections, |_, _, _| {
                (Extent::zero(), None, None)
            });
    }

    pub fn backspace(&mut self) {
        self.document.borrow_mut().edit(
            self.id,
            EditKind::Delete,
            &self.selections,
            |line, index, is_empty| {
                (
                    if is_empty {
                        if index == 0 {
                            Extent {
                                line_count: 1,
                                byte_count: 0,
                            }
                        } else {
                            Extent {
                                line_count: 0,
                                byte_count: line.graphemes().next_back().unwrap().len(),
                            }
                        }
                    } else {
                        Extent::zero()
                    },
                    None,
                    None,
                )
            },
        );
    }

    pub fn undo(&mut self) {
        self.document.borrow_mut().undo(self.id);
    }

    pub fn redo(&mut self) {
        self.document.borrow_mut().redo(self.id);
    }

    fn update_y(&mut self) {
        let start = self.y.len();
        let end = self.document.borrow().text.as_lines().len();
        if start == end + 1 {
            return;
        }
        let mut y = if start == 0 {
            0.0
        } else {
            self.line(start - 1, |line| line.y() + line.height())
        };
        let mut ys = mem::take(&mut self.y);
        self.blocks(start, end, |blocks| {
            for block in blocks {
                match block {
                    Block::Line { is_inlay, line } => {
                        if !is_inlay {
                            ys.push(y);
                        }
                        y += line.height();
                    }
                    Block::Widget(widget) => {
                        y += widget.height;
                    }
                }
            }
        });
        ys.push(y);
        self.y = ys;
    }

    pub fn handle_changes(&mut self) {
        while let Ok((selections, changes)) = self.change_receiver.try_recv() {
            self.apply_changes(selections, &changes);
        }
    }

    fn update_column_count(&mut self, index: usize) {
        let mut column_count = 0;
        let mut column = 0;
        self.line(index, |line| {
            for wrapped in line.wrappeds() {
                match wrapped {
                    Wrapped::Text { text, .. } => {
                        column += text
                            .column_count(self.settings.tab_column_count);
                    }
                    Wrapped::Widget(widget) => {
                        column += widget.column_count;
                    }
                    Wrapped::Wrap => {
                        column_count = column_count.max(column);
                        column = line.wrap_indent_column_count();
                    }
                }
            }
        });
        self.column_count[index] = Some(column_count.max(column));
    }

    fn update_wrap_data(&mut self, line: usize) {
        let wrap_data = match self.wrap_column {
            Some(wrap_column) => self.line(line, |line| {
                wrap::compute_wrap_data(line, wrap_column, self.settings.tab_column_count)
            }),
            None => WrapData::default(),
        };
        self.wrap_data[line] = Some(wrap_data);
        self.y.truncate(line + 1);
        self.update_column_count(line);
    }

    fn modify_selections(
        &mut self,
        reset_anchor: bool,
        mut f: impl FnMut(&CodeSession, Selection) -> Selection,
    ) {
        let mut selections = mem::take(&mut self.selections);
        for selection in &mut selections {
            *selection = f(&self, *selection);
            if reset_anchor {
                *selection = selection.reset_anchor();
            }
        }
        self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if let Some(merged_selection) = current_selection.merge(next_selection) {
                self.selections[current_selection_index] = merged_selection;
                self.selections.remove(next_selection_index);
                if let Some(pending_selection_index) = self.pending_selection_index.as_mut() {
                    if next_selection_index < *pending_selection_index {
                        *pending_selection_index -= 1;
                    }
                }
            } else {
                current_selection_index += 1;
            }
        }
        self.document.borrow_mut().force_new_edit_group();
    }

    fn apply_changes(&mut self, selections: Option<Vec<Selection>>, changes: &[Change]) {
        for change in changes {
            match &change.kind {
                ChangeKind::Insert(point, text) => {
                    self.column_count[point.line] = None;
                    self.wrap_data[point.line] = None;
                    let line_count = text.extent().line_count;
                    if line_count > 0 {
                        let line = point.line + 1;
                        self.y.truncate(line);
                        self.column_count
                            .splice(line..line, (0..line_count).map(|_| None));
                        self.fold_column
                            .splice(line..line, (0..line_count).map(|_| 0));
                        self.scale.splice(line..line, (0..line_count).map(|_| 1.0));
                        self.wrap_data
                            .splice(line..line, (0..line_count).map(|_| None));
                    }
                }
                ChangeKind::Delete(range) => {
                    self.column_count[range.start().line] = None;
                    self.wrap_data[range.start().line] = None;
                    let line_count = range.extent().line_count;
                    if line_count > 0 {
                        let start_line = range.start().line + 1;
                        let end_line = start_line + line_count;
                        self.y.truncate(start_line);
                        self.column_count.drain(start_line..end_line);
                        self.fold_column.drain(start_line..end_line);
                        self.scale.drain(start_line..end_line);
                        self.wrap_data.drain(start_line..end_line);
                    }
                }
            }
        }
        let line_count = self.document.borrow().text.as_lines().len();
        for line in 0..line_count {
            if self.wrap_data[line].is_none() {
                self.update_wrap_data(line);
            }
        }
        if let Some(selections) = selections {
            self.selections = selections;
        } else {
            for change in changes {
                for selection in &mut self.selections {
                    *selection = selection.apply_change(&change);
                }
            }
        }
        self.update_y();
    }
}

impl Drop for CodeSession {
    fn drop(&mut self) {
        self.document.borrow_mut().change_senders.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_data: Iter<'a, Option<WrapData>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays.next().unwrap(),
            wrap_data: self.wrap_data.next().unwrap().as_ref(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    lines: Lines<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    position: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.position)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.position += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(usize);

#[derive(Debug)]
pub struct CodeDocument {
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    history: History,
    tokenizer: Tokenizer,
    change_senders: HashMap<SessionId, Sender<(Option<Vec<Selection>>, Vec<Change>)>>,
}

impl CodeDocument {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let mut document = Self {
            text,
            tokens,
            inline_inlays: (0..line_count)
                .map(|line| {
                    if line % 5 == 0 {
                        [
                            (20, InlineInlay::Text("XXX".into())),
                            (40, InlineInlay::Text("XXX".into())),
                            (60, InlineInlay::Text("XXX".into())),
                            (80, InlineInlay::Text("XXX".into())),
                        ]
                        .into()
                    } else {
                        Vec::new()
                    }
                })
                .collect(),
            block_inlays: Vec::new(),
            history: History::new(),
            tokenizer: Tokenizer::new(line_count),
            change_senders: HashMap::new(),
        };
        document
            .tokenizer
            .update(&document.text, &mut document.tokens);
        document
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    fn edit(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&String, usize, bool) -> (Extent, Option<Text>, Option<Text>),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        let mut point = Point::zero();
        let mut prev_range_end = Point::zero();
        for range in selections
            .iter()
            .copied()
            .merge(
                |selection_0, selection_1| match selection_0.merge(selection_1) {
                    Some(selection) => Ok(selection),
                    None => Err((selection_0, selection_1)),
                },
            )
            .map(|selection| selection.range())
        {
            point += range.start() - prev_range_end;
            if !range.is_empty() {
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, range.extent())),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            let (delete_extent, insert_text_before, insert_text_after) = f(
                &self.text.as_lines()[point.line],
                point.byte,
                range.is_empty(),
            );
            if delete_extent != Extent::zero() {
                if delete_extent.line_count == 0 {
                    point.byte -= delete_extent.byte_count;
                } else {
                    point.line -= delete_extent.line_count;
                    point.byte = self.text.as_lines()[point.line].len() - delete_extent.byte_count;
                }
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Delete(Range::from_start_and_extent(point, delete_extent)),
                };
                let inverted_change = change.clone().invert(&self.text);
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_before) = insert_text_before {
                let extent = insert_text_before.extent();
                let change = Change {
                    drift: Drift::Before,
                    kind: ChangeKind::Insert(point, insert_text_before),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            if let Some(insert_text_after) = insert_text_after {
                let extent = insert_text_after.extent();
                let change = Change {
                    drift: Drift::After,
                    kind: ChangeKind::Insert(point, insert_text_after),
                };
                let inverted_change = change.clone().invert(&self.text);
                point += extent;
                self.text.apply_change(change.clone());
                changes.push(change);
                inverted_changes.push(inverted_change);
            }
            prev_range_end = range.end();
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines(
        &mut self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &[Selection],
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut changes = Vec::new();
        let mut inverted_changes = Vec::new();
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut changes, &mut inverted_changes, &mut f);
            }
        }
        self.history
            .edit(origin_id, kind, selections, inverted_changes);
        self.apply_changes(origin_id, None, &changes);
    }

    fn edit_lines_internal(
        &mut self,
        line: usize,
        changes: &mut Vec<Change>,
        inverted_changes: &mut Vec<Change>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let (byte, delete_byte_count, insert_text) = f(&self.text.as_lines()[line]);
        if delete_byte_count > 0 {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Delete(Range::from_start_and_extent(
                    Point { line, byte },
                    Extent {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                )),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
        if !insert_text.is_empty() {
            let change = Change {
                drift: Drift::Before,
                kind: ChangeKind::Insert(Point { line, byte }, insert_text.into()),
            };
            let inverted_change = change.clone().invert(&self.text);
            self.text.apply_change(change.clone());
            changes.push(change);
            inverted_changes.push(inverted_change);
        }
    }

    fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group()
    }

    fn undo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.undo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn redo(&mut self, origin_id: SessionId) {
        if let Some((selections, changes)) = self.history.redo(&mut self.text) {
            self.apply_changes(origin_id, Some(selections), &changes);
        }
    }

    fn apply_changes(
        &mut self,
        origin_id: SessionId,
        selections: Option<Vec<Selection>>,
        changes: &[Change],
    ) {
        for change in changes {
            self.apply_change_to_tokens(change);
            self.apply_change_to_inline_inlays(change);
            self.tokenizer.apply_change(change);
        }
        self.tokenizer.update(&self.text, &mut self.tokens);
        for (&session_id, change_sender) in &self.change_senders {
            if session_id == origin_id {
                change_sender
                    .send((selections.clone(), changes.to_vec()))
                    .unwrap();
            } else {
                change_sender
                    .send((
                        None,
                        changes
                            .iter()
                            .cloned()
                            .map(|change| Change {
                                drift: Drift::Before,
                                ..change
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = self.tokens[point.line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[point.line].len());
                if byte != point.byte {
                    let token = self.tokens[point.line][index];
                    let mid = point.byte - byte;
                    self.tokens[point.line][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    self.tokens[point.line].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.extent().line_count == 0 {
                    self.tokens[point.line]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][..index].iter().copied());
                    tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, self.tokens[point.line][index..].iter().copied());
                    self.tokens.splice(point.line..point.line + 1, tokens);
                }
            }
            ChangeKind::Delete(range) => {
                let mut byte = 0;
                let mut start = self.tokens[range.start().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.start().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.start().line].len());
                if byte != range.start().byte {
                    let token = self.tokens[range.start().line][start];
                    let mid = range.start().byte - byte;
                    self.tokens[range.start().line][start] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start += 1;
                    self.tokens[range.start().line].insert(
                        start,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end = self.tokens[range.end().line]
                    .iter()
                    .position(|token| {
                        if byte + token.len > range.end().byte {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(self.tokens[range.end().line].len());
                if byte != range.end().byte {
                    let token = self.tokens[range.end().line][end];
                    let mid = range.end().byte - byte;
                    self.tokens[range.end().line][end] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end += 1;
                    self.tokens[range.end().line].insert(
                        end,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if range.start().line == range.end().line {
                    self.tokens[range.start().line].drain(start..end);
                } else {
                    let mut tokens = self.tokens[range.start().line][..start]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    tokens.extend(self.tokens[range.end().line][end..].iter().copied());
                    self.tokens
                        .splice(range.start().line..range.end().line + 1, iter::once(tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&mut self, change: &Change) {
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = self.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        cmp::Ordering::Less => false,
                        cmp::Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        cmp::Ordering::Greater => true,
                    })
                    .unwrap_or(self.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut self.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, self.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        self.inline_inlays[point.line]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.extent().byte_count, inline_inlay)
                            }),
                    );
                    self.inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = self.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(self.inline_inlays[range.start().line].len());
                let end = self.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(self.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    self.inline_inlays[range.start().line].drain(start..end);
                    for (byte, _) in &mut self.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = self.inline_inlays[range.start().line]
                        .drain(..start)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(self.inline_inlays[range.end().line].drain(end..).map(
                        |(byte, inline_inlay)| {
                            (
                                range.start().byte + byte - range.end().byte.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    self.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}

fn reindent(
    string: &str,
    use_soft_tabs: bool,
    tab_column_count: usize,
    f: impl FnOnce(usize) -> usize,
) -> (usize, usize, String) {
    let indentation = string.indentation().unwrap_or("");
    let indentation_column_count = indentation.column_count(tab_column_count);
    let new_indentation_column_count = f(indentation_column_count);
    let new_indentation = new_indentation(
        new_indentation_column_count,
        use_soft_tabs,
        tab_column_count,
    );
    let len = indentation.longest_common_prefix(&new_indentation).len();
    (
        len,
        indentation.len() - len.min(indentation.len()),
        new_indentation[len..].to_owned(),
    )
}

fn new_indentation(column_count: usize, use_soft_tabs: bool, tab_column_count: usize) -> String {
    let tab_count;
    let space_count;
    if use_soft_tabs {
        tab_count = 0;
        space_count = column_count;
    } else {
        tab_count = column_count / tab_column_count;
        space_count = column_count % tab_column_count;
    }
    let tabs = iter::repeat("\t").take(tab_count);
    let spaces = iter::repeat(" ").take(space_count);
    tabs.chain(spaces).collect()
}
use crate::char::CharExt;

pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indentation(&self) -> Option<&str>;
    fn longest_common_prefix(&self, other: &str) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indentation(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn longest_common_prefix(&self, other: &str) -> &str {
        &self[..self
            .char_indices()
            .zip(other.chars())
            .find(|((_, char_0), char_1)| char_0 == char_1)
            .map(|((index, _), _)| index)
            .unwrap_or_else(|| self.len().min(other.len()))]
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
    }

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices {
            graphemes: self.graphemes(),
            start: self.as_ptr() as usize,
        }
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    string: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut end = 1;
        while !self.string.is_char_boundary(end) {
            end += 1;
        }
        let (grapheme, string) = self.string.split_at(end);
        self.string = string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut start = self.string.len() - 1;
        while !self.string.is_char_boundary(start) {
            start -= 1;
        }
        let (string, grapheme) = self.string.split_at(start);
        self.string = string;
        Some(grapheme)
    }
}

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
    start: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
use {
    crate::{change, Change, Extent, Point, Range},
    std::{io, io::BufRead, iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn newline() -> Self {
        Self {
            lines: vec![String::new(), String::new()],
        }
    }

    pub fn from_buf_reader<R>(reader: R) -> io::Result<Self>
    where
        R: BufRead,
    {
        Ok(Self {
            lines: reader.lines().collect::<Result<_, _>>()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.extent() == Extent::zero()
    }

    pub fn extent(&self) -> Extent {
        Extent {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line][range.start().byte..range.end().byte].to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines.push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
        }
        Text { lines }
    }

    pub fn insert(&mut self, point: Point, mut text: Self) {
        if text.extent().line_count == 0 {
            self.lines[point.line]
                .replace_range(point.byte..point.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line][..point.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line][point.byte..]);
            self.lines.splice(point.line..point.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, range: Range) {
        if range.start().line == range.end().line {
            self.lines[range.start().line].replace_range(range.start().byte..range.end().byte, "");
        } else {
            let mut line = self.lines[range.start().line][..range.start().byte].to_string();
            line.push_str(&self.lines[range.end().line][range.end().byte..]);
            self.lines
                .splice(range.start().line..range.end().line + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change.kind {
            change::ChangeKind::Insert(point, additional_text) => {
                self.insert(point, additional_text)
            }
            change::ChangeKind::Delete(range) => self.delete(range),
        }
    }

    pub fn into_line_count(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        Self {
            lines: string.lines().map(|string| string.to_owned()).collect(),
        }
    }
}

impl From<&String> for Text {
    fn from(string: &String) -> Self {
        string.as_str().into()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Whitespace,
}
use crate::{change::ChangeKind, token::TokenKind, Change, Text, Token};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
}

impl Tokenizer {
    pub fn new(line_count: usize) -> Self {
        Self {
            state: (0..line_count).map(|_| None).collect(),
        }
    }

    pub fn apply_change(&mut self, change: &Change) {
        match &change.kind {
            ChangeKind::Insert(point, text) => {
                self.state[point.line] = None;
                let line_count = text.extent().line_count;
                if line_count > 0 {
                    let line = point.line + 1;
                    self.state.splice(line..line, (0..line_count).map(|_| None));
                }
            }
            ChangeKind::Delete(range) => {
                self.state[range.start().line] = None;
                let line_count = range.extent().line_count;
                if line_count > 0 {
                    let start_line = range.start().line + 1;
                    let end_line = start_line + line_count;
                    self.state.drain(start_line..end_line);
                }
            }
        }
    }

    pub fn update(&mut self, text: &Text, tokens: &mut [Vec<Token>]) {
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut new_tokens = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => new_tokens.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    tokens[line] = new_tokens;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Initial(InitialState),
}

impl Default for State {
    fn default() -> State {
        State::Initial(InitialState)
    }
}

impl State {
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<Token>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (
            next_state,
            Some(Token {
                len: end - start,
                kind,
            }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InitialState;

impl InitialState {
    fn next(self, cursor: &mut Cursor<'_>) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1), cursor.peek(2)) {
            ('!', '=', _)
            | ('%', '=', _)
            | ('&', '&', _)
            | ('&', '=', _)
            | ('*', '=', _)
            | ('+', '=', _)
            | ('-', '=', _)
            | ('-', '>', _)
            | ('.', '.', _)
            | ('/', '=', _)
            | (':', ':', _)
            | ('<', '<', _)
            | ('<', '=', _)
            | ('=', '=', _)
            | ('=', '>', _)
            | ('>', '=', _)
            | ('>', '>', _)
            | ('^', '=', _)
            | ('|', '=', _)
            | ('|', '|', _) => {
                cursor.skip(2);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            ('.', char, _) if char.is_digit(10) => self.number(cursor),
            ('!', _, _)
            | ('#', _, _)
            | ('$', _, _)
            | ('%', _, _)
            | ('&', _, _)
            | ('*', _, _)
            | ('+', _, _)
            | (',', _, _)
            | ('-', _, _)
            | ('.', _, _)
            | ('/', _, _)
            | (':', _, _)
            | (';', _, _)
            | ('<', _, _)
            | ('=', _, _)
            | ('>', _, _)
            | ('?', _, _)
            | ('@', _, _)
            | ('^', _, _)
            | ('_', _, _)
            | ('|', _, _) => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Punctuator)
            }
            (char, _, _) if char.is_identifier_start() => self.identifier_or_keyword(cursor),
            (char, _, _) if char.is_digit(10) => self.number(cursor),
            (char, _, _) if char.is_whitespace() => self.whitespace(cursor),
            _ => {
                cursor.skip(1);
                (State::Initial(InitialState), TokenKind::Unknown)
            }
        }
    }

    fn identifier_or_keyword(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_identifier_start());
        let start = cursor.index;
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_identifier_continue()) {}
        let end = cursor.index;

        (
            State::Initial(InitialState),
            match &cursor.string[start..end] {
                "else" | "if" | "match" | "return" => TokenKind::BranchKeyword,
                "break" | "continue" | "for" | "loop" | "while" => TokenKind::LoopKeyword,
                "Self" | "as" | "async" | "await" | "const" | "crate" | "dyn" | "enum"
                | "extern" | "false" | "fn" | "impl" | "in" | "let" | "mod" | "move" | "mut"
                | "pub" | "ref" | "self" | "static" | "struct" | "super" | "trait" | "true"
                | "type" | "unsafe" | "use" | "where" => TokenKind::OtherKeyword,
                _ => TokenKind::Identifier,
            },
        )
    }

    fn number(self, cursor: &mut Cursor) -> (State, TokenKind) {
        match (cursor.peek(0), cursor.peek(1)) {
            ('0', 'b') => {
                cursor.skip(2);
                if !cursor.skip_digits(2) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'o') => {
                cursor.skip(2);
                if !cursor.skip_digits(8) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            ('0', 'x') => {
                cursor.skip(2);
                if !cursor.skip_digits(16) {
                    return (State::Initial(InitialState), TokenKind::Unknown);
                }
                return (State::Initial(InitialState), TokenKind::Number);
            }
            _ => {
                cursor.skip_digits(10);
                match cursor.peek(0) {
                    '.' if cursor.peek(1) != '.' && !cursor.peek(0).is_identifier_start() => {
                        cursor.skip(1);
                        if cursor.skip_digits(10) {
                            if cursor.peek(0) == 'E' || cursor.peek(0) == 'e' {
                                if !cursor.skip_exponent() {
                                    return (State::Initial(InitialState), TokenKind::Unknown);
                                }
                            }
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    'E' | 'e' => {
                        if !cursor.skip_exponent() {
                            return (State::Initial(InitialState), TokenKind::Unknown);
                        }
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                    _ => {
                        cursor.skip_suffix();
                        return (State::Initial(InitialState), TokenKind::Number);
                    }
                }
            }
        };
    }

    fn whitespace(self, cursor: &mut Cursor) -> (State, TokenKind) {
        debug_assert!(cursor.peek(0).is_whitespace());
        cursor.skip(1);
        while cursor.skip_if(|char| char.is_whitespace()) {}
        (State::Initial(InitialState), TokenKind::Whitespace)
    }
}

#[derive(Debug)]
pub struct Cursor<'a> {
    string: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Cursor { string, index: 0 }
    }

    fn peek(&self, index: usize) -> char {
        self.string[self.index..].chars().nth(index).unwrap_or('\0')
    }

    fn skip(&mut self, count: usize) {
        self.index = self.string[self.index..]
            .char_indices()
            .nth(count)
            .map_or(self.string.len(), |(index, _)| self.index + index);
    }

    fn skip_if<P>(&mut self, predicate: P) -> bool
    where
        P: FnOnce(char) -> bool,
    {
        if predicate(self.peek(0)) {
            self.skip(1);
            true
        } else {
            false
        }
    }

    fn skip_exponent(&mut self) -> bool {
        debug_assert!(self.peek(0) == 'E' || self.peek(0) == 'e');
        self.skip(1);
        if self.peek(0) == '+' || self.peek(0) == '-' {
            self.skip(1);
        }
        self.skip_digits(10)
    }

    fn skip_digits(&mut self, radix: u32) -> bool {
        let mut has_skip_digits = false;
        loop {
            match self.peek(0) {
                '_' => {
                    self.skip(1);
                }
                char if char.is_digit(radix) => {
                    self.skip(1);
                    has_skip_digits = true;
                }
                _ => break,
            }
        }
        has_skip_digits
    }

    fn skip_suffix(&mut self) -> bool {
        if self.peek(0).is_identifier_start() {
            self.skip(1);
            while self.skip_if(|char| char.is_identifier_continue()) {}
            return true;
        }
        false
    }
}

pub trait CharExt {
    fn is_identifier_start(self) -> bool;
    fn is_identifier_continue(self) -> bool;
}

impl CharExt for char {
    fn is_identifier_start(self) -> bool {
        match self {
            'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }

    fn is_identifier_continue(self) -> bool {
        match self {
            '0'..='9' | 'A'..='Z' | '_' | 'a'..='z' => true,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InlineWidget {
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    pub height: f64,
}
use crate::{char::CharExt, line::Inline, str::StrExt, Line};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct WrapData {
    pub wraps: Vec<usize>,
    pub indent_column_count: usize,
}

pub fn compute_wrap_data(line: Line<'_>, wrap_column: usize, tab_column_count: usize) -> WrapData {
    let mut indent_column_count: usize = line
        .text
        .indentation()
        .unwrap_or("")
        .chars()
        .map(|char| char.column_count(tab_column_count))
        .sum();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if indent_column_count + column_count > wrap_column {
                        indent_column_count = 0;
                        break;
                    }
                }
            }
            Inline::Widget(widget) => {
                if indent_column_count + widget.column_count > wrap_column {
                    indent_column_count = 0;
                    break;
                }
            }
        }
    }
    let mut byte = 0;
    let mut column = 0;
    let mut wraps = Vec::new();
    for inline in line.inlines() {
        match inline {
            Inline::Text { text, .. } => {
                for string in text.split_whitespace_boundaries() {
                    let column_count: usize = string
                        .chars()
                        .map(|char| char.column_count(tab_column_count))
                        .sum();
                    if column + column_count > wrap_column {
                        column = indent_column_count;
                        wraps.push(byte);
                    }
                    column += column_count;
                    byte += string.len();
                }
            }
            Inline::Widget(widget) => {
                if column + widget.column_count > wrap_column {
                    column = indent_column_count;
                    wraps.push(byte);
                }
                column += widget.column_count;
                byte += 1;
            }
        }
    }
    WrapData {
        wraps,
        indent_column_count,
    }
}