
use crate::{
    widget::*,
    makepad_derive_widget::*,
    makepad_draw::*,
    scroll_bars::{ScrollBars}
};

live_design!{
    FlatListBase = {{FlatList}} {}
}
/*
#[derive(Clone,Copy)]
struct ScrollSample{
    abs: f64,
    time: f64,
}*/
/*
enum ScrollState {
    Stopped,
    Drag{samples:Vec<ScrollSample>},
    Flick {delta: f64, next_frame: NextFrame},
    Pulldown {next_frame: NextFrame},
}
*/
#[derive(Clone, DefaultNone)]
pub enum FlatListAction {
    Scroll,
    None
}

#[derive(Live, Widget)]
pub struct FlatList {
    //#[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,

    #[live(0.2)] flick_scroll_minimum: f64,
    #[live(80.0)] flick_scroll_maximum: f64,
    #[live(0.005)] flick_scroll_scaling: f64,
    #[live(0.98)] flick_scroll_decay: f64,
    #[live(0.2)] swipe_drag_duration: f64,
    #[live(100.0)] max_pull_down: f64,
    #[live(true)] align_top_when_empty: bool,
    #[live(false)] grab_key_focus: bool,
    #[live(true)] drag_scrolling: bool,
    
    #[rust(Vec2Index::X)] vec_index: Vec2Index,
    #[redraw] #[live] scroll_bars: ScrollBars,
    #[live] capture_overload: bool,
    #[rust] draw_state: DrawStateWrap<()>,
    
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] items: ComponentMap<LiveId, (LiveId,WidgetRef)>,
    //#[rust(DragState::None)] drag_state: DragState,
    /*#[rust(ScrollState::Stopped)] scroll_state: ScrollState*/
}

impl LiveHook for FlatList {
            
    fn before_apply(&mut self, _cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = apply.from {
            self.templates.clear();
        }
    }
    
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        if nodes[index].is_instance_prop() {
            if let Some(live_ptr) = apply.from.to_live_ptr(cx, index){
                let id = nodes[index].id;
                self.templates.insert(id, live_ptr);
                // lets apply this thing over all our childnodes with that template
                for (templ_id, node) in self.items.values_mut() {
                    if *templ_id == id {
                        node.apply(cx, apply, index, nodes);
                    }
                }
            }
        }
        else {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
    
    fn after_apply(&mut self, _cx: &mut Cx, _applyl: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let Flow::Down = self.layout.flow {
            self.vec_index = Vec2Index::Y
        }
        else {
            self.vec_index = Vec2Index::X
        }
    }
}

impl FlatList {
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        self.scroll_bars.end(cx);
    }

    pub fn space_left(&self, cx:&mut Cx2d)->f64{
        let view_total = cx.turtle().used();
        let rect_now = cx.turtle().rect();
        rect_now.size.y - view_total.y
    }

    pub fn item(&mut self, cx: &mut Cx, id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let (_, entry) = self.items.get_or_insert(cx, id, | cx | {
                (template, WidgetRef::new_from_ptr(cx, Some(*ptr)))
            });
            Some(entry.clone())
        }
        else {
            warning!("Template not found: {template}. Did you add it to the <FlatList> instance in `live_design!{{}}`?");
            None
        }
    }

    /*
    fn delta_top_scroll(&mut self, cx: &mut Cx, delta: f64, clip_top: bool) {
        self.first_scroll += delta;
        if self.first_scroll > 0.0 && clip_top{
            self.first_scroll = 0.0;
        }
        self.scroll_bar.set_scroll_pos_no_action(cx, self.first_scroll);
    }*/
}


impl Widget for FlatList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {

