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

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            mix(self.color, self.color_focus, self.focus),
                            self.color_hover,
                            self.hover
                        ),
                        self.color_down,
                        self.down
                    ),
                    self.color_disabled,
                    self.disabled
                )
            }
        }
        
        draw_bg: {
            instance down: 0.0
            instance hover: 0.0
            instance focus: 0.0
            instance disabled: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                return sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(self.color, self.color_focus, self.focus),
                                self.color_hover,
                                self.hover
                            ),
                            self.color_down,
                            self.down
                        ),
                        self.color_disabled,
                        self.disabled
                    ), mix(.7, 1., self.hover)
                );
            }
        }
        
        draw_text: {
            instance down: 0.0
            instance hover: 0.0
            instance focus: 0.0,
            instance disabled: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER),
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER),
            uniform color_down: (THEME_COLOR_LABEL_INNER_DOWN),
            uniform color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            wrap: Word
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return
                mix(
                    mix(
                        mix(
                            mix(self.color, self.color_focus, self.focus),
                            self.color_hover,
                            self.hover
                        ),
                        self.color_down,
                        self.down
                    ),
                    self.color_disabled,
                    self.disabled
                )
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
        draw_bg: {
            instance down: 0.0
            instance hover: 0.0
            instance focus: 0.0
            instance disabled: 0.0

            uniform color_1: #0ff,
            uniform color_1_hover: #0ff,
            uniform color_1_down: #0ff,
            uniform color_1_focus: #0ff,
            uniform color_1_disabled: (THEME_COLOR_TEXT_DISABLED)

            uniform color_2: #A00
            uniform color_2_hover: #F00
            uniform color_2_down: #000
            uniform color_2_focus: #f00
            uniform color_2_disabled: (THEME_COLOR_TEXT_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                return sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y),
                                    mix(self.color_1_focus, self.color_2_focus, self.pos.y),
                                    self.focus
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                                self.hover
                            ),
                            mix(self.color_1_down, self.color_2_down, self.pos.y),
                            self.down
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, self.pos.y),
                        self.disabled
                    ), mix(.7, 1., self.hover));
            }
        }
        
        draw_text: {
            instance down: 0.0
            instance hover: 0.0
            instance focus: 0.0
            instance disabled: 0.0

            uniform color_1: #0ff,
            uniform color_1_hover: #0ff,
            uniform color_1_down: #0ff,
            uniform color_1_focus: #f00,
            uniform color_1_disabled: (THEME_COLOR_TEXT_DISABLED)

            uniform color_2: #A40
            uniform color_2_hover: #FA0
            uniform color_2_down: #0A0
            uniform color_2_focus: #0F0
            uniform color_2_disabled: (THEME_COLOR_TEXT_DISABLED)

            wrap: Word
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y),
                                    mix(self.color_1_focus, self.color_2_focus, self.pos.y),
                                    self.focus
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                                self.hover
                            ),
                            mix(self.color_1_down, self.color_2_down, self.pos.y),
                            self.down
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, self.pos.y),
                        self.disabled
                    );
            }
        }
    }
    
    pub LinkLabelGradientX = <LinkLabelGradientY> {
        draw_text: {
            fn get_color(self) -> vec4 {
                return
                mix(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, self.pos.x),
                                mix(self.color_1_focus, self.color_2_focus, self.pos.x),
                                self.focus
                            ),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x),
                            self.hover
                        ),
                        mix(self.color_1_down, self.color_2_down, self.pos.x),
                        self.down
                    ),
                    mix(self.color_1_disabled, self.color_2_disabled, self.pos.x),
                    self.disabled
                );
            }
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
