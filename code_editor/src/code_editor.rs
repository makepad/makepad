use {
    crate::{
        decoration::{Decoration, DecorationType},
        layout::{BlockElement, WrappedElement},
        selection::Affinity,
        session::{SelectionMode, Session},
        history::{NewGroup},
        settings::Settings,
        str::StrExt,
        text::Position,
        token::TokenKind,
        Line, Selection, Token,
    },
    makepad_widgets::*,
    std::fmt::Write,
    std::{mem, slice::Iter},
};

live_design! {
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme_desktop_dark::*;

    TokenColors = {{TokenColors}} {
        whitespace: #6E6E6E,
        delimiter: #a,
        delimiter_highlight: #f,
        error_decoration: #f00,
        warning_decoration: #0f0,
        
        unknown: #C0C0C0,
        branch_keyword: #C485BE,
        constant: #CC917B,
        identifier: #D4D4D4,
        loop_keyword: #FF8C00,
        number: #B6CEAA,
        other_keyword: #5B9BD3,
        punctuator: #D4D4D4,
        string: #CC917B,
        function: #fffcc9,
        typename: #56C9B1,
        comment: #638D54,
    }

    DrawIndentGuide = {{DrawIndentGuide}} {
        fn pixel(self) -> vec4 {
            let thickness = 0.8 + self.dpi_dilate * 0.5;
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.move_to(1., -1.);
            sdf.line_to(1., self.rect_size.y + 1.);
            return sdf.stroke(self.color, thickness);
        }
    }

    DrawDecoration = {{DrawDecoration}} {
        fn pixel(self) -> vec4 {
            let transformed_pos = vec2(self.pos.x, self.pos.y + 0.03 * sin(self.pos.x * self.rect_size.x));
            let cx = Sdf2d::viewport(transformed_pos * self.rect_size);
            cx.move_to(0.0, self.rect_size.y - 1.0);
            cx.line_to(self.rect_size.x, self.rect_size.y - 1.0);
            return cx.stroke(self.color, 0.8);
        }
    }

    DrawSelection = {{DrawSelection}} {
        uniform gloopiness: 8.0
        uniform border_radius: 2.0
        uniform focus: 1.0
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
            return sdf.fill(mix(THEME_COLOR_U_1 * 1.1, THEME_COLOR_U_3 * 0.8, self.focus));
        }
    }

    DrawCodeText = {{DrawCodeText}} { }

    CodeEditor = {{CodeEditor}} {
        height: Fill, width: Fill,
        margin: 0,

        scroll_bars: <ScrollBars> {}
        draw_bg: { color: (THEME_COLOR_BG_CONTAINER) }
        draw_gutter: {
            draw_depth: 1.0,
            text_style: <THEME_FONT_CODE> {},
            color: (THEME_COLOR_TEXT_META),
        }
        draw_text: {
            draw_depth: 1.0,
            text_style: <THEME_FONT_CODE> {}
            fn get_brightness(self)->float{
                return 1.1    
            }
            
            fn blend_color(self, incol: vec4) -> vec4 {
                if self.outline < 0.5 {
                    return incol
                }
                if self.pos.y < 0.12 {
                    return #f
                }
                return incol
            }
        }
        draw_indent_guide: {
           // draw_depth: 1.0,
            color: (THEME_COLOR_U_2),
        }
        draw_decoration: {
          //  draw_depth: 2.0,
        }
        draw_selection: {
           // draw_depth: 3.0,
        }

        draw_cursor: {
          //  draw_depth: 4.0,
            uniform blink: 0.0
            instance focus: 0.0
            fn pixel(self) -> vec4 {
                let color = mix(THEME_COLOR_U_HIDDEN, mix(self.color, THEME_COLOR_U_HIDDEN, self.blink),self.focus);
                return vec4(color.rgb*color.a, color.a);
            }
            color: (THEME_COLOR_WHITE),
        }


        draw_cursor_bg: {
            instance focus: 0.0
            fn pixel(self) -> vec4 {
                let color = mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_U_1, self.focus);
                return vec4(color.rgb * color.a, color.a);
            }
        }

        animator: {
            blink = {
                default: off
                off = {
                    from: {all: Forward {duration:0.05}}
                    apply: {
                        draw_cursor: {blink:0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {
                        draw_cursor: {blink:1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration:0.05}}
                    apply: {
                        draw_cursor: {focus:0.0}
                        draw_cursor_bg: {focus:0.0}
                        draw_selection: {focus:0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {
                        draw_cursor: {focus:1.0}
                        draw_cursor_bg: {focus:1.0}
                        draw_selection: {focus:1.0}
                    }
                }
            }
        }
    }

}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
struct DrawCodeText {
    #[deref]
    draw_super: DrawText,
    #[live]
    outline: f32,
}

#[derive(Live, LiveRegister)]
pub struct CodeEditor {
    #[walk] walk: Walk,
    #[live] scroll_bars: ScrollBars,
    #[live] draw_gutter: DrawText,
    #[live] draw_text: DrawCodeText,
    #[live] token_colors: TokenColors,
    #[live] draw_indent_guide: DrawIndentGuide,
    #[live] draw_decoration: DrawDecoration,
    #[live] draw_selection: DrawSelection,
    #[live] draw_cursor: DrawColor,
    #[live] draw_cursor_bg: DrawColor,
    #[live] draw_bg: DrawColor,
    #[rust(KeepCursorInView::Off)] keep_cursor_in_view: KeepCursorInView,
    #[rust] last_cursor_screen_pos: Option<DVec2>,

    #[rust] cell_size: DVec2,
    #[rust] gutter_rect: Rect,
    #[rust] viewport_rect: Rect,
    #[rust] unscrolled_rect: Rect,
    #[rust] line_start: usize,
    #[rust] line_end: usize,

    #[live(true)] word_wrap: bool,

    #[live(0.5)] blink_speed: f64,

    #[animator] animator: Animator,

    #[rust] blink_timer: Timer,
}

enum KeepCursorInView {
    Once,
    Always(DVec2, NextFrame),
    LockStart,
    Locked(DVec2),
    LockedCenter(DVec2, Position, Affinity),
    FontResize(DVec2),
    JumpToPosition,
    Off,
}

impl KeepCursorInView {
    fn is_once(&self) -> bool {
        match self {
            Self::Once => true,
            _ => false,
        }
    }
    fn is_locked(&self) -> bool {
        match self {
            Self::LockStart | Self::Locked(_) | Self::LockedCenter(_, _, _) => true,
            _ => false,
        }
    }
}
impl LiveHook for CodeEditor {
}
/*
impl LiveHook for CodeEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, CodeEditor)
    }
}

impl Widget for CodeEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }

    fn handle_event(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _scope:&mut WidgetScope,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionWrap),
    )->WidgetAction{
        
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _scope:&mut WidgetScope, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
} 

#[derive(Clone, PartialEq, WidgetRef)]
pub struct CodeEditorRef(WidgetRef);
*/

impl CodeEditor {
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }
    
    pub fn area(&self)->Area{
        self.scroll_bars.area()
    }
    
    pub fn walk(&self, _cx:&mut Cx)->Walk{
        self.walk
    }
    
    pub fn uid_to_widget(&self, _uid:WidgetUid)->WidgetRef{
        WidgetRef::empty()    
    }
    
    pub fn find_widgets(&self, _path: &[LiveId], _cached: WidgetCache, _results: &mut WidgetSet){
    }
        
    pub fn draw_walk_editor(&mut self, cx: &mut Cx2d, session: &mut Session, walk:Walk) {
        // This needs to be called first to ensure the session is up to date.
        session.handle_changes();

        self.cell_size =
            self.draw_text.text_style.font_size * self.draw_text.get_monospace_base(cx);
        let last_added_selection =
            session.selections()[session.last_added_selection_index().unwrap()];
        let (cursor_x, cursor_y) = session.layout().logical_to_normalized_position(
            last_added_selection.cursor.position,
            last_added_selection.cursor.affinity,
        );
        let cursor_pos = dvec2(cursor_x, cursor_y) * self.cell_size;
        self.last_cursor_screen_pos = Some(cursor_pos - self.scroll_bars.get_scroll_pos());
        match self.keep_cursor_in_view {
            KeepCursorInView::Once | KeepCursorInView::Always(_, _) => {
                // make a cursor bounding box
                let pad_above = dvec2(self.cell_size.x * 8.0, self.cell_size.y);
                let pad_below = dvec2(self.cell_size.x * 8.0, self.cell_size.y * 2.0);
                let rect = Rect {
                    pos: cursor_pos - pad_above,
                    size: pad_above + pad_below,
                };
                // only scroll into
                self.scroll_bars.scroll_into_view(cx, rect);
                if self.keep_cursor_in_view.is_once() {
                    self.keep_cursor_in_view = KeepCursorInView::Off
                }
            }
            KeepCursorInView::LockStart => {
                // lets get the on screen position
                let screen_pos = cursor_pos - self.scroll_bars.get_scroll_pos();
                let rect = Rect {
                    pos: dvec2(0.0, 0.0),
                    size: self.viewport_rect.size,
                };
                if rect.contains(screen_pos) {
                    self.keep_cursor_in_view = KeepCursorInView::Locked(screen_pos);
                } else {
                    let center = rect.size * 0.5 + self.unscrolled_rect.pos;
                    let ((pos, aff), _) = self.pick(session, center);
                    let (cursor_x, cursor_y) =
                        session.layout().logical_to_normalized_position(pos, aff);
                    let screen_pos = dvec2(cursor_x, cursor_y) * self.cell_size
                        - self.scroll_bars.get_scroll_pos();
                    self.keep_cursor_in_view = KeepCursorInView::LockedCenter(screen_pos, pos, aff);
                }
            }
            KeepCursorInView::Locked(pos) => {
                // ok so we want to keep cursor pos at the same screen position
                let new_pos = cursor_pos - self.scroll_bars.get_scroll_pos();
                let delta = pos - new_pos;
                let new_pos = self.scroll_bars.get_scroll_pos() - dvec2(0.0, delta.y);
                self.scroll_bars.set_scroll_pos_no_clip(cx, new_pos);
                //self.keep_cursor_in_view = KeepCursorInView::Locked(cursor_pos);
            }
            KeepCursorInView::LockedCenter(screen_pos, pos, aff) => {
                let (cursor_x, cursor_y) =
                    session.layout().logical_to_normalized_position(pos, aff);
                let new_pos =
                    dvec2(cursor_x, cursor_y) * self.cell_size - self.scroll_bars.get_scroll_pos();
                let delta = screen_pos - new_pos;
                let new_pos = self.scroll_bars.get_scroll_pos() - dvec2(0.0, delta.y);
                self.scroll_bars.set_scroll_pos_no_clip(cx, new_pos);
                //self.keep_cursor_in_view = KeepCursorInView::Locked(cursor_pos);
            }
            KeepCursorInView::JumpToPosition => {
                // alright so we need to make sure that cursor_pos
                // is in view.
                let padd = dvec2(self.cell_size.x * 10.0, self.cell_size.y * 10.0);
                self.scroll_bars.scroll_into_view(
                    cx,
                    Rect {
                        pos: cursor_pos - padd,
                        size: 2.0 * padd,
                    },
                );
                self.keep_cursor_in_view = KeepCursorInView::Off;
            }
            KeepCursorInView::FontResize(last_pos) => {
                let new_pos = cursor_pos - self.scroll_bars.get_scroll_pos();
                let delta = last_pos - new_pos;
                let new_pos = self.scroll_bars.get_scroll_pos() - dvec2(0.0, delta.y);
                self.scroll_bars.set_scroll_pos_no_clip(cx, new_pos);
                self.keep_cursor_in_view = KeepCursorInView::Off
            }
            KeepCursorInView::Off => {}
        }

        self.scroll_bars.begin(cx, walk, Layout::default());

        let turtle_rect = cx.turtle().rect();
        let gutter_width = (session
            .document()
            .as_text()
            .as_lines()
            .len()
            .to_string()
            .column_count()
            + 3) as f64
            * self.cell_size.x;
        self.gutter_rect = Rect {
            pos: turtle_rect.pos,
            size: DVec2 {
                x: gutter_width,
                y: turtle_rect.size.y,
            },
        };
        self.viewport_rect = Rect {
            pos: DVec2 {
                x: turtle_rect.pos.x + gutter_width,
                y: turtle_rect.pos.y,
            },
            size: DVec2 {
                x: turtle_rect.size.x - gutter_width,
                y: turtle_rect.size.y,
            },
        };

        let pad_left_top = dvec2(10., 10.);
        self.gutter_rect.pos += pad_left_top;
        self.gutter_rect.size -= pad_left_top;
        self.viewport_rect.pos += pad_left_top;
        self.viewport_rect.size -= pad_left_top;

        session.set_wrap_column(if self.word_wrap {
            Some((self.viewport_rect.size.x / self.cell_size.x) as usize)
        } else {
            None
        });

        let scroll_pos = self.scroll_bars.get_scroll_pos();

        self.line_start = session
            .layout()
            .find_first_line_ending_after_y(scroll_pos.y / self.cell_size.y - self.cell_size.y);
        self.line_end = session.layout().find_first_line_starting_after_y(
            (scroll_pos.y + self.viewport_rect.size.y) / self.cell_size.y,
        );
        self.unscrolled_rect = cx.turtle().unscrolled_rect();
        self.draw_bg.draw_abs(cx, cx.turtle().unscrolled_rect());

        self.draw_gutter(cx, session);
        self.draw_selection_layer(cx, session);
        self.draw_text_layer(cx, session);
        self.draw_indent_guide_layer(cx, session);
        self.draw_decoration_layer(cx, session);
        self.draw_selection_layer(cx, session);

        // Get the last added selection.
        // Get the normalized cursor position. To go from normalized to screen position, multiply by
        // the cell size, then shift by the viewport origin.

        cx.turtle_mut().set_used(
            session.layout().width() * self.cell_size.x,
            session.layout().height() * self.cell_size.y + (self.viewport_rect.size.y),
        );

        self.scroll_bars.end(cx);
        if session.update_folds() {
            self.scroll_bars.area().redraw(cx);
        } else if self.keep_cursor_in_view.is_locked() {
            self.keep_cursor_in_view = KeepCursorInView::Off;
        }
    }

    pub fn set_key_focus(&mut self, cx: &mut Cx) {
        cx.set_key_focus(self.scroll_bars.area());
    }

    pub fn set_cursor_and_scroll(
        &mut self,
        cx: &mut Cx,
        pos: Position,
        session: &mut Session,
    ) {
        session.set_selection(pos, Affinity::Before, SelectionMode::Simple, NewGroup::Yes);
        self.keep_cursor_in_view = KeepCursorInView::JumpToPosition;
        self.redraw(cx);
    }

    pub fn reset_font_size(&mut self) {
        self.draw_gutter.text_style.font_size = 9.0;
        self.draw_text.text_style.font_size = 9.0;
        if let Some(pos) = self.last_cursor_screen_pos {
            self.keep_cursor_in_view = KeepCursorInView::FontResize(pos);
        }
    }

    pub fn decrease_font_size(&mut self) {
        if self.draw_text.text_style.font_size > 3.0 {
            self.draw_text.text_style.font_size -= 1.0;
            self.draw_gutter.text_style.font_size = self.draw_text.text_style.font_size;
            if let Some(pos) = self.last_cursor_screen_pos {
                self.keep_cursor_in_view = KeepCursorInView::FontResize(pos);
            }
        }
    }

    pub fn increase_font_size(&mut self) {
        if self.draw_text.text_style.font_size < 20.0 {
            self.draw_text.text_style.font_size += 1.0;
            self.draw_gutter.text_style.font_size = self.draw_text.text_style.font_size;
            if let Some(pos) = self.last_cursor_screen_pos {
                self.keep_cursor_in_view = KeepCursorInView::FontResize(pos);
            }
        }
    }

    pub fn reset_cursor_blinker(&mut self, cx: &mut Cx) {
        self.animator_cut(cx, id!(blink.off));
        cx.stop_timer(self.blink_timer);
        self.blink_timer = cx.start_timeout(self.blink_speed)
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
        session: &mut Session,
    ) -> Vec<CodeEditorAction> {
        let mut actions = Vec::new();
        
        self.animator_handle_event(cx, event);

        session.handle_changes();

        if self.scroll_bars.handle_event(cx, event, scope).len()>0{
            self.redraw(cx);
        };
        
        if self.blink_timer.is_event(event).is_some() {
            if self.animator_in_state(cx, id!(blink.off)) {
                self.animator_play(cx, id!(blink.on));
            } else {
                self.animator_play(cx, id!(blink.off));
            }
            self.blink_timer = cx.start_timeout(self.blink_speed)
        }
        let mut keyboard_moved_cursor = false;
        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                is_repeat: false,
                ..
            }) => {
                session.fold();
                if !self.keep_cursor_in_view.is_locked() {
                    self.keep_cursor_in_view = KeepCursorInView::LockStart;
                }
                self.redraw(cx);
            }
            Hit::KeyUp(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                session.unfold();
                if !self.keep_cursor_in_view.is_locked() {
                    self.keep_cursor_in_view = KeepCursorInView::LockStart;
                }
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Minus,
                modifiers: KeyModifiers { control, logo, .. },
                ..
            }) => {
                if control || logo {
                    self.decrease_font_size();
                    self.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Key0,
                modifiers: KeyModifiers { control, logo, .. },
                ..
            }) => {
                if control || logo {
                    self.reset_font_size();
                    self.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Equals,
                modifiers: KeyModifiers { control, logo, .. },
                ..
            }) => {
                if control || logo {
                    self.increase_font_size();
                    self.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyW,
                modifiers: KeyModifiers { control, logo, .. },
                ..
            }) => {
                if control || logo {
                    self.word_wrap = !self.word_wrap;
                    self.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers: KeyModifiers {control, logo, ..},
                ..
            }) => {
                if control || logo {
                    //session.select_all();
                    self.redraw(cx);
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers:
                    KeyModifiers {
                        shift,
                        control,
                        logo,
                        ..
                    },
                ..
            }) => {
                if control || logo {
                    //session.move_to_start_of_line(!shift);
                } else {
                    session.move_left(!shift);
                }
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers:
                    KeyModifiers {
                        shift,
                        control,
                        logo,
                        ..
                    },
                ..
            }) => {
                if control || logo {
                    //session.move_to_end_of_line(!shift);
                } else {
                    session.move_right(!shift);
                }

                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_up(!shift);
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.move_down(!shift);
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Home,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.home(!shift);
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::End,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                session.end(!shift);
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::PageUp,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                for _ in 0..self.line_end - self.line_start - 3 {
                    session.move_up(!shift);
                }
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::PageDown,
                modifiers: KeyModifiers { shift, .. },
                ..
            }) => {
                for _ in 0..self.line_end - self.line_start - 3 {
                    session.move_down(!shift);
                }
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::TextInput(TextInputEvent {
                ref input,
                was_paste: false,
                ..
            }) if input.len() > 0 => {
                session.insert(input.into());
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::TextInput(TextInputEvent {
                ref input,
                was_paste: true,
                ..
            }) if input.len() > 0 => {
                session.paste(input.into());
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..
            }) => {
                session.enter();
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Tab,
                modifiers: KeyModifiers { shift: false, .. },
                ..
            }) => {
                session.indent();
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Tab,
                modifiers: KeyModifiers { shift: true, .. },
                ..
            }) => {
                session.outdent();
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) => {
                session.delete();
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) => {
                session.backspace();
                self.redraw(cx);
                keyboard_moved_cursor = true;
                actions.push(CodeEditorAction::TextDidChange);
            }
            Hit::TextCopy(ce) => {
                *ce.response.borrow_mut() = Some(session.copy());
                keyboard_moved_cursor = true;
            }
            Hit::TextCut(ce) => {
                *ce.response.borrow_mut() = Some(session.copy());
                session.delete();
                keyboard_moved_cursor = true;
                self.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers:
                    KeyModifiers {
                        logo: true,
                        shift: false,
                        ..
                    },
                ..
            }) => {
                if session.undo() {
                    cx.redraw_all();
                    actions.push(CodeEditorAction::TextDidChange);
                    keyboard_moved_cursor = true;
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers:
                    KeyModifiers {
                        logo: true,
                        shift: true,
                        ..
                    },
                ..
            }) => {
                if session.redo() {
                    self.redraw(cx);
                    actions.push(CodeEditorAction::TextDidChange);
                    keyboard_moved_cursor = true;
                }
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                tap_count,
                modifiers:
                    KeyModifiers {
                        alt: false,
                        shift: false,
                        ..
                    },
                ..
            }) => {
                self.animator_play(cx, id!(focus.on));
                cx.set_key_focus(self.scroll_bars.area());
                let ((cursor, affinity), is_in_gutter) = self.pick(session, abs);
                session.set_selection(
                    cursor,
                    affinity,
                    if is_in_gutter {
                        SelectionMode::Line
                    } else {
                        match tap_count {
                            1 => SelectionMode::Simple,
                            2 => SelectionMode::Word,
                            3 => SelectionMode::Line,
                            _ => SelectionMode::All,
                        }
                    },
                    NewGroup::Yes
                );
                self.reset_cursor_blinker(cx);
                self.keep_cursor_in_view = KeepCursorInView::Always(abs, cx.new_next_frame());
                self.redraw(cx);
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                tap_count,
                modifiers:
                    KeyModifiers {
                        alt: true,
                        shift: false,
                        ..
                    },
                ..
            }) => {
                self.animator_play(cx, id!(focus.on));
                cx.set_key_focus(self.scroll_bars.area());
                let ((cursor, affinity), is_in_gutter) = self.pick(session, abs);
                session.add_selection(
                    cursor,
                    affinity,
                    if is_in_gutter {
                        SelectionMode::Line
                    } else {
                        match tap_count {
                            1 => SelectionMode::Simple,
                            2 => SelectionMode::Word,
                            3 => SelectionMode::Line,
                            _ => SelectionMode::All,
                        }
                    },
                );
                self.reset_cursor_blinker(cx);
                self.keep_cursor_in_view = KeepCursorInView::Always(abs, cx.new_next_frame());
                self.redraw(cx);
            }
            Hit::FingerUp(_) => {
                self.reset_cursor_blinker(cx);
                self.keep_cursor_in_view = KeepCursorInView::Off;
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                modifiers: KeyModifiers { shift: true, .. },
                ..
            })
            | Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                self.reset_cursor_blinker(cx);
                if let KeepCursorInView::Always(old_abs, _) = &mut self.keep_cursor_in_view {
                    *old_abs = abs;
                }
                cx.set_cursor(MouseCursor::Text);
                let ((cursor, affinity), _) = self.pick(session, abs);
                session.move_to(cursor, affinity, NewGroup::Yes);
                // alright how are we going to do scrolling
                self.redraw(cx);
            }
            _ => {}
        }
        if keyboard_moved_cursor {
            self.keep_cursor_in_view = KeepCursorInView::Once;
            self.reset_cursor_blinker(cx);
        }
        if let KeepCursorInView::Always(abs, next) = &mut self.keep_cursor_in_view {
            if next.is_event(event).is_some() {
                *next = cx.new_next_frame();
                let abs = *abs;
                let ((cursor, affinity), _) = self.pick(session, abs);
                session.move_to(cursor, affinity, NewGroup::Yes);
                self.redraw(cx);
            }
        }
        actions
    }

    fn draw_gutter(&mut self, cx: &mut Cx2d, session: &Session) {
        let mut line_index = self.line_start;
        let mut origin_y = session.layout().line(self.line_start).y();
        let mut buf = String::new();
        for element in session
            .layout()
            .block_elements(self.line_start, self.line_end)
        {
            match element {
                BlockElement::Line { line, .. } => {
                    self.draw_gutter.font_scale = line.scale();
                    buf.clear();
                    let _ = write!(buf, "{: >4}", line_index + 1);
                    self.draw_gutter.draw_abs(
                        cx,
                        DVec2 {
                            x: 0.0,
                            y: origin_y,
                        } * self.cell_size
                            + self.gutter_rect.pos
                            + dvec2(
                                (1.0 - line.scale()) * -self.cell_size.x + self.gutter_rect.size.x
                                    - line.scale() * self.gutter_rect.size.x,
                                0.0,
                            ),
                        &buf,
                    );
                    line_index += 1;
                    origin_y += line.height();
                }
                BlockElement::Widget(widget) => {
                    origin_y += widget.height;
                }
            }
        }
    }

    fn draw_text_layer(&mut self, cx: &mut Cx2d, session: &Session) {
        let highlighted_delimiter_positions = session.highlighted_delimiter_positions();
        let mut line_index = self.line_start;
        let mut origin_y = session.layout().line(self.line_start).y();
        for element in session
            .layout()
            .block_elements(self.line_start, self.line_end)
        {
            match element {
                BlockElement::Line { line, .. } => {
                    self.draw_text.font_scale = line.scale();
                    let mut token_iter = line.tokens().iter().copied();
                    let mut token_slot = token_iter.next();
                    let mut row_index = 0;
                    let mut byte_index = 0;
                    let mut column_index = 0;
                    for element in line.wrapped_elements() {
                        match element {
                            WrappedElement::Text {
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
                                        TokenKind::Comment => self.token_colors.comment,
                                        TokenKind::Constant => self.token_colors.constant,
                                        TokenKind::Delimiter => self.token_colors.delimiter,
                                        TokenKind::Identifier => self.token_colors.identifier,
                                        TokenKind::LoopKeyword => self.token_colors.loop_keyword,
                                        TokenKind::Number => self.token_colors.number,
                                        TokenKind::OtherKeyword => self.token_colors.other_keyword,
                                        TokenKind::Punctuator => self.token_colors.punctuator,
                                        TokenKind::String => self.token_colors.string,
                                        TokenKind::Function => self.token_colors.function,
                                        TokenKind::Typename => self.token_colors.typename,
                                        TokenKind::Whitespace => self.token_colors.whitespace,
                                    };
                                    self.draw_text.outline = 0.0;
                                    if let TokenKind::Delimiter = token.kind {
                                        if highlighted_delimiter_positions.contains(&Position {
                                            line_index,
                                            byte_index,
                                        }) {
                                            self.draw_text.outline = 1.0;
                                            self.draw_text.color =
                                                self.token_colors.delimiter_highlight
                                        }
                                    }
                                    for grapheme in text_0.graphemes() {
                                        let (x, y) = line
                                            .grid_to_normalized_position(row_index, column_index);
                                        self.draw_text.draw_abs(
                                            cx,
                                            DVec2 { x, y: origin_y + y } * self.cell_size
                                                + self.viewport_rect.pos,
                                            grapheme,
                                        );
                                        byte_index += grapheme.len();
                                        column_index += grapheme.column_count();
                                    }
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                let (x, y) =
                                    line.grid_to_normalized_position(row_index, column_index);
                                self.draw_text.draw_abs(
                                    cx,
                                    DVec2 { x, y: origin_y + y } * self.cell_size
                                        + self.viewport_rect.pos,
                                    text,
                                );
                                column_index += text.column_count();
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                column_index = line.wrap_indent_column_count();
                                row_index += 1;
                            }
                        }
                    }
                    line_index += 1;
                    origin_y += line.height();
                }
                BlockElement::Widget(widget) => {
                    origin_y += widget.height;
                }
            }
        }
    }

    fn draw_indent_guide_layer(&mut self, cx: &mut Cx2d<'_>, session: &Session) {
        let mut origin_y = session.layout().line(self.line_start).y();
        for element in session
            .layout()
            .block_elements(self.line_start, self.line_end)
        {
            let Settings {
                tab_column_count, ..
            } = **session.settings();
            match element {
                BlockElement::Line { line, .. } => {
                    for row_index in 0..line.row_count() {
                        for column_index in
                            (0..line.indent_column_count()).step_by(tab_column_count)
                        {
                            let (x, y) = line.grid_to_normalized_position(row_index, column_index);
                            self.draw_indent_guide.draw_abs(
                                cx,
                                Rect {
                                    pos: DVec2 { x, y: origin_y + y } * self.cell_size
                                        + self.viewport_rect.pos,
                                    size: DVec2 {
                                        x: 2.0,
                                        y: line.scale() * self.cell_size.y,
                                    },
                                },
                            );
                        }
                    }
                    origin_y += line.height();
                }
                BlockElement::Widget(widget) => {
                    origin_y += widget.height;
                }
            }
        }
    }

    fn draw_decoration_layer(&mut self, cx: &mut Cx2d<'_>, session: &Session) {
        let mut active_decoration = None;
        let decorations = session.document().decorations();
        let mut decorations = decorations.iter();
        while decorations.as_slice().first().map_or(false, |decoration| {
            decoration.end().line_index < self.line_start
        }) {
            decorations.next().unwrap();
        }
        if decorations.as_slice().first().map_or(false, |decoration| {
            decoration.start().line_index < self.line_start
        }) {
            active_decoration = Some(ActiveDecoration {
                decoration: *decorations.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawDecorationLayer {
            code_editor: self,
            active_decoration,
            decorations,
        }
        .draw_decoration_layer(cx, session)
    }

    fn draw_selection_layer(&mut self, cx: &mut Cx2d<'_>, session: &Session) {
        let mut active_selection = None;
        let selections = session.selections();
        let mut selections = selections.iter();
        while selections.as_slice().first().map_or(false, |selection| {
            selection.end().line_index < self.line_start
        }) {
            selections.next().unwrap();
        }
        if selections.as_slice().first().map_or(false, |selection| {
            selection.start().line_index < self.line_start
        }) {
            active_selection = Some(ActiveSelection {
                selection: *selections.next().unwrap(),
                start_x: 0.0,
            });
        }
        DrawSelectionLayer {
            code_editor: self,
            active_selection,
            selections,
        }
        .draw_selection_layer(cx, session)
    }

    fn pick(&self, session: &Session, position: DVec2) -> ((Position, Affinity), bool) {
        let position = (position - self.viewport_rect.pos) / self.cell_size;
        if position.y < 0.0 {
            return (
                (
                    Position {
                        line_index: 0,
                        byte_index: 0,
                    },
                    Affinity::Before,
                ),
                false,
            );
        }
        let layout = session.layout();
        if position.y > session.layout().height() {
            let lines = layout.as_text().as_lines();
            return (
                (
                    Position {
                        line_index: lines.len() - 1,
                        byte_index: lines[lines.len() - 1].len(),
                    },
                    Affinity::After,
                ),
                false,
            );
        }
        let mut line_index = layout.find_first_line_ending_after_y(position.y);
        let mut origin_y = layout.line(line_index).y();
        for block in layout.block_elements(line_index, line_index + 1) {
            match block {
                BlockElement::Line {
                    is_inlay: false,
                    line,
                } => {
                    let mut byte_index = 0;
                    let mut row_index = 0;
                    let mut column_index = 0;
                    for element in line.wrapped_elements() {
                        match element {
                            WrappedElement::Text {
                                is_inlay: false,
                                text,
                            } => {
                                for grapheme in text.graphemes() {
                                    let (start_x, y) =
                                        line.grid_to_normalized_position(row_index, column_index);
                                    let start_y = origin_y + y;
                                    let (end_x, _) = line.grid_to_normalized_position(
                                        row_index,
                                        column_index + grapheme.column_count(),
                                    );
                                    let end_y = start_y + line.scale();
                                    if (start_y..=end_y).contains(&position.y) {
                                        let mid_x = (start_x + end_x) / 2.0;
                                        if (start_x..=mid_x).contains(&position.x) {
                                            return (
                                                (
                                                    Position {
                                                        line_index,
                                                        byte_index,
                                                    },
                                                    Affinity::After,
                                                ),
                                                false,
                                            );
                                        }
                                        if (mid_x..=end_x).contains(&position.x) {
                                            return (
                                                (
                                                    Position {
                                                        line_index,
                                                        byte_index: byte_index + grapheme.len(),
                                                    },
                                                    Affinity::Before,
                                                ),
                                                false,
                                            );
                                        }
                                    }
                                    byte_index += grapheme.len();
                                    column_index += grapheme.column_count();
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                let (start_x, y) =
                                    line.grid_to_normalized_position(row_index, column_index);
                                let start_y = origin_y + y;
                                let (end_x, _) = line.grid_to_normalized_position(
                                    row_index,
                                    column_index + text.column_count(),
                                );
                                let end_y = origin_y + line.scale();
                                if (start_y..=end_y).contains(&position.y)
                                    && (start_x..=end_x).contains(&position.x)
                                {
                                    return (
                                        (
                                            Position {
                                                line_index,
                                                byte_index,
                                            },
                                            Affinity::Before,
                                        ),
                                        false,
                                    );
                                }
                                column_index += text.column_count();
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                let (_, y) =
                                    line.grid_to_normalized_position(row_index, column_index);
                                let start_y = origin_y + y;
                                let end_y = start_y + line.scale();
                                if (start_y..=end_y).contains(&position.y) {
                                    return if position.x < 0.0 {
                                        (
                                            (
                                                Position {
                                                    line_index,
                                                    byte_index: 0,
                                                },
                                                Affinity::Before,
                                            ),
                                            true,
                                        )
                                    } else {
                                        (
                                            (
                                                Position {
                                                    line_index,
                                                    byte_index,
                                                },
                                                Affinity::Before,
                                            ),
                                            false,
                                        )
                                    };
                                }
                                column_index = line.wrap_indent_column_count();
                                row_index += 1;
                            }
                        }
                    }
                    let (_, y) = line.grid_to_normalized_position(row_index, column_index);
                    let start_y = origin_y + y;
                    let end_y = start_y + line.scale();
                    if (start_y..=end_y).contains(&position.y) {
                        return if position.x < 0.0 {
                            (
                                (
                                    Position {
                                        line_index,
                                        byte_index: 0,
                                    },
                                    Affinity::Before,
                                ),
                                true,
                            )
                        } else {
                            (
                                (
                                    Position {
                                        line_index,
                                        byte_index,
                                    },
                                    Affinity::Before,
                                ),
                                false,
                            )
                        };
                    }
                    line_index += 1;
                    origin_y += line.height();
                }
                BlockElement::Line {
                    is_inlay: true,
                    line,
                } => {
                    let start_y = origin_y;
                    let end_y = start_y + line.height();
                    if (start_y..=end_y).contains(&position.y) {
                        return (
                            (
                                Position {
                                    line_index,
                                    byte_index: 0,
                                },
                                Affinity::Before,
                            ),
                            false,
                        );
                    }
                    origin_y += line.height();
                }
                BlockElement::Widget(widget) => {
                    origin_y += widget.height;
                }
            }
        }
        panic!()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, DefaultNone)]
pub enum CodeEditorAction {
    TextDidChange,
    None
}

struct DrawDecorationLayer<'a> {
    code_editor: &'a mut CodeEditor,
    active_decoration: Option<ActiveDecoration>,
    decorations: Iter<'a, Decoration>,
}

impl<'a> DrawDecorationLayer<'a> {
    fn draw_decoration_layer(&mut self, cx: &mut Cx2d, session: &Session) {
        let mut line_index = self.code_editor.line_start;
        let mut origin_y = session.layout().line(line_index).y();
        for block in session
            .layout()
            .block_elements(self.code_editor.line_start, self.code_editor.line_end)
        {
            match block {
                BlockElement::Line {
                    is_inlay: false,
                    line,
                } => {
                    let mut byte_index = 0;
                    let mut row_index = 0;
                    let mut column_index = 0;
                    self.handle_event(
                        cx,
                        line_index,
                        line,
                        byte_index,
                        Affinity::Before,
                        origin_y,
                        row_index,
                        column_index,
                    );
                    for element in line.wrapped_elements() {
                        match element {
                            WrappedElement::Text {
                                is_inlay: false,
                                text,
                            } => {
                                for grapheme in text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line_index,
                                        line,
                                        byte_index,
                                        Affinity::After,
                                        origin_y,
                                        row_index,
                                        column_index,
                                    );
                                    byte_index += grapheme.len();
                                    column_index += grapheme.column_count();
                                    self.handle_event(
                                        cx,
                                        line_index,
                                        line,
                                        byte_index,
                                        Affinity::Before,
                                        origin_y,
                                        row_index,
                                        column_index,
                                    );
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                column_index += text.column_count();
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                if self.active_decoration.is_some() {
                                    self.draw_decoration(
                                        cx,
                                        line,
                                        origin_y,
                                        row_index,
                                        column_index,
                                    );
                                }
                                column_index = line.wrap_indent_column_count();
                                row_index += 1;
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line_index,
                        line,
                        byte_index,
                        Affinity::After,
                        origin_y,
                        row_index,
                        column_index,
                    );
                    if self.active_decoration.is_some() {
                        self.draw_decoration(cx, line, origin_y, row_index, column_index);
                    }
                    line_index += 1;
                    origin_y += line.height();
                }
                BlockElement::Line {
                    is_inlay: true,
                    line,
                } => {
                    origin_y += line.height();
                }
                BlockElement::Widget(widget) => {
                    origin_y += widget.height;
                }
            }
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line_index: usize,
        line: Line<'_>,
        byte_index: usize,
        affinity: Affinity,
        origin_y: f64,
        row_index: usize,
        column_index: usize,
    ) {
        let position = Position {
            line_index,
            byte_index,
        };
        if self.active_decoration.as_ref().map_or(false, |decoration| {
            decoration.decoration.end() == position && affinity == Affinity::Before
        }) {
            self.draw_decoration(cx, line, origin_y, row_index, column_index);
            self.active_decoration = None;
        }
        if self
            .decorations
            .as_slice()
            .first()
            .map_or(false, |decoration| {
                decoration.start() == position && affinity == Affinity::After
            })
        {
            let decoration = *self.decorations.next().unwrap();
            if !decoration.is_empty() {
                let (start_x, _) = line.grid_to_normalized_position(row_index, column_index);
                self.active_decoration = Some(ActiveDecoration {
                    decoration,
                    start_x,
                });
            }
        }
    }

    fn draw_decoration(
        &mut self,
        cx: &mut Cx2d,
        line: Line<'_>,
        origin_y: f64,
        row_index: usize,
        column_index: usize,
    ) {
        let start_x = mem::take(&mut self.active_decoration.as_mut().unwrap().start_x);
        let (x, y) = line.grid_to_normalized_position(row_index, column_index);
        self.code_editor.draw_decoration.color =
            match self.active_decoration.as_mut().unwrap().decoration.ty {
                DecorationType::Warning => self.code_editor.token_colors.warning_decoration,
                DecorationType::Error => self.code_editor.token_colors.error_decoration,
            };

        self.code_editor.draw_decoration.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: start_x,
                    y: origin_y + y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }
}

struct ActiveDecoration {
    decoration: Decoration,
    start_x: f64,
}

struct DrawSelectionLayer<'a> {
    code_editor: &'a mut CodeEditor,
    active_selection: Option<ActiveSelection>,
    selections: Iter<'a, Selection>,
}

impl<'a> DrawSelectionLayer<'a> {
    fn draw_selection_layer(&mut self, cx: &mut Cx2d, session: &Session) {
        let mut line_index = self.code_editor.line_start;
        let mut origin_y = session.layout().line(line_index).y();
        for block in session
            .layout()
            .block_elements(self.code_editor.line_start, self.code_editor.line_end)
        {
            match block {
                BlockElement::Line {
                    is_inlay: false,
                    line,
                } => {
                    let mut byte_index = 0;
                    let mut row_index = 0;
                    let mut column_index = 0;
                    self.handle_event(
                        cx,
                        line_index,
                        line,
                        byte_index,
                        Affinity::Before,
                        origin_y,
                        row_index,
                        column_index,
                    );
                    for element in line.wrapped_elements() {
                        match element {
                            WrappedElement::Text {
                                is_inlay: false,
                                text,
                            } => {
                                for grapheme in text.graphemes() {
                                    self.handle_event(
                                        cx,
                                        line_index,
                                        line,
                                        byte_index,
                                        Affinity::After,
                                        origin_y,
                                        row_index,
                                        column_index,
                                    );
                                    byte_index += grapheme.len();
                                    column_index += grapheme.column_count();
                                    self.handle_event(
                                        cx,
                                        line_index,
                                        line,
                                        byte_index,
                                        Affinity::Before,
                                        origin_y,
                                        row_index,
                                        column_index,
                                    );
                                }
                            }
                            WrappedElement::Text {
                                is_inlay: true,
                                text,
                            } => {
                                column_index += text.column_count();
                            }
                            WrappedElement::Widget(widget) => {
                                column_index += widget.column_count;
                            }
                            WrappedElement::Wrap => {
                                if self.active_selection.is_some() {
                                    self.draw_selection(
                                        cx,
                                        line,
                                        origin_y,
                                        row_index,
                                        column_index,
                                    );
                                }
                                column_index = line.wrap_indent_column_count();
                                row_index += 1;
                            }
                        }
                    }
                    self.handle_event(
                        cx,
                        line_index,
                        line,
                        byte_index,
                        Affinity::After,
                        origin_y,
                        row_index,
                        column_index,
                    );
                    column_index += 1;
                    if self.active_selection.is_some() {
                        self.draw_selection(cx, line, origin_y, row_index, column_index);
                    }
                    line_index += 1;
                    origin_y += line.height();
                }
                BlockElement::Line {
                    is_inlay: true,
                    line,
                } => {
                    origin_y += line.height();
                }
                BlockElement::Widget(widget) => {
                    origin_y += widget.height;
                }
            }
        }
        if self.active_selection.is_some() {
            self.code_editor.draw_selection.end(cx);
        }
    }

    fn handle_event(
        &mut self,
        cx: &mut Cx2d,
        line_index: usize,
        line: Line<'_>,
        byte_index: usize,
        affinity: Affinity,
        origin_y: f64,
        row_index: usize,
        column_index: usize,
    ) {
        let position = Position {
            line_index,
            byte_index,
        };
        if self.active_selection.as_ref().map_or(false, |selection| {
            selection.selection.end() == position && selection.selection.end_affinity() == affinity
        }) {
            self.draw_selection(cx, line, origin_y, row_index, column_index);
            self.code_editor.draw_selection.end(cx);
            let selection = self.active_selection.take().unwrap().selection;
            if selection.cursor.position == position && selection.cursor.affinity == affinity {
                self.draw_cursor(cx, line, origin_y, row_index, column_index);
            }
        }
        if self
            .selections
            .as_slice()
            .first()
            .map_or(false, |selection| {
                selection.start() == position && selection.start_affinity() == affinity
            })
        {
            let selection = *self.selections.next().unwrap();
            if selection.cursor.position == position && selection.cursor.affinity == affinity {
                self.draw_cursor_bg(cx, line, origin_y, row_index, column_index);
                self.draw_cursor(cx, line, origin_y, row_index, column_index);
            }
            if !selection.is_empty() {
                let (start_x, _) = line.grid_to_normalized_position(row_index, column_index);
                self.active_selection = Some(ActiveSelection { selection, start_x });
            }
            self.code_editor.draw_selection.begin();
        }
    }

    fn draw_selection(
        &mut self,
        cx: &mut Cx2d,
        line: Line<'_>,
        origin_y: f64,
        row_index: usize,
        column_index: usize,
    ) {
        let start_x = mem::take(&mut self.active_selection.as_mut().unwrap().start_x);
        let (x, y) = line.grid_to_normalized_position(row_index, column_index);
        if start_x == x {
            return;
        }
        self.code_editor.draw_selection.draw(
            cx,
            Rect {
                pos: DVec2 {
                    x: start_x,
                    y: origin_y + y,
                } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: x - start_x,
                    y: line.scale(),
                } * self.code_editor.cell_size,
            },
        );
    }

    fn draw_cursor(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: Line<'_>,
        origin_y: f64,
        row_index: usize,
        column_index: usize,
    ) {
        let (x, y) = line.grid_to_normalized_position(row_index, column_index);

        self.code_editor.draw_cursor.draw_abs(
            cx,
            Rect {
                pos: DVec2 { x, y: origin_y + y } * self.code_editor.cell_size
                    + self.code_editor.viewport_rect.pos,
                size: DVec2 {
                    x: 2.0,
                    y: line.scale() * self.code_editor.cell_size.y,
                },
            },
        );
    }

    fn draw_cursor_bg(
        &mut self,
        cx: &mut Cx2d<'_>,
        line: Line<'_>,
        origin_y: f64,
        row_index: usize,
        column_index: usize,
    ) {
        let (_x, y) = line.grid_to_normalized_position(row_index, column_index);

        self.code_editor.draw_cursor_bg.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: self.code_editor.unscrolled_rect.pos.x,
                    y: (origin_y + y) * self.code_editor.cell_size.y
                        + self.code_editor.viewport_rect.pos.y,
                },
                size: DVec2 {
                    x: self.code_editor.unscrolled_rect.size.x,
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

#[derive(Live, LiveHook, LiveRegister)]
struct TokenColors {
    #[live]
    unknown: Vec4,
    #[live]
    branch_keyword: Vec4,
    #[live]
    comment: Vec4,
    #[live]
    constant: Vec4,
    #[live]
    delimiter: Vec4,
    #[live]
    delimiter_highlight: Vec4,
    #[live]
    identifier: Vec4,
    #[live]
    loop_keyword: Vec4,
    #[live]
    number: Vec4,
    #[live]
    other_keyword: Vec4,
    #[live]
    function: Vec4,
    #[live]
    punctuator: Vec4,
    #[live]
    string: Vec4,
    #[live]
    typename: Vec4,
    #[live]
    whitespace: Vec4,
    #[live]
    error_decoration: Vec4,
    #[live]
    warning_decoration: Vec4,
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawIndentGuide {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4,
}

#[derive(Live, LiveHook, LiveRegister)]
struct DrawDecoration {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4,
}

#[derive(Live, LiveHook, LiveRegister)]
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
