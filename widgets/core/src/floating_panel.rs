//! FloatingPanel — a panel the person moves and sizes, that does not stop
//! the work.
//!
//! That last clause is the whole difference from a dialog, and it decides
//! every structural choice here. A dialog asks a question and is owed an
//! answer, so it paints a scrim, takes the keyboard, blocks scrolling and
//! goes away when you press outside it. An inspector, a palette, a set of
//! readings you keep an eye on while you work is none of those things: it
//! stays where you put it, the page underneath keeps working, and clicking
//! elsewhere is not a dismissal. So this derefs `View` and **not** `Modal` —
//! every one of Modal's behaviours would be wrong here.
//!
//! **The title bar is the handle and the bottom-right corner is the grip.**
//! Both are read from what was actually drawn rather than guessed from
//! padding, and both take the presses no control took first. The panel's
//! contents are dispatched before the handles are read, so a control
//! genuinely under the pointer — the scroll bar whose bottom end sits under
//! the corner grip, a field or a button someone put in the title bar — owns
//! its own press and keeps the mouse until it is let go. What is left over
//! is bare chrome, and bare chrome is what moves and sizes the panel.
//!
//! **It cannot be dragged off the screen.** The panel is pulled back inside
//! the pass every draw with `span_inboard`, the same arithmetic every
//! anchored popup already uses. A panel whose title bar is off the top edge
//! cannot be moved back, which makes losing it permanent.
//!
//! Promoted from the design overlay's note card, which has carried this
//! mechanism for months. What stayed behind is everything particular to
//! that overlay: its notes file, its session singleton, its leader line and
//! its own chrome. What came across is the ~80 lines that are the panel.

use crate::{
    button::ButtonWidgetRefExt, label::LabelWidgetRefExt, makepad_derive_widget::*,
    makepad_draw::*, overlay_place::span_inboard, view::View, widget::*,
};

/// What a panel reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum FloatingPanelAction {
    /// The close mark was pressed.
    Closed,
    /// The panel was moved or sized, and where it now is.
    Placed { pos: Vec2d, size: Vec2d },
    #[default]
    None,
}

/// What a press takes hold of, with nothing of the press in it. The
/// question "may this gesture start, and which one" is answered by
/// [`grab_at`] alone, so it can be asked of a test as easily as of a
/// pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Move,
    Size,
}

/// What the pointer is doing to the panel.
#[derive(Clone, Copy, Debug)]
enum Handling {
    /// Moving it: where the pointer took hold, and where the panel was.
    Move { held_at: Vec2d, from: Vec2d },
    /// Sizing it: where the pointer took hold, and how big it was.
    Size { held_at: Vec2d, from: Vec2d },
}

/// How big the corner grip is, in layout points.
const GRIP: f64 = 14.0;

