use crate::{
    widget::*,
    makepad_derive_widget::*,
    makepad_draw::*,
    scroll_bar::{ScrollBar, ScrollBarAction},
    portal_list::PortalListAction
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    use crate::scroll_bar::ScrollBar;
    
    pub PortalList2Base = {{PortalList2}} {}
    pub PortalList2 = <PortalList2Base> {
        width: Fill, height: Fill,
        capture_overload: true
        scroll_bar: <ScrollBar> {}
        flow: Down
    }
}

/// The maximum number of items that will be shown as part of a smooth scroll animation.
const SMOOTH_SCROLL_MAXIMUM_WINDOW: usize = 20;

#[derive(Clone,Copy)]
struct ScrollSample{
    abs: f64,
    time: f64,
}

enum ScrollState {
    Stopped,
    Drag{samples:Vec<ScrollSample>},
    Flick {delta: f64, next_frame: NextFrame},
    Pulldown {next_frame: NextFrame},
    ScrollingTo {target_id: usize, delta: f64, next_frame: NextFrame},
}

#[derive(Clone, Debug)]
enum DrawDirection {
    Up,
    Down
}

#[allow(unused)]
#[derive(Clone, Debug)]
enum ListDrawState {
    BeginItem {index: usize, pos: f64, viewport: Rect, direction:DrawDirection, min:usize, max:usize},
    EndItem {index: usize, size:Option<f64>, pos: f64,direction:DrawDirection, viewport: Rect, min:usize, max:usize},
    Ended {viewport: Rect, min:usize, max:usize}
}

#[derive(Clone, Debug, DefaultNone)]
pub enum PortalList2Action {
    Scroll,
    SmoothScrollReached,
    None
}

impl ListDrawState {
}

#[derive(Live, Widget)]
pub struct PortalList2 {
    #[redraw] #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    
    #[rust] range_start: usize,
    #[rust(usize::MAX)] range_end: usize,
    
    #[rust(0usize)] view_window: usize,
    #[rust(0usize)] visible_items: usize,
        
    #[live(0.2)] flick_scroll_minimum: f64,
    #[live(80.0)] flick_scroll_maximum: f64,
    #[live(0.005)] flick_scroll_scaling: f64,
    #[live(0.98)] flick_scroll_decay: f64,
        
    #[live(100.0)] max_pull_down: f64,
    
    #[live(false)] grab_key_focus: bool,
    #[live] capture_overload: bool,
    #[live(true)] drag_scrolling: bool,
    
    #[live(false)] auto_tail: bool,
    #[rust(false)] tail_range: bool,
    #[rust(false)] at_end: bool,
    #[rust(true)] not_filling_viewport: bool,
    #[rust] detect_tail_in_draw: bool,

    #[rust] first_id: usize,
    #[rust] first_scroll: f64,
    
    #[rust(Vec2Index::X)] vec_index: Vec2Index,
    #[live] scroll_bar: ScrollBar,
    
    #[rust] draw_state: DrawStateWrap<ListDrawState>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    
    #[rust] items: ComponentMap<usize, WidgetItem>,
    #[rust] reusable_items: Vec<WidgetItem>,
    #[rust] draw_lists: ComponentMap<usize, WidgetDrawList>,
    
    //#[rust(DragState::None)] drag_state: DragState,
    #[rust(ScrollState::Stopped)] scroll_state: ScrollState
}

struct WidgetItem{
    widget: WidgetRef,
    template: LiveId,
}

struct WidgetDrawList{
    draw_list: DrawList2d,
    area: Area
}

