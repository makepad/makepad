use {
    crate::makepad_draw::event::FingerLongPressEvent,
    crate::{
        animator::*, makepad_derive_widget::*, makepad_draw::*, makepad_script::ScriptFnRef,
        scroll_bars::ScrollBars, widget::*,
    },
    std::cell::RefCell,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ViewBase = set_type_default() do #(View::register_widget(vm))
}

// maybe we should put an enum on the bools like

#[derive(Script, ScriptHook, Clone, Copy)]
pub enum ViewOptimize {
    #[pick]
    None,
    DrawList,
    Texture,
}

impl Default for ViewOptimize {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Script, ScriptHook)]
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

#[derive(Script, Animator, WidgetRef, WidgetSet, WidgetRegister)]
pub struct View {
    #[source]
    source: ScriptObjectRef,
    // draw info per UI element
    #[live]
    pub draw_bg: DrawQuad,

    #[live(false)]
    pub show_bg: bool,

    #[layout]
    pub layout: Layout,

    #[walk]
    pub walk: Walk,

    //#[live] use_cache: bool,
    #[live]
    dpi_factor: Option<f64>,

    #[live(false)]
    pub new_batch: bool,

    #[live(false)]
    pub texture_caching: bool,

    #[rust]
    optimize: ViewOptimize,
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
    scroll_bars: ScriptObjectRef,

    #[live(false)]
    design_mode: bool,

    #[live(false)]
    pub debug_stream: bool,

    #[live]
    render: Option<ScriptFnRef>,

    #[rust]
    find_cache: RefCell<SmallVec<[(u64, WidgetSet); 3]>>,

    #[rust]
    scroll_bars_obj: Option<Box<ScrollBars>>,
    #[rust]
    view_size: Option<Vec2d>,

    #[rust]
    area: Area,
    #[rust]
    draw_list: Option<DrawList2d>,

    #[rust]
    texture_cache: Option<ViewTextureCache>,
    #[rust]
    defer_walks: SmallVec<[(LiveId, DeferredWalk); 1]>,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    pub children: SmallVec<[(LiveId, WidgetRef); 2]>,
    #[rust]
    live_update_order: SmallVec<[LiveId; 1]>,

    #[apply_default]
    animator: Animator,
}

struct ViewTextureCache {
    pass: DrawPass,
    _depth_texture: Texture,
    color_texture: Texture,
}

impl ScriptHook for View {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.live_update_order.clear();
            self.find_cache.get_mut().clear();
            // TEST: Clear all children to rebuild from scratch
            self.children.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Handle children from the object's vec
        // Skip for eval applies - eval should only affect this widget's own properties,
        // not propagate to children (which would use inherited vec from prototype)
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                let mut anon_index = 0usize;
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        // Determine the id: use prefixed id if available, otherwise use numbered id for anonymous children
                        let id = if let Some(id) = kv.key.as_id() {
                            Some(id)
                        } else if kv.key.is_nil() {
                            // Anonymous child widget - use numbered id
                            let id = LiveId(anon_index as u64);
                            anon_index += 1;
                            Some(id)
                        } else {
                            None
                        };

