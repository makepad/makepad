use {
    std::collections::hash_map::HashMap,
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        image_loading_widget::*,
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
    
    Debug = <Frame> {show_bg: true, draw_bg: {
        color: #f00
        fn pixel(self) -> vec4 {
            return self.color
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
        instance dither: 1.0
        fn get_color(self) -> vec4 {
            let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
            return mix(self.color, self.color2, self.pos.x + dither)
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    
    GradientY = <Frame> {show_bg: true, draw_bg: {
        instance color2: #f00
        instance dither: 1.0
        fn get_color(self) -> vec4 {
            let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
            return mix(self.color, self.color2, self.pos.y + dither)
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}

    // Legacy Image widget that is being replaced with the new Image widget
    ImageFrame = <Frame> {show_bg: true, draw_bg: {
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
    
    Image = <Frame> {show_bg: true, draw_bg: {
        texture image: texture2d
        instance image_scale: vec2(1.0, 1.0)
        instance image_pan: vec2(0.0, 0.0)
        uniform image_alpha: 1.0
        fn get_color(self) -> vec4 {
            return sample2d(self.image, self.pos * self.image_scale + self.image_pan).xyzw;
        }
        
        fn pixel(self) -> vec4 {
            let color = self.get_color();
            return Pal::premul(vec4(color.xyz, color.w * self.image_alpha))
        }
        
        shape: Solid,
        fill: Image
    }}
    
    CachedFrame = <Frame> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform marked: float,
            varying scale: vec2
            varying shift: vec2
            fn vertex(self) -> vec4 {
                let dpi = self.dpi_factor;
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size;
                self.shift = (self.rect_pos - floor_pos) / ceil_size;
                return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            }
            fn pixel(self) -> vec4 {
                return sample2d_rt(self.image, self.pos * self.scale + self.shift) + vec4(self.marked, 0.0, 0.0, 0.0);
            }
            
            shape: Solid,
            fill: Image
        }
    }
    CachedScrollXY = <CachedFrame> {
        scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}
    }
    CachedScrollX = <CachedFrame> {
        scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}
    }
    CachedScrollY = <CachedFrame> {
        scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
    }
    ScrollXY = <Frame> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}}
    ScrollX = <Frame> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}}
    ScrollY = <Frame> {scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}}
}

// maybe we should put an enum on the bools like

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum FrameOptimize{
    #[pick] None,
    DrawList,
    Texture    
}

impl FrameOptimize{
    fn is_texture(&self)->bool{
        if let Self::Texture = self{true} else{false}
    }
    fn is_draw_list(&self)->bool{
        if let Self::DrawList = self{true} else{false}
    }
    fn needs_draw_list(&self)->bool{
        return self.is_texture() ||self.is_draw_list()
    }
}

#[derive(Live)]
pub struct Frame { // draw info per UI element
    #[live] draw_bg: DrawColor,
    
    #[live(false)] show_bg: bool,
    
    #[live] layout: Layout,
    
    #[live] walk: Walk,
    
    #[live] image: LiveDependency,
    #[live] image_texture: Option<Texture>,
    #[live] image_scale: f64,
    
    //#[live] use_cache: bool,
    #[live] dpi_factor: Option<f64>,
    
    #[live] optimize: FrameOptimize,
    
    #[live(true)] visible: bool,
    
    #[live(false)] block_signal_event: bool,
    #[live] cursor: Option<MouseCursor>,
    #[live] scroll_bars: Option<LivePtr>,
    #[live(false)] design_mode: bool,
    
    #[rust] find_cache: HashMap<u64, WidgetSet>,
    
    #[rust] scroll_bars_obj: Option<Box<ScrollBars >>,
    #[rust] view_size: Option<DVec2>,
    
    #[rust] area: Area,
    #[rust] draw_list: Option<DrawList2d>,
    
    #[rust] texture_cache: Option<FrameTextureCache>,
    #[rust] defer_walks: Vec<(LiveId, DeferWalk)>,
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] children: ComponentMap<LiveId, WidgetRef>,
    #[rust] draw_order: Vec<LiveId>,
    
    #[state] state: LiveState,
}

struct FrameTextureCache {
    pass: Pass,
    _depth_texture: Texture,
    color_texture: Texture,
}

impl ImageLoadingWidget for Frame {
    fn image_filename(&self) -> &LiveDependency {
        &self.image
    }

    fn get_texture(&self) -> &Option<Texture> {
        &self.image_texture
    }

