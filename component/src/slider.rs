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
        instance drag: float
        const BORDER_RADIUS: 2.0
        
        fn pixel(self) -> vec4 {
            let hover = max(self.hover, self.drag);
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let grad_top = 5.0;
            let grad_bot = 1.0;
            
            // we need to move the slider range slightly inward
            
            // show the slider position in the body
            let body = mix(#3, mix(#5, #6, hover), step(self.pos.x, self.slide_pos));
            
            let body_transp = vec4(body.xyz, 0.0);
            let top_gradient = mix(body_transp, #1f, max(0.0, grad_top - sdf.pos.y) / grad_top);
            let inset_gradient = mix(
                #5c,
                top_gradient,
                clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
            );
            
            sdf.box(
                1.,
                1. + (self.rect_size.y - 4.0) * (1.0 - self.focus),
                self.rect_size.x - 2.0,
                (self.rect_size.y - 4.0) * self.focus + 2.0,
                BORDER_RADIUS
            )
            sdf.fill_keep(body)
            
            sdf.stroke(
                mix(inset_gradient, body, (1.0 - self.focus)),
                0.75
            )
            
            let xs = self.slide_pos * (self.rect_size.x-5.0)+2.0;
            sdf.rect(
                xs-1.5,
                1. + (self.rect_size.y - 5.0) * (1.0 - self.focus),
                3.0,
                (self.rect_size.y - 4.0) * (self.focus)+mix(4.0,2.0,self.focus)
            );
            sdf.fill(mix(#0000, #a, hover))
            
            return sdf.result
        }
    }
    
    Slider: {{Slider}} {
        
        state: {
            
            default = {
                default: true
                from: {all: Play::Forward {duration: 0.1}}
                apply: {draw_slider: {hover: 0.0}}
            }
            
            hover = {
                from: {all: Play::Snap}
                apply: {draw_slider: {hover: 1.0}}
            }
            
            focus = {
                track: focus
                from: {all: Play::Snap}
                apply: {draw_slider: {focus: 1.0}}
            }
            
            defocus = {
                default: true,
                track: focus
                from: {all: Play::Forward {duration: 0.1}}
                apply: {draw_slider: {focus: 0.0}}
            }
            
            drag = {
                track: drag
                from: {all: Play::Snap}
                apply: {draw_slider: {drag: 1.0}}
            }
            
            nodrag = {
                default: true,
                track: drag
                from: {all: Play::Forward {duration: 0.1}}
                apply: {draw_slider: {drag: 0.0}}
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
    
    #[alias(width, walk.width)]
    #[alias(height, walk.height)]
    #[alias(margin, walk.margin)]
    walk: Walk,
    
    state: State,
    
    #[rust] pub slide_pos: f32,
    #[rust] pub dragging: Option<f32>,
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

#[derive(Copy, Clone, PartialEq, FrameComponentAction)]
pub enum SliderAction {
    StartSlide,
    Slide(f32),
    EndSlide,
    None
}

impl Slider {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> SliderAction {
        self.state_handle_event(cx, event);
        
        match event.hits(cx, self.draw_slider.draw_vars.area) {
            HitEvent::KeyFocusLost(_) => {
                self.animate_state(cx, id!(defocus));
            }
            HitEvent::KeyFocus(_) => {
                self.animate_state(cx, id!(focus));
            }
            HitEvent::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
                match fe.hover_state {
                    HoverState::In => {
                        self.animate_state(cx, id!(hover));
                    },
                    HoverState::Out => {
                        //self.animate_state(cx, id!(defocus));
                        self.animate_state(cx, id!(default));
                    },
                    _ => ()
                }
            },
            HitEvent::FingerDown(_fe) => {
                cx.set_key_focus(self.draw_slider.draw_vars.area);
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.animate_state(cx, id!(drag));
                self.dragging = Some(self.slide_pos);
                return SliderAction::StartSlide
            },
            HitEvent::FingerUp(fe) => {
                self.animate_state(cx, id!(nodrag));
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
                self.dragging = None;
                return SliderAction::EndSlide;
            }
            HitEvent::FingerMove(fe) => {
                // lets drag the fucker
                if let Some(start_pos) = self.dragging{
                    self.slide_pos = (start_pos + (fe.rel.x - fe.rel_start.x) / fe.rect.size.x).max(0.0).min(1.0);
                    self.draw_slider.apply_over(cx, live!{
                        slide_pos:(self.slide_pos)
                    });
                    //return self.handle_finger(cx, fe.rel)
                }
            }
            _ => ()
        }
        SliderAction::None
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.slide_pos;
        self.draw_slider.draw_walk(cx, walk);
    }
}

