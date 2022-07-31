use {
    crate::{
        makepad_platform::*,
        component_map::*,
        frame::*
    },
    std::any::TypeId
};

live_register!{
    Frame: {{Frame}} {}
}

#[derive(Live)]
#[live_register(frame_component!(Frame))]
pub struct Frame { // draw info per UI element
    bg: DrawShape,
    
    layout: Layout,
    
    #[alias(width, walk.width)]
    #[alias(height, walk.height)]
    #[alias(margin, walk.margin)]
    pub walk: Walk,
    
    clip: bool,
    hidden: bool,
    user_draw: bool,
    cursor: Option<MouseCursor>,
    #[live(false)] design_mode: bool,
    #[rust] pub view: Option<View>,
    
    scroll_x: FrameRef,
    scroll_y: FrameRef,
    
    #[rust] defer_walks: Vec<(LiveId, DeferWalk)>,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] templates: ComponentMap<LiveId, (LivePtr, usize)>,
    #[rust] children: ComponentMap<LiveId, FrameRef>,
    #[rust] draw_order: Vec<LiveId>
}

impl LiveHook for Frame {
    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.clip && self.view.is_none() {
            self.view = Some(View::new(cx));
        }
    }
    
    fn apply_value_instance(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match from {
            ApplyFrom::Animate | ApplyFrom::ApplyOver => {
                if let Some(component) = self.children.get_mut(&nodes[index].id) {
                    component.apply(cx, from, index, nodes)
                }
                else {
                    nodes.skip_node(index)
                }
            }
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id} => {
                if !self.design_mode && nodes[index].origin.has_prop_type(LivePropType::Template) {
                    // lets store a pointer into our templates.
                    let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                    self.templates.insert(id, (live_ptr, self.draw_order.len()));
                    nodes.skip_node(index)
                }
                else if nodes[index].origin.has_prop_type(LivePropType::Instance)
                    || self.design_mode && nodes[index].origin.has_prop_type(LivePropType::Template) {
                    self.draw_order.push(id);
                    return self.children.get_or_insert(cx, id, | cx | {FrameRef::new(cx)})
                        .apply(cx, from, index, nodes);
                }
                else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    nodes.skip_node(index)
                }
            }
            _ => {
                nodes.skip_node(index)
            }
        }
    }
}


impl FrameComponent for Frame {
    
    fn handle_component_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FrameActionItem)
    ) {
        for id in &self.draw_order {
            if let Some(child) = self.children.get_mut(id) {
                let uid = child.as_uid();
                child.handle_component_event(cx, event, &mut | cx, action | {
                    dispatch_action(cx, action.mark(*id, uid));
                });
            }
        }
        
        match event.hits(cx, self.bg.area()) {
            Hit::FingerHoverIn(_) => if let Some(cursor) = &self.cursor {
                cx.set_cursor(*cursor);
            }
            _ => ()
        }
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, self_uid: FrameUid) -> FrameDraw {
        self.draw_walk(cx, walk, self_uid)
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        if let Some(view) = &mut self.view {
            view.redraw(cx);
        }
        for child in self.children.values_mut() {
            child.as_mut().unwrap().redraw(cx);
        }
    }
    
    fn create_child(
        &mut self,
        cx: &mut Cx,
        live_ptr: LivePtr,
        at: CreateAt,
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> Option<&mut Box<dyn FrameComponent >> {
        if self.design_mode {
            return None
        }
        
        self.draw_order.retain( | v | *v != new_id);
        
        // lets resolve the live ptr to something
        let mut x = FrameRef::new_from_ptr(cx, Some(live_ptr));
        
        x.as_mut().unwrap().apply(cx, ApplyFrom::ApplyOver, 0, nodes);
        
        self.children.insert(new_id, x);
        
        match at {
            CreateAt::Template => {
                if let Some((_, draw_order)) = self.templates.values().find( | l | l.0 == live_ptr) {
                    self.draw_order.insert(*draw_order, new_id);
                }
                else {
                    self.draw_order.push(new_id);
                }
            }
            CreateAt::Begin => {
                self.draw_order.insert(0, new_id);
            }
            CreateAt::End => {
                self.draw_order.push(new_id);
            }
            CreateAt::After(after_id) => {
                if let Some(index) = self.draw_order.iter().position( | v | *v == after_id) {
                    self.draw_order.insert(index + 1, new_id);
                }
                else {
                    self.draw_order.push(new_id);
                }
            }
            CreateAt::Before(before_id) => {
                if let Some(index) = self.draw_order.iter().position( | v | *v == before_id) {
                    self.draw_order.insert(index, new_id);
                }
                else {
                    self.draw_order.push(new_id);
                }
            }
        }
        
        self.children.get_mut(&new_id).unwrap().as_mut()
    }
    
    fn query_template(&self, id: LiveId) -> Option<LivePtr> {
        if let Some((live_ptr, _)) = self.templates.get(&id) {
            Some(*live_ptr)
        }
        else {
            None
        }
    }
    
    fn frame_query(&mut self, query: &FrameQuery, callback: &mut Option<FrameQueryCb>) -> FrameResult {
        match query {
            FrameQuery::All | FrameQuery::TypeId(_) => {
                for child in self.children.values_mut() {
                    child.frame_query(query, callback) ?
                }
            },
            FrameQuery::Path(path) => {
                if self.children.get(&path[0]).is_none() {
                    for child in self.children.values_mut() {
                        child.as_mut().unwrap().frame_query(query, callback) ?;
                    }
                }
                else {
                    if path.len()>1 {
                        self.children.get_mut(&path[0]).unwrap().as_mut().unwrap().frame_query(
                            &FrameQuery::Path(&path[1..]),
                            callback
                        ) ?;
                    }
                    else {
                        let child = self.children.get_mut(&path[0]).unwrap().as_mut().unwrap();
                        if let Some(callback) = callback {
                            callback.call(FrameFound::Child(child));
                        }
                        else {
                            return FrameResult::child(child);
                        }
                    }
                }
                
            }
            FrameQuery::Uid(_) => {
                for child in self.children.values_mut() {
                    child.frame_query(query, callback) ?
                }
            }
        }
        FrameResult::not_found()
    }
    
    
}

