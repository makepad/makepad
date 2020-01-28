use makepad_render::*;
use crate::scrollview::*;
use crate::textbuffer::*;
use crate::textcursor::*;
use crate::widgetstyle::*;
use crate::scrollshadow::*;

#[derive(Clone)]
pub struct TextEditor {
    pub view: ScrollView,
    //pub view_layout: Layout,
    pub bg: Quad,
    pub gutter_bg: Quad,
    pub cursor: Quad,
    pub selection: Quad,
    pub token_highlight: Quad,
    //pub select_highlight: Quad,
    pub cursor_row: Quad,
    pub paren_pair: Quad,
    pub indent_lines: Quad,
    pub shadow: ScrollShadow,
    pub message_marker: Quad,
    pub search_marker: Quad,
    pub text: Text,
    pub line_number_text: Text,
    pub cursors: TextCursorSet,

    //pub base_font_size: f32,
    pub open_font_scale: f32,
    pub folded_font_scale: f32,
    pub line_number_width: f32,
    pub line_number_click_margin: f32,
    pub draw_line_numbers: bool,
    pub top_padding: f32,
    //pub colors: CodeEditorColors,
    pub cursor_blink_speed: f64,
    pub _undo_id: u64,
    pub highlight_area_on: bool,

    pub mark_unmatched_parens: bool,
    pub draw_cursor_row: bool,
    pub search_markers_bypass: Vec<TextCursor>,
    pub folding_depth: usize,
    pub colors: CodeEditorColors,

    pub read_only: bool,
    pub multiline: bool,

    pub line_number_offset: usize,

    //pub _bg_area: Area,
    pub _scroll_pos_on_load: Option<Vec2>,
    pub _set_key_focus_on_load: bool,
    pub _set_last_cursor: Option<((usize, usize),bool)>,
    pub _view_area: Area,
    pub _highlight_area: Area,
    pub _text_inst: Option<AlignedInstance>,
    pub _line_number_inst: Option<AlignedInstance>,
    pub _line_number_chunk: Vec<char>,
    pub _text_area: Area,
    pub _scroll_pos: Vec2,
    pub _last_finger_move: Option<Vec2>,
    pub _paren_stack: Vec<ParenItem>,
    pub _indent_stack: Vec<(Color, f32)>,
    pub _indent_id_alloc: f32,
    pub _indent_line_inst: Option<InstanceArea>,
    pub _bg_inst: Option<InstanceArea>,
    pub _last_indent_color: Color,

    pub _line_geometry: Vec<LineGeom>,
    pub _anim_select: Vec<AnimSelect>,
    pub _visible_lines: usize,

    pub _select_scroll: Option<SelectScroll>,
    pub _grid_select_corner: Option<TextPos>,
    pub _is_row_select: bool,
    //pub _line_chunk: Vec<(f32, char)>,

    //pub _highlight_selection: Vec<char>,
    //pub _highlight_token: Vec<char>,
    pub _last_cursor_pos: TextPos,

    pub _anim_font_scale: f32,
    pub _line_largest_font: f32,
    pub _anim_folding: AnimFolding,

    pub _monospace_size: Vec2,
    pub _monospace_base: Vec2,

    pub _tokens_on_line: usize,
    pub _line_was_folded: bool,
    //pub _line_was_visible: bool,
    pub _final_fill_height: f32,
    pub _draw_cursors: DrawCursors,
    pub _draw_search: DrawCursors,
    pub _draw_messages: DrawCursors,

    pub _cursor_blink_timer: Timer,
    pub _cursor_blink_flipflop: f32,
    pub _cursor_area: Area,
    pub _highlight_visibility: f32,

    pub _last_tabs: usize,
    pub _newline_tabs: usize,

    pub _last_lag_mutation_id: u32
}

#[derive(Clone, PartialEq)]
pub enum TextEditorEvent {
    None,
    AutoFormat,
    LagChange,
    Change,
    KeyFocus,
    KeyFocusLost,
    Escape,
    Search(String),
    Decl(String)
}

#[derive(Default, Clone)]
pub struct CodeEditorColors {
    indent_line_unknown: Color,
    indent_line_fn: Color,
    indent_line_typedef: Color,
    indent_line_looping: Color,
    indent_line_flow: Color,
    paren_pair_match: Color,
    paren_pair_fail: Color,
    message_marker_error: Color,
    message_marker_warning: Color,
    message_marker_log: Color,
    line_number_normal: Color,
    line_number_highlight: Color,
    whitespace: Color,
    keyword: Color,
    flow: Color,
    looping: Color,
    identifier: Color,
    call: Color,
    type_name: Color,
    theme_name: Color,
    string: Color,
    number: Color,
    comment: Color,
    doc_comment: Color,
    paren_d1: Color,
    paren_d2: Color,
    operator: Color,
    delimiter: Color,
    unexpected: Color,
    warning: Color,
    error: Color,
    defocus: Color,
}

impl TextEditor {

    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            read_only: false,
            multiline: true,
            cursors: TextCursorSet::new(),
            indent_lines: Quad {
                z: 0.001,
                ..Quad::proto(cx)
            },
            view: ScrollView::proto(cx),
            bg: Quad ::proto(cx),
            shadow: ScrollShadow {
                z: 10.,
                ..ScrollShadow::proto(cx)
            },
            gutter_bg: Quad {
                z: 9.0,
                ..Quad::proto(cx)
            },
            colors: CodeEditorColors::default(),
            selection: Quad {
                z: 0.,
                ..Quad::proto(cx)
            },
            token_highlight: Quad::proto(cx),
            cursor: Quad::proto(cx),
            cursor_row: Quad::proto(cx),
            paren_pair: Quad::proto(cx),
            message_marker: Quad::proto(cx),
            search_marker: Quad::proto(cx),
            //code_icon: CodeIcon::proto(cx),
            //view_layout: Layout::default(),
            text: Text {
                z: 2.00,
                wrapping: Wrapping::Line,
                ..Text::proto(cx)
            },
            line_number_text: Text {
                z: 9.,
                wrapping: Wrapping::Line,
                ..Text::proto(cx)
            },
            //base_font_size: 8.0,
            open_font_scale: 1.0,
            folded_font_scale: 0.07,
            line_number_width: 45.,
            line_number_click_margin: 10.,
            draw_line_numbers: true,
            cursor_blink_speed: 0.5,
            top_padding: 27.,
            mark_unmatched_parens: true,
            highlight_area_on: true,
            draw_cursor_row: true,
            line_number_offset: 0,
            search_markers_bypass: Vec::new(),
            _scroll_pos_on_load: None,
            _set_key_focus_on_load: false,
            _set_last_cursor: None,
            _monospace_size: Vec2::default(),
            _monospace_base: Vec2::default(),
            _last_finger_move: None,
            _tokens_on_line: 0,
            _line_was_folded: false,
            //_line_was_visible: false,
            _scroll_pos: Vec2::default(),
            _visible_lines: 0,
            _undo_id: 0,


            _line_geometry: Vec::new(),

            _anim_select: Vec::new(),
            _grid_select_corner: None,
            _is_row_select: false,
            _view_area: Area::Empty,
            //_bg_area: Area::Empty,
            _highlight_area: Area::Empty,
            _highlight_visibility: 0.,
            _bg_inst: None,
            _text_inst: None,
            _text_area: Area::Empty,
            _line_number_inst: None,
            _line_number_chunk: Vec::new(),

            _anim_font_scale: 1.0,
            _line_largest_font: 0.,
            _final_fill_height: 0.,
            folding_depth: 2,
            _anim_folding: AnimFolding {
                state: AnimFoldingState::Open,
                focussed_line: 0,
                did_animate: false,
            },
            _select_scroll: None,
            _draw_cursors: DrawCursors::new(),
            _draw_search: DrawCursors::new(),
            _draw_messages: DrawCursors::new(),

            _paren_stack: Vec::new(),
            _indent_stack: Vec::new(),
            _indent_id_alloc: 0.0,
            _indent_line_inst: None,

            //_line_chunk: Vec::new(),
            //_highlight_selection: Vec::new(),
            //_highlight_token: Vec::new(),
            _last_cursor_pos: TextPos::zero(),
            _last_indent_color: Color::default(),

