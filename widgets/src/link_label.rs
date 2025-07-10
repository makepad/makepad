use crate::{
    makepad_derive_widget::*,
    widget::*,
    makepad_draw::*,
    button::Button,
};

live_design!{
    link widgets;
    
    use link::theme::*;
    use makepad_draw::shader::std::*;
    use crate::button::ButtonBase
    
    pub LinkLabelBase = {{LinkLabel}}<ButtonBase> {}
    pub LinkLabel = <LinkLabelBase> {
        width: Fit, height: Fit,
        margin: <THEME_MSPACE_V_2> {}
        padding: 0.,
        
        label_walk: { width: Fit, height: Fit },

        draw_icon: {
            instance hover: 0.0
            instance down: 0.0
            instance focus: 0.0
            instance disabled: 0.0

            uniform color_dither: 1.0
            uniform bg_gradient_horizontal: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            uniform color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            uniform color_2_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_2_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_2_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let color_2 = self.color;
                let color_2_hover = self.color_hover;
                let color_2_down = self.color_down;
                let color_2_focus = self.color_focus;
                let color_2_disabled = self.color_disabled;

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                    color_2_hover = self.color_2_hover
                    color_2_down = self.color_2_down;
                    color_2_focus = self.color_2_focus;
                    color_2_disabled = self.color_2_disabled;
                }

                let bg_gradient_dir = self.pos.y + dither;
                if (self.bg_gradient_horizontal > 0.5) {
                    bg_gradient_dir = self.pos.x + dither;
                }

                return mix(
                    mix(
                        mix(
                            mix(
                                mix(self.color, color_2, bg_gradient_dir),
                                mix(self.color_focus, color_2_focus, bg_gradient_dir),
                                self.focus
                            ),
                            mix(self.color_hover, color_2_hover, bg_gradient_dir),
                            self.hover
                        ),
                        mix(self.color_down, color_2_down, bg_gradient_dir),
                        self.down
                    ),
                    mix(self.color_disabled, color_2_disabled, bg_gradient_dir),
                    self.disabled
                );
            }
        }
        
        draw_bg: {
            instance down: 0.0
            instance hover: 0.0
            instance focus: 0.0
            instance disabled: 0.0

            uniform color_dither: 1.0
            uniform bg_gradient_horizontal: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
            
            uniform color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            uniform color_2_hover: #F00
            uniform color_2_down: #000
            uniform color_2_focus: #f00
            uniform color_2_disabled: (THEME_COLOR_TEXT_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let offset_y = 1.0

                let color_2 = self.color;
                let color_2_hover = self.color_hover;
                let color_2_down = self.color_down;
                let color_2_focus = self.color_focus;
                let color_2_disabled = self.color_disabled;

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                    color_2_hover = self.color_2_hover
                    color_2_down = self.color_2_down;
                    color_2_focus = self.color_2_focus;
                    color_2_disabled = self.color_2_disabled;
                }

                let bg_gradient_dir = self.pos.y + dither;
                if (self.bg_gradient_horizontal > 0.5) {
                    bg_gradient_dir = self.pos.x + dither;
                }

                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);

                return sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color, color_2, bg_gradient_dir),
                                    mix(self.color_focus, color_2_focus, bg_gradient_dir),
                                    self.focus
                                ),
                                mix(self.color_hover, color_2_hover, bg_gradient_dir),
                                self.hover
                            ),
                            mix(self.color_down, color_2_down, bg_gradient_dir),
                            self.down
                        ),
                        mix(self.color_disabled, color_2_disabled, bg_gradient_dir),
                        self.disabled
                    ), mix(.7, 1., self.hover));
            }
        }
        
        draw_text: {
            instance down: 0.0
            instance hover: 0.0
            instance focus: 0.0,
            instance disabled: 0.0

            wrap: Word
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }

            uniform color_dither: 1.0
            uniform bg_gradient_horizontal: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER),
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER),
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN),
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            uniform color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            uniform color_2_hover: #FA0
            uniform color_2_down: #0A0
            uniform color_2_focus: #0F0
            uniform color_2_disabled: (THEME_COLOR_TEXT_DISABLED)

            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let color_2 = self.color;
                let color_2_hover = self.color_hover;
                let color_2_down = self.color_down;
                let color_2_focus = self.color_focus;
                let color_2_disabled = self.color_disabled;

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                    color_2_hover = self.color_2_hover
                    color_2_down = self.color_2_down;
                    color_2_focus = self.color_2_focus;
                    color_2_disabled = self.color_2_disabled;
                }

                let bg_gradient_dir = self.pos.y + dither;
                if (self.bg_gradient_horizontal > 0.5) {
                    bg_gradient_dir = self.pos.x + dither;
                }

                return
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color, color_2, bg_gradient_dir),
                                    mix(self.color_focus, color_2_focus, bg_gradient_dir),
                                    self.focus
                                ),
                                mix(self.color_hover, color_2_hover, bg_gradient_dir),
                                self.hover
                            ),
                            mix(self.color_down, color_2_down, bg_gradient_dir),
                            self.down
                        ),
                        mix(self.color_disabled, color_2_disabled, bg_gradient_dir),
                        self.disabled
                    );
            }
        }
        
        animator: {
            disabled = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_icon: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
            time = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        //draw_bg: {anim_time: 0.0}
                    }
                }
                on = {
                    from: {all: Loop {duration: 1.0, end:1000000000.0}}
                    apply: {
                        draw_bg: {anim_time: [{time: 0.0, value: 0.0},{time:1.0, value:1.0}]}
                    }
                }
            }
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_icon: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    pub LinkLabelGradientY = <LinkLabel> {
        draw_icon: {
            bg_gradient_horizontal: 0.0

            color: (THEME_COLOR_LABEL_INNER)
            color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            color_2: #0ff
            color_2_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            color_2_down: (THEME_COLOR_LABEL_INNER_DOWN)
            color_2_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            color_2_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
        }

        draw_bg: {
            color: #0ff,
            color_hover: #0ff,
            color_down: #0ff,
            color_focus: #0ff,
            color_disabled: (THEME_COLOR_TEXT_DISABLED)

            color_2: #A00
            color_2_hover: #F00
            color_2_down: #000
            color_2_focus: #f00
            color_2_disabled: (THEME_COLOR_TEXT_DISABLED)
            
        }
        
        draw_text: {
            color: #0ff,
            color_hover: #0ff,
            color_down: #0ff,
            color_focus: #f00,
            color_disabled: (THEME_COLOR_TEXT_DISABLED)

            color_2: #F00
            color_2_hover: #FA0
            color_2_down: #0A0
            color_2_focus: #0F0
            color_2_disabled: (THEME_COLOR_TEXT_DISABLED)

        }
    }
    
    pub LinkLabelGradientX = <LinkLabel> {
        draw_icon: {
            bg_gradient_horizontal: 1.0

            color: (THEME_COLOR_LABEL_INNER)
            color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            color_2: #0ff
            color_2_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            color_2_down: (THEME_COLOR_LABEL_INNER_DOWN)
            color_2_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            color_2_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
        }

        draw_bg: {
            bg_gradient_horizontal: 1.0

            color: #0ff,
            color_hover: #0ff,
            color_down: #0ff,
            color_focus: #0ff,
            color_disabled: (THEME_COLOR_TEXT_DISABLED)

            color_2: #A00
            color_2_hover: #F00
            color_2_down: #000
            color_2_focus: #f00
            color_2_disabled: (THEME_COLOR_TEXT_DISABLED)
        }

        draw_text: {
            bg_gradient_horizontal: 1.0

            color: #0ff,
            color_hover: #0ff,
            color_down: #0ff,
            color_focus: #f00,
            color_disabled: (THEME_COLOR_TEXT_DISABLED)

            color_2: #F00
            color_2_hover: #FA0
            color_2_down: #0A0
            color_2_focus: #0F0
            color_2_disabled: (THEME_COLOR_TEXT_DISABLED)
        }
    }


    pub LinkLabelIcon = <LinkLabel> {
        padding: { bottom: 2. }
        align: {x: 0.0, y: 0.0 }
        label_walk: { margin: { left: (THEME_SPACE_2) } }
    }

}

