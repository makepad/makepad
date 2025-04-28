use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::{
            text::{
                geom::Point,
                selection::{
                    Cursor,
                    CursorPosition,
                    Selection
                },
                layouter::LaidoutText,
            },
            *
        },
        widget::*,
    },
    std::rc::Rc,
    unicode_segmentation::{GraphemeCursor, UnicodeSegmentation},
};


live_design! {
    link widgets;

    use link::theme::*;
    use makepad_draw::shader::std::*;

    DrawMaybeEmptyText = {{DrawMaybeEmptyText}} {}

    pub TextInputBase = {{TextInput}} {}
    
    pub TextInput = <TextInputBase> {
        width: 200,
        height: Fit,
        padding: <THEME_MSPACE_2> {}

        is_password: false,
        is_read_only: false,
        is_numeric_only: false
        empty_text: "Your text here",
        
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)

            uniform color_dither: 1.0

            color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET)
            uniform color_focus: (THEME_COLOR_OUTSET_ACTIVE)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)

            color: (THEME_COLOR_INSET)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                
                sdf.box(
                    1.0,
                    1.0,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                );

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                        self.focus
                    ),
                    self.border_size
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            self.color,
                            self.color_hover,
                            self.hover
                        ),
                        self.color_focus,
                        self.focus
                    )
                );
                
                return sdf.result;
            }
        }

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0

            color: (THEME_COLOR_TEXT)
            uniform color_hover: (THEME_COLOR_TEXT)
            uniform color_focus: (THEME_COLOR_TEXT)
            uniform color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
            uniform color_empty_focus: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)

            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(self.color, self.color_hover, self.hover),
                        self.color_focus,
                        self.focus
                    ),
                    mix(self.color_empty, self.color_empty_focus, self.hover),
                    self.is_empty
                );
            }
        }

        draw_selection: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

            uniform color: (THEME_COLOR_D_HIDDEN)
            uniform color_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
            uniform color_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(
                    mix(
                        mix(self.color, self.color_hover, self.hover),
                        mix(self.color_focus, self.color_hover, self.hover),
                        self.focus
                    )
                );
                return sdf.result;
            }
        }

        draw_cursor: {
            instance focus: 0.0

            uniform border_radius: 0.5

            uniform blink: 0.0
            uniform color: (THEME_COLOR_TEXT_CURSOR)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(
                    mix(THEME_COLOR_U_HIDDEN, self.color, (1.0-self.blink) * self.focus)
                );
                return sdf.result;
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
            hover = {
                default: off
                off = {
                    from: {
                        all: Forward { duration: 0.1 }
                    }
                    apply: {
                        draw_bg: { hover: 0.0 }
                        draw_text: { hover: 0.0 },
                        draw_selection: { hover: 0.0 }
                    }
                }
                on = {
                    from: { all: Snap }
                    apply: {
                        draw_bg: { hover: 0.0 }
                        draw_text: { hover: 1.0 },
                        draw_selection: { hover: 1.0 }
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {
                        all: Forward { duration: 0.25 }
                    }
                    apply: {
                        draw_bg: { focus: 0.0 }
                        draw_text: { focus: 0.0 },
                        draw_cursor: { focus: 0.0 },
                        draw_selection: { focus: 0.0 }
                    }
                }
                on = {
                    from: { all: Snap }
                    apply: {
                        draw_bg: { focus: 1.0 }
                        draw_text: { focus: 1.0 }
                        draw_cursor: { focus: 1.0 },
                        draw_selection: { focus: 1.0 }
                    }
                }
            }
        }
    }

    pub TextInputFlat = <TextInput> {
        draw_bg: {
            border_radius: (THEME_CORNER_RADIUS)
            border_size: (THEME_BEVELING)

            color_dither: 1.0

            color: (THEME_COLOR_INSET)
            color_hover: (THEME_COLOR_INSET_HOVER)
            color_focus: (THEME_COLOR_INSET_FOCUS)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
        }
    }

    pub TextInputFlatter = <TextInput> {
        draw_bg: {
            border_radius: (THEME_CORNER_RADIUS)
            border_size: 0.

            color: (THEME_COLOR_INSET)
            color_hover: (THEME_COLOR_INSET_HOVER)
            color_focus: (THEME_COLOR_INSET_FOCUS)
        }
    }

    pub TextInputGradientX = <TextInput> {
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET)
            uniform color_1_hover: (THEME_COLOR_INSET)
            uniform color_1_focus: (THEME_COLOR_OUTSET_ACTIVE)

            uniform color_2: (THEME_COLOR_INSET * 2.5)
            uniform color_2_hover: (THEME_COLOR_INSET * 3.0)
            uniform color_2_focus: (THEME_COLOR_INSET * 4.0)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                
                sdf.box(
                    1.0,
                    1.0,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                );

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                        self.focus
                    ),
                    self.border_size
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.x + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x + dither),
                            self.hover
                        ),
                        mix(self.color_1_focus, self.color_2_focus, self.pos.x + dither),
                        self.focus
                    )
                );
                
                return sdf.result
            }
        }

        draw_selection: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

            uniform color_1: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_1_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_1_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE)

            uniform color_2: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_2_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
            uniform color_2_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                sdf.box(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.x),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus, self.color_2_focus, self.pos.x),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x),
                            self.hover
                        ),
                        self.focus
                    )
                );

                return sdf.result
            }
        }
    }

    pub TextInputGradientY = <TextInput> {
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)

            uniform color_dither: 1.0

            uniform color_1: #3
            uniform color_1_hover: #3
            uniform color_1_focus: #2

            uniform color_2: (THEME_COLOR_INSET)
            uniform color_2_hover: #4
            uniform color_2_focus: (THEME_COLOR_OUTSET_ACTIVE)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                
                sdf.box(
                    1.0,
                    1.0,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                        self.focus
                    ), 
                    self.border_size
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                        self.focus
                    )
                );
                
                return sdf.result
            }
        }

        draw_selection: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

            uniform color_1: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_1_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
            uniform color_1_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

            uniform color_2: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_2_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_2_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                sdf.box(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                            self.hover
                        ),
                        self.focus
                    )
                );
                return sdf.result
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput {
    #[animator] animator: Animator,

    #[redraw] #[live] draw_bg: DrawColor,
    #[live] draw_text: DrawMaybeEmptyText,
    #[live] draw_selection: DrawQuad,
    #[live] draw_cursor: DrawQuad,

    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] label_align: Align,

    #[live] is_password: bool,
    #[live] is_read_only: bool,
    #[live] is_numeric_only: bool,
    #[live] empty_text: String,
    #[live] text: String,
    #[live(0.5)] blink_speed: f64,

    #[rust] password_text: String,
    #[rust] laidout_text: Option<Rc<LaidoutText>>,
    #[rust] text_area: Area,
    #[rust] selection: Selection,
    #[rust] history: History,
    #[rust] blink_timer: Timer,
}

