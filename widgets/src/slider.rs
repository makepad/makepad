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

impl LiveHook for Slider{
    fn after_new_from_doc(&mut self, _cx:&mut Cx){
        self.set_internal(self.default);
        self.update_text_input();
    }
}


#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawSlider {
    #[deref] draw_super: DrawQuad,
    #[live] slide_pos: f32,
    #[live] slider_type: SliderType
}

#[derive(Live, Widget)]
pub struct Slider {
    #[redraw] #[live] draw_slider: DrawSlider,
    
    #[walk] walk: Walk,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[rust] label_area: Area,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] draw_text: DrawText,
    #[live] text: String,
    
    #[live] text_input: TextInput,
    
    #[live] precision: usize,
    
    #[live] min: f64,
    #[live] max: f64,
    #[live] step: f64,
    #[live] default: f64,
    
    #[live] bind: String,

    // Indicates if the label of the slider responds to hover events
    // The primary use case for this kind of emitted actions is for tooltips displaying
    // and it is turned on by default, since this component already consumes finger events
    #[live(true)] hover_actions_enabled: bool,
    
    #[rust] pub relative_value: f64,
    #[rust] pub dragging: Option<f64>,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SliderAction {
    StartSlide,
    TextSlide(f64),
    Slide(f64),
    EndSlide,
    LabelHoverIn(Rect),
    LabelHoverOut,
    None
}

impl Slider {
    
    fn to_external(&self) -> f64 {
        let val = self.relative_value * (self.max - self.min) + self.min;
        if self.step != 0.0{
            return (val / self.step).floor()* self.step
        }
        else{
            val
        }
    }
    
    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.relative_value;
        self.relative_value = (external - self.min) / (self.max - self.min);
        old != self.relative_value
    }
    
    pub fn update_text_input(&mut self) {
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
    }
    
    pub fn update_text_input_and_redraw(&mut self, cx: &mut Cx) {
        self.update_text_input();
        self.text_input.redraw(cx);
    }
    
    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.relative_value as f32;
        self.draw_slider.begin(cx, walk, self.layout);
        
        if let Some(mut dw) = cx.defer_walk(self.label_walk) {
            //, (self.value*100.0) as usize);
            let walk = self.text_input.walk(cx);
            self.text_input.draw_walk_text_input(cx, walk);

            let label_walk = dw.resolve(cx);
            cx.begin_turtle(label_walk, Layout::default());
            self.draw_text.draw_walk(cx, label_walk, self.label_align, &self.text);
            cx.end_turtle_with_area(&mut self.label_area);
        }
        
        self.draw_slider.end(cx);
    }

    pub fn value(&self) -> f64 {
        self.to_external()
    }

    pub fn set_value(&mut self, v: f64) {
        let prev_value = self.value();
        self.set_internal(v);
        if v != prev_value {
            self.update_text_input();
        }
    }
}


impl Widget for Slider {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        
        for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
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
                    self.update_text_input_and_redraw(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::TextSlide(self.to_external()));
                }
                TextInputAction::Escape => {
                    self.update_text_input_and_redraw(cx);
                }
                _ => ()
            }
        };

        if self.hover_actions_enabled {
            match event.hits_with_capture_overload(cx, self.label_area, true) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverOut);
                },
                _ => ()
            }
        }

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
                self.dragging = Some(self.relative_value);
                cx.widget_action(uid, &scope.path, SliderAction::StartSlide);
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
                cx.widget_action(uid, &scope.path, SliderAction::EndSlide);
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    self.relative_value = (start_pos + rel.x / fe.rect.size.x).max(0.0).min(1.0);
                    self.set_internal(self.to_external());
                    self.draw_slider.redraw(cx);
                    self.update_text_input_and_redraw(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::Slide(self.to_external()));
                }
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_slider(cx, walk);
        DrawStep::done()
    }
    
    fn widget_to_data(&self, _cx: &mut Cx, actions:&Actions, nodes: &mut LiveNodeVec, path: &[LiveId])->bool{
        match actions.find_widget_action_cast(self.widget_uid()) {
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
                self.update_text_input_and_redraw(cx);
            }
        }
    }
    
    fn text(&self) -> String {
        format!("{}", self.to_external())
    }
        
    fn set_text(&mut self, v: &str) {
        if let Ok(v) = v.parse::<f64>(){
            self.set_internal(v);
            self.update_text_input()
        }
    }
        
}

impl SliderRef{
    pub fn value(&self)->Option<f64> {
        if let Some(inner) = self.borrow(){
            return Some(inner.value())
        }

        return None
    }

    pub fn set_value(&self, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(v)
        }
    }
    
    pub fn slided(&self, actions:&Actions)->Option<f64>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                    return Some(v)
                }
                _=>()
            }
        }
        None
    }

    pub fn label_hover_in(&self, actions:&Actions)->Option<Rect>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::LabelHoverIn(rect) => Some(rect),
                _=> None
            }
        } else {
            None
        }
    }

    pub fn label_hover_out(&self, actions:&Actions)->bool{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::LabelHoverOut => true,
                _=> false
            }
        } else {
            false
        }
    }
}