    fn set_texture(&mut self, texture: Option<Texture>) {
        self.image_texture = texture;
    }
}

impl LiveHook for Frame {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Frame)
    }
    
    fn before_apply(&mut self,  _cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = from{
            self.draw_order.clear();
        }
    }
    
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if self.optimize.needs_draw_list() && self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        if self.scroll_bars.is_some() {
            if self.scroll_bars_obj.is_none() {
                self.scroll_bars_obj = Some(Box::new(ScrollBars::new_from_ptr(cx, self.scroll_bars)));
            }
        }

        self.after_apply_for_image_loading_widget(cx, from, index, nodes);

        if let Some(image_texture) = &mut self.image_texture {
            if self.image_scale != 0.0 {
                let texture_desc = image_texture.get_desc(cx);
                self.walk = Walk::fixed_size(
                    DVec2 {
                        x: texture_desc.width.unwrap() as f64 * self.image_scale,
                        y: texture_desc.height.unwrap() as f64 * self.image_scale
                    }
                );
            }
        }
    }
    
    fn apply_value_instance(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        //! TODO
        // NOTE FOR LIVE RELOAD
        // the id is always unique
        // Draw order is never cleared.
        
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
            ApplyFrom::NewFromDoc {..} | ApplyFrom::UpdateFromDoc {..} => {
                /*if !self.design_mode && nodes[index].origin.has_prop_type(LivePropType::Template) {
                    // lets store a pointer into our templates.
                    let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                    self.templates.insert(id, (live_ptr, self.draw_order.len()));
                    nodes.skip_node(index)
                }
                else */if nodes[index].origin.has_prop_type(LivePropType::Instance)
                /*|| self.design_mode && nodes[index].origin.has_prop_type(LivePropType::Template) */ {
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


#[derive(Clone, WidgetSet)]
pub struct FrameSet(WidgetSet);

#[derive(Clone, WidgetAction)]
pub enum FrameAction {
    None,
    FingerDown(FingerDownEvent),
    FingerUp(FingerUpEvent),
    FingerMove(FingerMoveEvent)
}

impl FrameRef {
    pub fn cut_state(&self, cx: &mut Cx, state: &[LiveId; 2]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.cut_state(cx, state);
        }
    }
    
    pub fn animate_state(&self, cx: &mut Cx, state: &[LiveId; 2]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animate_state(cx, state);
        }
    }
    
    pub fn toggle_state(&self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: &[LiveId; 2], state2: &[LiveId; 2]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle_state(cx, is_state_1, animate, state1, state2);
        }
    }
    
    pub fn set_visible(&self, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.visible = visible
        }
    }
    
    pub fn set_texture(&self, slot: usize, texture: &Texture) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_texture(slot, texture);
        }
    }
    
    pub fn set_uniform(&self, cx: &Cx, uniform: &[LiveId], value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_uniform(cx, uniform, value);
        }
    }
    
    pub fn set_scroll_pos(&self, cx: &mut Cx, v: DVec2) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_scroll_pos(cx, v)
        }
    }
    
    pub fn area(&self) -> Area {
        if let Some(inner) = self.borrow_mut() {
            inner.area
        }
        else {
            Area::Empty
        }
    }
    
    pub fn child_count(&self) -> usize {
        if let Some(inner) = self.borrow_mut() {
            inner.draw_order.len()
        }
        else {
            0
        }
    }
}

impl FrameSet {
    
    pub fn cut_state(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        for item in self.iter() {
            item.cut_state(cx, state)
        }
    }
    
    pub fn animate_state(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        for item in self.iter() {
            item.animate_state(cx, state);
        }
    }
    
    pub fn toggle_state(&mut self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: &[LiveId; 2], state2: &[LiveId; 2]) {
        for item in self.iter() {
            item.toggle_state(cx, is_state_1, animate, state1, state2);
        }
    }

    pub fn set_visible(&self, visible: bool) {
        for item in self.iter() {
            item.set_visible(visible)
        }
    }
    
    pub fn set_texture(&self, slot: usize, texture: &Texture) {
        for item in self.iter() {
            item.set_texture(slot, texture)
        }
    }
    
    pub fn set_uniform(&self, cx: &Cx, uniform: &[LiveId], value: &[f32]) {
        for item in self.iter() {
            item.set_uniform(cx, uniform, value)
        }
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.redraw(cx);
        }
    }
}

impl Widget for Frame {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.state_handle_event(cx, event);
        