#[derive(Clone)]
enum DrawState {
    Drawing(usize),
    DeferWalk(usize)
}

impl dyn FrameComponent {
    pub fn by_path<T: 'static + FrameComponent>(&mut self, path: &[LiveId]) -> Option<&mut T> {
        if let Some(FrameFound::Child(child)) = self.frame_query(&FrameQuery::Path(path), &mut None).into_found() {
            return child.cast_mut::<T>()
        }
        None
    }
    
    pub fn by_type<T: 'static + FrameComponent>(&mut self) -> Option<&mut T> {
        
        if let Some(FrameFound::Child(child)) = self.frame_query(&FrameQuery::TypeId(TypeId::of::<T>()), &mut None).into_found() {
            return child.cast_mut::<T>()
        }
        None
    }
}

impl Frame {
    
    pub fn component_by_uid(&mut self, uid:FrameUid) -> Option<&mut Box<dyn FrameComponent>> {
        if let Some(FrameFound::Child(child)) = self.frame_query(&FrameQuery::Uid(uid), &mut None).into_found() {
            return Some(child)
        }
        None
    }
    
    pub fn component_by_path(&mut self, path: &[LiveId]) -> Option<&mut Box<dyn FrameComponent>> {
        if let Some(FrameFound::Child(child)) = self.frame_query(&FrameQuery::Path(path), &mut None).into_found() {
            return Some(child)
        }
        None
    }
    
    pub fn by_path<T: 'static + FrameComponent>(&mut self, path: &[LiveId]) -> Option<&mut T> {
        if let Some(FrameFound::Child(child)) = self.frame_query(&FrameQuery::Path(path), &mut None).into_found() {
            return child.cast_mut::<T>()
        }
        None
    }
    
    pub fn by_type<T: 'static + FrameComponent>(&mut self) -> Option<&mut T> {
        
        if let Some(FrameFound::Child(child)) = self.frame_query(&FrameQuery::TypeId(TypeId::of::<T>()), &mut None).into_found() {
            return child.cast_mut::<T>()
        }
        None
    }
    
    pub fn area(&self) -> Area {
        if let Some(view) = &self.view {
            view.area()
        }
        else {
            self.bg.draw_vars.area
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d,) -> FrameDraw {
        self.draw_walk(cx, self.get_walk(), FrameUid::default())
    }
    
    // fetch all the children on this frame and call data_bind_read
    pub fn bind_read(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        let _ = self.frame_query(&FrameQuery::All, &mut Some(FrameQueryCb {cx: cx, cb: &mut | cx, result | {
            if let FrameFound::Child(child) = result {
                child.bind_read(cx, nodes);
            }
        }}));
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk, self_uid: FrameUid) -> FrameDraw {
        if self.hidden {
            return FrameDraw::done()
        }
        // the beginning state
        if self.draw_state.begin(cx, DrawState::Drawing(0)) {
            self.defer_walks.clear();
            
            if self.clip {
                if self.view.as_mut().unwrap().begin(cx, walk, self.layout).not_redrawing() {
                    return FrameDraw::done()
                };
                walk = Walk::default();
            }
            
            // ok so.. we have to keep calling draw till we return LiveId(0)
            if self.bg.shape != Shape::None {
                if self.bg.fill == Fill::Image{
                    self.bg.draw_vars.texture_slots[0] = Some(cx.get_internal_font_atlas_texture_id());
                }
                self.bg.begin(cx, walk, self.layout);
            }
            else {
                cx.begin_turtle(walk, self.layout);
            }
            
            if self.user_draw {
                return FrameDraw::not_done(self_uid)
            }
        }
        
        while let DrawState::Drawing(step) = self.draw_state.get() {
            if step < self.draw_order.len() {
                let id = self.draw_order[step];
                if let Some(child) = self.children.get_mut(&id) {
                    let walk = child.get_walk();
                    if let Some(fw) = cx.defer_walk(walk) {
                        self.defer_walks.push((id, fw));
                    }
                    else {
                        child.draw_component(cx, walk) ?;
                    }
                }
                self.draw_state.set(DrawState::Drawing(step + 1));
            }
            else {
                self.draw_state.set(DrawState::DeferWalk(0));
            }
        }
        
        while let DrawState::DeferWalk(step) = self.draw_state.get() {
            if step < self.defer_walks.len() {
                let (id, dw) = &self.defer_walks[step];
                if let Some(child) = self.children.get_mut(&id) {
                    let walk = dw.resolve(cx);
                    child.draw_component(cx, walk) ?;
                }
                self.draw_state.set(DrawState::DeferWalk(step + 1));
            }
            else {
                if self.bg.shape != Shape::None {
                    self.bg.end(cx);
                }
                else {
                    cx.end_turtle();
                }
                if self.clip {
                    self.view.as_mut().unwrap().end(cx);
                }
                self.draw_state.end();
                break;
            }
        }
        FrameDraw::done()
    }
}