impl TextInput {
    pub fn is_password(&self) -> bool {
        self.is_password
    }

    pub fn set_is_password(&mut self, cx: &mut Cx, is_password: bool) {
        self.is_password = is_password;
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
    }

    pub fn toggle_is_password(&mut self, cx: &mut Cx) {
        self.set_is_password(cx, !self.is_password);
    }

    pub fn is_read_only(&self) -> bool {
        self.is_read_only
    }

    pub fn set_is_read_only(&mut self, cx: &mut Cx, is_read_only: bool) {
        self.is_read_only = is_read_only;
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
    }

    pub fn toggle_is_read_only(&mut self, cx: &mut Cx) {
        self.set_is_read_only(cx, !self.is_read_only);
    }

    pub fn is_numeric_only(&self) -> bool {
        self.is_numeric_only
    }

    pub fn set_is_numeric_only(&mut self, cx: &mut Cx, is_numeric_only: bool) {
        self.is_numeric_only = is_numeric_only;
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
    }

    pub fn toggle_is_numeric_only(&mut self, cx: &mut Cx) {
        self.set_is_numeric_only(cx, !self.is_numeric_only);
    }

    pub fn empty_text(&self) -> &str {
        &self.empty_text
    }

    pub fn set_empty_text(&mut self, cx: &mut Cx, empty_text: String) {
        self.empty_text = empty_text;
        if self.text.is_empty() {
            self.draw_bg.redraw(cx);
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, cx: &mut Cx, text: String) {
        self.text = self.filter_input(text, true);
        self.set_selection(
            cx,
            Selection {
                anchor: Cursor {
                    index: self.selection.anchor.index.min(self.text.len()),
                    prefer_next_row: self.selection.anchor.prefer_next_row,
                },
                cursor: Cursor {
                    index: self.selection.cursor.index.min(self.text.len()),
                    prefer_next_row: self.selection.cursor.prefer_next_row,
                }
            }
        );
        self.history.clear();
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn set_selection(&mut self, cx: &mut Cx, selection: Selection) {
        self.selection = selection;
        self.history.force_new_edit_group();
        self.reset_blink_timer(cx);
        self.draw_bg.redraw(cx);
    }

    pub fn cursor(&self) -> Cursor {
        self.selection.cursor
    }

    pub fn set_cursor(&mut self, cx: &mut Cx, cursor: Cursor, keep_selection: bool) {
        self.set_selection(
            cx,
            Selection {
                anchor: if keep_selection {
                    self.selection.anchor
                } else {
                    cursor
                },
                cursor
            }
        );
    }
    
    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.start().index..self.selection.end().index]
    }

