//! DropSlider — a compact value chip that opens a POPOVER slider.
//!
//! For bars and other tight chrome where an inline slider would fight
//! window-dragging or crowd the line: the widget itself is a small click
//! target (icon + live readout); a CLICK opens a horizontal slider in an
//! overlay panel just below the chip, dragging there sets the value live,
//! and releasing or clicking anywhere else closes it. No drag interaction
//! exists on the chip itself, by construction.
//!
//! The popover draws on its own overlay draw list (the `TipLayer` idiom)
//! from the chip's FINAL rect, so it floats over every panel and never
//! disturbs layout. Value semantics mirror `ValueInput`: `min`/`max`,
//! `display_scale`/`precision`/`suffix` shape the readout (`0.9` shown as
//! `90%` with scale 100, suffix "%"). Every change emits
//! [`DropSliderAction::Changed`]; hosts push external updates back with
//! `set_value`.

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{orphan_sweep_locks, place_overlay, PlaceRequest, Placement},
    widget::*,
};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum DropSliderAction {
    /// The value changed (live while the popover slider is dragged).
    Changed(f64),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DropSliderBase = #(DropSlider::register_widget(vm))
    mod.widgets.DropSlider = set_type_default() do mod.widgets.DropSliderBase{
        width: Fit
        height: 22
        padding: Inset{left: 7.0 right: 8.0 top: 0.0 bottom: 0.0}
        align: Align{x: 0.0, y: 0.5}

        draw_bg +: {
            hover: uniform(0.0)
            open: uniform(0.0)
            color: #x272e38
            color_hover: #x2f3842
            border_color: #xffffff26
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, theme.corner_radius)
                sdf.fill(self.color.mix(self.color_hover, max(self.hover, self.open)))
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_icon +: {
            color: #xd6dee6
        }
        icon_walk: Walk{width: 10 height: Fit margin: Inset{right: 5.0}}
        draw_text +: {
            color: #xf4f7fa
            text_style: theme.font_bold{font_size: 9}
        }
        // The popover panel + its slider parts.
        draw_panel +: {
            color: #x181c23
            border_color: #xffffff2e
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, theme.corner_radius)
                sdf.fill(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_track +: { color: #x2b343f }
        draw_fill +: { color: #xff5c39 }
        draw_knob +: { color: #xe8eef4 }
    }
}

/// What a raw press while the popover is open turns out to be.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PanelPress {
    /// In the panel, and nothing else holds the mouse: scrub, and the press
    /// is spent.
    Scrub,
    /// In the panel, but a control that is dragged continuously holds the
    /// mouse. No scrub starts — that control keeps the pointer until the
    /// release — and the press is spent all the same, since the panel is
    /// drawn over whatever is under it.
    Swallow,
    /// On the chip: the chip's own hit deals with it, this must not.
    Chip,
    /// Outside both: the popover closes.
    Dismiss,
}

/// What to do with a raw press while the popover is open.
///
/// `mouse_held_outside` is [`CxFingers::is_mouse_held_outside`] asked with
/// this widget's chip area. It is the app-wide rule: a control that is dragged
/// continuously locks the pointer on its press, and every other gesture that
/// would start from the same press stands down until the release. Dismissing
/// is not such a gesture and is left alone by it — a press that lands nowhere
/// near the popover closes the popover whatever else is going on.
fn panel_press(inside_panel: bool, inside_chip: bool, mouse_held_outside: bool) -> PanelPress {
    if inside_panel {
        if mouse_held_outside {
            PanelPress::Swallow
        } else {
            PanelPress::Scrub
        }
    } else if inside_chip {
        PanelPress::Chip
    } else {
        PanelPress::Dismiss
    }
}

/// Popover geometry (layout points).
const PANEL_W: f64 = 150.0;
const PANEL_H: f64 = 30.0;
const PANEL_PAD: f64 = 12.0;
const PANEL_GAP: f64 = 4.0;
const TRACK_H: f64 = 6.0;
const KNOB_W: f64 = 10.0;

#[derive(Script, Widget)]
pub struct DropSlider {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_track: DrawColor,
    #[live]
    draw_fill: DrawColor,
    #[live]
    draw_knob: DrawColor,

    #[live(0.0)]
    pub min: f64,
    #[live(1.0)]
    pub max: f64,
    #[live(0.0)]
    pub default: f64,
    /// Readout = value × display_scale, at `precision` decimals + suffix.
    #[live(1.0)]
    pub display_scale: f64,
    #[live(0.0)]
    pub precision: f64,
    #[live]
    pub suffix: String,

    #[rust]
    value_init: bool,
    #[rust]
    value: f64,
    #[rust]
    open: bool,
    /// Held while the popover is open, so `Escape` belongs to it rather than to
    /// whatever it was opened in front of.
    #[rust]
    cancel_scope: Option<CancelScope>,
    #[rust]
    dragging: bool,
    /// The popover rect of the last draw (event-side hit tests use it).
    #[rust]
    panel_rect: Rect,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for DropSlider {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl DropSlider {
    /// Where the panel hangs: centred under the chip, PANEL_GAP below it.
    /// The request is unbounded on purpose — this panel never flipped or
    /// shifted, and giving it the pass would change where it lands. Both
    /// the draw and the event side read this one function.
    fn panel_rect(chip: Rect) -> Rect {
        place_overlay(&PlaceRequest {
            anchor: chip,
            size: dvec2(PANEL_W, PANEL_H),
            bounds: Rect::default(),
            gap: PANEL_GAP,
            placement: Placement::BOTTOM_CENTER,
            match_anchor_width: false,
        })
        .rect
    }

    fn readout(&self) -> String {
        format!(
            "{:.*}{}",
            self.precision.max(0.0) as usize,
            self.value * self.display_scale,
            self.suffix
        )
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        if self.dragging {
            return;
        }
        let value = value.clamp(self.min, self.max);
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            self.value_init = true;
            self.redraw_all(cx);
        }
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
    }

    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if self.open != open {
            self.open = open;
            if open {
                self.cancel_scope = Some(self.begin_cancel_scope(cx));
            } else if let Some(scope) = self.cancel_scope.take() {
                cx.end_cancel_scope(scope);
            }
            self.dragging = false;
            // The popover owns the pointer while it is up. The slider in it
            // is a control that is dragged continuously, and the app-wide
            // rule for those is that nothing else may be handed a hover, a
            // press or a focus from that pointer on the way. The panel is
            // drawn on an overlay list and has no area the hit system will
            // route to, so the lock cannot come from a capture: the SWEEP
            // LOCK is what stands in for it, and `hits()` answers Nothing to
            // every area but this widget's own for as long as it is held.
            // (The same idiom as `DropToggles`, whose popover is the same
            // shape.)
            //
            // This is the only place the lock is taken, and the only place it
            // is given back on purpose. A chip that never gets here again —
            // dropped, or hidden with the popover up — hands it over instead:
            // see the `Drop` below.
            if open {
                cx.sweep_lock(self.draw_bg.area());
            } else {
                cx.sweep_unlock(self.draw_bg.area());
            }
            self.draw_bg.set_uniform(cx, id!(open), &[if open { 1.0 } else { 0.0 }]);
            self.redraw_all(cx);
        }
    }

    fn drag_to(&mut self, cx: &mut Cx, uid: WidgetUid, x: f64) {
        let x0 = self.panel_rect.pos.x + PANEL_PAD;
        let w = (self.panel_rect.size.x - PANEL_PAD * 2.0).max(1.0);
        let fraction = ((x - x0) / w).clamp(0.0, 1.0);
        let value = self.min + fraction * (self.max - self.min);
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            cx.widget_action(uid, DropSliderAction::Changed(value));
            self.redraw_all(cx);
        }
    }
}

/// A chip dropped with its popover open — the page holding it swapped for
/// another, its row recycled by a list — cannot let go of the sweep lock
/// itself: a drop has no `Cx`. A lock nobody holds turns EVERY hit in the
/// application away, in every window, until the app is restarted, so the
/// area is left for the next event to release
/// (`release_orphaned_sweep_locks`, which the window runs as each event
/// reaches it). The same idiom as `LineMenu`, `PillNav` and `Modal`.
///
/// A chip merely HIDDEN keeps its lock a moment longer: nothing calls a
/// widget that is not drawn, so it has no moment of its own to notice. The
/// same sweep catches it — `release_stale_sweep_locks` lets go of every lock
/// whose area names nothing drawn any more, which is what the chip's area
/// becomes as soon as the list it was drawn into is drawn again without it.
impl Drop for DropSlider {
    fn drop(&mut self) {
        if self.open {
            orphan_sweep_locks(&[self.draw_bg.area()]);
        }
    }
}

impl Widget for DropSlider {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.value_init {
            self.value_init = true;
            self.value = self.default.clamp(self.min, self.max);
        }
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &self.readout());
        self.draw_bg.end(cx);

        if self.open {
            if let Some(draw_list) = self.draw_list.as_mut() {
                // The PROVEN popup idiom (PopupMenu): draw the panel as
                // turtle content at the overlay root, then SHIFT the whole
                // list to hang under the chip. (draw_abs into a bare
                // overlay list renders nothing — learned the hard way.)
                draw_list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                self.draw_panel.begin(
                    cx,
                    Walk::fixed(PANEL_W, PANEL_H),
                    Layout::default(),
                );
                let panel = cx.turtle().rect();
                let x0 = panel.pos.x + PANEL_PAD;
                let w = panel.size.x - PANEL_PAD * 2.0;
                let y = panel.pos.y + (panel.size.y - TRACK_H) * 0.5;
                self.draw_track.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0, y), size: dvec2(w, TRACK_H) },
                );
                let fraction = if self.max > self.min {
                    ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.draw_fill.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0, y), size: dvec2(w * fraction, TRACK_H) },
                );
                self.draw_knob.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(
                            x0 + (w - KNOB_W) * fraction,
                            y - (14.0 - TRACK_H) * 0.5,
                        ),
                        size: dvec2(KNOB_W, 14.0),
                    },
                );
                self.draw_panel.end(cx);
                let chip = self.draw_bg.area().rect(cx);
                cx.end_pass_sized_turtle_with_shift(
                    self.draw_bg.area(),
                    Self::panel_rect(chip).pos - chip.pos,
                );
                draw_list.end(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.open && crate::modal::ModalAction::is_dismissal(event) {
            self.set_open(cx, false);
            return;
        }
        if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
            && event.back_pressed()
        {
            self.set_open(cx, false);
            return;
        }
        // The popover owns the pointer while open: a press inside scrubs,
        // a press outside chip AND panel closes. The CHIP toggle itself
        // lives in the hits arm below — one press, one state change.
        if self.open {
            // The panel hangs under the chip deterministically.
            let chip = self.draw_bg.area().rect(cx);
            self.panel_rect = Self::panel_rect(chip);
            match event {
                Event::MouseDown(me) => {
                    // Raw presses walk past a sweep lock, so this widget
                    // both asks and answers for them itself.
                    match panel_press(
                        self.panel_rect.contains(me.abs),
                        chip.contains(me.abs),
                        cx.fingers.is_mouse_held_outside(&[self.draw_bg.area()]),
                    ) {
                        PanelPress::Scrub => {
                            self.dragging = true;
                            self.drag_to(cx, uid, me.abs.x);
                            me.handled.set(self.draw_bg.area());
                        }
                        // Inside the panel but somebody else holds the
                        // mouse: no scrub, and the press is still spent —
                        // the panel is over whatever is under it and must
                        // not be pressed through.
                        PanelPress::Swallow => {
                            me.handled.set(self.draw_bg.area());
                        }
                        PanelPress::Dismiss => {
                            self.set_open(cx, false);
                        }
                        PanelPress::Chip => {}
                    }
                }
                Event::MouseMove(me) => {
                    if self.dragging {
                        // Asked again on every move: a control can take the
                        // pointer after the scrub began, and the scrub hands
                        // it over rather than fighting for it.
                        if cx.fingers.is_mouse_held_outside(&[self.draw_bg.area()]) {
                            self.dragging = false;
                        } else {
                            self.drag_to(cx, uid, me.abs.x);
                        }
                    }
                }
                Event::MouseUp(_) => {
                    self.dragging = false;
                }
                // The host took the mouse press itself away (not a cancel of
                // some other capture of a press still held): the scrub ends
                // where it is.
                Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) => {
                    self.dragging = false;
                }
                Event::KeyDown(ke)
                    if ke.key_code == KeyCode::Escape
                        && self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s)) =>
                {
                    self.set_open(cx, false);
                }
                _ => {}
            }
        }
        // Named as its own sweep area, or the lock this widget takes while
        // its popover is open would turn the chip's own hits away along with
        // everybody else's.
        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[1.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[0.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.modifiers.shift => {
                // A chip with a marked default should have a way back to it
                // that is not aim: shift on the chip is it, and the popover
                // stays as it was so a hand can carry on from the default.
                let value = self.default.clamp(self.min, self.max);
                self.value_init = true;
                if (value - self.value).abs() > f64::EPSILON {
                    self.value = value;
                    cx.widget_action(uid, DropSliderAction::Changed(value));
                    self.redraw_all(cx);
                }
            }
            Hit::FingerDown(_) => {
                self.set_open(cx, !self.open);
            }
            _ => {}
        }
    }
}

