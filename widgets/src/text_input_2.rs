use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::{text::{selection::{Cursor, Selection}, layout::LaidoutText}, *},
        widget::*,
    },
    std::rc::Rc,
};


live_design! {
    link widgets;

    use link::theme::*;
    use makepad_draw::shader::std::*;

    pub TextInput2Base = {{TextInput2}} {}
    
    pub TextInput2 = <TextInput2Base> {
        width: 200,
        height: Fit,

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            wrap: Word,
            text_style: {
                font_family: <THEME_FONT_FAMILY_REGULAR> {},
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: 16.0
            }
        }

        draw_selection: {
            instance hover: 0.0
            instance focus: 1.0 // TODO: Animate this
            
            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_BG_HIGHLIGHT_INLINE, self.focus));
                return sdf.result
            }
        }

        draw_cursor: {
            instance focus: 1.0 // TODO: Animate this
            uniform border_radius: 0.5

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_TEXT_CURSOR, self.focus));
                return sdf.result
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput2 {
    #[redraw] #[live] draw_bg: DrawColor,
    #[live] draw_text: DrawText2,
    #[live] draw_selection: DrawQuad,
    #[live] draw_cursor: DrawQuad,

    #[layout] layout: Layout,
    #[walk] text_walk: Walk,
    #[live] text_align: Align,

    #[live] pub text: String,
    #[rust] laidout_text: Option<Rc<LaidoutText>>,
    #[rust] text_area: Area,
    #[rust] selection: Selection,
}

impl TextInput2 {
    fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }

    fn move_cursor_left(&mut self, keep_selection: bool) {
        use makepad_draw::text::selection::Affinity;

        self.set_cursor(
            Cursor {
                index: prev_grapheme_boundary(&self.text, self.selection.cursor.index),
                affinity: Affinity::After,
            },
            keep_selection
        );
    }

    fn move_cursor_right(&mut self, keep_selection: bool) {
        use makepad_draw::text::selection::Affinity;
        
        self.set_cursor(
            Cursor {
                index: next_grapheme_boundary(&self.text, self.selection.cursor.index),
                affinity: Affinity::Before,
            },
            keep_selection,
        );
    }

    fn move_cursor_up(&mut self, keep_selection: bool) {
        use makepad_draw::text::selection::Position;

        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = laidout_text.cursor_to_position(self.selection.cursor);
        self.set_cursor(
            laidout_text.position_to_cursor(Position {
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

    fn move_cursor_down(&mut self, keep_selection: bool) {
        use makepad_draw::text::selection::Position;
        
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = laidout_text.cursor_to_position(self.selection.cursor);
        self.set_cursor(
            laidout_text.position_to_cursor(Position {
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

    fn set_cursor(&mut self, cursor: Cursor, keep_selection: bool) {
        self.selection.cursor = cursor;
        if !keep_selection {
            self.selection.anchor = cursor;
        }
    }
}

impl Widget for TextInput2 {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        use makepad_draw::text::selection::Position;
        
        self.draw_bg.begin(cx, walk, self.layout);

        self.laidout_text = Some(self.draw_text.layout(cx, self.text_walk, self.text_align, &self.text));
        let laidout_text = self.laidout_text.as_ref().unwrap();

        let text_rect = self.draw_text.draw_walk_laidout(
            cx,
            self.text_walk,
            self.text_align,
            laidout_text,
        );
        cx.add_aligned_rect_area(&mut self.text_area, text_rect);

        let Position {
            row_index,
            x_in_lpxs,
        } = laidout_text.cursor_to_position(self.selection.cursor);
        let row = &laidout_text.rows[row_index];
        self.draw_cursor.draw_abs(
            cx,
            rect(
                text_rect.pos.x + x_in_lpxs as f64 - 2.0 / 2.0,
                text_rect.pos.y + (row.origin_in_lpxs.y - row.ascender_in_lpxs) as f64,
                2.0,
                (row.ascender_in_lpxs - row.descender_in_lpxs) as f64,
            )
        );

        for rect_in_lpxs in laidout_text.selection_rects_in_lpxs(self.selection) {
            self.draw_selection.draw_abs(
                cx,
                rect(
                    text_rect.pos.x + rect_in_lpxs.origin.x as f64,
                    text_rect.pos.y + rect_in_lpxs.origin.y as f64,
                    rect_in_lpxs.size.width as f64,
                    rect_in_lpxs.size.height as f64,
                )
            );
        }

        self.draw_bg.end(cx);

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        use makepad_draw::text::geom::Point;

        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_left(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_right(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_up(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {
                    shift: keep_selection,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_down(keep_selection);
                self.draw_bg.redraw(cx);
            },
            Hit::FingerDown(FingerDownEvent {
                abs,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.set_key_focus(cx);
                let laidout_text = self.laidout_text.as_ref().unwrap();
                let rel = abs - self.text_area.rect(cx).pos;
                self.set_cursor(laidout_text.point_in_lpxs_to_cursor(
                    Point::new(rel.x as f32, rel.y as f32)
                ), false);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(FingerMoveEvent {
                abs,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.set_key_focus(cx);
                let laidout_text = self.laidout_text.as_ref().unwrap();
                let rel = abs - self.text_area.rect(cx).pos;
                self.set_cursor(laidout_text.point_in_lpxs_to_cursor(
                    Point::new(rel.x as f32, rel.y as f32)
                ), true);
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }
}

fn prev_grapheme_boundary(text: &str, index: usize) -> usize {
    use unicode_segmentation::GraphemeCursor;

    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.prev_boundary(text, 0).unwrap().unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, index: usize) -> usize {
    use unicode_segmentation::GraphemeCursor;

    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.next_boundary(text, 0).unwrap().unwrap_or(text.len())
}