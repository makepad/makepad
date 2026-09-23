//! Modal — the layer that stops the page: a scrim over the whole window and
//! a content view on it.
//!
//! **Nothing under the scrim hears the pointer.** The modal is drawn over
//! widgets that the event is walked through before it, so a press claimed
//! when it reaches the modal has already been taken by whatever lay under
//! it: a list beside the page chose a row through the scrim. While it is
//! open the modal holds the sweep lock, which turns every other hit test
//! away, and lifts it only around its own content and scrim, the pairing
//! every popup in this crate uses. An overlay opened inside the content
//! stays above it.
//!
//! **The keyboard goes back where it was.** A handle to an area goes stale
//! the moment its list is drawn again, and the page under a modal is drawn
//! again all the time, so the place the keyboard came from is followed to
//! the handle it has when the modal closes.
//!
//! **A modal dropped while it is open lets go.** A page rebuilt under an
//! open modal (a theme or a story switch) takes the modal with it, and the
//! lock and the scroll block it held would otherwise stop the pointer and
//! the wheel for good; the next event releases them.

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::KeyCode,
    overlay_place::orphan_sweep_locks,
    view::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ModalBase = #(Modal::register_widget(vm))

    // No `width`/`height`: a modal is an overlay and claims NO slot in its
    // parent's layout — `Modal::on_after_apply` pins the walk it reports
    // upward to `Walk::empty()`, and the overlay itself is sized by the pass
    // (see `Modal::draw_walk`). Declaring `Fill` here made the modal a
    // *deferred fill* of its parent, which took a share of the parent's
    // spare length whether or not the modal ever drew.
    mod.widgets.Modal = mod.widgets.ModalBase{
        flow: Overlay
        align: Center

        draw_bg +: {
            pixel: fn() {
                return vec4(0. 0. 0. 0.0)
            }
        }

        bg_view := View{
            width: Fill
            height: Fill
            show_bg: true
            draw_bg +: {
                color: uniform(#000000B3)
                pixel: fn() {
                    return self.color
                }
            }
        }

        content := View{
            width: Fit
            height: Fit
            flow: Down
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ModalAction {
    Dismissed,
    #[default]
    None,
}

impl ModalAction {
    /// Sent directly through a closing host's descendants, unlike the
    /// widget-tagged dismissal action observed by the application.
    pub(crate) fn is_dismissal(event: &Event) -> bool {
        matches!(event, Event::Actions(actions) if actions.iter().any(|action|
            matches!(action.downcast_ref::<Self>(), Some(Self::Dismissed))))
    }
}

#[derive(Script, Widget)]
pub struct Modal {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    #[live]
    draw_bg: DrawQuad,

    #[rust]
    is_open: bool,
    /// Held while open, so an Escape belongs to this modal rather than to whatever
    /// it was opened in front of. Kept even when `can_dismiss` is false: the modal
    /// still owns the press, it just declines to act on it.
    #[rust]
    cancel_scope: Option<CancelScope>,
    /// The content's area, carried across redraws.
    ///
    /// The keyboard is moved into this area, and a view hands out a fresh one
    /// on every redraw and migrates nothing — so without carrying it the
    /// focus would stop matching the moment the modal repainted.
    #[rust]
    content_area: Area,
    /// Set by `open`, spent by the first draw that gives the content an area.
    #[rust]
    wants_focus: bool,
    /// Whether the modal can be dismissed via an external interaction, including:
    /// clicking outside the content view, pressing Escape, or performing
    /// the back navigational gesture (e.g., on Android).
    #[live(true)]
    can_dismiss: bool,
    /// Where the keyboard was when the modal opened.
    #[rust]
    restore_focus: Area,
    /// Whether the modal holds the sweep lock, on `draw_bg`'s area.
    #[rust]
    locked: bool,
}

thread_local! {
    /// Scroll blocks whose modals were dropped while open. A widget is
    /// dropped where no `Cx` is at hand, so the blocks wait for the next
    /// event, as a dropped overlay's sweep locks do.
    static ORPHANED_BLOCKS: std::cell::RefCell<Vec<Area>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn orphan_scroll_blocks(areas: &[Area]) {
    let _ = ORPHANED_BLOCKS.try_with(|orphans| orphans.borrow_mut().extend_from_slice(areas));
}

/// Release every scroll block a dropped modal left behind. The window does
/// this as each event reaches it, before anything can scroll; a modal does
/// it too, for a tree with no window above it.
pub(crate) fn release_orphaned_scroll_blocks(cx: &mut Cx) {
    let orphans = ORPHANED_BLOCKS
        .try_with(|orphans| std::mem::take(&mut *orphans.borrow_mut()))
        .unwrap_or_default();
    for area in orphans {
        cx.unblock_scrolling_within_area(area);
    }
}

/// Whether two handles name the same drawn thing, however many redraws
/// apart: the identity the platform gives the owner of a lock or a block,
/// which leaves the `redraw_id` out.
fn same_slot(a: Area, b: Area) -> bool {
    match (a, b) {
        (Area::Instance(a), Area::Instance(b)) => {
            a.draw_list_id == b.draw_list_id
                && a.draw_item_id == b.draw_item_id
                && a.instance_offset == b.instance_offset
        }
        (Area::Rect(a), Area::Rect(b)) => a.draw_list_id == b.draw_list_id && a.rect_id == b.rect_id,
        (Area::Empty, Area::Empty) => true,
        _ => false,
    }
}

/// The handle `area` has now, after any redraws of its list since it was
/// taken, or `Area::Empty` when that slot is no longer drawn.
///
/// Every draw hands out a fresh handle, and the platform moves only the
/// handles it holds itself (the key focus, captures, locks) onto it. One a
/// widget keeps goes stale at the next redraw of its list, and the keyboard
/// given to it reaches nothing: a drawer closed over a page that had been
/// drawn again left no widget with the focus, so the button that opened it
/// no longer answered Return. The slot is followed the way the platform
/// follows a lock's owner.
pub(crate) fn area_after_redraws(cx: &Cx, area: Area) -> Area {
    match area {
        Area::Empty => Area::Empty,
        Area::Instance(inst) => {
            let Some(list) = cx.draw_lists.checked_index(inst.draw_list_id) else {
                return Area::Empty;
            };
            if cx.draw_lists.is_id_freed(inst.draw_list_id) {
                return Area::Empty;
            }
            if list.redraw_id == inst.redraw_id {
                return area;
            }
            if inst.draw_item_id >= list.draw_items.len() {
                return Area::Empty;
            }
            let item = &list.draw_items[inst.draw_item_id];
            let Some(call) = item.kind.draw_call() else {
                return Area::Empty;
            };
            let end = inst.instance_offset + inst.instance_count.max(1) * call.total_instance_slots;
            if item.instances.as_ref().map_or(true, |buffer| buffer.len() < end) {
                return Area::Empty;
            }
            Area::Instance(InstanceArea { redraw_id: list.redraw_id, ..inst })
        }
        Area::Rect(ra) => {
            let Some(list) = cx.draw_lists.checked_index(ra.draw_list_id) else {
                return Area::Empty;
            };
            if cx.draw_lists.is_id_freed(ra.draw_list_id) || ra.rect_id >= list.rect_areas.len() {
                return Area::Empty;
            }
            Area::Rect(RectArea { redraw_id: list.redraw_id, ..ra })
        }
    }
}

impl ScriptHook for Modal {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // A modal occupies NO space in the layout that holds it. `draw_walk`
        // opens its own overlay draw list on a root turtle for the pass and
        // never walks the parent's turtle, so the walk this widget reports
        // upward must claim nothing.
        //
        // It used to report `Fill`/`Fill` — the size of the overlay it paints
        // inside its own pass, which is not a request the parent can honour.
        // A `Fill` child of a `flow: Down` parent is a *deferred fill*: the
        // parent hands it an equal share of the column's spare height at
        // resolve time, whether or not the child then draws a single pixel.
        // Three closed modals parked beside a `height: Fill` sibling split
        // that column four ways — on the VJ DJ page 856pt of spare height
        // became 214pt each, and the content under the fill was laid out
        // with a negative height and never drawn at all.
        //
        // Forced here rather than only left out of the DSL, so that an
        // instance writing `Modal{height: Fill}` cannot bring the bug back.
        self.view.walk = Walk::empty();
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl Widget for Modal {
    fn visit_cancel(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        self.is_open && self.cancel_children_impl(visit)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        release_orphaned_scroll_blocks(cx);
        if !self.is_open {
            return;
        }
        if ModalAction::is_dismissal(event) {
            self.close(cx);
            return;
        }
        // The content's widgets and the scrim use plain hits, which this
        // modal's own lock turns away: lifted around them.
        let lifted = self.lift_lock(cx);

        // Forward the event to the inner `content` view.
        let content = self.view.widget(cx, ids!(content));
        content.handle_event(cx, event, scope);

        // Proactively consume any hit that occurred in the bg area, which prevents the hit
        // from being handled by any views underneath this modal.
        let bg_area = self.draw_bg.area();
        let bg_area_hit = event.hits(cx, bg_area);

        let owns_cancel = self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s));
        // This needs to be done here such that a non-dismissable modal (`can_dismiss` = false)
        // will still block back-navigation handling for widgets that are behind it.
        let back_pressed = owns_cancel && event.back_pressed();

        if self.can_dismiss {
            // Close the modal if any of the following conditions occur:
            // * If this modal owns the back navigational action/gesture (e.g., on Android),
            // * If an `Escape` press this modal owns was released. Ownership, not key
            //   focus, is what keeps a widget behind the modal from acting on the press.
            // * If this modal owns a click of the mouse's back button, the desktop
            //   equivalent of that gesture.
            // * If there was a click/tap in the background area, outside of the inner `content` view.
            let should_close = back_pressed
                || match bg_area_hit {
                    // A press taken away dismisses nothing.
                    Hit::FingerUp(fe) => !fe.cancelled && !content.area().rect(cx).contains(fe.abs),
                    _ => false,
                }
                || (owns_cancel && (
                    matches!(event, Event::KeyUp(key) if key.key_code == KeyCode::Escape)
                    || matches!(event, Event::MouseUp(e) if e.button.is_back())
                ));
            if should_close {
                // Tagged with the MODAL's uid: `ModalRef::dismissed` looks the
                // action up by `self.widget_uid()`, so the content view's uid
                // never matched and every caller's dismiss branch was dead.
                let uid = self.widget_uid();
                cx.widget_action(uid, ModalAction::Dismissed);
                self.close(cx);
            }
        }
        self.restore_lock(cx, lifted);
    }

    /// The incoming `walk` is deliberately ignored: a modal is not laid out by
    /// its parent at all. It paints over the whole pass, on a root turtle
    /// sized by the pass, so its geometry comes from `Walk::fill()` against
    /// that root — never from the slot a parent thought it was handing over.
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(self.view.layout);
        self.draw_bg.begin(cx, Walk::fill(), self.view.layout);

        if self.is_open {
            let bg_view = self.view.widget(cx, ids!(bg_view));
            let _ = bg_view.draw_walk(
                cx,
                scope,
                Walk::fill().with_abs_pos(Vec2d { x: 0., y: 0. }),
            );

            let content = self.view.widget(cx, ids!(content));
            let _ = content.draw_all(cx, scope);
        }

        self.draw_bg.end(cx);
        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);

        // We must re-set the blocked scrolling area, as it might've changed after each draw.
        if self.is_open {
            let content_area = self.view.widget(cx, ids!(content)).area();
            // Move the key focus along with the area, or the keyboard stops
            // reaching a modal that has merely repainted.
            self.content_area = cx.update_area_refs(self.content_area, content_area);
            if self.wants_focus && !self.content_area.is_empty() {
                self.wants_focus = false;
                cx.set_key_focus(self.content_area);
            }
            cx.block_scrolling_except_within(content_area);
            // An open before the first draw had no area to lock with.
            self.take_lock(cx);
        }
        DrawStep::done()
    }
}

impl Drop for Modal {
    fn drop(&mut self) {
        if !self.is_open {
            return;
        }
        if self.locked {
            orphan_sweep_locks(&[self.draw_bg.area()]);
        }
        if !self.content_area.is_empty() {
            orphan_scroll_blocks(&[self.content_area]);
        }
    }
}

impl Modal {
    /// Whether the modal is showing. A widget built on one needs this as
    /// much as a caller holding a reference does.
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// The scrim's area: what this modal holds the pointer with, and what a
    /// press on the scrim captures.
    ///
    /// A widget built on a modal needs it to tell its OWN pointer from
    /// another control's. While a control holds the mouse the interaction is
    /// locked to that control, so a raw gesture on the panel — a sheet's
    /// grabber, a press read straight off the event — must stand down; but a
    /// press on the scrim is captured by this area and is the panel's own,
    /// not somebody else's. See `Fingers::is_mouse_held_outside`.
    pub fn scrim_area(&self) -> Area {
        self.draw_bg.area()
    }

    pub fn open(&mut self, cx: &mut Cx) {
        if !self.is_open {
            // Before this modal moves the keyboard into its content.
            self.restore_focus = cx.key_focus();
        }
        self.is_open = true;
        // Assigning drops any previous scope, which matters because `open()` has no
        // already-open guard and callers re-open freely.
        self.cancel_scope = Some(self.begin_cancel_scope(cx));
        // Redraw the overlay draw_list directly so the first open is visible
        // even before the overlay content has refreshed its draw area.
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
        let content = self.view.widget(cx, ids!(content));
        // NOT the key focus here: at this point the content has never been
        // drawn and its area is Empty, so the focus went to nothing at all.
        // It is taken in `draw_walk`, where the content has an area to take
        // it with.
        self.wants_focus = true;
        content.set_scroll_pos(cx, Vec2d { x: 0.0, y: 0.0 });
        self.take_lock(cx);
    }

    /// Take the sweep lock, once there is an area to take it with.
    fn take_lock(&mut self, cx: &mut Cx) {
        let area = self.draw_bg.area();
        if self.is_open && !self.locked && area.is_valid(cx) {
            cx.sweep_lock(area);
            self.locked = true;
        }
    }

    /// Clear the way for one event's dispatch to the content and the scrim:
    /// this modal's lock and every lock beneath it leave the stack, and the
    /// locks above it stay. Answers what left, innermost first, for
    /// `restore_lock`.
    ///
    /// Beneath is only what this modal lies over, so none of it may turn
    /// the content away: a modal opened over another one, beside it in the
    /// tree, would otherwise find its own buttons dead under the first
    /// one's lock. Above is an overlay opened over this modal. One opened
    /// over it from elsewhere still turns the content away, and one opened
    /// inside the content lifts its own lock around its own content and so
    /// finds nothing left in its way. With nothing above, the content hits
    /// exactly as it would with no lock at all.
    fn lift_lock(&mut self, cx: &mut Cx) -> Option<Vec<Area>> {
        if !self.locked {
            return None;
        }
        let ours = self.draw_bg.area();
        let mut above = Vec::new();
        let mut found = false;
        while let Some(top) = cx.sweep_lock_area() {
            if same_slot(top, ours) {
                found = true;
                break;
            }
            cx.sweep_unlock(top);
            above.push(top);
        }
        let mut lifted = Vec::new();
        if found {
            while let Some(top) = cx.sweep_lock_area() {
                cx.sweep_unlock(top);
                lifted.push(top);
            }
        } else {
            // Released by something else: taken again on the next draw.
            self.locked = false;
        }
        for area in above.into_iter().rev() {
            cx.sweep_lock(area);
        }
        found.then_some(lifted)
    }

    /// Put back what `lift_lock` took out, under whatever holds the pointer
    /// now: an overlay opened inside the content while the way was clear
    /// keeps the pointer over this modal. A modal closed meanwhile does not
    /// take its lock back.
    fn restore_lock(&mut self, cx: &mut Cx, lifted: Option<Vec<Area>>) {
        let Some(lifted) = lifted else {
            return;
        };
        let mut now = Vec::new();
        while let Some(top) = cx.sweep_lock_area() {
            cx.sweep_unlock(top);
            now.push(top);
        }
        let ours = self.draw_bg.area();
        for area in lifted.into_iter().rev() {
            if !self.is_open && same_slot(area, ours) {
                continue;
            }
            cx.sweep_lock(area);
        }
        for area in now.into_iter().rev() {
            cx.sweep_lock(area);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        // Closing an already-closed modal must be a no-op. App code routinely
        // calls `close()` defensively (handlers that dismiss a modal whether
        // or not it happens to be open). Without this guard the focus
        // hand-back below would still run and yank key focus away
        // from an unrelated widget — e.g. a text field the user just tapped,
        // which on mobile then dismisses the soft keyboard.
        if !self.is_open {
            return;
        }
        if let Some(scope) = self.cancel_scope.take() {
            cx.end_cancel_scope(scope);
        }
        // Inform the inner modal content that its modal is being dismissed.
        let content = self.view.widget(cx, ids!(content));
        content.handle_event(
            cx,
            &Event::Actions(vec![Box::new(ModalAction::Dismissed)]),
            &mut Scope::empty(),
        );
        self.is_open = false;
        // Overlay widgets need their dedicated draw_list invalidated explicitly;
        // a background redraw alone can leave the previous frame visible too long.
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
        if std::mem::take(&mut self.locked) {
            cx.sweep_unlock(self.draw_bg.area());
        }
        // Back where it was when the modal opened, at the handle that place
        // has now. Not the platform's previous focus: that is the content
        // itself as soon as anything inside it took the keyboard.
        let back = area_after_redraws(cx, std::mem::take(&mut self.restore_focus));
        cx.set_key_focus(back);
        // Release only this modal's block. A modal closed under another one
        // (the platform nests these) must leave the other's block in place;
        // the plain `unblock_scrolling` pops whichever block is innermost.
        cx.unblock_scrolling_within_area(content.area());
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).cast(),
            ModalAction::Dismissed
        )
    }
}

