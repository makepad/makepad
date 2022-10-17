use {
    crate::{
        makepad_derive_widget::*,
        makepad_image_formats::jpeg,
        makepad_image_formats::png,
        makepad_draw_2d::*,
        widget::*,
        scroll_bars::ScrollBars,
    },
};

live_register!{
    import crate::scroll_bars::ScrollBars;    

    Frame: {{Frame}} {}

    Solid: Frame {bg: {shape: Solid}}
    Rect: Frame {bg: {shape: Rect}}
    Box: Frame {bg: {shape: Box}}
    BoxX: Frame {bg: {shape: BoxX}}
    BoxY: Frame {bg: {shape: BoxY}}
    BoxAll: Frame {bg: {shape: BoxAll}}
    GradientY: Frame {bg: {shape: GradientY}}
    Circle: Frame {bg: {shape: Circle}}
    Hexagon: Frame {bg: {shape: Hexagon}}
    GradientX: Frame {bg: {shape: Solid, fill: GradientX}}
    GradientY: Frame {bg: {shape: Solid, fill: GradientY}}
    Image: Frame {bg: {shape: Solid, fill: Image}}
    UserDraw: Frame {user_draw: true}
    ScrollXY: Frame {scroll_bars: ScrollBars{show_scroll_x:true, show_scroll_y:true}}
    ScrollX: Frame {scroll_bars: ScrollBars{show_scroll_x:true, show_scroll_y:false}}
    ScrollY: Frame {scroll_bars: ScrollBars{show_scroll_x:false, show_scroll_y:true}}
}

#[derive(Live)]
#[live_register(widget!(Frame))]
pub struct Frame { // draw info per UI element
    bg: DrawShape,
    
    pub layout: Layout,
    
    pub walk: Walk,
    
    image: LiveDependency,
    
    image_texture: Texture,
    
    has_view: bool,
    hidden: bool,
    user_draw: bool,
    
    cursor: Option<MouseCursor>,
    scroll_bars: Option<LivePtr>,

    #[rust] scroll_bars_obj: Option<ScrollBars>,
    
    #[live(false)] design_mode: bool,
    #[rust] area: Area,
    #[rust] pub view: Option<View>,
    
    #[rust] defer_walks: Vec<(LiveId, DeferWalk)>,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] templates: ComponentMap<LiveId, (LivePtr, usize)>,
    #[rust] children: ComponentMap<LiveId, WidgetRef>,
    #[rust] draw_order: Vec<LiveId>
}

impl LiveHook for Frame {
    
    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if self.has_view && self.view.is_none() {
            self.view = Some(View::new(cx));
        }
        if self.scroll_bars.is_some(){
            if self.scroll_bars_obj.is_none(){
                self.scroll_bars_obj = Some(ScrollBars::new_from_ptr(cx, self.scroll_bars));
            }
        }
        // lets load the image resource
        let image_path = self.image.as_ref();
        if image_path.len()>0 {
            let mut image_buffer = None;
            match cx.get_dependency(image_path) {
                Ok(data) => {
                    if image_path.ends_with(".jpg") {
                        match jpeg::decode(data) {
                            Ok(image) => {
                                image_buffer = Some(image);
                            }
                            Err(err) => {
                                cx.apply_image_decoding_failed(live_error_origin!(), index, nodes, image_path, &err);
                            }
                        }
                    }
                    else if image_path.ends_with(".png") {
                        match png::decode(data) {
                            Ok(image) => {
                                image_buffer = Some(image);
                            }
                            Err(err) => {
                                cx.apply_image_decoding_failed(live_error_origin!(), index, nodes, image_path, &err);
                            }
                        }
                    }
                    else {
                        cx.apply_image_type_not_supported(live_error_origin!(), index, nodes, image_path);
                    }
                }
                Err(err) => {
                    cx.apply_resource_not_loaded(live_error_origin!(), index, nodes, image_path, &err);
                }
            }
            if let Some(mut image_buffer) = image_buffer.take() {
                self.image_texture.set_desc(cx, TextureDesc {
                    format: TextureFormat::ImageBGRA,
                    width: Some(image_buffer.width),
                    height: Some(image_buffer.height),
                    multisample: None
                });
                self.image_texture.swap_image_u32(cx, &mut image_buffer.data);
            }
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
                    return self.children.get_or_insert(cx, id, | cx | {WidgetRef::new(cx)})
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

#[derive(Clone, PartialEq, WidgetRef)]
pub struct FrameRef(WidgetRef);

impl FrameRef{
    pub fn widget_query(&self, query: &WidgetQuery, callback: &mut WidgetQueryCb) -> WidgetResult {
        self.0.widget_query(query, callback)
    }
    
    pub fn get_widget(&self, path: &[LiveId]) -> WidgetRef {
         self.0.get_widget(path)
    }

    pub fn handle_event_vec(&self, cx: &mut Cx, event: &Event) -> WidgetActions {
        self.0.handle_widget_event_vec(cx, event)
    }

    pub fn redraw(&self, cx: &mut Cx) {
        self.0.redraw(cx)
    }

    pub fn template(&self,
        cx: &mut Cx,
        path: &[LiveId],
        new_id: &[LiveId;1],
        nodes: &[LiveNode]
    ) -> WidgetRef {
        self.0.template(cx, path, new_id, nodes)
    }
    
    pub fn draw(&self, cx: &mut Cx2d,) -> WidgetDraw {
        if let Some(mut inner) = self.inner_mut(){
            return inner.draw(cx)
        }
        WidgetDraw::done()
    }

    pub fn set_scroll_pos(&self, cx:&mut Cx, v:DVec2){
        if let Some(mut inner) = self.inner_mut(){
            inner.set_scroll_pos(cx, v)
        }
    }

    pub fn area(&self)->Area{
        if let Some(inner) = self.inner(){
            inner.area
        }
        else{
            Area::Empty
        }
    }
}

impl Widget for Frame {
    
    fn handle_widget_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        if let Some(scroll_bars) = &mut self.scroll_bars_obj{
            scroll_bars.handle_main_event(cx, event, &mut |_,_|{});
        }

        for id in &self.draw_order {
            if let Some(child) = self.children.get_mut(id) {
                child.handle_widget_event(cx, event, &mut | cx, action | {
                    dispatch_action(cx, action.set_widget(&child));
                });
            }
        }
        
        if let Some(cursor) = &self.cursor {
            match event.hits(cx, self.area()) {
                Hit::FingerDown(_) => {
                    cx.set_key_focus(Area::Empty);
                }
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(*cursor);
                }
                _ => ()
            }
        }
        
        if let Some(scroll_bars) = &mut self.scroll_bars_obj{
            scroll_bars.handle_scroll_event(cx, event, &mut |_,_|{});
        }
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        for child in self.children.values_mut() {
            child.redraw(cx);
        }
    }
    