impl LiveHook for PortalList2 {
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
                for (_, item) in self.items.iter_mut() {
                    if item.template == id {
                        item.widget.apply(cx, apply, index, nodes);
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

impl PortalList2 {
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk)->bool{
        
        if let Some(state) = self.draw_state.begin_state(cx){
            cx.begin_turtle(walk, self.layout);
            let viewport = cx.turtle().padded_rect();
            *state = Some(ListDrawState::BeginItem{
                min: self.first_id,
                max: self.first_id,
                index: self.first_id,
                pos: self.first_scroll,
                viewport,
                direction: DrawDirection::Down
            });
            true
        }
        else{
            false
        }
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area);
        if let Some(ListDrawState::Ended{min, max, viewport:_}) = self.draw_state.get(){
            for _id in min..max{
            }
        }
        else{
            panic!()
        }
    }
    
    fn begin_item(&mut self, cx:&mut Cx2d,  id:usize, viewport:Rect)->Option<f64>{
        let vi = self.vec_index;
        let layout = if vi == Vec2Index::Y { 
            Layout::flow_down()
        } else { 
            Layout::flow_right()
        };
        let size =  cx.turtle().padded_rect().size.index(vi);
        
        // alright lets look the items drawlist up
        let dl = if let Some(dl) = self.draw_lists.get_mut(&id){
            if !cx.will_redraw_check_axis(&mut dl.draw_list, size, vi){
                // lets emit the drawlist and not redraw it
                cx.append_sub_draw_list(&dl.draw_list);
                // return the height of the previous drawlist.
                return Some(dl.area.rect(cx).size.index(vi))
            }
            dl
        }
        else{
            self.draw_lists.insert(id, WidgetDrawList{
                draw_list: DrawList2d::new(cx),
                area: Area::Empty,
            });
            self.draw_lists.get_mut(&id).unwrap()
        };
        // lets begin drawlist,
        dl.draw_list.begin_always(cx);
        match vi {
            Vec2Index::Y => {
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x, viewport.pos.y)),
                    margin: Default::default(),
                    width: Size::Fill,
                    height: Size::Fit
                }, layout);
            }
            Vec2Index::X => {
                cx.begin_turtle(Walk {
                    abs_pos: Some(dvec2(viewport.pos.x , viewport.pos.y)),
                    margin: Default::default(),
                    width: Size::Fit,
                    height: Size::Fill
                }, layout);
            }
        }
        None
    }
    
    fn end_item(&mut self, cx:&mut Cx2d, id: usize)->f64{
        // ok lets end an item
        let dl = self.draw_lists.get_mut(&id).unwrap();
        let rect = cx.end_turtle_with_area(&mut dl.area);
        dl.draw_list.end(cx);
        let vi = self.vec_index;
        rect.size.index(vi)
    }
    
    /// Returns the index of the next visible item that will be drawn by this PortalList.
    pub fn next_visible_item(&mut self, cx: &mut Cx2d) -> Option<usize> {
        let vi = self.vec_index;
        //let layout = if vi == Vec2Index::Y { Layout::flow_down() } else { Layout::flow_right() };
        if let Some(draw_state) = self.draw_state.get() {
            match draw_state {
                ListDrawState::BeginItem{index, pos, viewport, direction, min, max} => {
                    let size = self.begin_item(cx, index, viewport);
                    self.draw_state.set(ListDrawState::EndItem {
                        min: min.min(index),
                        max: max.max(index),
                        index,
                        pos,
                        direction,
                        viewport,
                        size
                    });
                    if size.is_none(){
                        return Some(index);
                    }
                    else {
                        return self.next_visible_item(cx);
                    }
                }
                ListDrawState::EndItem {index, pos, viewport, size, direction, min, max}  => {
                    
                    let size = if let Some(size) = size{
                        size
                    }
                    else{
                        self.end_item(cx, index)
                    };
                    if size == 0.0{ // terminate, possible infinite loop
                        //error!("Can't use 0.0 size items in portal list");
                         //return None
                    }
                    match direction{
                        DrawDirection::Down=>{
                            let next_pos = pos + size;
                            if next_pos >= viewport.size.index(vi){
                                if self.first_id > 0{
                                    self.draw_state.set(ListDrawState::BeginItem {
                                        index: self.first_id - 1,
                                        pos: self.first_scroll,
                                        direction:DrawDirection::Up,
                                        viewport,
                                        min,
                                        max
                                    });
                                    return self.next_visible_item(cx);
                                }
                                else{
                                    self.draw_state.set(ListDrawState::Ended{
                                        viewport,
                                        min,
                                        max
                                    });
                                    return None
                                }
                            }
                            else{
                                self.draw_state.set(ListDrawState::BeginItem {
                                    index: index + 1,
                                    pos: next_pos,
                                    min,
                                    max,
                                    direction:DrawDirection::Down,
                                    viewport,
                                });
                                return self.next_visible_item(cx)
                            }
                        }
                        DrawDirection::Up=>{
                            let next_pos = pos - size;
                            if next_pos < 0.0 || index == 0{
                                self.draw_state.set(ListDrawState::Ended{
                                    viewport,
                                    min,
                                    max
                                });
                                return None
                            }
                            else{
                                self.draw_state.set(ListDrawState::BeginItem {
                                    index: index - 1,
                                    pos: next_pos,
                                    min,
                                    max,
                                    direction:DrawDirection::Up,
                                    viewport,
                                });
                                return self.next_visible_item(cx)
                            }
                        }
                    }
                }
                _ => ()
            }
        }
        None
    }
    
    /// Creates a new widget from the given `template` or returns an existing widget,
    /// if one already exists with the same `entry_id`.
    ///
    /// If you care whether the widget already existed or not, use [`PortalList::item_with_existed()`] instead.
    ///
    /// ## Return
    /// * If a widget already existed for the given `entry_id` and `template`,
    ///   this returns a reference to that widget.
    /// * If a new widget was created successfully, this returns a reference to that new widget.
    /// * If the given `template` could not be found, this returns `None`.
    pub fn item(&mut self, cx: &mut Cx, entry_id: usize, template: LiveId) -> WidgetRef {
        self.item_with_existed(cx, entry_id, template).0
    }
    
    /// Creates a new widget from the given `template` or returns an existing widget,
    /// if one already exists with the same `entry_id` and `template`.
    ///
    /// * If you only want to check whether the item already existed without creating one,
    ///   use [`PortalList::get_item()`] instead.
    /// * If you don't care whether the widget already existed or not, use [`PortalList::item()`] instead.
    ///
    /// ## Return
    /// * If a widget of the same `template` already existed for the given `entry_id`,
    ///   this returns a tuple of that widget and `true`.
    /// * If a new widget was created successfully, either because an item with the given `entry_id`
    ///   did not exist or because the existing item with the given `entry_id` did not use the given `template`,
    ///   this returns a tuple of that widget and `false`.
    /// * If the given `template` could not be found, this returns `None`.
    pub fn item_with_existed(&mut self, cx: &mut Cx, entry_id: usize, template: LiveId) -> (WidgetRef, bool) {
        use std::collections::hash_map::Entry;
        if let Some(ptr) = self.templates.get(&template) {
            match self.items.entry(entry_id) {
                Entry::Occupied(mut occ) => {
                    if occ.get().template == template {
                        (occ.get().widget.clone(), true)
                    } else {
                        let widget_ref =  if let Some(pos) = self.reusable_items.iter().position(|v| v.template == template){
                            self.reusable_items.remove(pos).widget
                        }
                        else{
                            WidgetRef::new_from_ptr(cx, Some(*ptr))
                        };
                        occ.insert(WidgetItem{
                            template, 
                            widget:widget_ref.clone(),
                        });
                        (widget_ref, false)
                    }
                }
                Entry::Vacant(vac) => {
                    let widget_ref =  if let Some(pos) = self.reusable_items.iter().position(|v| v.template == template){
                        self.reusable_items.remove(pos).widget
                    }
                    else{
                        WidgetRef::new_from_ptr(cx, Some(*ptr))
                    };
                    vac.insert(WidgetItem{
                        template, 
                        widget: widget_ref.clone(),
                    });
                    (widget_ref, false)
                }
            }
        } else {
            warning!("Template not found: {template}. Did you add it to the <PortalList> instance in `live_design!{{}}`?");
            (WidgetRef::empty(), false)
        }
    }
    
    /// Returns the "start" position of the item with the given `entry_id`
    /// relative to the "start" position of the PortalList.
    ///
    /// * For vertical lists, the start position is the top of the item
    ///   relative to the top of the PortalList.
    /// * For horizontal lists, the start position is the left side of the item
    ///   relative to the left side of the PortalList.
    ///
    /// Returns `None` if the item with the given `entry_id` does not exist
    /// or if the item's area rectangle is zero.
    ///
    /// TODO: FIXME: this may not properly handle bottom-up lists
    ///              or lists that go from right to left.
    pub fn position_of_item(&self, cx: &Cx, entry_id: usize) -> Option<f64> {
        const ZEROED: Rect = Rect { pos: DVec2 { x: 0.0, y: 0.0 }, size: DVec2 { x: 0.0, y: 0.0 } };
        
        if let Some(item) = self.items.get(&entry_id) {
            let item_rect = item.widget.area().rect(cx);
            if item_rect == ZEROED {
                return None;
            }
            let self_rect = self.area.rect(cx);
            if self_rect == ZEROED {
                return None;
            }
            let vi = self.vec_index;
            Some(item_rect.pos.index(vi) - self_rect.pos.index(vi))
        } else {
            None
        }
    }
        
    /// Returns a reference to the template and widget for the given `entry_id`.
    pub fn get_item(&self, entry_id: usize) -> Option<(LiveId,WidgetRef)> {
        if let Some(item) = self.items.get(&entry_id){
            Some((item.template.clone(), item.widget.clone()))
        }
        else{
            None
        }
    }
        
    pub fn set_item_range(&mut self, cx: &mut Cx, range_start: usize, range_end: usize) {
        self.range_start = range_start;
        if self.range_end != range_end {
            self.range_end = range_end;
            if self.tail_range{
                self.first_id = self.range_end.max(1) - 1;
                self.first_scroll = 0.0;
            }
            self.update_scroll_bar(cx);
        }
    }
    
    pub fn update_scroll_bar(&mut self, cx: &mut Cx) {
        let scroll_pos = ((self.first_id - self.range_start) as f64 / ((self.range_end - self.range_start).max(self.view_window + 1) - self.view_window) as f64) * self.scroll_bar.get_scroll_view_total();
        // move the scrollbar to the right 'top' position
        self.scroll_bar.set_scroll_pos_no_action(cx, scroll_pos);
    }
    
    fn delta_top_scroll(&mut self, cx: &mut Cx, delta: f64, clip_top: bool) {
        if self.range_start == self.range_end{
            self.first_scroll = 0.0
        }
        else{
            self.first_scroll += delta;
        }            
        if self.first_id == self.range_start {
            self.first_scroll = self.first_scroll.min(self.max_pull_down);
        }
        if self.first_id == self.range_start && self.first_scroll > 0.0 && clip_top {
            self.first_scroll = 0.0;
        }
        self.update_scroll_bar(cx);
    }

    /// Returns `true` if currently at the end of the list, meaning that the lasat item
    /// is visible in the viewport.
    pub fn is_at_end(&self) -> bool {
        self.at_end
    }

    /// Returns the number of items that are currently visible in the viewport,
    /// including partially visible items.
    pub fn visible_items(&self) -> usize {
        self.visible_items
    }

    /// Returns `true` if this sanity check fails: the first item ID is within the item range.
    ///
    /// Returns `false` if the sanity check passes as expected.
    pub fn fails_sanity_check_first_id_within_item_range(&self) -> bool {
        !self.tail_range
            && (self.first_id > self.range_end)
    }
}


