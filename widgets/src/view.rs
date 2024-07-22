use {
    crate::{makepad_derive_widget::*, makepad_draw::*, scroll_bars::ScrollBars, widget::*},
    std::collections::hash_map::HashMap,
};

live_design! {
    ViewBase = {{View}} {debug:None}
}

// maybe we should put an enum on the bools like

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum ViewOptimize {
    #[pick]
    None,
    DrawList,
    Texture,
}

#[derive(Live)]
#[live_ignore]
pub enum ViewDebug {
    #[pick]
    None,
    R,
    G,
    B,
    M,
    Margin,
    P,
    Padding,
    A,
    All,
    #[live(Vec4::default())]
    Color(Vec4),
}

impl LiveHook for ViewDebug {
    fn skip_apply(
        &mut self,
        _cx: &mut Cx,
        _apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Vec4(v) => {
                *self = Self::Color(*v);
                Some(index + 1)
            }
            LiveValue::Color(v) => {
                *self = Self::Color(Vec4::from_u32(*v));
                Some(index + 1)
            }
            LiveValue::Bool(v) => {
                if *v {
                    *self = Self::R;
                } else {
                    *self = Self::None;
                }
                Some(index + 1)
            }
            LiveValue::Float64(v) => {
                if *v != 0.0 {
                    *self = Self::R;
                } else {
                    *self = Self::None;
                }
                Some(index + 1)
            }
            LiveValue::Int64(v) => {
                if *v != 0 {
                    *self = Self::R;
                } else {
                    *self = Self::None;
                }
                Some(index + 1)
            }
            _ => None,
        }
    }
}

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum EventOrder {
    Down,
    #[pick]
    Up,
    #[live(Default::default())]
    List(Vec<LiveId>),
}

impl ViewOptimize {
    fn is_texture(&self) -> bool {
        if let Self::Texture = self {
            true
        } else {
            false
        }
    }
    fn is_draw_list(&self) -> bool {
        if let Self::DrawList = self {
            true
        } else {
            false
        }
    }
    fn needs_draw_list(&self) -> bool {
        return self.is_texture() || self.is_draw_list();
    }
}

#[derive(Live, LiveRegisterWidget, WidgetRef, WidgetSet)]
pub struct View {
    // draw info per UI element
    #[live]
    pub draw_bg: DrawColor,

    #[live(false)]
    pub show_bg: bool,

    #[layout]
    pub layout: Layout,

    #[walk]
    pub walk: Walk,

    //#[live] use_cache: bool,
    #[live]
    dpi_factor: Option<f64>,

    #[live]
    optimize: ViewOptimize,
    #[live]
    debug: ViewDebug,
    #[live]
    event_order: EventOrder,

    #[live(true)]
    pub visible: bool,

    #[live(true)]
    grab_key_focus: bool,
    #[live(false)]
    block_signal_event: bool,
    #[live]
    cursor: Option<MouseCursor>,
    #[live(false)]
    capture_overload: bool,
    #[live]
    scroll_bars: Option<LivePtr>,
    #[live(false)]
    design_mode: bool,

    #[rust]
    find_cache: HashMap<u64, WidgetSet>,

    #[rust]
    scroll_bars_obj: Option<Box<ScrollBars>>,
    #[rust]
    view_size: Option<DVec2>,

    #[rust]
    area: Area,
    #[rust]
    draw_list: Option<DrawList2d>,

    #[rust]
    texture_cache: Option<ViewTextureCache>,
    #[rust]
    defer_walks: Vec<(LiveId, DeferWalk)>,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    children: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    draw_order: Vec<LiveId>,

    #[animator]
    animator: Animator,
}

struct ViewTextureCache {
    pass: Pass,
    _depth_texture: Texture,
    color_texture: Texture,
}

impl LiveHook for View {
    fn before_apply(
        &mut self,
        _cx: &mut Cx,
        apply: &mut Apply,
        _index: usize,
        _nodes: &[LiveNode],
    ) {
        if let ApplyFrom::UpdateFromDoc { .. } = apply.from {
            self.draw_order.clear();
            self.find_cache.clear();
        }
    }