        let uid = self.widget_uid();
        self.scroll_bars.handle_event(cx, event, scope);
        /*
        let mut scroll_to = None;
        self.scroll_bars.handle_event_with(cx, event, &mut | _cx, action | {
            // snap the scrollbar to a top-index with scroll_pos 0
            if let ScrollBarAction::Scroll {scroll_pos, view_total, view_visible} = action {
                scroll_to = Some((scroll_pos, scroll_pos+0.5 >= view_total - view_visible))
            }
        });
        */
        for (item_id,item) in self.items.values_mut() {
            let item_uid = item.widget_uid();
            scope.with_id(*item_id, |scope|{
                cx.group_widget_actions(uid, item_uid, |cx|{
                    item.handle_event(cx, event, scope)
                });
            })
        }
        /*
        match &mut self.scroll_state {
            ScrollState::Flick {delta, next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    *delta = *delta * self.flick_scroll_decay;
                    if delta.abs()>self.flick_scroll_minimum {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;
                        self.delta_top_scroll(cx, delta, true);
                        dispatch_action(cx, FlatListAction::Scroll.into_action(uid));
                        self.scroll_bars.redraw(cx);
                    } else {
                        self.scroll_state = ScrollState::Stopped;
                    }
                }
            }
            ScrollState::Pulldown {next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    // we have to bounce back
                    if self.first_scroll > 0.0 {
                        self.first_scroll *= 0.9;
                        if self.first_scroll < 1.0 {
                            self.first_scroll = 0.0;
                        }
                        else {
                            *next_frame = cx.new_next_frame();
                            dispatch_action(cx, FlatListAction::Scroll.into_action(uid));
                        }
                        self.scroll_bars.redraw(cx);
                    }
                    else {
                        self.scroll_state = ScrollState::Stopped
                    }
                }
            }
            _=>()
        }*/
        /*
        let vi = self.vec_index;
        let is_scroll = if let Event::Scroll(_) = event {true} else {false};
        if self.scroll_bars.is_area_captured(cx){
            self.scroll_state = ScrollState::Stopped;
        }*/
        /*
        if !self.scroll_bars.is_area_captured(cx) || is_scroll{ 
            match event.hits_with_capture_overload(cx, self.area, self.capture_overload) {
                Hit::FingerScroll(e) => {
                    self.scroll_state = ScrollState::Stopped;
                    self.delta_top_scroll(cx, -e.scroll.index(vi), true);
                    dispatch_action(cx, FlatListAction::Scroll.into_action(uid));
                    self.area.redraw(cx);
                },
                
                Hit::FingerDown(e) => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area);
                    }
                    if self.drag_scrolling{
                        self.scroll_state = ScrollState::Drag {
                            samples: vec![ScrollSample{abs:e.abs.index(vi),time:e.time}]
                        };
                    }
                }
                Hit::FingerMove(e) => {
                    //log!("Finger move {} {}", e.time, e.abs);
                    cx.set_cursor(MouseCursor::Default);
                    match &mut self.scroll_state {
                        ScrollState::Drag {samples}=>{
                            let new_abs = e.abs.index(vi);
                            let old_sample = *samples.last().unwrap();
                            samples.push(ScrollSample{abs:new_abs, time:e.time});
                            if samples.len()>4{
                                samples.remove(0);
                            }
                            self.delta_top_scroll(cx, new_abs - old_sample.abs, false);
                            self.area.redraw(cx);
                        }
                        _=>()
                    }
                }
                Hit::FingerUp(_e) => {
                    //log!("Finger up {} {}", e.time, e.abs);
                    match &mut self.scroll_state {
                        ScrollState::Drag {samples}=>{
                            // alright so we need to see if in the last couple of samples
                            // we have a certain distance per time
                            let mut last = None;
                            let mut scaled_delta = 0.0;
                            for sample in samples.iter().rev(){
                                if last.is_none(){
                                    last = Some(sample);
                                }
                                else{
                                    scaled_delta += (last.unwrap().abs - sample.abs)/ (last.unwrap().time - sample.time)
                                }
                            }
                            scaled_delta *= self.flick_scroll_scaling;
                            if  self.first_scroll > 0.0 {
                                self.scroll_state = ScrollState::Pulldown {next_frame: cx.new_next_frame()};
                            }
                            else if scaled_delta.abs() > self.flick_scroll_minimum{
                                
                                self.scroll_state = ScrollState::Flick {
                                    delta: scaled_delta.min(self.flick_scroll_maximum).max(-self.flick_scroll_maximum),
                                    next_frame: cx.new_next_frame()
                                };
                            }
                            else{
                                self.scroll_state = ScrollState::Stopped;
                            }
                        }
                        _=>()
                    }
                    // ok so. lets check our gap from 'drag'
                    // here we kinda have to take our last delta and animate it
                }
                Hit::KeyFocus(_) => {
                }
                Hit::KeyFocusLost(_) => {
                }
                _ => ()
            }
        }*/
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            self.begin(cx, walk);
            return DrawStep::make_step()
        }
        self.end(cx);
        self.draw_state.end();
        DrawStep::done()
    }
}

impl FlatListRef {
   
    pub fn item(&self, cx: &mut Cx, entry_id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(mut inner) = self.borrow_mut() {
            inner.item(cx, entry_id, template)
        }
        else {
            None
        }
    }
    
    pub fn items_with_actions(&self, actions: &Actions) -> Vec<(LiveId, WidgetRef)> {
        let mut set = Vec::new();
        self.items_with_actions_vec(actions, &mut set);
        set
    }
    
    fn items_with_actions_vec(&self, actions: &Actions, set: &mut Vec<(LiveId, WidgetRef)>) {
        let uid = self.widget_uid();
        for action in actions {
            if let Some(action) = action.downcast_ref::<WidgetAction>(){
                if let Some(group) = &action.group{
                    if group.group_uid == uid{
                        if let Some(inner) = self.borrow() {
                            for (item_id, (_templ_id, item)) in inner.items.iter() {
                                if group.item_uid == item.widget_uid(){
                                    set.push((*item_id, item.clone()))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl FlatListSet {
    pub fn items_with_actions(&self, actions: &Actions) -> Vec<(LiveId, WidgetRef)> {
        let mut set = Vec::new();
        for list in self.iter() {
            list.items_with_actions_vec(actions, &mut set)
        }
        set
    }
}