impl Widget for PortalList2 {

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        
        let mut scroll_to = None;
        self.scroll_bar.handle_event_with(cx, event, &mut | _cx, action | {
            // snap the scrollbar to a top-index with scroll_pos 0
            if let ScrollBarAction::Scroll {scroll_pos, view_total, view_visible} = action {
                scroll_to = Some((scroll_pos, scroll_pos+0.5 >= view_total - view_visible))
            }
        });
        if let Some((scroll_to, at_end)) = scroll_to {
            if at_end && self.auto_tail{
                self.first_id = self.range_end.max(1) - 1;
                self.first_scroll = 0.0;
                self.tail_range = true;
            }
            else if self.tail_range {
                self.tail_range = false;
            }

            self.first_id = ((scroll_to / self.scroll_bar.get_scroll_view_visible()) * self.view_window as f64) as usize;
            self.first_scroll = 0.0;
            cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
            self.area.redraw(cx);
        }
        
        for item in self.items.values_mut() {
            let item_uid = item.widget.widget_uid();
            cx.group_widget_actions(uid, item_uid, |cx|{
                item.widget.handle_event(cx, event, scope)
            });
        }
        
        match &mut self.scroll_state {
            ScrollState::ScrollingTo {target_id, delta, next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    let target_id = *target_id;

                    let distance_to_target = target_id as isize - self.first_id as isize;
                    let target_passed = distance_to_target.signum() == delta.signum() as isize;
                    // check to see if we passed the target and fix it. this may happen if the delta is too high,
                    // so we can just correct the first id, since the animation isn't being smooth anyways.
                    if target_passed {
                        self.first_id = target_id;
                        self.area.redraw(cx);
                    }

                    let distance_to_target = target_id as isize - self.first_id as isize;

                    // If the target is under first_id (its bigger than it), and end is reached,
                    // first_id would never be the target, so we just take it as reached.
                    let target_visible_at_end = self.at_end && target_id > self.first_id;
                    let target_reached = distance_to_target == 0 || target_visible_at_end;

                    if !target_reached {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;

                        self.delta_top_scroll(cx, delta, true);
                        cx.widget_action(uid, &scope.path, PortalListAction::Scroll);

                        self.area.redraw(cx);
                    } else {
                        self.scroll_state = ScrollState::Stopped;
                        cx.widget_action(uid, &scope.path, PortalListAction::SmoothScrollReached);
                    }
                }
            }
            ScrollState::Flick {delta, next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    *delta = *delta * self.flick_scroll_decay;
                    if delta.abs()>self.flick_scroll_minimum {
                        *next_frame = cx.new_next_frame();
                        let delta = *delta;
                        self.delta_top_scroll(cx, delta, true);
                        cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                        self.area.redraw(cx);
                    } else {
                        self.scroll_state = ScrollState::Stopped;
                    }
                }
            }
            ScrollState::Pulldown {next_frame} => {
                if let Some(_) = next_frame.is_event(event) {
                    // we have to bounce back
                    if self.first_id == self.range_start && self.first_scroll > 0.0 {
                        self.first_scroll *= 0.9;
                        if self.first_scroll < 1.0 {
                            self.first_scroll = 0.0;
                        }
                        else {
                            *next_frame = cx.new_next_frame();
                            cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                        }
                        self.area.redraw(cx);
                    }
                    else {
                        self.scroll_state = ScrollState::Stopped
                    }
                }
            }
            _=>()
        }
        let vi = self.vec_index;
        let is_scroll = if let Event::Scroll(_) = event {true} else {false};
        if self.scroll_bar.is_area_captured(cx){
            self.scroll_state = ScrollState::Stopped;
        }
        if !self.scroll_bar.is_area_captured(cx) || is_scroll{ 
            match event.hits_with_capture_overload(cx, self.area, self.capture_overload) {
                Hit::FingerScroll(e) => {
                    if self.tail_range {
                        self.tail_range = false;
                    }
                    self.detect_tail_in_draw = true;
                    self.scroll_state = ScrollState::Stopped;
                    self.delta_top_scroll(cx, -e.scroll.index(vi), true);
                    cx.widget_action(uid, &scope.path, PortalListAction::Scroll);
                    self.area.redraw(cx);
                },
                
                Hit::KeyDown(ke) => match ke.key_code {
                    KeyCode::Home => {
                        self.first_id = 0;
                        self.first_scroll = 0.0;
                        self.tail_range = false;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    },
                    KeyCode::End => {
                        self.first_id = self.range_end.max(1) - 1;
                        self.first_scroll = 0.0;
                        if self.auto_tail {
                            self.tail_range = true;
                        }
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    },
                    KeyCode::PageUp => {
                        self.first_id = self.first_id.max(self.view_window) - self.view_window;
                        self.first_scroll = 0.0;
                        self.tail_range = false;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    },
                    KeyCode::PageDown => {
                        self.first_id += self.view_window;
                        self.first_scroll = 0.0;
                        if self.first_id >= self.range_end.max(1) {
                            self.first_id = self.range_end.max(1) - 1;
                        }
                        self.detect_tail_in_draw = true;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    },
                    KeyCode::ArrowDown => {
                        self.first_id += 1;
                        if self.first_id >= self.range_end.max(1) {
                            self.first_id = self.range_end.max(1) - 1;
                        }
                        self.detect_tail_in_draw = true;
                        self.first_scroll = 0.0;
                        self.update_scroll_bar(cx);
                        self.area.redraw(cx);
                    },
                    KeyCode::ArrowUp => {
                        if self.first_id > 0 {
                            self.first_id -= 1;
                            if self.first_id < self.range_start {
                                self.first_id = self.range_start;
                            }
                            self.first_scroll = 0.0;
                            self.area.redraw(cx);
                            self.tail_range = false;
                            self.update_scroll_bar(cx);
                        }
                    },
                    _ => ()
                }
                Hit::FingerDown(e) => {
                    //log!("Finger down {} {}", e.time, e.abs);
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area);
                    }
                    // ok so fingerdown eh.
                    if self.tail_range {
                        self.tail_range = false;
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
                            let mut total_delta = 0.0;
                            for sample in samples.iter().rev(){
                                if last.is_none(){
                                    last = Some(sample);
                                }
                                else{
                                    total_delta += last.unwrap().abs - sample.abs;
                                    scaled_delta += (last.unwrap().abs - sample.abs) / (last.unwrap().time - sample.time)
                                }
                            }
                            scaled_delta *= self.flick_scroll_scaling;
                            if self.first_id == self.range_start && self.first_scroll > 0.0 {
                                self.scroll_state = ScrollState::Pulldown {next_frame: cx.new_next_frame()};
                            }
                            else if total_delta.abs() > 10.0 && scaled_delta.abs() > self.flick_scroll_minimum{
                                
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
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        if self.begin(cx, walk) {
            return DrawStep::make_step()
        }
        // ok so if we are
        if let Some(_) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl PortalList2Ref {
    /// Sets the first item to be shown and its scroll offset.
    ///
    /// On the next draw pass, this PortalList will draw the item with the given `id`
    /// as the first item in the list, and will set the *scroll offset*
    /// (from the top of the viewport to the beginning of the first item)
    /// to the given value `s`.
    pub fn set_first_id_and_scroll(&self, id: usize, s: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.first_id = id;
            inner.first_scroll = s;
        }
    }
    
    /// Sets the first item to be shown by this PortalList to the item with the given `id`.
    pub fn set_first_id(&self, id: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.first_id = id;
        }
    }
    
    /// Returns the ID of the item currently shown as the first item in this PortalList.
    pub fn first_id(&self) -> usize {
        if let Some(inner) = self.borrow() {
            inner.first_id
        }
        else {
            0
        }
    }
    
    /// Enables whether the PortalList auto-tracks the last item in the list.
    ///
    /// If `true`, the PortalList will continually scroll to the last item in the list
    /// automatically, as new items are added.
    /// If `false`, the PortalList will not auto-scroll to the last item.
    pub fn set_tail_range(&self, tail_range: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.tail_range = tail_range
        }
    }

    /// See [`PortalList::is_at_end()`].
    pub fn is_at_end(&self) -> bool {
        let Some(inner) = self.borrow() else { return false };
        inner.is_at_end()
    }

    /// See [`PortalList::visible_items()`].
    pub fn visible_items(&self) -> usize {
        let Some(inner) = self.borrow() else { return 0 };
        inner.visible_items()
    }

    /// Returns whether the given `actions` contain an action indicating that this PortalList was scrolled.
    pub fn scrolled(&self, actions: &Actions) -> bool {
        if let PortalListAction::Scroll = actions.find_widget_action(self.widget_uid()).cast() {
            return true;
        }
        false
    }

    /// Returns the current scroll offset of this PortalList.
    ///
    /// See [`PortalListRef::set_first_id_and_scroll()`] for more information.
    pub fn scroll_position(&self) -> f64 {
        let Some(inner) = self.borrow_mut() else { return 0.0 };
        inner.first_scroll
    }
    
    /// See [`PortalList::item()`].
    pub fn item(&self, cx: &mut Cx, entry_id: usize, template: LiveId) -> WidgetRef {
        if let Some(mut inner) = self.borrow_mut(){
            inner.item(cx, entry_id, template)
        }
        else{
            WidgetRef::empty()
        }
    }

    /// See [`PortalList::item_with_existed()`].
    pub fn item_with_existed(&self, cx: &mut Cx, entry_id: usize, template: LiveId) -> (WidgetRef, bool) {
        if let Some(mut inner) = self.borrow_mut(){
            inner.item_with_existed(cx, entry_id, template)
        }
        else{
            (WidgetRef::empty(), false)
        }
    }

    /// See [`PortalList::get_item()`].
    pub fn get_item(&self, entry_id: usize) -> Option<(LiveId, WidgetRef)> {
        let Some(inner) = self.borrow() else { return None };
        inner.get_item(entry_id)
    }
    
    pub fn position_of_item(&self, cx:&Cx, entry_id: usize) -> Option<f64>{
        let Some(inner) = self.borrow() else { return None };
        inner.position_of_item(cx, entry_id)
    }
    
    pub fn items_with_actions(&self, actions: &Actions) -> ItemsWithActions {
        let mut set = Vec::new();
        self.items_with_actions_vec(actions, &mut set);
        set
    }
    
    fn items_with_actions_vec(&self, actions: &Actions, set: &mut ItemsWithActions) {
        let uid = self.widget_uid();
        if let Some(inner) = self.borrow() {
            for action in actions {
                if let Some(action) = action.as_widget_action(){
                    if let Some(group) = &action.group{
                        if group.group_uid == uid{
                            for (item_id, item) in inner.items.iter() {
                                if group.item_uid == item.widget.widget_uid(){
                                    set.push((*item_id, item.widget.clone()))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    pub fn any_items_with_actions(&self, actions: &Actions)->bool {
        let uid = self.widget_uid();
        for action in actions {
            if let Some(action) = action.as_widget_action(){
                if let Some(group) = &action.group{
                    if group.group_uid == uid{
                        return true
                    }
                }
            }
        }
        false
    }

    /// Initiates a smooth scrolling animation to the specified target item in the list.
    ///
    /// ## Arguments
    /// * `target_id`: The ID (index) of the item to scroll to.
    /// * `speed`: A positive floating-point value that controls the speed of the animation.
    ///    The `speed` will always be treated as an absolute value, with the direction of the scroll
    ///    (up or down) determined by whether `target_id` is above or below the current item.
    /// * `max_items_to_show`: The maximum number of items to show during the scrolling animation.
    ///    If `None`, the default value of 20 is used.
    ///
    /// ## Example
    /// ```rust,ignore
    /// // Scrolls to item 42 at speed 100.0, including at most 30 items in the scroll animation.
    /// smooth_scroll_to(&mut cx, 42, 100.0, Some(30));
    /// ```
    pub fn smooth_scroll_to(&self, cx: &mut Cx, target_id: usize, speed: f64, max_items_to_show: Option<usize>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.items.is_empty() { return };
        if target_id < inner.range_start || target_id > inner.range_end { return };

        let max_items_to_show = max_items_to_show.unwrap_or(SMOOTH_SCROLL_MAXIMUM_WINDOW);
        let scroll_direction: f64;
        let starting_id: Option<usize>;
        if target_id > inner.first_id {
            // Scrolling down to a larger item index
            scroll_direction = -1.0;
            starting_id = ((target_id - inner.first_id) > max_items_to_show)
                .then_some(target_id - max_items_to_show);
        } else {
            // Scrolling up to a smaller item index
            scroll_direction = 1.0;
            starting_id = ((inner.first_id - target_id) > max_items_to_show)
                .then_some(target_id + max_items_to_show);
        };

        // First, if the target_id was too far away, jump directly to a closer starting_id.
        if let Some(start) = starting_id {
            log!("smooth_scroll_to(): jumping from first ID {} to start ID {}", inner.first_id, start);
            inner.first_id = start;
        }
        // Then, we kick off the actual smooth scroll process.
        inner.scroll_state = ScrollState::ScrollingTo {
            target_id,
            delta: speed.abs() * scroll_direction as f64,
            next_frame: cx.new_next_frame()
        };
    }

    /// Returns the ID of the item that is currently being smoothly scrolled to, if any.
    pub fn is_smooth_scrolling(&self) -> Option<usize> {
        let Some(inner) = self.borrow_mut() else { return None };
        if let ScrollState::ScrollingTo { target_id, .. } = inner.scroll_state {
            Some(target_id)
        } else {
            None
        }
    }

    /// Returns whether the given `actions` contain an action indicating that this PortalList completed
    /// a smooth scroll, reaching the target.
    pub fn smooth_scroll_reached(&self, actions: &Actions) -> bool {
        if let PortalListAction::SmoothScrollReached = actions.find_widget_action(self.widget_uid()).cast() {
            return true;
        }
        false
    }

    /// Trigger an scrolling animation to the end of the list
    ///
    /// ## Arguments
    /// * `speed`: This value controls how fast the scrolling animation is.
    ///    Note: This number should be large enough to reach the end, so it is important to
    ///    test the passed number. TODO provide a better implementation to ensure that the end
    ///    is always reached, no matter the speed value.
    /// * `max_items_to_show`: The maximum number of items to show during the scrolling animation.
    ///    If `None`, the default value of 20 is used.
    pub fn smooth_scroll_to_end(&self, cx: &mut Cx, speed: f64, max_items_to_show: Option<usize>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.items.is_empty() { return };

        let starting_id = inner.range_end
            .saturating_sub(max_items_to_show.unwrap_or(SMOOTH_SCROLL_MAXIMUM_WINDOW))
            .max(inner.first_id); // don't start before the current first_id

        // First, we jump directly to the starting_id.
        inner.first_id = starting_id;
        // Then, we kick off the actual scrolling process.
        inner.scroll_state = ScrollState::Flick {
            delta: -speed,
            next_frame: cx.new_next_frame()
        };
    }

    /// It indicates if we have items not displayed towards the end of the list (below)
    /// For instance, it is useful to show or hide a "jump to the most recent" button
    /// on a chat messages list
    pub fn further_items_bellow_exist(&self) -> bool {
        let Some(inner) = self.borrow() else { return false };
        !(inner.at_end || inner.not_filling_viewport)
    }
}

type ItemsWithActions = Vec<(usize, WidgetRef)>;

impl PortalList2Set {
    pub fn set_first_id(&self, id: usize) {
        for list in self.iter() {
            list.set_first_id(id)
        }
    }
    
    pub fn items_with_actions(&self, actions: &Actions) -> ItemsWithActions {
        let mut set = Vec::new();
        for list in self.iter() {
            list.items_with_actions_vec(actions, &mut set)
        }
        set
    }
}