    fn create_child(
        &mut self,
        cx: &mut Cx,
        live_ptr: LivePtr,
        at: CreateAt,
        new_id: LiveId,
        nodes: &[LiveNode]
    ) -> WidgetRef {
        if self.design_mode {
            return WidgetRef::empty()
        }
        
        self.draw_order.retain( | v | *v != new_id);
        
        // lets resolve the live ptr to something
        let mut x = WidgetRef::new_from_ptr(cx, Some(live_ptr));
        
        x.apply(cx, ApplyFrom::ApplyOver, 0, nodes);
        
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
        
        self.children.get_mut(&new_id).unwrap().clone()
    }
    
    fn query_template(&self, id: LiveId) -> Option<LivePtr> {
        if let Some((live_ptr, _)) = self.templates.get(&id) {
            Some(*live_ptr)
        }
        else {
            None
        }
    }
    
    fn widget_query(&mut self, query: &WidgetQuery, callback: &mut WidgetQueryCb) -> WidgetResult {
        match query {
            WidgetQuery::All=> {
                for child in self.children.values_mut() {
                    child.widget_query(query, callback) ?
                }
            },
            WidgetQuery::Template(path) | WidgetQuery::Path(path) => {
                if self.children.get(&path[0]).is_none() {
                    for child in self.children.values_mut() {
                        child.widget_query(query, callback) ?;
                    }
                }
                else {
                    if path.len()>1 {
                        self.children.get_mut(&path[0]).unwrap().widget_query(
                            &WidgetQuery::Path(&path[1..]),
                            callback
                        ) ?;
                    }
                    else {
                        let child = self.children.get_mut(&path[0]).unwrap();
                        callback(WidgetFound::Child(child.clone()))?;
                    }
                }
            }
        }
        WidgetResult::next()
    }
    
    
}

#[derive(Clone)]
enum DrawState {
    Drawing(usize),
    DeferWalk(usize)
}

impl Frame {

    pub fn set_scroll_pos(&mut self, cx:&mut Cx, v:DVec2){
        if let Some(scroll_bars) = &mut self.scroll_bars_obj{
            scroll_bars.set_scroll_pos(cx, v);
        }
        else{
            self.layout.scroll = v;
        }
    }

    pub fn area(&self)->Area{
        self.area
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d,) -> WidgetDraw {
        self.draw_walk(cx, self.get_walk())
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) -> WidgetDraw {
        if self.hidden {
            return WidgetDraw::done()
        }
        // the beginning state
        if self.draw_state.begin(cx, DrawState::Drawing(0)) {
            self.defer_walks.clear();
            
            if self.has_view {
                if self.view.as_mut().unwrap().begin(cx).not_redrawing() {
                    return WidgetDraw::done()
                };
                walk = Walk::default();
            }
            
            // ok so.. we have to keep calling draw till we return LiveId(0)
            let scroll = if let Some(scroll_bars) = &mut self.scroll_bars_obj{
                scroll_bars.begin_nav_area(cx);
                scroll_bars.get_scroll_pos()
            }
            else{
                self.layout.scroll
            };
            
            if self.bg.shape != Shape::None {
                if self.bg.fill == Fill::Image {
                    self.bg.draw_vars.set_texture(0, &self.image_texture);
                }
                self.bg.begin(cx, walk, self.layout.with_scroll(scroll));
            }
            else {
                cx.begin_turtle(walk, self.layout.with_scroll(scroll));
            }
            
            if self.user_draw {
                return WidgetDraw::not_done(WidgetRef::empty())
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
                        child.draw_widget(cx, walk) ?;
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
                    child.draw_widget(cx, walk) ?;
                }
                self.draw_state.set(DrawState::DeferWalk(step + 1));
            }
            else {
                if let Some(scroll_bars) = &mut self.scroll_bars_obj{
                    scroll_bars.draw_scroll_bars(cx);
                };

                if self.bg.shape != Shape::None {
                    self.bg.end(cx);
                    self.area = self.bg.area();
                }
                else {
                    cx.end_turtle_with_area(&mut self.area);
                };
                
                if let Some(scroll_bars) = &mut self.scroll_bars_obj{
                    scroll_bars.set_area(self.area);
                    scroll_bars.end_nav_area(cx);
                };

                if self.has_view {
                    self.view.as_mut().unwrap().end(cx);
                }
                self.draw_state.end();
                break;
            }
        }
        WidgetDraw::done()
    }
}

