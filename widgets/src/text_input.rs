use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    },
    unicode_segmentation::{GraphemeCursor, UnicodeSegmentation},
};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    DrawLabel = {{DrawLabel}} {}
    
    pub TextInputBase = {{TextInput}} {}
    
    pub TextInput = <TextInputBase> {
        width: 200, height: Fit,
        padding: <THEME_MSPACE_2> {}
        
        label_align: {y: 0.}
        clip_x: false,
        clip_y: false,
        
        cursor_width: 2.0,
        
        is_read_only: false,
        is_numeric_only: false,
        empty_message: "0",
         
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)

            uniform color_dither: 1.0

            color: (THEME_COLOR_INSET_DEFAULT)
            uniform color_hover: (THEME_COLOR_INSET_DEFAULT)
            uniform color_focus: (THEME_COLOR_CTRL_ACTIVE)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let grad_top = 5.0;
                let grad_bot = 1.5;
                
                sdf.box(
                    1.,
                    1.,
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
                    ), self.border_size)

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
                )
                
                return sdf.result
            }
        }
        
        draw_text: {
            instance hover: 0.0
            instance focus: 0.0

            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_hover: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_focus: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
            uniform color_empty_focus: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)

            wrap: Word,

            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }

            fn get_color(self) -> vec4 {
                return
                mix(
                    mix(
                        mix(self.color, self.color_hover, self.hover),
                        self.color_focus,
                        self.focus
                    ),
                    mix(self.color_empty, self.color_empty_focus, self.hover),
                    self.is_empty
                )
            }
        }
        
        draw_cursor: {
            instance focus: 0.0
            uniform border_radius: 0.5
            uniform color: (THEME_COLOR_TEXT_CURSOR)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(
                    mix(THEME_COLOR_U_HIDDEN, self.color, self.focus)
                );
                return sdf.result
            }
        }
        
        draw_highlight: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

            uniform color: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            uniform color_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
            uniform color_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

            fn pixel(self) -> vec4 {
                //return mix(#f00,#0f0,self.pos.y)
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.fill(
                    mix(
                        mix(self.color, self.color_hover, self.hover),
                        mix(self.color_focus, self.color_hover, self.hover),
                        self.focus)
                    ); // Pad color
                    return sdf.result
                }
            }

            animator: {
                hover = {
                    default: off
                    off = {
                        from: {all: Forward {duration: 0.1}}
                        apply: {
                            draw_bg: { hover: 0.0 }
                            draw_text: { hover: 0.0 },
                            draw_highlight: { hover: 0.0 }
                        }
                    }
                    on = {
                        from: {all: Snap}
                        apply: {
                            draw_bg: { hover: 1.0 }
                            draw_text: {hover: 1.0},
                            draw_highlight: {hover: 1.0}
                        }
                    }
                }
                focus = {
                    default: off
                    off = {
                        from: {all: Forward {duration: .25}}
                        apply: {
                            draw_bg: {focus: 0.0},
                            draw_text: {focus: 0.0},
                            draw_cursor: {focus: 0.0},
                            draw_highlight: {focus: 0.0}
                        }
                    }
                    on = {
                        from: {all: Snap}
                        apply: {
                            draw_bg: {focus: 1.0},
                            draw_text: {focus: 1.0}
                            draw_cursor: {focus: 1.0},
                            draw_highlight: {focus: 1.0}
                        }
                    }
                }
            }

        }
    

    pub TextInputGradientX = <TextInput> {
        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_DEFAULT)
            uniform color_1_hover: (THEME_COLOR_INSET_DEFAULT)
            uniform color_1_focus: (THEME_COLOR_CTRL_ACTIVE)

            uniform color_2: #f00
            uniform color_2_hover: #0ff
            uniform color_2_focus: #0f0

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let grad_top = 5.0;
                let grad_bot = 1.5;
                
                sdf.box(
                    1.,
                    1.,
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
                    ), self.border_size)

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

        draw_highlight: {
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
                //return mix(#f00,#0f0,self.pos.y)
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
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
                    ); // Pad color
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

            uniform color_2: (THEME_COLOR_INSET_DEFAULT)
            uniform color_2_hover: #4
            uniform color_2_focus: (THEME_COLOR_CTRL_ACTIVE)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let grad_top = 5.0;
                let grad_bot = 1.5;
                
                sdf.box(
                    1.,
                    1.,
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
                    ), self.border_size)

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

        draw_highlight: {
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
                //return mix(#f00,#0f0,self.pos.y)
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.,
                    0.,
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
                    ); // Pad color
                    return sdf.result
                }
            }

    }


}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawLabel {
    #[deref] draw_super: DrawText,
    #[live] is_empty: f32,
}