    pub fn reset_blink_timer(&mut self, cx: &mut Cx) {
        self.animator_cut(cx, id!(blink.off));
        if !self.is_read_only {
            cx.stop_timer(self.blink_timer);
            self.blink_timer = cx.start_timeout(self.blink_speed)
        }
    }

    fn cursor_to_position(&self, cursor: Cursor) -> CursorPosition {
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = laidout_text.cursor_to_position(self.cursor_to_password_cursor(cursor));
        CursorPosition {
            row_index: position.row_index,
            x_in_lpxs: position.x_in_lpxs * self.draw_text.font_scale,
        }
    }

    fn point_in_lpxs_to_cursor(&self, point_in_lpxs: Point<f32>) -> Cursor {
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let cursor = laidout_text.point_in_lpxs_to_cursor(point_in_lpxs / self.draw_text.font_scale);
        self.password_cursor_to_cursor(cursor)
    }

    fn position_to_cursor(&self, position: CursorPosition) -> Cursor {
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let cursor = laidout_text.position_to_cursor(CursorPosition {
            row_index: position.row_index,
            x_in_lpxs: position.x_in_lpxs / self.draw_text.font_scale,
        });
        self.password_cursor_to_cursor(cursor)
    }

    fn selection_to_password_selection(&self, selection: Selection) -> Selection {
        Selection {
            cursor: self.cursor_to_password_cursor(selection.cursor),
            anchor: self.cursor_to_password_cursor(selection.anchor),
        }
    }

    fn cursor_to_password_cursor(&self, cursor: Cursor) -> Cursor {
        Cursor {
            index: self.index_to_password_index(cursor.index),
            prefer_next_row: cursor.prefer_next_row,
        }
    }

    fn password_cursor_to_cursor(&self, password_cursor: Cursor) -> Cursor {
        Cursor {
            index: self.password_index_to_index(password_cursor.index),
            prefer_next_row: password_cursor.prefer_next_row,
        }
    }

    fn index_to_password_index(&self, index: usize) -> usize {
        if !self.is_password {
            return index;
        }
        let grapheme_index = self.text[..index].graphemes(true).count();
        self.password_text
            .grapheme_indices(true)
            .nth(grapheme_index).map_or(self.password_text.len(), |(index, _)| index)
    }

    fn password_index_to_index(&self, password_index: usize) -> usize {
        if !self.is_password {
            return password_index;
        }
        let grapheme_index = self.password_text[..password_index].graphemes(true).count();
        self.text
            .grapheme_indices(true)
            .nth(grapheme_index).map_or(self.text.len(), |(index, _)| index)
    }

    fn inner_walk(&self) -> Walk {
        if self.walk.width.is_fit() {
            Walk::fit()
        } else {
            Walk::fill_fit()
        }
    }

