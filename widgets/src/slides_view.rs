use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::widgets::*;
    use makepad_draw::shader::std::*;
    
    pub SlidesViewBase = {{SlidesView}} {
    }
    
    pub SlidesView = <SlidesViewBase> {
        anim_speed: 0.9
    }
    
    pub Slide = <RoundedView> {
        width: Fill, height: Fill,
        flow: Down, spacing: 10,
        align: { x: 0.0, y: 0.5 }
        padding: 50.
        draw_bg: {
            color: (THEME_COLOR_SLIDES_BG),
            border_radius: (THEME_CONTAINER_CORNER_RADIUS)
        }
        title = <H1> {
            text: "SlideTitle",
            draw_text: {
                color: (THEME_COLOR_TEXT)
            }
        }
    }
    
    pub SlideChapter = <Slide> {
        width: Fill, height: Fill,
        flow: Down,
        align: {x: 0.0, y: 0.5}
        spacing: 10,
        padding: 50,
        draw_bg: {
            color: (THEME_COLOR_SLIDES_CHAPTER),
            border_radius: (THEME_CONTAINER_CORNER_RADIUS)
        }
        title = <H1> {
            text: "SlideTitle",
            draw_text: {
                color: (THEME_COLOR_TEXT)
            }
        }
    }
    
    pub SlideBody = <H2> {
        text: "Body of the slide"
        draw_text: {
            color: (THEME_COLOR_TEXT)
        }
    }
}

#[derive(Live, LiveRegisterWidget, WidgetRef, WidgetSet)]
pub struct SlidesView {
    #[layout] layout: Layout,
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[rust] children: ComponentMap<LiveId, WidgetRef>,
    #[rust] draw_order: Vec<LiveId>,
    #[rust] next_frame: NextFrame,
    #[rust] current_slide: f64,
    #[rust] goal_slide: f64,
    #[live] anim_speed: f64,
    #[rust] draw_state: DrawStateWrap<DrawState>,
}

impl WidgetNode for SlidesView{
    fn walk(&mut self, _cx:&mut Cx) -> Walk{
        self.walk
    }
    
    fn area(&self)->Area{self.area}
    
    fn redraw(&mut self, cx: &mut Cx){
        self.area.redraw(cx)
    }
    
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        for child in self.children.values() {
            child.find_widgets(path, cached, results);
        }
    }
    
    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{
        for child in self.children.values() {
            let x = child.uid_to_widget(uid);
            if !x.is_empty(){return x}
        }
        WidgetRef::empty()
    }
}   

    

#[derive(Clone)]
enum DrawState {
    DrawFirst,
    DrawSecond,
}

impl LiveHook for SlidesView {
    fn before_apply(&mut self, _cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = apply.from {
            //self.children.clear();
            self.draw_order.clear();
        }
    }
    
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match apply.from {
            ApplyFrom::Animate | ApplyFrom::Over => {
                if let Some(component) = self.children.get_mut(&nodes[index].id) {
                    component.apply(cx, apply, index, nodes)
                }
                else {
                    nodes.skip_node(index)
                }
            }
            ApplyFrom::NewFromDoc {..} | ApplyFrom::UpdateFromDoc {..} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    self.draw_order.push(id);
                    return self.children.get_or_insert(cx, id, | cx | {
                        WidgetRef::new(cx)
                    }).apply(cx, apply, index, nodes);
                }
                else {cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    nodes.skip_node(index)
                }
            }
            _ => {
                nodes.skip_node(index)
            }
        }
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx){
        self.next_frame(cx);
    }
    
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SlidesViewAction {
    Flipped(usize),
    None,
}