#[derive(Live, LiveHook, Widget)]
pub struct TextInput {
    #[animator] animator: Animator,
    
    #[redraw] #[live] draw_bg: DrawColor,
    #[live] pub draw_text: DrawLabel,
    #[live] draw_highlight: DrawQuad,
    #[live] draw_cursor: DrawQuad,
    
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] label_align: Align,

    #[live] cursor_width: f64,

    #[live] pub is_read_only: bool,
    #[live] pub is_numeric_only: bool,
    #[live] pub empty_message: String,
    #[live] pub text: String,

    #[rust] cursor: Cursor,
    #[rust] history: History,
}

impl TextInput {
    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }

    pub fn get_cursor(&self) -> &Cursor {
        &self.cursor
    }

    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }

    pub fn select_all(&mut self) {
        self.set_cursor(Cursor {
            head: IndexAffinity {
                index: self.text.len(),
                affinity: Affinity::After,
            },
            tail: IndexAffinity {
                index: 0,
                affinity: Affinity::Before,
            },
        });
    }

    pub fn filter_input(&mut self, input: String, is_replace: bool) -> String {
        if self.is_numeric_only {
            let mut dot = if is_replace {
                false
            } else {
                let before = &self.text.split_at(self.cursor.start().index).0;
                let after = &self.text.split_at(self.cursor.end().index).1;
                before.contains('.') || after.contains('.')
            };

            input.chars().filter_map(|char| {
                match char {
                    '.' | ',' if !dot => { dot = true; Some('.') },
                    char if char.is_ascii_digit() => Some(char),
                    _ => None,
                }
            }).collect()
        } else {
            input
        }
    }

    pub fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group();
    }

    fn inner_walk(&self) -> Walk {
        if self.walk.width.is_fit() {
            Walk::fit()
        } else {
            Walk::fill_fit()
        }
    }

    fn position_to_index_affinity(&self, cx: &mut Cx2d, width: f64, position: DVec2) -> IndexAffinity {
        let inner_walk = self.inner_walk();
        self.draw_text.position_to_index_affinity(
            cx,
            inner_walk,
            self.label_align,
            width,
            &self.text,
            position,
        )
    }

    fn cursor_position(&self, cx: &mut Cx2d, width: f64) -> DVec2 {
        let inner_walk = self.inner_walk();
        self.draw_text.index_affinity_to_position(
            cx,
            inner_walk,
            self.label_align,
            width,
            &self.text,
            self.cursor.head,
        )
    }

    fn move_cursor_left(&mut self, is_select: bool) {
        let Some(index) = prev_grapheme_boundary(&self.text, self.cursor.head.index) else {
            return;
        };
        self.move_cursor_to(
            IndexAffinity {
                index,
                affinity: Affinity::After,
            },
            is_select
        );
    }

    fn move_cursor_right(&mut self, is_select: bool) {
        let Some(index) = next_grapheme_boundary(&self.text, self.cursor.head.index) else {
            return;
        };
        self.move_cursor_to(
            IndexAffinity {
                index,
                affinity: Affinity::Before,
            },
            is_select
        );
    }

    fn move_cursor_up(&mut self, cx: &mut Cx2d, width: f64, is_select: bool) {
        let position = self.cursor_position(cx, width);
        let line_spacing = self.draw_text.line_spacing(cx);
        let index_affinity = self.position_to_index_affinity(cx, width, DVec2 {
            x: position.x,
            y: position.y - 0.5 * line_spacing,
        });
        self.move_cursor_to(index_affinity, is_select)
    }

    fn move_cursor_down(&mut self, cx: &mut Cx2d, width: f64, is_select: bool) {
        let position = self.cursor_position(cx, width);
        let line_spacing = self.draw_text.line_spacing(cx);
        let index_affinity = self.position_to_index_affinity(cx, width, DVec2 {
            x: position.x,
            y: position.y + 1.5 * line_spacing,
        });
        self.move_cursor_to(index_affinity, is_select);
    }

    fn move_cursor_to(&mut self, index_affinity: IndexAffinity, is_select: bool) {
        self.cursor.head = index_affinity;
        if !is_select {
            self.cursor.tail = self.cursor.head;
        }
        self.history.force_new_edit_group();
    }

    fn select_word(&mut self) {
        if self.cursor.head.index < self.cursor.tail.index { 
            self.cursor.head = IndexAffinity {
                index: self.ceil_word_boundary(self.cursor.head.index),
                affinity: Affinity::After,
            };
        } else if self.cursor.head.index > self.cursor.tail.index {
            self.cursor.head = IndexAffinity {
                index: self.floor_word_boundary(self.cursor.head.index),
                affinity: Affinity::Before,
            };
        } else {
            self.cursor.tail = IndexAffinity {
                index: self.ceil_word_boundary(self.cursor.head.index),
                affinity: Affinity::After,
            };
            self.cursor.head = IndexAffinity {
                index: self.floor_word_boundary(self.cursor.head.index),
                affinity: Affinity::Before,
            };
        }
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

    fn apply_edit(&mut self, edit: Edit) {
        self.cursor.head.index = edit.start + edit.replace_with.len();
        self.cursor.tail = self.cursor.head;
        self.history.apply_edit(edit, &mut self.text);
    }

    fn undo(&mut self) {
        if let Some(cursor) = self.history.undo(self.cursor, &mut self.text) {
            self.cursor = cursor;
        }
    }

    fn redo(&mut self) {
        if let Some(cursor) = self.history.redo(self.cursor, &mut self.text) {
            self.cursor = cursor;
        }
    }
}