        if self.block_signal_event {
            if let Event::Signal = event {
                return
            }
        }
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
                if child.is_visible() || !event.requires_visibility() {
                    child.handle_widget_event_with(cx, event, dispatch_action);
                }
            }
        }
        if self.cursor.is_some() || self.state.live_ptr.is_some(){
            match event.hits(cx, self.area()) {
                Hit::FingerDown(d) => {
                    cx.set_key_focus(Area::Empty);
                    dispatch_action(cx, FrameAction::FingerDown(d).into_action(uid))
                }
                Hit::FingerMove(d) => {
                    dispatch_action(cx, FrameAction::FingerMove(d).into_action(uid))
                }
                Hit::FingerUp(d) => {
                    dispatch_action(cx, FrameAction::FingerUp(d).into_action(uid))
                }
                Hit::FingerHoverIn(_) => {
                    if let Some(cursor) = &self.cursor{
                        cx.set_cursor(*cursor);
                    }
                    if self.state.live_ptr.is_some(){
                        self.animate_state(cx, id!(hover.on));
                    }
                }
                Hit::FingerHoverOut(_)=>{
                    if self.state.live_ptr.is_some(){
                        self.animate_state(cx, id!(hover.off));
                    }
                }
                _ => ()
            }
        }
        
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.handle_scroll_event(cx, event, &mut | _, _ | {});
        }
    }
    
    fn is_visible(&self) -> bool {
        self.visible
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk)
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        for child in self.children.values_mut() {
            child.redraw(cx);
        }
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        match cached {
            WidgetCache::Yes | WidgetCache::Clear => {
                if let WidgetCache::Clear = cached {
                    self.find_cache.clear();
                }
                let mut hash = 0u64;
                for i in 0..path.len() {
                    hash ^= path[i].0
                }
                if let Some(widget_set) = self.find_cache.get(&hash) {
                    results.extend_from_set(widget_set);
                    return
                }
                let mut local_results = WidgetSet::empty();
                if let Some(child) = self.children.get_mut(&path[0]) {
                    if path.len()>1 {
                        child.find_widgets(&path[1..], WidgetCache::No, &mut local_results);
                    }
                    else {
                        local_results.push(child.clone());
                    }
                }
                for child in self.children.values_mut() {
                    child.find_widgets(path, WidgetCache::No, &mut local_results);
                }
                if !local_results.is_empty() {
                    results.extend_from_set(&local_results);
                }
                self.find_cache.insert(hash, local_results);
            }
            WidgetCache::No => {
                if let Some(child) = self.children.get_mut(&path[0]) {
                    if path.len()>1 {
                        child.find_widgets(&path[1..], WidgetCache::No, results);
                    }
                    else {
                        results.push(child.clone());
                    }
                }
                for child in self.children.values_mut() {
                    child.find_widgets(path, WidgetCache::No, results);
                }
            }
        }
    }
}

#[derive(Clone)]
enum DrawState {
    Drawing(usize, bool),
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
    
