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
    
    pub LinkLabelBase = {{LinkLabel}} {}
    pub LinkLabel = <LinkLabelBase> {
        // TODO: add a focus states
        instance hover: 0.0
        instance pressed: 0.0
        
        width: Fit, height: Fit,
        margin: <THEME_MSPACE_2> {}
        padding: 0.,
        
        label_walk: { width: Fit, height: Fit, },
        
        draw_bg: {
            instance pressed: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let offset_y = 1.0
                sdf.move_to(0., self.rect_size.y - offset_y);
                sdf.line_to(self.rect_size.x, self.rect_size.y - offset_y);
                return sdf.stroke(mix(
                    THEME_COLOR_TEXT_DEFAULT,
                    THEME_COLOR_TEXT_PRESSED,
                    self.pressed
                ), mix(.7, 1., self.hover));
            }
        }
        
        draw_text: {
            wrap: Word
            color: (THEME_COLOR_TEXT_DEFAULT),
            instance color_hover: (THEME_COLOR_TEXT_HOVER),
            instance color_pressed: (THEME_COLOR_TEXT_PRESSED),
            instance pressed: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR>{
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color_hover,
                        self.hover
                    ),
                    self.color_pressed,
                    self.pressed
                )
            }
        }
        
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
        
    }
    
    pub LinkLabelIcon = <LinkLabel> {
        padding: { bottom: 2. }
        label_walk: { margin: { left: -5. }},
        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance pressed: 0.0
            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        mix(self.color, #f, 0.5),
                        self.hover
                    ),
                    self.color * 0.75,
                    self.pressed
                )
            }
        }
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
    
    fn set_text(&mut self, v:&str){
        self.button.set_text(v);
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