impl Widget for TextInput {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let rect = self.draw_bg.area().rect(cx);
        let padded_rect = Rect {
            pos: rect.pos + self.layout.padding.left_top(),
            size: rect.size - self.layout.padding.size(),
        };

        let uid = self.widget_uid();

        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        
        match event.hit_designer(cx, self.draw_bg.area()){
            HitDesigner::DesignerPick(_e)=>{
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _=>()
        }
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
                self.force_new_edit_group();
                // TODO: Select all if necessary
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocus);
            },
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                cx.hide_text_ime();
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocusLost);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                modifiers: KeyModifiers {
                    shift: is_select,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_left(is_select);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                modifiers: KeyModifiers {
                    shift: is_select,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                self.move_cursor_right( is_select);
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowUp,
                modifiers: KeyModifiers {
                    shift: is_select,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                let event = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                self.move_cursor_up(&mut cx, padded_rect.size.x, is_select);
                self.draw_bg.redraw(&mut cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowDown,
                modifiers: KeyModifiers {
                    shift: is_select,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                let event = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                self.move_cursor_down(&mut cx, padded_rect.size.x, is_select);
                self.draw_bg.redraw(&mut cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Home,
                ..
            }) => {
                self.move_cursor_to(
                    IndexAffinity {
                        index: 0,
                        affinity: Affinity::Before,
                    },
                    false
                );
                self.history.force_new_edit_group();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::End,
                ..
            }) => {
                self.move_cursor_to(
                    IndexAffinity {
                        index: self.text.len(),
                        affinity: Affinity::After,
                    },
                    false
                );
                self.history.force_new_edit_group();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: false,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) => {
                cx.hide_text_ime();
                cx.widget_action(uid, &scope.path, TextInputAction::Return(self.text.clone()));
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers {
                    shift: true,
                    logo: false,
                    alt: false,
                    control: false
                },
                ..
            }) if !self.is_read_only => {
                self.history.create_or_extend_edit_group(
                    EditKind::Other,
                    self.cursor,
                );
                self.apply_edit(Edit {
                    start: self.cursor.start().index,
                    end: self.cursor.end().index,
                    replace_with: "\n".to_string(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                cx.widget_action(uid, &scope.path, TextInputAction::Escape);
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) if !self.is_read_only => {
                let mut start = self.cursor.start().index;
                let end = self.cursor.end().index;
                if start == end {
                    start = prev_grapheme_boundary(&self.text, start).unwrap_or(0);
                }
                self.history.create_or_extend_edit_group(EditKind::Backspace, self.cursor);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) if !self.is_read_only => {
                let start = self.cursor.start().index;
                let mut end = self.cursor.end().index;
                if start == end {
                    end = next_grapheme_boundary(&self.text, end).unwrap_or(self.text.len());
                }
                self.history.create_or_extend_edit_group(EditKind::Delete, self.cursor);
                self.apply_edit(Edit {
                    start,
                    end,
                    replace_with: String::new(),
                });
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers: KeyModifiers {
                    control: true,
                    ..
                },
                ..
            }) | Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers: KeyModifiers {
                    logo: true,
                    ..
                },
                ..
            }) => {
                self.select_all();
                self.draw_bg.redraw(cx);
            },
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers,
                ..
            }) if modifiers.is_primary() && !modifiers.shift && !self.is_read_only => {
                self.undo();
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers,
                ..
            }) if modifiers.is_primary() && modifiers.shift && !self.is_read_only => {
                self.redo();
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
            }
            Hit::KeyDown(ke)=>{
                cx.widget_action(uid, &scope.path, 
                TextInputAction::KeyDownUnhandled(ke));
            }
            Hit::TextInput(TextInputEvent {
                input,
                replace_last,
                was_paste,
            }) if !self.is_read_only => {
                let input = self.filter_input(input, false);
                if !input.is_empty() {
                    let mut start = self.cursor.start().index;
                    let end = self.cursor.end().index;
                    if replace_last {
                        start -= self.history.last_inserted_text(&self.text).map_or(0, |text| text.len());
                    }
                    self.history.create_or_extend_edit_group(
                        if replace_last || was_paste {
                            EditKind::Other
                        } else {
                            EditKind::Insert
                        },
                        self.cursor,
                    );
                    self.apply_edit(Edit {
                        start,
                        end,
                        replace_with: input,
                    });
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
                }
            }
            Hit::TextCopy(event) => {
                let highlight = &self.text[self.cursor.start().index..self.cursor.end().index];
                *event.response.borrow_mut() = Some(highlight.to_string());
            }
            Hit::TextCut(event) => {
                let highlight = &self.text[self.cursor.start().index..self.cursor.end().index];
                *event.response.borrow_mut() = Some(highlight.to_string());
                if !highlight.is_empty() {
                    self.history.create_or_extend_edit_group(EditKind::Other, self.cursor);
                    self.apply_edit(Edit {
                        start: self.cursor.start().index,
                        end: self.cursor.end().index,
                        replace_with: String::new(),
                    });
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, &scope.path, TextInputAction::Change(self.text.clone()));
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(FingerDownEvent {
                abs,
                tap_count,
                device,
                ..
            }) => {
                let event = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                let index_affinity = self.position_to_index_affinity(
                    &mut cx,
                    padded_rect.size.x,
                    abs - padded_rect.pos
                );
                self.move_cursor_to(index_affinity, false);
                if device.is_primary_hit() {
                    if tap_count == 2 {
                        self.select_word();
                    } else if tap_count == 3 {
                        self.select_all();
                    }
                    self.set_key_focus(&mut *cx);
                }
                self.draw_bg.redraw(&mut *cx);
            }
            Hit::FingerMove(FingerMoveEvent {
                abs,
                tap_count,
                ..
            }) => {
                let event: DrawEvent = DrawEvent::default();
                let mut cx = Cx2d::new(cx, &event);
                let index_affinity = self.position_to_index_affinity(
                    &mut cx,
                    padded_rect.size.x,
                    abs - padded_rect.pos
                );
                self.move_cursor_to(index_affinity, true);
                if tap_count == 2 {
                    self.select_word();
                } else if tap_count == 3 {
                    self.select_all();
                }
                self.draw_bg.redraw(&mut *cx);
            }
            _ => {}
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);

        self.draw_highlight.append_to_draw_call(cx);

        let inner_walk = self.inner_walk();

        // Draw text
        if self.text.is_empty() {
            self.draw_text.is_empty = 1.0;
            // Always draw non-secret text when displaying an empty message.
            let was_secret = self.draw_text.text_style.is_secret;
            self.draw_text.text_style.is_secret = false;
            self.draw_text.draw_walk(
                cx,
                inner_walk,
                self.label_align,
                &self.empty_message,
            );
            self.draw_text.text_style.is_secret = was_secret;
        } else {
            self.draw_text.is_empty = 0.0;
            self.draw_text.draw_walk(
                cx,
                inner_walk,
                self.label_align,
                &self.text,
            );
        }

        let padded_rect = cx.turtle().padded_rect();

        // Draw highlight
        let rects = self.draw_text.selected_rects(
            cx,
            inner_walk,
            self.label_align,
            padded_rect.size.x,
            &self.text,
            self.cursor.head.min(self.cursor.tail),
            self.cursor.head.max(self.cursor.tail)
        );
        for rect in rects {
            self.draw_highlight.draw_abs(cx, Rect {
                pos: padded_rect.pos + rect.pos,
                size: rect.size,
            });
        }
     
        // Draw cursor
        let cursor_position = self.cursor_position(cx, padded_rect.size.x);
        let cursor_height = self.draw_text.line_height(cx);
        self.draw_cursor.draw_abs(cx, Rect {
            pos: padded_rect.pos + dvec2(cursor_position.x - 0.5 * self.cursor_width, cursor_position.y),
            size: dvec2(self.cursor_width, cursor_height)
        });

        self.draw_bg.end(cx);

        if cx.has_key_focus(self.draw_bg.area()) {
            let padding = dvec2(self.layout.padding.left, self.layout.padding.top);
            cx.show_text_ime(
                self.draw_bg.area(), 
                padding + cursor_position - self.cursor_width * 0.5
            );
        }

        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());

        DrawStep::done()
    }
    
    fn text(&self) -> String {
        self.text.to_string()
    }
    
    fn set_text(&mut self, cx:&mut Cx, text: &str) {
        if self.text == text {
            return;
        }
        self.text = self.filter_input(text.to_string(), true);
        self.cursor.head.index = self.cursor.head.index.min(text.len());
        self.cursor.tail.index = self.cursor.tail.index.min(text.len());
        self.history.clear();
        self.redraw(cx);
    }
}