                        if let Some(id) = id {
                            if apply.is_reload() {
                                self.live_update_order.push(id);
                            }

                            if let Some((_, node)) =
                                self.children.iter_mut().find(|(id2, _)| *id2 == id)
                            {
                                node.script_apply(vm, apply, scope, kv.value);
                            } else {
                                let widget =
                                    WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                                self.children.push((id, widget));
                            }
                        }
                    }
                });
            }
        }

        if apply.is_reload() {
            // update/delete children list
            // Only reorder and truncate if we actually processed items from the vec.
            // If vec was empty but children exist, this is likely an incomplete parse
            // during streaming - preserve existing children.
            if !self.live_update_order.is_empty() || self.children.is_empty() {
                for (idx, id) in self.live_update_order.iter().enumerate() {
                    if let Some(pos) = self.children.iter().position(|(i, _v)| *i == *id) {
                        self.children.swap(idx, pos);
                    }
                }
                self.children.truncate(self.live_update_order.len());
            }
        }

        if self.texture_caching {
            self.optimize = ViewOptimize::Texture;
        } else if self.new_batch {
            self.optimize = ViewOptimize::DrawList;
        }

        if self.optimize.needs_draw_list() && self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::script_new(vm));
        }
        if !self.scroll_bars.is_zero() {
            if self.scroll_bars_obj.is_none() {
                self.scroll_bars_obj = Some(Box::new(ScrollBars::script_from_value(
                    vm,
                    self.scroll_bars.as_object().into(),
                )));
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ViewAction {
    #[default]
    None,
    FingerDown(FingerDownEvent),
    FingerUp(FingerUpEvent),
    FingerLongPress(FingerLongPressEvent),
    FingerMove(FingerMoveEvent),
    FingerHoverIn(FingerHoverEvent),
    FingerHoverOut(FingerHoverEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
}

impl View {
    pub fn set_debug_dump(&mut self, cx: &mut Cx, debug: bool) {
        if let Some(draw_list) = &self.draw_list {
            cx.draw_lists[draw_list.id()].debug_dump = debug;
        }
    }

    pub fn repaint(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            if let Some(pass_id) = cx.draw_lists[draw_list.id()].draw_pass_id {
                cx.repaint_pass(pass_id);
            }
        }
    }
}

impl ViewRef {
    pub fn set_debug_dump(&self, cx: &mut Cx, debug: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_debug_dump(cx, debug);
        }
    }

    pub fn repaint(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.repaint(cx);
        }
    }

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

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_visible(cx, visible)
        }
    }

    pub fn visible(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.visible()
        } else {
            false
        }
    }

    pub fn set_texture(&self, slot: usize, texture: &Texture) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_texture(slot, texture);
        }
    }

    pub fn set_uniform(&self, cx: &Cx, uniform: LiveId, value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_uniform(cx, uniform, value);
        }
    }

    pub fn set_scroll_pos(&self, cx: &mut Cx, v: Vec2d) {
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
            inner.children.len()
        } else {
            0
        }
    }

    pub fn set_key_focus(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow_mut() {
            inner.set_key_focus(cx);
        }
    }
}

impl ViewSet {
    pub fn animator_cut(&self, cx: &mut Cx, state: &[LiveId; 2]) {
        for item in self.iter() {
            item.animator_cut(cx, state)
        }
    }

    pub fn animator_play(&self, cx: &mut Cx, state: &[LiveId; 2]) {
        for item in self.iter() {
            item.animator_play(cx, state);
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
        for item in self.iter() {
            item.toggle_state(cx, is_state_1, animate, state1, state2);
        }
    }

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        for item in self.iter() {
            item.set_visible(cx, visible)
        }
    }

    pub fn set_texture(&self, slot: usize, texture: &Texture) {
        for item in self.iter() {
            item.set_texture(slot, texture)
        }
    }