    fn layout_text(&mut self, cx: &mut Cx2d) {
        if self.laidout_text.is_some() {
            return;
        }
        let text = if self.is_password {
            self.password_text.clear();
            for grapheme in self.text.graphemes(true) {
                self.password_text.push(if grapheme == "\n" {
                    '\n'
                } else {
                    '•'
                });
            }
            &self.password_text
        } else {
            &self.text
        };
        let turtle_rect = cx.turtle().padded_rect();
        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap_width_in_lpxs = if cx.turtle().layout().flow == Flow::RightWrap {
            max_width_in_lpxs
        } else {
            None
        };
        self.laidout_text = Some(self.draw_text.layout(
            cx,
            0.0,
            0.0,
            wrap_width_in_lpxs,
            self.label_align, 
            text
        ));
    }

    fn draw_text(&mut self, cx: &mut Cx2d) -> Rect {
        let inner_walk = self.inner_walk();
        let text_rect = if self.text.is_empty() {
            self.draw_text.is_empty = 1.0;
            self.draw_text.draw_walk(
                cx,
                inner_walk,
                self.label_align,
                &self.empty_text
            )
        } else {
            self.draw_text.is_empty = 0.0;
            let laidout_text = self.laidout_text.as_ref().unwrap();
            self.draw_text.draw_walk_laidout(
                cx,
                inner_walk,
                self.label_align,
                laidout_text,
            )
        };
        cx.add_aligned_rect_area(&mut self.text_area, text_rect);
        text_rect
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d, text_rect: Rect) -> DVec2 {
        let CursorPosition {
            row_index,
            x_in_lpxs,
        } = self.cursor_to_position(self.selection.cursor);
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let row = &laidout_text.rows[row_index];
        let cursor_pos = dvec2(
            (x_in_lpxs - 1.0 * self.draw_text.font_scale) as f64,
            ((row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.draw_text.font_scale) as f64,
        );
        self.draw_cursor.draw_abs(
            cx,
            rect(
                text_rect.pos.x + cursor_pos.x,
                text_rect.pos.y + cursor_pos.y,
                (2.0 * self.draw_text.font_scale) as f64,
                ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.draw_text.font_scale) as f64,
            )
        );
        cursor_pos
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, text_rect: Rect) {
        let laidout_text = self.laidout_text.as_ref().unwrap();
        
        self.draw_selection.begin_many_instances(cx);
        for rect_in_lpxs in laidout_text.selection_rects_in_lpxs(
            self.selection_to_password_selection(self.selection)
        ) {
            self.draw_selection.draw_abs(
                cx,
                rect(
                    text_rect.pos.x + (rect_in_lpxs.origin.x * self.draw_text.font_scale) as f64,
                    text_rect.pos.y + (rect_in_lpxs.origin.y * self.draw_text.font_scale) as f64,
                    (rect_in_lpxs.size.width * self.draw_text.font_scale) as f64,
                    (rect_in_lpxs.size.height * self.draw_text.font_scale) as f64,
                )
            );
        }
        self.draw_selection.end_many_instances(cx);
    }

    pub fn move_cursor_left(&mut self, cx: &mut Cx, keep_selection: bool) {
        self.set_cursor(
            cx,
            Cursor {
                index: prev_grapheme_boundary(&self.text, self.selection.cursor.index),
                prefer_next_row: true,
            },
            keep_selection
        );
    }

    pub fn move_cursor_right(&mut self, cx: &mut Cx, keep_selection: bool) {
        self.set_cursor(
            cx,
            Cursor {
                index: next_grapheme_boundary(&self.text, self.selection.cursor.index),
                prefer_next_row: false,
            },
            keep_selection,
        );
    }

    pub fn move_cursor_up(&mut self, cx: &mut Cx, keep_selection: bool) {
        let position = self.cursor_to_position(self.selection.cursor);
        self.set_cursor(
            cx,
            self.position_to_cursor(CursorPosition {
                row_index: if position.row_index == 0 {
                    0
                } else {
                    position.row_index - 1
                },
                x_in_lpxs: position.x_in_lpxs,
            }),
            keep_selection
        );
    }

