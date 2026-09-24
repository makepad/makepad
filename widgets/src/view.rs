use {
    crate::makepad_draw::event::{DigitId, FingerLongPressEvent},
    crate::{
        animator::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        makepad_script::{ScriptFnRef, ScriptIp},
        scroll_bars::{ScrollBars, ScrollExtent},
        widget::*,
        widget_async::{
            CxWidgetToScriptCallExt, ScriptAsyncCalls, ScriptAsyncId, ScriptAsyncResult,
        },
        widget_tree::CxWidgetExt,
    },
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.EventOrder = #(EventOrder::script_api(vm))
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
    #[uid]
    uid: WidgetUid,
    #[source]
    pub source: ScriptObjectRef,
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

    #[imperative]
    #[live(true)]
    #[apply_state]
    pub visible: bool,
    #[live(false)]
    skip_widget_tree_search: bool,

    #[live(true)]
    grab_key_focus: bool,
    #[live(false)]
    block_signal_event: bool,
    #[live]
    pub cursor: Option<MouseCursor>,
    #[live(false)]
    capture_overload: bool,
    #[live]
    scroll_bars: ScriptObjectRef,

    #[live(false)]
    design_mode: bool,

    #[live(false)]
    pub debug_stream: bool,

    #[live]
    on_render: ScriptFnRef,
    /// `|index|` of the direct child a tap landed on. Lives on the container
    /// so `on_render` rows stay closure-free and drag scrolling keeps working.
    #[live]
    on_item_tap: ScriptFnRef,

    #[rust]
    script_async: ScriptAsyncCalls,
    #[rust]
    applying_style_render: bool,
    #[rust]
    item_tap_live: bool,

    #[rust]
    scroll_bars_obj: Option<Box<ScrollBars>>,
    /// The digit of a press this View stood down from, cleared by its release.
    ///
    /// `capture_overload` hands a View the FingerDown for a press a child
    /// control already captured. While that control holds the pointer the
    /// press is not the View's to take, so it takes no key focus, reports no
    /// `ViewAction::FingerDown` and plays no `down` state — and it must not
    /// report the matching FingerUp either: an up for a press it never took is
    /// the same press-like state, arriving one event later.
    #[rust]
    stood_down_press: Option<DigitId>,
    #[rust]
    view_size: Option<Vec2d>,
    // Forces the next Texture-mode draw to re-render its offscreen pass instead of cache-hitting.
    // Set on a repopulate or optimize-mode flip, since neither moves the rect that will_redraw sees.
    #[rust]
    force_texture_redraw: bool,
    // Caps the offscreen texture's height (in Texture mode only) so a huge Fit-height view never
    // allocates a render target past the GPU's max texture size. Content past the cap is clipped.
    #[rust]
    texture_max_height: Option<f64>,

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
    /// Where the children were last declared: the construction site of the
    /// value whose vec last held them. A reload whose value comes from the
    /// same site with an empty vec has deleted the last child; a reload from
    /// another site (a restyle re-applying the widget's own template over a
    /// body evaluated elsewhere) says nothing about them.
    #[rust]
    children_made_at: Option<ScriptIp>,

    #[apply_default]
    animator: Animator,
}

struct ViewTextureCache {
    pass: DrawPass,
    _depth_texture: Texture,
    color_texture: Texture,
}

