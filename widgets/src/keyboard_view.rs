use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    widget::*,
};

live_design!{
    link widgets;
    pub KeyboardViewBase = {{KeyboardView}} {}
    pub KeyboardView = <KeyboardViewBase>{}
}


#[derive(Live, LiveHook, Widget)]
pub struct KeyboardView {
    #[deref] view: View,
    #[redraw] #[rust] area: Area,
    #[live] outer_layout: Layout,
    #[live] outer_walk: Walk,
    #[live] keyboard_walk: Walk,
    #[live] keyboard_min_shift: f64,
    #[rust] next_frame: NextFrame,
    
    #[rust] keyboard_shift: f64,
    #[rust(AnimState::Closed)] anim_state: AnimState,
    #[rust] draw_state: DrawStateWrap<Walk>,
}

enum AnimState{
    Closed,
    Opening{duration:f64, start_time:f64, ease:Ease, height:f64},
    Open,
    Closing{duration:f64, start_time:f64,  ease:Ease, height:f64}
}

impl KeyboardView {

    fn compute_max_height(&self, height:f64, cx:&Cx)->f64{
        let self_rect = self.area.rect(cx);
        let ime_rect = cx.get_ime_area_rect();
        let av_height = self_rect.size.y - height;
        let ime_height = ime_rect.size.y + ime_rect.pos.y + self.keyboard_min_shift;
        if  ime_height > av_height {
            return ime_height - av_height 
        }
        0.0
    }
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) { 
        cx.begin_turtle(walk, self.outer_layout.with_scroll(dvec2(0.,self.keyboard_shift)));
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area);
    }
}

impl Widget for KeyboardView {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(e) = self.next_frame.is_event(event){
            match &self.anim_state{
                AnimState::Opening{duration, start_time, ease, height}=>{
                    if e.time - start_time < *duration{
                        self.keyboard_shift = ease.map((e.time - start_time)/duration) * height;
                        self.next_frame = cx.new_next_frame();
                    }
                    else{
                        self.keyboard_shift = *height;
                        self.anim_state = AnimState::Open;
                    }
                    self.redraw(cx);
                }
                AnimState::Closing{duration, start_time, ease, height}=>{
                    if e.time - start_time < *duration{
                        self.keyboard_shift = (1.0-ease.map((e.time - start_time)/duration)) * height;
                        self.next_frame = cx.new_next_frame();
                    }
                    else{ 
                        self.keyboard_shift = 0.0;
                        self.anim_state = AnimState::Closed;
                    }
                    self.redraw(cx);
                }
                _=>()
            }
        }
        match event{
            Event::VirtualKeyboard(vk)=>{
                match vk{
                    VirtualKeyboardEvent::WillShow{time, height, ease, duration}=>{
                        // ok so lets run an animation with next
                        let height = self.compute_max_height(*height, cx);
                        self.anim_state = AnimState::Opening{
                            duration: *duration,
                            start_time: *time,
                            ease: *ease,
                            height: height
                        };
                        self.next_frame = cx.new_next_frame();
                    }
                    VirtualKeyboardEvent::WillHide{time, height:_, ease, duration}=>{
                        self.anim_state = AnimState::Closing{
                            height: self.keyboard_shift,
                            duration: *duration,
                            start_time: *time,
                            ease: *ease,
                        };
                        self.next_frame = cx.new_next_frame();
                    }
                    VirtualKeyboardEvent::DidShow{time:_, height}=>{
                        if let AnimState::Closed = self.anim_state{
                            self.keyboard_shift = self.compute_max_height(*height, cx);
                        }
                        self.anim_state = AnimState::Open;
                        self.redraw(cx);
                    }
                    VirtualKeyboardEvent::DidHide{time:_}=>{
                        self.anim_state = AnimState::Closed;
                        self.keyboard_shift = 0.0;
                        self.redraw(cx);
                    }
                }
            }
            _=>()
        }
        self.view.handle_event(cx, event, scope);
    }
    
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin_with(cx, &(), |cx,_|{
            self.view.walk(cx)
        }){
            self.begin(cx, walk);
        }
        if let Some(walk) = self.draw_state.get() {
            self.view.draw_walk(cx, scope, walk)?;
        }
        self.end(cx);
        DrawStep::done()
    }
}