/// The saved (checkpointed) state of a text input widget.
#[derive(Clone, Debug, Default)]
pub struct TextInputState {
    pub text: String,
    pub cursor: Cursor,
    history: History,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    pub head: IndexAffinity,
    pub tail: IndexAffinity,
}

impl Cursor {
    pub fn start(&self) -> IndexAffinity {
        self.head.min(self.tail)
    }
    
    pub fn end(&self) -> IndexAffinity {
        self.head.max(self.tail)
    }
}

#[derive(Clone, Debug, Default)]
struct History {
    current_edit_kind: Option<EditKind>,
    undo_stack: EditStack,
    redo_stack: EditStack,
}

impl History {
    pub fn last_inserted_text<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.undo_stack.edits.last().map(|edit| &text[edit.start..edit.end])
    }
    
    pub fn force_new_edit_group(&mut self) {
        self.current_edit_kind = None;
    }
    
    pub fn create_or_extend_edit_group(
        &mut self,
        edit_kind: EditKind,
        cursor: Cursor,
    ) {
        if !self
        .current_edit_kind
        .map_or(false, |current_edit_kind| current_edit_kind.can_merge_with(edit_kind))
        {
            self.undo_stack.push_edit_group(cursor);
            self.current_edit_kind = Some(edit_kind);
        }
    }
    
    fn apply_edit(&mut self, edit: Edit, text: &mut String) {
        let inverted_edit = edit.invert(&text);
        edit.apply(text);
        self.undo_stack.push_edit(inverted_edit);
        self.redo_stack.clear();
    }
    
    pub fn undo(
        &mut self,
        cursor: Cursor,
        text: &mut String,
    ) -> Option<Cursor> {
        let mut edits = Vec::new();
        if let Some(new_cursor) = self.undo_stack.pop_edit_group(&mut edits) {
            self.redo_stack.push_edit_group(cursor);
            for edit in edits {
                let inverted_edit = edit.clone().invert(text);
                edit.apply(text);
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_cursor)
        } else {
            None
        }
    }
    
    pub fn redo(
        &mut self,
        cursor: Cursor,
        text: &mut String,
    ) -> Option<Cursor> {
        let mut edits = Vec::new();
        if let Some(new_cursor) = self.redo_stack.pop_edit_group(&mut edits) {
            self.undo_stack.push_edit_group(cursor);
            for edit in edits {
                let inverted_edit = edit.clone().invert(text);
                edit.apply(text);
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_cursor)
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

#[derive(Clone, Debug, Default)]
struct EditStack {
    edit_groups: Vec<EditGroup>,
    edits: Vec<Edit>,
}

impl EditStack {
    fn push_edit_group(&mut self, cursor: Cursor) {
        self.edit_groups.push(EditGroup {
            cursor,
            edit_start: self.edits.len(),
        });
    }
    
    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }
    
    fn pop_edit_group(&mut self, edits: &mut Vec<Edit>) -> Option<Cursor> {
        match self.edit_groups.pop() {
            Some(edit_group) => {
                edits.extend(self.edits.drain(edit_group.edit_start..).rev());
                Some(edit_group.cursor)
            }
            None => None,
        }
    }
    
    fn clear(&mut self) {
        self.edit_groups.clear();
        self.edits.clear();
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

#[derive(Clone, Copy, Debug)]
struct EditGroup {
    cursor: Cursor,
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

fn next_grapheme_boundary(string: &str, index: usize) -> Option<usize> {
    let mut cursor = GraphemeCursor::new(index, string.len(), true);
    cursor.next_boundary(string, 0).unwrap()
}

fn prev_grapheme_boundary(string: &str, index: usize) -> Option<usize> {
    let mut cursor = GraphemeCursor::new(index, string.len(), true);
    cursor.prev_boundary(string, 0).unwrap()
}

#[derive(Clone, Debug, PartialEq, DefaultNone)]
pub enum TextInputAction {
    Change(String),
    Return(String),
    KeyDownUnhandled(KeyEvent),
    Escape,
    KeyFocus,
    KeyFocusLost,
    None
}

impl TextInputRef {
    pub fn changed(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::Change(val) = action{
                return Some(val);
            }
        }
        None
    }
    
    pub fn key_down_unhandled(&self, actions: &Actions) -> Option<KeyEvent> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::KeyDownUnhandled(val) = action{
                return Some(val);
            }
        }
        None
    }

    pub fn returned(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::Return(val) = action{
                return Some(val);
            }
        }
        None
    }
    
    pub fn escape(&self, actions: &Actions) -> bool {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()){
            if let TextInputAction::Escape = action {
                return true;
            }
        }
        false
    }
    
    pub fn set_cursor(&self, head: usize, tail: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_cursor(Cursor {
                head: IndexAffinity {
                    index: head,
                    affinity: Affinity::After,
                },
                tail: IndexAffinity {
                    index: tail,
                    affinity: Affinity::Before,
                },
            });
        }
    }

    pub fn set_key_focus(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow() {
            inner.set_key_focus(cx);
        }
    }

    /// Saves the internal state of this text input widget
    /// to a new `TextInputState` object.
    pub fn save_state(&self) -> TextInputState {
        if let Some(inner) = self.borrow() {
            TextInputState {
                text: inner.text.clone(),
                cursor: inner.cursor,
                history: inner.history.clone(),
            }
        } else {
            TextInputState::default()
        }
    }

    /// Restores the internal state of this text input widget
    /// from the given `TextInputState` object.
    pub fn restore_state(&self, state: TextInputState) {
        if let Some(mut inner) = self.borrow_mut() {
            // Don't use `set_text()` here, as it has other side effects.
            inner.text = state.text;
            inner.cursor = state.cursor;
            inner.history = state.history;
        }
    }

}