/// A clickable label widget that opens a URL when clicked.
///
/// This is a wrapper around (and derefs to) a [`Button`] widget.
#[derive(Live, LiveHook, Widget)]
pub struct LinkLabel {
    #[deref] button: Button,
    #[live] pub url: String,
    #[live] pub open_in_place: bool,
}

impl Widget for LinkLabel {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) {
        let actions = cx.capture_actions(|cx|{
            self.button.handle_event(cx, event, scope);
        });
        if self.url.len()>0 && self.clicked(&actions){
            cx.open_url(&self.url, if self.open_in_place{OpenUrlInPlace::Yes}else{OpenUrlInPlace::No});
        }
        cx.extend_actions(actions);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.button.draw_walk(cx, scope, walk)
    }
    
    fn text(&self)->String{
        self.button.text()
    }
    
    fn set_text(&mut self, cx:&mut Cx, v:&str){
        self.button.set_text(cx, v);
    }
}

impl LinkLabelRef {
    /// See [`Button::clicked()`].
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.clicked(actions))
    }

    /// See [`Button::pressed()`].
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.pressed(actions))
    }

    /// See [`Button::released()`].
    pub fn released(&self, actions: &Actions) -> bool {
        self.borrow().map_or(false, |b| b.released(actions))
    }

    /// See [`Button::clicked_modifiers()`].
    pub fn clicked_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.clicked_modifiers(actions))
    }

    /// See [`Button::pressed_modifiers()`].
    pub fn pressed_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.pressed_modifiers(actions))
    }

    /// See [`Button::released_modifiers()`].
    pub fn released_modifiers(&self, actions: &Actions) -> Option<KeyModifiers> {
        self.borrow().and_then(|b| b.released_modifiers(actions))
    }
}