    pub fn set_uniform(&self, cx: &Cx, uniform: LiveId, value: &[f32]) {
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

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        for (_, child) in &mut self.children {
            child.redraw(cx);
        }
    }

    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        for (_, child) in &self.children {
            let x = child.uid_to_widget(uid);
            if !x.is_empty() {
                return x;
            }
        }
        WidgetRef::empty()
    }

    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        match cached {
            WidgetCache::Yes | WidgetCache::Clear => {
                if let WidgetCache::Clear = cached {
                    self.find_cache.borrow_mut().clear();
                    if path.len() == 0 {
                        return;
                    }
                }
                let mut hash = 0u64;
                for i in 0..path.len() {
                    hash ^= path[i].0
                }
                if let Some((_, widget_set)) =
                    self.find_cache.borrow().iter().find(|(h, _v)| h == &hash)
                {
                    results.extend_from_set(widget_set);
                    /*#[cfg(not(ignore_query))]
                    if results.0.len() == 0{
                        log!("Widget query not found: {:?} on view {:?}", path, self.widget_uid());
                    }
                    #[cfg(panic_query)]
                    if results.0.len() == 0{
                        panic!("Widget query not found: {:?} on view {:?}", path, self.widget_uid());
                    }*/
                    return;
                }
                let mut local_results = WidgetSet::empty();
                if let Some((_, child)) = self.children.iter().find(|(id, _)| *id == path[0]) {
                    if path.len() > 1 {
                        child.find_widgets(&path[1..], WidgetCache::No, &mut local_results);
                    } else {
                        local_results.push(child.clone());
                    }
                }
                for (_, child) in &self.children {
                    child.find_widgets(path, WidgetCache::No, &mut local_results);
                }
                if !local_results.is_empty() {
                    results.extend_from_set(&local_results);
                }
                /* #[cfg(not(ignore_query))]
                if local_results.0.len() == 0{
                    log!("Widget query not found: {:?} on view {:?}", path, self.widget_uid());
                }
                #[cfg(panic_query)]
                if local_results.0.len() == 0{
                    panic!("Widget query not found: {:?} on view {:?}", path, self.widget_uid());
                }*/
                self.find_cache.borrow_mut().push((hash, local_results));
            }
            WidgetCache::No => {
                if let Some((_, child)) = self.children.iter().find(|(id, _)| *id == path[0]) {
                    if path.len() > 1 {
                        child.find_widgets(&path[1..], WidgetCache::No, results);
                    } else {
                        results.push(child.clone());
                    }
                }
                for (_, child) in &self.children {
                    child.find_widgets(path, WidgetCache::No, results);
                }
            }
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for (_, child) in &self.children {
            child.find_widgets_from_point(cx, point, found);
        }
    }

    fn widget_tree_walk(&self, nodes: &mut Vec<WidgetTreeNode>) {
        for (id, child) in &self.children {
            child.widget_tree_walk_named(*id, nodes);
        }
    }

    fn selection_text_len(&self) -> usize {
        for (_, child) in &self.children {
            let v = child.selection_text_len();
            if v > 0 {
                return v;
            }
        }
        0
    }

    fn selection_point_to_char_index(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        for (_, child) in &self.children {
            if let Some(v) = child.selection_point_to_char_index(cx, abs) {
                return Some(v);
            }
        }
        None
    }

    fn selection_set(&mut self, anchor: usize, cursor: usize) {
        for (_, child) in &self.children {
            child.selection_set(anchor, cursor);
        }
    }

    fn selection_clear(&mut self) {
        for (_, child) in &self.children {
            child.selection_clear();
        }
    }

    fn selection_select_all(&mut self) {
        for (_, child) in &self.children {
            child.selection_select_all();
        }
    }

    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        for (_, child) in &self.children {
            let v = child.selection_get_text_for_range(start, end);
            if !v.is_empty() {
                return v;
            }
        }
        String::new()
    }

    fn selection_get_full_text(&self) -> String {
        for (_, child) in &self.children {
            let v = child.selection_get_full_text();
            if !v.is_empty() {
                return v;
            }
        }
        String::new()
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.redraw(cx);
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }
}