/// What a press at `at` takes hold of, if anything.
///
/// `held_elsewhere` is [`CxFingers::is_mouse_held_outside`] asked with the
/// panel's own area. A control that is dragged continuously — a slider, a
/// scroll bar, another panel's grip — takes the mouse on its press and
/// keeps it until the release, and every other gesture that would start
/// from that same pointer stands down meanwhile. Moving and sizing a panel
/// are two such gestures, so they ask before they start.
///
/// `primary` keeps the secondary button out of it: a right-press is a
/// press on a menu, not a hold on a handle, and this used to answer it by
/// dragging the panel around.
///
/// `unclaimed` is the press with nothing on it yet — `me.handled` still
/// empty. It answers the half of the rule `held_elsewhere` cannot: the
/// press that lands ON a control, whose capture does not exist yet when
/// the press is read. The panel's contents are dispatched before this is
/// asked, so by now the control has both captured and marked the press, and
/// a handle that took it anyway would drag the panel out from under a
/// pointer that is already somebody's.
fn grab_at(
    at: Vec2d,
    band: Rect,
    close_mark: Rect,
    grip: Rect,
    movable: bool,
    resizable: bool,
    primary: bool,
    held_elsewhere: bool,
    unclaimed: bool,
) -> Option<Grab> {
    if !primary || held_elsewhere || !unclaimed {
        return None;
    }
    if resizable && grip.contains(at) {
        return Some(Grab::Size);
    }
    if movable && band.contains(at) && !close_mark.contains(at) {
        return Some(Grab::Move);
    }
    None
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FloatingPanelBase = #(FloatingPanel::register_widget(vm))

    /** A panel the person moves and sizes, that does not stop the work:
     * an inspector, a palette, a set of readings kept in view. */
    mod.widgets.FloatingPanel = set_type_default() do mod.widgets.FloatingPanelBase{
        flow: Overlay
        /** where it opens, in window points */
        pos: vec2(80., 80.)
        /** how big it opens */
        size: vec2(320., 240.)
        /** it never gets smaller than this: the title row needs the width */
        min_size: vec2(160., 96.)
        /** the title bar moves it 0..1 step 1 */
        movable: true
        /** the bottom-right corner sizes it 0..1 step 1 */
        resizable: true
        /** the title, also what `text()` answers */
        title: ""
        /** a close mark on the title bar 0..1 step 1 */
        closable: true

        content := RoundedShadowView{
            width: Fill
            height: Fill
            flow: Down
            show_bg: true
            draw_bg +: {
                color: theme.color_surface_container_high
                border_color: theme.color_outline
                border_size: 1.0
                border_radius: theme.radius_l
                shadow_color: theme.color_elevation_4
                shadow_radius: uniform(theme.elevation_4_radius)
                shadow_offset: uniform(vec2(0., theme.elevation_4_offset_y))
            }

            header := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{y: 0.5}
                padding: Inset{left: 14. right: 6. top: 8. bottom: 8.}
                spacing: theme.space_2
                title_label := Label{
                    width: Fill
                    draw_text +: {
                        text_style: theme.font_title_s
                        color: theme.color_on_surface
                    }
                }
                close := ButtonFlat{
                    width: 22.
                    height: 22.
                    text: "\u{00d7}"
                }
            }

            body := View{
                width: Fill
                height: Fill
                flow: Down
                spacing: theme.space_2
                padding: Inset{left: 14. right: 14. top: 0. bottom: 12.}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct FloatingPanel {
    #[source]
    source: ScriptObjectRef,

    #[deref]
    view: View,

    #[rust]
    draw_list: Option<DrawList2d>,

    #[live(Vec2d { x: 80., y: 80. })]
    pub pos: Vec2d,
    #[live(Vec2d { x: 320., y: 240. })]
    pub size: Vec2d,
    #[live(Vec2d { x: 160., y: 96. })]
    pub min_size: Vec2d,
    #[live(true)]
    pub movable: bool,
    #[live(true)]
    pub resizable: bool,
    #[live]
    pub title: String,
    #[live(true)]
    pub closable: bool,

    #[rust]
    open: bool,
    /// Whether the chrome has been written from the props this open.
    #[rust]
    dressed: bool,
    #[rust]
    handling: Option<Handling>,
    /// The title bar and the close mark as they were last drawn. Read from
    /// the draw rather than computed from padding, so restyling the header
    /// cannot quietly move the handle out from under the pointer.
    #[rust]
    band: Rect,
    #[rust]
    close_mark: Rect,
    /// The sheet as it was last drawn — the area the drag CAPTURES, so the
    /// pointer is locked to this panel from the press until the release.
    /// The deref `View`'s own area is empty here (this widget draws its
    /// content itself onto an overlay list and never ends a turtle into
    /// it), so it is the content's area or nothing.
    #[rust]
    panel_area: Area,
}

impl ScriptHook for FloatingPanel {
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
        // The panel takes NO room in the layout that holds it: it paints on
        // its own overlay list against a root turtle for the pass. Reporting
        // `Fill` upward would make it a deferred fill and hand it a share of
        // its parent's spare height for a panel that may not even be open —
        // the bug spelled out in `modal.rs`, forced closed here for the same
        // reason it is forced closed there.
        self.view.walk = Walk::empty();
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl FloatingPanel {
    pub fn open(&mut self, cx: &mut Cx) {
        if !self.open {
            self.open = true;
            self.dressed = false;
            self.handling = None;
            self.redraw(cx);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if self.open {
            self.open = false;
            self.handling = None;
            self.redraw(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Where the panel is and how big, in window points.
    pub fn placement(&self) -> (Vec2d, Vec2d) {
        (self.pos, self.size)
    }

    /// Put the panel somewhere. The caller keeps the value if it wants it
    /// back next run: a widget that writes files is a widget that has
    /// learned something it has no business knowing.
    pub fn place(&mut self, cx: &mut Cx, pos: Vec2d, size: Vec2d) {
        self.pos = pos;
        self.size = size;
        self.redraw(cx);
    }

    fn dress(&mut self, cx: &mut Cx) {
        let content = self.view.widget(cx, ids!(content));
        content.label(cx, ids!(title_label)).set_text(cx, &self.title);
        content.widget(cx, ids!(close)).set_visible(cx, self.closable);
    }

    /// The corner grip, in window points.
    fn grip_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.pos.x + self.size.x - GRIP, self.pos.y + self.size.y - GRIP),
            size: dvec2(GRIP, GRIP),
        }
    }

    fn say_placed(&mut self, cx: &mut Cx) {
        let uid = self.widget_uid();
        let (pos, size) = (self.pos, self.size);
        cx.widget_action(uid, FloatingPanelAction::Placed { pos, size });
    }
}

impl Widget for FloatingPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(self.view.layout);

        if self.open {
            if !self.dressed {
                self.dressed = true;
                self.dress(cx.cx.cx);
            }
            // Never off the screen. A panel whose title bar has gone past an
            // edge cannot be dragged back, so losing it would be permanent.
            let pass = cx.current_pass_size();
            self.size.x = self.size.x.max(self.min_size.x).min(pass.x.max(self.min_size.x));
            self.size.y = self.size.y.max(self.min_size.y).min(pass.y.max(self.min_size.y));
            self.pos.x = span_inboard(self.pos.x, self.size.x, 0.0, pass.x);
            self.pos.y = span_inboard(self.pos.y, self.size.y, 0.0, pass.y);

            let content = self.view.widget(cx.cx.cx, ids!(content));
            let mut view = content.borrow_mut::<crate::view::View>();
            if let Some(view) = view.as_mut() {
                view.walk = Walk {
                    width: Size::Fixed(self.size.x),
                    height: Size::Fixed(self.size.y),
                    abs_pos: Some(self.pos),
                    ..Walk::default()
                };
            }
            drop(view);
            let _ = content.draw_all(cx, scope);

            // The handle, the mark and the sheet, as drawn.
            self.band = content.widget(cx.cx.cx, ids!(header)).area().rect(cx.cx.cx);
            self.close_mark = content.widget(cx.cx.cx, ids!(close)).area().rect(cx.cx.cx);
            self.panel_area = content.area();
        }

        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.open {
            return;
        }
        let grip = self.grip_rect();
        let panel = Rect { pos: self.pos, size: self.size };

        // A drag that has hold of the pointer hears about it wherever the
        // pointer goes, and hears about it first. That is what the capture
        // taken at the press below buys: `hits` hands these moves to this
        // panel alone, outside its own bounds included, and no other widget
        // takes a hover or a press from them on the way past.
        if self.handling.is_some() {
            match event.hits(cx, self.panel_area) {
                Hit::FingerMove(fe) => match self.handling {
                    Some(Handling::Move { held_at, from }) => {
                        self.pos = from + (fe.abs - held_at);
                        cx.set_cursor(MouseCursor::Move);
                        self.redraw(cx);
                        return;
                    }
                    Some(Handling::Size { held_at, from }) => {
                        let to = from + (fe.abs - held_at);
                        self.size = dvec2(to.x.max(self.min_size.x), to.y.max(self.min_size.y));
                        cx.set_cursor(MouseCursor::NwseResize);
                        self.redraw(cx);
                        return;
                    }
                    None => {}
                },
                _ => {}
            }
            // The release ends it, and the RAW release does — not the hit.
            // A capture can be handed on or dropped under a widget's feet
            // (a sweep, a redraw that lost the area), and a panel still
            // holding a drag it was never told had ended would follow the
            // pointer with the button up.
            if let Event::MouseUp(_) = event {
                if self.handling.take().is_some() {
                    self.say_placed(cx);
                    self.redraw(cx);
                    return;
                }
            }
        }

        // The contents go FIRST, and that order is the whole fix. A gesture
        // that starts from a pointer may only start on a press nothing else
        // wanted, and neither way of asking works before the contents have
        // run: a capture does not exist until the control has been given the
        // press, so `is_mouse_held_outside` asked ahead of them was blind to
        // exactly the case it is there for — a slider inside the panel,
        // pressed, and the panel dragged out from under it.
        //
        // The two other ways of getting there were weighed and lost.
        // DECIDING later, on the first move, gives up the capture: `hits`
        // takes one on a press and nowhere else, so the panel would be back
        // to dragging off raw moves with no lock on the pointer at all.
        // Testing for bare background with
        // `find_interactive_widget_from_point` cannot see the body's scroll
        // bars — a `View` keeps those beside its children and the search
        // walks children — which is the collision the default chrome
        // actually has, the vertical bar's bottom end under the 14pt corner
        // grip; and it WOULD see this panel's own title `Label`, which does
        // not opt out of `is_interactive` (`window.rs` records the same
        // trap), leaving the bar undraggable by the one thing it is for.
        self.view.widget(cx, ids!(content)).handle_event(cx, event, scope);

        // Everything this panel owns, for the one question a gesture that
        // starts from a pointer has to ask: is the mouse already held by
        // something else? The panel draws its whole subtree into one area,
        // so that area is the whole answer. Asked after the contents ran, so
        // a control inside the panel that just took the press is on the list.
        let held_elsewhere = cx.fingers.is_mouse_held_outside(&[self.panel_area]);

        // The handle takes a press the contents did not.
        //
        // It takes it through `hits` and not the raw press it used to read,
        // because `hits` CAPTURES: from here until the release the mouse
        // belongs to this panel. Before that, `grab_at` asks whether there is
        // anything to take at all — a control that already holds the mouse
        // keeps it, a control that has just been handed this press keeps it,
        // and a secondary press holds nothing.
        if let Event::MouseDown(me) = event {
            if self.handling.is_none() {
                let grab = grab_at(
                    me.abs,
                    self.band,
                    self.close_mark,
                    grip,
                    self.movable,
                    self.resizable,
                    me.button.is_primary(),
                    held_elsewhere,
                    me.handled.get().is_empty(),
                );
                if let Some(grab) = grab {
                    if let Hit::FingerDown(fe) = event.hits(cx, self.panel_area) {
                        self.handling = Some(match grab {
                            Grab::Move => Handling::Move { held_at: fe.abs, from: self.pos },
                            Grab::Size => Handling::Size { held_at: fe.abs, from: self.size },
                        });
                        return;
                    }
                }
            }
        }

        if let Event::Actions(actions) = event {
            let content = self.view.widget(cx, ids!(content));
            if content.widget(cx, ids!(close)).as_button().clicked(actions) {
                let uid = self.widget_uid();
                self.close(cx);
                cx.widget_action(uid, FloatingPanelAction::Closed);
            }
        }

        match event {
            // The corner says what it does before it is grabbed — unless the
            // mouse is out on loan to a drag somewhere else, in which case
            // the pointer showing is that drag's to decide and not ours to
            // paint a resize arrow over.
            Event::MouseMove(me) if self.handling.is_none() => {
                if self.resizable
                    && grip.contains(me.abs)
                    && me.handled.get().is_empty()
                    && !held_elsewhere
                {
                    cx.set_cursor(MouseCursor::NwseResize);
                }
            }
            // A press anywhere on the panel is the panel's, so the page
            // beneath it does not also act on it. Outside the panel nothing
            // is claimed — that is what "does not stop the work" means.
            //
            // A press one of the panel's OWN contents took is that widget's,
            // and taking the key focus here would pull it straight back off
            // the caret it just placed: the contents run above, so a press
            // they wanted is marked handled by the time this is read.
            Event::MouseDown(me) => {
                if panel.contains(me.abs) && me.handled.get().is_empty() && !held_elsewhere {
                    cx.set_key_focus(self.panel_area);
                }
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.title != v {
            self.title = v.to_string();
            self.dressed = false;
            self.redraw(cx);
        }
    }
}

impl FloatingPanelRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// Where it is and how big, for a caller that wants to put it back.
    pub fn placement(&self) -> Option<(Vec2d, Vec2d)> {
        self.borrow().map(|inner| inner.placement())
    }

    pub fn place(&self, cx: &mut Cx, pos: Vec2d, size: Vec2d) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.place(cx, pos, size);
        }
    }

    /// Whether the close mark was pressed this pass.
    pub fn closed(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|a| matches!(a.cast::<FloatingPanelAction>(), FloatingPanelAction::Closed))
            .unwrap_or(false)
    }

    /// Where a drag just left it, if one did.
    pub fn placed(&self, actions: &Actions) -> Option<(Vec2d, Vec2d)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<FloatingPanelAction>() {
            FloatingPanelAction::Placed { pos, size } => Some((pos, size)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel is pulled back inside the pass, so one dragged at the edge
    /// cannot end up somewhere it can never be dragged back from.
    #[test]
    fn a_panel_cannot_be_left_off_the_screen() {
        crate::on_test_cx(|| {
        // Off the right: pulled back so its right edge sits on the edge.
        assert_eq!(span_inboard(900.0, 320.0, 0.0, 1000.0), 680.0);
        // Off the left: the low edge wins.
        assert_eq!(span_inboard(-50.0, 320.0, 0.0, 1000.0), 0.0);
        // Already inside: untouched.
        assert_eq!(span_inboard(100.0, 320.0, 0.0, 1000.0), 100.0);
        });
    }

    /// A panel at 100,100 sized 320x240: its title bar, its close mark and
    /// its corner, where a draw would leave them.
    fn handles() -> (Rect, Rect, Rect) {
        let band = Rect { pos: dvec2(100.0, 100.0), size: dvec2(320.0, 38.0) };
        let close_mark = Rect { pos: dvec2(392.0, 108.0), size: dvec2(22.0, 22.0) };
        let grip = Rect { pos: dvec2(406.0, 326.0), size: dvec2(14.0, 14.0) };
        (band, close_mark, grip)
    }

    /// What each part of the panel does with a press, and what the parts
    /// that are not handles do with one: nothing, so the press goes on to
    /// the contents.
    #[test]
    fn the_bar_moves_it_the_corner_sizes_it_and_the_close_mark_does_neither() {
        crate::on_test_cx(|| {
        let (band, close_mark, grip) = handles();
        let take = |at| grab_at(at, band, close_mark, grip, true, true, true, false, true);
        assert_eq!(take(dvec2(200.0, 115.0)), Some(Grab::Move), "the title bar is the handle");
        assert_eq!(take(dvec2(410.0, 330.0)), Some(Grab::Size), "the corner is the grip");
        assert_eq!(take(dvec2(400.0, 115.0)), None, "the close mark closes, it does not move");
        assert_eq!(take(dvec2(200.0, 250.0)), None, "the body belongs to the contents");
        assert_eq!(take(dvec2(50.0, 50.0)), None, "and the page is not the panel");
        // A panel told it does not move, or does not size, does not.
        assert_eq!(
            grab_at(dvec2(200.0, 115.0), band, close_mark, grip, false, true, true, false, true),
            None
        );
        assert_eq!(
            grab_at(dvec2(410.0, 330.0), band, close_mark, grip, true, false, true, false, true),
            None
        );
        });
    }

    /// The app-wide rule, at the one place this panel can break it: a
    /// control that is dragged continuously holds the mouse from its press
    /// until its release, and neither of the panel's two gestures may start
    /// from that pointer meanwhile.
    ///
    /// Reachable with one hand: hold a slider down and press the panel's
    /// corner with the other button. The panel used to read the raw press,
    /// ask nobody, and start sizing itself against a pointer the slider was
    /// still tracking.
    #[test]
    fn neither_gesture_starts_while_another_control_holds_the_mouse() {
        crate::on_test_cx(|| {
        let (band, close_mark, grip) = handles();
        let held = |at| grab_at(at, band, close_mark, grip, true, true, true, true, true);
        assert_eq!(held(dvec2(200.0, 115.0)), None, "the bar stands down");
        assert_eq!(held(dvec2(410.0, 330.0)), None, "and so does the corner");
        });
    }

    /// A press that is not the primary button holds nothing either. It is a
    /// press on a menu on its way somewhere, and this used to answer it by
    /// carrying the panel around.
    #[test]
    fn a_secondary_press_takes_no_handle() {
        crate::on_test_cx(|| {
        let (band, close_mark, grip) = handles();
        let second = |at| grab_at(at, band, close_mark, grip, true, true, false, false, true);
        assert_eq!(second(dvec2(200.0, 115.0)), None);
        assert_eq!(second(dvec2(410.0, 330.0)), None);
        });
    }

    /// The same rule at the press itself, where no capture exists yet to
    /// ask about. A control the press landed on has been handed it by the
    /// dispatch above and has marked it; a handle that took it anyway would
    /// be moving the panel with a pointer that is already somebody's.
    #[test]
    fn a_press_a_control_has_already_taken_takes_no_handle() {
        crate::on_test_cx(|| {
        let (band, close_mark, grip) = handles();
        let taken = |at| grab_at(at, band, close_mark, grip, true, true, true, false, false);
        assert_eq!(taken(dvec2(200.0, 115.0)), None, "a control in the title bar keeps its press");
        assert_eq!(taken(dvec2(410.0, 330.0)), None, "and one under the corner keeps its press");
        });
    }

    /// A panel wider than the window keeps its low edge on screen rather
    /// than being pushed off the other side to make its width fit.
    #[test]
    fn a_panel_wider_than_the_window_keeps_its_near_edge() {
        crate::on_test_cx(|| {
        assert_eq!(span_inboard(40.0, 1200.0, 0.0, 1000.0), 0.0);
        });
    }

    // The two above are the decision on its own. What follows presses a
    // real panel, because the decision is only as good as the moment it is
    // asked at, and that moment is an ordering — the thing a pure test of
    // `grab_at` cannot see.

    use crate::button::ButtonAction;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: DVec2 = DVec2 { x: 800.0, y: 600.0 };

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// A window-less pass with the overlay a window keeps, which the panel's
    /// own draw list hangs off.
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

    /// What the panel has hold of, if anything. The moves that carry a
    /// drag arrive as `Hit::FingerMove`, which needs the button state the
    /// real event pump keeps, so what a test can see is the decision itself:
    /// whether the press was taken as a handle at all.
    fn handling(panel: &WidgetRef) -> Option<Handling> {
        panel.borrow::<FloatingPanel>().expect("a FloatingPanel").handling
    }

    fn pressed(actions: &Actions, button: &WidgetRef) -> bool {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .any(|action| {
                action.widget_uid == button.widget_uid()
                    && matches!(action.cast::<ButtonAction>(), ButtonAction::Pressed(_))
            })
    }

    /// A panel with a button in its title bar, open and drawn where it was
    /// told to stand.
    fn panel_page(cx: &mut Cx, target: &mut Target) -> WidgetRef {
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    panel := FloatingPanel{
                        title: "Readings"
                        pos: vec2(100., 100.)
                        size: vec2(320., 240.)
                        content +: {
                            header +: {
                                knob := Button{width: 40. height: 22. text: "K"}
                            }
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        target.draw(cx, &root);
        root.widget(cx, ids!(panel)).as_floating_panel().open(cx);
        target.draw(cx, &root);
        root
    }

    /// The app-wide rule where this panel could only break it at the press
    /// itself: a control sitting IN the title bar owns presses that land on
    /// it, and the handle around it does not carry the panel off instead.
    ///
    /// The contents used to be dispatched after the handle had already taken
    /// the press and returned, so the button never saw it at all — and no
    /// capture had been taken for `is_mouse_held_outside` to find.
    #[test]
    fn a_control_in_the_title_bar_keeps_its_press() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut target = Target::new(&mut cx);
        let root = panel_page(&mut cx, &mut target);
        let panel = root.widget(&cx, ids!(panel));
        let knob = panel.widget(&cx, ids!(knob));
        let knob_rect = knob.area().rect(&cx);
        assert!(knob_rect.size.x > 0.0, "the button in the title bar is drawn");
        let band = panel.widget(&cx, ids!(header)).area().rect(&cx);
        let at = knob_rect.pos + knob_rect.size * 0.5;
        assert!(band.contains(at), "and it stands on the handle, which is the whole point");

        let actions = cx.capture_actions(|cx| root.handle_event(cx, &press(at), &mut Scope::empty()));
        assert!(pressed(&actions, &knob), "the press reached the button");
        assert!(handling(&panel).is_none(), "and the handle around it took nothing");
        });
    }

    /// And the handle still works: the bare bar moves the panel. The title
    /// `Label` lies across it and does not stand the handle down — a `Label`
    /// reports where the pointer is and never takes it, which is why the
    /// question asked here is who TOOK the press rather than what is drawn
    /// under it.
    #[test]
    fn the_bare_title_bar_still_moves_the_panel() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut target = Target::new(&mut cx);
        let root = panel_page(&mut cx, &mut target);
        let panel = root.widget(&cx, ids!(panel));
        let band = panel.widget(&cx, ids!(header)).area().rect(&cx);
        let knob = panel.widget(&cx, ids!(knob)).area().rect(&cx);
        let close = panel.widget(&cx, ids!(close)).area().rect(&cx);
        let at = dvec2(band.pos.x + 30.0, band.pos.y + band.size.y * 0.5);
        assert!(band.contains(at), "the press lands on the bar");
        assert!(!knob.contains(at) && !close.contains(at), "and on none of the controls standing in it");

        root.handle_event(&mut cx, &press(at), &mut Scope::empty());
        assert!(
            matches!(handling(&panel), Some(Handling::Move { .. })),
            "the bar took the handle"
        );
        });
    }
}