/// Frozen cached framebuffer, including the pass and attachments that own it.
/// Retain this until the compositor has finished presenting the old frame.
pub struct ViewTextureSnapshot { cache: ViewTextureCache }
impl ViewTextureSnapshot {
    pub fn texture(&self) -> &Texture { &self.cache.color_texture }
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
                let mut declared = 0usize;
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
                            if !WidgetRef::value_is_newable_widget(vm, kv.value) {
                                continue;
                            }
                            declared += 1;

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
                if declared > 0 {
                    let made_at = vm.bx.heap.object_data(obj).made_at;
                    self.children_made_at = (!made_at.is_unknown()).then_some(made_at);
                }
            }
        }

        if apply.is_reload() {
            // Update or delete the children. An empty vec is the last child
            // deleted only when a live edit re-applies the declaration that
            // held the children; any other empty vec (a restyle re-applying
            // the widget's own template, an `on_render` view's declaration,
            // which is empty by design, a streaming parse still short of its
            // children) leaves them alone.
            let same_declaration = apply.is_live_edit_reload()
                && self.children_made_at.is_some()
                && value
                    .as_object()
                    .map(|obj| Some(vm.bx.heap.object_data(obj).made_at) == self.children_made_at)
                    .unwrap_or(false);
            if !self.live_update_order.is_empty() || self.children.is_empty() || same_declaration {
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
            if let Some(bars) = self.scroll_bars_obj.as_mut() {
                if apply.is_reload() {
                    bars.script_apply(vm, apply, scope, self.scroll_bars.as_object().into());
                }
            } else {
                self.scroll_bars_obj = Some(Box::new(ScrollBars::script_from_value(
                    vm,
                    self.scroll_bars.as_object().into(),
                )));
            }
        }

        vm.cx_mut().widget_tree_mark_dirty(self.uid);
        // Dynamic children emitted by on_render have no declaration in this
        // source vec. Re-render them against the new style too, preserving edits.
        if matches!(apply,Apply::ScriptReapply) && !self.applying_style_render && self.on_render.as_object()!=ScriptObject::ZERO {
            let _=self.script_call(vm,id!(render_style),NIL);
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
    /// Build the object `on_render` appends its children to. It is re-applied to this
    /// View with `Apply::Reload`, so whatever isn't reachable on its prototype chain
    /// reverts to the base widget's defaults.
    ///
    /// It therefore protos off the *instance* source — the declaration site — not the
    /// instance's template. Protoing off `source.proto` skipped the declaration level
    /// and silently dropped every property set there: `results := ScrollYView{height: 610}`
    /// re-applied as the base `height: Fill`, which a `Fit` parent defers to zero, so
    /// the view (and every rendered row in it) vanished after the first `render()`.
    ///
    /// `script_result` restores `self.source` afterwards, so the chain stays
    /// `me -> declaration` instead of growing a link per render. `no_vec` leaves `me`'s
    /// own vec empty so `on_render` fully owns the child list.
    fn make_render_me(&self, vm: &mut ScriptVm) -> ScriptValue {
        if self.source.is_zero() {
            return NIL;
        }

        vm.bx
            .heap
            .new_with_proto_no_vec(self.source.as_object().into())
            .into()
    }

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
    /// Updates this view's typed walk without evaluating script.
    pub fn set_walk(&self, cx: &mut Cx, walk: Walk) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.walk = walk;
            inner.redraw(cx);
        }
    }

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

    /// Forces the next Texture-mode draw to re-render its offscreen texture. Call after
    /// repopulating a texture-cached view whose content changed.
    pub fn redraw_texture_cache(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.redraw_texture_cache();
        }
    }

    /// Caps the offscreen texture's height in Texture mode (`None` = uncapped). See the `View`
    /// method for details.
    /// Caps the offscreen texture's height when this view is in Texture mode. `None` (the default)
    /// leaves it uncapped. Only useful for a Fit-height cached view whose content can be taller than
    /// the GPU's max texture size; content past the cap is clipped.
    pub fn set_texture_max_height(&self, max: Option<f64>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_texture_max_height(max);
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

    pub fn animator_play_with(&self, cx: &mut Cx, state: &[LiveId; 2], play: Play) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play_with(cx, state, play);
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

    /// See [`View::scroll_extent`]. `None` when empty, as for a view
    /// without scroll bars.
    pub fn scroll_extent(&self) -> Option<ScrollExtent> {
        self.borrow().and_then(|inner| inner.scroll_extent())
    }

    pub fn area(&self) -> Area {
        if let Some(inner) = self.borrow_mut() {
            inner.area
        } else {
            Area::Empty
        }
    }

    pub fn cached_texture_id(&self) -> Option<TextureId> {
        self.borrow().and_then(|inner| {
            inner
                .texture_cache
                .as_ref()
                .map(|cache| cache.color_texture.texture_id())
        })
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

#[cfg(test)]
mod contextual_size_tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;

    #[test]
    fn cached_view_re_resolves_contextual_width_after_parent_resize() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut view = cx.with_vm(|vm| {
            crate::script_mod(vm);
            View::script_new_with_default(vm)
        });
        view.view_size = Some(dvec2(17.0, 23.0));
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx = Cx2d::new(&mut draw);
        let declaration = Walk {
            width: Size::Rel {
                base: Base::Parent,
                factor: 0.5,
            },
            height: Size::fit(),
            max_width: Some(FitBound::Abs(300.0)),
            ..Default::default()
        };

        cx.begin_root_turtle(dvec2(200.0, 100.0), Layout::default());
        let first = view.walk_from_previous_size(&cx, declaration);
        assert_eq!(first.width.to_fixed(), Some(100.0));
        assert_eq!(first.height.to_fixed(), Some(23.0));
        assert_eq!(first.max_width, declaration.max_width);
        cx.end_turtle();

        cx.begin_root_turtle(dvec2(400.0, 100.0), Layout::default());
        let resized = view.walk_from_previous_size(&cx, declaration);
        assert_eq!(resized.width.to_fixed(), Some(200.0));
        assert_eq!(resized.height.to_fixed(), Some(23.0));
        cx.end_turtle();
    }

    #[test]
    fn texture_snapshot_detaches_only_a_completed_cache() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut view = cx.with_vm(|vm| {
            crate::script_mod(vm);
            View::script_new_with_default(vm)
        });
        let pass = DrawPass::new(&mut cx);
        let pass_id = pass.draw_pass_id();
        let texture = Texture::new(&mut cx);
        let texture_id = texture.texture_id();
        view.texture_cache = Some(ViewTextureCache {
            pass,
            _depth_texture: Texture::new(&mut cx),
            color_texture: texture,
        });

        cx.passes[pass_id].paint_dirty = true;
        assert!(view.take_texture_snapshot(&mut cx).is_none());
        assert!(view.texture_cache.is_some());

        cx.passes[pass_id].paint_dirty = false;
        cx.passes[pass_id].live_with_parent = true;
        let snapshot = view.take_texture_snapshot(&mut cx).unwrap();
        assert_eq!(snapshot.texture().texture_id(), texture_id);
        assert!(view.texture_cache.is_none());
        assert!(view.force_texture_redraw);
        assert!(cx.passes[pass_id].main_draw_list_id.is_none());
        assert!(matches!(
            cx.passes[pass_id].parent,
            CxDrawPassParent::None
        ));
        assert!(!cx.passes[pass_id].live_with_parent);
    }
}

