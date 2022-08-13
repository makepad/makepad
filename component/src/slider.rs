use {
    crate::{
        makepad_derive_frame::*,
        makepad_draw_2d::*,
        frame::*,
        text_input::{TextInput, TextInputAction}
    }
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    DrawSlider: {{DrawSlider}} {
        instance hover: float
        instance focus: float
        instance drag: float
        
        fn pixel(self) -> vec4 {
            let slider_height = 3;
            let nub_size = mix(3, 4, self.hover);
            let nubbg_size = 18
            
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            
            let slider_bg_color = mix(#38, #30, self.focus);
            
            let slider_color = mix(mix(#5, #68, self.hover), #68, self.focus);
            let nub_color = mix(mix(#8, #f, self.hover), mix(#c, #f, self.drag), self.focus);
            let nubbg_color = mix(#eee0, #8, self.drag);
            
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
            margin: {left: 4.0, top: 3.0}
            width: Fill,
            height: Fill
        }
        
        label_align: {
            y: 0.0
        }
        
        text_input: {
            cursor_margin_bottom: 3.0,
            cursor_margin_top: 4.0,
            select_pad_edges: 3.0
            cursor_size: 2.0,
            empty_message: "0",
            numeric_only: true,
            bg: {
                shape: None
                color: #5
                radius: 2
            },
            layout: {
                padding: 0,
                align: {y: 0.}
            },
            walk: {
                margin: {top: 3, right: 5}
            }
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        slider: {hover: 0.0}
                        //text_input: {state: {hover = off}}
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Play::Snap}
                    apply: {
                        slider: {hover: 1.0}
                        //text_input: {state: {hover = on}}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {slider: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Play::Snap}
                    apply: {slider: {drag: 1.0}}
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
    slider: DrawSlider,
    
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
    
    min: f64,
    max: f64,
    
    #[rust] pub value: f64,
    #[rust] pub dragging: Option<f64>,
}

#[derive(Clone, FrameAction)]
pub enum SliderAction {
    StartSlide,
    TextSlide(f64),
    Slide(f64),
    EndSlide,
    None
}

impl FrameComponent for Slider {
    fn bind_read(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        if let Some(LiveValue::Float(v)) = nodes.read_path(&self.bind) {
            self.set_internal(*v);
            self.update_text_input(cx);
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.slider.redraw(cx);
    }
    
    fn handle_component_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)) {
        self.handle_event(cx, event, &mut | cx, slider, action | {
            let mut delta = Vec::new();
            match &action {
                SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                    if slider.bind.len()>0 {
                        delta.write_path(&slider.bind, LiveValue::Float(*v as f64));
                    }
                },
                _ => ()
            };
            dispatch_action(cx, FrameActionItem::new(action.into()).bind_delta(delta))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, _self_uid: FrameUid) -> FrameDraw {
        self.draw_walk(cx, walk);
        FrameDraw::done()
    }
}

impl Slider {
    
    fn to_external(&self) -> f64 {
        self.value * (self.max - self.min) + self.min
    }
    
    fn set_internal(&mut self, external: f64) {
        self.value = (external - self.min) / (self.max - self.min)
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, &mut Self, SliderAction)) {
        self.state_handle_event(cx, event);
        for action in self.text_input.handle_event_iter(cx, event) {
            match action {
                TextInputAction::KeyFocus => {
                    self.animate_state(cx, ids!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animate_state(cx, ids!(focus.off));
                }
                TextInputAction::Return(value) => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input(cx);
                    dispatch_action(cx, self, SliderAction::TextSlide(self.to_external()));
                }
                TextInputAction::Escape => {
                    self.update_text_input(cx);
                }
                _ => ()
            }
        };
        match event.hits(cx, self.slider.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                // cx.set_key_focus(self.slider.area());
                self.text_input.read_only = true;
                self.text_input.set_key_focus(cx);
                self.text_input.select_all();
                self.text_input.redraw(cx);
                
                self.animate_state(cx, ids!(drag.on));
                self.dragging = Some(self.value);
                dispatch_action(cx, self, SliderAction::StartSlide);
            },
            Hit::FingerUp(fe) => {
                self.text_input.read_only = false;
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                self.text_input.create_external_undo();
                self.animate_state(cx, ids!(drag.off));
                if fe.is_over && fe.digit.has_hovers() {
                    self.animate_state(cx, ids!(hover.on));
                }
                else {
                    self.animate_state(cx, ids!(hover.off));
                }
                self.dragging = None;
                dispatch_action(cx, self, SliderAction::EndSlide);
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    self.value = (start_pos + rel.x / fe.rect.size.x).max(0.0).min(1.0);
                    self.slider.redraw(cx);
                    self.update_text_input(cx);
                    dispatch_action(cx, self, SliderAction::Slide(self.to_external()));
                }
            }
            _ => ()
        }
    }
    
    pub fn update_text_input(&mut self, cx: &mut Cx) {
        self.text_input.text = format!("{:.2}", self.to_external());
        self.text_input.select_all();
        self.text_input.redraw(cx)
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.slider.slide_pos = self.value as f32;
        self.slider.begin(cx, walk, self.layout);
        
        if let Some(dw) = cx.defer_walk(self.label_walk) {
            //, (self.value*100.0) as usize);
            self.text_input.draw_walk(cx, self.text_input.get_walk());
            self.label_text.draw_walk(cx, dw.resolve(cx), self.label_align, &self.label);
        }
        
        self.slider.end(cx);
    }
}