impl ModalRef {
    /// Returns whether the modal is currently open (displayed).
    pub fn is_open(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_open
        } else {
            false
        }
    }

    /// Opens (displays) the model.
    #[doc(alias = "show")]
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    /// Closes (hides) the modal.
    #[doc(alias = "hide")]
    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    /// Returns `true` if this modal was dismissed by the given `actions`.
    pub fn dismissed(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.dismissed(actions)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::ButtonAction;
    use crate::makepad_draw::cx_draw::CxDraw;
    use crate::popover::PopoverWidgetRefExt;
    use std::cell::Cell;

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    const SIZE: DVec2 = DVec2 { x: 800.0, y: 600.0 };

    /// A window-less pass with the overlay a window keeps, which the modal's
    /// own list hangs off.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    /// A page filled by one button, a modal over it with a button in its
    /// content, and a popover in the content as well.
    fn page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    under := Button{width: Fill height: Fill text: "under"}
                    modal := Modal{
                        content := View{
                            width: 300.
                            height: 200.
                            flow: Down
                            inner := Button{width: Fill height: 60. text: "inner"}
                            pop := PopoverToggle{
                                opener := Button{width: 120. height: 40. text: "more"}
                                content := View{
                                    width: 160.
                                    height: 40.
                                    choice := Button{width: Fill height: Fill text: "choice"}
                                }
                            }
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn release(abs: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    /// The event walked the way a shell walks a side bar before the page
    /// that holds the modal: the widget under the scrim first.
    fn walk(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        let under = root.widget(cx, ids!(under));
        let modal = root.widget(cx, ids!(modal));
        cx.capture_actions(|cx| {
            under.handle_event(cx, event, &mut Scope::empty());
            modal.handle_event(cx, event, &mut Scope::empty());
        })
    }

    fn pressed(actions: &Actions, button: &WidgetRef) -> bool {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .any(|action| action.widget_uid == button.widget_uid() && matches!(action.cast::<ButtonAction>(), ButtonAction::Pressed(_)))
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> DVec2 {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    /// A press lands on the content or on the scrim, never on the page under
    /// them, even when the page is walked first; a press on the scrim sends
    /// the modal away and gives the pointer back.
    #[test]
    fn nothing_walked_before_the_modal_hears_a_press_through_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let modal = root.widget(&cx, ids!(modal));
        let under = root.widget(&cx, ids!(under));
        let inner = root.widget(&cx, ids!(inner));
        modal.as_modal().open(&mut cx);
        target.draw(&mut cx, &root);

        let on_inner = middle(&cx, &inner);
        let actions = walk(&mut cx, &root, &press(on_inner));
        assert!(!pressed(&actions, &under), "the page under the content took the press");
        assert!(pressed(&actions, &inner), "the content's button did not");
        walk(&mut cx, &root, &release(on_inner));
        assert!(modal.as_modal().is_open(), "a press on the content keeps the modal");

        let on_scrim = dvec2(20.0, 20.0);
        let actions = walk(&mut cx, &root, &press(on_scrim));
        assert!(!pressed(&actions, &under), "the page under the scrim took the press");
        assert!(modal.as_modal().is_open());
        walk(&mut cx, &root, &release(on_scrim));
        assert!(!modal.as_modal().is_open(), "a press on the scrim dismisses");
        assert_eq!(cx.sweep_lock_area(), None, "and gives the pointer back");
        });
    }

    /// A popover opened inside the content keeps the pointer over the modal
    /// that holds it: its choice hears the press, the modal stays open.
    #[test]
    fn an_overlay_opened_inside_the_content_stays_above_the_modal() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let modal = root.widget(&cx, ids!(modal));
        modal.as_modal().open(&mut cx);
        target.draw(&mut cx, &root);
        let held = cx.sweep_lock_area().expect("an open modal holds the pointer");

        let opener = middle(&cx, &root.widget(&cx, ids!(opener)));
        walk(&mut cx, &root, &press(opener));
        walk(&mut cx, &root, &release(opener));
        let pop = root.widget(&cx, ids!(pop));
        assert!(pop.as_popover().is_open(), "the popover in the content opened");
        target.draw(&mut cx, &root);
        assert_ne!(cx.sweep_lock_area(), Some(held), "the popover's lock is on top");
        // Walked again, which lifts and takes the modal's lock again.
        walk(&mut cx, &root, &Event::Actions(Vec::new()));
        assert_ne!(cx.sweep_lock_area(), Some(held), "still on top after the modal took its lock back");

        let choice = root.widget(&cx, ids!(choice));
        let on_choice = middle(&cx, &choice);
        let actions = walk(&mut cx, &root, &press(on_choice));
        assert!(pressed(&actions, &choice), "the popover's choice did not hear its press");
        assert!(modal.as_modal().is_open());
        });
    }

    /// A popover on the page under the scrim is as deaf as a button there:
    /// a press on its anchor opens nothing while the modal is up, in either
    /// walk order, and the release of it is the scrim's, which dismisses.
    /// With the modal gone the same press opens the popover.
    #[test]
    fn a_popover_under_the_scrim_does_not_open() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    page_pop := PopoverToggle{
                        page_opener := Button{width: 200. height: 40. text: "menu"}
                        content := View{
                            width: 160.
                            height: 40.
                            page_choice := Button{width: Fill height: Fill text: "choice"}
                        }
                    }
                    modal := Modal{
                        content := View{
                            width: 300.
                            height: 200.
                            inner := Button{width: Fill height: Fill text: "inner"}
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let pop = root.widget(&cx, ids!(page_pop));
        let modal = root.widget(&cx, ids!(modal));
        let on_anchor = middle(&cx, &root.widget(&cx, ids!(page_opener)));

        for pop_first in [true, false] {
            modal.as_modal().open(&mut cx);
            target.draw(&mut cx, &root);
            assert!(
                !root.widget(&cx, ids!(inner)).area().rect(&cx).contains(on_anchor),
                "the anchor lies under the scrim, not the content"
            );
            let order = if pop_first { [&pop, &modal] } else { [&modal, &pop] };
            for event in [press(on_anchor), release(on_anchor)] {
                cx.capture_actions(|cx| {
                    for widget in order {
                        widget.handle_event(cx, &event, &mut Scope::empty());
                    }
                });
                assert!(!pop.as_popover().is_open(), "the popover under the scrim opened (popover walked first: {pop_first})");
            }
            assert!(!modal.as_modal().is_open(), "the press was the scrim's, and dismissed");
            assert_eq!(cx.sweep_lock_area(), None);
            target.draw(&mut cx, &root);
        }

        for event in [press(on_anchor), release(on_anchor)] {
            cx.capture_actions(|cx| {
                pop.handle_event(cx, &event, &mut Scope::empty());
                modal.handle_event(cx, &event, &mut Scope::empty());
            });
        }
        assert!(pop.as_popover().is_open(), "with the modal gone the press opens it");
        });
    }

    /// A modal opened over another one beside it in the tree hears its own
    /// content in whichever order the two are walked, and the one under it
    /// hears nothing; closed, it gives the pointer back to the first.
    #[test]
    fn a_modal_opened_over_another_hears_its_own_content() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    first := Modal{
                        content := View{
                            width: 400.
                            height: 300.
                            first_button := Button{width: Fill height: Fill text: "first"}
                        }
                    }
                    second := Modal{
                        content := View{
                            width: 200.
                            height: 100.
                            second_button := Button{width: Fill height: Fill text: "second"}
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let first = root.widget(&cx, ids!(first));
        let second = root.widget(&cx, ids!(second));
        first.as_modal().open(&mut cx);
        target.draw(&mut cx, &root);
        let held = cx.sweep_lock_area();
        second.as_modal().open(&mut cx);
        target.draw(&mut cx, &root);

        let first_button = root.widget(&cx, ids!(first_button));
        let second_button = root.widget(&cx, ids!(second_button));
        let on_second = middle(&cx, &second_button);
        assert!(first_button.area().rect(&cx).contains(on_second), "the second lies over the first");
        for order in [[&first, &second], [&second, &first]] {
            let actions = cx.capture_actions(|cx| {
                for modal in order {
                    modal.handle_event(cx, &press(on_second), &mut Scope::empty());
                }
            });
            assert!(pressed(&actions, &second_button), "the modal on top did not hear its own button");
            assert!(!pressed(&actions, &first_button), "the modal underneath heard a press through the one on top");
            cx.capture_actions(|cx| {
                for modal in order {
                    modal.handle_event(cx, &release(on_second), &mut Scope::empty());
                }
            });
            assert!(first.as_modal().is_open() && second.as_modal().is_open());
        }
        second.as_modal().close(&mut cx);
        let now = cx.sweep_lock_area().expect("the first modal still holds the pointer");
        assert!(same_slot(now, held.unwrap()), "the first modal has the pointer again, not {now:?}");
        });
    }

    /// The keyboard goes back to what had it when the modal opened, though
    /// the page has been drawn again meanwhile and a widget inside the
    /// content took the keyboard.
    #[test]
    fn closing_gives_the_keyboard_back_after_the_page_has_been_drawn_again() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let modal = root.widget(&cx, ids!(modal));
        let under = root.widget(&cx, ids!(under));
        cx.set_key_focus(under.area());
        settle_focus(&mut cx);
        let before = under.area();

        modal.as_modal().open(&mut cx);
        target.draw(&mut cx, &root);
        settle_focus(&mut cx);
        let inner = root.widget(&cx, ids!(inner)).area();
        cx.set_key_focus(inner);
        settle_focus(&mut cx);
        root.redraw(&mut cx);
        target.draw(&mut cx, &root);
        assert_ne!(under.area(), before, "the page was drawn again, so the handle moved");

        modal.as_modal().close(&mut cx);
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(under.area()), "the keyboard went to {:?}, not back to {:?}", cx.key_focus(), under.area());
        });
    }

    /// A page rebuilt under an open modal takes the modal with it; the next
    /// event gives the pointer and the wheel back.
    #[test]
    fn a_modal_dropped_while_open_lets_go_of_the_pointer_and_the_wheel() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut target = Target::new(&mut cx);
        let old = page(&mut cx);
        target.draw(&mut cx, &old);
        old.widget(&cx, ids!(modal)).as_modal().open(&mut cx);
        target.draw(&mut cx, &old);
        assert!(cx.sweep_lock_area().is_some(), "an open modal holds the pointer");
        drop(old);

        let new = page(&mut cx);
        target.draw(&mut cx, &new);
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        release_orphaned_scroll_blocks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the dropped modal still holds the pointer");
        let area = new.widget(&cx, ids!(under)).area();
        assert!(cx.is_scrolling_allowed_within(&area), "the dropped modal still blocks the wheel");
        });
    }

    /// A handle kept across redraws is followed to the slot's handle now,
    /// and to nothing once the list no longer draws that slot.
    #[test]
    fn a_kept_handle_is_followed_to_the_handle_its_slot_has_now() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let under = root.widget(&cx, ids!(under));
        let kept = under.area();
        assert_eq!(area_after_redraws(&cx, kept), kept, "a fresh handle is its own");
        root.redraw(&mut cx);
        target.draw(&mut cx, &root);
        assert_ne!(under.area(), kept);
        assert_eq!(area_after_redraws(&cx, kept), under.area());
        assert_eq!(area_after_redraws(&cx, Area::Empty), Area::Empty);
        });
    }
}
