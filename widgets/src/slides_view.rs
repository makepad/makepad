use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    frame::*,
    widget::*,
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::frame::*;
    import makepad_widgets::label::Label;
    //registry Widget::*;
    
    const SLIDE_WIDTH = 1920
    
    SlideBody = <Label> {
        draw_label: {
            color: #D
            text_style: {
                font_size: 35
            }
        }
        label: ""
    }
    
    Slide = <Box> {
        draw_bg: { color: #x1A, radius: 5.0 }
        walk: {width: (SLIDE_WIDTH), height: Fill}
        layout: {align: {x: 0.0, y: 0.5}, flow: Down, spacing: 10, padding: 50 }
        title = <Label> {
            draw_label: {
                color: #f
                text_style: {
                    font_size: 84
                }
            }
            label: "SlideTitle"
        }
    }

    SlideChapter = <Slide> {
        draw_bg: { color: #xFF5C39, radius: 5.0 }
        walk: {width: (SLIDE_WIDTH), height: Fill}
        layout: {align: {x: 0.0, y: 0.5}, flow: Down, spacing: 10, padding: 50 }
        title = <Label> {
            draw_label: {
                color: #x181818
                text_style: {
                    font_size: 120
                }
            }
            label: "SlideTitle"
        }
    }
    
    SlidesView = {{SlidesView}} {
        slide_width: (SLIDE_WIDTH)
        goal_pos: 0.0
        anim_speed: 0.9
        <ScrollX> {
            walk: {width: Fill, height: Fill}
        }
    }
}


#[derive(Live)]
pub struct SlidesView {
    #[live] slide_width: f64,
    #[live] goal_pos: f64,
    #[live] current_pos: f64,
    #[live] anim_speed: f64,
    #[deref] frame: Frame,
    #[rust] next_frame: NextFrame
}

impl LiveHook for SlidesView {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, SlidesView)
    }
}

#[derive(Clone, WidgetAction)]
pub enum SlidesViewAction {
    None,
}

impl Widget for SlidesView {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.frame.handle_widget_event_with(cx, event, dispatch_action);
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }
    
    fn get_walk(&self) -> Walk {
        self.frame.get_walk()
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx)
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.frame.find_widgets(path, cached, results);
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.frame.draw_walk_widget(cx, walk)
    }
}

impl SlidesView {
    fn next_frame(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
    }
    
    pub fn next_slide(&mut self, cx: &mut Cx) {
        self.goal_pos += 1.0;
        // lets cap goal pos on the # of slides
        let max_goal_pos = (self.frame.child_count().max(1) - 1) as f64;
        if self.goal_pos > max_goal_pos {
            self.goal_pos = max_goal_pos
        }
        self.next_frame(cx);
    }
    
    pub fn prev_slide(&mut self, cx: &mut Cx) {
        self.goal_pos -= 1.0;
        if self.goal_pos < 0.0 {
            self.goal_pos = 0.0;
        }
        self.next_frame(cx);
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, SlidesViewAction)) {
        // lets handle mousedown, setfocus
        match event {
            Event::Construct => {
                self.next_frame(cx);
            }
            Event::NextFrame(ne) if ne.set.contains(&self.next_frame) => {
                self.current_pos = self.current_pos * self.anim_speed + self.goal_pos * (1.0 - self.anim_speed);
                if (self.current_pos - self.goal_pos).abs()>0.00001 {
                    self.next_frame(cx);
                }
                self.frame.set_scroll_pos(cx, dvec2((self.current_pos * self.slide_width).floor(), 0.0));
                self.frame.redraw(cx);
            }
            _ => ()
        }
        match event.hits(cx, self.frame.area()) {
            Hit::KeyDown(KeyEvent {key_code: KeyCode::ArrowRight, ..}) => {
                self.next_slide(cx);
            }
            Hit::KeyDown(KeyEvent {key_code: KeyCode::ArrowLeft, ..}) => {
                self.prev_slide(cx);
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.frame.area());
            },
            _ => ()
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        while self.frame.draw_walk_widget(cx, walk).is_hook() {
        }
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, PartialEq, WidgetRef)]
pub struct SlidesViewRef(WidgetRef);

impl SlidesViewRef {
    pub fn next_slide(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.next_slide(cx);
        }
    }
    pub fn prev_slide(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.prev_slide(cx);
        }
    }
}

#[derive(Clone,  WidgetSet)]
pub struct SlidesViewSet(WidgetSet);

impl SlidesViewSet {
    pub fn next_slide(&self, cx:&mut Cx){
        for item in self.iter() {
            item.next_slide(cx);
        }
    }
    pub fn prev_slide(&self, cx:&mut Cx){
        for item in self.iter() {
            item.prev_slide(cx);
        }
    }
}

