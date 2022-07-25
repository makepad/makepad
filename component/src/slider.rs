use {
    crate::{
        makepad_derive_frame::*,
        makepad_platform::*,
        frame_traits::*,
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
            let slider_height = 3;
            let nub_size = 3
            let nubbg_size = 8
            
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            
            let slider_bg_color = #3;
            
            let slider_color = mix(mix(#7, #8, self.hover), #9, self.focus);
            let nub_color = mix(mix(#9, #c, self.hover), #e, self.focus);
            let nubbg_color = mix(#bbb0, #b, self.focus);
            
            
            sdf.rect(0, self.rect_size.y - slider_height, self.rect_size.x, slider_height)
            sdf.fill(slider_bg_color);
            
            sdf.rect(0, self.rect_size.y - slider_height, self.slide_pos * (self.rect_size.x - nub_size) + nub_size, slider_height)
            sdf.fill(slider_color);
            
            let nubbg_x = self.slide_pos * (self.rect_size.x - nub_size) - nubbg_size * 0.5 + 0.5 * nub_size;
            sdf.rect(nubbg_x, self.rect_size.y - slider_height, nubbg_size, slider_height)
            sdf.fill(nubbg_color);
            
            // the nub
            let nub_x = self.slide_pos * (self.rect_size.x - nub_size);
            sdf.rect(nub_x, self.rect_size.y - slider_height, nub_size, slider_height)
            sdf.fill(nub_color);
            
            return sdf.result
        }
    }
    
    Slider: {{Slider}} {
        min: 0.0,
        max: 1.0,
        
        label_text: {
            color: #9
        }
        
        label_walk: {
            margin: {left: 4.0}
            width: Fill,
            height: Fill
        }
        
        label_align: {
            y: 0.5
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
    draw_super: DrawQuad,
    slide_pos: f32
}

#[derive(Live, LiveHook)]
#[live_register(frame_component!(Slider))]
pub struct Slider {
    draw_slider: DrawSlider,
    
    #[alias(width, walk.width)]
    #[alias(height, walk.height)]
    #[alias(margin, walk.margin)]
    walk: Walk,
    
    layout: Layout,
    state: State,
    
    label_walk: Walk,
    label_align: Align,
    label_text: DrawText,
    label: String,
    
    bind: String,
    
    text_input: TextInput,
    
    min: f32,
    max: f32,
    
    #[rust] pub value: f32,
    #[rust] pub dragging: Option<f32>,
}

#[derive(Clone, FrameAction)]
pub enum SliderAction {
    StartSlide,
    Slide(f32),
    EndSlide,
    None
}

impl FrameComponent for Slider {
    fn bind_read(&mut self, _cx: &mut Cx, nodes: &[LiveNode]) {
        if let Some(LiveValue::Float(v)) = nodes.read_path(&self.bind) {
            self.set_internal(*v as f32);
        }
    }
    
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)) {
        self.handle_event(cx, event, &mut | cx, slider, action | {
            let mut apply = Vec::new();
            match &action {
                SliderAction::Slide(v) => {
                    if slider.bind.len()>0 {
                        apply.write_path(&slider.bind, LiveValue::Float(*v as f64));
                    }
                },
                _ => ()
            };
            dispatch_action(cx, FrameActionItem::from_bind_apply(apply, action.into()))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, _self_uid: FrameUid) -> FrameDraw {
        self.draw_walk(cx, walk);
        FrameDraw::Done
    }
}

impl Slider {
    
    fn to_external(&self) -> f32 {
        self.value * (self.max - self.min) + self.min
    }
    
    fn set_internal(&mut self, external: f32) {
        self.value = (external - self.min) / (self.max - self.min)
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, &mut Self, SliderAction)) {
        self.state_handle_event(cx, event);
        self.text_input.handle_event(cx, event, &mut | _, _ | {});
        match event.hits(cx, self.draw_slider.area()) {
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
                cx.set_key_focus(self.draw_slider.area());
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.animate_state(cx, ids!(drag.on));
                self.dragging = Some(self.value);
                dispatch_action(cx, self, SliderAction::StartSlide);
            },
            HitEvent::FingerUp(fe) => {
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                
                self.animate_state(cx, ids!(drag.off));
                if fe.is_over && fe.input_type.has_hovers() {
                    self.animate_state(cx, ids!(hover.on));
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
                self.dragging = None;
                dispatch_action(cx, self, SliderAction::EndSlide);
            }
            HitEvent::FingerMove(fe) => {
                if let Some(start_pos) = self.dragging {
                    self.value = (start_pos + (fe.rel.x - fe.rel_start.x) / fe.rect.size.x).max(0.0).min(1.0);
                    self.draw_slider.area().redraw(cx);
                    dispatch_action(cx, self, SliderAction::Slide(self.to_external()));
                }
            }
            _ => ()
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.value;
        self.draw_slider.begin(cx, walk, self.layout);
        if let Some(dw) = cx.defer_walk(self.label_walk) {
            self.text_input.value = format!("{:.2}", self.to_external()); //, (self.value*100.0) as usize);
            self.text_input.draw_walk(cx, self.text_input.get_walk());
            self.label_text.draw_walk(cx, dw.resolve(cx), self.label_align, &self.label);
        }
        self.draw_slider.end(cx);
    }
}