            _cursor_blink_timer: Timer::empty(),
            _cursor_blink_flipflop: 0.,
            _cursor_area: Area::Empty,
            _last_lag_mutation_id: 0,
            _last_tabs: 0,
            _newline_tabs: 0,
        }
    }

    pub fn gutter_width() -> FloatId {uid!()}
    pub fn padding_top() -> FloatId {uid!()}
    pub fn shadow_size() -> FloatId {uid!()}

    pub fn layout_bg() -> LayoutId {uid!()}
    pub fn text_style_editor_text() -> TextStyleId {uid!()}

    pub fn color_bg() -> ColorId {uid!()}
    pub fn color_gutter_bg() -> ColorId {uid!()}

    pub fn color_selection() -> ColorId {uid!()}
    pub fn color_selection_defocus() -> ColorId {uid!()}
    pub fn color_highlight() -> ColorId {uid!()}
    pub fn color_cursor() -> ColorId {uid!()}
    pub fn color_cursor_row() -> ColorId {uid!()}

    pub fn color_indent_line_unknown() -> ColorId {uid!()}
    pub fn color_indent_line_fn() -> ColorId {uid!()}
    pub fn color_indent_line_typedef() -> ColorId {uid!()}
    pub fn color_indent_line_looping() -> ColorId {uid!()}
    pub fn color_indent_line_flow() -> ColorId {uid!()}
    pub fn color_paren_pair_match() -> ColorId {uid!()}
    pub fn color_paren_pair_fail() -> ColorId {uid!()}
    pub fn color_message_marker_error() -> ColorId {uid!()}
    pub fn color_message_marker_warning() -> ColorId {uid!()}
    pub fn color_message_marker_log() -> ColorId {uid!()}
    pub fn color_search_marker() -> ColorId {uid!()}
    pub fn color_line_number_normal() -> ColorId {uid!()}
    pub fn color_line_number_highlight() -> ColorId {uid!()}

    pub fn color_whitespace() -> ColorId {uid!()}
    pub fn color_keyword() -> ColorId {uid!()}
    pub fn color_flow() -> ColorId {uid!()}
    pub fn color_looping() -> ColorId {uid!()}
    pub fn color_identifier() -> ColorId {uid!()}
    pub fn color_call() -> ColorId {uid!()}
    pub fn color_type_name() -> ColorId {uid!()}
    pub fn color_theme_name() -> ColorId {uid!()}
    pub fn color_string() -> ColorId {uid!()}
    pub fn color_number() -> ColorId {uid!()}
    pub fn color_comment() -> ColorId {uid!()}
    pub fn color_doc_comment() -> ColorId {uid!()}
    pub fn color_paren_d1() -> ColorId {uid!()}
    pub fn color_paren_d2() -> ColorId {uid!()}
    pub fn color_operator() -> ColorId {uid!()}
    pub fn color_delimiter() -> ColorId {uid!()}
    pub fn color_unexpected() -> ColorId {uid!()}
    pub fn color_warning() -> ColorId {uid!()}
    pub fn color_error() -> ColorId {uid!()}
    pub fn color_defocus() -> ColorId {uid!()}

    pub fn shader_bg() -> ShaderId {uid!()}
    pub fn shader_indent_lines() -> ShaderId {uid!()}
    pub fn shader_cursor() -> ShaderId {uid!()}
    pub fn shader_selection() -> ShaderId {uid!()}
    pub fn shader_paren_pair() -> ShaderId {uid!()}
    pub fn shader_cursor_row() -> ShaderId {uid!()}
    //pub fn shader_token_highlight() -> ShaderId {uid!()}
    pub fn shader_search_marker() -> ShaderId {uid!()}
    pub fn shader_message_marker() -> ShaderId {uid!()}

    pub fn instance_indent_id() -> InstanceFloat {uid!()}
    pub fn uniform_indent_sel() -> UniformFloat {uid!()}
    pub fn uniform_cursor_blink() -> UniformFloat {uid!()}
    pub fn instance_select_prev_x() -> InstanceFloat {uid!()}
    pub fn instance_select_prev_w() -> InstanceFloat {uid!()}
    pub fn instance_select_next_x() -> InstanceFloat {uid!()}
    pub fn instance_select_next_w() -> InstanceFloat {uid!()}
    pub fn uniform_highlight_visible() -> UniformFloat {uid!()}
    pub fn instance_shadow_dir() -> InstanceFloat {uid!()}

    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        Self::layout_bg().set(cx, Layout::default());
        Self::shadow_size().set(cx, 6.0);
        Self::gutter_width().set(cx, 45.0);
        Self::padding_top().set(cx, 27.);
        Self::text_style_editor_text().set(cx, Theme::text_style_fixed().get(cx));

        Self::shader_indent_lines().set(cx, Quad::def_quad_shader().compose(shader_ast !({
            let indent_id: Self::instance_indent_id();
            // uniforms
            let indent_sel: Self::uniform_indent_sel();
            fn pixel() -> vec4 {
                let col = color;
                let thickness = 0.8 + dpi_dilate * 0.5;
                if indent_id == indent_sel {
                    col *= vec4(1., 1., 1., 1.);
                    thickness *= 1.3;
                }
                else {
                    col *= vec4(0.75, 0.75, 0.75, 0.75);
                }
                df_viewport(pos * vec2(w, h));
                df_move_to(1., -1.);
                df_line_to(1., h + 1.);
                return df_stroke(col, thickness);
            }
        })));

        Self::shader_cursor().set(cx, Quad::def_quad_shader().compose(shader_ast !({
            let blink: Self::uniform_cursor_blink();
            fn pixel() -> vec4 {
                if blink<0.5 {
                    return vec4(color.rgb * color.a, color.a)
                }
                else {
                    return vec4(0., 0., 0., 0.);
                }
            }
        })));

        Self::shader_selection().set(cx, Quad::def_quad_shader().compose(shader_ast !({
            let prev_x: Self::instance_select_prev_x();
            let prev_w: Self::instance_select_prev_w();
            let next_x: Self::instance_select_next_x();
            let next_w: Self::instance_select_next_w();
            const gloopiness: float = 8.;
            const border_radius: float = 2.;

            fn vertex() -> vec4 { // custom vertex shader because we widen the draweable area a bit for the gloopiness
                let shift: vec2 = -draw_scroll.xy;
                let clipped: vec2 = clamp(
                    geom * vec2(w + 16., h) + vec2(x, y) + shift - vec2(8., 0.),
                    draw_clip.xy,
                    draw_clip.zw
                );
                pos = (clipped - shift - vec2(x, y)) / vec2(w, h);
                return camera_projection * (camera_view * (view_transform * vec4(clipped.x, clipped.y, z + draw_zbias, 1.)));
            }

            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                if prev_w > 0. {
                    df_box(prev_x, -h, prev_w, h, border_radius);
                    df_gloop(gloopiness);
                }
                if next_w > 0. {
                    df_box(next_x, h, next_w, h, border_radius);
                    df_gloop(gloopiness);
                }
                //df_shape *= cos(pos.x*8.)+cos(pos.y*16.);
                return df_fill(color);
            }
        })));

        Self::shader_paren_pair().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                //df_rect(0.,0.,w,h);
                //df_rect(0.5,0.5,w-1.,h-1.);
                //return df_stroke(color, 0.75 + dpi_dilate*0.75);
                //df_rect(0.,h-1.-dpi_dilate,w,1.+dpi_dilate);
                df_rect(0., h - 1.5 - dpi_dilate, w, 1.5 + dpi_dilate);
                return df_fill(color);
                //df_rect(0.01,0.,w,h);
                //return df_fill(color);
            }
        })));

        Self::shader_cursor_row().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_rect(0., 0., w, h);
                return df_fill(color);
                /*
                df_move_to(0.,0.5);
                df_line_to(w,0.5);
                df_move_to(0.,h-0.5);
                df_line_to(w,h-0.5);
                returndf_stroke(color,0.75+dpi_dilate*0.75);*/
            }
        })));
        /*
        Self::shader_select_highlight().set(cx,   Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(0.5, 0.5, w - 1., h - 1., 1.);
                return df_fill(color);
            }
        })));*/
        /*
        Self::shader_token_highlight().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            let visible: Self::uniform_highlight_visible();
            fn pixel() -> vec4 {
                if visible<0.5 {
                    return vec4(0., 0., 0., 0.)
                }
                df_viewport(pos * vec2(w, h));
                df_box(0.5, 0.5, w - 1., h - 1., 1.);
                return df_fill(color);
            }
        })));*/

        Self::shader_search_marker().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                let pos2 = vec2(pos.x, pos.y + 0.03 * sin(pos.x * w));
                df_viewport(pos2 * vec2(w, h));
                //df_rect(0.,0.,w,h);
                df_move_to(0., h - 1.);
                df_line_to(w, h - 1.);
                return df_stroke(color("white"), 0.8);
                /*
                df_viewport(pos * vec2(w, h));
                df_box(0.5, 0.5, w - 1., h - 1., 1.);
                return df_fill(color);*/
            }
        })));

        Self::shader_message_marker().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                let pos2 = vec2(pos.x, pos.y + 0.03 * sin(pos.x * w));
                df_viewport(pos2 * vec2(w, h));
                //df_rect(0.,0.,w,h);
                df_move_to(0., h - 1.);
                df_line_to(w, h - 1.);
                return df_stroke(color, 0.8);
            }
        })));

        Self::shader_bg().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                return vec4(color.rgb * color.a, color.a);
            }
        })));
    }

    pub fn apply_style(&mut self, cx: &mut Cx) {
        // copy over colors
        self.colors.indent_line_unknown = Self::color_indent_line_unknown().get(cx);
        self.colors.indent_line_fn = Self::color_indent_line_fn().get(cx);
        self.colors.indent_line_typedef = Self::color_indent_line_typedef().get(cx);
        self.colors.indent_line_looping = Self::color_indent_line_looping().get(cx);
        self.colors.indent_line_flow = Self::color_indent_line_flow().get(cx);
        self.search_marker.color = Self::color_search_marker().get(cx);
        self.colors.paren_pair_match = Self::color_paren_pair_match().get(cx);
        self.colors.paren_pair_fail = Self::color_paren_pair_fail().get(cx);
        self.colors.message_marker_error = Self::color_message_marker_error().get(cx);
        self.colors.message_marker_warning = Self::color_message_marker_warning().get(cx);
        self.colors.message_marker_log = Self::color_message_marker_log().get(cx);
        self.colors.line_number_normal = Self::color_line_number_normal().get(cx);
        self.colors.line_number_highlight = Self::color_line_number_highlight().get(cx);
        self.colors.whitespace = Self::color_whitespace().get(cx);
        self.colors.keyword = Self::color_keyword().get(cx);
        self.colors.flow = Self::color_flow().get(cx);
        self.colors.looping = Self::color_looping().get(cx);
        self.colors.identifier = Self::color_identifier().get(cx);
        self.colors.call = Self::color_call().get(cx);
        self.colors.type_name = Self::color_type_name().get(cx);
        self.colors.theme_name = Self::color_theme_name().get(cx);
        self.colors.string = Self::color_string().get(cx);
        self.colors.number = Self::color_number().get(cx);
        self.colors.comment = Self::color_comment().get(cx);
        self.colors.doc_comment = Self::color_doc_comment().get(cx);
        self.colors.paren_d1 = Self::color_paren_d1().get(cx);
        self.colors.paren_d2 = Self::color_paren_d2().get(cx);
        self.colors.operator = Self::color_operator().get(cx);
        self.colors.delimiter = Self::color_delimiter().get(cx);
        self.colors.unexpected = Self::color_unexpected().get(cx);
        self.colors.warning = Self::color_warning().get(cx);
        self.colors.error = Self::color_error().get(cx);
        self.colors.defocus = Self::color_defocus().get(cx);
        self.bg.color = Self::color_bg().get(cx);
        self.gutter_bg.color = Self::color_gutter_bg().get(cx);

        self.line_number_width = Self::gutter_width().get(cx);
        self.top_padding = Self::padding_top().get(cx);

        self.selection.color = if self.has_key_focus(cx) {
            Self::color_selection().get(cx)
        }else {
            Self::color_selection_defocus().get(cx)
        };
        self.token_highlight.color = Self::color_highlight().get(cx);
        self.cursor.color = Self::color_cursor().get(cx);
        self.cursor_row.color = Self::color_cursor_row().get(cx);
        self.text.text_style = Self::text_style_editor_text().get(cx);
        self.line_number_text.text_style = Self::text_style_editor_text().get(cx);

        self.bg.shader = Self::shader_bg().get(cx);
        self.indent_lines.shader = Self::shader_indent_lines().get(cx);
        self.cursor.shader = Self::shader_cursor().get(cx);
        self.selection.shader = Self::shader_selection().get(cx);
        self.paren_pair.shader = Self::shader_paren_pair().get(cx);
        self.cursor_row.shader = Self::shader_cursor_row().get(cx);
        //self.token_highlight.shader = Self::shader_token_highlight().get(cx);
        self.message_marker.shader = Self::shader_message_marker().get(cx);
        self.search_marker.shader = Self::shader_search_marker().get(cx);
    }
    /*
    fn reset_highlight_visible(&mut self, cx: &mut Cx) {
        self._highlight_visibility = 0.0;
        self._highlight_area.write_uniform_float(cx, Self::uniform_highlight_visible(), self._highlight_visibility);
    }*/

    fn reset_cursor_blinker(&mut self, cx: &mut Cx) {
        cx.stop_timer(&mut self._cursor_blink_timer);
        self._cursor_blink_timer = cx.start_timer(self.cursor_blink_speed * 0.5, false);
        self._cursor_blink_flipflop = 0.;
        self._cursor_area.write_uniform_float(cx, Self::uniform_cursor_blink(), self._cursor_blink_flipflop);
    }

    fn handle_finger_down(&mut self, cx: &mut Cx, fe: &FingerDownEvent, text_buffer: &mut TextBuffer) {
        cx.set_down_mouse_cursor(MouseCursor::Text);
        // give us the focus
        self.set_key_focus(cx);
        self._undo_id += 1;

        let offset;
        //let scroll_pos = self._bg_area.get_scroll_pos(cx);
        if fe.rel.x < self.line_number_width - self.line_number_click_margin {
            offset = self.compute_offset_from_ypos(cx, fe.abs.y, text_buffer, false);
            let range = text_buffer.get_nearest_line_range(offset);
            self.cursors.set_last_clamp_range(range);
            self._is_row_select = true;
        }
        else {
            offset = self.text.find_closest_offset(cx, &self._text_area, fe.abs);
            match fe.tap_count {
                1 => {
                },
                2 => {
                    if let Some((coffset, len)) = TextCursorSet::get_nearest_token_chunk(offset, &text_buffer) {
                        self.cursors.set_last_clamp_range((coffset, len));
                    }
                },
                3 => {
                    if let Some((coffset, len)) = TextCursorSet::get_nearest_token_chunk(offset, &text_buffer) {
                        //self.cursors.set_last_clamp_range((coffset, len));
                        let (start, line_len) = text_buffer.get_nearest_line_range(offset);
                        let mut chunk_offset = coffset;
                        let mut chunk_len = len;
                        if start < chunk_offset {
                            chunk_len += chunk_offset - start;
                            chunk_offset = start;
                            if line_len > chunk_len {
                                chunk_len = line_len;
                            }
                        }
                        self.cursors.set_last_clamp_range((chunk_offset, chunk_len));
                    }
                    else {
                        let range = text_buffer.get_nearest_line_range(offset);
                        self.cursors.set_last_clamp_range(range);
                    }
                },
                _ => {
                    //let range = (0, text_buffer.calc_char_count());
                    //self.cursors.set_last_clamp_range(range);
                }
            }
            // ok so we should scan a range
        }

        if fe.modifiers.shift {
            if fe.modifiers.logo || fe.modifiers.control { // grid select
                let pos = self.compute_grid_text_pos_from_abs(cx, fe.abs);
                self._grid_select_corner = Some(self.cursors.grid_select_corner(pos, text_buffer));
                self.cursors.grid_select(self._grid_select_corner.unwrap(), pos, text_buffer);
                if self.cursors.set.len() == 0 {
                    self.cursors.clear_and_set_last_cursor_head_and_tail(offset, offset, text_buffer);
                }
            }
            else { // simply place selection
                self.cursors.clear_and_set_last_cursor_head(offset, text_buffer);
            }
        }
        else { // cursor drag with possible add
            if fe.modifiers.logo || fe.modifiers.control {
                self.cursors.add_last_cursor_head_and_tail(offset, offset, text_buffer);
            }
            else {
                self.cursors.clear_and_set_last_cursor_head_and_tail(offset, offset, text_buffer);
            }
        }

        self.view.redraw_view_area(cx);
        self._last_finger_move = Some(fe.abs);
        //self.update_highlight(cx, text_buffer);
        self.reset_cursor_blinker(cx);
    }

    fn handle_finger_move(&mut self, cx: &mut Cx, fe: &FingerMoveEvent, text_buffer: &mut TextBuffer) {
        let cursor_moved = if let Some(grid_select_corner) = self._grid_select_corner {
            let pos = self.compute_grid_text_pos_from_abs(cx, fe.abs);
            self.cursors.grid_select(grid_select_corner, pos, text_buffer)
        }
        else if self._is_row_select {
            let offset = self.compute_offset_from_ypos(cx, fe.abs.y, text_buffer, true);
            self.cursors.set_last_cursor_head(offset, text_buffer)
        }
        else {
            let offset = self.text.find_closest_offset(cx, &self._text_area, fe.abs);
            self.cursors.set_last_cursor_head(offset, text_buffer)
        };
        self._last_finger_move = Some(fe.abs);
        // determine selection drag scroll dynamics
        let repaint_scroll = self.check_select_scroll_dynamics(&fe);
        //if cursor_moved {
        //     self.update_highlight(cx, text_buffer);
        //};
        if repaint_scroll || cursor_moved {
            self.view.redraw_view_area(cx);
        }
        if cursor_moved {
            self.reset_cursor_blinker(cx);
        }
    }

    fn handle_finger_up(&mut self, cx: &mut Cx, _fe: &FingerUpEvent, _text_buffer: &mut TextBuffer) {
        self.cursors.clear_last_clamp_range();
        self._select_scroll = None;
        self._last_finger_move = None;
        self._grid_select_corner = None;
        self._is_row_select = false;
        //self.update_highlight(cx, text_buffer);
        self.reset_cursor_blinker(cx);
    }

    fn handle_key_down(&mut self, cx: &mut Cx, ke: &KeyEvent, text_buffer: &mut TextBuffer) {
        let cursor_moved = match ke.key_code {
            KeyCode::KeyE =>{
                if ke.modifiers.logo || ke.modifiers.control {
                    let pos = self.cursors.get_last_cursor_head();
                    let mut moved = false;
                    for result in text_buffer.markers.search_cursors.iter().rev(){
                        if result.head < pos{
                            if ke.modifiers.shift{
                                self.cursors.add_last_cursor_head_and_tail(result.head, result.tail, text_buffer);
                            }
                            else{
                                self.cursors.set_last_cursor_head_and_tail(result.head, result.tail, text_buffer);
                            }
                            moved = true;
                            break;
                        }
                    }

                    moved
                }
                else{
                    false
                }
            }
            KeyCode::KeyD =>{
                if ke.modifiers.logo || ke.modifiers.control {
                    let pos = self.cursors.get_last_cursor_head();
                    let mut moved = false;
                    for result in text_buffer.markers.search_cursors.iter(){
                        if result.tail > pos{
                            if ke.modifiers.shift{
                                self.cursors.add_last_cursor_head_and_tail(result.head, result.tail, text_buffer);
                            }
                            else{
                                self.cursors.set_last_cursor_head_and_tail(result.head, result.tail, text_buffer);
                            }
                            moved = true;
                            break;
                        }
                    }
                    moved
                }
                else{
                    false
                }
            }
            KeyCode::ArrowUp => {
                if !self.multiline {
                    false
                }
                else if ke.modifiers.logo || ke.modifiers.control {
                    false
                }
                else {
                    if self._anim_folding.state.is_folded() && self.cursors.set.len() == 1 {
                        // compute the nearest nonfolded line up
                        let delta = self.compute_next_unfolded_line_up(text_buffer);
                        self.cursors.move_up(delta, ke.modifiers.shift, text_buffer);
                    }
                    else {
                        self.cursors.move_up(1, ke.modifiers.shift, text_buffer);
                    }
                    self._undo_id += 1;
                    true
                }
            },
            KeyCode::ArrowDown => {
                if !self.multiline{
                    false
                }
                else if ke.modifiers.logo || ke.modifiers.control {
                    false
                }
                else {
                    if self._anim_folding.state.is_folded() && self.cursors.set.len() == 1 {
                        // compute the nearest nonfolded line down
                        let delta = self.compute_next_unfolded_line_down(text_buffer);
                        self.cursors.move_down(delta, ke.modifiers.shift, text_buffer);
                    }
                    else {
                        self.cursors.move_down(1, ke.modifiers.shift, text_buffer);
                    }
                    self._undo_id += 1;
                    true
                }
            },
            KeyCode::ArrowLeft => {
                if ke.modifiers.logo || ke.modifiers.control { // token skipping
                    self.cursors.move_left_nearest_token(ke.modifiers.shift, text_buffer)
                }
                else {
                    self.cursors.move_left(1, ke.modifiers.shift, text_buffer);
                }
                self._undo_id += 1;
                true
            },
            KeyCode::ArrowRight => {
                if ke.modifiers.logo || ke.modifiers.control { // token skipping
                    self.cursors.move_right_nearest_token(ke.modifiers.shift, text_buffer)
                }
                else {
                    self.cursors.move_right(1, ke.modifiers.shift, text_buffer);
                }
                self._undo_id += 1;
                true
            },
            KeyCode::PageUp => {
                self.cursors.move_up(self._visible_lines.max(5) - 4, ke.modifiers.shift, text_buffer);
                self._undo_id += 1;
                true
            },
            KeyCode::PageDown => {
                self.cursors.move_down(self._visible_lines.max(5) - 4, ke.modifiers.shift, text_buffer);
                self._undo_id += 1;
                true
            },
            KeyCode::Home => {
                self.cursors.move_home(ke.modifiers.shift, text_buffer);
                self._undo_id += 1;
                true
            },
            KeyCode::End => {
                self.cursors.move_end(ke.modifiers.shift, text_buffer);
                self._undo_id += 1;
                true
            },
            KeyCode::Backspace => {
                if !self.read_only {
                    self.cursors.backspace(text_buffer, self._undo_id);
                    true
                }
                else {
                    false
                }
            },
            KeyCode::Delete => {
                if !self.read_only {
                    self.cursors.delete(text_buffer);
                    true
                }
                else {
                    false
                }
            },
            KeyCode::KeyZ => {
                if !self.read_only {
                    if ke.modifiers.logo || ke.modifiers.control {
                        if ke.modifiers.shift { // redo
                            text_buffer.redo(true, &mut self.cursors);
                            true
                        }
                        else { // undo
                            text_buffer.undo(true, &mut self.cursors);
                            true
                        }
                    }
                    else {
                        false
                    }
                }
                else {
                    false
                }
            },
            KeyCode::KeyX => { // cut, the actual copy comes from the TextCopy event from the platform layer
                if !self.read_only && (ke.modifiers.logo || ke.modifiers.control) { // cut
                    self.cursors.replace_text("", text_buffer);
                    true
                }
                else {
                    false
                }
            },
            KeyCode::KeyA => { // select all
                if ke.modifiers.logo || ke.modifiers.control { // cut
                    self.cursors.select_all(text_buffer);
                    // don't scroll!
                    self.view.redraw_view_area(cx);
                    false
                }
                else {
                    false
                }
            },
            KeyCode::Alt => {
                // how do we find the center line of the view
                // its simply the top line
                self.start_code_folding(cx, text_buffer);
                false
                //return CodeEditorEvent::FoldStart
            },
            KeyCode::Tab => {
                if !self.read_only {
                    if ke.modifiers.shift {
                        self.cursors.remove_tab(text_buffer, 4);
                    }
                    else {
                        self.cursors.insert_tab(text_buffer, "    ");
                    }
                    true
                }
                else {
                    false
                }
            },
            KeyCode::Return => {
                if !self.read_only && self.multiline {
                    if !ke.modifiers.control && !ke.modifiers.logo {
                        self.cursors.insert_newline_with_indent(text_buffer);
                    }
                    true
                }
                else {
                    false
                }
            },
            _ => false
        };
        if cursor_moved {
            //self.update_highlight(cx, text_buffer);
            self.scroll_last_cursor_visible(cx, text_buffer, 0.);
            self.view.redraw_view_area(cx);
            self.reset_cursor_blinker(cx);
        }
    }

    fn handle_text_input(&mut self, cx: &mut Cx, te: &TextInputEvent, text_buffer: &mut TextBuffer) {
        if te.replace_last {
            text_buffer.undo(false, &mut self.cursors);
        }

        if !te.was_paste && te.input.len() == 1 {
            match te.input.chars().next().unwrap() {
                '(' => {
                    self.cursors.insert_around("(", ")", text_buffer);
                },
                '[' => {
                    self.cursors.insert_around("[", "]", text_buffer);
                },
                '{' => {
                    self.cursors.insert_around("{", "}", text_buffer);
                },
                '"' => {
                    self.cursors.insert_around("\"", "\"", text_buffer);
                },
                ')' => {
                    self.cursors.overwrite_if_exists_or_deindent(")", 4, text_buffer);
                },
                ']' => {
                    self.cursors.overwrite_if_exists_or_deindent("]", 4, text_buffer);
                },
                '}' => {
                    self.cursors.overwrite_if_exists_or_deindent("}", 4, text_buffer);
                },
                _ => {
                    self.cursors.replace_text(&te.input, text_buffer);
                }
            }
            // lets insert a newline
        }
        else {
            if !self.multiline {
                let replaced = te.input.replace("\n", "");
                self.cursors.replace_text(&replaced, text_buffer);
            }
            else {
                self.cursors.replace_text(&te.input, text_buffer);
            }
        }
        //self.update_highlight(cx, text_buffer);
        self.scroll_last_cursor_visible(cx, text_buffer, 0.);
        self.view.redraw_view_area(cx);
        self.reset_cursor_blinker(cx);

        cx.send_signal(text_buffer.signal, TextBuffer::status_data_update());

    }

    pub fn handle_text_editor(&mut self, cx: &mut Cx, event: &mut Event, text_buffer: &mut TextBuffer) -> TextEditorEvent {
        if self.view.handle_scroll_bars(cx, event) {
            if let Some(last_finger_move) = self._last_finger_move {
                if let Some(grid_select_corner) = self._grid_select_corner {
                    let pos = self.compute_grid_text_pos_from_abs(cx, last_finger_move);
                    self.cursors.grid_select(grid_select_corner, pos, text_buffer);
                }
                else {
                    let offset = self.text.find_closest_offset(cx, &self._text_area, last_finger_move);
                    self.cursors.set_last_cursor_head(offset, text_buffer);
                }
            }
            // the editor actually redraws on scroll, its because we don't actually
            // generate the entire file as GPU text-buffer just the visible area
            // in JS this wasn't possible performantly but in Rust its a breeze.
            self.view.redraw_view_area(cx);
        }
        let last_mutation_id = text_buffer.mutation_id;
        // global events
        match event {
            Event::Timer(te) => if self._cursor_blink_timer.is_timer(te) {
                if self.has_key_focus(cx) {
                    self._cursor_blink_timer = cx.start_timer(self.cursor_blink_speed, false);
                }
                // update the cursor uniform to blink it.
                self._cursor_blink_flipflop = 1.0 - self._cursor_blink_flipflop;
                self._undo_id += 1;
                self._highlight_visibility = 1.0;
                self._cursor_area.write_uniform_float(cx, Self::uniform_cursor_blink(), self._cursor_blink_flipflop);
                if self.highlight_area_on {
                    self._highlight_area.write_uniform_float(cx, Self::uniform_highlight_visible(), self._highlight_visibility);
                }

                // ok see if we changed.
                if self._last_lag_mutation_id != text_buffer.mutation_id {
                    let was_filechange = self._last_lag_mutation_id != 0;
                    self._last_lag_mutation_id = text_buffer.mutation_id;
                    if was_filechange {
                        // lets post a signal on the textbuffer
                        return TextEditorEvent::LagChange;
                    }
                }
            },
            Event::Signal(se) => if let Some(statusses) = se.signals.get(&text_buffer.signal) {
                for status in statusses {
                    if *status == TextBuffer::status_loaded()
                        || *status == TextBuffer::status_message_update()
                        || *status == TextBuffer::status_search_update()
                        || *status == TextBuffer::status_data_update() {
                        self.view.redraw_view_area(cx);
                    }
                    //else if *status == TextBuffer::status_jump_to_offset() {
                    //    if !text_buffer.is_loaded {
                    //        self._jump_to_offset = true;
                    //    }
                    //    else {
                    //        self.do_jump_to_offset(cx, text_buffer);
                    //    }
                    // }
                    else if *status == TextBuffer::status_keyboard_update() {
                        if let Some(key_down) = &text_buffer.keyboard.key_down {
                            match key_down {
                                KeyCode::Alt => {
                                    self.start_code_folding(cx, text_buffer);
                                },
                                _ => ()
                            }
                        }
                        if let Some(key_up) = &text_buffer.keyboard.key_up {
                            match key_up {
                                KeyCode::Alt => {
                                    self.start_code_unfolding(cx, text_buffer);
                                },
                                _ => ()
                            }
                        }
                    }
                }
            },
            _ => ()
        }
        // editor local
        match event.hits(cx, self.view.get_view_area(cx), HitOpt::default()) {
            Event::KeyFocus(_kf) => {
                self.reset_cursor_blinker(cx);
                self.view.redraw_view_area(cx);
                return TextEditorEvent::KeyFocus
            },
            Event::KeyFocusLost(_kf) => {
                self.view.redraw_view_area(cx);
                return TextEditorEvent::KeyFocusLost
            },
            Event::FingerDown(fe) => {
                self.handle_finger_down(cx, &fe, text_buffer);
            },
            Event::FingerHover(_fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Text);
            },
            Event::FingerUp(fe) => {
                self.handle_finger_up(cx, &fe, text_buffer);
            },
            Event::FingerMove(fe) => {
                self.handle_finger_move(cx, &fe, text_buffer);
            },
            Event::KeyDown(ke) => {
                if ke.key_code == KeyCode::Escape {
                    let pos =  self.cursors.get_last_cursor_head();
                    self.cursors.clear_and_set_last_cursor_head_and_tail(pos, pos, text_buffer);
                    return TextEditorEvent::Escape
                }/*
                if ke.key_code == KeyCode::KeyD && (ke.modifiers.logo || ke.modifiers.control) {
                    // check if d is inside a search cursor
                    let pos = self.cursors.get_last_cursor_head();
                    let mut repeat = false;
                    for result in text_buffer.markers.search_cursors.iter().rev(){
                        if pos >= result.tail && pos <= result.head {
                            repeat = true;
                            break;
                        }
                    }/*
                    if !repeat{
                        let search = self.cursors.get_ident_around_last_cursor_and_set(text_buffer);
                        // lets select the ident

                        return TextEditorEvent::Decl(search)
                    }*/
                }*/
                if ke.key_code == KeyCode::KeyF && (ke.modifiers.logo || ke.modifiers.control) {
                    let search = self.cursors.get_ident_around_last_cursor_and_set(text_buffer);
                    return TextEditorEvent::Search(search)
                }
                if ke.key_code == KeyCode::KeyS && (ke.modifiers.logo || ke.modifiers.control) {
                    let search = self.cursors.get_ident_around_last_cursor_and_set(text_buffer);
                    return TextEditorEvent::Decl(search)
                }

                if ke.key_code == KeyCode::Return && (ke.modifiers.logo || ke.modifiers.control) {
                    return TextEditorEvent::AutoFormat
                }
                self.handle_key_down(cx, &ke, text_buffer);
            },
            Event::KeyUp(ke) => {
                match ke.key_code {
                    KeyCode::Alt => {
                        self.start_code_unfolding(cx, text_buffer);
                    },
                    _ => (),
                }
                self.reset_cursor_blinker(cx);
            },
            Event::TextInput(te) => {
                if !self.read_only {
                    self.handle_text_input(cx, &te, text_buffer);
                }
            },
            Event::TextCopy(_) => match event { // access the original event
                Event::TextCopy(req) => {
                    req.response = Some(self.cursors.get_all_as_string(text_buffer));
                },
                _ => ()
            },
            _ => ()
        };
        // i need to know if selection changed, ifso
        //
        if last_mutation_id != text_buffer.mutation_id {
            TextEditorEvent::Change
        }
        else {
            TextEditorEvent::None
        }
    }

    pub fn has_key_focus(&self, cx: &Cx) -> bool {
        cx.has_key_focus(self._view_area)
    }

    pub fn set_key_focus(&mut self, cx: &mut Cx) {
        if self._view_area == Area::Empty {
            self._set_key_focus_on_load = true;
            return
        }
        cx.set_key_focus(self._view_area);
        self.reset_cursor_blinker(cx);
    }

    pub fn new_draw_calls(&mut self, cx: &mut Cx, line_number_bg: bool) {
        // layering, this sets the draw call order
        self._highlight_area = cx.new_instance_draw_call(&self.token_highlight.shader, 0).into();
        //cx.new_instance_layer(self.select_highlight.shader_id, 0);
        cx.new_instance_draw_call(&self.cursor_row.shader, 0);
        cx.new_instance_draw_call(&self.selection.shader, 0);
        cx.new_instance_draw_call(&self.message_marker.shader, 0);
        cx.new_instance_draw_call(&self.search_marker.shader, 0);
        cx.new_instance_draw_call(&self.paren_pair.shader, 0);

        // force next begin_text in another drawcall
        self._text_inst = Some(self.text.begin_text(cx));
        self._indent_line_inst = Some(cx.new_instance_draw_call(&self.indent_lines.shader, 0));

        self._cursor_area = cx.new_instance_draw_call(&self.cursor.shader, 0).into();

        if self.draw_line_numbers {
            if line_number_bg {
                let inst = self.gutter_bg.draw_quad_rel(cx, Rect {x: 0., y: 0., w: self.line_number_width, h: cx.get_height_total()});
                inst.set_do_scroll(cx, false, false);
            }
            let inst = self.line_number_text.begin_text(cx);
            inst.inst.set_do_scroll(cx, false, true);
            self._line_number_inst = Some(inst);
        }
    }

    pub fn init_draw_state(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        self._monospace_base = self.text.get_monospace_base(cx);
        self.set_font_scale(cx, self.open_font_scale);
        self._draw_cursors = DrawCursors::new();
        self._draw_messages = DrawCursors::new();
        self._draw_search = DrawCursors::new();
        self._tokens_on_line = 0;
        self._visible_lines = 0;
        self._newline_tabs = 0;
        self._last_tabs = 0;
        self._indent_stack.truncate(0);
        self._indent_id_alloc = 1.0;
        self._paren_stack.truncate(0);
        self._draw_cursors.set_next(&self.cursors.set);
        self._draw_search.set_next(
            if self.search_markers_bypass.len() > 0 {&self.search_markers_bypass}else {&text_buffer.markers.search_cursors}
        );
        self._line_geometry.truncate(0);
        self._line_largest_font = self.text.text_style.font_size;
        self._last_indent_color = self.colors.indent_line_unknown;
        // indent
        cx.move_turtle(self.line_number_width, self.top_padding);
    }

    pub fn begin_text_editor(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) -> Result<(), ()> {
        // adjust dilation based on DPI factor
        self.view.begin_view(cx, Self::layout_bg().get(cx)) ?;

        self.apply_style(cx);

        if !text_buffer.is_loaded {
            //et bg_inst = self.bg.begin_quad(cx, &Layout {
            //    align: Align::left_top(),
            //    ..self.bg_layout.clone()
            //});
            self.text.color = color("#666");
            self.text.draw_text(cx, "...");
            //self.bg.end_quad(cx, &bg_inst);
            //self._bg_area = bg_inst.into_area();
            self.view.end_view(cx);
            return Err(())
        }
        else {
            //let bg_area = bg_inst.into_area();
            let view_area = self.view.get_view_area(cx);
            cx.update_area_refs(self._view_area, view_area);
            //self._bg_area = bg_area;
            self._view_area = view_area;
            if self._set_key_focus_on_load {
                self._set_key_focus_on_load = false;
                self.set_key_focus(cx);
            }

            let inst = self.bg.begin_quad_fill(cx);
            inst.set_do_scroll(cx, false, false); // don't scroll the bg
            self._bg_inst = Some(inst);

            self.new_draw_calls(cx, true);

            if let Some(select_scroll) = &mut self._select_scroll {
                let scroll_pos = self.view.get_scroll_pos(cx);
                if self.view.set_scroll_pos(cx, Vec2 {
                    x: scroll_pos.x + select_scroll.delta.x,
                    y: scroll_pos.y + select_scroll.delta.y
                }) {
                    self.view.redraw_view_area(cx);
                }
                else {
                    select_scroll.at_end = true;
                }
            }

            if text_buffer.markers.mutation_id != text_buffer.mutation_id {
                self._draw_messages.term(&text_buffer.markers.message_cursors);
            }
            else {
                self._draw_messages.set_next(&text_buffer.markers.message_cursors);
            }
            self._last_cursor_pos = self.cursors.get_last_cursor_text_pos(text_buffer);


            // lets compute our scroll line position and keep it where it is
            self.do_folding_animation_step(cx);

            self.init_draw_state(cx, text_buffer);

            self._scroll_pos = self.view.get_scroll_pos(cx);

            return Ok(())
        }
    }

    fn do_folding_animation_step(&mut self, cx: &mut Cx) {
        // run the folding animation
        let anim_folding = &mut self._anim_folding;
        if anim_folding.state.is_animating() {
            anim_folding.state.next_anim_step();
            if anim_folding.state.is_animating() {
                self.view.redraw_view_area(cx);
            }
            anim_folding.did_animate = true;
        }
        else {
            anim_folding.did_animate = false;
        }
        //let new_anim_font_size =
        self._anim_font_scale = anim_folding.state.get_font_size(self.open_font_scale, self.folded_font_scale);

        if self._anim_folding.did_animate {
            let mut ypos = self.top_padding;
            let mut ypos_at_line = ypos;
            let focus_line = self._anim_folding.focussed_line;
            if focus_line < self._line_geometry.len() {
                for (line, geom) in self._line_geometry.iter().enumerate() {
                    if focus_line == line {
                        ypos_at_line = ypos;
                    }
                    ypos += if geom.was_folded {
                        self._monospace_base.y * self.text.text_style.font_size * self._anim_font_scale
                    }
                    else {
                        self._monospace_base.y * self.text.text_style.font_size
                    }
                }
                ypos += self._final_fill_height;
                let dy = self._line_geometry[focus_line].walk.y - ypos_at_line;
                let sv = self.view.get_scroll_view_total();
                self.view.set_scroll_view_total(cx, Vec2 {x: sv.x, y: ypos});
                let scroll_pos = self.view.get_scroll_pos(cx);
                self.view.set_scroll_pos(cx, Vec2 {
                    x: scroll_pos.x,
                    y: scroll_pos.y - dy
                });
            }
        }
    }
    /*
    fn update_highlight(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        self._highlight_selection = self.cursors.get_selection_highlight(text_buffer);
        let new_token = self.cursors.get_token_highlight(text_buffer);
        if new_token != self._highlight_token {
            self.reset_highlight_visible(cx);
        }
        self._highlight_token = new_token;

    }*/

    fn draw_new_line(&mut self, cx: &mut Cx) {
        // line geometry is used for scrolling look up of cursors
        let line_geom = LineGeom {
            walk: cx.get_rel_turtle_pos(),
            font_size: self._line_largest_font,
            was_folded: self._line_was_folded,
            indent_id: if let Some((_, id)) = self._indent_stack.last() {*id}else {0.}
        };

        // draw a linenumber if we are visible
        let origin = cx.get_turtle_origin();
        if self.draw_line_numbers && cx.turtle_line_is_visible(self._monospace_size.y, self._scroll_pos) {
            // lets format a number, we go to 4 numbers
            // yes this is dumb as rocks. but we need to be cheapnfast
            let chunk = &mut self._line_number_chunk;
            chunk.truncate(0);
            let line_num = self._line_geometry.len() + 1 + self.line_number_offset;
            let mut scale = 10000;
            let mut fill = false;
            loop {
                let digit = ((line_num / scale) % 10) as u8;
                if digit != 0 {
                    fill = true;
                }
                if fill {
                    chunk.push((48 + digit) as char);
                }
                else {
                    chunk.push(' ');
                }
                if scale <= 1 {
                    break
                }
                scale /= 10;
            }
            if line_num == self._last_cursor_pos.row + 1 {
                self.line_number_text.color = self.colors.line_number_highlight;
            }
            else {
                self.line_number_text.color = self.colors.line_number_normal;
            }
            let chunk_width = self._monospace_size.x * 5.0;
            self.line_number_text.add_text(cx, origin.x + (self.line_number_width - chunk_width - self.line_number_click_margin), origin.y + line_geom.walk.y, 0, self._line_number_inst.as_mut().unwrap(), chunk, | _, _, _, _ | {0.});
        }

        cx.turtle_new_line_min_height(self._monospace_size.y);

        cx.move_turtle(self.line_number_width, 0.);

        self._tokens_on_line = 0;
        //self._line_was_visible = false;

        self._draw_cursors.process_newline();
        self._draw_messages.process_newline();

        // highlighting the selection
        /*let hl_len = self._highlight_selection.len();
        if hl_len != 0 {
            for bp in 0..self._line_chunk.len().max(hl_len) - hl_len {
                let mut found = true;
                for ip in 0..hl_len {
                    if self._highlight_selection[ip] != self._line_chunk[bp + ip].1 {
                        found = false;
                        break;
                    }
                }
                if found { // output a rect
                    let origin = cx.get_turtle_origin();
                    let min_x = self._line_chunk[bp].0;
                    let max_x = self._line_chunk[bp + hl_len].0;
                    self.draw_token_highlight_quad(cx, Rect {
                        x: min_x,
                        y: line_geom.walk.y + origin.y,
                        w: max_x - min_x,
                        h: self._monospace_size.y,
                    });
                }
            }
            self._line_chunk.truncate(0);
        }*/

        // search for all markings
        self._line_geometry.push(line_geom);
        self._line_largest_font = self.text.text_style.font_size;
    }

    fn draw_indent_lines(&mut self, cx: &mut Cx, geom_y: f32, tabs: usize) {
        let y_pos = geom_y - cx.get_turtle_origin().y;
        let tab_variable_width = self._monospace_base.x * 4. * self.text.text_style.font_size * self._anim_font_scale;
        let tab_fixed_width = self._monospace_base.x * 4. * self.text.text_style.font_size;
        let mut off = self.line_number_width;
        for i in 0..tabs {
            let (indent_color, indent_id) = if i < self._indent_stack.len() {self._indent_stack[i]}else {
                (self.colors.indent_line_unknown, 0.)
            };
            let tab_width = if i < self.folding_depth {tab_fixed_width}else {tab_variable_width};
            self.indent_lines.color = indent_color;
            let inst = self.indent_lines.draw_quad_rel(cx, Rect {
                x: off,
                y: y_pos,
                w: tab_width,
                h: self._monospace_size.y
            });
            off += tab_width;
            inst.push_float(cx, indent_id);
        }
    }

    pub fn draw_chunk(&mut self, cx: &mut Cx, token_chunks_index: usize, flat_text: &Vec<char>, token_chunk: &TokenChunk, markers: &TextBufferMarkers) {
        if token_chunk.len == 0 {
            return
        }

        let token_type = token_chunk.token_type;
        let chunk = &flat_text[token_chunk.offset..(token_chunk.offset + token_chunk.len)]; //chunk;
        let offset = token_chunk.offset; // end_offset - chunk.len() - 1;
        let next_char = token_chunk.next;

        // maintain paren stack
        if token_type == TokenType::ParenOpen {
            self.draw_paren_open(token_chunks_index, offset, next_char, chunk);
        }

        // do indent depth walking
        if self._tokens_on_line == 0 {
            let font_scale = match token_type {
                TokenType::Whitespace => {
                    let tabs = chunk.len()>>2;
                    while tabs > self._indent_stack.len() {
                        self._indent_stack.push((self._last_indent_color, self._indent_id_alloc));
                        // allocating an indent_id, we also need to
                        self._indent_id_alloc += 1.0;
                    }
                    while tabs < self._indent_stack.len() {
                        self._indent_stack.pop();
                        if let Some(indent) = self._indent_stack.last() {
                            self._last_indent_color = indent.0;
                        }
                    }
                    // lets do the code folding here. if we are tabs > fold line
                    // lets change the fontsize
                    if tabs >= self.folding_depth || next_char == '\n' {
                        // ok lets think. we need to move it over by the delta of 8 spaces * _anim_font_size
                        let dx = (self._monospace_base.x * self.text.text_style.font_size * 4. * (self.folding_depth as f32)) - (self._monospace_base.x * self.text.text_style.font_size * self._anim_font_scale * 4. * (self.folding_depth as f32));
                        cx.move_turtle(dx, 0.0);
                        self._line_was_folded = true;
                        self._anim_font_scale
                    }
                    else {
                        self._line_was_folded = false;
                        self.open_font_scale
                    }
                }
                TokenType::Newline | TokenType::CommentLine | TokenType::CommentChunk | TokenType::CommentMultiBegin | TokenType::CommentMultiEnd | TokenType::Hash => {
                    self._line_was_folded = true;
                    self._anim_font_scale
                }
                _ => {
                    self._indent_stack.truncate(0);
                    self._line_was_folded = false;
                    self.open_font_scale
                }
            };
            self.set_font_scale(cx, font_scale);
        }
        // colorise indent lines properly
        if self._tokens_on_line < 4 {
            match token_type {
                TokenType::Flow => {
                    self._last_indent_color = self.colors.indent_line_flow;
                },
                TokenType::Looping => {
                    self._last_indent_color = self.colors.indent_line_looping;
                },
                TokenType::TypeDef => {
                    self._last_indent_color = self.colors.indent_line_typedef;
                },
                TokenType::Fn | TokenType::Call => {
                    self._last_indent_color = self.colors.indent_line_fn;
                }
                _ => ()
            }
        }
        // lets check if the geom is visible
        if let Some(geom) = cx.walk_turtle_right_no_wrap(
            self._monospace_size.x * (chunk.len() as f32),
            self._monospace_size.y,
            self._scroll_pos
        ) {
            let mut mark_spaces = 0.0;
            // determine chunk color
            self.text.color = match token_type {
                TokenType::Whitespace => {
                    if self._tokens_on_line == 0 && chunk[0] == ' ' {
                        let tabs = chunk.len()>>2;
                        // if self._last_tabs
                        self._last_tabs = tabs;
                        self._newline_tabs = tabs;
                        self.draw_indent_lines(cx, geom.y, tabs);
                    }
                    else if next_char == '\n' {
                        mark_spaces = 1.0;
                    }
                    self.colors.whitespace
                },
                TokenType::Newline => {
                    if self._tokens_on_line == 0 {
                        self._newline_tabs = 0;
                        self.draw_indent_lines(cx, geom.y, self._last_tabs);
                    }
                    else {
                        self._last_tabs = self._newline_tabs;
                        self._newline_tabs = 0;
                    }
                    self.colors.whitespace
                },
                TokenType::BuiltinType => self.colors.keyword,
                TokenType::Keyword => self.colors.keyword,
                TokenType::Bool => self.colors.keyword,
                TokenType::Error => self.colors.error,
                TokenType::Warning => self.colors.warning,
                TokenType::Defocus => self.colors.defocus,
                TokenType::Flow => {
                    self.colors.flow
                }
                TokenType::Looping => {
                    self.colors.looping
                }
                TokenType::TypeDef => {
                    self.colors.keyword
                }
                TokenType::Impl => {
                    self.colors.keyword
                }
                TokenType::Fn => {
                    self.colors.keyword
                }
                TokenType::Identifier => {/*
                    if chunk == &self._highlight_token[0..] {
                        self.draw_token_highlight_quad(cx, geom);

                    }*/
                    self.colors.identifier
                }
                TokenType::Call => {/*
                    if chunk == &self._highlight_token[0..] {
                        self.draw_token_highlight_quad(cx, geom);
                    }*/
                    self.colors.call
                },
                TokenType::TypeName => {/*
                    if chunk == &self._highlight_token[0..] {
                        self.draw_token_highlight_quad(cx, geom);
                    }*/
                    self.colors.type_name
                },
                TokenType::ThemeName => {/*
                    if chunk == &self._highlight_token[0..] {
                        self.draw_token_highlight_quad(cx, geom);
                    }*/
                    self.colors.theme_name
                },
                TokenType::Regex => self.colors.string,
                TokenType::String => self.colors.string,
                TokenType::Number => self.colors.number,

                TokenType::StringMultiBegin => self.colors.string,
                TokenType::StringChunk => self.colors.string,
                TokenType::StringMultiEnd => self.colors.string,

                TokenType::CommentMultiBegin => self.colors.comment,
                TokenType::CommentMultiEnd => self.colors.comment,
                TokenType::CommentLine => self.colors.comment,
                TokenType::CommentChunk => self.colors.comment,
                TokenType::ParenOpen => {
                    let depth = self._paren_stack.len();
                    self._paren_stack.last_mut().unwrap().geom_open = Some(geom);
                    match depth % 2 {
                        0 => self.colors.paren_d1,
                        _ => self.colors.paren_d2,
                    }
                },
                TokenType::ParenClose => {
                    if let Some(paren) = self._paren_stack.last_mut() {
                        paren.geom_close = Some(geom);
                    }
                    else if self.mark_unmatched_parens {
                        self.paren_pair.color = self.colors.paren_pair_fail;
                        self.paren_pair.draw_quad_abs(cx, geom);
                    }
                    let depth = self._paren_stack.len();
                    match depth % 2 {
                        0 => self.colors.paren_d1,
                        _ => self.colors.paren_d2,
                        //_=>self.colors.paren_d3
                    }
                },
                TokenType::Operator => self.colors.operator,
                TokenType::Namespace => self.colors.operator,
                TokenType::Hash => self.colors.operator,
                TokenType::Delimiter => self.colors.delimiter,
                TokenType::Colon => self.colors.delimiter,
                TokenType::Splat => self.colors.operator,
                TokenType::Eof => self.colors.unexpected,
                TokenType::Unexpected => self.colors.unexpected
            };

            if self._tokens_on_line == 0 {
                self._visible_lines += 1;
                //self._line_was_visible = true;
            }

            let cursors = &self.cursors.set;
            //let messages_cursors = &text_buffer.message_cursors;
            let last_cursor = self.cursors.last_cursor;
            let draw_cursors = &mut self._draw_cursors;
            let draw_messages = &mut self._draw_messages;
            let draw_search = &mut self._draw_search;

            let height = self._monospace_size.y;
            let search_cursors = if self.search_markers_bypass.len()>0 {&self.search_markers_bypass} else {&markers.search_cursors};
            // actually generate the GPU data for the text
            let z = 2.0; // + self._paren_stack.len() as f32;
            //self.text.z = z;
            //let line_chunk = &mut self._line_chunk;
            if search_cursors.len() > 0 { // slow loop
                self.text.add_text(cx, geom.x, geom.y, offset, self._text_inst.as_mut().unwrap(), &chunk, | ch, offset, x, w | {
                    //line_chunk.push((x, ch));
                    draw_search.mark_text_select_only(search_cursors, offset, x, geom.y, w, height);
                    draw_messages.mark_text_select_only(&markers.message_cursors, offset, x, geom.y, w, height);
                    draw_cursors.mark_text_with_cursor(cursors, ch, offset, x, geom.y, w, height, z, last_cursor, mark_spaces)
                });
            }
            else {
                self.text.add_text(cx, geom.x, geom.y, offset, self._text_inst.as_mut().unwrap(), &chunk, | ch, offset, x, w | {
                    //line_chunk.push((x, ch));
                    draw_messages.mark_text_select_only(&markers.message_cursors, offset, x, geom.y, w, height);
                    draw_cursors.mark_text_with_cursor(cursors, ch, offset, x, geom.y, w, height, z, last_cursor, mark_spaces)
                });
            }
        }
        self._tokens_on_line += 1;

        // Do all the Paren matching highlighting drawing
        if token_chunk.token_type == TokenType::ParenClose {
            self.draw_paren_close(cx, token_chunks_index, offset, next_char, chunk);
        }
        else {
            if token_type == TokenType::Newline {
                self.draw_new_line(cx);
            }
        }
    }
    /*
    fn draw_token_highlight_quad(&mut self, cx: &mut Cx, geom: Rect) {
        let inst = self.token_highlight.draw_quad_abs(cx, geom);
        if inst.need_uniforms_now(cx) {
            inst.push_uniform_float(cx, self._highlight_visibility);
        }
    }*/

    fn draw_paren_open(&mut self, token_chunks_index: usize, offset: usize, next_char: char, chunk: &[char]) {
        let marked = if let Some(pos) = self.cursors.get_last_cursor_singular() {
            pos == offset || pos == offset + 1 && next_char != '(' && next_char != '{' && next_char != '['
        }
        else {false};

        self._paren_stack.push(ParenItem {
            pair_start: token_chunks_index, //self.token_chunks.len(),
            geom_open: None,
            geom_close: None,
            marked: marked,
            exp_paren: chunk[0]
        });
    }

    fn draw_paren_close(&mut self, cx: &mut Cx, token_chunks_index: usize, offset: usize, next_char: char, chunk: &[char]) {
        //let token_chunks_len = self.token_chunks.len();
        if self._paren_stack.len() == 0 {
            return
        }
        let last = self._paren_stack.pop().unwrap();
        // !!!!self.token_chunks[last.pair_start].pair_token = token_chunks_index;
        if last.geom_open.is_none() && last.geom_close.is_none() {
            return
        }
        if !self.has_key_focus(cx) {
            return
        }
        if let Some(pos) = self.cursors.get_last_cursor_singular() {
            if self.mark_unmatched_parens {
                // cursor is near the last one or its marked
                let fail = if last.exp_paren == '(' && chunk[0] != ')' ||
                last.exp_paren == '[' && chunk[0] != ']' ||
                last.exp_paren == '{' && chunk[0] != '}' {
                    self.paren_pair.color = self.colors.paren_pair_fail;
                    true
                }
                else {
                    self.paren_pair.color = self.colors.paren_pair_match;
                    false
                };
                if fail || pos == offset || pos == offset + 1 && next_char != ')' && next_char != '}' && next_char != ']' || last.marked {
                    // fuse the tokens
                    if last.pair_start + 1 == token_chunks_index && !last.geom_open.is_none() && !last.geom_close.is_none() {
                        let geom_open = last.geom_open.unwrap();
                        let geom_close = last.geom_open.unwrap();
                        let geom = Rect {
                            x: geom_open.x,
                            y: geom_open.y,
                            w: geom_open.w + geom_close.w,
                            h: geom_close.h
                        };
                        self.paren_pair.draw_quad_abs(cx, geom);
                    }
                    else {
                        if let Some(rc) = last.geom_open {
                            self.paren_pair.draw_quad_abs(cx, rc);
                        }
                        if let Some(rc) = last.geom_close {
                            self.paren_pair.draw_quad_abs(cx, rc);
                        }
                    }
                }
            };
        }
    }

    fn draw_paren_unmatched(&mut self, cx: &mut Cx) {
        if !self.mark_unmatched_parens {
            return
        }
        while self._paren_stack.len()>0 {
            let last = self._paren_stack.pop().unwrap();
            if self.has_key_focus(cx) && !last.geom_open.is_none() {
                self.paren_pair.color = self.colors.paren_pair_fail;
                if let Some(rc) = last.geom_open {
                    self.paren_pair.draw_quad_abs(cx, rc);
                }
            }
        }
    }

    pub fn end_text_editor(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {

        // lets insert an empty newline at the bottom so its nicer to scroll
        self.draw_new_line(cx);
        if !cx.is_height_computed() {
            cx.walk_turtle(Walk::wh(Width::Fix(0.0), Height::Fix(self._monospace_size.y)));
        }

        self._text_area = self.text.end_text(cx, self._text_inst.as_ref().unwrap());

        if self.draw_line_numbers {
            self.line_number_text.end_text(cx, self._line_number_inst.as_ref().unwrap());
        }

        // unmatched highlighting
        self.draw_paren_unmatched(cx);
        self.draw_cursors(cx);
        //self.do_selection_animations(cx);
        self.draw_selections(cx);
        self.draw_search_markers(cx);
        self.draw_message_markers(cx, text_buffer);

        // inject a final page
        self._final_fill_height = cx.get_height_total() - self._monospace_size.y;
        if !cx.is_height_computed() {
            cx.walk_turtle(Walk::wh(Width::Fix(0.0), Height::Fix(self._final_fill_height)));
        }
        // last bits
        self.do_selection_scrolling(cx, text_buffer);
        self.place_ime_and_draw_cursor_row(cx);
        self.set_indent_line_highlight_id(cx);
        self.draw_shadows(cx);
        self.bg.end_quad_fill(cx, &self._bg_inst.take().unwrap());

        self.view.end_view(cx);

        if let Some(((head, tail), at_top)) = self._set_last_cursor {
            self._set_last_cursor = None;
            self._scroll_pos_on_load = None;
            self.cursors.clear_and_set_last_cursor_head_and_tail(head, tail, text_buffer);
            // i want the thing to be the top
            if at_top {
                self.scroll_last_cursor_top(cx, text_buffer);
            }
            else {
                self.scroll_last_cursor_visible(cx, text_buffer, self._final_fill_height * 0.8);
            }

            self.view.redraw_view_area(cx);
        }
        else if let Some(scroll_pos_on_load) = self._scroll_pos_on_load {
            self.view.set_scroll_pos(cx, scroll_pos_on_load);
            self._scroll_pos_on_load = None;
        }
    }

    pub fn set_last_cursor(&mut self, cx: &mut Cx, cursor:(usize, usize), at_top:bool) {
        self._set_last_cursor = Some((cursor, at_top));
        self.view.redraw_view_area(cx);
    }


    fn draw_cursors(&mut self, cx: &mut Cx) {
        if self.has_key_focus(cx) {
            let origin = cx.get_turtle_origin();
            for rc in &self._draw_cursors.cursors {
                self.cursor.z = rc.z + 0.1;

                let inst = self.cursor.draw_quad_rel(cx, Rect {x: rc.x - origin.x, y: rc.y - origin.y, w: rc.w, h: rc.h});
                if inst.need_uniforms_now(cx) {
                    inst.push_uniform_float(cx, self._cursor_blink_flipflop);
                    //blink
                }
            }
        }
    }

    fn draw_shadows(&mut self, cx: &mut Cx) {
        let gutter_width = Self::gutter_width().get(cx);
        self.shadow.draw_shadow_left_at(cx, Rect {
            x: gutter_width,
            y: 0.,
            w: 0.,
            h: cx.get_height_total()
        });

        self.shadow.draw_shadow_top(cx);
    }

    fn draw_message_markers(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        let origin = cx.get_turtle_origin();
        let message_markers = &mut self._draw_messages.selections;

        for i in 0..message_markers.len() {
            let mark = &message_markers[i];
            let body = &text_buffer.markers.message_bodies[mark.index];
            self.message_marker.color = match body.level {
                TextBufferMessageLevel::Warning => self.colors.message_marker_warning,
                TextBufferMessageLevel::Error => self.colors.message_marker_error,
                TextBufferMessageLevel::Log => self.colors.message_marker_log,
            };
            self.message_marker.draw_quad_rel(cx, Rect {x: mark.rc.x - origin.x, y: mark.rc.y - origin.y, w: mark.rc.w, h: mark.rc.h});
        }
    }

    pub fn draw_search_markers(&mut self, cx: &mut Cx) {
        let origin = cx.get_turtle_origin();

        for mark in &self._draw_search.selections {
            self.search_marker.draw_quad_rel(cx, Rect {x: mark.rc.x - origin.x, y: mark.rc.y - origin.y, w: mark.rc.w, h: mark.rc.h});
        }
    }

    pub fn draw_selections(&mut self, cx: &mut Cx) {
        let origin = cx.get_turtle_origin();
        let sel = &mut self._draw_cursors.selections;
        // draw selections
        for i in 0..sel.len() {
            let cur = &sel[i];

            let mk_inst = self.selection.draw_quad_rel(cx, Rect {x: cur.rc.x - origin.x, y: cur.rc.y - origin.y, w: cur.rc.w, h: cur.rc.h});

            // do we have a prev?
            if i > 0 && sel[i - 1].index == cur.index {
                let p_rc = &sel[i - 1].rc;
                mk_inst.push_vec2(cx, Vec2 {x: p_rc.x - cur.rc.x, y: p_rc.w});
                // prev_x, prev_w
            }
            else {
                mk_inst.push_vec2(cx, Vec2 {x: 0., y: -1.});
                // prev_x, prev_w
            }
            // do we have a next
            if i < sel.len() - 1 && sel[i + 1].index == cur.index {
                let n_rc = &sel[i + 1].rc;
                mk_inst.push_vec2(cx, Vec2 {x: n_rc.x - cur.rc.x, y: n_rc.w});
                // prev_x, prev_w
            }
            else {
                mk_inst.push_vec2(cx, Vec2 {x: 0., y: -1.});
                // prev_x, prev_w
            }
        }
    }

    fn place_ime_and_draw_cursor_row(&mut self, cx: &mut Cx) {
        // place the IME
        if let Some(last_cursor) = self._draw_cursors.last_cursor {
            let rc = self._draw_cursors.cursors[last_cursor];
            if let Some(_) = self.cursors.get_last_cursor_singular() {
                // lets draw the cursor line
                if self.draw_cursor_row {
                    self.cursor_row.draw_quad_abs(cx, Rect {
                        x: self.line_number_width + cx.get_turtle_origin().x,
                        y: rc.y,
                        w: cx.get_width_total().max(cx.get_turtle_bounds().x) - self.line_number_width,
                        h: rc.h
                    });
                }
            }
            if cx.has_key_focus(self.view.get_view_area(cx)) {
                let scroll_pos = self.view.get_scroll_pos(cx);
                cx.show_text_ime(rc.x - scroll_pos.x, rc.y - scroll_pos.y);
            }
            else {
                cx.hide_text_ime();
            }
        }
    }

    fn do_selection_scrolling(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        // do select scrolling
        if let Some(select_scroll) = self._select_scroll.clone() {
            if let Some(grid_select_corner) = self._grid_select_corner {
                // self.cursors.grid_select(offset, text_buffer);
                let pos = self.compute_grid_text_pos_from_abs(cx, select_scroll.abs);
                self.cursors.grid_select(grid_select_corner, pos, text_buffer);
            }
            else {
                let offset = self.text.find_closest_offset(cx, &self._text_area, select_scroll.abs);
                self.cursors.set_last_cursor_head(offset, text_buffer);
            }
            if select_scroll.at_end {
                self._select_scroll = None;
            }
            self.view.redraw_view_area(cx);
        }
    }

    fn _do_selection_animations(&mut self, cx: &mut Cx) {
        if !self._anim_folding.state.is_animating() {
            let sel = &mut self._draw_cursors.selections;

            let mut anim_select_any = false;
            for i in 0..sel.len() {
                let cur = &mut sel[i];
                let start_time = if self._select_scroll.is_none() && !self._last_finger_move.is_none() {1.}else {0.};
                // silly selection animation start
                if i < self._anim_select.len() && cur.rc.y < self._anim_select[i].ypos {
                    // insert new one at the top
                    self._anim_select.insert(i, AnimSelect {time: start_time, invert: true, ypos: cur.rc.y});
                }
                let (wtime, htime, invert) = if i < self._anim_select.len() {
                    let len = self._anim_select.len() - 1;
                    let anim = &mut self._anim_select[i];
                    anim.ypos = cur.rc.y;
                    if anim.time <= 0.0001 {
                        anim.time = 0.0
                    }
                    else {
                        anim.time = anim.time * 0.55;
                        //= 0.1;
                        anim_select_any = true;
                    }
                    if i == len {
                        (anim.time, anim.time, i == 0 && anim.invert)
                    }
                    else {
                        (anim.time, 0., i == 0 && anim.invert)
                    }
                }
                else {
                    self._anim_select.push(AnimSelect {time: start_time, invert: i == 0, ypos: cur.rc.y});
                    anim_select_any = true;
                    (start_time, start_time, false)
                };
                let wtime = 1.0 - wtime as f32;
                let htime = 1.0 - htime as f32;

                if invert {
                    cur.rc.w = cur.rc.w * wtime;
                    cur.rc.h = cur.rc.h * htime;
                }
                else {
                    cur.rc.x = cur.rc.x + (cur.rc.w * (1. - wtime));
                    cur.rc.w = cur.rc.w * wtime;
                    cur.rc.h = cur.rc.h * htime;
                }
            }
            self._anim_select.truncate(sel.len());
            if anim_select_any {
                self.view.redraw_view_area(cx);
            }
        }
    }

    fn set_indent_line_highlight_id(&mut self, cx: &mut Cx) {
        // compute the line which our last cursor is on so we can set the highlight id
        if let Some(indent_inst) = self._indent_line_inst {
            let indent_id = if self.cursors.is_last_cursor_singular() && self._last_cursor_pos.row < self._line_geometry.len() {
                self._line_geometry[self._last_cursor_pos.row].indent_id
            }else {0.};
            let area: Area = indent_inst.clone().into();
            area.write_uniform_float(cx, Self::uniform_indent_sel(), indent_id);
        }
    }

    // set it once per line otherwise the LineGeom stuff isn't really working out.
    fn set_font_scale(&mut self, _cx: &Cx, font_scale: f32) {
        self.text.font_scale = font_scale;
        self.line_number_text.font_scale = font_scale;
        if font_scale > self._line_largest_font {
            self._line_largest_font = font_scale;
        }
        self._monospace_size.x = self._monospace_base.x * self.text.text_style.font_size * font_scale;
        self._monospace_size.y = self._monospace_base.y * self.text.text_style.font_size * font_scale;
    }

    pub fn reset_cursors(&mut self) {
        self.cursors = TextCursorSet::new();
    }

    fn scroll_last_cursor_visible(&mut self, cx: &mut Cx, text_buffer: &TextBuffer, height_pad: f32) {
        // so we have to compute (approximately) the rect of our cursor
        if self.cursors.last_cursor >= self.cursors.set.len() {
            panic !("LAST CURSOR INVALID");
        }

        let pos = self.cursors.get_last_cursor_text_pos(text_buffer);

        // alright now lets query the line geometry
        let row = pos.row.min(self._line_geometry.len() - 1);
        if row < self._line_geometry.len() {
            let geom = &self._line_geometry[row];
            let mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            //self.text.get_monospace_size(cx, geom.font_size);
            let rect = Rect {
                x: (pos.col as f32) * mono_size.x, // - self.line_number_width,
                y: geom.walk.y - mono_size.y * 1. - 0.5 * height_pad,
                w: mono_size.x * 4. + self.line_number_width,
                h: mono_size.y * 4. + height_pad
            };

            // scroll this cursor into view
            self.view.scroll_into_view(cx, rect);
        }
    }

    fn scroll_last_cursor_top(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        // so we have to compute (approximately) the rect of our cursor
        if self.cursors.last_cursor >= self.cursors.set.len() {
            panic !("LAST CURSOR INVALID");
        }

        let pos = self.cursors.get_last_cursor_text_pos(text_buffer);

        // alright now lets query the line geometry
        let row = pos.row.min(self._line_geometry.len() - 1);
        if row < self._line_geometry.len() {
            let geom = &self._line_geometry[row];
            let mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            //self.text.get_monospace_size(cx, geom.font_size);
            let rect = Rect {
                x: 0., // (pos.col as f32) * mono_size.x - self.line_number_width,
                y: geom.walk.y - mono_size.y * 1.,
                w: mono_size.x * 4. + self.line_number_width,
                h: self._final_fill_height + mono_size.y * 1.
            };

            // scroll this cursor into view
            self.view.scroll_into_view_no_smooth(cx, rect);
        }
    }

    fn compute_grid_text_pos_from_abs(&mut self, cx: &Cx, abs: Vec2) -> TextPos {
        //
        let rel = self.view.get_view_area(cx).abs_to_rel(cx, abs);
        let mut mono_size = Vec2::default();
        for (row, geom) in self._line_geometry.iter().enumerate() {
            //let geom = &self._line_geometry[pos.row];
            mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            if rel.y < geom.walk.y || rel.y >= geom.walk.y && rel.y <= geom.walk.y + mono_size.y { // its on the right line
                let col = ((rel.x - self.line_number_width).max(0.) / mono_size.x) as usize;
                // do a dumb calc
                return TextPos {row: row, col: col};
            }
        }
        // otherwise the file is too short, lets use the last line
        TextPos {row: self._line_geometry.len() - 1, col: (rel.x.max(0.) / mono_size.x) as usize}
    }

    fn compute_offset_from_ypos(&mut self, cx: &Cx, ypos_abs: f32, text_buffer: &TextBuffer, end: bool) -> usize {
        let rel = self.view.get_view_area(cx).abs_to_rel(cx, Vec2 {x: 0.0, y: ypos_abs});
        let mut mono_size;
        // = Vec2::zero();
        let end_col = if end {1 << 31}else {0};
        for (row, geom) in self._line_geometry.iter().enumerate() {
            //let geom = &self._line_geometry[pos.row];
            mono_size = Vec2 {x: self._monospace_base.x * geom.font_size, y: self._monospace_base.y * geom.font_size};
            if rel.y < geom.walk.y || rel.y >= geom.walk.y && rel.y <= geom.walk.y + mono_size.y { // its on the right line
                return text_buffer.text_pos_to_offset(TextPos {row: row, col: end_col})
            }
        }
        return text_buffer.text_pos_to_offset(TextPos {row: self._line_geometry.len() - 1, col: end_col})
    }

    fn start_code_folding(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        // start code folding anim
        let speed = 0.98;
        //self._anim_folding.depth = if halfway {1}else {2};
        //self._anim_folding.zoom_scale = if halfway {0.5}else {1.};
        //if halfway{9.0} else{1.0};
        self._anim_folding.state.do_folding(speed, 0.95);
        self._anim_folding.focussed_line = self.compute_focussed_line_for_folding(cx, text_buffer);
        //println!("FOLDING {}",self._anim_folding.focussed_line);
        self.view.redraw_view_area(cx);
    }

    fn start_code_unfolding(&mut self, cx: &mut Cx, text_buffer: &TextBuffer) {
        let speed = 0.96;
        self._anim_folding.state.do_opening(speed, 0.97);
        self._anim_folding.focussed_line = self.compute_focussed_line_for_folding(cx, text_buffer);
        //println!("UNFOLDING {}",self._anim_folding.focussed_line);
        self.view.redraw_view_area(cx);
        // return to normal size
    }

    fn check_select_scroll_dynamics(&mut self, fe: &FingerMoveEvent) -> bool {
        let pow_scale = 0.1;
        let pow_fac = 3.;
        let max_speed = 40.;
        let pad_scroll = 20.;
        let rect = Rect {
            x: fe.rect.x + pad_scroll,
            y: fe.rect.y + pad_scroll,
            w: fe.rect.w - 2. * pad_scroll,
            h: fe.rect.h - 2. * pad_scroll,
        };
        let delta = Vec2 {
            x: if fe.abs.x < rect.x {
                -((rect.x - fe.abs.x) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else if fe.abs.x > rect.x + rect.w {
                ((fe.abs.x - (rect.x + rect.w)) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else {
                0.
            },
            y: if fe.abs.y < rect.y {
                -((rect.y - fe.abs.y) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else if fe.abs.y > rect.y + rect.h {
                ((fe.abs.y - (rect.y + rect.h)) * pow_scale).powf(pow_fac).min(max_speed)
            }
            else {
                0.
            }
        };
        let last_scroll_none = self._select_scroll.is_none();
        if delta.x != 0. || delta.y != 0. {
            self._select_scroll = Some(SelectScroll {
                abs: fe.abs,
                delta: delta,
                at_end: false
            });
        }
        else {
            self._select_scroll = None;
        }
        last_scroll_none
    }

    fn compute_next_unfolded_line_up(&self, text_buffer: &TextBuffer) -> usize {
        let pos = self.cursors.get_last_cursor_text_pos(text_buffer);
        let mut delta = 1;
        if pos.row > 0 && pos.row < self._line_geometry.len() {
            let mut scan = pos.row - 1;
            while scan >0 {
                if !self._line_geometry[scan].was_folded {
                    delta = pos.row - scan;
                    break;
                }
                scan -= 1;
            }
        };
        delta
    }

    fn compute_next_unfolded_line_down(&self, text_buffer: &TextBuffer) -> usize {
        let pos = self.cursors.get_last_cursor_text_pos(text_buffer);
        let mut delta = 1;
        let mut scan = pos.row + 1;
        while scan < self._line_geometry.len() {
            if !self._line_geometry[scan].was_folded {
                delta = scan - pos.row;
                break;
            }
            scan += 1;
        }
        delta
    }


    fn compute_focussed_line_for_folding(&self, cx: &Cx, text_buffer: &TextBuffer) -> usize {
        let scroll = self.view.get_scroll_pos(cx);
        let rect = self.view.get_view_area(cx).get_rect(cx);

        // first try if our last cursor is in view
        let pos = self.cursors.get_last_cursor_text_pos(text_buffer);
        if pos.row < self._line_geometry.len() {
            let geom = &self._line_geometry[pos.row];
            // check if cursor is visible
            if geom.walk.y - scroll.y > 0. && geom.walk.y - scroll.y <rect.h { // visible
                //println!("FOUND");
                return pos.row
            }
        }

        // scan for the centerline otherwise
        let scroll = self.view.get_scroll_pos(cx);
        let center_y = rect.h * 0.5 + scroll.y;
        for (line, geom) in self._line_geometry.iter().enumerate() {
            if geom.walk.y > center_y {
                //println!("CENTER");
                return line
            }
        }

        // if we cant find the centerline, use the view top
        for (line, geom) in self._line_geometry.iter().enumerate() {
            if geom.walk.y > scroll.y {
                //println!("TOP");
                return line
            }
        }

        // cant find anything
        return 0
    }


}

#[derive(Clone)]
pub enum AnimFoldingState {
    Open,
    Opening(f32, f32, f32),
    Folded,
    Folding(f32, f32, f32)
}

#[derive(Clone)]
pub struct AnimFolding {
    pub state: AnimFoldingState,
    pub focussed_line: usize,
    pub did_animate: bool
}

#[derive(Clone)]
pub struct AnimSelect {
    pub ypos: f32,
    pub invert: bool,
    pub time: f64
}

#[derive(Clone, Default)]
pub struct LineGeom {
    walk: Vec2,
    was_folded: bool,
    font_size: f32,
    indent_id: f32
}

#[derive(Clone, Default)]
pub struct SelectScroll {
    // pub margin:Margin,
    pub delta: Vec2,
    pub abs: Vec2,
    pub at_end: bool
}

#[derive(Clone)]
pub struct ParenItem {
    pair_start: usize,
    geom_open: Option<Rect>,
    geom_close: Option<Rect>,
    marked: bool,
    exp_paren: char
}

impl AnimFoldingState {
    fn is_animating(&self) -> bool {
        match self {
            AnimFoldingState::Open => false,
            AnimFoldingState::Folded => false,
            _ => true
        }
    }

    fn is_folded(&self) -> bool {
        match self {
            AnimFoldingState::Folded => true,
            AnimFoldingState::Folding(_, _, _) => true,
            _ => false
        }
    }

    fn get_font_size(&self, open_size: f32, folded_size: f32) -> f32 {
        match self {
            AnimFoldingState::Open => open_size,
            AnimFoldingState::Folded => folded_size,
            AnimFoldingState::Opening(f, _, _) => f * folded_size + (1. - f) * open_size,
            AnimFoldingState::Folding(f, _, _) => f * open_size + (1. - f) * folded_size,
        }
    }

    fn do_folding(&mut self, speed: f32, speed2: f32) {
        *self = match self {
            AnimFoldingState::Open => AnimFoldingState::Folding(1.0, speed, speed2),
            AnimFoldingState::Folded => AnimFoldingState::Folded,
            AnimFoldingState::Opening(f, _, _) => AnimFoldingState::Folding(1.0 - *f, speed, speed2),
            AnimFoldingState::Folding(f, _, _) => AnimFoldingState::Folding(*f, speed, speed2),
        }
    }

    fn do_opening(&mut self, speed: f32, speed2: f32) {
        *self = match self {
            AnimFoldingState::Open => AnimFoldingState::Open,
            AnimFoldingState::Folded => AnimFoldingState::Opening(1.0, speed, speed2),
            AnimFoldingState::Opening(f, _, _) => AnimFoldingState::Opening(*f, speed, speed2),
            AnimFoldingState::Folding(f, _, _) => AnimFoldingState::Opening(1.0 - *f, speed, speed2),
        }
    }

    fn next_anim_step(&mut self) {
        *self = match self {
            AnimFoldingState::Open => AnimFoldingState::Open,
            AnimFoldingState::Folded => AnimFoldingState::Folded,
            AnimFoldingState::Opening(f, speed, speed2) => {
                let new_f = *f * *speed;
                if new_f < 0.001 {
                    AnimFoldingState::Open
                }
                else {
                    AnimFoldingState::Opening(new_f, *speed * *speed2, *speed2)
                }
            },
            AnimFoldingState::Folding(f, speed, speed2) => {
                let new_f = *f * *speed;
                if new_f < 0.001 {
                    AnimFoldingState::Folded
                }
                else {
                    AnimFoldingState::Folding(new_f, *speed * *speed2, *speed2)
                }
            },
        }
    }
}

