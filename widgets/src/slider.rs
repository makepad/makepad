use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        text_input::{TextInput, TextInputAction}
    }
};

live_design!{
    DrawSlider = {{DrawSlider}} {}
    SliderBase = {{Slider}} {}
}

#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum SliderType {
    #[pick] Horizontal = shader_enum(1),
    Vertical = shader_enum(2),
    Rotary = shader_enum(3),
}


#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSlider {
    #[deref] draw_super: DrawQuad,
    #[live] slide_pos: f32,
    #[live] slider_type: SliderType
}

#[derive(Live)]
pub struct Slider {
    #[live] draw_slider: DrawSlider,
    
    #[walk] walk: Walk,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] draw_text: DrawText,
    #[live] text: String,
    
    #[live] text_input: TextInput,
    
    #[live] precision: usize,
    
    #[live] min: f64,
    #[live] max: f64,
    #[live] step: f64,
    
    #[live] bind: String,
    
    #[rust] pub value: f64,
    #[rust] pub dragging: Option<f64>,
}

impl LiveHook for Slider{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,Slider)
    }
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
        let val = self.value * (self.max - self.min) + self.min;
        if self.step != 0.0{
            return (val * self.step).floor() / self.step
        }
        else{
            val
        }
    }
    
    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.value;
        self.value = (external - self.min) / (self.max - self.min);
        old != self.value
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, SliderAction)) {
        self.animator_handle_event(cx, event);
        for action in self.text_input.handle_event(cx, event) {
            match action {
                TextInputAction::KeyFocus => {
                    self.animator_play(cx, id!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animator_play(cx, id!(focus.off));
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
        match event.hits(cx, self.draw_slider.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                // cx.set_key_focus(self.slider.area());
                self.text_input.read_only = true;
                self.text_input.set_key_focus(cx);
                self.text_input.select_all();
                self.text_input.redraw(cx);
                
                self.animator_play(cx, id!(drag.on));
                self.dragging = Some(self.value);
                dispatch_action(cx, SliderAction::StartSlide);
            },
            Hit::FingerUp(fe) => {
                self.text_input.read_only = false;
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                self.text_input.create_external_undo();
                self.animator_play(cx, id!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
                self.dragging = None;
                dispatch_action(cx, SliderAction::EndSlide);
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    self.value = (start_pos + rel.x / fe.rect.size.x).max(0.0).min(1.0);
                    self.set_internal(self.to_external());
                    self.draw_slider.redraw(cx);
                    self.update_text_input(cx);
                    dispatch_action(cx, SliderAction::Slide(self.to_external()));
                }
            }
            _ => ()
        }
    }
    
    pub fn update_text_input(&mut self, cx: &mut Cx) {
        let e = self.to_external();
        self.text_input.text = match self.precision{
            0=>format!("{:.0}",e),
            1=>format!("{:.1}",e),
            2=>format!("{:.2}",e),
            3=>format!("{:.3}",e),
            4=>format!("{:.4}",e),
            5=>format!("{:.5}",e),
            6=>format!("{:.6}",e),
            7=>format!("{:.7}",e),
            _=>format!("{}",e)
        };
        self.text_input.select_all();
        self.text_input.redraw(cx)
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.value as f32;
        self.draw_slider.begin(cx, walk, self.layout);
        
        if let Some(mut dw) = cx.defer_walk(self.label_walk) {
            //, (self.value*100.0) as usize);
            let walk = self.text_input.walk(cx);
            self.text_input.draw_walk(cx, walk);
            self.draw_text.draw_walk(cx, dw.resolve(cx), self.label_align, &self.text);
        }
        
        self.draw_slider.end(cx);
    }
}


impl Widget for Slider {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_slider.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
    
    fn widget_to_data(&self, _cx: &mut Cx, actions:&WidgetActions, nodes: &mut LiveNodeVec, path: &[LiveId])->bool{
        match actions.single_action(self.widget_uid()) {
            SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                nodes.write_field_value(path, LiveValue::Float64(v as f64));
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes:&[LiveNode], path: &[LiveId]){
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(value) = value.as_float() {
                if self.set_internal(value) {
                    self.redraw(cx)
                }
                self.update_text_input(cx);
            }
        }
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct SliderRef(WidgetRef);
