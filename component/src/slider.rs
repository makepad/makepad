use {
    crate::{
        makepad_platform::*,
        frame_component::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawSlider: {{DrawSlider}} {
        instance hover: float
        instance focus: float
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box(0., 0., self.rect_size.x, self.rect_size.y, 2.0)
            return sdf.fill(#f)
        }
    }
    
    Slider: {{Slider}} {
        
        state: {
            
            default = {
                default: true
                from: {all: Play::Forward {duration: 0.1}}
                apply: {
                    draw_slider: {hover: 0.0}
                }
            }
            
            hover = {
                from: {
                    all: Play::Forward {duration: 0.1}
                    pressed: Play::Forward {duration: 0.01}
                }
                apply: {
                    draw_slider: {
                        hover: [{time: 0.0, value: 1.0}],
                    }
                }
            }
            
            has_focus = {
                from: {all: Play::Snap}
                apply: {draw_slider: {focus: 1.0}}
            }
            
            no_focus = {
                from: {all: Play::Snap}
                apply: {draw_slider: {focus: 0.0}}
            }
            
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSlider {
    deref_target: DrawQuad,
    slide_pos: f32
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(Slider))]
pub struct Slider {
    draw_slider: DrawSlider,
    walk: Walk,
    state: State,
    
    #[rust] pub pos: f32,
    #[rust] pub dragging: bool,
}

impl FrameComponent for Slider {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, _self_id: LiveId) -> FrameComponentActionRef {
        self.handle_event(cx, event).into()
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId> {
        self.draw_walk(cx, walk);
        Ok(())
    }
}

#[derive(Copy, Clone, PartialEq, IntoFrameComponentAction)]
pub enum SliderAction {
    StartSlide,
    Slide(f32),
    EndSlide,
    None
}

impl Default for SliderAction {
    fn default() -> Self {Self::None}
}

impl Slider {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> SliderAction {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.draw_slider.draw_vars.area) {
            HitEvent::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
                match fe.hover_state {
                    HoverState::In => {
                        self.animate_state(cx, id!(hover));
                    },
                    HoverState::Out => {
                        self.animate_state(cx, id!(default));
                    },
                    _ => ()
                }
            },
            HitEvent::FingerDown(_fe) => {
                self.animate_state(cx, id!(pressed));
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.dragging = true;
                return SliderAction::StartSlide
            },
            HitEvent::FingerUp(fe) => {
                if fe.is_over {
                    if fe.input_type.has_hovers() {
                        self.animate_state(cx, id!(hover));
                    }
                    else {
                        self.animate_state(cx, id!(default));
                    }
                }
                else {
                    self.animate_state(cx, id!(default));
                }
                self.dragging = false;
                return SliderAction::EndSlide;
            }
            HitEvent::FingerMove(_fe) => {
                //return self.handle_finger(cx, fe.rel)
            },
            _ => ()
        }
        SliderAction::None
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.draw_walk(cx, walk);
    }
}