    pub fn move_cursor_down(&mut self, cx: &mut Cx, keep_selection: bool) {
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = self.cursor_to_position(self.selection.cursor);
        self.set_cursor(
            cx,
            self.position_to_cursor(CursorPosition {
                row_index: if position.row_index == laidout_text.rows.len() - 1 {
                    laidout_text.rows.len() - 1
                } else {
                    position.row_index + 1 
                },
                x_in_lpxs: position.x_in_lpxs,
            }),
            keep_selection
        );
    }

    pub fn select_all(&mut self, cx: &mut Cx) {
        self.set_selection(
            cx,
            Selection {
                anchor: Cursor { index: 0, prefer_next_row: false },
                cursor: Cursor { index: self.text.len(), prefer_next_row: false },
            }
        );
    }

    pub fn select_word(&mut self, cx: &mut Cx) {
        if self.selection.cursor.index < self.selection.anchor.index { 
            self.set_cursor(
                cx, 
                Cursor {
                    index: self.ceil_word_boundary(self.selection.cursor.index),
                    prefer_next_row: true,
                },
                true,
            );
        } else if self.selection.cursor.index > self.selection.anchor.index {
            self.set_cursor(
                cx,
                Cursor {
                    index: self.floor_word_boundary(self.selection.cursor.index),
                    prefer_next_row: false,
                },
                true,
            );
        } else {
            self.set_selection(
                cx,
                Selection {
                    anchor: Cursor {
                        index: self.ceil_word_boundary(self.selection.cursor.index),
                        prefer_next_row: true,
                    },
                    cursor: Cursor {
                        index: self.floor_word_boundary(self.selection.cursor.index),
                        prefer_next_row: false,
                    }
                },
            );
        }
    }

    pub fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group();
    }

    fn ceil_word_boundary(&self, index: usize) -> usize {
        let mut prev_word_boundary_index = 0;
        for (word_boundary_index, _) in self.text.split_word_bound_indices() {
            if word_boundary_index > index {
                return prev_word_boundary_index;
            }
            prev_word_boundary_index = word_boundary_index;
        }
        prev_word_boundary_index
    }

    fn floor_word_boundary(&self, index: usize) -> usize {
        let mut prev_word_boundary_index = self.text.len();
        for (word_boundary_index, _) in self.text.split_word_bound_indices().rev() {
            if word_boundary_index < index {
                return prev_word_boundary_index;
            }
            prev_word_boundary_index = word_boundary_index;
        }
        prev_word_boundary_index
    }

    fn filter_input(&self, input: String, is_set_text: bool) -> String {
        if self.is_numeric_only {
            let mut contains_dot = if is_set_text {
                false   
            } else {
                let before_selection = self.text[..self.selection.start().index].to_string();
                let after_selection = self.text[self.selection.end().index..].to_string();
                before_selection.contains('.') || after_selection.contains('.')
            };
            input.chars().filter(|char| {
                match char {
                    '.' | ',' if !contains_dot => {
                        contains_dot = true;
                        true
                    },
                    char => char.is_ascii_digit(),
                }
            }).collect()
        } else {
            input
        }
    }

    fn create_or_extend_edit_group(&mut self, edit_kind: EditKind) {
        self.history.create_or_extend_edit_group(edit_kind, self.selection);
    }

    fn apply_edit(&mut self, edit: Edit) {
        self.selection.cursor.index = edit.start + edit.replace_with.len();
        self.selection.anchor.index = self.selection.cursor.index;
        self.history.apply_edit(edit, &mut self.text);
        self.laidout_text = None;
    }

    fn undo(&mut self) -> bool {
        if let Some(new_selection) = self.history.undo(self.selection, &mut self.text) {
            self.laidout_text = None;
            self.selection = new_selection;
            true
        } else {
            false
        }
    }

    fn redo(&mut self) -> bool {
        if let Some(new_selection) = self.history.redo(self.selection, &mut self.text) {
            self.laidout_text = None;
            self.selection = new_selection;
            true
        } else {
            false
        }
    }
    
    fn reset_cursor_blinker(&mut self, cx: &mut Cx) {
        if self.is_read_only{
            self.animator_cut(cx, id!(blink.off));
        }
        else{
            self.animator_cut(cx, id!(blink.off));
            cx.stop_timer(self.blink_timer);
            self.blink_timer = cx.start_timeout(self.blink_speed)
        }
    }
}

