use {
    std::collections::hash_map::HashMap,
    crate::{
        makepad_derive_widget::*,
        makepad_image_formats::jpeg,
        makepad_image_formats::png,
        makepad_draw::*,
        widget::*,
        scroll_bars::ScrollBars,
    },
};

live_design!{
    import crate::scroll_bars::ScrollBars;
    import makepad_draw::shader::std::*;
    import makepad_draw::shader::draw_color::DrawColor;
    Frame = {{Frame}} {}
    
    Solid = <Frame> {show_bg: true, draw_bg: {
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    
    Rect = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.rect(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0)
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result
        }
    }}
    
    Box = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: 2.5
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                max(1.0, self.radius)
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    BoxX = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: vec2(2.5, 2.5)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box_x(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                self.radius.x,
                self.radius.y
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    BoxY = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: vec2(2.5, 2.5)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box_y(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                self.radius.x,
                self.radius.y
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    BoxAll = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: vec4(2.5, 2.5, 2.5, 2.5)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box_all(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                self.radius.x,
                self.radius.y,
                self.radius.z,
                self.radius.w
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    Circle = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: 5.0
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            if self.radius.x > 0.0 {
                sdf.circle(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.radius.x
                )
            }
            else {
                sdf.circle(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    min(
                        (self.rect_size.x - (self.inset.x + self.inset.z + 2.0 * self.border_width)) * 0.5,
                        (self.rect_size.y - (self.inset.y + self.inset.w + 2.0 * self.border_width)) * 0.5
                    )
                )
            }
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result
        }
    }}
    
    Hexagon = <Frame> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: 5
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            if self.radius.x > 0.0 {
                sdf.hexagon(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.radius
                )
            }
            else {
                sdf.hexagon(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    min(
                        (self.rect_size.x - (self.inset.x + self.inset.z + 2.0 * self.border_width)) * 0.5,
                        (self.rect_size.y - (self.inset.y + self.inset.w + 2.0 * self.border_width)) * 0.5
                    )
                )
            }
            sdf.fill_keep(color)
            if self.border_width > 0.0 {
                sdf.stroke(self.border_color, self.border_width)
            }
            return sdf.result
        }
    }}
    
    GradientX = <Frame> {show_bg: true, draw_bg: {
        instance color2: #f00
        fn get_color(self) -> vec4 {
            return mix(self.color, self.color2, self.pos.x)
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    
    GradientY = <Frame> {show_bg: true, draw_bg: {
        instance color2: #f00
        fn get_color(self) -> vec4 {
            return mix(self.color, self.color2, self.pos.y)
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    
    Image = <Frame> {show_bg: true, draw_bg: {
        texture image: texture2d
        instance image_scale: vec2(1.0, 1.0)
        instance image_pan: vec2(0.0, 0.0)
        fn get_color(self) -> vec4 {
            return sample2d(self.image, self.pos * self.image_scale + self.image_pan).xyzw;
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
        
        shape: Solid,
        fill: Image
    }}
    
    CachedFrame = <Frame> {
        has_view: true,
        use_cache: true,
        draw_bg: {
            texture image: texture2d
            
            fn pixel(self) -> vec4 {
                return sample2d_rt(self.image, self.pos);
            }
            
            shape: Solid,
            fill: Image
        }
    }
    
    UserDraw = <Frame> {user_draw: true}
    ScrollXY = <Frame> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}}
    ScrollX = <Frame> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}}
    ScrollY = <Frame> {scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}}
}

#[derive(Live)]
#[live_design_fn(widget_factory!(Frame))]
pub struct Frame { // draw info per UI element
    draw_bg: DrawColor,
    
    #[live(false)] show_bg: bool,
    
    pub layout: Layout,
    
    pub walk: Walk,
    
    image: LiveDependency,
    
    image_texture: Texture,
    
    use_cache: bool,
    has_view: bool,
    #[live(true)] visible: bool,
    user_draw: bool,
    
    #[rust] find_cache: HashMap<u64, (WidgetRef, usize)>,
    
    cursor: Option<MouseCursor>,
    scroll_bars: Option<LivePtr>,
    
    #[rust] scroll_bars_obj: Option<ScrollBars>,
    
    #[live(false)] design_mode: bool,
    #[rust] view_size: Option<DVec2>,
    #[rust] area: Area,
    #[rust] pub view: Option<View>,
    #[rust] cache: Option<FrameTextureCache>,
    #[rust] defer_walks: Vec<(LiveId, DeferWalk)>,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] templates: ComponentMap<LiveId, (LivePtr, usize)>,
    #[rust] children: ComponentMap<LiveId, WidgetRef>,
    #[rust] draw_order: Vec<LiveId>
}

struct FrameTextureCache {
    pass: Pass,
    _depth_texture: Texture,
    color_texture: Texture,
}

impl LiveHook for Frame {
    
    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if self.has_view && self.view.is_none() {
            self.view = Some(View::new(cx));
        }
        if self.scroll_bars.is_some() {
            if self.scroll_bars_obj.is_none() {
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

impl FrameRef {
    
    pub fn handle_event(&self, cx: &mut Cx, event: &Event) -> WidgetActions {
        self.0.handle_widget_event(cx, event)
    }
    
    /*pub fn template(&self, cx: &mut Cx, path: &[LiveId], new_id: &[LiveId; 1], nodes: &[LiveNode]) -> WidgetRef {
        self.0.template(cx, path, new_id, nodes)
    }*/
    
    pub fn draw(&self, cx: &mut Cx2d,) -> WidgetDraw {
        if let Some(mut inner) = self.inner_mut() {
            return inner.draw(cx)
        }
        WidgetDraw::done()
    }
    
    pub fn set_visible(&mut self, visible: bool) {
        if let Some(mut inner) = self.inner_mut() {
            inner.visible = visible
        }
    }
    
    pub fn set_scroll_pos(&self, cx: &mut Cx, v: DVec2) {
        if let Some(mut inner) = self.inner_mut() {
            inner.set_scroll_pos(cx, v)
        }
    }
    
    pub fn area(&self) -> Area {
        if let Some(inner) = self.inner() {
            inner.area
        }
        else {
            Area::Empty
        }
    }
}

impl Widget for Frame {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    
    fn handle_widget_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            let mut redraw = false;
            scroll_bars.handle_main_event(cx, event, &mut | _, _ | {
                // lets invalidate all children
                redraw = true;
            });
            if redraw {
                cx.redraw_area_and_children(self.area);
            }
        }
        
        for id in &self.draw_order {
            if let Some(child) = self.children.get_mut(id) {
                child.handle_widget_event_fn(cx, event, dispatch_action);
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
        
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.handle_scroll_event(cx, event, &mut | _, _ | {});
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
    
    fn find_widget(&mut self, path: &[LiveId], cached: WidgetCache,) -> WidgetResult {
        match cached {
            WidgetCache::Yes | WidgetCache::Clear => {
                if let WidgetCache::Clear = cached {
                    self.find_cache.clear();
                }
                let mut hash = 0u64;
                for i in 0..path.len() {
                    hash ^= path[i].0
                }
                if let Some((widget, store_count)) = self.find_cache.get(&hash) {
                    let now_count = widget.strong_count();
                    if now_count >= *store_count {
                        return WidgetResult::found(widget.clone())
                    }
                }
                
                if let Some(child) = self.children.get_mut(&path[0]) {
                    if path.len()>1 {
                        if let Some(result) = child.find_widget(&path[1..], WidgetCache::No).into_found() {
                            let store_count = result.strong_count();
                            self.find_cache.insert(hash, (result.clone(), store_count));
                            return WidgetResult::found(result)
                        }
                    }
                    else {
                        return WidgetResult::found(child.clone());
                    }
                }
                for child in self.children.values_mut() {
                    if let Some(result) = child.find_widget(path, WidgetCache::No).into_found() {
                        let store_count = result.strong_count();
                        self.find_cache.insert(hash, (result.clone(), store_count));
                        return WidgetResult::found(result)
                    }
                }
            }
            WidgetCache::No => {
                if let WidgetCache::Clear = cached {
                    self.find_cache.clear();
                }
                if let Some(child) = self.children.get_mut(&path[0]) {
                    if path.len()>1 {
                        if let Some(result) = child.find_widget(&path[1..], WidgetCache::No).into_found() {
                            return WidgetResult::found(result)
                        }
                    }
                    else {
                        return WidgetResult::found(child.clone());
                    }
                }
                for child in self.children.values_mut() {
                    if let Some(result) = child.find_widget(path, WidgetCache::No).into_found() {
                        return WidgetResult::found(result)
                    }
                }
            }
        }
        WidgetResult::not_found()
    }
    
    fn find_template(&self, id: &[LiveId; 1]) -> Option<LivePtr> {
        if let Some((live_ptr, _)) = self.templates.get(&id[0]) {
            Some(*live_ptr)
        }
        else {
            None
        }
    }
}

#[derive(Clone)]
enum DrawState {
    Drawing(usize),
    DeferWalk(usize)
}

impl Frame {
    
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, v: DVec2) {
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.set_scroll_pos(cx, v);
        }
        else {
            self.layout.scroll = v;
        }
    }
    
    pub fn area(&self) -> Area {
        self.area
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d,) -> WidgetDraw {
        self.draw_walk(cx, self.get_walk())
    }
    
    pub fn walk_from_previous_size(&self, walk:Walk)->Walk{
        let view_size = self.view_size.unwrap();
        Walk {
            abs_pos: walk.abs_pos,
            width: if walk.width.is_fill(){walk.width}else{Size::Fixed(view_size.x)},
            height: if walk.height.is_fill(){walk.height}else{Size::Fixed(view_size.y)},
            margin: walk.margin
        }
    }
     
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if !self.visible {
            return WidgetDraw::done()
        }
        // the beginning state
        if self.draw_state.begin(cx, DrawState::Drawing(0)) {
            self.defer_walks.clear();
            
            if self.has_view {
                // ok so.. how do we render this to texture
                if self.use_cache {
                    if !cx.view_will_redraw(self.view.as_ref().unwrap()) {
                        if let Some(cache) = &self.cache{
                            self.draw_bg.draw_vars.set_texture(0, &cache.color_texture);
                            let walk = self.walk_from_previous_size(walk);
                            let mut rect = cx.walk_turtle_with_area(&mut self.area, walk);
                            let dpi = cx.current_dpi_factor();
                            rect.size = (rect.size / dpi).floor()*dpi;
                            self.draw_bg.draw_abs(cx, rect);
                            self.area = self.draw_bg.area();
                            cx.set_pass_area(&cache.pass, self.area);
                        }
                        return WidgetDraw::done()
                    }
                    // lets start a pass
                    if self.cache.is_none() {
                        self.cache = Some(FrameTextureCache {
                            pass: Pass::new(cx),
                            _depth_texture: Texture::new(cx),
                            color_texture: Texture::new(cx)
                        });
                        let cache = self.cache.as_mut().unwrap();
                        //cache.pass.set_depth_texture(cx, &cache.depth_texture, PassClearDepth::ClearWith(1.0));
                        cache.pass.add_color_texture(cx, &cache.color_texture, PassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
                    }
                    let cache = self.cache.as_mut().unwrap();
                    cx.make_child_pass(&cache.pass); 
                    cx.begin_pass(&cache.pass);
                }
                
                if self.view.as_mut().unwrap().begin(cx).is_not_redrawing() {
                    let walk = self.walk_from_previous_size(walk);
                    cx.walk_turtle_with_area(&mut self.area, walk);
                    return WidgetDraw::done()
                };
            }
            
            
            // ok so.. we have to keep calling draw till we return LiveId(0)
            let scroll = if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                scroll_bars.begin_nav_area(cx);
                scroll_bars.get_scroll_pos()
            }
            else {
                self.layout.scroll
            };
            
            if self.show_bg {
                if self.image.as_ref().len() > 0 {
                    self.draw_bg.draw_vars.set_texture(0, &self.image_texture);
                }
                self.draw_bg.begin(cx, walk, self.layout.with_scroll(scroll));
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
                if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                    scroll_bars.draw_scroll_bars(cx);
                };
                
                if self.show_bg {
                    if self.use_cache {
                        panic!("dont use show_bg and use_cache at the same time");
                    }
                    self.draw_bg.end(cx);
                    self.area = self.draw_bg.area();
                }
                else {
                    cx.end_turtle_with_area(&mut self.area);
                };
                
                if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                    scroll_bars.set_area(self.area);
                    scroll_bars.end_nav_area(cx);
                };
                 
                if self.has_view {
                    let mut rect = self.area.get_rect(cx);
                    self.view_size = Some(rect.size);
                    let dpi = cx.current_dpi_factor();
                    rect.size = (rect.size / dpi).floor()*dpi;
                    self.view.as_mut().unwrap().end(cx);
                    if self.use_cache {
                        let cache = self.cache.as_mut().unwrap();
                        cx.end_pass(&cache.pass);
                        self.draw_bg.draw_vars.set_texture(0, &cache.color_texture);
                        self.draw_bg.draw_abs(cx, rect);
                        let area = self.draw_bg.area();
                        let cache = self.cache.as_mut().unwrap();
                        cx.set_pass_area(&cache.pass, area);
                    }
                }
                self.draw_state.end();
                break;
            }
        }
        WidgetDraw::done()
    }
}