impl DropSliderRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let DropSliderAction::Changed(v) =
            actions.find_widget_action(self.widget_uid())?.cast()
        {
            return Some(v);
        }
        None
    }

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;

    const CHIP: DVec2 = dvec2(90.0, 22.0);

    /// A chip drawn on its own, so its area names a real draw list and the
    /// lock it takes is a lock that stands.
    fn drawn_chip(cx: &mut Cx, list: &mut DrawList2d, pass: &DrawPass) -> DropSlider {
        cx.with_vm(crate::script_mod);
        let mut chip = cx.with_vm(DropSlider::script_new_with_default);
        pass.set_size(cx, CHIP);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(CHIP, Layout::flow_down());
        let _ = chip.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(CHIP.x, CHIP.y));
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(pass);
        chip
    }

    /// A scrub the host took away (`Event::FingerCancel` for the mouse)
    /// ends there: a later move with no press changes nothing.
    #[test]
    fn a_cancelled_scrub_ends_and_a_later_move_changes_nothing() {
        crate::on_test_cx(|| {
        let mut cx = crate::checkout_test_cx();
        let mut list = DrawList2d::new(&mut cx);
        let pass = DrawPass::new(&mut cx);
        let mut chip = drawn_chip(&mut cx, &mut list, &pass);
        chip.open = true;
        chip.dragging = true;
        let chip_rect = chip.draw_bg.area().rect(&cx);
        let panel = DropSlider::panel_rect(chip_rect);
        let mv = |x: f64| Event::MouseMove(MouseMoveEvent {
            abs: dvec2(x, panel.pos.y + panel.size.y * 0.5),
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.1,
            handled: std::cell::Cell::new(Area::Empty),
        });
        // Scrubbing moves the value (so the check below means something).
        chip.handle_event(&mut cx, &mv(panel.pos.x + 5.0), &mut Scope::empty());
        let before = chip.value();
        chip.handle_event(&mut cx, &mv(panel.pos.x + panel.size.x - 5.0), &mut Scope::empty());
        assert!(chip.value() != before, "the scrub did not move the value; the test proves nothing");
        // The host takes the mouse press itself away.
        cx.fingers.cancel_digit(live_id!(mouse).into());
        chip.handle_event(&mut cx, &Event::FingerCancel(crate::event::FingerCancelEvent {
            window_id: WindowId(1, 1),
            digit_id: live_id!(mouse).into(),
            device: DigitDevice::Mouse { button: MouseButton::PRIMARY },
            abs: dvec2(panel.pos.x, panel.pos.y),
            time: 0.2,
            modifiers: KeyModifiers::default(),
        }), &mut Scope::empty());
        assert!(!chip.dragging, "the scrub survived its cancel");
        let after_cancel = chip.value();
        chip.handle_event(&mut cx, &mv(panel.pos.x + 5.0), &mut Scope::empty());
        assert_eq!(chip.value(), after_cancel, "a move after the cancel still scrubbed");
        });
    }

    #[test]
    fn a_press_in_the_panel_scrubs_and_is_spent() {
        assert_eq!(panel_press(true, false, false), PanelPress::Scrub);
    }

    #[test]
    fn a_press_in_the_panel_while_another_control_holds_the_mouse_does_not_scrub() {
        // The control that took the pointer keeps it until the release. The
        // press is still the popover's, so it is swallowed rather than
        // pressed through the panel into whatever is underneath.
        assert_eq!(panel_press(true, false, true), PanelPress::Swallow);
    }

    #[test]
    fn a_press_on_the_chip_is_left_to_the_chips_own_hit() {
        assert_eq!(panel_press(false, true, false), PanelPress::Chip);
        assert_eq!(panel_press(false, true, true), PanelPress::Chip);
    }

    #[test]
    fn a_press_outside_both_closes_the_popover_whatever_holds_the_mouse() {
        // Dismissal is not a drag and does not stand down: a popover left up
        // while a control elsewhere is being dragged must still be closable.
        assert_eq!(panel_press(false, false, false), PanelPress::Dismiss);
        assert_eq!(panel_press(false, false, true), PanelPress::Dismiss);
    }

    /// A chip dropped with its popover open cannot unlock anything itself,
    /// so it hands the lock over: until somebody lets go of it every hit in
    /// the application answers Nothing, which is a dead window.
    #[test]
    fn a_chip_dropped_with_its_popover_open_leaves_its_lock_for_the_next_event() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let mut chip = drawn_chip(&mut cx, &mut list, &pass);
        chip.set_open(&mut cx, true);
        assert!(cx.sweep_lock_area().is_some(), "an open popover holds the pointer");

        drop(chip);
        // The list the chip was drawn into is still alive and still valid,
        // so nothing but the hand-over can account for the release.
        assert!(cx.sweep_lock_area().is_some(), "a drop has no Cx to let go with");
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the next event lets go of it");
    }

    /// And a chip dropped with nothing open hands over nothing: the sweep
    /// that follows must not take a lock somebody else is holding.
    #[test]
    fn a_chip_dropped_closed_hands_over_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let pass = DrawPass::new(&mut cx);
        // Each in its own draw list, so drawing the second does not make
        // the first one's area stale — the sweep would let go of a stale
        // lock for a reason this test is not about.
        let mut keeper_list = DrawList2d::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let keeper = drawn_chip(&mut cx, &mut keeper_list, &pass);
        let mut chip = drawn_chip(&mut cx, &mut list, &pass);
        chip.set_open(&mut cx, true);
        chip.set_open(&mut cx, false);
        let held = keeper.draw_bg.area();
        cx.sweep_lock(held);
        drop(chip);
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), Some(held), "somebody else's lock went with it");
    }

    #[test]
    fn the_panel_hangs_under_the_chip() {
        let chip = Rect { pos: dvec2(100.0, 40.0), size: dvec2(60.0, 22.0) };
        let panel = DropSlider::panel_rect(chip);
        assert_eq!(panel.size, dvec2(PANEL_W, PANEL_H));
        assert_eq!(panel.pos.y, chip.pos.y + chip.size.y + PANEL_GAP);
        // Centred on the chip, so the press rects the arms above test are
        // the rects the drawing put on screen.
        assert_eq!(
            panel.pos.x + panel.size.x * 0.5,
            chip.pos.x + chip.size.x * 0.5
        );
    }
}
