use {
    crate::{
        makepad_derive_widget::*,
        frame::*,
        data_binding::DataBinding,
        makepad_draw_2d::*,
        widget::*,
        text_input::{TextInput, TextInputAction}
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    DrawSlider = {{DrawSlider}} {
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
    
    Slider = {{Slider}} {
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
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        slider: {hover: 0.0}
                        //text_input: {state: {hover = off}}
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        slider: {hover: 1.0}
                        //text_input: {state: {hover = on}}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {slider: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
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
#[live_design_fn(widget_factory!(Slider))]
pub struct Slider {
    slider: DrawSlider,
    
    walk: Walk,
    
    layout: Layout,
    state: State,
    
    label_walk: Walk,
    label_align: Align,
    label_text: DrawText,
    label: String,
    
    text_input: TextInput,
    
    min: f64,
    max: f64,
    
    bind: String,
    
    #[rust] pub value: f64,
    #[rust] pub dragging: Option<f64>,
}

#[derive(Clone, WidgetAction)]
pub enum SliderAction {
    StartSlide,
    TextSlide(f64),
    Slide(f64),
    EndSlide,
    None
}


impl Slider {
    
    fn to_external(&self) -> f64 {
        self.value * (self.max - self.min) + self.min
    }
    
    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.value;
        self.value = (external - self.min) / (self.max - self.min);
        old != self.value
    }
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, SliderAction)) {
        self.state_handle_event(cx, event);
        for action in self.text_input.handle_event(cx, event) {
            match action {
                TextInputAction::KeyFocus => {
                    self.animate_state(cx, id!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animate_state(cx, id!(focus.off));
                }
                TextInputAction::Return(value) => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input(cx);
                    dispatch_action(cx, SliderAction::TextSlide(self.to_external()));
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
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                // cx.set_key_focus(self.slider.area());
                self.text_input.read_only = true;
                self.text_input.set_key_focus(cx);
                self.text_input.select_all();
                self.text_input.redraw(cx);
                
                self.animate_state(cx, id!(drag.on));
                self.dragging = Some(self.value);
                dispatch_action(cx, SliderAction::StartSlide);
            },
            Hit::FingerUp(fe) => {
                self.text_input.read_only = false;
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                self.text_input.create_external_undo();
                self.animate_state(cx, id!(drag.off));
                if fe.is_over && fe.digit.has_hovers() {
                    self.animate_state(cx, id!(hover.on));
                }
                else {
                    self.animate_state(cx, id!(hover.off));
                }
                self.dragging = None;
                dispatch_action(cx, SliderAction::EndSlide);
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    self.value = (start_pos + rel.x / fe.rect.size.x).max(0.0).min(1.0);
                    self.slider.redraw(cx);
                    self.update_text_input(cx);
                    dispatch_action(cx, SliderAction::Slide(self.to_external()));
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


impl Widget for Slider {
    fn redraw(&mut self, cx: &mut Cx) {
        self.slider.redraw(cx);
    }
    
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    
    fn handle_widget_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_fn(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
    
    fn bind_to(&mut self, cx: &mut Cx, db: &mut DataBinding, act: &WidgetActions, path: &[LiveId]) {
        match db {
            DataBinding::FromWidgets{nodes,..} => if let Some(item) = act.find_single_action(self.widget_uid()) {
                match item.action() {
                    SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                        nodes.write_by_field_path(path, &[LiveNode::from_value(LiveValue::Float64(v as f64))]);
                    }
                    _ => ()
                }
            }
            DataBinding::ToWidgets{nodes,..} => {
                if let Some(value) = nodes.read_by_field_path(path) {
                    if let Some(value) = value.as_float() {
                        if self.set_internal(value) {
                            self.redraw(cx)
                        }
                        self.update_text_input(cx);
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct SliderRef(WidgetRef);