impl Widget for View {
    fn is_interactive(&self) -> bool {
        self.cursor.is_some() || self.animator.is_defined
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }

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
            scroll_bars.handle_main_event(cx, event, scope, &mut actions);
            if actions.len() > 0 {
                cx.redraw_area_and_children(self.area);
            };
        }

        // If the UI tree has changed significantly (e.g. AdaptiveView varaints changed),
        // we need to clear the cache and re-query widgets.
        if cx.widget_query_invalidation_event.is_some() {
            self.find_cache.borrow_mut().clear();
        }

        match &self.event_order {
            EventOrder::Up => {
                for (id, child) in self.children.iter_mut().rev() {
                    scope.with_id(*id, |scope| {
                        child.handle_event(cx, event, scope);
                    });
                }
            }
            EventOrder::Down => {
                for (id, child) in self.children.iter_mut() {
                    scope.with_id(*id, |scope| {
                        child.handle_event(cx, event, scope);
                    })
                }
            }
            EventOrder::List(list) => {
                for id in list {
                    if let Some((_, child)) = self.children.iter_mut().find(|(id2, _)| id2 == id) {
                        scope.with_id(*id, |scope| {
                            child.handle_event(cx, event, scope);
                        })
                    }
                }
            }
        }

        match event.hit_designer(cx, self.area()) {
            HitDesigner::DesignerPick(_e) => {
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _ => (),
        }

        if self.visible && self.cursor.is_some() || self.animator.is_defined {
            match event.hits_with_capture_overload(cx, self.area(), self.capture_overload) {
                Hit::FingerDown(e) => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area());
                    }
                    cx.widget_action(uid, &scope.path, ViewAction::FingerDown(e));
                    if self.animator.is_defined {
                        self.animator_play(cx, ids!(down.on));
                    }
                }
                Hit::FingerMove(e) => cx.widget_action(uid, &scope.path, ViewAction::FingerMove(e)),
                Hit::FingerLongPress(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerLongPress(e))
                }
                Hit::FingerUp(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerUp(e));
                    if self.animator.is_defined {
                        self.animator_play(cx, ids!(down.off));
                    }
                }
                Hit::FingerHoverIn(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerHoverIn(e));
                    if let Some(cursor) = &self.cursor {
                        cx.set_cursor(*cursor);
                    }
                    if self.animator.is_defined {
                        self.animator_play(cx, ids!(hover.on));
                    }
                }
                Hit::FingerHoverOut(e) => {
                    cx.widget_action(uid, &scope.path, ViewAction::FingerHoverOut(e));
                    if self.animator.is_defined {
                        self.animator_play(cx, ids!(hover.off));
                    }
                }
                Hit::KeyDown(e) => cx.widget_action(uid, &scope.path, ViewAction::KeyDown(e)),
                Hit::KeyUp(e) => cx.widget_action(uid, &scope.path, ViewAction::KeyUp(e)),
                _ => (),
            }
        }

        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.handle_scroll_event(cx, event, scope, &mut Vec::new());
        }
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
                            cx.set_pass_area(&texture_cache.pass, self.area);
                            //}
                        }
                        return DrawStep::done();
                    }
                    // lets start a pass
                    if self.texture_cache.is_none() {
                        self.texture_cache = Some(ViewTextureCache {
                            pass: DrawPass::new(cx),
                            _depth_texture: Texture::new(cx),
                            color_texture: Texture::new(cx),
                        });
                        let texture_cache = self.texture_cache.as_mut().unwrap();
                        //cache.pass.set_depth_texture(cx, &cache.depth_texture, PassClearDepth::ClearWith(1.0));
                        texture_cache.color_texture = Texture::new_with_format(
                            cx,
                            TextureFormat::RenderBGRAu8 {
                                size: TextureSize::Auto,
                                initial: true,
                            },
                        );
                        texture_cache.pass.set_color_texture(
                            cx,
                            &texture_cache.color_texture,
                            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
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
            if step < self.children.len() {
                //let id = self.draw_order[step];
                if let Some((id, child)) = self.children.get_mut(step) {
                    if child.visible() {
                        let walk = child.walk(cx);
                        if resume {
                            scope.with_id(*id, |scope| child.draw_walk(cx, scope, walk))?;
                        } else if let Some(fw) = cx.defer_walk_turtle(walk) {
                            self.defer_walks.push((*id, fw));
                        } else {
                            self.draw_state.set(DrawState::Drawing(step, true));
                            scope.with_id(*id, |scope| child.draw_walk(cx, scope, walk))?;
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
                if let Some((id, child)) = self.children.iter_mut().find(|(id2, _)| id2 == id) {
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
                        cx.set_pass_area(&texture_cache.pass, area);
                        //}
                    }
                }
                self.draw_state.end();
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
    pub fn swap_child(&mut self, pos_a: usize, pos_b: usize) {
        self.children.swap(pos_a, pos_b);
    }

    pub fn child_index(&mut self, comp: &WidgetRef) -> Option<usize> {
        if let Some(pos) = self.children.iter().position(|(_, w)| w == comp) {
            Some(pos)
        } else {
            None
        }
    }

    pub fn child_at_index(&mut self, index: usize) -> Option<&WidgetRef> {
        if let Some(f) = self.children.get(index) {
            Some(&f.1)
        } else {
            None
        }
    }

    pub fn set_scroll_pos(&mut self, cx: &mut Cx, v: Vec2d) {
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
        let view_size = self.view_size.unwrap_or(Vec2d::default());
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
            metrics: Metrics::default(),
        }
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    pub fn debug_print_children(&self) {
        // Debug output removed
    }

    pub fn debug_props(&self) -> String {
        format!(
            "walk=({:?},{:?}) flow={:?} show_bg={} visible={} padding=({},{},{},{}) spacing={:?}",
            self.walk.width,
            self.walk.height,
            self.layout.flow,
            self.show_bg,
            self.visible,
            self.layout.padding.left,
            self.layout.padding.top,
            self.layout.padding.right,
            self.layout.padding.bottom,
            self.layout.spacing
        )
    }

    pub fn debug_dump_tree(&self, depth: usize) -> String {
        let mut out = String::new();
        let indent = "  ".repeat(depth);
        out.push_str(&format!("{}View {{ {} }}\n", indent, self.debug_props()));
        for (id, child) in &self.children {
            out.push_str(&format!("{}  [id={:?}] ", indent, id));
            if let Some(view) = child.borrow_mut::<View>() {
                out.push_str(&format!("\n{}", view.debug_dump_tree(depth + 2)));
            } else {
                let text = child.text();
                if text.is_empty() {
                    out.push_str("<widget>\n");
                } else {
                    out.push_str(&format!("<widget text={:?}>\n", text));
                }
            }
        }
        out
    }

    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }
}
