use {
    crate::{
        makepad_platform::*,
        frame_component::*,
        text_input::TextInput,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawSlider: {{DrawSlider}} {
        instance hover: float
        instance focus: float
        instance drag: float
        
        fn pixel(self) -> vec4 {
            let hover = max(self.hover, self.drag);
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let grad_top = 5.0;
            let grad_bot = 1.0;
            
            // we need to move the slider range slightly inward
            let xbody = self.slide_pos * (self.rect_size.x - 1.0) + 0.5;
            // show the slider position in the body
            let body = mix(
                mix(#3a, #3, hover),
                mix(#5, #6, hover),
                step(sdf.pos.x, xbody)
            );
            
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
                mix(1.0, 2.0, self.focus)
            )
            sdf.fill_keep(body)
            
            sdf.stroke(
                mix(inset_gradient, body, (1.0 - self.focus)),
                0.75
            )
            
            let xs = self.slide_pos * (self.rect_size.x - 5.0) + 2.0;
            sdf.rect(
                xs - 1.5,
                1. + (self.rect_size.y - 5.0) * (1.0 - self.focus),
                3.0,
                (self.rect_size.y - 4.0) * (self.focus) + mix(4.0, 2.0, self.focus)
            );
            sdf.fill(mix(#0000, #7, hover))
            
            return sdf.result
        }
    }
    
    Slider: {{Slider}} {
        
        layout: {
            flow: Right,
            align: {x: 1.0}
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        draw_slider: {hover: 0.0}
                        text_input: {state: {hover = off}}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        draw_slider: {hover: 1.0}
                        text_input: {state: {hover = on}}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        draw_slider: {focus: 0.0}
                        text_input: {state: {focus = off}}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                        text_input: {state: {focus = on}}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {draw_slider: {drag: 0.0}}
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {draw_slider: {drag: 1.0}}
                }
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
    
    layout: Layout,
    state: State,
    
    label_text: DrawText,
    label: String,
    
    text_input: TextInput,
    
    #[rust] pub value: f32,
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
        self.text_input.handle_event(cx, event);
        match event.hits(cx, self.draw_slider.draw_vars.area) {
            HitEvent::KeyFocusLost(_) => {
                self.animate_state(cx, ids!(focus.off));
            }
            HitEvent::KeyFocus(_) => {
                self.animate_state(cx, ids!(focus.on));
            }
            HitEvent::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
                match fe.hover_state {
                    HoverState::In => {
                        self.animate_state(cx, ids!(hover.on));
                    },
                    HoverState::Out => {
                        //self.animate_state(cx, id!(defocus));
                        self.animate_state(cx, ids!(hover.off));
                    },
                    _ => ()
                }
            },
            HitEvent::FingerDown(_fe) => {
                cx.set_key_focus(self.draw_slider.draw_vars.area);
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.animate_state(cx, ids!(drag.on));
                self.dragging = Some(self.value);
                return SliderAction::StartSlide
            },
            HitEvent::FingerUp(fe) => {
                self.animate_state(cx, ids!(drag.off));
                if fe.is_over && fe.input_type.has_hovers() {
                    self.animate_state(cx, ids!(hover.on));
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
                self.dragging = None;
                return SliderAction::EndSlide;
            }
            HitEvent::FingerMove(fe) => {
                // lets drag the fucker
                if let Some(start_pos) = self.dragging {
                    self.value = (start_pos + (fe.rel.x - fe.rel_start.x) / fe.rect.size.x).max(0.0).min(1.0);
                    self.draw_slider.draw_vars.redraw(cx);
                    /*self.draw_slider.apply_over(cx, live!{
                        slide_pos: (self.value)
                    });*/
                    //return self.handle_finger(cx, fe.rel)
                }
            }
            _ => ()
        }
        SliderAction::None
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.value;
        self.draw_slider.begin(cx, walk, self.layout);
        self.text_input.value = format!("{:.2}", self.value); //, (self.value*100.0) as usize);
        self.text_input.draw_walk(cx, self.text_input.get_walk());
        self.draw_slider.end(cx);
    }
}