    pub fn walk_from_previous_size(&self, walk: Walk) -> Walk {
        let view_size = self.view_size.unwrap_or(DVec2::default());
        Walk {
            abs_pos: walk.abs_pos,
            width: if walk.width.is_fill() {walk.width}else {Size::Fixed(view_size.x)},
            height: if walk.height.is_fill() {walk.height}else {Size::Fixed(view_size.y)},
            margin: walk.margin
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        // the beginning state
        if self.draw_state.begin(cx, DrawState::Drawing(0, false)) {
            if !self.visible {
                self.draw_state.end();
                return WidgetDraw::done()
            }
            
            self.defer_walks.clear();
            
            match self.optimize{
                FrameOptimize::Texture=>{
                    let walk = self.walk_from_previous_size(walk);
                    if !cx.will_redraw(self.draw_list.as_mut().unwrap(), walk) {
                        if let Some(texture_cache) = &self.texture_cache {
                            self.draw_bg.draw_vars.set_texture(0, &texture_cache.color_texture);
                            let mut rect = cx.walk_turtle_with_area(&mut self.area, walk);
                            rect.size *= 2.0 / self.dpi_factor.unwrap_or(1.0);
                            self.draw_bg.draw_abs(cx, rect);
                            self.area = self.draw_bg.area();
                            cx.set_pass_scaled_area(&texture_cache.pass, self.area, 2.0 / self.dpi_factor.unwrap_or(1.0));
                        }
                        return WidgetDraw::done()
                    }
                    // lets start a pass
                    if self.texture_cache.is_none() {
                        self.texture_cache = Some(FrameTextureCache {
                            pass: Pass::new(cx),
                            _depth_texture: Texture::new(cx),
                            color_texture: Texture::new(cx)
                        });
                        let texture_cache = self.texture_cache.as_mut().unwrap();
                        //cache.pass.set_depth_texture(cx, &cache.depth_texture, PassClearDepth::ClearWith(1.0));
                        texture_cache.pass.add_color_texture(cx, &texture_cache.color_texture, PassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
                    }
                    let texture_cache = self.texture_cache.as_mut().unwrap();
                    cx.make_child_pass(&texture_cache.pass);
                    cx.begin_pass(&texture_cache.pass, self.dpi_factor);
                    self.draw_list.as_mut().unwrap().begin_always(cx)
                }
                FrameOptimize::DrawList=>{
                    let walk = self.walk_from_previous_size(walk);
                    if self.draw_list.as_mut().unwrap().begin(cx, walk).is_not_redrawing() {
                        cx.walk_turtle_with_area(&mut self.area, walk);
                        return WidgetDraw::done()
                    }
                }
                _=>()
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
                if let Some(image_texture) = &self.image_texture {
                    self.draw_bg.draw_vars.set_texture(0, image_texture);
                }
                self.draw_bg.begin(cx, walk, self.layout.with_scroll(scroll).with_scale(2.0 / self.dpi_factor.unwrap_or(2.0)));
            }
            else {
                cx.begin_turtle(walk, self.layout.with_scroll(scroll).with_scale(2.0 / self.dpi_factor.unwrap_or(2.0)));
            }
        }
        
        while let Some(DrawState::Drawing(step, resume)) = self.draw_state.get() {
            if step < self.draw_order.len() {
                let id = self.draw_order[step];
                if let Some(child) = self.children.get_mut(&id) {
                    if child.is_visible() {
                        let walk = child.get_walk();
                        if resume {
                            child.draw_walk_widget(cx, walk) ?;
                        }
                        else if let Some(fw) = cx.defer_walk(walk) {
                            self.defer_walks.push((id, fw));
                        }
                        else {
                            self.draw_state.set(DrawState::Drawing(step, true));
                            child.draw_walk_widget(cx, walk) ?;
                        }
                    }
                }
                self.draw_state.set(DrawState::Drawing(step + 1, false));
            }
            else {
                self.draw_state.set(DrawState::DeferWalk(0));
            }
        }
        
        while let Some(DrawState::DeferWalk(step)) = self.draw_state.get() {
            if step < self.defer_walks.len() {
                let (id, dw) = &mut self.defer_walks[step];
                if let Some(child) = self.children.get_mut(&id) {
                    let walk = dw.resolve(cx);
                    child.draw_walk_widget(cx, walk) ?;
                }
                self.draw_state.set(DrawState::DeferWalk(step + 1));
            }
            else {
                if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                    scroll_bars.draw_scroll_bars(cx);
                };
                
                if self.show_bg {
                    if self.optimize.is_texture() {
                        panic!("dont use show_bg and texture cazching at the same time");
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
                
                if self.optimize.needs_draw_list() {
                    let rect = self.area.get_rect(cx);
                    self.view_size = Some(rect.size);
                    self.draw_list.as_mut().unwrap().end(cx);
                    
                    if self.optimize.is_texture() {
                        let texture_cache = self.texture_cache.as_mut().unwrap();
                        cx.end_pass(&texture_cache.pass);
                        /*if cache.pass.id_equals(4){
                            self.draw_bg.draw_vars.set_uniform(cx, id!(marked),&[1.0]);
                        }
                        else{
                            self.draw_bg.draw_vars.set_uniform(cx, id!(marked),&[0.0]);
                        }*/
                        self.draw_bg.draw_vars.set_texture(0, &texture_cache.color_texture);
                        self.draw_bg.draw_abs(cx, rect);
                        let area = self.draw_bg.area();
                        let texture_cache = self.texture_cache.as_mut().unwrap();
                        cx.set_pass_scaled_area(&texture_cache.pass, area, 2.0 / self.dpi_factor.unwrap_or(1.0));
                    }
                }
                self.draw_state.end();
            }
        }
        WidgetDraw::done()
    }
    
    pub fn child_count(&self) -> usize {
        self.draw_order.len()
    }
}