impl Widget for TextInput {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_selection.append_to_draw_call(cx);
        self.layout_text(cx);
        let text_rect = self.draw_text(cx);
        let cursor_pos = self.draw_cursor(cx, text_rect);
        self.draw_selection(cx, text_rect);
        self.draw_bg.end(cx);
        if cx.has_key_focus(self.draw_bg.area()) {
            cx.show_text_ime(
                self.draw_bg.area(), 
                cursor_pos,
            );
        }
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());
        DrawStep::done()
    }
    
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        if self.blink_timer.is_event(event).is_some() {
            if self.animator_in_state(cx, id!(blink.off)) {
                self.animator_play(cx, id!(blink.on));
            } else {
                self.animator_play(cx, id!(blink.off));
            }
            self.blink_timer = cx.start_timeout(self.blink_speed)
        }

        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
                self.reset_cursor_blinker(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocus);
            },
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                self.animator_play(cx, id!(blink.on));
                cx.stop_timer(self.blink_timer);
                cx.hide_text_ime();
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocusLost);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => self.move_cursor_left(cx, keep_selection),
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => self.move_cursor_right(cx, keep_selection),
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => self.move_cursor_up(cx, keep_selection),
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => self.move_cursor_down(cx, keep_selection),
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers,
                ..
            }) if modifiers.is_primary() => self.select_all(cx),
            Hit::FingerDown(FingerDownEvent {
                abs,
                tap_count,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.set_key_focus(cx);
                let rel = abs - self.text_area.rect(cx).pos;
                self.set_cursor(
                    cx,
                    self.point_in_lpxs_to_cursor(
                    Point::new(rel.x as f32, rel.y as f32)
                    ),
                    false
                );
                match tap_count {
                    2 => self.select_word(cx),
                    3 => self.select_all(cx),
                    _ => {}
                }
            }
            Hit::FingerMove(FingerMoveEvent {
                abs,
                tap_count,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.set_key_focus(cx);
                let rel = abs - self.text_area.rect(cx).pos;
                self.set_cursor(
                    cx,
                    self.point_in_lpxs_to_cursor(
                    Point::new(rel.x as f32, rel.y as f32)
                    ),
                    true
                );
                match tap_count {
                    2 => self.select_word(cx),
                    3 => self.select_all(cx),
                    _ => {}
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: false,
                    ..
                },
                ..
            }) => {
                cx.hide_text_ime();
                cx.widget_action(uid, &scope.path, TextInputAction::Returned(self.text.clone()));
            },

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                cx.widget_action(uid, &scope.path, TextInputAction::Escaped);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: true,
                    ..
                },
                ..
            }) if !self.is_read_only => {
                self.create_or_extend_edit_group(EditKind::Other);
                self.apply_edit(Edit {
                    start: self.selection.start().index,
                    end: self.selection.end().index,
                    replace_with: "\n".to_string(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) if !self.is_read_only => {
                let mut start = self.selection.start().index;
                let end = self.selection.end().index;
                if start == end {
                    start = prev_grapheme_boundary(&self.text, start);
                }
                self.create_or_extend_edit_group(EditKind::Backspace);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) if !self.is_read_only => {
                let start = self.selection.start().index;
                let mut end = self.selection.end().index;
                if start == end {
                    end = next_grapheme_boundary(&self.text, end);
                }
                self.create_or_extend_edit_group(EditKind::Delete);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: modifiers @ KeyModifiers {
                    shift: false,
                    ..
                },
                ..
            }) if modifiers.is_primary() && !self.is_read_only => {
                if !self.undo() {
                    return;
                }
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: modifiers @ KeyModifiers {
                    shift: true,
                    ..
                },
                ..
            }) if modifiers.is_primary() && !self.is_read_only => {
                if !self.redo() {
                    return;
                }
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
            }
            Hit::TextInput(TextInputEvent {
                input,
                replace_last,
                was_paste,
                ..
            }) if !self.is_read_only => {
                let input = self.filter_input(input, false);
                if input.is_empty() {
                    return;
                }
                self.create_or_extend_edit_group(
                    if replace_last || was_paste {
                        EditKind::Other
                    } else {
                        EditKind::Insert
                    }
                );
                self.apply_edit(Edit {
                    start: self.selection.start().index,
                    end: self.selection.end().index,
                    replace_with: input
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
            }
            Hit::TextCopy(event) => {
                *event.response.borrow_mut() = Some(self.selected_text().to_string());
            }
            Hit::TextCut(event) => {
                *event.response.borrow_mut() = Some(self.selected_text().to_string());
                if !self.selected_text().is_empty() {
                    self.history.create_or_extend_edit_group(EditKind::Other, self.selection);
                    self.apply_edit(Edit {
                        start: self.selection.start().index,
                        end: self.selection.end().index,
                        replace_with: String::new(),
                    });
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInputAction::Changed(self.text.clone()));
                }
            }
            Hit::KeyDown(event) => {
                cx.widget_action(uid, &scope.path, TextInputAction::KeyDownUnhandled(event));
            }
            _ => {}
        }
    }
}

