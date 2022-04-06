use crate::makepad_platform::*;


live_register!{
    use makepad_platform::shader::std::*;
    
    DrawSlider: {{DrawSlider}} {
        instance hover: float
        instance pressed: float
         
        fn pixel(self) -> vec4 {
            
            return #f00;
        }
    }
    
    Slider: {{Slider}} {
        
        default_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply: {
                draw_wheel: {pressed: 0.0, hover: 0.0}
            }
        }
        
        hover_state: {
            from: {
                all: Play::Forward {duration: 0.1}
                pressed_state: Play::Forward {duration: 0.01}
            }
            apply: {
                draw_wheel: {
                    pressed: 0.0,
                    hover: [{time: 0.0, value: 1.0}],
                }
            }
        }
        
        pressed_state: {
            from: {all: Play::Forward {duration: 0.2}}
            apply: {
                draw_wheel: {
                    pressed: [{time: 0.0, value: 1.0}],
                    hover: 1.0,
                }
            }
        }
    }
}


#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSlider {
    deref_target: DrawQuad,
    pos:f32
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(Slider))]
pub struct Slider {
    draw_slider: DrawSlider,
    walk: Walk,
    
    #[state(default_state)]
    animator: Animator,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    
    #[rust] pub pos: f32,
    #[rust] pub dragging: bool,
}

pub enum SliderAction {
    StartSlide,
    Slide(f32),
    EndSlide,
    None
}

impl Slider {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> SliderAction {
        self.animator_handle_event(cx, event);
        
        match event.hits(cx, self.draw_wheel.draw_vars.area) {
            HitEvent::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
                match fe.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state);
                    },
                    HoverState::Out => {
                        self.animate_to(cx, self.default_state);
                    },
                    _ => ()
                }
            },
            HitEvent::FingerDown(fe) => {
                self.animate_to(cx, self.pressed_state);
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.dragging = true;
                return SliderAction::StartSlide
            },
            HitEvent::FingerUp(fe) => {
                if fe.is_over {
                    if fe.input_type.has_hovers() {
                        self.animate_to(cx, self.hover_state);
                    }
                    else {
                        self.animate_to(cx, self.default_state);
                    }
                }
                else {
                    self.animate_to(cx, self.default_state);
                }
                self.drag_mode = ColorPickerDragMode::None;
                return ColorPickerAction::DoneChanging;
            }
            HitEvent::FingerMove(fe) => {
                return self.handle_finger(cx, fe.rel)
            },
            _ => ()
        }
        ColorPickerAction::None
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_slider.draw_walk(cx, self.walk);
    }
}