#[cfg(test)]
mod pointer_capture_tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const PANE: DVec2 = dvec2(300.0, 120.0);

    /// One draw of a widget over the whole pane. Each widget gets its own draw
    /// list, so two of them can overlap — a control over a view, the way a
    /// child sits inside its container — without either draw invalidating the
    /// other's area.
    fn frame(cx: &mut Cx, widget: &mut dyn Widget, pass: &DrawPass, draw_list: &mut DrawList2d) {
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(PANE, Layout::flow_down());
        let _ = widget.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(PANE.x, PANE.y));
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }

    /// What one press and its release got out of the View.
    struct Pressed {
        /// Where the key focus ended up.
        focus: Area,
        button: Area,
        view: Area,
        /// Whether the View reported the press out to the app.
        reported_down: bool,
        /// And whether it reported the release.
        reported_up: bool,
    }

    /// The View's own action out of the batch a just-handled event left
    /// pending, before `handle_actions` drains it.
    fn reported(cx: &Cx, uid: WidgetUid) -> Option<ViewAction> {
        cx.new_actions
            .find_widget_action(uid)
            .map(|action| action.cast::<ViewAction>())
    }

    fn mouse_down(at: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs: at,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        })
    }

    fn mouse_up(at: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs: at,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 2.0,
        })
    }

    /// A press in the middle of a `capture_overload` View with a Button drawn
    /// over it, and the release that follows. With `control`, the button is
    /// handed the press first — the way a container's children are handled
    /// before its own hits — and captures the mouse; the View then sees the
    /// same press through the overload and has to decide how much of it is
    /// its to take.
    ///
    /// A Button stands in for any continuously dragged control: what the rule
    /// asks is who holds the mouse, not what kind of control it is.
    fn press(control: bool) -> Pressed {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut view = cx.with_vm(View::script_new_with_default);
        let mut button = cx.with_vm(crate::button::Button::script_new_with_default);
        // The two things that make a View hit-test its own area at all.
        view.capture_overload = true;
        view.cursor = Some(MouseCursor::Default);
        let uid = view.widget_uid();

        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, PANE);
        let mut view_list = DrawList2d::new(&mut cx);
        let mut button_list = DrawList2d::new(&mut cx);
        frame(&mut cx, &mut view, &pass, &mut view_list);
        frame(&mut cx, &mut button, &pass, &mut button_list);

        let at = PANE * 0.5;
        assert!(
            view.area().is_valid(&cx) && view.area().clipped_rect(&cx).contains(at),
            "the press point is not over the view"
        );
        assert!(
            button.area().is_valid(&cx) && button.area().clipped_rect(&cx).contains(at),
            "the press point is not over the control"
        );

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        let event = mouse_down(at);
        if control {
            button.handle_event(&mut cx, &event, &mut Scope::empty());
            assert!(
                cx.fingers.any_areas_captured(),
                "the control did not take the press, so this test proves nothing"
            );
        }
        view.handle_event(&mut cx, &event, &mut Scope::empty());
        let reported_down = matches!(reported(&cx, uid), Some(ViewAction::FingerDown(_)));
        // `set_key_focus` only records the request; the focus moves when the
        // cycle runs, which it does after the press's actions are dispatched.
        cx.handle_actions();
        let focus = cx.key_focus();

        // The same pointer let go. The control that held it still holds it —
        // nothing releases a digit in a test — so the release is the tail of
        // exactly the press above.
        let event = mouse_up(at);
        if control {
            button.handle_event(&mut cx, &event, &mut Scope::empty());
        }
        view.handle_event(&mut cx, &event, &mut Scope::empty());
        let reported_up = matches!(reported(&cx, uid), Some(ViewAction::FingerUp(_)));
        cx.fingers.first_mouse_button = None;
        cx.handle_actions();

        Pressed {
            focus,
            button: button.area(),
            view: view.area(),
            reported_down,
            reported_up,
        }
    }

    /// The rule, for the one press a View can be handed that is not its own: a
    /// control holding the mouse keeps the keyboard it took with that press,
    /// and the container co-capturing the same press does not pull the focus
    /// out from under it.
    #[test]
    fn a_control_holding_the_mouse_keeps_the_key_focus_it_took() {
        let pressed = press(true);
        assert_eq!(
            pressed.focus, pressed.button,
            "the container took the control's key focus"
        );
        assert_ne!(
            pressed.focus, pressed.view,
            "the container took the control's key focus"
        );
    }

    /// And the View is not broken while fixing it: a press nothing else holds
    /// is its own, and still moves the keyboard to it.
    #[test]
    fn a_press_nothing_else_holds_still_focuses_the_view() {
        let pressed = press(false);
        assert_eq!(
            pressed.focus, pressed.view,
            "the view stopped grabbing key focus from its own press"
        );
    }

    /// The focus is only one of the press-like states the rule names. A View
    /// that stood down from the press must not report it either: a
    /// `ViewAction::FingerDown` out to the app is the container claiming a
    /// press the control holding the pointer already owns.
    #[test]
    fn a_control_holding_the_mouse_keeps_the_press_from_being_reported() {
        assert!(
            !press(true).reported_down,
            "the container reported a press a control was holding"
        );
        assert!(
            press(false).reported_down,
            "the view stopped reporting its own press"
        );
    }

    /// And the release that ends it. A View that stood down at the press has
    /// no press to end, so an up for it would hand the app half a click out of
    /// a gesture that was never the View's.
    #[test]
    fn a_press_a_view_stood_down_from_reports_no_release() {
        assert!(
            !press(true).reported_up,
            "the container reported the release of a press it never took"
        );
        assert!(
            press(false).reported_up,
            "the view stopped reporting the release of its own press"
        );
    }

    /// A View that scrolls, with content four panes tall so its vertical bar
    /// has somewhere to run. Answers the View and the pass state that has to
    /// outlive it.
    fn scrolling_view(cx: &mut Cx) -> (View, DrawPass, DrawList2d) {
        let mut view = cx.with_vm(View::script_new_with_default);
        view.capture_overload = true;
        view.cursor = Some(MouseCursor::Default);
        view.scroll_bars_obj = Some(Box::new(cx.with_vm(ScrollBars::script_new_with_default)));

        let mut tall = cx.with_vm(View::script_new_with_default);
        tall.walk = Walk::fixed(PANE.x, PANE.y * 4.0);
        view.children
            .push((live_id!(tall), WidgetRef::new_with_inner(Box::new(tall))));

        let pass = DrawPass::new(cx);
        pass.set_size(cx, PANE);
        let mut draw_list = DrawList2d::new(cx);
        // Twice: the first draw is what tells the bars how tall the content is.
        for _ in 0..2 {
            frame(cx, &mut view, &pass, &mut draw_list);
        }
        (view, pass, draw_list)
    }

    /// The areas a View owns are its own and its bars': the set it hands
    /// `is_mouse_held_outside`, which is what keeps it from reading its own
    /// scroll bar as an outsider.
    #[test]
    fn a_views_own_areas_include_its_scroll_bars() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let (view, _pass, _list) = scrolling_view(&mut cx);
        let bars = view
            .scroll_bars_obj
            .as_ref()
            .expect("the view lost its scroll bars")
            .bar_areas();
        let mine = view.pointer_areas();
        assert_eq!(
            mine.as_slice(),
            &[view.area(), bars[0], bars[1]],
            "a view asked about the pointer with something other than everything it owns"
        );
    }

    /// FIX 2, the one a View can hit on its own: a press on its OWN scroll bar
    /// is a press on its own furniture. The bar captures the mouse like any
    /// dragged control, and a View that named only its content area would read
    /// that as an outsider and stand down against itself — dropping the focus
    /// and the press report for a press that was its all along.
    #[test]
    fn a_view_does_not_stand_down_against_its_own_scroll_bar() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let (mut view, _pass, _list) = scrolling_view(&mut cx);
        let uid = view.widget_uid();

        let bar = view.pointer_areas()[2];
        assert!(
            bar.is_valid(&cx),
            "the view drew no vertical scroll bar, so this test proves nothing"
        );
        let rect = bar.clipped_rect(&cx);
        let at = rect.pos + rect.size * 0.5;

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WindowId(1, 1)));
        let event = mouse_down(at);
        view.handle_event(&mut cx, &event, &mut Scope::empty());
        assert!(
            cx.fingers.is_area_captured(bar),
            "the scroll bar did not take the press, so this test proves nothing"
        );
        let reported = matches!(reported(&cx, uid), Some(ViewAction::FingerDown(_)));
        cx.handle_actions();
        let focus = cx.key_focus();
        cx.fingers.first_mouse_button = None;

        assert!(
            reported,
            "the view stood down from a press on its own scroll bar"
        );
        assert_eq!(
            focus,
            view.area(),
            "the view refused the key focus of a press on its own scroll bar"
        );
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

    pub fn animator_play_with(&self, cx: &mut Cx, state: &[LiveId; 2], play: Play) {
        for item in self.iter() {
            item.animator_play_with(cx, state, play);
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
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn set_scroll_pos(&mut self, cx: &mut Cx, v: Vec2d) {
        self.set_scroll_pos(cx, v);
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        for (_, child) in &mut self.children {
            child.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.children {
            visit(*id, child.clone());
        }
    }

    fn cancel_children_impl(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        if !self.visible {
            return false;
        }
        for (id, child) in &self.children {
            if let EventOrder::List(order) = &self.event_order {
                if !order.contains(id) {
                    continue;
                }
            }
            visit(*id, child.clone());
        }
        true
    }

    fn skip_widget_tree_search(&self) -> bool {
        self.skip_widget_tree_search
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for (_, child) in &self.children {
            child.find_widgets_from_point(cx, point, found);
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
            // A view that has never drawn has an empty area, so its own
            // redraw is a no-op and the visibility change only lands on
            // the next unrelated full repaint (the classic "appears after
            // hot reload" bug). Escalate to a full redraw in that case.
            if visible && matches!(self.area, Area::Empty) {
                cx.redraw_all();
            } else {
                self.redraw(cx);
            }
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }
}

impl Widget for View {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(render) || method == live_id!(render_style) {
            // `me` protos off `self.source`, and the caller's `args` object
            // travels into the VM that owns `on_render` — both are heap values,
            // and a heap value means nothing outside the heap that minted it.
            // Refuse when this view was minted somewhere else: rendering it
            // would build `me` here holding another heap's object index (or
            // plant these args over there), and nothing would notice until that
            // heap's next GC walked the value and indexed out of bounds, in
            // code that did nothing wrong. Includes the case that actually
            // bites — a view whose isolate has since been torn down.
            if !self.source.is_zero() && self.source.heap_key() != vm.bx.heap.heap_key() {
                return ScriptAsyncResult::MethodNotFound;
            }
            let me = self.make_render_me(vm);
            return vm.with_cx_mut(|cx| {
                cx.widget_to_script_async_call_fwd(
                    self.uid,
                    &mut self.script_async,
                    me,
                    self.source.clone(),
                    self.on_render.clone(),
                    args,
                    method,
                )
            });
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        let Some(call) = self.script_async.take(id) else {
            return;
        };

        if call.method() == id!(render) || call.method()==id!(render_style) {
            if result.is_err() {
                // An error mid-closure abandons every child emitted before it
                // and used to do so with ZERO diagnostics — the "on_render
                // silently renders nothing" family cost days to bisect. Say
                // what happened.
                let msg = vm.bx.heap.temp_string_with(|heap, out| {
                    heap.cast_to_string(result, out);
                    out.to_string()
                });
                error!("on_render closure failed; discarding its output: {msg}");
                return;
            }
            if let Some(me_obj) = call.me().as_object() {
                // The closure's FINAL statement becomes its return value (the
                // parser converts the last commit into a RETURN), so a widget
                // emitted last would otherwise vanish. Commit it as the last
                // child; non-widget values are skipped downstream anyway.
                if let Some(ret_obj) = result.as_object() {
                    if ret_obj != me_obj {
                        // Unchecked: `me` is this render's own throwaway
                        // object (make_render_me), never frozen.
                        vm.bx.heap.vec_push_unchecked(me_obj, NIL, result);
                    }
                }
                // `#[source]` is reassigned by any non-eval script apply, so this Reload
                // would leave `source` pointing at `me`. Keep it on the declaration object:
                // the next `make_render_me` protos off it (see there), and `me` is a
                // throwaway whose children already hold their own refs.
                let declaration = self.source.clone();
                let style=call.method()==id!(render_style);
                self.applying_style_render=style;
                self.script_apply(vm, &if style {Apply::ScriptReapply}else{Apply::Reload}, &mut Scope::empty(), me_obj.into());
                self.applying_style_render=false;
                self.source = declaration;
                self.redraw(vm.cx_mut());
            }
        }
    }

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
        if let Event::ClearHover = event {
            if self.animator.is_defined {
                self.animator_play(cx, ids!(hover.off));
                self.animator_play(cx, ids!(down.off));
            }
        }
        // A press that catches an in-progress momentum fling stops the scroll and is consumed:
        // it must not also activate a child widget under the finger, as on iOS, Android, and
        // macOS. So when the scroll bars report a caught fling, skip dispatching this press to
        // children. This runs before the child loop below, since children would otherwise
        // capture the press first.
        let mut fling_caught = false;
        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            let mut actions = Vec::new();
            scroll_bars.handle_main_event(cx, event, scope, &mut actions);
            if actions.len() > 0 {
                cx.redraw_area_and_children(self.area);
            };
            fling_caught = scroll_bars.catch_fling_on_press(cx, event);
        }

        if !fling_caught {
            match &self.event_order {
                EventOrder::Up => {
                    for (_id, child) in self.children.iter_mut().rev() {
                        child.handle_event(cx, event, scope);
                    }
                }
                EventOrder::Down => {
                    for (_id, child) in self.children.iter_mut() {
                        child.handle_event(cx, event, scope);
                    }
                }
                EventOrder::List(list) => {
                    for id in list {
                        if let Some((_, child)) =
                            self.children.iter_mut().find(|(id2, _)| id2 == id)
                        {
                            child.handle_event(cx, event, scope);
                        }
                    }
                }
            }
        }

        event.hit_tweak_ray(self.area(), self.widget_uid());

        // Also skip this View's own press-driven hit handling when a fling was caught, so the
        // catching press is fully consumed (it neither activates a child nor fires this
        // View's own FingerDown/animator). `fling_caught` is only ever true on a press event
        // over a flinging scroll view, so key/hover handling is unaffected.
        if !fling_caught && (self.visible && self.cursor.is_some() || self.animator.is_defined) {
            match event.hits_with_capture_overload(cx, self.area(), self.capture_overload) {
                Hit::FingerDown(e) => {
                    // `capture_overload` hands this View the FingerDown for a
                    // press one of its children already captured, so the press
                    // reaching here is not necessarily its own. A control that
                    // is dragged continuously owns the pointer from its press
                    // until the release, and while it does, nothing else may
                    // take a focus or a press-like state from that pointer on
                    // the way past. For a View that is all three of these: the
                    // keyboard the control took with the press (a text input's
                    // caret, a list's selection), the `ViewAction::FingerDown`
                    // it would report out to the app, and the `down` state that
                    // makes it look pressed. It stands down from the whole
                    // press, not only from the focus.
                    //
                    // Without `capture_overload` nothing else can hold this
                    // press, so the question only ever bites there. Its own
                    // scroll bars are `mine` (see `pointer_areas`), so a press
                    // on a bar of its own is still its own press. Mouse only,
                    // deliberately: `is_mouse_held_outside` ignores touch
                    // captures.
                    if cx.fingers.is_mouse_held_outside(&self.pointer_areas()) {
                        // Remembered, because the release has to stand down
                        // too: reporting an up for a press this View never took
                        // would hand the app half a click out of a gesture that
                        // was never its own.
                        self.stood_down_press = Some(e.digit_id);
                    } else {
                        if self.stood_down_press == Some(e.digit_id) {
                            self.stood_down_press = None;
                        }
                        if self.grab_key_focus {
                            cx.set_key_focus(self.area());
                        }
                        cx.widget_action(uid, ViewAction::FingerDown(e));
                        if self.animator.is_defined {
                            self.animator_play(cx, ids!(down.on));
                        }
                    }
                }
                Hit::FingerMove(e) => {
                    // The moves of a press this View stood down from are the
                    // holding control's drag, not this View's gesture.
                    if self.stood_down_press != Some(e.digit_id) {
                        cx.widget_action(uid, ViewAction::FingerMove(e));
                    }
                }
                Hit::FingerLongPress(e) => {
                    if self.stood_down_press != Some(e.digit_id) {
                        cx.widget_action(uid, ViewAction::FingerLongPress(e));
                    }
                }
                Hit::FingerUp(e) => {
                    if self.stood_down_press == Some(e.digit_id) {
                        // The press ends as it began, unreported. `down.off` is
                        // not played either: it would be the tail of an
                        // animation that never started.
                        self.stood_down_press = None;
                    } else {
                        cx.widget_action(uid, ViewAction::FingerUp(e));
                        if self.animator.is_defined {
                            self.animator_play(cx, ids!(down.off));
                        }
                    }
                }
                Hit::FingerHoverIn(e) => {
                    cx.widget_action(uid, ViewAction::FingerHoverIn(e));
                    if let Some(cursor) = &self.cursor {
                        cx.set_cursor(*cursor);
                    }
                    if self.animator.is_defined {
                        self.animator_play(cx, ids!(hover.on));
                    }
                }
                Hit::FingerHoverOut(e) => {
                    cx.widget_action(uid, ViewAction::FingerHoverOut(e));
                    if self.animator.is_defined {
                        self.animator_play(cx, ids!(hover.off));
                    }
                }
                Hit::KeyDown(e) => cx.widget_action(uid, ViewAction::KeyDown(e)),
                Hit::KeyUp(e) => cx.widget_action(uid, ViewAction::KeyUp(e)),
                _ => (),
            }
        }

        if let Some(scroll_bars) = &mut self.scroll_bars_obj {
            scroll_bars.handle_scroll_event(cx, event, scope, &mut Vec::new());
        }

        // After the scroll bars so their drag capture comes first; the overload
        // lets this view capture the same press alongside it.
        if fling_caught {
            self.item_tap_live = false;
        } else if self.visible && self.on_item_tap.as_object() != ScriptObject::ZERO {
            match event.hits_with_capture_overload(cx, self.area(), true) {
                Hit::FingerDown(e) => {
                    // A child that already owns this press (a Button) gets the tap.
                    self.item_tap_live = !cx.fingers.is_digit_captured_elsewhere(e.digit_id, self.area());
                }
                Hit::FingerUp(e) if e.was_tap() && self.item_tap_live => {
                    self.item_tap_live = false;
                    // clipped_rect: scrolled and clipped, like the hit test itself.
                    let index = self.children.iter()
                        .position(|(_, child)| child.area().clipped_rect(cx).contains(e.abs));
                    if let Some(index) = index {
                        cx.widget_to_script_call(
                            uid,
                            NIL,
                            self.source.clone(),
                            self.on_item_tap.clone(),
                            &[(index as f64).into()],
                        );
                    }
                }
                _ => (),
            }
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
                    let walk = self.walk_from_previous_size(cx, walk);
                    // A repopulate or optimize-mode flip forces one real re-render: the size-based
                    // cache check can't see content changes on a recycled/toggled view.
                    let force = std::mem::take(&mut self.force_texture_redraw);
                    // Re-render whenever the on-screen rect changes (position or size) or a
                    // repopulate/mode-flip forced it; only a truly unchanged view cache-hits.
                    if !force && !cx.will_redraw(self.draw_list.as_mut().unwrap(), walk) {
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
                            // The cache hit still consumes the texture: keep
                            // the pass attached, or a repaint of the cached
                            // subtree (an animated child, `repaint`) would find
                            // it orphaned and never re-render.
                            cx.make_child_pass(&texture_cache.pass);
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
                    let walk = self.walk_from_previous_size(cx, walk);
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

            // Only Texture mode allocates an offscreen render target sized to this turtle, so cap
            // its height there. None/DrawList render into the parent turtle and must stay uncapped.
            let walk = if self.optimize.is_texture() {
                self.walk_with_texture_max_height(walk)
            } else {
                walk
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
                            child.draw_walk(cx, scope, walk)?;
                        } else if let Some(fw) = cx.defer_walk_turtle(walk) {
                            self.defer_walks.push((*id, fw));
                        } else {
                            self.draw_state.set(DrawState::Drawing(step, true));
                            child.draw_walk(cx, scope, walk)?;
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
                if let Some((_id, child)) = self.children.iter_mut().find(|(id2, _)| id2 == id) {
                    let walk = dw.resolve(cx);
                    child.draw_walk(cx, scope, walk)?;
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

                // Track measured size in every mode. The None path must update it too, or a later
                // Texture draw sizes its offscreen turtle to a stale height and clips the content.
                self.view_size = Some(self.area.rect(cx).size);

                if self.optimize.needs_draw_list() {
                    let rect = self.area.rect(cx);
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

trait EventTweakRayExt {
    fn hit_tweak_ray(&self, area: Area, widget_uid: WidgetUid);
}

impl EventTweakRayExt for Event {
    fn hit_tweak_ray(&self, area: Area, widget_uid: WidgetUid) {
        if let Event::TweakRay(e) = self {
            if !area.is_empty() {
                e.hit_widget_uids.borrow_mut().push(widget_uid.0);
            }
        }
    }
}

#[derive(Clone)]
enum DrawState {
    Drawing(usize, bool),
    DeferWalk(usize),
}

impl View {
    /// The design-mode container seam: a view marked `design_mode: true` is
    /// transparent to the tweaker's pick resolution — its children resolve,
    /// it never consumes the hit itself.
    pub fn design_mode(&self) -> bool {
        self.design_mode
    }

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

    /// Where this view is scrolled to and how far it can go. `None` for a
    /// view without scroll bars: its `layout.scroll` is an offset someone
    /// set, not a position a reader can move, so there is no extent to report.
    pub fn scroll_extent(&self) -> Option<ScrollExtent> {
        self.scroll_bars_obj.as_ref().map(|bars| bars.extent())
    }

    pub fn area(&self) -> Area {
        self.area
    }

    /// Every area this View owns, for the pointer-capture rule: its own
    /// content area and, when it scrolls, both of its scroll bar handles.
    ///
    /// [`CxFingers::is_mouse_held_outside`] is asked with the areas the host
    /// owns, and a scroll bar of its own is one of them. The bar captures the
    /// mouse on its press like any dragged control, so a View that named only
    /// `self.area` would read its own bar as something else holding the
    /// pointer and stand down against itself — refusing the key focus and the
    /// press report for a press on its own furniture.
    ///
    /// The helper matches captures by owner, so a handle redrawn mid-drag
    /// still counts as one of these even though its `redraw_id` has moved on.
    pub fn pointer_areas(&self) -> SmallVec<[Area; 3]> {
        let mut areas = SmallVec::new();
        areas.push(self.area);
        if let Some(bars) = &self.scroll_bars_obj {
            areas.extend(bars.bar_areas());
        }
        areas
    }

    /// Switch this view's draw optimization at runtime (e.g. toggle texture caching per frame).
    /// Allocates the backing draw_list on demand when promoting to a draw-list-backed mode.
    pub fn set_optimize(&mut self, cx: &mut Cx, optimize: ViewOptimize) {
        // A mode flip (e.g. None<->Texture) is a guaranteed content/layout change, and the None
        // path never updates view_size, so force one fresh texture re-render on the change.
        if core::mem::discriminant(&self.optimize) != core::mem::discriminant(&optimize) {
            self.redraw_texture_cache();
        }
        self.optimize = optimize;
        if self.optimize.needs_draw_list() && self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
    }

    /// Forces the next Texture-mode draw to re-render its offscreen texture instead of compositing
    /// the cached one, and drops the stale measured size so the re-render sizes to the new content.
    /// Call after repopulating a texture-cached view's children.
    pub fn redraw_texture_cache(&mut self) {
        self.force_texture_redraw = true;
        self.view_size = None;
    }

    /// Draw the current texture cache into `rect` without walking the view's children again.
    ///
    /// This is useful when a compositor needs the same cached surface in another pass during the
    /// current frame. Keeping the cache pass attached ensures a pending repaint still reaches the
    /// texture before it is sampled.
    pub fn draw_cached_texture(&mut self, cx: &mut Cx2d, rect: Rect) -> bool {
        let Some(texture_cache) = &self.texture_cache else {
            return false;
        };
        self.draw_bg
            .draw_vars
            .set_texture(0, &texture_cache.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        cx.make_child_pass(&texture_cache.pass);
        true
    }

    /// Detach a completed texture cache so its framebuffer can be retained as a frozen snapshot.
    ///
    /// A dirty pass has not reached the GPU yet and cannot safely be detached. Once detached, the
    /// next draw builds a new cache while the returned snapshot keeps the old pass and attachments
    /// alive for compositing.
    pub fn take_texture_snapshot(&mut self, cx: &mut Cx) -> Option<ViewTextureSnapshot> {
        let texture_cache = self.texture_cache.take()?;
        let pass = &mut cx.passes[texture_cache.pass.draw_pass_id()];
        if pass.paint_dirty {
            self.texture_cache = Some(texture_cache);
            return None;
        }
        pass.main_draw_list_id = None;
        pass.parent = CxDrawPassParent::None;
        pass.live_with_parent = false;
        self.force_texture_redraw = true;
        Some(ViewTextureSnapshot {
            cache: texture_cache,
        })
    }

    /// Caps the offscreen texture's height when this view is in Texture mode. `None` (the default)
    /// leaves it uncapped. Only useful for a Fit-height cached view whose content can be taller than
    /// the GPU's max texture size; content past the cap is clipped.
    pub fn set_texture_max_height(&mut self, max: Option<f64>) {
        if self.texture_max_height != max {
            self.texture_max_height = max;
            self.redraw_texture_cache();
        }
    }

    fn walk_with_texture_max_height(&self, walk: Walk) -> Walk {
        match self.texture_max_height {
            Some(max) if !walk.height.is_fill() => Walk { height: Size::Fixed(max), ..walk },
            _ => walk,
        }
    }

    pub fn walk_from_previous_size(&self, cx: &Cx2d, walk: Walk) -> Walk {
        let walk = cx.resolve_walk(walk, ResolveAt::BeforeBegin);
        // Fill and Fixed sizes are already known before drawing, so keep them live —
        // a Fixed size can be fresh truth for this frame (e.g. a deferred fill the
        // parent just resolved), and pinning it to the previous frame's measurement
        // would make the cached-draw dirty check miss a pure size change. Only
        // content-driven sizes fall back to the previous measurement, since they
        // cannot be known before the children draw.
        let view_size = self.view_size.unwrap_or(Vec2d::default());
        Walk {
            width: if walk.width.is_fill() || walk.width.is_fixed() {
                walk.width
            } else {
                Size::Fixed(view_size.x)
            },
            height: if walk.height.is_fill() || walk.height.is_fixed() {
                walk.height
            } else {
                Size::Fixed(view_size.y)
            },
            metrics: Metrics::default(),
            ..walk
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
