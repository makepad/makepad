#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}
use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    let mut context = self.state.code_editor.context(self.state.view_id);
                    self.code_editor.draw(&mut cx, &mut context);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, &mut self.state.code_editor, self.state.view_id, event)
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{state::ViewId, Affinity, Context, Document, Position, Selection, State},
    makepad_widgets::*,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

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

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
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

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.begin(cx, context);
        let document = context.document();
        self.draw_text(cx, &document);
        self.draw_selections(cx, &document);
        self.end(cx, context);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.context(view_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.context(view_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.context(view_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.context(view_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.fold_line(line, 2 * settings.indent_column_count);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.unfold_line(line);
                    }
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    if alt {
                        context.insert_cursor(cursor);
                    } else {
                        context.set_cursor(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    context.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        context.wrap_lines((self.viewport_rect.size.x / self.cell_size.x) as usize);
        let document = context.document();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        let document = context.document();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if context.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt, token::TokenKind};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                document::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        let mut active_selection = None;
        let mut selections = document.selections();
        while selections
            .first()
            .map_or(false, |selection| selection.end().0.line < self.start_line)
        {
            selections = &selections[1..];
        }
        if selections.first().map_or(false, |selection| {
            selection.start().0.line < self.start_line
        }) {
            let (selection, remaining_selections) = selections.split_first().unwrap();
            selections = remaining_selections;
            active_selection = Some(ActiveSelection::new(*selection, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, document)
    }

    fn pick(&self, document: &Document<'_>, pos: DVec2) -> Option<(Position, Affinity)> {
        use crate::{document, line, str::StrExt};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, byte),
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, next_byte),
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_column_count);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, byte), Affinity::After));
                    }
                    line += 1;
                    y += next_y;
                }
                document::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, 0), Affinity::Before));
                    }
                    y = next_y;
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: &'a [Selection],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_column_count);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                column += 1;
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                document::Element::Line(true, line_ref) => {
                    y += line_ref.height();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Affinity,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let position = Position::new(line, byte);
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == (position, bias)
        }) {
            self.draw_selection(cx, x, y, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self
            .selections
            .first()
            .map_or(false, |selection| selection.start() == (position, bias))
        {
            let (selection, selections) = self.selections.split_first().unwrap();
            self.selections = selections;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection: *selection,
                    start_x: x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

impl ActiveSelection {
    fn new(selection: Selection, start_x: f64) -> Self {
        Self { selection, start_x }
    }
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

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
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
use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    start_column_after_wrap: &'a mut Vec<usize>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        start_column_after_wrap: &'a mut Vec<usize>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.start_column_after_wrap,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = Vec::new();
            mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            let line_ref = document.line(line);
            let mut start_column_after_wrap = line_ref
                .text()
                .indentation()
                .column_count(document.settings().tab_column_count);
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            if start_column_after_wrap
                                + string.column_count(document.settings().tab_column_count)
                                > max_column
                            {
                                start_column_after_wrap = 0;
                            }
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        if start_column_after_wrap + widget.column_count > max_column {
                            start_column_after_wrap = 0;
                        }
                    }
                }
            }
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = start_column_after_wrap;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = start_column_after_wrap;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            self.start_column_after_wrap[line] = start_column_after_wrap;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
        *self.latest_selection_index = 0;
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_column: usize) {
        self.fold_column[line_index] = fold_column;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if !current_selection.should_merge(next_selection) {
                current_selection_index += 1;
                continue;
            }
            let start = current_selection.start().min(next_selection.start());
            let end = current_selection.end().max(next_selection.end());
            let anchor;
            let cursor;
            if current_selection.anchor <= next_selection.cursor {
                anchor = start;
                cursor = end;
            } else {
                anchor = end;
                cursor = start;
            }
            self.selections[current_selection_index] =
                Selection::new(anchor, cursor, current_selection.preferred_column);
            self.selections.remove(next_selection_index);
            if next_selection_index < *self.latest_selection_index {
                *self.latest_selection_index -= 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            let anchor;
            let cursor;
            if selection.anchor <= selection.cursor {
                anchor = (diffed_start, selection.start().1);
                cursor = (diffed_end, selection.end().1);
            } else {
                anchor = (diffed_end, selection.end().1);
                cursor = (diffed_start, selection.start().1);
            }
            *selection = Selection::new(anchor, cursor, selection.preferred_column);
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.start_column_after_wrap.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_column_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_column
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
use {
    crate::{Length, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Diff {
    operations: Vec<Operation>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.operations.iter(),
        }
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut operations_0 = self.operations.into_iter();
        let mut operations_1 = other.operations.into_iter();
        let mut operation_slot_0 = operations_0.next();
        let mut operation_slot_1 = operations_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(length_0)), Some(Operation::Retain(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(length_0)), Some(Operation::Delete(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            let text_length = text.length();
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length - text_length));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(length));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(text.length() - length));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(length);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(len)), None) => {
                    builder.retain(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(len)), op) => {
                    builder.delete(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = op;
                }
                (None, Some(Operation::Retain(len))) => {
                    builder.retain(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, Some(Operation::Delete(len))) => {
                    builder.delete(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, None) => break,
                (op, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = op;
                    operation_slot_1 = operations_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Operation;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Operation;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.operations.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delete(&mut self, length: Length) {
        use std::mem;

        if length == Length::default() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_length)] => {
                *last_length += length;
            }
            [.., Operation::Delete(second_last_length), Operation::Insert(_)] => {
                *second_last_length += length;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(length));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(length)),
        }
    }

    pub fn retain(&mut self, length: Length) {
        if length == Length::default() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_length)) => {
                *last_length += length;
            }
            _ => self.operations.push(Operation::Retain(length)),
        }
    }

    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.operations.push(Operation::Insert(text)),
        }
    }

    pub fn finish(mut self) -> Diff {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Diff {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Operation>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Operation>,
}

impl Iterator for IntoIter {
    type Item = Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    Delete(Length),
    Retain(Length),
    Insert(Text),
}

impl Operation {
    pub fn info(&self) -> OperationInfo {
        match *self {
            Self::Delete(length) => OperationInfo::Delete(length),
            Self::Retain(length) => OperationInfo::Retain(length),
            Self::Insert(ref text) => OperationInfo::Insert(text.length()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationInfo {
    Delete(Length),
    Retain(Length),
    Insert(Length),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Strategy {
    InsertBefore,
    InsertAfter,
}
use {
    crate::{line, token::TokenInfo, Affinity, Line, Selection, Settings, Text, Tokenizer},
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Document<'a> {
    settings: &'a Settings,
    text: &'a Text,
    tokenizer: &'a Tokenizer,
    text_inlays: &'a [Vec<(usize, String)>],
    line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
    wrap_bytes: &'a [Vec<usize>],
    start_column_after_wrap: &'a [usize],
    fold_column: &'a [usize],
    scale: &'a [f64],
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    summed_heights: &'a [f64],
    selections: &'a [Selection],
    latest_selection_index: usize,
}

impl<'a> Document<'a> {
    pub fn new(
        settings: &'a Settings,
        text: &'a Text,
        tokenizer: &'a Tokenizer,
        text_inlays: &'a [Vec<(usize, String)>],
        line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
        wrap_bytes: &'a [Vec<usize>],
        start_column_after_wrap: &'a [usize],
        fold_column: &'a [usize],
        scale: &'a [f64],
        line_inlays: &'a [(usize, LineInlay)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        summed_heights: &'a [f64],
        selections: &'a [Selection],
        latest_selection_index: usize,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
        }
    }

    pub fn settings(&self) -> &'a Settings {
        self.settings
    }

    pub fn compute_width(&self) -> f64 {
        let mut max_width = 0.0f64;
        for element in self.elements(0, self.line_count()) {
            max_width = max_width.max(match element {
                Element::Line(_, line) => line.compute_width(self.settings.tab_column_count),
                Element::Widget(_, widget) => widget.width,
            });
        }
        max_width
    }

    pub fn height(&self) -> f64 {
        self.summed_heights[self.line_count() - 1]
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => {
                if line_index == self.line_count() {
                    line_index
                } else {
                    line_index + 1
                }
            }
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.as_lines().len()
    }

    pub fn line(&self, line: usize) -> Line<'a> {
        Line::new(
            &self.text.as_lines()[line],
            &self.tokenizer.token_infos()[line],
            &self.text_inlays[line],
            &self.line_widget_inlays[line],
            &self.wrap_bytes[line],
            self.start_column_after_wrap[line],
            self.fold_column[line],
            self.scale[line],
        )
    }

    pub fn lines(&self, start_line: usize, end_line: usize) -> Lines<'a> {
        Lines {
            text: self.text.as_lines()[start_line..end_line].iter(),
            token_infos: self.tokenizer.token_infos()[start_line..end_line].iter(),
            text_inlays: self.text_inlays[start_line..end_line].iter(),
            line_widget_inlays: self.line_widget_inlays[start_line..end_line].iter(),
            wrap_bytes: self.wrap_bytes[start_line..end_line].iter(),
            start_column_after_wrap: self.start_column_after_wrap[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
        }
    }

    pub fn line_y(&self, line: usize) -> f64 {
        if line == 0 {
            0.0
        } else {
            self.summed_heights[line - 1]
        }
    }

    pub fn elements(&self, start_line: usize, end_line: usize) -> Elements<'a> {
        Elements {
            lines: self.lines(start_line, end_line),
            line_inlays: &self.line_inlays[self
                .line_inlays
                .iter()
                .position(|(line, _)| *line >= start_line)
                .unwrap_or(self.line_inlays.len())..],
            widget_inlays: &self.widget_inlays[self
                .widget_inlays
                .iter()
                .position(|((line, _), _)| *line >= start_line)
                .unwrap_or(self.widget_inlays.len())..],
            line: start_line,
        }
    }

    pub fn selections(&self) -> &'a [Selection] {
        self.selections
    }

    pub fn latest_selection_index(&self) -> usize {
        self.latest_selection_index
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
    token_infos: slice::Iter<'a, Vec<TokenInfo>>,
    text_inlays: slice::Iter<'a, Vec<(usize, String)>>,
    line_widget_inlays: slice::Iter<'a, Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: slice::Iter<'a, Vec<usize>>,
    start_column_after_wrap: slice::Iter<'a, usize>,
    fold_column: slice::Iter<'a, usize>,
    scale: slice::Iter<'a, f64>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line::new(
            self.text.next()?,
            self.token_infos.next()?,
            self.text_inlays.next()?,
            self.line_widget_inlays.next()?,
            self.wrap_bytes.next()?,
            *self.start_column_after_wrap.next()?,
            *self.fold_column.next()?,
            *self.scale.next()?,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    lines: Lines<'a>,
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    line: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .line_inlays
            .first()
            .map_or(false, |(line, _)| *line == self.line)
        {
            let ((_, line), line_inlays) = self.line_inlays.split_first().unwrap();
            self.line_inlays = line_inlays;
            return Some(Element::Line(true, line.as_line()));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let line = self.lines.next()?;
        self.line += 1;
        Some(Element::Line(false, line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element<'a> {
    Line(bool, Line<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LineInlay {
    text: String,
}

impl LineInlay {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_line(&self) -> Line<'_> {
        Line::new(&self.text, &[], &[], &[], &[], 0, 0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: f64,
    pub height: f64,
}

impl Widget {
    pub fn new(id: usize, width: f64, height: f64) -> Self {
        Self { id, width, height }
    }
}
use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: Range) -> Diff {
    replace(range, "\n".into())
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Position::new(
            position.line,
            text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        );
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Position::new(prev_line, text.as_lines()[prev_line].len());
    }
    position
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Length {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Length {
    pub fn new(line_count: usize, byte_count: usize) -> Self {
        Self {
            line_count,
            byte_count,
        }
    }
}

impl Add for Length {
    type Output = Length;

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

impl AddAssign for Length {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Length {
    type Output = Length;

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

impl SubAssign for Length {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
pub mod bias;
pub mod char;
pub mod code_editor;
pub mod context;
pub mod diff;
pub mod document;
pub mod edit_ops;
pub mod length;
pub mod line;
pub mod move_ops;
pub mod position;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;

pub use crate::{
    bias::Affinity, code_editor::CodeEditor, context::Context, diff::Diff, document::Document,
    length::Length, line::Line, position::Position, range::Range, selection::Selection,
    settings::Settings, state::State, text::Text, token::Token, tokenizer::Tokenizer,
};
use {
    crate::{
        token::{TokenInfo, TokenKind},
        Affinity, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap_bytes: &'a [usize],
    start_column_after_wrap: usize,
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap_bytes: &'a [usize],
        start_column_after_wrap: usize,
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
        }
    }

    pub fn compute_column_count(&self, tab_column_count: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_column_count = 0;
        let mut summed_column_count = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    summed_column_count += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_column_count += widget.column_count;
                }
                WrappedElement::Wrap => {
                    max_summed_column_count = max_summed_column_count.max(summed_column_count);
                    summed_column_count = self.start_column_after_wrap();
                }
            }
        }
        max_summed_column_count.max(summed_column_count)
    }

    pub fn row_count(&self) -> usize {
        self.wrap_bytes.len() + 1
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        self.column_to_x(self.compute_column_count(tab_column_count))
    }

    pub fn height(&self) -> f64 {
        self.scale * self.row_count() as f64
    }

    pub fn byte_bias_to_row_column(
        &self,
        (byte, bias): (usize, Affinity),
        tab_column_count: usize,
    ) -> (usize, usize) {
        use crate::str::StrExt;

        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if byte == current_byte && bias == Affinity::Before {
            return (row, column);
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        if byte == current_byte && bias == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if byte == current_byte && bias == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                WrappedElement::Token(true, token) => {
                    column += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    row += 1;
                    column = self.start_column_after_wrap();
                }
            }
        }
        if byte == current_byte && bias == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_column_to_byte_bias(
        &self,
        (row, column): (usize, usize),
        tab_column_count: usize,
    ) -> (usize, Affinity) {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut current_row = 0;
        let mut current_column = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_column_count);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte = byte + grapheme.len();
                        current_column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = current_column + token.text.column_count(tab_column_count);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    current_column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.start_column_after_wrap();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold_column = column.min(self.fold_column);
        let column_count_after_fold_column = column - column_count_before_fold_column;
        column_count_before_fold_column as f64 + self.scale * column_count_after_fold_column as f64
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn tokens(&self) -> Tokens<'a> {
        Tokens {
            text: self.text,
            token_infos: self.token_infos.iter(),
        }
    }

    pub fn elements(&self) -> Elements<'a> {
        let mut tokens = self.tokens();
        Elements {
            token: tokens.next(),
            tokens,
            text_inlays: self.text_inlays,
            widget_inlays: self.widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap_bytes: self.wrap_bytes,
            byte: 0,
        }
    }

    pub fn start_column_after_wrap(&self) -> usize {
        self.start_column_after_wrap
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }
}

#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    token_infos: slice::Iter<'a, TokenInfo>,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.token_infos.next() {
            Some(token_info) => {
                let (text_0, text_1) = self.text.split_at(token_info.byte_count);
                self.text = text_1;
                Token::new(text_0, token_info.kind)
            }
            None => {
                if self.text.is_empty() {
                    return None;
                }
                let text = self.text;
                self.text = "";
                Token::new(text, TokenKind::Unknown)
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    byte: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.widget_inlays.first() {
            byte_count = byte_count.min(byte - self.byte);
        }
        let token = if byte_count < token.text.len() {
            let (text_0, text_1) = token.text.split_at(byte_count);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
        } else {
            self.token = self.tokens.next();
            token
        };
        self.byte += token.text.len();
        Some(Element::Token(false, token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap_bytes: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Affinity::Before, widget));
        }
        if self
            .wrap_bytes
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.wrap_bytes = &self.wrap_bytes[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.wrap_bytes.first() {
                    byte_count = byte_count.min(*byte - self.byte);
                }
                let token = if byte_count < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(byte_count);
                    self.element = Some(Element::Token(is_inlay, Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.byte += token.text.len();
                WrappedElement::Token(is_inlay, token)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Affinity::After, widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub column_count: usize,
}

impl Widget {
    pub fn new(id: usize, column_count: usize) -> Self {
        Self { id, column_count }
    }
}
mod app;

fn main() {
    app::app_main();
}
use crate::{Affinity, Document, Position};

pub fn move_left(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    ((position, Affinity::Before), None)
}

pub fn move_right(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    ((position, Affinity::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_first_row_of_line(document, (position, bias)) {
        return move_to_prev_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_first_line(position) {
        return move_to_last_row_of_prev_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_last_row_of_line(document, (position, bias)) {
        return move_to_next_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_last_line(document, position) {
        return move_to_first_row_of_next_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Position) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    document
        .line(position.line)
        .byte_bias_to_row_column(
            (position.byte, bias),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    let line = document.line(position.line);
    line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    )
    .0 == line.row_count() - 1
}

fn is_at_first_line(position: Position) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Position) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    (
        (
            Position::new(
                position.line,
                document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            ),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        (
            Position::new(
                position.line,
                line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            ),
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    let prev_line = position.line - 1;
    (
        (
            Position::new(prev_line, document.line(prev_line).text().len()),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Position) -> ((Position, Affinity), Option<usize>) {
    (
        (Position::new(position.line + 1, 0), Affinity::Before),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row - 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row + 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = position.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, bias) = prev_line_ref.row_column_to_byte_bias(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    ((Position::new(prev_line, byte), bias), Some(column))
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = position.line + 1;
    let (byte, bias) = document
        .line(next_line)
        .row_column_to_byte_bias((0, column), document.settings().tab_column_count);
    ((Position::new(next_line, byte), bias), Some(column))
}
use {
    crate::{diff::Strategy, Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }

    pub fn apply_diff(self, diff: &Diff, strategy: Strategy) -> Position {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_position = Position::default();
        let mut distance_to_position = self - Position::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_position += length;
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_position + distance_to_position;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_position == Length::default() {
                        break match strategy {
                            Strategy::InsertBefore => diffed_position + length,
                            Strategy::InsertAfter => diffed_position,
                        };
                    } else {
                        diffed_position += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_position = Length::default();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_position + distance_to_position;
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.byte_count,
            }
        } else {
            Self {
                line: self.line + length.line_count,
                byte: length.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Length {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Length {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }
}
use crate::{Affinity, Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Position, Affinity),
        cursor: (Position, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(mut self, mut other: Self) -> bool {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Position, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Position, Affinity) {
        self.anchor.max(self.cursor)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce((Position, Affinity), Option<usize>) -> ((Position, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub indent_column_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            indent_column_count: 4,
        }
    }
}
use {
    crate::{
        document, document::LineInlay, line, Affinity, Context, Document, Selection, Settings,
        Text, Tokenizer,
    },
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    view_id: usize,
    views: HashMap<ViewId, View>,
    editor_id: usize,
    editors: HashMap<EditorId, Editor>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn document(&self, view_id: ViewId) -> Document<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        Document::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.start_column_after_wrap,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
            view.latest_selection_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut self.settings,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.as_lines().len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
                start_column_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
                latest_selection_index: 0,
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.context(view_id).update_summed_heights();
        Ok(view_id)
    }

    fn open_editor(&mut self, path: impl AsRef<Path>) -> io::Result<EditorId> {
        use std::fs;

        let editor_id = EditorId(self.editor_id);
        self.editor_id += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                tokenizer,
                text_inlays: (0..line_count)
                    .map(|line| {
                        if line % 2 == 0 {
                            [
                                (20, "###".into()),
                                (40, "###".into()),
                                (60, "###".into()),
                                (80, "###".into()),
                            ]
                            .into()
                        } else {
                            [].into()
                        }
                    })
                    .collect(),
                line_inlays: [
                    (
                        10,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        20,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        30,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        40,
                        LineInlay::new("##################################################".into()),
                    ),
                ]
                .into(),
                line_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                document_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct View {
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Affinity), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Affinity), document::Widget)>,
}
pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize;
    fn indentation(&self) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        use crate::char::CharExt;

        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize {
        self.indentation().column_count(tab_column_count) / indent_column_count
    }

    fn indentation(&self) -> &str {
        &self[..self
            .char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(self.len())]
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
        let mut prev_grapheme_is_whitespace = None;
        let index = self
            .string
            .grapheme_indices()
            .find_map(|(index, next_grapheme)| {
                let next_grapheme_is_whitespace =
                    next_grapheme.chars().all(|char| char.is_whitespace());
                let is_whitespace_boundary =
                    prev_grapheme_is_whitespace.map_or(false, |prev_grapheme_is_whitespace| {
                        prev_grapheme_is_whitespace != next_grapheme_is_whitespace
                    });
                prev_grapheme_is_whitespace = Some(next_grapheme_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(string)
    }
}
use {
    crate::{Diff, Length, Position, Range},
    std::{borrow::Cow, ops::AddAssign},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.length() == Length::default()
    }

    pub fn length(&self) -> Length {
        Length {
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

    pub fn take(&mut self, len: Length) -> Self {
        let mut lines = self
            .lines
            .drain(..len.line_count as usize)
            .collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte_count].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Length) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, position: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line][..position.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line][position.byte..]);
            self.lines
                .splice(position.line..position.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte + length.byte_count, "");
        } else {
            let mut line = self.lines[position.line][..position.byte].to_string();
            line.push_str(&self.lines[position.line + length.line_count][length.byte_count..]);
            self.lines.splice(
                position.line..position.line + length.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Operation;

        let mut position = Position::default();
        for operation in diff {
            match operation {
                Operation::Delete(length) => self.delete(position, length),
                Operation::Retain(length) => position += length,
                Operation::Insert(text) => {
                    let length = text.length();
                    self.insert(position, text);
                    position += length;
                }
            }
        }
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, mut other: Self) {
        other
            .lines
            .first_mut()
            .unwrap()
            .replace_range(..0, self.lines.last().unwrap());
        self.lines
            .splice(self.lines.len() - 1..self.lines.len(), other.lines);
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<char> for Text {
    fn from(char: char) -> Self {
        Self {
            lines: match char {
                '\n' | '\r' => vec![String::new(), String::new()],
                _ => vec![char.into()],
            },
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.split('\n').map(|line| line.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
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

impl From<Cow<'_, str>> for Text {
    fn from(string: Cow<'_, str>) -> Self {
        string.as_ref().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    pub fn new(text: &'a str, kind: TokenKind) -> Self {
        Self { text, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub byte_count: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self {
            byte_count: len,
            kind,
        }
    }
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
use crate::{
    token::{TokenInfo, TokenKind},
    Diff, Text,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
    token_infos: Vec<Vec<TokenInfo>>,
}

impl Tokenizer {
    pub fn new(text: &Text) -> Self {
        let line_count = text.as_lines().len();
        let mut tokenizer = Self {
            state: (0..line_count).map(|_| None).collect(),
            token_infos: (0..line_count).map(|_| Vec::new()).collect(),
        };
        tokenizer.retokenize(&Diff::new(), text);
        tokenizer
    }

    pub fn token_infos(&self) -> &[Vec<TokenInfo>] {
        &self.token_infos
    }

    pub fn retokenize(&mut self, diff: &Diff, text: &Text) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    self.state.drain(line..line + length.line_count);
                    self.token_infos.drain(line..line + length.line_count);
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                    self.state
                        .splice(line..line, (0..length.line_count).map(|_| None));
                    self.token_infos
                        .splice(line..line, (0..length.line_count).map(|_| Vec::new()));
                    line += length.line_count;
                }
            }
        }
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut token_infos = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => token_infos.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    self.token_infos[line] = token_infos;
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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenInfo>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token_kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (next_state, Some(TokenInfo::new(end - start, token_kind)))
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}
use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    let mut context = self.state.code_editor.context(self.state.view_id);
                    self.code_editor.draw(&mut cx, &mut context);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, &mut self.state.code_editor, self.state.view_id, event)
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{state::ViewId, Affinity, Context, Document, Position, Selection, State},
    makepad_widgets::*,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

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

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
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

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.begin(cx, context);
        let document = context.document();
        self.draw_text(cx, &document);
        self.draw_selections(cx, &document);
        self.end(cx, context);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.context(view_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.context(view_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.context(view_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.context(view_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.fold_line(line, 2 * settings.indent_column_count);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.unfold_line(line);
                    }
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    if alt {
                        context.insert_cursor(cursor);
                    } else {
                        context.set_cursor(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    context.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        context.wrap_lines((self.viewport_rect.size.x / self.cell_size.x) as usize);
        let document = context.document();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        let document = context.document();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if context.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt, token::TokenKind};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                document::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        let mut active_selection = None;
        let mut selections = document.selections();
        while selections
            .first()
            .map_or(false, |selection| selection.end().0.line < self.start_line)
        {
            selections = &selections[1..];
        }
        if selections.first().map_or(false, |selection| {
            selection.start().0.line < self.start_line
        }) {
            let (selection, remaining_selections) = selections.split_first().unwrap();
            selections = remaining_selections;
            active_selection = Some(ActiveSelection::new(*selection, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, document)
    }

    fn pick(&self, document: &Document<'_>, pos: DVec2) -> Option<(Position, Affinity)> {
        use crate::{document, line, str::StrExt};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, byte),
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, next_byte),
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_column_count);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, byte), Affinity::After));
                    }
                    line += 1;
                    y += next_y;
                }
                document::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, 0), Affinity::Before));
                    }
                    y = next_y;
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: &'a [Selection],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_column_count);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                column += 1;
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                document::Element::Line(true, line_ref) => {
                    y += line_ref.height();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Affinity,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let position = Position::new(line, byte);
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == (position, bias)
        }) {
            self.draw_selection(cx, x, y, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self
            .selections
            .first()
            .map_or(false, |selection| selection.start() == (position, bias))
        {
            let (selection, selections) = self.selections.split_first().unwrap();
            self.selections = selections;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection: *selection,
                    start_x: x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

impl ActiveSelection {
    fn new(selection: Selection, start_x: f64) -> Self {
        Self { selection, start_x }
    }
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

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
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
use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    start_column_after_wrap: &'a mut Vec<usize>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        start_column_after_wrap: &'a mut Vec<usize>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.start_column_after_wrap,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = Vec::new();
            mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            let line_ref = document.line(line);
            let mut start_column_after_wrap = line_ref
                .text()
                .indentation()
                .column_count(document.settings().tab_column_count);
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            if start_column_after_wrap
                                + string.column_count(document.settings().tab_column_count)
                                > max_column
                            {
                                start_column_after_wrap = 0;
                            }
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        if start_column_after_wrap + widget.column_count > max_column {
                            start_column_after_wrap = 0;
                        }
                    }
                }
            }
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = start_column_after_wrap;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = start_column_after_wrap;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            self.start_column_after_wrap[line] = start_column_after_wrap;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
        *self.latest_selection_index = 0;
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_column: usize) {
        self.fold_column[line_index] = fold_column;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if !current_selection.should_merge(next_selection) {
                current_selection_index += 1;
                continue;
            }
            let start = current_selection.start().min(next_selection.start());
            let end = current_selection.end().max(next_selection.end());
            let anchor;
            let cursor;
            if current_selection.anchor <= next_selection.cursor {
                anchor = start;
                cursor = end;
            } else {
                anchor = end;
                cursor = start;
            }
            self.selections[current_selection_index] =
                Selection::new(anchor, cursor, current_selection.preferred_column);
            self.selections.remove(next_selection_index);
            if next_selection_index < *self.latest_selection_index {
                *self.latest_selection_index -= 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            let anchor;
            let cursor;
            if selection.anchor <= selection.cursor {
                anchor = (diffed_start, selection.start().1);
                cursor = (diffed_end, selection.end().1);
            } else {
                anchor = (diffed_end, selection.end().1);
                cursor = (diffed_start, selection.start().1);
            }
            *selection = Selection::new(anchor, cursor, selection.preferred_column);
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.start_column_after_wrap.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_column_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_column
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
use {
    crate::{Length, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Diff {
    operations: Vec<Operation>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.operations.iter(),
        }
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut operations_0 = self.operations.into_iter();
        let mut operations_1 = other.operations.into_iter();
        let mut operation_slot_0 = operations_0.next();
        let mut operation_slot_1 = operations_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(length_0)), Some(Operation::Retain(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(length_0)), Some(Operation::Delete(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            let text_length = text.length();
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length - text_length));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(length));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(text.length() - length));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(length);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(len)), None) => {
                    builder.retain(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(len)), op) => {
                    builder.delete(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = op;
                }
                (None, Some(Operation::Retain(len))) => {
                    builder.retain(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, Some(Operation::Delete(len))) => {
                    builder.delete(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, None) => break,
                (op, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = op;
                    operation_slot_1 = operations_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Operation;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Operation;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.operations.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delete(&mut self, length: Length) {
        use std::mem;

        if length == Length::default() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_length)] => {
                *last_length += length;
            }
            [.., Operation::Delete(second_last_length), Operation::Insert(_)] => {
                *second_last_length += length;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(length));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(length)),
        }
    }

    pub fn retain(&mut self, length: Length) {
        if length == Length::default() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_length)) => {
                *last_length += length;
            }
            _ => self.operations.push(Operation::Retain(length)),
        }
    }

    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.operations.push(Operation::Insert(text)),
        }
    }

    pub fn finish(mut self) -> Diff {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Diff {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Operation>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Operation>,
}

impl Iterator for IntoIter {
    type Item = Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    Delete(Length),
    Retain(Length),
    Insert(Text),
}

impl Operation {
    pub fn info(&self) -> OperationInfo {
        match *self {
            Self::Delete(length) => OperationInfo::Delete(length),
            Self::Retain(length) => OperationInfo::Retain(length),
            Self::Insert(ref text) => OperationInfo::Insert(text.length()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationInfo {
    Delete(Length),
    Retain(Length),
    Insert(Length),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Strategy {
    InsertBefore,
    InsertAfter,
}
use {
    crate::{line, token::TokenInfo, Affinity, Line, Selection, Settings, Text, Tokenizer},
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Document<'a> {
    settings: &'a Settings,
    text: &'a Text,
    tokenizer: &'a Tokenizer,
    text_inlays: &'a [Vec<(usize, String)>],
    line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
    wrap_bytes: &'a [Vec<usize>],
    start_column_after_wrap: &'a [usize],
    fold_column: &'a [usize],
    scale: &'a [f64],
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    summed_heights: &'a [f64],
    selections: &'a [Selection],
    latest_selection_index: usize,
}

impl<'a> Document<'a> {
    pub fn new(
        settings: &'a Settings,
        text: &'a Text,
        tokenizer: &'a Tokenizer,
        text_inlays: &'a [Vec<(usize, String)>],
        line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
        wrap_bytes: &'a [Vec<usize>],
        start_column_after_wrap: &'a [usize],
        fold_column: &'a [usize],
        scale: &'a [f64],
        line_inlays: &'a [(usize, LineInlay)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        summed_heights: &'a [f64],
        selections: &'a [Selection],
        latest_selection_index: usize,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
        }
    }

    pub fn settings(&self) -> &'a Settings {
        self.settings
    }

    pub fn compute_width(&self) -> f64 {
        let mut max_width = 0.0f64;
        for element in self.elements(0, self.line_count()) {
            max_width = max_width.max(match element {
                Element::Line(_, line) => line.compute_width(self.settings.tab_column_count),
                Element::Widget(_, widget) => widget.width,
            });
        }
        max_width
    }

    pub fn height(&self) -> f64 {
        self.summed_heights[self.line_count() - 1]
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => {
                if line_index == self.line_count() {
                    line_index
                } else {
                    line_index + 1
                }
            }
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.as_lines().len()
    }

    pub fn line(&self, line: usize) -> Line<'a> {
        Line::new(
            &self.text.as_lines()[line],
            &self.tokenizer.token_infos()[line],
            &self.text_inlays[line],
            &self.line_widget_inlays[line],
            &self.wrap_bytes[line],
            self.start_column_after_wrap[line],
            self.fold_column[line],
            self.scale[line],
        )
    }

    pub fn lines(&self, start_line: usize, end_line: usize) -> Lines<'a> {
        Lines {
            text: self.text.as_lines()[start_line..end_line].iter(),
            token_infos: self.tokenizer.token_infos()[start_line..end_line].iter(),
            text_inlays: self.text_inlays[start_line..end_line].iter(),
            line_widget_inlays: self.line_widget_inlays[start_line..end_line].iter(),
            wrap_bytes: self.wrap_bytes[start_line..end_line].iter(),
            start_column_after_wrap: self.start_column_after_wrap[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
        }
    }

    pub fn line_y(&self, line: usize) -> f64 {
        if line == 0 {
            0.0
        } else {
            self.summed_heights[line - 1]
        }
    }

    pub fn elements(&self, start_line: usize, end_line: usize) -> Elements<'a> {
        Elements {
            lines: self.lines(start_line, end_line),
            line_inlays: &self.line_inlays[self
                .line_inlays
                .iter()
                .position(|(line, _)| *line >= start_line)
                .unwrap_or(self.line_inlays.len())..],
            widget_inlays: &self.widget_inlays[self
                .widget_inlays
                .iter()
                .position(|((line, _), _)| *line >= start_line)
                .unwrap_or(self.widget_inlays.len())..],
            line: start_line,
        }
    }

    pub fn selections(&self) -> &'a [Selection] {
        self.selections
    }

    pub fn latest_selection_index(&self) -> usize {
        self.latest_selection_index
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
    token_infos: slice::Iter<'a, Vec<TokenInfo>>,
    text_inlays: slice::Iter<'a, Vec<(usize, String)>>,
    line_widget_inlays: slice::Iter<'a, Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: slice::Iter<'a, Vec<usize>>,
    start_column_after_wrap: slice::Iter<'a, usize>,
    fold_column: slice::Iter<'a, usize>,
    scale: slice::Iter<'a, f64>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line::new(
            self.text.next()?,
            self.token_infos.next()?,
            self.text_inlays.next()?,
            self.line_widget_inlays.next()?,
            self.wrap_bytes.next()?,
            *self.start_column_after_wrap.next()?,
            *self.fold_column.next()?,
            *self.scale.next()?,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    lines: Lines<'a>,
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    line: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .line_inlays
            .first()
            .map_or(false, |(line, _)| *line == self.line)
        {
            let ((_, line), line_inlays) = self.line_inlays.split_first().unwrap();
            self.line_inlays = line_inlays;
            return Some(Element::Line(true, line.as_line()));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let line = self.lines.next()?;
        self.line += 1;
        Some(Element::Line(false, line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element<'a> {
    Line(bool, Line<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LineInlay {
    text: String,
}

impl LineInlay {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_line(&self) -> Line<'_> {
        Line::new(&self.text, &[], &[], &[], &[], 0, 0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: f64,
    pub height: f64,
}

impl Widget {
    pub fn new(id: usize, width: f64, height: f64) -> Self {
        Self { id, width, height }
    }
}
use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: Range) -> Diff {
    replace(range, "\n".into())
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Position::new(
            position.line,
            text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        );
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Position::new(prev_line, text.as_lines()[prev_line].len());
    }
    position
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Length {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Length {
    pub fn new(line_count: usize, byte_count: usize) -> Self {
        Self {
            line_count,
            byte_count,
        }
    }
}

impl Add for Length {
    type Output = Length;

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

impl AddAssign for Length {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Length {
    type Output = Length;

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

impl SubAssign for Length {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
pub mod bias;
pub mod char;
pub mod code_editor;
pub mod context;
pub mod diff;
pub mod document;
pub mod edit_ops;
pub mod length;
pub mod line;
pub mod move_ops;
pub mod position;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;

pub use crate::{
    bias::Affinity, code_editor::CodeEditor, context::Context, diff::Diff, document::Document,
    length::Length, line::Line, position::Position, range::Range, selection::Selection,
    settings::Settings, state::State, text::Text, token::Token, tokenizer::Tokenizer,
};
use {
    crate::{
        token::{TokenInfo, TokenKind},
        Affinity, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap_bytes: &'a [usize],
    start_column_after_wrap: usize,
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap_bytes: &'a [usize],
        start_column_after_wrap: usize,
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
        }
    }

    pub fn compute_column_count(&self, tab_column_count: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_column_count = 0;
        let mut summed_column_count = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    summed_column_count += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_column_count += widget.column_count;
                }
                WrappedElement::Wrap => {
                    max_summed_column_count = max_summed_column_count.max(summed_column_count);
                    summed_column_count = self.start_column_after_wrap();
                }
            }
        }
        max_summed_column_count.max(summed_column_count)
    }

    pub fn row_count(&self) -> usize {
        self.wrap_bytes.len() + 1
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        self.column_to_x(self.compute_column_count(tab_column_count))
    }

    pub fn height(&self) -> f64 {
        self.scale * self.row_count() as f64
    }

    pub fn byte_bias_to_row_column(
        &self,
        (byte, bias): (usize, Affinity),
        tab_column_count: usize,
    ) -> (usize, usize) {
        use crate::str::StrExt;

        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if byte == current_byte && bias == Affinity::Before {
            return (row, column);
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        if byte == current_byte && bias == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if byte == current_byte && bias == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                WrappedElement::Token(true, token) => {
                    column += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    row += 1;
                    column = self.start_column_after_wrap();
                }
            }
        }
        if byte == current_byte && bias == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_column_to_byte_bias(
        &self,
        (row, column): (usize, usize),
        tab_column_count: usize,
    ) -> (usize, Affinity) {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut current_row = 0;
        let mut current_column = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_column_count);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte = byte + grapheme.len();
                        current_column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = current_column + token.text.column_count(tab_column_count);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    current_column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.start_column_after_wrap();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold_column = column.min(self.fold_column);
        let column_count_after_fold_column = column - column_count_before_fold_column;
        column_count_before_fold_column as f64 + self.scale * column_count_after_fold_column as f64
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn tokens(&self) -> Tokens<'a> {
        Tokens {
            text: self.text,
            token_infos: self.token_infos.iter(),
        }
    }

    pub fn elements(&self) -> Elements<'a> {
        let mut tokens = self.tokens();
        Elements {
            token: tokens.next(),
            tokens,
            text_inlays: self.text_inlays,
            widget_inlays: self.widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap_bytes: self.wrap_bytes,
            byte: 0,
        }
    }

    pub fn start_column_after_wrap(&self) -> usize {
        self.start_column_after_wrap
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }
}

#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    token_infos: slice::Iter<'a, TokenInfo>,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.token_infos.next() {
            Some(token_info) => {
                let (text_0, text_1) = self.text.split_at(token_info.byte_count);
                self.text = text_1;
                Token::new(text_0, token_info.kind)
            }
            None => {
                if self.text.is_empty() {
                    return None;
                }
                let text = self.text;
                self.text = "";
                Token::new(text, TokenKind::Unknown)
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    byte: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.widget_inlays.first() {
            byte_count = byte_count.min(byte - self.byte);
        }
        let token = if byte_count < token.text.len() {
            let (text_0, text_1) = token.text.split_at(byte_count);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
        } else {
            self.token = self.tokens.next();
            token
        };
        self.byte += token.text.len();
        Some(Element::Token(false, token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap_bytes: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Affinity::Before, widget));
        }
        if self
            .wrap_bytes
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.wrap_bytes = &self.wrap_bytes[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.wrap_bytes.first() {
                    byte_count = byte_count.min(*byte - self.byte);
                }
                let token = if byte_count < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(byte_count);
                    self.element = Some(Element::Token(is_inlay, Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.byte += token.text.len();
                WrappedElement::Token(is_inlay, token)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Affinity::After, widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub column_count: usize,
}

impl Widget {
    pub fn new(id: usize, column_count: usize) -> Self {
        Self { id, column_count }
    }
}
mod app;

fn main() {
    app::app_main();
}
use crate::{Affinity, Document, Position};

pub fn move_left(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    ((position, Affinity::Before), None)
}

pub fn move_right(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    ((position, Affinity::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_first_row_of_line(document, (position, bias)) {
        return move_to_prev_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_first_line(position) {
        return move_to_last_row_of_prev_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_last_row_of_line(document, (position, bias)) {
        return move_to_next_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_last_line(document, position) {
        return move_to_first_row_of_next_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Position) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    document
        .line(position.line)
        .byte_bias_to_row_column(
            (position.byte, bias),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    let line = document.line(position.line);
    line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    )
    .0 == line.row_count() - 1
}

fn is_at_first_line(position: Position) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Position) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    (
        (
            Position::new(
                position.line,
                document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            ),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        (
            Position::new(
                position.line,
                line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            ),
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    let prev_line = position.line - 1;
    (
        (
            Position::new(prev_line, document.line(prev_line).text().len()),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Position) -> ((Position, Affinity), Option<usize>) {
    (
        (Position::new(position.line + 1, 0), Affinity::Before),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row - 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row + 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = position.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, bias) = prev_line_ref.row_column_to_byte_bias(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    ((Position::new(prev_line, byte), bias), Some(column))
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = position.line + 1;
    let (byte, bias) = document
        .line(next_line)
        .row_column_to_byte_bias((0, column), document.settings().tab_column_count);
    ((Position::new(next_line, byte), bias), Some(column))
}
use {
    crate::{diff::Strategy, Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }

    pub fn apply_diff(self, diff: &Diff, strategy: Strategy) -> Position {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_position = Position::default();
        let mut distance_to_position = self - Position::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_position += length;
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_position + distance_to_position;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_position == Length::default() {
                        break match strategy {
                            Strategy::InsertBefore => diffed_position + length,
                            Strategy::InsertAfter => diffed_position,
                        };
                    } else {
                        diffed_position += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_position = Length::default();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_position + distance_to_position;
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.byte_count,
            }
        } else {
            Self {
                line: self.line + length.line_count,
                byte: length.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Length {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Length {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }
}
use crate::{Affinity, Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Position, Affinity),
        cursor: (Position, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(mut self, mut other: Self) -> bool {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Position, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Position, Affinity) {
        self.anchor.max(self.cursor)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce((Position, Affinity), Option<usize>) -> ((Position, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub indent_column_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            indent_column_count: 4,
        }
    }
}
use {
    crate::{
        document, document::LineInlay, line, Affinity, Context, Document, Selection, Settings,
        Text, Tokenizer,
    },
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    view_id: usize,
    views: HashMap<ViewId, View>,
    editor_id: usize,
    editors: HashMap<EditorId, Editor>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn document(&self, view_id: ViewId) -> Document<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        Document::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.start_column_after_wrap,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
            view.latest_selection_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut self.settings,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.as_lines().len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
                start_column_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
                latest_selection_index: 0,
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.context(view_id).update_summed_heights();
        Ok(view_id)
    }

    fn open_editor(&mut self, path: impl AsRef<Path>) -> io::Result<EditorId> {
        use std::fs;

        let editor_id = EditorId(self.editor_id);
        self.editor_id += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                tokenizer,
                text_inlays: (0..line_count)
                    .map(|line| {
                        if line % 2 == 0 {
                            [
                                (20, "###".into()),
                                (40, "###".into()),
                                (60, "###".into()),
                                (80, "###".into()),
                            ]
                            .into()
                        } else {
                            [].into()
                        }
                    })
                    .collect(),
                line_inlays: [
                    (
                        10,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        20,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        30,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        40,
                        LineInlay::new("##################################################".into()),
                    ),
                ]
                .into(),
                line_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                document_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct View {
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Affinity), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Affinity), document::Widget)>,
}
pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize;
    fn indentation(&self) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        use crate::char::CharExt;

        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize {
        self.indentation().column_count(tab_column_count) / indent_column_count
    }

    fn indentation(&self) -> &str {
        &self[..self
            .char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(self.len())]
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
        let mut prev_grapheme_is_whitespace = None;
        let index = self
            .string
            .grapheme_indices()
            .find_map(|(index, next_grapheme)| {
                let next_grapheme_is_whitespace =
                    next_grapheme.chars().all(|char| char.is_whitespace());
                let is_whitespace_boundary =
                    prev_grapheme_is_whitespace.map_or(false, |prev_grapheme_is_whitespace| {
                        prev_grapheme_is_whitespace != next_grapheme_is_whitespace
                    });
                prev_grapheme_is_whitespace = Some(next_grapheme_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(string)
    }
}
use {
    crate::{Diff, Length, Position, Range},
    std::{borrow::Cow, ops::AddAssign},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.length() == Length::default()
    }

    pub fn length(&self) -> Length {
        Length {
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

    pub fn take(&mut self, len: Length) -> Self {
        let mut lines = self
            .lines
            .drain(..len.line_count as usize)
            .collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte_count].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Length) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, position: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line][..position.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line][position.byte..]);
            self.lines
                .splice(position.line..position.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte + length.byte_count, "");
        } else {
            let mut line = self.lines[position.line][..position.byte].to_string();
            line.push_str(&self.lines[position.line + length.line_count][length.byte_count..]);
            self.lines.splice(
                position.line..position.line + length.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Operation;

        let mut position = Position::default();
        for operation in diff {
            match operation {
                Operation::Delete(length) => self.delete(position, length),
                Operation::Retain(length) => position += length,
                Operation::Insert(text) => {
                    let length = text.length();
                    self.insert(position, text);
                    position += length;
                }
            }
        }
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, mut other: Self) {
        other
            .lines
            .first_mut()
            .unwrap()
            .replace_range(..0, self.lines.last().unwrap());
        self.lines
            .splice(self.lines.len() - 1..self.lines.len(), other.lines);
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<char> for Text {
    fn from(char: char) -> Self {
        Self {
            lines: match char {
                '\n' | '\r' => vec![String::new(), String::new()],
                _ => vec![char.into()],
            },
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.split('\n').map(|line| line.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
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

impl From<Cow<'_, str>> for Text {
    fn from(string: Cow<'_, str>) -> Self {
        string.as_ref().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    pub fn new(text: &'a str, kind: TokenKind) -> Self {
        Self { text, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub byte_count: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self {
            byte_count: len,
            kind,
        }
    }
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
use crate::{
    token::{TokenInfo, TokenKind},
    Diff, Text,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
    token_infos: Vec<Vec<TokenInfo>>,
}

impl Tokenizer {
    pub fn new(text: &Text) -> Self {
        let line_count = text.as_lines().len();
        let mut tokenizer = Self {
            state: (0..line_count).map(|_| None).collect(),
            token_infos: (0..line_count).map(|_| Vec::new()).collect(),
        };
        tokenizer.retokenize(&Diff::new(), text);
        tokenizer
    }

    pub fn token_infos(&self) -> &[Vec<TokenInfo>] {
        &self.token_infos
    }

    pub fn retokenize(&mut self, diff: &Diff, text: &Text) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    self.state.drain(line..line + length.line_count);
                    self.token_infos.drain(line..line + length.line_count);
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                    self.state
                        .splice(line..line, (0..length.line_count).map(|_| None));
                    self.token_infos
                        .splice(line..line, (0..length.line_count).map(|_| Vec::new()));
                    line += length.line_count;
                }
            }
        }
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut token_infos = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => token_infos.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    self.token_infos[line] = token_infos;
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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenInfo>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token_kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (next_state, Some(TokenInfo::new(end - start, token_kind)))
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}
use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    let mut context = self.state.code_editor.context(self.state.view_id);
                    self.code_editor.draw(&mut cx, &mut context);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, &mut self.state.code_editor, self.state.view_id, event)
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{state::ViewId, Affinity, Context, Document, Position, Selection, State},
    makepad_widgets::*,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

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

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
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

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.begin(cx, context);
        let document = context.document();
        self.draw_text(cx, &document);
        self.draw_selections(cx, &document);
        self.end(cx, context);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.context(view_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.context(view_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.context(view_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.context(view_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.fold_line(line, 2 * settings.indent_column_count);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.unfold_line(line);
                    }
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    if alt {
                        context.insert_cursor(cursor);
                    } else {
                        context.set_cursor(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    context.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        context.wrap_lines((self.viewport_rect.size.x / self.cell_size.x) as usize);
        let document = context.document();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        let document = context.document();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if context.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt, token::TokenKind};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                document::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        let mut active_selection = None;
        let mut selections = document.selections();
        while selections
            .first()
            .map_or(false, |selection| selection.end().0.line < self.start_line)
        {
            selections = &selections[1..];
        }
        if selections.first().map_or(false, |selection| {
            selection.start().0.line < self.start_line
        }) {
            let (selection, remaining_selections) = selections.split_first().unwrap();
            selections = remaining_selections;
            active_selection = Some(ActiveSelection::new(*selection, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, document)
    }

    fn pick(&self, document: &Document<'_>, pos: DVec2) -> Option<(Position, Affinity)> {
        use crate::{document, line, str::StrExt};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, byte),
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, next_byte),
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_column_count);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, byte), Affinity::After));
                    }
                    line += 1;
                    y += next_y;
                }
                document::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, 0), Affinity::Before));
                    }
                    y = next_y;
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: &'a [Selection],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_column_count);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                column += 1;
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                document::Element::Line(true, line_ref) => {
                    y += line_ref.height();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Affinity,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let position = Position::new(line, byte);
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == (position, bias)
        }) {
            self.draw_selection(cx, x, y, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self
            .selections
            .first()
            .map_or(false, |selection| selection.start() == (position, bias))
        {
            let (selection, selections) = self.selections.split_first().unwrap();
            self.selections = selections;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection: *selection,
                    start_x: x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

impl ActiveSelection {
    fn new(selection: Selection, start_x: f64) -> Self {
        Self { selection, start_x }
    }
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

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
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
use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    start_column_after_wrap: &'a mut Vec<usize>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        start_column_after_wrap: &'a mut Vec<usize>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.start_column_after_wrap,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = Vec::new();
            mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            let line_ref = document.line(line);
            let mut start_column_after_wrap = line_ref
                .text()
                .indentation()
                .column_count(document.settings().tab_column_count);
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            if start_column_after_wrap
                                + string.column_count(document.settings().tab_column_count)
                                > max_column
                            {
                                start_column_after_wrap = 0;
                            }
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        if start_column_after_wrap + widget.column_count > max_column {
                            start_column_after_wrap = 0;
                        }
                    }
                }
            }
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = start_column_after_wrap;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = start_column_after_wrap;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            self.start_column_after_wrap[line] = start_column_after_wrap;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
        *self.latest_selection_index = 0;
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_column: usize) {
        self.fold_column[line_index] = fold_column;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if !current_selection.should_merge(next_selection) {
                current_selection_index += 1;
                continue;
            }
            let start = current_selection.start().min(next_selection.start());
            let end = current_selection.end().max(next_selection.end());
            let anchor;
            let cursor;
            if current_selection.anchor <= next_selection.cursor {
                anchor = start;
                cursor = end;
            } else {
                anchor = end;
                cursor = start;
            }
            self.selections[current_selection_index] =
                Selection::new(anchor, cursor, current_selection.preferred_column);
            self.selections.remove(next_selection_index);
            if next_selection_index < *self.latest_selection_index {
                *self.latest_selection_index -= 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            let anchor;
            let cursor;
            if selection.anchor <= selection.cursor {
                anchor = (diffed_start, selection.start().1);
                cursor = (diffed_end, selection.end().1);
            } else {
                anchor = (diffed_end, selection.end().1);
                cursor = (diffed_start, selection.start().1);
            }
            *selection = Selection::new(anchor, cursor, selection.preferred_column);
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.start_column_after_wrap.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_column_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_column
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
use {
    crate::{Length, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Diff {
    operations: Vec<Operation>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.operations.iter(),
        }
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut operations_0 = self.operations.into_iter();
        let mut operations_1 = other.operations.into_iter();
        let mut operation_slot_0 = operations_0.next();
        let mut operation_slot_1 = operations_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(length_0)), Some(Operation::Retain(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(length_0)), Some(Operation::Delete(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            let text_length = text.length();
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length - text_length));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(length));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(text.length() - length));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(length);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(len)), None) => {
                    builder.retain(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(len)), op) => {
                    builder.delete(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = op;
                }
                (None, Some(Operation::Retain(len))) => {
                    builder.retain(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, Some(Operation::Delete(len))) => {
                    builder.delete(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, None) => break,
                (op, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = op;
                    operation_slot_1 = operations_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Operation;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Operation;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.operations.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delete(&mut self, length: Length) {
        use std::mem;

        if length == Length::default() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_length)] => {
                *last_length += length;
            }
            [.., Operation::Delete(second_last_length), Operation::Insert(_)] => {
                *second_last_length += length;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(length));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(length)),
        }
    }

    pub fn retain(&mut self, length: Length) {
        if length == Length::default() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_length)) => {
                *last_length += length;
            }
            _ => self.operations.push(Operation::Retain(length)),
        }
    }

    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.operations.push(Operation::Insert(text)),
        }
    }

    pub fn finish(mut self) -> Diff {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Diff {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Operation>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Operation>,
}

impl Iterator for IntoIter {
    type Item = Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    Delete(Length),
    Retain(Length),
    Insert(Text),
}

impl Operation {
    pub fn info(&self) -> OperationInfo {
        match *self {
            Self::Delete(length) => OperationInfo::Delete(length),
            Self::Retain(length) => OperationInfo::Retain(length),
            Self::Insert(ref text) => OperationInfo::Insert(text.length()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationInfo {
    Delete(Length),
    Retain(Length),
    Insert(Length),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Strategy {
    InsertBefore,
    InsertAfter,
}
use {
    crate::{line, token::TokenInfo, Affinity, Line, Selection, Settings, Text, Tokenizer},
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Document<'a> {
    settings: &'a Settings,
    text: &'a Text,
    tokenizer: &'a Tokenizer,
    text_inlays: &'a [Vec<(usize, String)>],
    line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
    wrap_bytes: &'a [Vec<usize>],
    start_column_after_wrap: &'a [usize],
    fold_column: &'a [usize],
    scale: &'a [f64],
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    summed_heights: &'a [f64],
    selections: &'a [Selection],
    latest_selection_index: usize,
}

impl<'a> Document<'a> {
    pub fn new(
        settings: &'a Settings,
        text: &'a Text,
        tokenizer: &'a Tokenizer,
        text_inlays: &'a [Vec<(usize, String)>],
        line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
        wrap_bytes: &'a [Vec<usize>],
        start_column_after_wrap: &'a [usize],
        fold_column: &'a [usize],
        scale: &'a [f64],
        line_inlays: &'a [(usize, LineInlay)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        summed_heights: &'a [f64],
        selections: &'a [Selection],
        latest_selection_index: usize,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
        }
    }

    pub fn settings(&self) -> &'a Settings {
        self.settings
    }

    pub fn compute_width(&self) -> f64 {
        let mut max_width = 0.0f64;
        for element in self.elements(0, self.line_count()) {
            max_width = max_width.max(match element {
                Element::Line(_, line) => line.compute_width(self.settings.tab_column_count),
                Element::Widget(_, widget) => widget.width,
            });
        }
        max_width
    }

    pub fn height(&self) -> f64 {
        self.summed_heights[self.line_count() - 1]
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => {
                if line_index == self.line_count() {
                    line_index
                } else {
                    line_index + 1
                }
            }
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.as_lines().len()
    }

    pub fn line(&self, line: usize) -> Line<'a> {
        Line::new(
            &self.text.as_lines()[line],
            &self.tokenizer.token_infos()[line],
            &self.text_inlays[line],
            &self.line_widget_inlays[line],
            &self.wrap_bytes[line],
            self.start_column_after_wrap[line],
            self.fold_column[line],
            self.scale[line],
        )
    }

    pub fn lines(&self, start_line: usize, end_line: usize) -> Lines<'a> {
        Lines {
            text: self.text.as_lines()[start_line..end_line].iter(),
            token_infos: self.tokenizer.token_infos()[start_line..end_line].iter(),
            text_inlays: self.text_inlays[start_line..end_line].iter(),
            line_widget_inlays: self.line_widget_inlays[start_line..end_line].iter(),
            wrap_bytes: self.wrap_bytes[start_line..end_line].iter(),
            start_column_after_wrap: self.start_column_after_wrap[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
        }
    }

    pub fn line_y(&self, line: usize) -> f64 {
        if line == 0 {
            0.0
        } else {
            self.summed_heights[line - 1]
        }
    }

    pub fn elements(&self, start_line: usize, end_line: usize) -> Elements<'a> {
        Elements {
            lines: self.lines(start_line, end_line),
            line_inlays: &self.line_inlays[self
                .line_inlays
                .iter()
                .position(|(line, _)| *line >= start_line)
                .unwrap_or(self.line_inlays.len())..],
            widget_inlays: &self.widget_inlays[self
                .widget_inlays
                .iter()
                .position(|((line, _), _)| *line >= start_line)
                .unwrap_or(self.widget_inlays.len())..],
            line: start_line,
        }
    }

    pub fn selections(&self) -> &'a [Selection] {
        self.selections
    }

    pub fn latest_selection_index(&self) -> usize {
        self.latest_selection_index
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
    token_infos: slice::Iter<'a, Vec<TokenInfo>>,
    text_inlays: slice::Iter<'a, Vec<(usize, String)>>,
    line_widget_inlays: slice::Iter<'a, Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: slice::Iter<'a, Vec<usize>>,
    start_column_after_wrap: slice::Iter<'a, usize>,
    fold_column: slice::Iter<'a, usize>,
    scale: slice::Iter<'a, f64>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line::new(
            self.text.next()?,
            self.token_infos.next()?,
            self.text_inlays.next()?,
            self.line_widget_inlays.next()?,
            self.wrap_bytes.next()?,
            *self.start_column_after_wrap.next()?,
            *self.fold_column.next()?,
            *self.scale.next()?,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    lines: Lines<'a>,
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    line: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .line_inlays
            .first()
            .map_or(false, |(line, _)| *line == self.line)
        {
            let ((_, line), line_inlays) = self.line_inlays.split_first().unwrap();
            self.line_inlays = line_inlays;
            return Some(Element::Line(true, line.as_line()));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let line = self.lines.next()?;
        self.line += 1;
        Some(Element::Line(false, line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element<'a> {
    Line(bool, Line<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LineInlay {
    text: String,
}

impl LineInlay {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_line(&self) -> Line<'_> {
        Line::new(&self.text, &[], &[], &[], &[], 0, 0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: f64,
    pub height: f64,
}

impl Widget {
    pub fn new(id: usize, width: f64, height: f64) -> Self {
        Self { id, width, height }
    }
}
use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: Range) -> Diff {
    replace(range, "\n".into())
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Position::new(
            position.line,
            text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        );
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Position::new(prev_line, text.as_lines()[prev_line].len());
    }
    position
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Length {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Length {
    pub fn new(line_count: usize, byte_count: usize) -> Self {
        Self {
            line_count,
            byte_count,
        }
    }
}

impl Add for Length {
    type Output = Length;

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

impl AddAssign for Length {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Length {
    type Output = Length;

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

impl SubAssign for Length {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
pub mod bias;
pub mod char;
pub mod code_editor;
pub mod context;
pub mod diff;
pub mod document;
pub mod edit_ops;
pub mod length;
pub mod line;
pub mod move_ops;
pub mod position;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;

pub use crate::{
    bias::Affinity, code_editor::CodeEditor, context::Context, diff::Diff, document::Document,
    length::Length, line::Line, position::Position, range::Range, selection::Selection,
    settings::Settings, state::State, text::Text, token::Token, tokenizer::Tokenizer,
};
use {
    crate::{
        token::{TokenInfo, TokenKind},
        Affinity, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap_bytes: &'a [usize],
    start_column_after_wrap: usize,
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap_bytes: &'a [usize],
        start_column_after_wrap: usize,
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
        }
    }

    pub fn compute_column_count(&self, tab_column_count: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_column_count = 0;
        let mut summed_column_count = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    summed_column_count += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_column_count += widget.column_count;
                }
                WrappedElement::Wrap => {
                    max_summed_column_count = max_summed_column_count.max(summed_column_count);
                    summed_column_count = self.start_column_after_wrap();
                }
            }
        }
        max_summed_column_count.max(summed_column_count)
    }

    pub fn row_count(&self) -> usize {
        self.wrap_bytes.len() + 1
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        self.column_to_x(self.compute_column_count(tab_column_count))
    }

    pub fn height(&self) -> f64 {
        self.scale * self.row_count() as f64
    }

    pub fn byte_bias_to_row_column(
        &self,
        (byte, bias): (usize, Affinity),
        tab_column_count: usize,
    ) -> (usize, usize) {
        use crate::str::StrExt;

        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if byte == current_byte && bias == Affinity::Before {
            return (row, column);
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        if byte == current_byte && bias == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if byte == current_byte && bias == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                WrappedElement::Token(true, token) => {
                    column += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    row += 1;
                    column = self.start_column_after_wrap();
                }
            }
        }
        if byte == current_byte && bias == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_column_to_byte_bias(
        &self,
        (row, column): (usize, usize),
        tab_column_count: usize,
    ) -> (usize, Affinity) {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut current_row = 0;
        let mut current_column = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_column_count);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte = byte + grapheme.len();
                        current_column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = current_column + token.text.column_count(tab_column_count);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    current_column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.start_column_after_wrap();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold_column = column.min(self.fold_column);
        let column_count_after_fold_column = column - column_count_before_fold_column;
        column_count_before_fold_column as f64 + self.scale * column_count_after_fold_column as f64
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn tokens(&self) -> Tokens<'a> {
        Tokens {
            text: self.text,
            token_infos: self.token_infos.iter(),
        }
    }

    pub fn elements(&self) -> Elements<'a> {
        let mut tokens = self.tokens();
        Elements {
            token: tokens.next(),
            tokens,
            text_inlays: self.text_inlays,
            widget_inlays: self.widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap_bytes: self.wrap_bytes,
            byte: 0,
        }
    }

    pub fn start_column_after_wrap(&self) -> usize {
        self.start_column_after_wrap
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }
}

#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    token_infos: slice::Iter<'a, TokenInfo>,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.token_infos.next() {
            Some(token_info) => {
                let (text_0, text_1) = self.text.split_at(token_info.byte_count);
                self.text = text_1;
                Token::new(text_0, token_info.kind)
            }
            None => {
                if self.text.is_empty() {
                    return None;
                }
                let text = self.text;
                self.text = "";
                Token::new(text, TokenKind::Unknown)
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    byte: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.widget_inlays.first() {
            byte_count = byte_count.min(byte - self.byte);
        }
        let token = if byte_count < token.text.len() {
            let (text_0, text_1) = token.text.split_at(byte_count);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
        } else {
            self.token = self.tokens.next();
            token
        };
        self.byte += token.text.len();
        Some(Element::Token(false, token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap_bytes: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Affinity::Before, widget));
        }
        if self
            .wrap_bytes
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.wrap_bytes = &self.wrap_bytes[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.wrap_bytes.first() {
                    byte_count = byte_count.min(*byte - self.byte);
                }
                let token = if byte_count < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(byte_count);
                    self.element = Some(Element::Token(is_inlay, Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.byte += token.text.len();
                WrappedElement::Token(is_inlay, token)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Affinity::After, widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub column_count: usize,
}

impl Widget {
    pub fn new(id: usize, column_count: usize) -> Self {
        Self { id, column_count }
    }
}
mod app;

fn main() {
    app::app_main();
}
use crate::{Affinity, Document, Position};

pub fn move_left(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    ((position, Affinity::Before), None)
}

pub fn move_right(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    ((position, Affinity::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_first_row_of_line(document, (position, bias)) {
        return move_to_prev_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_first_line(position) {
        return move_to_last_row_of_prev_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_last_row_of_line(document, (position, bias)) {
        return move_to_next_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_last_line(document, position) {
        return move_to_first_row_of_next_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Position) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    document
        .line(position.line)
        .byte_bias_to_row_column(
            (position.byte, bias),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    let line = document.line(position.line);
    line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    )
    .0 == line.row_count() - 1
}

fn is_at_first_line(position: Position) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Position) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    (
        (
            Position::new(
                position.line,
                document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            ),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        (
            Position::new(
                position.line,
                line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            ),
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    let prev_line = position.line - 1;
    (
        (
            Position::new(prev_line, document.line(prev_line).text().len()),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Position) -> ((Position, Affinity), Option<usize>) {
    (
        (Position::new(position.line + 1, 0), Affinity::Before),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row - 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row + 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = position.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, bias) = prev_line_ref.row_column_to_byte_bias(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    ((Position::new(prev_line, byte), bias), Some(column))
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = position.line + 1;
    let (byte, bias) = document
        .line(next_line)
        .row_column_to_byte_bias((0, column), document.settings().tab_column_count);
    ((Position::new(next_line, byte), bias), Some(column))
}
use {
    crate::{diff::Strategy, Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }

    pub fn apply_diff(self, diff: &Diff, strategy: Strategy) -> Position {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_position = Position::default();
        let mut distance_to_position = self - Position::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_position += length;
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_position + distance_to_position;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_position == Length::default() {
                        break match strategy {
                            Strategy::InsertBefore => diffed_position + length,
                            Strategy::InsertAfter => diffed_position,
                        };
                    } else {
                        diffed_position += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_position = Length::default();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_position + distance_to_position;
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.byte_count,
            }
        } else {
            Self {
                line: self.line + length.line_count,
                byte: length.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Length {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Length {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }
}
use crate::{Affinity, Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Position, Affinity),
        cursor: (Position, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(mut self, mut other: Self) -> bool {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Position, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Position, Affinity) {
        self.anchor.max(self.cursor)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce((Position, Affinity), Option<usize>) -> ((Position, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub indent_column_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            indent_column_count: 4,
        }
    }
}
use {
    crate::{
        document, document::LineInlay, line, Affinity, Context, Document, Selection, Settings,
        Text, Tokenizer,
    },
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    view_id: usize,
    views: HashMap<ViewId, View>,
    editor_id: usize,
    editors: HashMap<EditorId, Editor>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn document(&self, view_id: ViewId) -> Document<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        Document::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.start_column_after_wrap,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
            view.latest_selection_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut self.settings,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.as_lines().len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
                start_column_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
                latest_selection_index: 0,
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.context(view_id).update_summed_heights();
        Ok(view_id)
    }

    fn open_editor(&mut self, path: impl AsRef<Path>) -> io::Result<EditorId> {
        use std::fs;

        let editor_id = EditorId(self.editor_id);
        self.editor_id += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                tokenizer,
                text_inlays: (0..line_count)
                    .map(|line| {
                        if line % 2 == 0 {
                            [
                                (20, "###".into()),
                                (40, "###".into()),
                                (60, "###".into()),
                                (80, "###".into()),
                            ]
                            .into()
                        } else {
                            [].into()
                        }
                    })
                    .collect(),
                line_inlays: [
                    (
                        10,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        20,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        30,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        40,
                        LineInlay::new("##################################################".into()),
                    ),
                ]
                .into(),
                line_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                document_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct View {
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Affinity), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Affinity), document::Widget)>,
}
pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize;
    fn indentation(&self) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        use crate::char::CharExt;

        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize {
        self.indentation().column_count(tab_column_count) / indent_column_count
    }

    fn indentation(&self) -> &str {
        &self[..self
            .char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(self.len())]
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
        let mut prev_grapheme_is_whitespace = None;
        let index = self
            .string
            .grapheme_indices()
            .find_map(|(index, next_grapheme)| {
                let next_grapheme_is_whitespace =
                    next_grapheme.chars().all(|char| char.is_whitespace());
                let is_whitespace_boundary =
                    prev_grapheme_is_whitespace.map_or(false, |prev_grapheme_is_whitespace| {
                        prev_grapheme_is_whitespace != next_grapheme_is_whitespace
                    });
                prev_grapheme_is_whitespace = Some(next_grapheme_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(string)
    }
}
use {
    crate::{Diff, Length, Position, Range},
    std::{borrow::Cow, ops::AddAssign},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.length() == Length::default()
    }

    pub fn length(&self) -> Length {
        Length {
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

    pub fn take(&mut self, len: Length) -> Self {
        let mut lines = self
            .lines
            .drain(..len.line_count as usize)
            .collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte_count].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Length) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, position: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line][..position.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line][position.byte..]);
            self.lines
                .splice(position.line..position.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte + length.byte_count, "");
        } else {
            let mut line = self.lines[position.line][..position.byte].to_string();
            line.push_str(&self.lines[position.line + length.line_count][length.byte_count..]);
            self.lines.splice(
                position.line..position.line + length.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Operation;

        let mut position = Position::default();
        for operation in diff {
            match operation {
                Operation::Delete(length) => self.delete(position, length),
                Operation::Retain(length) => position += length,
                Operation::Insert(text) => {
                    let length = text.length();
                    self.insert(position, text);
                    position += length;
                }
            }
        }
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, mut other: Self) {
        other
            .lines
            .first_mut()
            .unwrap()
            .replace_range(..0, self.lines.last().unwrap());
        self.lines
            .splice(self.lines.len() - 1..self.lines.len(), other.lines);
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<char> for Text {
    fn from(char: char) -> Self {
        Self {
            lines: match char {
                '\n' | '\r' => vec![String::new(), String::new()],
                _ => vec![char.into()],
            },
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.split('\n').map(|line| line.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
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

impl From<Cow<'_, str>> for Text {
    fn from(string: Cow<'_, str>) -> Self {
        string.as_ref().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    pub fn new(text: &'a str, kind: TokenKind) -> Self {
        Self { text, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub byte_count: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self {
            byte_count: len,
            kind,
        }
    }
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
use crate::{
    token::{TokenInfo, TokenKind},
    Diff, Text,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
    token_infos: Vec<Vec<TokenInfo>>,
}

impl Tokenizer {
    pub fn new(text: &Text) -> Self {
        let line_count = text.as_lines().len();
        let mut tokenizer = Self {
            state: (0..line_count).map(|_| None).collect(),
            token_infos: (0..line_count).map(|_| Vec::new()).collect(),
        };
        tokenizer.retokenize(&Diff::new(), text);
        tokenizer
    }

    pub fn token_infos(&self) -> &[Vec<TokenInfo>] {
        &self.token_infos
    }

    pub fn retokenize(&mut self, diff: &Diff, text: &Text) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    self.state.drain(line..line + length.line_count);
                    self.token_infos.drain(line..line + length.line_count);
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                    self.state
                        .splice(line..line, (0..length.line_count).map(|_| None));
                    self.token_infos
                        .splice(line..line, (0..length.line_count).map(|_| Vec::new()));
                    line += length.line_count;
                }
            }
        }
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut token_infos = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => token_infos.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    self.token_infos[line] = token_infos;
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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenInfo>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token_kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (next_state, Some(TokenInfo::new(end - start, token_kind)))
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}
use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    let mut context = self.state.code_editor.context(self.state.view_id);
                    self.code_editor.draw(&mut cx, &mut context);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, &mut self.state.code_editor, self.state.view_id, event)
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{state::ViewId, Affinity, Context, Document, Position, Selection, State},
    makepad_widgets::*,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

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

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
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

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.begin(cx, context);
        let document = context.document();
        self.draw_text(cx, &document);
        self.draw_selections(cx, &document);
        self.end(cx, context);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.context(view_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.context(view_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.context(view_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.context(view_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.fold_line(line, 2 * settings.indent_column_count);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.unfold_line(line);
                    }
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    if alt {
                        context.insert_cursor(cursor);
                    } else {
                        context.set_cursor(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    context.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        context.wrap_lines((self.viewport_rect.size.x / self.cell_size.x) as usize);
        let document = context.document();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        let document = context.document();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if context.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt, token::TokenKind};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                document::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        let mut active_selection = None;
        let mut selections = document.selections();
        while selections
            .first()
            .map_or(false, |selection| selection.end().0.line < self.start_line)
        {
            selections = &selections[1..];
        }
        if selections.first().map_or(false, |selection| {
            selection.start().0.line < self.start_line
        }) {
            let (selection, remaining_selections) = selections.split_first().unwrap();
            selections = remaining_selections;
            active_selection = Some(ActiveSelection::new(*selection, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, document)
    }

    fn pick(&self, document: &Document<'_>, pos: DVec2) -> Option<(Position, Affinity)> {
        use crate::{document, line, str::StrExt};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, byte),
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, next_byte),
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_column_count);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, byte), Affinity::After));
                    }
                    line += 1;
                    y += next_y;
                }
                document::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, 0), Affinity::Before));
                    }
                    y = next_y;
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: &'a [Selection],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_column_count);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                column += 1;
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                document::Element::Line(true, line_ref) => {
                    y += line_ref.height();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Affinity,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let position = Position::new(line, byte);
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == (position, bias)
        }) {
            self.draw_selection(cx, x, y, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self
            .selections
            .first()
            .map_or(false, |selection| selection.start() == (position, bias))
        {
            let (selection, selections) = self.selections.split_first().unwrap();
            self.selections = selections;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection: *selection,
                    start_x: x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

impl ActiveSelection {
    fn new(selection: Selection, start_x: f64) -> Self {
        Self { selection, start_x }
    }
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

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
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
use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    start_column_after_wrap: &'a mut Vec<usize>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        start_column_after_wrap: &'a mut Vec<usize>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.start_column_after_wrap,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = Vec::new();
            mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            let line_ref = document.line(line);
            let mut start_column_after_wrap = line_ref
                .text()
                .indentation()
                .column_count(document.settings().tab_column_count);
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            if start_column_after_wrap
                                + string.column_count(document.settings().tab_column_count)
                                > max_column
                            {
                                start_column_after_wrap = 0;
                            }
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        if start_column_after_wrap + widget.column_count > max_column {
                            start_column_after_wrap = 0;
                        }
                    }
                }
            }
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = start_column_after_wrap;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = start_column_after_wrap;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            self.start_column_after_wrap[line] = start_column_after_wrap;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
        *self.latest_selection_index = 0;
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_column: usize) {
        self.fold_column[line_index] = fold_column;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if !current_selection.should_merge(next_selection) {
                current_selection_index += 1;
                continue;
            }
            let start = current_selection.start().min(next_selection.start());
            let end = current_selection.end().max(next_selection.end());
            let anchor;
            let cursor;
            if current_selection.anchor <= next_selection.cursor {
                anchor = start;
                cursor = end;
            } else {
                anchor = end;
                cursor = start;
            }
            self.selections[current_selection_index] =
                Selection::new(anchor, cursor, current_selection.preferred_column);
            self.selections.remove(next_selection_index);
            if next_selection_index < *self.latest_selection_index {
                *self.latest_selection_index -= 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            let anchor;
            let cursor;
            if selection.anchor <= selection.cursor {
                anchor = (diffed_start, selection.start().1);
                cursor = (diffed_end, selection.end().1);
            } else {
                anchor = (diffed_end, selection.end().1);
                cursor = (diffed_start, selection.start().1);
            }
            *selection = Selection::new(anchor, cursor, selection.preferred_column);
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.start_column_after_wrap.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_column_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_column
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
use {
    crate::{Length, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Diff {
    operations: Vec<Operation>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.operations.iter(),
        }
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut operations_0 = self.operations.into_iter();
        let mut operations_1 = other.operations.into_iter();
        let mut operation_slot_0 = operations_0.next();
        let mut operation_slot_1 = operations_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(length_0)), Some(Operation::Retain(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(length_0)), Some(Operation::Delete(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            let text_length = text.length();
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length - text_length));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(length));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(text.length() - length));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(length);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(len)), None) => {
                    builder.retain(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(len)), op) => {
                    builder.delete(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = op;
                }
                (None, Some(Operation::Retain(len))) => {
                    builder.retain(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, Some(Operation::Delete(len))) => {
                    builder.delete(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, None) => break,
                (op, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = op;
                    operation_slot_1 = operations_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Operation;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Operation;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.operations.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delete(&mut self, length: Length) {
        use std::mem;

        if length == Length::default() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_length)] => {
                *last_length += length;
            }
            [.., Operation::Delete(second_last_length), Operation::Insert(_)] => {
                *second_last_length += length;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(length));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(length)),
        }
    }

    pub fn retain(&mut self, length: Length) {
        if length == Length::default() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_length)) => {
                *last_length += length;
            }
            _ => self.operations.push(Operation::Retain(length)),
        }
    }

    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.operations.push(Operation::Insert(text)),
        }
    }

    pub fn finish(mut self) -> Diff {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Diff {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Operation>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Operation>,
}

impl Iterator for IntoIter {
    type Item = Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    Delete(Length),
    Retain(Length),
    Insert(Text),
}

impl Operation {
    pub fn info(&self) -> OperationInfo {
        match *self {
            Self::Delete(length) => OperationInfo::Delete(length),
            Self::Retain(length) => OperationInfo::Retain(length),
            Self::Insert(ref text) => OperationInfo::Insert(text.length()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationInfo {
    Delete(Length),
    Retain(Length),
    Insert(Length),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Strategy {
    InsertBefore,
    InsertAfter,
}
use {
    crate::{line, token::TokenInfo, Affinity, Line, Selection, Settings, Text, Tokenizer},
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Document<'a> {
    settings: &'a Settings,
    text: &'a Text,
    tokenizer: &'a Tokenizer,
    text_inlays: &'a [Vec<(usize, String)>],
    line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
    wrap_bytes: &'a [Vec<usize>],
    start_column_after_wrap: &'a [usize],
    fold_column: &'a [usize],
    scale: &'a [f64],
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    summed_heights: &'a [f64],
    selections: &'a [Selection],
    latest_selection_index: usize,
}

impl<'a> Document<'a> {
    pub fn new(
        settings: &'a Settings,
        text: &'a Text,
        tokenizer: &'a Tokenizer,
        text_inlays: &'a [Vec<(usize, String)>],
        line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
        wrap_bytes: &'a [Vec<usize>],
        start_column_after_wrap: &'a [usize],
        fold_column: &'a [usize],
        scale: &'a [f64],
        line_inlays: &'a [(usize, LineInlay)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        summed_heights: &'a [f64],
        selections: &'a [Selection],
        latest_selection_index: usize,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
        }
    }

    pub fn settings(&self) -> &'a Settings {
        self.settings
    }

    pub fn compute_width(&self) -> f64 {
        let mut max_width = 0.0f64;
        for element in self.elements(0, self.line_count()) {
            max_width = max_width.max(match element {
                Element::Line(_, line) => line.compute_width(self.settings.tab_column_count),
                Element::Widget(_, widget) => widget.width,
            });
        }
        max_width
    }

    pub fn height(&self) -> f64 {
        self.summed_heights[self.line_count() - 1]
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => {
                if line_index == self.line_count() {
                    line_index
                } else {
                    line_index + 1
                }
            }
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.as_lines().len()
    }

    pub fn line(&self, line: usize) -> Line<'a> {
        Line::new(
            &self.text.as_lines()[line],
            &self.tokenizer.token_infos()[line],
            &self.text_inlays[line],
            &self.line_widget_inlays[line],
            &self.wrap_bytes[line],
            self.start_column_after_wrap[line],
            self.fold_column[line],
            self.scale[line],
        )
    }

    pub fn lines(&self, start_line: usize, end_line: usize) -> Lines<'a> {
        Lines {
            text: self.text.as_lines()[start_line..end_line].iter(),
            token_infos: self.tokenizer.token_infos()[start_line..end_line].iter(),
            text_inlays: self.text_inlays[start_line..end_line].iter(),
            line_widget_inlays: self.line_widget_inlays[start_line..end_line].iter(),
            wrap_bytes: self.wrap_bytes[start_line..end_line].iter(),
            start_column_after_wrap: self.start_column_after_wrap[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
        }
    }

    pub fn line_y(&self, line: usize) -> f64 {
        if line == 0 {
            0.0
        } else {
            self.summed_heights[line - 1]
        }
    }

    pub fn elements(&self, start_line: usize, end_line: usize) -> Elements<'a> {
        Elements {
            lines: self.lines(start_line, end_line),
            line_inlays: &self.line_inlays[self
                .line_inlays
                .iter()
                .position(|(line, _)| *line >= start_line)
                .unwrap_or(self.line_inlays.len())..],
            widget_inlays: &self.widget_inlays[self
                .widget_inlays
                .iter()
                .position(|((line, _), _)| *line >= start_line)
                .unwrap_or(self.widget_inlays.len())..],
            line: start_line,
        }
    }

    pub fn selections(&self) -> &'a [Selection] {
        self.selections
    }

    pub fn latest_selection_index(&self) -> usize {
        self.latest_selection_index
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
    token_infos: slice::Iter<'a, Vec<TokenInfo>>,
    text_inlays: slice::Iter<'a, Vec<(usize, String)>>,
    line_widget_inlays: slice::Iter<'a, Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: slice::Iter<'a, Vec<usize>>,
    start_column_after_wrap: slice::Iter<'a, usize>,
    fold_column: slice::Iter<'a, usize>,
    scale: slice::Iter<'a, f64>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line::new(
            self.text.next()?,
            self.token_infos.next()?,
            self.text_inlays.next()?,
            self.line_widget_inlays.next()?,
            self.wrap_bytes.next()?,
            *self.start_column_after_wrap.next()?,
            *self.fold_column.next()?,
            *self.scale.next()?,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    lines: Lines<'a>,
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    line: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .line_inlays
            .first()
            .map_or(false, |(line, _)| *line == self.line)
        {
            let ((_, line), line_inlays) = self.line_inlays.split_first().unwrap();
            self.line_inlays = line_inlays;
            return Some(Element::Line(true, line.as_line()));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let line = self.lines.next()?;
        self.line += 1;
        Some(Element::Line(false, line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element<'a> {
    Line(bool, Line<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LineInlay {
    text: String,
}

impl LineInlay {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_line(&self) -> Line<'_> {
        Line::new(&self.text, &[], &[], &[], &[], 0, 0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: f64,
    pub height: f64,
}

impl Widget {
    pub fn new(id: usize, width: f64, height: f64) -> Self {
        Self { id, width, height }
    }
}
use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: Range) -> Diff {
    replace(range, "\n".into())
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Position::new(
            position.line,
            text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        );
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Position::new(prev_line, text.as_lines()[prev_line].len());
    }
    position
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Length {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Length {
    pub fn new(line_count: usize, byte_count: usize) -> Self {
        Self {
            line_count,
            byte_count,
        }
    }
}

impl Add for Length {
    type Output = Length;

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

impl AddAssign for Length {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Length {
    type Output = Length;

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

impl SubAssign for Length {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
pub mod bias;
pub mod char;
pub mod code_editor;
pub mod context;
pub mod diff;
pub mod document;
pub mod edit_ops;
pub mod length;
pub mod line;
pub mod move_ops;
pub mod position;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;

pub use crate::{
    bias::Affinity, code_editor::CodeEditor, context::Context, diff::Diff, document::Document,
    length::Length, line::Line, position::Position, range::Range, selection::Selection,
    settings::Settings, state::State, text::Text, token::Token, tokenizer::Tokenizer,
};
use {
    crate::{
        token::{TokenInfo, TokenKind},
        Affinity, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap_bytes: &'a [usize],
    start_column_after_wrap: usize,
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap_bytes: &'a [usize],
        start_column_after_wrap: usize,
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
        }
    }

    pub fn compute_column_count(&self, tab_column_count: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_column_count = 0;
        let mut summed_column_count = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    summed_column_count += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_column_count += widget.column_count;
                }
                WrappedElement::Wrap => {
                    max_summed_column_count = max_summed_column_count.max(summed_column_count);
                    summed_column_count = self.start_column_after_wrap();
                }
            }
        }
        max_summed_column_count.max(summed_column_count)
    }

    pub fn row_count(&self) -> usize {
        self.wrap_bytes.len() + 1
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        self.column_to_x(self.compute_column_count(tab_column_count))
    }

    pub fn height(&self) -> f64 {
        self.scale * self.row_count() as f64
    }

    pub fn byte_bias_to_row_column(
        &self,
        (byte, bias): (usize, Affinity),
        tab_column_count: usize,
    ) -> (usize, usize) {
        use crate::str::StrExt;

        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if byte == current_byte && bias == Affinity::Before {
            return (row, column);
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        if byte == current_byte && bias == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if byte == current_byte && bias == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                WrappedElement::Token(true, token) => {
                    column += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    row += 1;
                    column = self.start_column_after_wrap();
                }
            }
        }
        if byte == current_byte && bias == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_column_to_byte_bias(
        &self,
        (row, column): (usize, usize),
        tab_column_count: usize,
    ) -> (usize, Affinity) {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut current_row = 0;
        let mut current_column = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_column_count);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte = byte + grapheme.len();
                        current_column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = current_column + token.text.column_count(tab_column_count);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    current_column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.start_column_after_wrap();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold_column = column.min(self.fold_column);
        let column_count_after_fold_column = column - column_count_before_fold_column;
        column_count_before_fold_column as f64 + self.scale * column_count_after_fold_column as f64
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn tokens(&self) -> Tokens<'a> {
        Tokens {
            text: self.text,
            token_infos: self.token_infos.iter(),
        }
    }

    pub fn elements(&self) -> Elements<'a> {
        let mut tokens = self.tokens();
        Elements {
            token: tokens.next(),
            tokens,
            text_inlays: self.text_inlays,
            widget_inlays: self.widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap_bytes: self.wrap_bytes,
            byte: 0,
        }
    }

    pub fn start_column_after_wrap(&self) -> usize {
        self.start_column_after_wrap
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }
}

#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    token_infos: slice::Iter<'a, TokenInfo>,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.token_infos.next() {
            Some(token_info) => {
                let (text_0, text_1) = self.text.split_at(token_info.byte_count);
                self.text = text_1;
                Token::new(text_0, token_info.kind)
            }
            None => {
                if self.text.is_empty() {
                    return None;
                }
                let text = self.text;
                self.text = "";
                Token::new(text, TokenKind::Unknown)
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    byte: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.widget_inlays.first() {
            byte_count = byte_count.min(byte - self.byte);
        }
        let token = if byte_count < token.text.len() {
            let (text_0, text_1) = token.text.split_at(byte_count);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
        } else {
            self.token = self.tokens.next();
            token
        };
        self.byte += token.text.len();
        Some(Element::Token(false, token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap_bytes: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Affinity::Before, widget));
        }
        if self
            .wrap_bytes
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.wrap_bytes = &self.wrap_bytes[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.wrap_bytes.first() {
                    byte_count = byte_count.min(*byte - self.byte);
                }
                let token = if byte_count < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(byte_count);
                    self.element = Some(Element::Token(is_inlay, Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.byte += token.text.len();
                WrappedElement::Token(is_inlay, token)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Affinity::After, widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub column_count: usize,
}

impl Widget {
    pub fn new(id: usize, column_count: usize) -> Self {
        Self { id, column_count }
    }
}
mod app;

fn main() {
    app::app_main();
}
use crate::{Affinity, Document, Position};

pub fn move_left(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    ((position, Affinity::Before), None)
}

pub fn move_right(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    ((position, Affinity::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_first_row_of_line(document, (position, bias)) {
        return move_to_prev_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_first_line(position) {
        return move_to_last_row_of_prev_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_last_row_of_line(document, (position, bias)) {
        return move_to_next_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_last_line(document, position) {
        return move_to_first_row_of_next_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Position) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    document
        .line(position.line)
        .byte_bias_to_row_column(
            (position.byte, bias),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    let line = document.line(position.line);
    line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    )
    .0 == line.row_count() - 1
}

fn is_at_first_line(position: Position) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Position) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    (
        (
            Position::new(
                position.line,
                document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            ),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        (
            Position::new(
                position.line,
                line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            ),
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    let prev_line = position.line - 1;
    (
        (
            Position::new(prev_line, document.line(prev_line).text().len()),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Position) -> ((Position, Affinity), Option<usize>) {
    (
        (Position::new(position.line + 1, 0), Affinity::Before),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row - 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row + 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = position.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, bias) = prev_line_ref.row_column_to_byte_bias(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    ((Position::new(prev_line, byte), bias), Some(column))
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = position.line + 1;
    let (byte, bias) = document
        .line(next_line)
        .row_column_to_byte_bias((0, column), document.settings().tab_column_count);
    ((Position::new(next_line, byte), bias), Some(column))
}
use {
    crate::{diff::Strategy, Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }

    pub fn apply_diff(self, diff: &Diff, strategy: Strategy) -> Position {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_position = Position::default();
        let mut distance_to_position = self - Position::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_position += length;
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_position + distance_to_position;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_position == Length::default() {
                        break match strategy {
                            Strategy::InsertBefore => diffed_position + length,
                            Strategy::InsertAfter => diffed_position,
                        };
                    } else {
                        diffed_position += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_position = Length::default();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_position + distance_to_position;
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.byte_count,
            }
        } else {
            Self {
                line: self.line + length.line_count,
                byte: length.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Length {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Length {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }
}
use crate::{Affinity, Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Position, Affinity),
        cursor: (Position, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(mut self, mut other: Self) -> bool {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Position, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Position, Affinity) {
        self.anchor.max(self.cursor)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce((Position, Affinity), Option<usize>) -> ((Position, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub indent_column_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            indent_column_count: 4,
        }
    }
}
use {
    crate::{
        document, document::LineInlay, line, Affinity, Context, Document, Selection, Settings,
        Text, Tokenizer,
    },
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    view_id: usize,
    views: HashMap<ViewId, View>,
    editor_id: usize,
    editors: HashMap<EditorId, Editor>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn document(&self, view_id: ViewId) -> Document<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        Document::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.start_column_after_wrap,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
            view.latest_selection_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut self.settings,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.as_lines().len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
                start_column_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
                latest_selection_index: 0,
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.context(view_id).update_summed_heights();
        Ok(view_id)
    }

    fn open_editor(&mut self, path: impl AsRef<Path>) -> io::Result<EditorId> {
        use std::fs;

        let editor_id = EditorId(self.editor_id);
        self.editor_id += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                tokenizer,
                text_inlays: (0..line_count)
                    .map(|line| {
                        if line % 2 == 0 {
                            [
                                (20, "###".into()),
                                (40, "###".into()),
                                (60, "###".into()),
                                (80, "###".into()),
                            ]
                            .into()
                        } else {
                            [].into()
                        }
                    })
                    .collect(),
                line_inlays: [
                    (
                        10,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        20,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        30,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        40,
                        LineInlay::new("##################################################".into()),
                    ),
                ]
                .into(),
                line_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                document_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct View {
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Affinity), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Affinity), document::Widget)>,
}
pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize;
    fn indentation(&self) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        use crate::char::CharExt;

        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize {
        self.indentation().column_count(tab_column_count) / indent_column_count
    }

    fn indentation(&self) -> &str {
        &self[..self
            .char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(self.len())]
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
        let mut prev_grapheme_is_whitespace = None;
        let index = self
            .string
            .grapheme_indices()
            .find_map(|(index, next_grapheme)| {
                let next_grapheme_is_whitespace =
                    next_grapheme.chars().all(|char| char.is_whitespace());
                let is_whitespace_boundary =
                    prev_grapheme_is_whitespace.map_or(false, |prev_grapheme_is_whitespace| {
                        prev_grapheme_is_whitespace != next_grapheme_is_whitespace
                    });
                prev_grapheme_is_whitespace = Some(next_grapheme_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(string)
    }
}
use {
    crate::{Diff, Length, Position, Range},
    std::{borrow::Cow, ops::AddAssign},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.length() == Length::default()
    }

    pub fn length(&self) -> Length {
        Length {
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

    pub fn take(&mut self, len: Length) -> Self {
        let mut lines = self
            .lines
            .drain(..len.line_count as usize)
            .collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte_count].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Length) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, position: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line][..position.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line][position.byte..]);
            self.lines
                .splice(position.line..position.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte + length.byte_count, "");
        } else {
            let mut line = self.lines[position.line][..position.byte].to_string();
            line.push_str(&self.lines[position.line + length.line_count][length.byte_count..]);
            self.lines.splice(
                position.line..position.line + length.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Operation;

        let mut position = Position::default();
        for operation in diff {
            match operation {
                Operation::Delete(length) => self.delete(position, length),
                Operation::Retain(length) => position += length,
                Operation::Insert(text) => {
                    let length = text.length();
                    self.insert(position, text);
                    position += length;
                }
            }
        }
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, mut other: Self) {
        other
            .lines
            .first_mut()
            .unwrap()
            .replace_range(..0, self.lines.last().unwrap());
        self.lines
            .splice(self.lines.len() - 1..self.lines.len(), other.lines);
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<char> for Text {
    fn from(char: char) -> Self {
        Self {
            lines: match char {
                '\n' | '\r' => vec![String::new(), String::new()],
                _ => vec![char.into()],
            },
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.split('\n').map(|line| line.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
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

impl From<Cow<'_, str>> for Text {
    fn from(string: Cow<'_, str>) -> Self {
        string.as_ref().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    pub fn new(text: &'a str, kind: TokenKind) -> Self {
        Self { text, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub byte_count: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self {
            byte_count: len,
            kind,
        }
    }
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
use crate::{
    token::{TokenInfo, TokenKind},
    Diff, Text,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
    token_infos: Vec<Vec<TokenInfo>>,
}

impl Tokenizer {
    pub fn new(text: &Text) -> Self {
        let line_count = text.as_lines().len();
        let mut tokenizer = Self {
            state: (0..line_count).map(|_| None).collect(),
            token_infos: (0..line_count).map(|_| Vec::new()).collect(),
        };
        tokenizer.retokenize(&Diff::new(), text);
        tokenizer
    }

    pub fn token_infos(&self) -> &[Vec<TokenInfo>] {
        &self.token_infos
    }

    pub fn retokenize(&mut self, diff: &Diff, text: &Text) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    self.state.drain(line..line + length.line_count);
                    self.token_infos.drain(line..line + length.line_count);
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                    self.state
                        .splice(line..line, (0..length.line_count).map(|_| None));
                    self.token_infos
                        .splice(line..line, (0..length.line_count).map(|_| Vec::new()));
                    line += length.line_count;
                }
            }
        }
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut token_infos = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => token_infos.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    self.token_infos[line] = token_infos;
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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenInfo>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token_kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (next_state, Some(TokenInfo::new(end - start, token_kind)))
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}
use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    let mut context = self.state.code_editor.context(self.state.view_id);
                    self.code_editor.draw(&mut cx, &mut context);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, &mut self.state.code_editor, self.state.view_id, event)
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{state::ViewId, Affinity, Context, Document, Position, Selection, State},
    makepad_widgets::*,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

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

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
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

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.begin(cx, context);
        let document = context.document();
        self.draw_text(cx, &document);
        self.draw_selections(cx, &document);
        self.end(cx, context);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.context(view_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.context(view_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.context(view_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.context(view_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.fold_line(line, 2 * settings.indent_column_count);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.unfold_line(line);
                    }
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    if alt {
                        context.insert_cursor(cursor);
                    } else {
                        context.set_cursor(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    context.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        context.wrap_lines((self.viewport_rect.size.x / self.cell_size.x) as usize);
        let document = context.document();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        let document = context.document();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if context.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt, token::TokenKind};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                document::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        let mut active_selection = None;
        let mut selections = document.selections();
        while selections
            .first()
            .map_or(false, |selection| selection.end().0.line < self.start_line)
        {
            selections = &selections[1..];
        }
        if selections.first().map_or(false, |selection| {
            selection.start().0.line < self.start_line
        }) {
            let (selection, remaining_selections) = selections.split_first().unwrap();
            selections = remaining_selections;
            active_selection = Some(ActiveSelection::new(*selection, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, document)
    }

    fn pick(&self, document: &Document<'_>, pos: DVec2) -> Option<(Position, Affinity)> {
        use crate::{document, line, str::StrExt};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, byte),
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, next_byte),
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_column_count);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, byte), Affinity::After));
                    }
                    line += 1;
                    y += next_y;
                }
                document::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, 0), Affinity::Before));
                    }
                    y = next_y;
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: &'a [Selection],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_column_count);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                column += 1;
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                document::Element::Line(true, line_ref) => {
                    y += line_ref.height();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Affinity,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let position = Position::new(line, byte);
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == (position, bias)
        }) {
            self.draw_selection(cx, x, y, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self
            .selections
            .first()
            .map_or(false, |selection| selection.start() == (position, bias))
        {
            let (selection, selections) = self.selections.split_first().unwrap();
            self.selections = selections;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection: *selection,
                    start_x: x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

impl ActiveSelection {
    fn new(selection: Selection, start_x: f64) -> Self {
        Self { selection, start_x }
    }
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

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
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
use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    start_column_after_wrap: &'a mut Vec<usize>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        start_column_after_wrap: &'a mut Vec<usize>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.start_column_after_wrap,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = Vec::new();
            mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            let line_ref = document.line(line);
            let mut start_column_after_wrap = line_ref
                .text()
                .indentation()
                .column_count(document.settings().tab_column_count);
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            if start_column_after_wrap
                                + string.column_count(document.settings().tab_column_count)
                                > max_column
                            {
                                start_column_after_wrap = 0;
                            }
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        if start_column_after_wrap + widget.column_count > max_column {
                            start_column_after_wrap = 0;
                        }
                    }
                }
            }
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = start_column_after_wrap;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = start_column_after_wrap;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            self.start_column_after_wrap[line] = start_column_after_wrap;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
        *self.latest_selection_index = 0;
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_column: usize) {
        self.fold_column[line_index] = fold_column;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if !current_selection.should_merge(next_selection) {
                current_selection_index += 1;
                continue;
            }
            let start = current_selection.start().min(next_selection.start());
            let end = current_selection.end().max(next_selection.end());
            let anchor;
            let cursor;
            if current_selection.anchor <= next_selection.cursor {
                anchor = start;
                cursor = end;
            } else {
                anchor = end;
                cursor = start;
            }
            self.selections[current_selection_index] =
                Selection::new(anchor, cursor, current_selection.preferred_column);
            self.selections.remove(next_selection_index);
            if next_selection_index < *self.latest_selection_index {
                *self.latest_selection_index -= 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            let anchor;
            let cursor;
            if selection.anchor <= selection.cursor {
                anchor = (diffed_start, selection.start().1);
                cursor = (diffed_end, selection.end().1);
            } else {
                anchor = (diffed_end, selection.end().1);
                cursor = (diffed_start, selection.start().1);
            }
            *selection = Selection::new(anchor, cursor, selection.preferred_column);
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.start_column_after_wrap.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_column_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_column
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
use {
    crate::{Length, Text},
    std::{slice, vec},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Diff {
    operations: Vec<Operation>,
}

impl Diff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.operations.iter(),
        }
    }

    pub fn compose(self, other: Self) -> Self {
        use std::cmp::Ordering;

        let mut builder = Builder::new();
        let mut operations_0 = self.operations.into_iter();
        let mut operations_1 = other.operations.into_iter();
        let mut operation_slot_0 = operations_0.next();
        let mut operation_slot_1 = operations_1.next();
        loop {
            match (operation_slot_0, operation_slot_1) {
                (Some(Operation::Retain(length_0)), Some(Operation::Retain(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.retain(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.retain(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Retain(length_0)), Some(Operation::Delete(length_1))) => {
                    match length_0.cmp(&length_1) {
                        Ordering::Less => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(length_1 - length_0));
                        }
                        Ordering::Equal => {
                            builder.delete(length_0);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.delete(length_1);
                            operation_slot_0 = Some(Operation::Retain(length_0 - length_1));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Retain(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            let text_length = text.length();
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Retain(length - text_length));
                        }
                        Ordering::Equal => {
                            builder.insert(text);
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            builder.insert(text.take(length));
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(mut text)), Some(Operation::Delete(length))) => {
                    match text.length().cmp(&length) {
                        Ordering::Less => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = Some(Operation::Delete(text.length() - length));
                        }
                        Ordering::Equal => {
                            operation_slot_0 = operations_0.next();
                            operation_slot_1 = operations_1.next();
                        }
                        Ordering::Greater => {
                            text.skip(length);
                            operation_slot_0 = Some(Operation::Insert(text));
                            operation_slot_1 = operations_1.next();
                        }
                    }
                }
                (Some(Operation::Insert(text)), None) => {
                    builder.insert(text);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Retain(len)), None) => {
                    builder.retain(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = None;
                }
                (Some(Operation::Delete(len)), op) => {
                    builder.delete(len);
                    operation_slot_0 = operations_0.next();
                    operation_slot_1 = op;
                }
                (None, Some(Operation::Retain(len))) => {
                    builder.retain(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, Some(Operation::Delete(len))) => {
                    builder.delete(len);
                    operation_slot_0 = None;
                    operation_slot_1 = operations_1.next();
                }
                (None, None) => break,
                (op, Some(Operation::Insert(text))) => {
                    builder.insert(text);
                    operation_slot_0 = op;
                    operation_slot_1 = operations_1.next();
                }
            }
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a Diff {
    type Item = &'a Operation;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Diff {
    type Item = Operation;
    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.operations.into_iter(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Builder {
    operations: Vec<Operation>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delete(&mut self, length: Length) {
        use std::mem;

        if length == Length::default() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Delete(last_length)] => {
                *last_length += length;
            }
            [.., Operation::Delete(second_last_length), Operation::Insert(_)] => {
                *second_last_length += length;
            }
            [.., last_operation @ Operation::Insert(_)] => {
                let operation = mem::replace(last_operation, Operation::Delete(length));
                self.operations.push(operation);
            }
            _ => self.operations.push(Operation::Delete(length)),
        }
    }

    pub fn retain(&mut self, length: Length) {
        if length == Length::default() {
            return;
        }
        match self.operations.last_mut() {
            Some(Operation::Retain(last_length)) => {
                *last_length += length;
            }
            _ => self.operations.push(Operation::Retain(length)),
        }
    }

    pub fn insert(&mut self, text: Text) {
        if text.is_empty() {
            return;
        }
        match self.operations.as_mut_slice() {
            [.., Operation::Insert(last_text)] => {
                *last_text += text;
            }
            _ => self.operations.push(Operation::Insert(text)),
        }
    }

    pub fn finish(mut self) -> Diff {
        if let Some(Operation::Retain(_)) = self.operations.last() {
            self.operations.pop();
        }
        Diff {
            operations: self.operations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Operation>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter {
    iter: vec::IntoIter<Operation>,
}

impl Iterator for IntoIter {
    type Item = Operation;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    Delete(Length),
    Retain(Length),
    Insert(Text),
}

impl Operation {
    pub fn info(&self) -> OperationInfo {
        match *self {
            Self::Delete(length) => OperationInfo::Delete(length),
            Self::Retain(length) => OperationInfo::Retain(length),
            Self::Insert(ref text) => OperationInfo::Insert(text.length()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationInfo {
    Delete(Length),
    Retain(Length),
    Insert(Length),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Strategy {
    InsertBefore,
    InsertAfter,
}
use {
    crate::{line, token::TokenInfo, Affinity, Line, Selection, Settings, Text, Tokenizer},
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Document<'a> {
    settings: &'a Settings,
    text: &'a Text,
    tokenizer: &'a Tokenizer,
    text_inlays: &'a [Vec<(usize, String)>],
    line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
    wrap_bytes: &'a [Vec<usize>],
    start_column_after_wrap: &'a [usize],
    fold_column: &'a [usize],
    scale: &'a [f64],
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    summed_heights: &'a [f64],
    selections: &'a [Selection],
    latest_selection_index: usize,
}

impl<'a> Document<'a> {
    pub fn new(
        settings: &'a Settings,
        text: &'a Text,
        tokenizer: &'a Tokenizer,
        text_inlays: &'a [Vec<(usize, String)>],
        line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
        wrap_bytes: &'a [Vec<usize>],
        start_column_after_wrap: &'a [usize],
        fold_column: &'a [usize],
        scale: &'a [f64],
        line_inlays: &'a [(usize, LineInlay)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        summed_heights: &'a [f64],
        selections: &'a [Selection],
        latest_selection_index: usize,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
        }
    }

    pub fn settings(&self) -> &'a Settings {
        self.settings
    }

    pub fn compute_width(&self) -> f64 {
        let mut max_width = 0.0f64;
        for element in self.elements(0, self.line_count()) {
            max_width = max_width.max(match element {
                Element::Line(_, line) => line.compute_width(self.settings.tab_column_count),
                Element::Widget(_, widget) => widget.width,
            });
        }
        max_width
    }

    pub fn height(&self) -> f64 {
        self.summed_heights[self.line_count() - 1]
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => {
                if line_index == self.line_count() {
                    line_index
                } else {
                    line_index + 1
                }
            }
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.as_lines().len()
    }

    pub fn line(&self, line: usize) -> Line<'a> {
        Line::new(
            &self.text.as_lines()[line],
            &self.tokenizer.token_infos()[line],
            &self.text_inlays[line],
            &self.line_widget_inlays[line],
            &self.wrap_bytes[line],
            self.start_column_after_wrap[line],
            self.fold_column[line],
            self.scale[line],
        )
    }

    pub fn lines(&self, start_line: usize, end_line: usize) -> Lines<'a> {
        Lines {
            text: self.text.as_lines()[start_line..end_line].iter(),
            token_infos: self.tokenizer.token_infos()[start_line..end_line].iter(),
            text_inlays: self.text_inlays[start_line..end_line].iter(),
            line_widget_inlays: self.line_widget_inlays[start_line..end_line].iter(),
            wrap_bytes: self.wrap_bytes[start_line..end_line].iter(),
            start_column_after_wrap: self.start_column_after_wrap[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
        }
    }

    pub fn line_y(&self, line: usize) -> f64 {
        if line == 0 {
            0.0
        } else {
            self.summed_heights[line - 1]
        }
    }

    pub fn elements(&self, start_line: usize, end_line: usize) -> Elements<'a> {
        Elements {
            lines: self.lines(start_line, end_line),
            line_inlays: &self.line_inlays[self
                .line_inlays
                .iter()
                .position(|(line, _)| *line >= start_line)
                .unwrap_or(self.line_inlays.len())..],
            widget_inlays: &self.widget_inlays[self
                .widget_inlays
                .iter()
                .position(|((line, _), _)| *line >= start_line)
                .unwrap_or(self.widget_inlays.len())..],
            line: start_line,
        }
    }

    pub fn selections(&self) -> &'a [Selection] {
        self.selections
    }

    pub fn latest_selection_index(&self) -> usize {
        self.latest_selection_index
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
    token_infos: slice::Iter<'a, Vec<TokenInfo>>,
    text_inlays: slice::Iter<'a, Vec<(usize, String)>>,
    line_widget_inlays: slice::Iter<'a, Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: slice::Iter<'a, Vec<usize>>,
    start_column_after_wrap: slice::Iter<'a, usize>,
    fold_column: slice::Iter<'a, usize>,
    scale: slice::Iter<'a, f64>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line::new(
            self.text.next()?,
            self.token_infos.next()?,
            self.text_inlays.next()?,
            self.line_widget_inlays.next()?,
            self.wrap_bytes.next()?,
            *self.start_column_after_wrap.next()?,
            *self.fold_column.next()?,
            *self.scale.next()?,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    lines: Lines<'a>,
    line_inlays: &'a [(usize, LineInlay)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    line: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .line_inlays
            .first()
            .map_or(false, |(line, _)| *line == self.line)
        {
            let ((_, line), line_inlays) = self.line_inlays.split_first().unwrap();
            self.line_inlays = line_inlays;
            return Some(Element::Line(true, line.as_line()));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let line = self.lines.next()?;
        self.line += 1;
        Some(Element::Line(false, line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element<'a> {
    Line(bool, Line<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LineInlay {
    text: String,
}

impl LineInlay {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_line(&self) -> Line<'_> {
        Line::new(&self.text, &[], &[], &[], &[], 0, 0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: f64,
    pub height: f64,
}

impl Widget {
    pub fn new(id: usize, width: f64, height: f64) -> Self {
        Self { id, width, height }
    }
}
use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: Range) -> Diff {
    replace(range, "\n".into())
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Position::new(
            position.line,
            text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        );
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Position::new(prev_line, text.as_lines()[prev_line].len());
    }
    position
}
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Length {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Length {
    pub fn new(line_count: usize, byte_count: usize) -> Self {
        Self {
            line_count,
            byte_count,
        }
    }
}

impl Add for Length {
    type Output = Length;

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

impl AddAssign for Length {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Length {
    type Output = Length;

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

impl SubAssign for Length {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
pub mod bias;
pub mod char;
pub mod code_editor;
pub mod context;
pub mod diff;
pub mod document;
pub mod edit_ops;
pub mod length;
pub mod line;
pub mod move_ops;
pub mod position;
pub mod range;
pub mod selection;
pub mod settings;
pub mod state;
pub mod str;
pub mod text;
pub mod token;
pub mod tokenizer;

pub use crate::{
    bias::Affinity, code_editor::CodeEditor, context::Context, diff::Diff, document::Document,
    length::Length, line::Line, position::Position, range::Range, selection::Selection,
    settings::Settings, state::State, text::Text, token::Token, tokenizer::Tokenizer,
};
use {
    crate::{
        token::{TokenInfo, TokenKind},
        Affinity, Token,
    },
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line<'a> {
    text: &'a str,
    token_infos: &'a [TokenInfo],
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    wrap_bytes: &'a [usize],
    start_column_after_wrap: usize,
    fold_column: usize,
    scale: f64,
}

impl<'a> Line<'a> {
    pub fn new(
        text: &'a str,
        token_infos: &'a [TokenInfo],
        text_inlays: &'a [(usize, String)],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        wrap_bytes: &'a [usize],
        start_column_after_wrap: usize,
        fold_column: usize,
        scale: f64,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
        }
    }

    pub fn compute_column_count(&self, tab_column_count: usize) -> usize {
        use crate::str::StrExt;

        let mut max_summed_column_count = 0;
        let mut summed_column_count = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(_, token) => {
                    summed_column_count += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    summed_column_count += widget.column_count;
                }
                WrappedElement::Wrap => {
                    max_summed_column_count = max_summed_column_count.max(summed_column_count);
                    summed_column_count = self.start_column_after_wrap();
                }
            }
        }
        max_summed_column_count.max(summed_column_count)
    }

    pub fn row_count(&self) -> usize {
        self.wrap_bytes.len() + 1
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        self.column_to_x(self.compute_column_count(tab_column_count))
    }

    pub fn height(&self) -> f64 {
        self.scale * self.row_count() as f64
    }

    pub fn byte_bias_to_row_column(
        &self,
        (byte, bias): (usize, Affinity),
        tab_column_count: usize,
    ) -> (usize, usize) {
        use crate::str::StrExt;

        let mut current_byte = 0;
        let mut row = 0;
        let mut column = 0;
        if byte == current_byte && bias == Affinity::Before {
            return (row, column);
        }
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        if byte == current_byte && bias == Affinity::After {
                            return (row, column);
                        }
                        current_byte += grapheme.len();
                        column += grapheme.column_count(tab_column_count);
                        if byte == current_byte && bias == Affinity::Before {
                            return (row, column);
                        }
                    }
                }
                WrappedElement::Token(true, token) => {
                    column += token.text.column_count(tab_column_count);
                }
                WrappedElement::Widget(_, widget) => {
                    column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    row += 1;
                    column = self.start_column_after_wrap();
                }
            }
        }
        if byte == current_byte && bias == Affinity::After {
            return (row, column);
        }
        panic!()
    }

    pub fn row_column_to_byte_bias(
        &self,
        (row, column): (usize, usize),
        tab_column_count: usize,
    ) -> (usize, Affinity) {
        use crate::str::StrExt;

        let mut byte = 0;
        let mut current_row = 0;
        let mut current_column = 0;
        for wrapped_element in self.wrapped_elements() {
            match wrapped_element {
                WrappedElement::Token(false, token) => {
                    for grapheme in token.text.graphemes() {
                        let next_column = current_column + grapheme.column_count(tab_column_count);
                        if current_row == row && (current_column..next_column).contains(&column) {
                            return (byte, Affinity::After);
                        }
                        byte = byte + grapheme.len();
                        current_column = next_column;
                    }
                }
                WrappedElement::Token(true, token) => {
                    let next_column = current_column + token.text.column_count(tab_column_count);
                    if current_row == row && (current_column..next_column).contains(&column) {
                        return (byte, Affinity::Before);
                    }
                    current_column = next_column;
                }
                WrappedElement::Widget(_, widget) => {
                    current_column += widget.column_count;
                }
                WrappedElement::Wrap => {
                    if current_row == row {
                        return (byte, Affinity::Before);
                    }
                    current_row += 1;
                    current_column = self.start_column_after_wrap();
                }
            }
        }
        if current_row == row {
            return (byte, Affinity::After);
        }
        panic!()
    }

    pub fn column_to_x(&self, column: usize) -> f64 {
        let column_count_before_fold_column = column.min(self.fold_column);
        let column_count_after_fold_column = column - column_count_before_fold_column;
        column_count_before_fold_column as f64 + self.scale * column_count_after_fold_column as f64
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn tokens(&self) -> Tokens<'a> {
        Tokens {
            text: self.text,
            token_infos: self.token_infos.iter(),
        }
    }

    pub fn elements(&self) -> Elements<'a> {
        let mut tokens = self.tokens();
        Elements {
            token: tokens.next(),
            tokens,
            text_inlays: self.text_inlays,
            widget_inlays: self.widget_inlays,
            byte: 0,
        }
    }

    pub fn wrapped_elements(&self) -> WrappedElements<'a> {
        let mut elements = self.elements();
        WrappedElements {
            element: elements.next(),
            elements,
            wrap_bytes: self.wrap_bytes,
            byte: 0,
        }
    }

    pub fn start_column_after_wrap(&self) -> usize {
        self.start_column_after_wrap
    }

    pub fn fold_column(&self) -> usize {
        self.fold_column
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }
}

#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    token_infos: slice::Iter<'a, TokenInfo>,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.token_infos.next() {
            Some(token_info) => {
                let (text_0, text_1) = self.text.split_at(token_info.byte_count);
                self.text = text_1;
                Token::new(text_0, token_info.kind)
            }
            None => {
                if self.text.is_empty() {
                    return None;
                }
                let text = self.text;
                self.text = "";
                Token::new(text, TokenKind::Unknown)
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    token: Option<Token<'a>>,
    tokens: Tokens<'a>,
    text_inlays: &'a [(usize, String)],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    byte: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .text_inlays
            .first()
            .map_or(false, |(byte, _)| *byte == self.byte)
        {
            let ((_, text), text_inlays) = self.text_inlays.split_first().unwrap();
            self.text_inlays = text_inlays;
            return Some(Element::Token(true, Token::new(text, TokenKind::Unknown)));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((byte, bias), _)| {
                *byte == self.byte && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let token = self.token.take()?;
        let mut byte_count = token.text.len();
        if let Some((byte, _)) = self.text_inlays.first() {
            byte_count = byte_count.min(*byte - self.byte);
        }
        if let Some(((byte, _), _)) = self.widget_inlays.first() {
            byte_count = byte_count.min(byte - self.byte);
        }
        let token = if byte_count < token.text.len() {
            let (text_0, text_1) = token.text.split_at(byte_count);
            self.token = Some(Token::new(text_1, token.kind));
            Token::new(text_0, token.kind)
        } else {
            self.token = self.tokens.next();
            token
        };
        self.byte += token.text.len();
        Some(Element::Token(false, token))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Element<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Debug)]
pub struct WrappedElements<'a> {
    element: Option<Element<'a>>,
    elements: Elements<'a>,
    wrap_bytes: &'a [usize],
    byte: usize,
}

impl<'a> Iterator for WrappedElements<'a> {
    type Item = WrappedElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(Element::Widget(Affinity::Before, ..)) = self.element {
            let Element::Widget(_, widget) = self.element.take().unwrap() else {
                panic!()
            };
            self.element = self.elements.next();
            return Some(WrappedElement::Widget(Affinity::Before, widget));
        }
        if self
            .wrap_bytes
            .first()
            .map_or(false, |byte| *byte == self.byte)
        {
            self.wrap_bytes = &self.wrap_bytes[1..];
            return Some(WrappedElement::Wrap);
        }
        Some(match self.element.take()? {
            Element::Token(is_inlay, token) => {
                let mut byte_count = token.text.len();
                if let Some(byte) = self.wrap_bytes.first() {
                    byte_count = byte_count.min(*byte - self.byte);
                }
                let token = if byte_count < token.text.len() {
                    let (text_0, text_1) = token.text.split_at(byte_count);
                    self.element = Some(Element::Token(is_inlay, Token::new(text_1, token.kind)));
                    Token::new(text_0, token.kind)
                } else {
                    self.element = self.elements.next();
                    token
                };
                self.byte += token.text.len();
                WrappedElement::Token(is_inlay, token)
            }
            Element::Widget(Affinity::After, widget) => {
                self.element = self.elements.next();
                WrappedElement::Widget(Affinity::After, widget)
            }
            Element::Widget(Affinity::Before, _) => panic!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WrappedElement<'a> {
    Token(bool, Token<'a>),
    Widget(Affinity, Widget),
    Wrap,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub column_count: usize,
}

impl Widget {
    pub fn new(id: usize, column_count: usize) -> Self {
        Self { id, column_count }
    }
}
mod app;

fn main() {
    app::app_main();
}
use crate::{Affinity, Document, Position};

pub fn move_left(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    ((position, Affinity::Before), None)
}

pub fn move_right(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    ((position, Affinity::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_first_row_of_line(document, (position, bias)) {
        return move_to_prev_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_first_line(position) {
        return move_to_last_row_of_prev_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    if !is_at_last_row_of_line(document, (position, bias)) {
        return move_to_next_row_of_line(document, (position, bias), preferred_column);
    }
    if !is_at_last_line(document, position) {
        return move_to_first_row_of_next_line(document, (position, bias), preferred_column);
    }
    ((position, bias), preferred_column)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Position) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    document
        .line(position.line)
        .byte_bias_to_row_column(
            (position.byte, bias),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
) -> bool {
    let line = document.line(position.line);
    line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    )
    .0 == line.row_count() - 1
}

fn is_at_first_line(position: Position) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Position) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    (
        (
            Position::new(
                position.line,
                document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            ),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        (
            Position::new(
                position.line,
                line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            ),
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Position,
) -> ((Position, Affinity), Option<usize>) {
    let prev_line = position.line - 1;
    (
        (
            Position::new(prev_line, document.line(prev_line).text().len()),
            Affinity::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Position) -> ((Position, Affinity), Option<usize>) {
    (
        (Position::new(position.line + 1, 0), Affinity::Before),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row - 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row + 1, column), document.settings().tab_column_count);
    ((Position::new(position.line, byte), bias), Some(column))
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = position.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, bias) = prev_line_ref.row_column_to_byte_bias(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    ((Position::new(prev_line, byte), bias), Some(column))
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    (position, bias): (Position, Affinity),
    preferred_column: Option<usize>,
) -> ((Position, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_bias_to_row_column(
        (position.byte, bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = position.line + 1;
    let (byte, bias) = document
        .line(next_line)
        .row_column_to_byte_bias((0, column), document.settings().tab_column_count);
    ((Position::new(next_line, byte), bias), Some(column))
}
use {
    crate::{diff::Strategy, Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }

    pub fn apply_diff(self, diff: &Diff, strategy: Strategy) -> Position {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_position = Position::default();
        let mut distance_to_position = self - Position::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_position += length;
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_position + distance_to_position;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_position == Length::default() {
                        break match strategy {
                            Strategy::InsertBefore => diffed_position + length,
                            Strategy::InsertAfter => diffed_position,
                        };
                    } else {
                        diffed_position += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_position = Length::default();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_position + distance_to_position;
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.byte_count,
            }
        } else {
            Self {
                line: self.line + length.line_count,
                byte: length.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Length {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Length {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
use crate::{Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }
}
use crate::{Affinity, Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Position, Affinity),
        cursor: (Position, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(mut self, mut other: Self) -> bool {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Position, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Position, Affinity) {
        self.anchor.max(self.cursor)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce((Position, Affinity), Option<usize>) -> ((Position, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Settings {
    pub tab_column_count: usize,
    pub indent_column_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_column_count: 4,
            indent_column_count: 4,
        }
    }
}
use {
    crate::{
        document, document::LineInlay, line, Affinity, Context, Document, Selection, Settings,
        Text, Tokenizer,
    },
    std::{
        collections::{HashMap, HashSet},
        io,
        path::Path,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    settings: Settings,
    view_id: usize,
    views: HashMap<ViewId, View>,
    editor_id: usize,
    editors: HashMap<EditorId, Editor>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn document(&self, view_id: ViewId) -> Document<'_> {
        let view = &self.views[&view_id];
        let editor = &self.editors[&view.editor_id];
        Document::new(
            &self.settings,
            &editor.text,
            &editor.tokenizer,
            &editor.text_inlays,
            &editor.line_widget_inlays,
            &view.wrap_bytes,
            &view.start_column_after_wrap,
            &view.fold_column,
            &view.scale,
            &editor.line_inlays,
            &editor.document_widget_inlays,
            &view.summed_heights,
            &view.selections,
            view.latest_selection_index,
        )
    }

    pub fn context(&mut self, view_id: ViewId) -> Context<'_> {
        let view = self.views.get_mut(&view_id).unwrap();
        let editor = self.editors.get_mut(&view.editor_id).unwrap();
        Context::new(
            &mut self.settings,
            &mut editor.text,
            &mut editor.tokenizer,
            &mut editor.text_inlays,
            &mut editor.line_widget_inlays,
            &mut view.wrap_bytes,
            &mut view.start_column_after_wrap,
            &mut view.fold_column,
            &mut view.scale,
            &mut editor.line_inlays,
            &mut editor.document_widget_inlays,
            &mut view.summed_heights,
            &mut view.selections,
            &mut view.latest_selection_index,
            &mut view.folding_lines,
            &mut view.unfolding_lines,
        )
    }

    pub fn open_view(&mut self, path: impl AsRef<Path>) -> io::Result<ViewId> {
        let editor_id = self.open_editor(path)?;
        let view_id = ViewId(self.view_id);
        self.view_id += 1;
        let line_count = self.editors[&editor_id].text.as_lines().len();
        self.views.insert(
            view_id,
            View {
                editor_id,
                wrap_bytes: (0..line_count).map(|_| [].into()).collect(),
                start_column_after_wrap: (0..line_count).map(|_| 0).collect(),
                fold_column: (0..line_count).map(|_| 0).collect(),
                scale: (0..line_count).map(|_| 1.0).collect(),
                summed_heights: Vec::new(),
                selections: [Selection::default()].into(),
                latest_selection_index: 0,
                folding_lines: HashSet::new(),
                unfolding_lines: HashSet::new(),
            },
        );
        self.context(view_id).update_summed_heights();
        Ok(view_id)
    }

    fn open_editor(&mut self, path: impl AsRef<Path>) -> io::Result<EditorId> {
        use std::fs;

        let editor_id = EditorId(self.editor_id);
        self.editor_id += 1;
        let bytes = fs::read(path.as_ref())?;
        let text: Text = String::from_utf8_lossy(&bytes).into();
        let tokenizer = Tokenizer::new(&text);
        let line_count = text.as_lines().len();
        self.editors.insert(
            editor_id,
            Editor {
                text,
                tokenizer,
                text_inlays: (0..line_count)
                    .map(|line| {
                        if line % 2 == 0 {
                            [
                                (20, "###".into()),
                                (40, "###".into()),
                                (60, "###".into()),
                                (80, "###".into()),
                            ]
                            .into()
                        } else {
                            [].into()
                        }
                    })
                    .collect(),
                line_inlays: [
                    (
                        10,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        20,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        30,
                        LineInlay::new("##################################################".into()),
                    ),
                    (
                        40,
                        LineInlay::new("##################################################".into()),
                    ),
                ]
                .into(),
                line_widget_inlays: (0..line_count).map(|_| [].into()).collect(),
                document_widget_inlays: [].into(),
            },
        );
        Ok(editor_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ViewId(usize);

#[derive(Clone, Debug, PartialEq)]
struct View {
    editor_id: EditorId,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    wrap_bytes: Vec<Vec<usize>>,
    start_column_after_wrap: Vec<usize>,
    summed_heights: Vec<f64>,
    selections: Vec<Selection>,
    latest_selection_index: usize,
    folding_lines: HashSet<usize>,
    unfolding_lines: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct EditorId(usize);

#[derive(Clone, Debug, PartialEq)]
struct Editor {
    text: Text,
    tokenizer: Tokenizer,
    text_inlays: Vec<Vec<(usize, String)>>,
    line_widget_inlays: Vec<Vec<((usize, Affinity), line::Widget)>>,
    line_inlays: Vec<(usize, LineInlay)>,
    document_widget_inlays: Vec<((usize, Affinity), document::Widget)>,
}
pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize;
    fn indentation(&self) -> &str;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        use crate::char::CharExt;

        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn indent_level(&self, tab_column_count: usize, indent_column_count: usize) -> usize {
        self.indentation().column_count(tab_column_count) / indent_column_count
    }

    fn indentation(&self) -> &str {
        &self[..self
            .char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(self.len())]
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
        let mut prev_grapheme_is_whitespace = None;
        let index = self
            .string
            .grapheme_indices()
            .find_map(|(index, next_grapheme)| {
                let next_grapheme_is_whitespace =
                    next_grapheme.chars().all(|char| char.is_whitespace());
                let is_whitespace_boundary =
                    prev_grapheme_is_whitespace.map_or(false, |prev_grapheme_is_whitespace| {
                        prev_grapheme_is_whitespace != next_grapheme_is_whitespace
                    });
                prev_grapheme_is_whitespace = Some(next_grapheme_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(string)
    }
}
use {
    crate::{Diff, Length, Position, Range},
    std::{borrow::Cow, ops::AddAssign},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.length() == Length::default()
    }

    pub fn length(&self) -> Length {
        Length {
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

    pub fn take(&mut self, len: Length) -> Self {
        let mut lines = self
            .lines
            .drain(..len.line_count as usize)
            .collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte_count].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Length) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, position: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line][..position.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line][position.byte..]);
            self.lines
                .splice(position.line..position.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line]
                .replace_range(position.byte..position.byte + length.byte_count, "");
        } else {
            let mut line = self.lines[position.line][..position.byte].to_string();
            line.push_str(&self.lines[position.line + length.line_count][length.byte_count..]);
            self.lines.splice(
                position.line..position.line + length.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Operation;

        let mut position = Position::default();
        for operation in diff {
            match operation {
                Operation::Delete(length) => self.delete(position, length),
                Operation::Retain(length) => position += length,
                Operation::Insert(text) => {
                    let length = text.length();
                    self.insert(position, text);
                    position += length;
                }
            }
        }
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, mut other: Self) {
        other
            .lines
            .first_mut()
            .unwrap()
            .replace_range(..0, self.lines.last().unwrap());
        self.lines
            .splice(self.lines.len() - 1..self.lines.len(), other.lines);
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<char> for Text {
    fn from(char: char) -> Self {
        Self {
            lines: match char {
                '\n' | '\r' => vec![String::new(), String::new()],
                _ => vec![char.into()],
            },
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.split('\n').map(|line| line.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
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

impl From<Cow<'_, str>> for Text {
    fn from(string: Cow<'_, str>) -> Self {
        string.as_ref().into()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    pub fn new(text: &'a str, kind: TokenKind) -> Self {
        Self { text, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub byte_count: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self {
            byte_count: len,
            kind,
        }
    }
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
use crate::{
    token::{TokenInfo, TokenKind},
    Diff, Text,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Tokenizer {
    state: Vec<Option<(State, State)>>,
    token_infos: Vec<Vec<TokenInfo>>,
}

impl Tokenizer {
    pub fn new(text: &Text) -> Self {
        let line_count = text.as_lines().len();
        let mut tokenizer = Self {
            state: (0..line_count).map(|_| None).collect(),
            token_infos: (0..line_count).map(|_| Vec::new()).collect(),
        };
        tokenizer.retokenize(&Diff::new(), text);
        tokenizer
    }

    pub fn token_infos(&self) -> &[Vec<TokenInfo>] {
        &self.token_infos
    }

    pub fn retokenize(&mut self, diff: &Diff, text: &Text) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    self.state.drain(line..line + length.line_count);
                    self.token_infos.drain(line..line + length.line_count);
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    self.state[line] = None;
                    self.token_infos[line] = Vec::new();
                    self.state
                        .splice(line..line, (0..length.line_count).map(|_| None));
                    self.token_infos
                        .splice(line..line, (0..length.line_count).map(|_| Vec::new()));
                    line += length.line_count;
                }
            }
        }
        let mut state = State::default();
        for line in 0..text.as_lines().len() {
            match self.state[line] {
                Some((start_state, end_state)) if state == start_state => {
                    state = end_state;
                }
                _ => {
                    let start_state = state;
                    let mut token_infos = Vec::new();
                    let mut cursor = Cursor::new(&text.as_lines()[line]);
                    loop {
                        let (next_state, token) = state.next(&mut cursor);
                        state = next_state;
                        match token {
                            Some(token) => token_infos.push(token),
                            None => break,
                        }
                    }
                    self.state[line] = Some((start_state, state));
                    self.token_infos[line] = token_infos;
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
    pub fn next(self, cursor: &mut Cursor) -> (State, Option<TokenInfo>) {
        if cursor.peek(0) == '\0' {
            return (self, None);
        }
        let start = cursor.index;
        let (next_state, token_kind) = match self {
            State::Initial(state) => state.next(cursor),
        };
        let end = cursor.index;
        assert!(start < end);
        (next_state, Some(TokenInfo::new(end - start, token_kind)))
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}
use {
    makepad_code_editor::{code_editor, state::ViewId, CodeEditor},
    makepad_widgets::*,
};

live_design! {
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::hook_widget::HookWidget;

    App = {{App}} {
        ui: <DesktopWindow> {
            code_editor = <HookWidget> {}
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    code_editor: CodeEditor,
    #[rust]
    state: State,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            let mut cx = Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(&mut cx).hook_widget() {
                if next == self.ui.get_widget(id!(code_editor)) {
                    let mut context = self.state.code_editor.context(self.state.view_id);
                    self.code_editor.draw(&mut cx, &mut context);
                }
            }
            return;
        }
        self.ui.handle_widget_event(cx, event);
        self.code_editor
            .handle_event(cx, &mut self.state.code_editor, self.state.view_id, event)
    }
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        code_editor::live_design(cx);
    }
}

struct State {
    code_editor: makepad_code_editor::State,
    view_id: ViewId,
}

impl Default for State {
    fn default() -> Self {
        let mut code_editor = makepad_code_editor::State::new();
        let view_id = code_editor.open_view("code_editor/src/line.rs").unwrap();
        Self {
            code_editor,
            view_id,
        }
    }
}

app_main!(App);
pub trait CharExt {
    fn column_count(self, tab_column_count: usize) -> usize;
}

impl CharExt for char {
    fn column_count(self, tab_column_count: usize) -> usize {
        match self {
            '\t' => tab_column_count,
            _ => 1,
        }
    }
}
use {
    crate::{state::ViewId, Affinity, Context, Document, Position, Selection, State},
    makepad_widgets::*,
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;

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

    CodeEditor = {{CodeEditor}} {
        walk: {
            width: Fill,
            height: Fill,
            margin: 0,
        }
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

#[derive(Live, LiveHook)]
pub struct CodeEditor {
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawSelection,
    #[live]
    draw_cursor: DrawColor,
    #[live]
    token_colors: TokenColors,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    cell_size: DVec2,
    #[rust]
    start_line: usize,
    #[rust]
    end_line: usize,
}

impl CodeEditor {
    pub fn draw(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.begin(cx, context);
        let document = context.document();
        self.draw_text(cx, &document);
        self.draw_selections(cx, &document);
        self.end(cx, context);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, state: &mut State, view_id: ViewId, event: &Event) {
        use crate::str::StrExt;

        self.scroll_bars.handle_event_with(cx, event, &mut |cx, _| {
            cx.redraw_all();
        });
        match event {
            Event::TextInput(TextInputEvent { input, .. }) => {
                state.context(view_id).replace(input.into());
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                state.context(view_id).enter();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                state.context(view_id).delete();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                state.context(view_id).backspace();
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_left(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_right(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_up(*shift);
                cx.redraw_all();
            }

            Event::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                state.context(view_id).move_cursors_down(*shift);
                cx.redraw_all();
            }
            Event::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.fold_line(line, 2 * settings.indent_column_count);
                    }
                }
                cx.redraw_all();
            }
            Event::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                let mut context = state.context(view_id);
                for line in 0..context.document().line_count() {
                    let document = context.document();
                    let settings = document.settings();
                    if document
                        .line(line)
                        .text()
                        .indent_level(settings.tab_column_count, settings.indent_column_count)
                        >= 2
                    {
                        context.unfold_line(line);
                    }
                }
                cx.redraw_all();
            }
            _ => {}
        }
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent {
                abs,
                rect,
                modifiers: KeyModifiers { alt, .. },
                ..
            }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    if alt {
                        context.insert_cursor(cursor);
                    } else {
                        context.set_cursor(cursor);
                    }
                    cx.redraw_all();
                }
            }
            Hit::FingerMove(FingerMoveEvent { abs, rect, .. }) => {
                let document = state.document(view_id);
                if let Some(cursor) = self.pick(&document, abs - rect.pos) {
                    let mut context = state.context(view_id);
                    context.move_cursor_to(true, cursor);
                    cx.redraw_all();
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        self.viewport_rect = Rect {
            pos: self.scroll_bars.get_scroll_pos(),
            size: cx.turtle().rect().size,
        };
        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        context.wrap_lines((self.viewport_rect.size.x / self.cell_size.x) as usize);
        let document = context.document();
        self.start_line =
            document.find_first_line_ending_after_y(self.viewport_rect.pos.y / self.cell_size.y);
        self.end_line = document.find_first_line_starting_after_y(
            (self.viewport_rect.pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.scroll_bars.begin(cx, self.walk, Layout::default());
    }

    fn end(&mut self, cx: &mut Cx2d<'_>, context: &mut Context<'_>) {
        let document = context.document();
        cx.turtle_mut().set_used(
            document.compute_width() * self.cell_size.x,
            document.height() * self.cell_size.y,
        );
        self.scroll_bars.end(cx);
        if context.update_fold_animations() {
            cx.redraw_all();
        }
    }

    fn draw_text(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt, token::TokenKind};

        let mut y = document.line_y(self.start_line);
        for element in document.elements(self.start_line, self.end_line) {
            let mut column = 0;
            match element {
                document::Element::Line(_, line) => {
                    self.draw_text.font_scale = line.scale();
                    for wrapped_element in line.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(_, token) => {
                                self.draw_text.color = match token.kind {
                                    TokenKind::Unknown => self.token_colors.unknown,
                                    TokenKind::BranchKeyword => self.token_colors.branch_keyword,
                                    TokenKind::Identifier => self.token_colors.identifier,
                                    TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                    TokenKind::Number => self.token_colors.number,
                                    TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                    TokenKind::Punctuator => self.token_colors.punctuator,
                                    TokenKind::Whitespace => self.token_colors.whitespace,
                                };
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 {
                                        x: line.column_to_x(column),
                                        y,
                                    } * self.cell_size
                                        - self.viewport_rect.pos,
                                    token.text,
                                );
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                y += line.scale();
                                column = line.start_column_after_wrap();
                            }
                        }
                    }
                    y += line.scale();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
    }

    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        let mut active_selection = None;
        let mut selections = document.selections();
        while selections
            .first()
            .map_or(false, |selection| selection.end().0.line < self.start_line)
        {
            selections = &selections[1..];
        }
        if selections.first().map_or(false, |selection| {
            selection.start().0.line < self.start_line
        }) {
            let (selection, remaining_selections) = selections.split_first().unwrap();
            selections = remaining_selections;
            active_selection = Some(ActiveSelection::new(*selection, 0.0));
        }
        DrawSelectionsContext {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selections(cx, document)
    }

    fn pick(&self, document: &Document<'_>, pos: DVec2) -> Option<(Position, Affinity)> {
        use crate::{document, line, str::StrExt};

        let pos = (pos + self.viewport_rect.pos) / self.cell_size;
        let mut line = document.find_first_line_ending_after_y(pos.y);
        let mut y = document.line_y(line);
        for element in document.elements(line, line + 1) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    let next_byte = byte + grapheme.len();
                                    let next_column = column
                                        + grapheme
                                            .column_count(document.settings().tab_column_count);
                                    let next_y = y + line_ref.scale();
                                    let x = line_ref.column_to_x(column);
                                    let next_x = line_ref.column_to_x(next_column);
                                    let mid_x = (x + next_x) / 2.0;
                                    if (y..=next_y).contains(&pos.y) {
                                        if (x..=mid_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, byte),
                                                Affinity::After,
                                            ));
                                        }
                                        if (mid_x..=next_x).contains(&pos.x) {
                                            return Some((
                                                Position::new(line, next_byte),
                                                Affinity::Before,
                                            ));
                                        }
                                    }
                                    byte = next_byte;
                                    column = next_column;
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                let next_column = column
                                    + token
                                        .text
                                        .column_count(document.settings().tab_column_count);
                                let x = line_ref.column_to_x(column);
                                let next_x = line_ref.column_to_x(next_column);
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) && (x..=next_x).contains(&pos.x) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                column = next_column;
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                let next_y = y + line_ref.scale();
                                if (y..=next_y).contains(&pos.y) {
                                    return Some((Position::new(line, byte), Affinity::Before));
                                }
                                y = next_y;
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    let next_y = y + line_ref.scale();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, byte), Affinity::After));
                    }
                    line += 1;
                    y += next_y;
                }
                document::Element::Line(true, line_ref) => {
                    let next_y = y + line_ref.height();
                    if (y..=next_y).contains(&pos.y) {
                        return Some((Position::new(line, 0), Affinity::Before));
                    }
                    y = next_y;
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        None
    }
}

struct DrawSelectionsContext<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: &'a [Selection],
}

impl<'a> DrawSelectionsContext<'a> {
    fn draw_selections(&mut self, cx: &mut Cx2d<'_>, document: &Document<'_>) {
        use crate::{document, line, str::StrExt};

        let mut line = self.code_editor.start_line;
        let mut y = document.line_y(line);
        for element in document.elements(self.code_editor.start_line, self.code_editor.end_line) {
            match element {
                document::Element::Line(false, line_ref) => {
                    let mut byte = 0;
                    let mut column = 0;
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::Before,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    for wrapped_element in line_ref.wrapped_elements() {
                        match wrapped_element {
                            line::WrappedElement::Token(false, token) => {
                                for grapheme in token.text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::After,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                    byte += grapheme.len();
                                    column +=
                                        grapheme.column_count(document.settings().tab_column_count);
                                    self.handle_event(
                                        cx,
                                        line,
                                        byte,
                                        Affinity::Before,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                            }
                            line::WrappedElement::Token(true, token) => {
                                column += token
                                    .text
                                    .column_count(document.settings().tab_column_count);
                            }
                            line::WrappedElement::Widget(_, widget) => {
                                column += widget.column_count;
                            }
                            line::WrappedElement::Wrap => {
                                column += 1;
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line_ref.column_to_x(column),
                                        y,
                                        line_ref.scale(),
                                    );
                                }
                                y += line_ref.scale();
                                column = line_ref.start_column_after_wrap();
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line,
                        byte,
                        Affinity::After,
                        line_ref.column_to_x(column),
                        y,
                        line_ref.scale(),
                    );
                    column += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line_ref.column_to_x(column), y, line_ref.scale());
                    }
                    line += 1;
                    y += line_ref.scale();
                }
                document::Element::Line(true, line_ref) => {
                    y += line_ref.height();
                }
                document::Element::Widget(_, widget) => {
                    y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: usize,
        byte: usize,
        bias: Affinity,
        x: f64,
        y: f64,
        height: f64,
    ) {
        let position = Position::new(line, byte);
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == (position, bias)
        }) {
            self.draw_selection(cx, x, y, height);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
        }
        if self
            .selections
            .first()
            .map_or(false, |selection| selection.start() == (position, bias))
        {
            let (selection, selections) = self.selections.split_first().unwrap();
            self.selections = selections;
            if selection.cursor == (position, bias) {
                self.draw_cursor(cx, x, y, height);
            }
            if !selection.is_empty() {
                self.active_selection = Some(ActiveSelection {
                    selection: *selection,
                    start_x: x,
                });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        use std::mem;

        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 { x: start_x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: height,
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d<'_>, x: f64, y: f64, height: f64) {
        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y } * self.code_editor.cell_size
                    - self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: height * self.code_editor.cell_size.y,
                },
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActiveSelection {
    selection: Selection,
    start_x: f64,
}

impl ActiveSelection {
    fn new(selection: Selection, start_x: f64) -> Self {
        Self { selection, start_x }
    }
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

    fn end(&mut self, cx: &mut Cx2d<'_>) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    fn draw(&mut self, cx: &mut Cx2d<'_>, rect: Rect) {
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

#[derive(Live, LiveHook)]
pub struct TokenColors {
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
use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    start_column_after_wrap: &'a mut Vec<usize>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        start_column_after_wrap: &'a mut Vec<usize>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            start_column_after_wrap,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.start_column_after_wrap,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = Vec::new();
            mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            let line_ref = document.line(line);
            let mut start_column_after_wrap = line_ref
                .text()
                .indentation()
                .column_count(document.settings().tab_column_count);
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            if start_column_after_wrap
                                + string.column_count(document.settings().tab_column_count)
                                > max_column
                            {
                                start_column_after_wrap = 0;
                            }
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        if start_column_after_wrap + widget.column_count > max_column {
                            start_column_after_wrap = 0;
                        }
                    }
                }
            }
            for element in line_ref.elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = start_column_after_wrap;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = start_column_after_wrap;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            self.start_column_after_wrap[line] = start_column_after_wrap;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
        *self.latest_selection_index = 0;
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_column: usize) {
        self.fold_column[line_index] = fold_column;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
        let mut current_selection_index = 0;
        while current_selection_index + 1 < self.selections.len() {
            let next_selection_index = current_selection_index + 1;
            let current_selection = self.selections[current_selection_index];
            let next_selection = self.selections[next_selection_index];
            assert!(current_selection.start() <= next_selection.start());
            if !current_selection.should_merge(next_selection) {
                current_selection_index += 1;
                continue;
            }
            let start = current_selection.start().min(next_selection.start());
            let end = current_selection.end().max(next_selection.end());
            let anchor;
            let cursor;
            if current_selection.anchor <= next_selection.cursor {
                anchor = start;
                cursor = end;
            } else {
                anchor = end;
                cursor = start;
            }
            self.selections[current_selection_index] =
                Selection::new(anchor, cursor, current_selection.preferred_column);
            self.selections.remove(next_selection_index);
            if next_selection_index < *self.latest_selection_index {
                *self.latest_selection_index -= 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            let anchor;
            let cursor;
            if selection.anchor <= selection.cursor {
                anchor = (diffed_start, selection.start().1);
                cursor = (diffed_end, selection.end().1);
            } else {
                anchor = (diffed_end, selection.end().1);
                cursor = (diffed_start, selection.start().1);
            }
            *selection = Selection::new(anchor, cursor, selection.preferred_column);
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.start_column_after_wrap.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_column_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_column
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