impl TextInputRef {
    pub fn is_password(&self) -> bool {
        if let Some(inner) = self.borrow(){
            inner.is_password()
        }
        else{
            false
        }
    }
 
    pub fn set_is_password(&self, cx: &mut Cx, is_password: bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_is_password(cx, is_password);
        }
    }
 
    pub fn toggle_is_password(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.toggle_is_password(cx);
        }
    }

    pub fn is_read_only(&self) -> bool {
        if let Some(inner) = self.borrow(){
            inner.is_read_only()
        }
        else{
            false
        }
    }

    pub fn set_is_read_only(&self, cx: &mut Cx, is_read_only: bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_is_read_only(cx, is_read_only);
        }
    }

    pub fn toggle_is_read_only(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.toggle_is_read_only(cx);
        }
    }

    pub fn is_numeric_only(&self) -> bool {
        if let Some(inner) = self.borrow(){
            inner.is_numeric_only()
        }
        else{
            false
        }
    }

    pub fn set_is_numeric_only(&self, cx: &mut Cx, is_numeric_only: bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_is_numeric_only(cx, is_numeric_only);
        }
    }

    pub fn toggle_is_numeric_only(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.toggle_is_numeric_only(cx);
        }
    }

    pub fn empty_text(&self) -> String {
        if let Some(inner) = self.borrow(){
            inner.empty_text().to_string()
        }
        else{
            String::new()
        }
    }

    pub fn set_empty_text(&self, cx: &mut Cx, empty_text: String) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_empty_text(cx, empty_text);
        }
    }

    pub fn text(&self) -> String {
        if let Some(inner) = self.borrow(){
            inner.text().to_string()
        }
        else{
            String::new()
        }
    }

    pub fn set_text(&self, cx: &mut Cx, text: String) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_text(cx, text);
        }
    }

    pub fn selection(&self) -> Selection {
        if let Some(inner) = self.borrow(){
            inner.selection()
        }
        else{
            Default::default()
        }
    }

    pub fn set_selection(&self, cx: &mut Cx, selection: Selection) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_selection(cx, selection);
        }
    }

    pub fn cursor(&self) -> Cursor {
        if let Some(inner) = self.borrow(){
            inner.cursor()
        }
        else{
            Default::default()
        }
    }

    pub fn set_cursor(&self, cx: &mut Cx, cursor: Cursor, keep_selection: bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.set_cursor(cx, cursor, keep_selection);
        }
    }

    pub fn selected_text(&self) -> String {
        if let Some(inner) = self.borrow(){
            inner.selected_text().to_string()
        }
        else{
            String::new()
        }
    }

    pub fn returned(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::Returned(text) = action{
                return Some(text);
            }
        }
        None
    }
    
    pub fn escaped(&self, actions: &Actions) -> bool {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::Escaped = action {
                return true;
            }
        }
        false
    }

    pub fn changed(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::Changed(text) = action{
                return Some(text);
            }
        }
        None
    }

    pub fn key_down_unhandled(&self, actions: &Actions) -> Option<KeyEvent> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::KeyDownUnhandled(event) = action{
                return Some(event);
            }
        }
        None
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum TextInputAction {
    None,
    KeyFocus,
    KeyFocusLost,
    Returned(String),
    Escaped,
    Changed(String),
    KeyDownUnhandled(KeyEvent),
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
struct DrawMaybeEmptyText {
    #[deref] draw_super: DrawText,
    #[live] is_empty: f32,
}

#[derive(Clone, Debug, Default)]
struct History {
    current_edit_kind: Option<EditKind>,
    undo_stack: EditStack,
    redo_stack: EditStack,
}

impl History {
    fn force_new_edit_group(&mut self) {
        self.current_edit_kind = None;
    }

    fn create_or_extend_edit_group(&mut self, edit_kind: EditKind, selection: Selection) {
        if !self.current_edit_kind.map_or(false, |current_edit_kind| current_edit_kind.can_merge_with(edit_kind)) {
            self.undo_stack.push_edit_group(selection);
            self.current_edit_kind = Some(edit_kind);
        }
    }

    fn apply_edit(&mut self, edit: Edit, text: &mut String) {
        let inverted_edit = edit.invert(&text);
        edit.apply(text);
        self.undo_stack.push_edit(inverted_edit);
        self.redo_stack.clear();
    }

    fn undo(
        &mut self,
        selection: Selection,
        text: &mut String,
    ) -> Option<Selection> {
        if let Some((new_selection, edits)) = self.undo_stack.pop_edit_group() {
            self.redo_stack.push_edit_group(selection);
            for edit in &edits {
                let inverted_edit = edit.invert(text);
                edit.apply(text);
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_selection)
        } else {
            None
        }
    }

    fn redo(
        &mut self,
        selection: Selection,
        text: &mut String,
    ) -> Option<Selection> {
        if let Some((new_selection, edits)) = self.redo_stack.pop_edit_group() {
            self.undo_stack.push_edit_group(selection);
            for edit in &edits {
                let inverted_edit = edit.invert(text);
                edit.apply(text);
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_selection)
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.current_edit_kind = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditKind {
    Insert,
    Backspace,
    Delete,
    Other,
}

impl EditKind {
    fn can_merge_with(self, other: EditKind) -> bool {
        if self == Self::Other {
            false
        } else {
            self == other
        }
    }
}

#[derive(Clone, Debug, Default)]
struct EditStack {
    edit_groups: Vec<EditGroup>,
    edits: Vec<Edit>,
}

impl EditStack {
    fn push_edit_group(&mut self, selection: Selection) {
        self.edit_groups.push(EditGroup {
            selection,
            edit_start: self.edits.len(),
        });
    }
    
    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }
    
    fn pop_edit_group(&mut self) -> Option<(Selection, Vec<Edit>)> {
        match self.edit_groups.pop() {
            Some(edit_group) => Some((
                edit_group.selection,
                self.edits.drain(edit_group.edit_start..).rev().collect()
            )),
            None => None,
        }
    }
    
    fn clear(&mut self) {
        self.edit_groups.clear();
        self.edits.clear();
    }
}

#[derive(Clone, Copy, Debug)]
struct EditGroup {
    selection: Selection,
    edit_start: usize
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    end: usize,
    replace_with: String,
}

impl Edit {
    fn apply(&self, text: &mut String) {
        text.replace_range(self.start..self.end, &self.replace_with);
    }

    fn invert(&self, text: &str) -> Self {
        Self {
            start: self.start,
            end: self.start + self.replace_with.len(),
            replace_with: text[self.start..self.end].to_string(),
        }
    }
}

fn prev_grapheme_boundary(text: &str, index: usize) -> usize {
    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.prev_boundary(text, 0).unwrap().unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, index: usize) -> usize {
    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.next_boundary(text, 0).unwrap().unwrap_or(text.len())
}