impl Widget for SlidesView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // lets handle mousedown, setfocus
        match event {
            Event::NextFrame(ne) if ne.set.contains(&self.next_frame) => {
                self.current_slide = self.current_slide * self.anim_speed + self.goal_slide * (1.0 - self.anim_speed);
                if (self.current_slide - self.goal_slide).abs()>0.00001 {
                    self.next_frame(cx);
                    self.area.redraw(cx);
                }
                else {
                    self.current_slide = self.current_slide.round();
                }
                                
            }
            _ => ()
        }

        //let uid = self.widget_uid();
        // lets grab the two slides we are seeing
        let current = self.current_slide.floor() as usize;
        if let Some(current_id) = self.draw_order.get(current) {
            if let Some(current) = self.children.get(&current_id) {
                scope.with_id(*current_id, |scope|{
                    current.handle_event(cx, event, scope);
                })
            }
        }
        if self.current_slide.fract() >0.0 {
            let next = current + 1;
            if let Some(next_id) = self.draw_order.get(next) {
                if let Some(next) = self.children.get(&next_id) {
                    scope.with_id(*next_id, |scope|{
                        next.handle_event(cx, event, scope);
                    })
                }
            }
        }
        match event.hits(cx, self.area) {
            Hit::KeyDown(KeyEvent {key_code: KeyCode::ArrowRight, ..}) => {
                self.next_slide(cx);
                let uid = self.widget_uid();
                cx.widget_action(uid, &scope.path, SlidesViewAction::Flipped(self.goal_slide as usize));
                
            }
            Hit::KeyDown(KeyEvent {key_code: KeyCode::ArrowLeft, ..}) => {
                self.prev_slide(cx);
                let uid = self.widget_uid();
                cx.widget_action(uid, &scope.path, SlidesViewAction::Flipped(self.goal_slide as usize));
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.area);
            },
            _ => ()
        }
                        
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
        // alright lets draw the child slide
        // we always maximally show 2 slides
        if self.draw_state.begin(cx, DrawState::DrawFirst) {
            cx.begin_turtle(walk, Layout::flow_overlay());
            let rect = cx.turtle().rect();
            cx.begin_turtle(Walk {
                abs_pos: None,
                margin: Default::default(),
                width: Size::Fill,
                height: Size::Fill
            }, Layout::flow_down().with_scroll(
                dvec2(rect.size.x * self.current_slide.fract(), 0.0)
            ));
            
        }
        if let Some(DrawState::DrawFirst) = self.draw_state.get() {
            let first = self.current_slide.floor() as usize;
            if let Some(first_id) = self.draw_order.get(first) {
                if let Some(slide) = self.children.get(&first_id) {
                    let walk = slide.walk(cx);
                    scope.with_id(*first_id, |scope|{
                        slide.draw_walk(cx, scope, walk)
                    })?;
                }
            }
            cx.end_turtle();
            let rect = cx.turtle().rect();
            cx.begin_turtle(Walk {
                abs_pos: None,
                margin: Default::default(),
                width: Size::Fill,
                height: Size::Fill
            }, Layout::flow_down().with_scroll(
                dvec2(-rect.size.x * (1.0-self.current_slide.fract()), 0.0)
            ));
            self.draw_state.set(DrawState::DrawSecond);
        }
        if let Some(DrawState::DrawSecond) = self.draw_state.get() {
            if self.current_slide.fract() > 0.0 {
                let second = self.current_slide.floor() as usize + 1;
                if let Some(second_id) = self.draw_order.get(second) {
                    if let Some(slide) = self.children.get(&second_id) {
                        let walk = slide.walk(cx);
                        scope.with_id(*second_id, |scope|{
                            slide.draw_walk(cx, scope, walk) 
                        })?;
                    }
                }
            }
        }
        cx.end_turtle();
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl SlidesView {
    fn next_frame(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
    }
    
    pub fn next_slide(&mut self, cx: &mut Cx) {
        self.goal_slide += 1.0;
        // lets cap goal pos on the # of slides
        let max_goal_slide = (self.draw_order.len().max(1) - 1) as f64;
        if self.goal_slide > max_goal_slide {
            self.goal_slide = max_goal_slide
        }
        self.next_frame(cx);
    }
    
    pub fn prev_slide(&mut self, cx: &mut Cx) {
        self.goal_slide -= 1.0;
        if self.goal_slide < 0.0 {
            self.goal_slide = 0.0;
        }
        self.next_frame(cx);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl SlidesViewRef {
    pub fn flipped(&self, actions:&Actions)->Option<usize>{
        if let SlidesViewAction::Flipped(m) = actions.find_widget_action(self.widget_uid()).cast_ref() {
            Some(*m)
        } else{
            None
        }
    }                   
    
            
    pub fn set_current_slide(&self, cx:&mut Cx, slide:usize){
        if let Some(mut inner) = self.borrow_mut() {
            inner.goal_slide = slide as f64;
            inner.current_slide = slide as f64;
            inner.redraw(cx);
        }
    }
        
    pub fn set_goal_slide(&self, cx:&mut Cx, slide:usize){
        if let Some(mut inner) = self.borrow_mut() {
            inner.goal_slide = slide as f64;
            inner.next_frame(cx);
        }
    }
    pub fn get_slide(&self)->usize{
        if let Some(inner) = self.borrow() {
            return inner.current_slide as usize
        }
        0
    }
    pub fn next_slide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.next_slide(cx);
        }
    }
    pub fn prev_slide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.prev_slide(cx);
        }
    }
}

impl SlidesViewSet {
    pub fn next_slide(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.next_slide(cx);
        }
    }
    pub fn prev_slide(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.prev_slide(cx);
        }
    }
}