    fn after_apply(
        &mut self,
        cx: &mut Cx,
        _applyl: &mut Apply,
        _index: usize,
        _nodes: &[LiveNode],
    ) {
        if self.optimize.needs_draw_list() && self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        if self.scroll_bars.is_some() {
            if self.scroll_bars_obj.is_none() {
                self.scroll_bars_obj =
                    Some(Box::new(ScrollBars::new_from_ptr(cx, self.scroll_bars)));
            }
        }
    }

    fn apply_value_instance(
        &mut self,
        cx: &mut Cx,
        apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> usize { 

        let id = nodes[index].id;
        match apply.from {
            ApplyFrom::Animate | ApplyFrom::Over => {
                if let Some(component) = self.children.get_mut(&nodes[index].id) {
                    component.apply(cx, apply, index, nodes)
                } else {
                    nodes.skip_node(index)
                }
            }
            ApplyFrom::NewFromDoc { .. } | ApplyFrom::UpdateFromDoc { .. } => {
                if nodes[index].is_instance_prop() {
                    self.draw_order.push(id);
                    return self
                        .children
                        .get_or_insert(cx, id, |cx| WidgetRef::new(cx))
                        .apply(cx, apply, index, nodes);
                } else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    nodes.skip_node(index)
                }
            }
            _ => nodes.skip_node(index),
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum ViewAction {
    None,
    FingerDown(FingerDownEvent),
    FingerUp(FingerUpEvent),
    FingerMove(FingerMoveEvent),
    FingerHoverIn(FingerHoverEvent),
    FingerHoverOut(FingerHoverEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
}

impl ViewRef {
    pub fn finger_down(&self, actions: &Actions) -> Option<FingerDownEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::FingerDown(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn finger_up(&self, actions: &Actions) -> Option<FingerUpEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::FingerUp(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn finger_move(&self, actions: &Actions) -> Option<FingerMoveEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::FingerMove(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn finger_hover_in(&self, actions: &Actions) -> Option<FingerHoverEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::FingerHoverIn(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn finger_hover_out(&self, actions: &Actions) -> Option<FingerHoverEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::FingerHoverOut(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn key_down(&self, actions: &Actions) -> Option<KeyEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::KeyDown(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn key_up(&self, actions: &Actions) -> Option<KeyEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ViewAction::KeyUp(fd) = item.cast() {
                return Some(fd);
            }
        }
        None
    }

    pub fn animator_cut(&self, cx: &mut Cx, state: &[LiveId; 2]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_cut(cx, state);
        }
    }

    pub fn animator_play(&self, cx: &mut Cx, state: &[LiveId; 2]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play(cx, state);
        }
    }

    pub fn toggle_state(
        &self,
        cx: &mut Cx,
        is_state_1: bool,
        animate: Animate,
        state1: &[LiveId; 2],
        state2: &[LiveId; 2],
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_toggle(cx, is_state_1, animate, state1, state2);
        }
    }

    pub fn set_visible(&self, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.visible = visible
        }
    }

    pub fn set_visible_and_redraw(&self, cx: &mut Cx, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.visible = visible;
            inner.redraw(cx);
        }
    }

    pub fn visible(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.visible
        } else {
            false
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
        } else {
            Area::Empty
        }
    }

    pub fn child_count(&self) -> usize {
        if let Some(inner) = self.borrow_mut() {
            inner.draw_order.len()
        } else {
            0
        }
    }
}

impl ViewSet {
    pub fn animator_cut(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        for item in self.iter() {
            item.animator_cut(cx, state)
        }
    }

    pub fn animator_play(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        for item in self.iter() {
            item.animator_play(cx, state);
        }
    }

    pub fn toggle_state(
        &mut self,
        cx: &mut Cx,
        is_state_1: bool,
        animate: Animate,
        state1: &[LiveId; 2],
        state2: &[LiveId; 2],
    ) {
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

    pub fn finger_down(&self, actions: &Actions) -> Option<FingerDownEvent> {
        for item in self.iter() {
            if let Some(e) = item.finger_down(actions) {
                return Some(e);
            }
        }
        None
    }

    pub fn finger_up(&self, actions: &Actions) -> Option<FingerUpEvent> {
        for item in self.iter() {
            if let Some(e) = item.finger_up(actions) {
                return Some(e);
            }
        }
        None
    }

    pub fn finger_move(&self, actions: &Actions) -> Option<FingerMoveEvent> {
        for item in self.iter() {
            if let Some(e) = item.finger_move(actions) {
                return Some(e);
            }
        }
        None
    }

    pub fn key_down(&self, actions: &Actions) -> Option<KeyEvent> {
        for item in self.iter() {
            if let Some(e) = item.key_down(actions) {
                return Some(e);
            }
        }
        None
    }

    pub fn key_up(&self, actions: &Actions) -> Option<KeyEvent> {
        for item in self.iter() {
            if let Some(e) = item.key_up(actions) {
                return Some(e);
            }
        }
        None
    }
}

impl WidgetNode for View {
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
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
                    return;
                }
                let mut local_results = WidgetSet::empty();
                if let Some(child) = self.children.get_mut(&path[0]) {
                    if path.len() > 1 {
                        child.find_widgets(&path[1..], WidgetCache::No, &mut local_results);
                    } else {
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
                    if path.len() > 1 {
                        child.find_widgets(&path[1..], WidgetCache::No, results);
                    } else {
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

impl Widget for View {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }

        if self.block_signal_event {
            if let Event::Signal = event {
                return;
            }
        }
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            let mut actions = Vec::new();
            scroll_bars.handle_main_event(cx, event, &mut actions);
            if actions.len() > 0 {
                cx.redraw_area_and_children(self.area);
            };
        }

        match &self.event_order {
            EventOrder::Up => {
                for id in self.draw_order.iter().rev() {
                    if let Some(child) = self.children.get_mut(id) {
                        if child.is_visible() || !event.requires_visibility() {
                            scope.with_id(*id, |scope| {
                                child.handle_event(cx, event, scope);
                            });
                        }
                    }
                }
            }
            EventOrder::Down => {
                for id in self.draw_order.iter() {
                    if let Some(child) = self.children.get_mut(id) {
                        if child.is_visible() || !event.requires_visibility() {
                            scope.with_id(*id, |scope| {
                                child.handle_event(cx, event, scope);
                            })
                        }
                    }
                }
            }
            EventOrder::List(list) => {
                for id in list {
                    if let Some(child) = self.children.get_mut(id) {
                        if child.is_visible() || !event.requires_visibility() {
                            scope.with_id(*id, |scope| {
                                child.handle_event(cx, event, scope);
                            })
                        }
                    }
                }
            }
        }

        if self.visible && self.cursor.is_some() || self.animator.live_ptr.is_some() {
            match event.hits_with_capture_overload(cx, self.area(), self.capture_overload) {
                Hit::FingerDown(e) => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area());
                    }
                    cx.widget_action(uid, &scope.path, ViewAction::FingerDown(e));
                    if self.animator.live_ptr.is_some() {
                        self.animator_play(cx, id!(down.on));
                    }
                }
                Hit::FingerMove(e) => cx.widget_action(uid, &scope.path, ViewAction::FingerMove(e)),
                Hit::FingerUp(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerUp(e));
                    if self.animator.live_ptr.is_some() {
                        self.animator_play(cx, id!(down.off));
                    }
                }
                Hit::FingerHoverIn(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerHoverIn(e));
                    if let Some(cursor) = &self.cursor {
                        cx.set_cursor(*cursor);
                    }
                    if self.animator.live_ptr.is_some() {
                        self.animator_play(cx, id!(hover.on));
                    }
                }
                Hit::FingerHoverOut(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerHoverOut(e));
                    if self.animator.live_ptr.is_some() {
                        self.animator_play(cx, id!(hover.off));
                    }
                }
                Hit::KeyDown(e) => cx.widget_action(uid, &scope.path, ViewAction::KeyDown(e)),
                Hit::KeyUp(e) => cx.widget_action(uid, &scope.path, ViewAction::KeyUp(e)),
                _ => (),
            }
        }

        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.handle_scroll_event(cx, event, &mut Vec::new());
        }
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // the beginning state
        if self.draw_state.begin(cx, DrawState::Drawing(0, false)) {
            if !self.visible {
                self.draw_state.end();
                return DrawStep::done();
            }

            self.defer_walks.clear();

            match self.optimize {
                ViewOptimize::Texture => {
                    let walk = self.walk_from_previous_size(walk);
                    if !cx.will_redraw(self.draw_list.as_mut().unwrap(), walk) {
                        if let Some(texture_cache) = &self.texture_cache {
                            self.draw_bg
                                .draw_vars
                                .set_texture(0, &texture_cache.color_texture);
                            let mut rect = cx.walk_turtle_with_area(&mut self.area, walk);
                            // NOTE(eddyb) see comment lower below for why this is
                            // disabled (it used to match `set_pass_scaled_area`).
                            if false {
                                rect.size *= 2.0 / self.dpi_factor.unwrap_or(1.0);
                            }
                            self.draw_bg.draw_abs(cx, rect);
                            self.area = self.draw_bg.area();
                            /*if false {
                                // FIXME(eddyb) this was the previous logic,
                                // but the only tested apps that use `CachedView`
                                // are sized correctly (regardless of `dpi_factor`)
                                // *without* extra scaling here.
                                cx.set_pass_scaled_area(
                                    &texture_cache.pass,
                                    self.area,
                                    2.0 / self.dpi_factor.unwrap_or(1.0),
                                );
                            } else {*/
                                cx.set_pass_area(
                                    &texture_cache.pass,
                                    self.area,
                                );
                            //}
                        }
                        return DrawStep::done();
                    }
                    // lets start a pass
                    if self.texture_cache.is_none() {
                        self.texture_cache = Some(ViewTextureCache {
                            pass: Pass::new(cx),
                            _depth_texture: Texture::new(cx),
                            color_texture: Texture::new(cx),
                        });
                        let texture_cache = self.texture_cache.as_mut().unwrap();
                        //cache.pass.set_depth_texture(cx, &cache.depth_texture, PassClearDepth::ClearWith(1.0));
                        texture_cache.color_texture = Texture::new_with_format(
                            cx,
                            TextureFormat::RenderBGRAu8 {
                                size: TextureSize::Auto,
                            },
                        );
                        texture_cache.pass.add_color_texture(
                            cx,
                            &texture_cache.color_texture,
                            PassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
                        );
                    }
                    let texture_cache = self.texture_cache.as_mut().unwrap();
                    cx.make_child_pass(&texture_cache.pass);
                    cx.begin_pass(&texture_cache.pass, self.dpi_factor);
                    self.draw_list.as_mut().unwrap().begin_always(cx)
                }
                ViewOptimize::DrawList => {
                    let walk = self.walk_from_previous_size(walk);
                    if self
                        .draw_list
                        .as_mut()
                        .unwrap()
                        .begin(cx, walk)
                        .is_not_redrawing()
                    {
                        cx.walk_turtle_with_area(&mut self.area, walk);
                        return DrawStep::done();
                    }
                }
                _ => (),
            }

            // ok so.. we have to keep calling draw till we return LiveId(0)
            let scroll = if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                scroll_bars.begin_nav_area(cx);
                scroll_bars.get_scroll_pos()
            } else {
                self.layout.scroll
            };

            if self.show_bg {
                /*if let Some(image_texture) = &self.image_texture {
                    self.draw_bg.draw_vars.set_texture(0, image_texture);
                }*/
                self.draw_bg
                    .begin(cx, walk, self.layout.with_scroll(scroll)); //.with_scale(2.0 / self.dpi_factor.unwrap_or(2.0)));
            } else {
                cx.begin_turtle(walk, self.layout.with_scroll(scroll)); //.with_scale(2.0 / self.dpi_factor.unwrap_or(2.0)));
            }
        }

        while let Some(DrawState::Drawing(step, resume)) = self.draw_state.get() {
            if step < self.draw_order.len() {
                let id = self.draw_order[step];
                if let Some(child) = self.children.get_mut(&id) {
                    if child.is_visible() {
                        let walk = child.walk(cx);
                        if resume {
                            scope.with_id(id, |scope| child.draw_walk(cx, scope, walk))?;
                        } else if let Some(fw) = cx.defer_walk(walk) {
                            self.defer_walks.push((id, fw));
                        } else {
                            self.draw_state.set(DrawState::Drawing(step, true));
                            scope.with_id(id, |scope| child.draw_walk(cx, scope, walk))?;
                        }
                    }
                }
                self.draw_state.set(DrawState::Drawing(step + 1, false));
            } else {
                self.draw_state.set(DrawState::DeferWalk(0));
            }
        }

        while let Some(DrawState::DeferWalk(step)) = self.draw_state.get() {
            if step < self.defer_walks.len() {
                let (id, dw) = &mut self.defer_walks[step];
                if let Some(child) = self.children.get_mut(&id) {
                    let walk = dw.resolve(cx);
                    scope.with_id(*id, |scope| child.draw_walk(cx, scope, walk))?;
                }
                self.draw_state.set(DrawState::DeferWalk(step + 1));
            } else {
                if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                    scroll_bars.draw_scroll_bars(cx);
                };

                if self.show_bg {
                    if self.optimize.is_texture() {
                        panic!("dont use show_bg and texture caching at the same time");
                    }
                    self.draw_bg.end(cx);
                    self.area = self.draw_bg.area();
                } else {
                    cx.end_turtle_with_area(&mut self.area);
                };

                if let Some(scroll_bars) = &mut self.scroll_bars_obj {
                    scroll_bars.set_area(self.area);
                    scroll_bars.end_nav_area(cx);
                };

                if self.optimize.needs_draw_list() {
                    let rect = self.area.rect(cx);
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
                        self.draw_bg
                            .draw_vars
                            .set_texture(0, &texture_cache.color_texture);
                        self.draw_bg.draw_abs(cx, rect);
                        let area = self.draw_bg.area();
                        let texture_cache = self.texture_cache.as_mut().unwrap();
                       /* if false {
                            // FIXME(eddyb) this was the previous logic,
                            // but the only tested apps that use `CachedView`
                            // are sized correctly (regardless of `dpi_factor`)
                            // *without* extra scaling here.
                            cx.set_pass_scaled_area(
                                &texture_cache.pass,
                                area,
                                2.0 / self.dpi_factor.unwrap_or(1.0),
                            );
                        } else {*/
                            cx.set_pass_area(
                                &texture_cache.pass,
                                area,
                            );
                        //}
                    }
                }
                self.draw_state.end();
            }
        }
        match &self.debug {
            ViewDebug::None => {}
            ViewDebug::Color(c) => {
                cx.debug.area(self.area, *c);
            }
            ViewDebug::R => {
                cx.debug.area(self.area, Vec4::R);
            }
            ViewDebug::G => {
                cx.debug.area(self.area, Vec4::G);
            }
            ViewDebug::B => {
                cx.debug.area(self.area, Vec4::B);
            }
            ViewDebug::M | ViewDebug::Margin => {
                let tl = dvec2(self.walk.margin.left, self.walk.margin.top);
                let br = dvec2(self.walk.margin.right, self.walk.margin.bottom);
                cx.debug.area_offset(self.area, tl, br, Vec4::B);
                cx.debug.area(self.area, Vec4::R);
            }
            ViewDebug::P | ViewDebug::Padding => {
                let tl = dvec2(-self.layout.padding.left, -self.walk.margin.top);
                let br = dvec2(-self.layout.padding.right, -self.layout.padding.bottom);
                cx.debug.area_offset(self.area, tl, br, Vec4::G);
                cx.debug.area(self.area, Vec4::R);
            }
            ViewDebug::All | ViewDebug::A => {
                let tl = dvec2(self.walk.margin.left, self.walk.margin.top);
                let br = dvec2(self.walk.margin.right, self.walk.margin.bottom);
                cx.debug.area_offset(self.area, tl, br, Vec4::B);
                let tl = dvec2(-self.layout.padding.left, -self.walk.margin.top);
                let br = dvec2(-self.layout.padding.right, -self.layout.padding.bottom);
                cx.debug.area_offset(self.area, tl, br, Vec4::G);
                cx.debug.area(self.area, Vec4::R);
            }
        }
        DrawStep::done()
    }
}

#[derive(Clone)]
enum DrawState {
    Drawing(usize, bool),
    DeferWalk(usize),
}

impl View {
    pub fn set_scroll_pos(&mut self, cx: &mut Cx, v: DVec2) {
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.set_scroll_pos(cx, v);
        } else {
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
            width: if walk.width.is_fill() {
                walk.width
            } else {
                Size::Fixed(view_size.x)
            },
            height: if walk.height.is_fill() {
                walk.height
            } else {
                Size::Fixed(view_size.y)
            },
            margin: walk.margin,
        }
    }

    pub fn child_count(&self) -> usize {
        self.draw_order.len()
    }
    
    pub fn debug_print_children(&self){
        log!("Debug print view children {:?}", self.children.len());
        for i in 0..self.draw_order.len(){
            log!("Child: {}",self.draw_order[i])
        }
    }
}
