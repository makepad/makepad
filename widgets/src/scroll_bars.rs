//! ScrollBars — the scroll offset of a box, the two bars that show it, and
//! `auto_tail`: a box that stays at the bottom while content is still
//! arriving, and lets go the moment the reader scrolls up.
//!
//! **The release rule is the feature.** Pinning to the bottom is the easy
//! half, and every host has already hand-rolled it — `set_scroll_pos` with a
//! large number, retried for a few frames, because a `ViewRef` hands out a
//! scroll setter and nothing else: no getter, no totals, no at-the-end
//! predicate. What those versions never do is LET GO. They re-pin on every
//! arriving chunk, so a reader who scrolls up mid-stream is dragged back down
//! by the next line — worse than no follower at all. So the rule is stated
//! from the reader's side:
//!
//! - A reader-driven scroll — a wheel, a drag of a bar, a fling — releases
//!   the follower at once, and asks for the decision to be made again on the
//!   next frame.
//! - That frame takes hold again only if the reader is back at the end,
//!   within [`TAIL_SLACK`], or if the content now fits and there is no end to
//!   be away from. A scroll DOWN that lands at the bottom therefore re-pins;
//!   a scroll UP does not.
//! - Nothing else re-pins. Content arriving, or being trimmed away under a
//!   released reader until the clamp happens to leave them at the bottom, is
//!   not the reader coming back, and is never read as consent to follow
//!   again.
//!
//! A host move — [`ScrollBars::set_scroll_pos`], a focus jump — is neither: it
//! leaves the current state alone and re-opens the question for the next
//! frame, so it can neither be fought by the follower nor silently keep it
//! following the wrong place. [`ScrollBars::set_tailing`] is the deliberate
//! control, for a "jump to the newest" button.
//!
//! The arithmetic is exact here and nowhere else: [`ScrollBars::draw_scroll_bars`]
//! already holds this frame's real total and visible size from the turtle, so
//! the follower reads the end rather than guessing at it, and the blind
//! multi-frame retry disappears along with the frame counter. The decision
//! itself is [`tail_follow_y`], a free function over a [`ScrollExtent`] with
//! no `Cx` and no widget in it, so the rule above can be read — and tested —
//! as arithmetic.
//!
//! `auto_tail` means here exactly what it means on `PortalList`, which owns
//! the name: same word, same release rule, so a host that has met one has met
//! both. This is the flat-box version, for anything that draws its content in
//! one turtle instead of virtualising rows.

use crate::{event::TouchState, makepad_draw::*, scroll_bar::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ScrollBarsBase = #(ScrollBars::script_component(vm))

    mod.widgets.ScrollBarsTabs = mod.widgets.ScrollBarsBase {
        show_scroll_x: true
        show_scroll_y: true
        scroll_bar_x: mod.widgets.ScrollBarTabs {}
        scroll_bar_y: mod.widgets.ScrollBarTabs {}
    }

    mod.widgets.ScrollBars = set_type_default() do mod.widgets.ScrollBarsBase {
        show_scroll_x: true
        show_scroll_y: true
        scroll_bar_x: mod.widgets.ScrollBar {}
        scroll_bar_y: mod.widgets.ScrollBar {}
    }
}

#[derive(Script, ScriptHook)]
pub struct ScrollBars {
    #[live]
    show_scroll_x: bool,
    #[live]
    show_scroll_y: bool,
    #[live(false)]
    ignore_scroll_input: bool,
    /// Follow the bottom while content arrives, and let go when the reader
    /// scrolls up. Off by default, so a box that was never asked to follow
    /// behaves exactly as it did before. See the module doc for the rule.
    #[live(false)]
    auto_tail: bool,
    #[live]
    scroll_bar_x: ScrollBar,
    #[live]
    scroll_bar_y: ScrollBar,
    #[rust]
    nav_scroll_index: Option<NavScrollIndex>,
    /// The INVERSE of "following", so that a box applied with
    /// `auto_tail: true` starts out following with no hook to run: at birth
    /// the reader has not let go of anything.
    #[rust(false)]
    tail_released: bool,
    /// Something moved the box that was not the follower, so the next draw
    /// has to decide again from where it actually landed.
    #[rust(false)]
    tail_rearm: bool,
    #[rust]
    scroll: Vec2d,
    #[rust]
    area: Area,
}

pub enum ScrollBarsAction {
    ScrollX(f64),
    ScrollY(f64),
    None,
}

/// Where a scrolling box is and how far it can go, per axis, in the box's
/// own points. An axis that does not scroll reads zero on all three, so a
/// caller asking "is there more below" never has to know which axes the box
/// was built with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollExtent {
    /// How far the content has been moved under the box.
    pub pos: Vec2d,
    /// The size of everything the box holds.
    pub total: Vec2d,
    /// The size of the part the box shows.
    pub visible: Vec2d,
}

impl ScrollExtent {
    /// The furthest the offset can go on each axis, never negative: content
    /// smaller than its box cannot be scrolled at all.
    pub fn max(&self) -> Vec2d {
        dvec2(
            (self.total.x - self.visible.x).max(0.0),
            (self.total.y - self.visible.y).max(0.0),
        )
    }

    /// True when the vertical axis can scroll and is within `slack` of its
    /// end. Content that fits is never "at the end": there is no end to have
    /// reached, and a follower would otherwise light the last row on a short
    /// page before the reader got anywhere.
    pub fn at_end_y(&self, slack: f64) -> bool {
        self.total.y - self.visible.y > 0.5 && self.pos.y >= self.total.y - self.visible.y - slack
    }
}

/// How many points short of the end still count as the end. A momentum
/// stream or a smooth scroll can stop a hair short of the last pixel, and a
/// follower that demanded the exact maximum would let go there and never take
/// hold again. It is deliberately small: two points is rounding, not a line of
/// text, so a reader who scrolls up by anything they can see has let go.
pub const TAIL_SLACK: f64 = 2.0;

/// What the follower carries between frames: whether the box was asked to
/// follow at all, whether it is following right now, and whether something
/// moved it since the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TailState {
    /// The box's `auto_tail` setting.
    pub enabled: bool,
    /// Whether it is holding the end right now.
    pub pinned: bool,
    /// Whether `pinned` has to be decided again from where the box now is,
    /// rather than carried over. A reader-driven scroll sets this.
    pub rearm: bool,
}

/// What the follower wants done with the frame being drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TailDecision {
    /// Whether the box holds the end after this frame.
    pub pinned: bool,
    /// The vertical offset to move to, or `None` to leave the reader exactly
    /// where they put themselves.
    pub scroll_y: Option<f64>,
}

/// Decide, from this frame's real extent, whether the box follows the end and
/// where that puts it.
///
/// This is the whole of the rule in the module doc, and it is pure: the widget
/// around it only carries [`TailState`] across frames and applies the offset.
/// The two halves that hosts get wrong both live here — a chunk arriving never
/// changes `pinned` (only a reader does, through `rearm`), and `scroll_y` is
/// this frame's end rather than a large number retried until it sticks.
pub fn tail_follow_y(state: TailState, extent: ScrollExtent, slack: f64) -> TailDecision {
    if !state.enabled {
        return TailDecision {
            pinned: false,
            scroll_y: None,
        };
    }
    let pinned = if state.rearm {
        // The box moved under someone else's hand, so where it landed decides.
        // Content that fits has no end to be away from, and a box with nothing
        // to scroll has not been scrolled away from: keep the end, so the
        // first chunk that overflows is followed rather than stranding a
        // reader who never did anything.
        extent.max().y <= 0.5 || extent.at_end_y(slack)
    } else {
        // Nobody moved it. Content arriving, or being trimmed away, can
        // neither take hold of the end nor let go of it.
        state.pinned
    };
    TailDecision {
        pinned,
        // Holding the end means wherever this frame's total put it: the same
        // expression whether the content grew or shrank, so a trim walks the
        // follower back up instead of leaving it clamped against a blank.
        scroll_y: if pinned { Some(extent.max().y) } else { None },
    }
}

impl ScrollBars {
    /// Whether the box is following the end right now. False the moment the
    /// reader scrolls up, which is what a host puts its "jump to the newest"
    /// affordance behind.
    pub fn is_tailing(&self) -> bool {
        self.auto_tail && !self.tail_released
    }

    /// Take hold of the end again, or let go, with no reader involved — what a
    /// host's own "jump to the newest" button calls. Does nothing at all on a
    /// box that was not built to follow.
    pub fn set_tailing(&mut self, tailing: bool) {
        if self.auto_tail {
            self.tail_released = !tailing;
            self.tail_rearm = false;
        }
    }

    /// A reader moved the box themselves: let go now, so anything asking
    /// [`Self::is_tailing`] before the next frame is told the truth, and let
    /// that frame decide from where they land whether to take hold again.
    fn release_tail(&mut self) {
        if self.auto_tail {
            self.tail_released = true;
            self.tail_rearm = true;
        }
    }

    /// Something other than the follower and other than the reader moved the
    /// box — a host `set_scroll_pos`, a focus jump. Leave the state alone and
    /// re-open the question for the next frame.
    fn arm_tail(&mut self) {
        if self.auto_tail {
            self.tail_rearm = true;
        }
    }

    /// Run [`tail_follow_y`] against the extent the bars hold for the frame
    /// being drawn, and apply what it decides.
    ///
    /// This runs at the END of the box's draw, the first moment the real total
    /// is known, so a move made here shows up on the next frame — the content
    /// turtle for this one has already been walked at the old offset. That is
    /// why it asks for a redraw, and why it stays silent when the offset it
    /// wants is the one already in place: a follower holding a still stream
    /// must not spin frames.
    fn follow_tail(&mut self, cx: &mut Cx2d) {
        if !self.auto_tail {
            return;
        }
        let decision = tail_follow_y(
            TailState {
                enabled: true,
                pinned: !self.tail_released,
                rearm: self.tail_rearm,
            },
            self.extent(),
            TAIL_SLACK,
        );
        self.tail_rearm = false;
        self.tail_released = !decision.pinned;
        if let Some(y) = decision.scroll_y {
            if (y - self.scroll.y).abs() > 0.01 {
                // `no_action` on purpose: the follower moving the box is not a
                // scroll event, and must not come back around as one and
                // release itself.
                self.scroll_bar_y.set_scroll_pos_no_action(cx, y);
                let y = self.scroll_bar_y.get_scroll_pos();
                self.set_scroll_y(cx, y);
                self.redraw(cx);
            }
        }
    }

    pub fn set_scroll_x(&mut self, _cx: &mut Cx, value: f64) {
        self.scroll.x = value;
    }

    pub fn set_scroll_y(&mut self, _cx: &mut Cx, value: f64) {
        self.scroll.y = value;
    }

    pub fn get_scroll_pos(&self) -> Vec2d {
        self.scroll
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) -> Vec<ScrollBarsAction> {
        let mut actions = Vec::new();
        self.handle_main_event(cx, event, scope, &mut actions);
        self.handle_scroll_event(cx, event, scope, &mut actions);
        actions
    }

    pub fn handle_main_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope,
        actions: &mut Vec<ScrollBarsAction>,
    ) {
        if let Event::Trigger(te) = event {
            if let Some(triggers) = te.triggers.get(&self.area) {
                if let Some(trigger) = triggers.iter().find(|t| t.id == live_id!(scroll_focus_nav))
                {
                    let self_rect = self.area.rect(cx);
                    self.scroll_into_view(
                        cx,
                        trigger
                            .from
                            .rect(cx)
                            .translate(-self_rect.pos + self.scroll)
                            .add_margin(dvec2(5.0, 5.0)),
                    );
                }
            }
        }

        if self.show_scroll_x {
            let mut ret_x = None;
            self.scroll_bar_x
                .handle_event_with(cx, event, &mut |_cx, action| match action {
                    ScrollBarAction::Scroll { scroll_pos, .. } => {
                        ret_x = Some(scroll_pos);
                        actions.push(ScrollBarsAction::ScrollX(scroll_pos))
                    }
                    _ => (),
                });
            if let Some(x) = ret_x {
                self.scroll.x = x;
                self.redraw(cx);
            }
        }
        if self.show_scroll_y {
            let mut ret_y = None;
            self.scroll_bar_y
                .handle_event_with(cx, event, &mut |_cx, action| match action {
                    ScrollBarAction::Scroll { scroll_pos, .. } => {
                        ret_y = Some(scroll_pos);
                        actions.push(ScrollBarsAction::ScrollY(scroll_pos))
                    }
                    _ => (),
                });
            if let Some(y) = ret_y {
                self.scroll.y = y;
                // A drag of the bar is the reader moving the box: let go.
                self.release_tail();
                self.redraw(cx);
            }
        }
    }

    pub fn handle_scroll_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope,
        actions: &mut Vec<ScrollBarsAction>,
    ) {
        if !self.ignore_scroll_input {
            if self.show_scroll_x {
                let mut ret_x = None;
                self.scroll_bar_x
                    .handle_scroll_event(cx, event, self.area, &mut |_cx, action| match action {
                        ScrollBarAction::Scroll { scroll_pos, .. } => {
                            ret_x = Some(scroll_pos);
                            actions.push(ScrollBarsAction::ScrollX(scroll_pos))
                        }
                        _ => (),
                    });
                if let Some(x) = ret_x {
                    self.scroll.x = x;
                    self.redraw(cx);
                }
            }
            if self.show_scroll_y {
                let mut ret_y = None;
                self.scroll_bar_y
                    .handle_scroll_event(cx, event, self.area, &mut |_cx, action| match action {
                        ScrollBarAction::Scroll { scroll_pos, .. } => {
                            ret_y = Some(scroll_pos);
                            actions.push(ScrollBarsAction::ScrollY(scroll_pos))
                        }
                        _ => (),
                    });
                if let Some(y) = ret_y {
                    self.scroll.y = y;
                    // A wheel, a trackpad, the tail of a fling: the reader
                    // moving the box. Let go; the next draw re-pins only if
                    // this landed them back at the end.
                    self.release_tail();
                    self.redraw(cx);
                }
            }
        }
    }

    /// Stop an in-progress momentum fling on either bar when a press lands inside the
    /// scrollable content, the "press to catch the scroll" behavior that iOS, Android, and
    /// macOS all have. It applies on any tap, click, or touch, independent of `drag_scrolling`
    /// or finger count. Returns `true` if a fling was caught, in which case the containing view
    /// treats the press as consumed and does not forward it to children, so catching a runaway
    /// scroll never also activates a widget under the finger.
    ///
    /// It tests the raw press event against the content rect rather than `event.hits`, so it
    /// fires even when a child would otherwise capture THIS press, and it must run before the
    /// view dispatches the event to its children. A pointer another control is ALREADY holding
    /// is the one exception (see the body): that press is not the reader reaching for the
    /// brake, it belongs to whatever the pointer is locked to.
    pub fn catch_fling_on_press(&mut self, cx: &mut Cx, event: &Event) -> bool {
        // The pointer-capture rule's other half. Catching a fling CONSUMES the
        // press, so while another control holds the mouse this must stand
        // down: a press that arrives with the pointer already locked to a
        // slider belongs to that slider, and the box would otherwise take a
        // press-like state from a pointer it does not own. The box's own areas
        // — the content and both handles — are not "outside", so a press on a
        // handle still catches the fling the reader is reaching for, and a
        // press during the box's own drag still counts.
        //
        // Touch captures are ignored by `is_mouse_held_outside`, so a finger
        // may still catch a fling through a control, as it may still drag the
        // content under one.
        if matches!(event, Event::MouseDown(_))
            && cx.fingers.is_mouse_held_outside(&[
                self.area,
                self.scroll_bar_x.area(),
                self.scroll_bar_y.area(),
            ])
        {
            return false;
        }
        let area_rect = self.area.rect(cx);
        // The press time also gates the coast check below, so a momentum stream that
        // silently stopped reaching this view can't leave it eating presses.
        let press_time = match event {
            Event::MouseDown(e) if area_rect.contains(e.abs) => e.time,
            Event::TouchUpdate(e)
                if e.touches
                    .iter()
                    .any(|t| matches!(t.state, TouchState::Start) && area_rect.contains(t.abs)) =>
            {
                e.time
            }
            _ => return false,
        };
        // Use `|` so both bars' catch markers are consumed by this one press.
        let press_catch = (self.show_scroll_x && self.scroll_bar_x.take_press_catch(press_time))
            | (self.show_scroll_y && self.scroll_bar_y.take_press_catch(press_time));
        let flinging = press_catch
            || (self.show_scroll_x && self.scroll_bar_x.is_motion_live(press_time))
            || (self.show_scroll_y && self.scroll_bar_y.is_motion_live(press_time));
        if !flinging {
            return false;
        }
        // Stop both axes: a single press catches a 2D fling on both bars. The press
        // is consumed in every motion case, even ones with nothing to stop (the
        // bounce spring keeps settling on its own).
        if self.show_scroll_x {
            self.scroll_bar_x.stop_fling(press_time);
        }
        if self.show_scroll_y {
            self.scroll_bar_y.stop_fling(press_time);
        }
        true
    }

    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: Vec2d) -> bool {
        //let view_area = Area::DrawList(DrawListArea{draw_list_id:draw_list_id, redraw_id:cx.redraw_id});
        let mut changed = false;
        if self.show_scroll_x {
            if self.scroll_bar_x.set_scroll_pos(cx, pos.x) {
                changed = true;
            }
            let scroll_pos = self.scroll_bar_x.get_scroll_pos();
            self.set_scroll_x(cx, scroll_pos);
        }
        if self.show_scroll_y {
            if self.scroll_bar_y.set_scroll_pos(cx, pos.y) {
                changed = true;
            }
            let scroll_pos = self.scroll_bar_y.get_scroll_pos();
            self.set_scroll_y(cx, scroll_pos);
        }
        self.arm_tail();
        changed
    }

    pub fn set_scroll_pos_no_clip(&mut self, cx: &mut Cx, pos: Vec2d) -> bool {
        let mut changed = false;
        if self.show_scroll_x {
            if self.scroll_bar_x.set_scroll_pos_no_clip(cx, pos.x) {
                changed = true;
            }
            self.set_scroll_x(cx, pos.x);
        }
        if self.show_scroll_y {
            if self.scroll_bar_y.set_scroll_pos_no_clip(cx, pos.y) {
                changed = true;
            }
            self.set_scroll_y(cx, pos.y);
        }
        self.arm_tail();
        changed
    }

    /// Read straight from the two bars. `get_scroll_view_total` and
    /// `get_scroll_view_visible` take `&mut self` for no reason, and this is
    /// read from places that only hold `&self`: a snapshot, a follower
    /// polling the page it tracks.
    pub fn extent(&self) -> ScrollExtent {
        let axis = |on: bool, bar: &ScrollBar, pos: f64| {
            if on {
                (pos, bar.get_scroll_view_total(), bar.get_scroll_view_visible())
            } else {
                (0.0, 0.0, 0.0)
            }
        };
        let (pos_x, total_x, visible_x) = axis(self.show_scroll_x, &self.scroll_bar_x, self.scroll.x);
        let (pos_y, total_y, visible_y) = axis(self.show_scroll_y, &self.scroll_bar_y, self.scroll.y);
        ScrollExtent {
            pos: dvec2(pos_x, pos_y),
            total: dvec2(total_x, total_y),
            visible: dvec2(visible_x, visible_y),
        }
    }

    pub fn get_scroll_view_total(&mut self) -> Vec2d {
        Vec2d {
            x: if self.show_scroll_x {
                self.scroll_bar_x.get_scroll_view_total()
            } else {
                0.
            },
            y: if self.show_scroll_y {
                self.scroll_bar_y.get_scroll_view_total()
            } else {
                0.
            },
        }
    }

    pub fn get_scroll_view_visible(&mut self) -> Vec2d {
        Vec2d {
            x: if self.show_scroll_x {
                self.scroll_bar_x.get_scroll_view_visible()
            } else {
                0.
            },
            y: if self.show_scroll_y {
                self.scroll_bar_y.get_scroll_view_visible()
            } else {
                0.
            },
        }
    }

    pub fn get_viewport_rect(&mut self, _cx: &mut Cx) -> Rect {
        let pos = self.get_scroll_pos();
        let size = self.get_scroll_view_visible();
        Rect { pos, size }
    }

    pub fn scroll_into_view(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_scroll_x {
            self.scroll_bar_x
                .scroll_into_view(cx, rect.pos.x, rect.size.x, true);
        }
        if self.show_scroll_y {
            self.scroll_bar_y
                .scroll_into_view(cx, rect.pos.y, rect.size.y, true);
        }
        self.arm_tail();
    }

    pub fn scroll_into_view_no_smooth(&mut self, cx: &mut Cx, rect: Rect) {
        if self.show_scroll_x {
            self.scroll_bar_x
                .scroll_into_view(cx, rect.pos.x, rect.size.x, false);
        }
        if self.show_scroll_y {
            self.scroll_bar_y
                .scroll_into_view(cx, rect.pos.y, rect.size.y, false);
        }
        self.arm_tail();
    }

    pub fn scroll_into_view_abs(&mut self, cx: &mut Cx, rect: Rect) {
        let self_rect = self.area.rect(cx);
        if self.show_scroll_x {
            self.scroll_bar_x
                .scroll_into_view(cx, rect.pos.x - self_rect.pos.x, rect.size.x, true);
        }
        if self.show_scroll_y {
            self.scroll_bar_y
                .scroll_into_view(cx, rect.pos.y - self_rect.pos.y, rect.size.y, true);
        }
        self.arm_tail();
    }

    pub fn set_scroll_target(&mut self, cx: &mut Cx, pos: Vec2d) {
        if self.show_scroll_x {
            self.scroll_bar_x.set_scroll_target(cx, pos.x);
        }
        if self.show_scroll_y {
            self.scroll_bar_y.set_scroll_target(cx, pos.y);
        }
        self.arm_tail();
    }

    // all in one scrollbar api

    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk, layout: Layout) {
        cx.begin_turtle(walk, layout.with_scroll(self.scroll));
        self.begin_nav_area(cx);
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_scroll_bars(cx);
        // this needs to be a rect_area
        cx.end_turtle_with_area(&mut self.area);
        self.end_nav_area(cx);
    }

    pub fn end_with_shift(&mut self, cx: &mut Cx2d) {
        self.draw_scroll_bars(cx);
        // this needs to be a rect_area
        cx.end_turtle_with_area(&mut self.area);
        self.end_nav_area(cx);
    }
    // separate API

    pub fn begin_nav_area(&mut self, cx: &mut Cx2d) {
        self.nav_scroll_index = Some(cx.add_begin_scroll());
    }

    pub fn end_nav_area(&mut self, cx: &mut Cx2d) {
        if !self.area.is_valid(cx) {
            error!("Call set area before end_nav_area");
            return;
        }
        cx.add_end_scroll(self.nav_scroll_index.take().unwrap(), self.area);
    }

    pub fn draw_scroll_bars(&mut self, cx: &mut Cx2d) {
        // lets ask the turtle our actual bounds
        let view_total = cx.turtle().used();
        let mut rect_now = cx.turtle().rect();

        // The turtle's rect might be `NaN` if either size dimension is `Fit`,
        // so we look at each dimension's max value to ensure that it can be scrollable.
        if rect_now.size.y.is_nan() {
            rect_now.size.y = cx
                .current_turtle_max_height()
                .map_or(view_total.y, |max| view_total.y.min(max));
        }
        if rect_now.size.x.is_nan() {
            rect_now.size.x = cx
                .current_turtle_max_width()
                .map_or(view_total.x, |max| view_total.x.min(max));
        }

        if self.show_scroll_x {
            let scroll_pos =
                self.scroll_bar_x
                    .draw_scroll_bar(cx, ScrollAxis::Horizontal, rect_now, view_total);
            self.set_scroll_x(cx, scroll_pos);
        }
        if self.show_scroll_y {
            let scroll_pos =
                self.scroll_bar_y
                    .draw_scroll_bar(cx, ScrollAxis::Vertical, rect_now, view_total);
            self.set_scroll_y(cx, scroll_pos);
            // The bar has just been handed this frame's real total and visible
            // size, so from here the end is a number rather than a guess.
            self.follow_tail(cx);
        }
    }

    pub fn set_area(&mut self, area: Area) {
        self.area = area;
    }

    pub fn area(&self) -> Area {
        self.area
    }

    /// The two bar handles this box owns, for the pointer-capture rule.
    ///
    /// A bar captures the mouse on its press exactly like any other
    /// continuously dragged control, so a host that asked
    /// [`CxFingers::is_mouse_held_outside`] with its content area alone would
    /// read its own bar as an outsider holding the pointer and stand down
    /// against itself. Hosts name these alongside their own area; see
    /// [`ScrollBar::area`] for the same note one level down.
    pub fn bar_areas(&self) -> [Area; 2] {
        [self.scroll_bar_x.area(), self.scroll_bar_y.area()]
    }

    pub fn redraw(&self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

#[cfg(test)]
mod style_reapply_tests {
    use super::*;
    use crate::desktop_style::{install, DesktopStyle, StyleSheet};

    /// Scroll offsets are runtime (`#[rust]`) state on both the bar pair and
    /// each bar; re-walking the template on a style change must not move them.
    #[test]
    fn style_reapply_preserves_scroll_offset() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let original = crate::script_eval!(vm, {use mod.widgets.* ScrollBars{}});
            let mut bars = ScrollBars::script_from_value(vm, original);
            vm.with_cx_mut(|cx| {
                assert!(bars.set_scroll_pos_no_clip(cx, dvec2(0.0, 120.0)));
            });
            assert_eq!(bars.get_scroll_pos(), dvec2(0.0, 120.0));
            for (style, dark) in [(DesktopStyle::NextStep, false), (DesktopStyle::Windows, true), (DesktopStyle::Omarchy, false)] {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.with_reload(crate::script_mod);
                bars.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), original);
                assert_eq!(bars.get_scroll_pos(), dvec2(0.0, 120.0), "{}", style.id());
                assert_eq!(bars.scroll_bar_y.get_scroll_pos(), 120.0, "{}", style.id());
                assert_eq!(bars.scroll_bar_x.get_scroll_pos(), 0.0, "{}", style.id());
            }
        });
    }
}

#[cfg(test)]
mod extent_tests {
    use super::*;

    /// The extent is what the bars hold, and an axis the box was built
    /// without reads zero rather than whatever its idle bar last kept.
    #[test]
    fn an_extent_reads_what_the_bars_hold() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {use mod.widgets.* ScrollBars{show_scroll_x: false}});
            let mut bars = ScrollBars::script_from_value(vm, value);
            vm.with_cx_mut(|cx| {
                bars.set_scroll_pos_no_clip(cx, dvec2(0.0, 120.0));
                bars.scroll_bar_y.set_scroll_view_total(cx, 900.0);
                // A hidden axis is never read, whatever its bar was told.
                bars.scroll_bar_x.set_scroll_view_total(cx, 700.0);
                bars.set_scroll_x(cx, 40.0);
            });
            let extent = bars.extent();
            assert_eq!(extent.pos, dvec2(0.0, 120.0));
            assert_eq!(extent.total, dvec2(0.0, 900.0));
            assert_eq!(extent.visible.x, 0.0, "the hidden axis reads zero");
        });

        let extent = |pos: f64, total: f64, visible: f64| ScrollExtent {
            pos: dvec2(0.0, pos),
            total: dvec2(0.0, total),
            visible: dvec2(0.0, visible),
        };
        let fits = extent(0.0, 300.0, 400.0);
        assert!(!fits.at_end_y(8.0), "content that fits has no end to reach");
        assert_eq!(fits.max(), dvec2(0.0, 0.0), "and nowhere to scroll to");
        let middle = extent(250.0, 1000.0, 400.0);
        assert!(!middle.at_end_y(8.0));
        assert_eq!(middle.max(), dvec2(0.0, 600.0));
        assert!(extent(600.0, 1000.0, 400.0).at_end_y(0.0), "at the end");
        assert!(extent(594.0, 1000.0, 400.0).at_end_y(8.0), "within the slack");
        assert!(!extent(590.0, 1000.0, 400.0).at_end_y(8.0), "short of the slack");
    }
}

#[cfg(test)]
mod tail_tests {
    use super::*;

    /// A box the reader is not touching, at `pos` in `total` points of
    /// content, showing `visible` of them.
    fn extent(pos: f64, total: f64, visible: f64) -> ScrollExtent {
        ScrollExtent {
            pos: dvec2(0.0, pos),
            total: dvec2(0.0, total),
            visible: dvec2(0.0, visible),
        }
    }

    fn holding(pinned: bool) -> TailState {
        TailState {
            enabled: true,
            pinned,
            rearm: false,
        }
    }

    /// The reader just moved the box, whatever it was holding before.
    fn moved(pinned: bool) -> TailState {
        TailState {
            enabled: true,
            pinned,
            rearm: true,
        }
    }

    /// Content arriving under a reader who is at the end moves the offset to
    /// the new end, every frame, with no retry and no guessed large number.
    #[test]
    fn tail_holds_the_end_while_content_arrives() {
        let visible = 400.0;
        let decision = tail_follow_y(holding(true), extent(600.0, 1000.0, visible), TAIL_SLACK);
        assert!(decision.pinned);
        assert_eq!(decision.scroll_y, Some(600.0), "the end of 1000 in a 400 box");

        // Three chunks land with nobody touching the box.
        let mut pos = 600.0;
        for (total, end) in [(1200.0, 800.0), (1600.0, 1200.0), (4000.0, 3600.0)] {
            let decision = tail_follow_y(holding(true), extent(pos, total, visible), TAIL_SLACK);
            assert!(decision.pinned, "an arriving chunk is not a reader letting go");
            assert_eq!(decision.scroll_y, Some(end), "total {total}");
            pos = decision.scroll_y.unwrap();
        }
    }

    /// The essential half: an upward scroll lets go, and the chunks that keep
    /// arriving afterwards leave the reader exactly where they put themselves.
    #[test]
    fn an_upward_scroll_releases_the_tail() {
        let visible = 400.0;
        // Holding the end of 4000 points, the reader wheels up 300.
        let decision = tail_follow_y(moved(true), extent(3300.0, 4000.0, visible), TAIL_SLACK);
        assert!(!decision.pinned, "scrolled up, so no longer following");
        assert_eq!(decision.scroll_y, None, "and not moved back down");

        // 5000 more points arrive while they read. Nothing drags them down.
        for total in [4600.0, 6200.0, 9000.0] {
            let decision =
                tail_follow_y(holding(false), extent(3300.0, total, visible), TAIL_SLACK);
            assert!(!decision.pinned, "total {total}");
            assert_eq!(decision.scroll_y, None, "total {total}");
        }
    }

    /// Scrolling back down to the end is the reader asking to follow again —
    /// the same deliberate move that let go, in the other direction.
    #[test]
    fn scrolling_back_to_the_end_re_pins_the_tail() {
        let visible = 400.0;
        // 9000 points of content, so the end is at 8600.
        let decision = tail_follow_y(moved(false), extent(8600.0, 9000.0, visible), TAIL_SLACK);
        assert!(decision.pinned, "back at the end, so following again");
        assert_eq!(decision.scroll_y, Some(8600.0));

        // A fling that stops a point short of the end still counts as the end.
        let decision = tail_follow_y(moved(false), extent(8599.0, 9000.0, visible), TAIL_SLACK);
        assert!(decision.pinned, "within the slack");
        assert_eq!(decision.scroll_y, Some(8600.0), "and snapped flush to it");

        // One line short of it is a reader reading that line, not the end.
        let decision = tail_follow_y(moved(false), extent(8580.0, 9000.0, visible), TAIL_SLACK);
        assert!(!decision.pinned, "20 points short is a place, not rounding");
        assert_eq!(decision.scroll_y, None);
    }

    /// A stream that trims its own backlog shortens the content under a reader
    /// who let go, and the clamp can leave them sitting exactly at the new end.
    /// That is the content moving, not the reader coming back, and it must not
    /// be read as consent to follow again.
    #[test]
    fn a_content_shrink_never_fights_the_reader() {
        let visible = 400.0;
        // Released at 3300 of 9000. The backlog is trimmed to 3700 points, so
        // the end is now 3300: the reader is at it without having moved.
        let decision = tail_follow_y(holding(false), extent(3300.0, 3700.0, visible), TAIL_SLACK);
        assert!(!decision.pinned, "a trim is not the reader scrolling back down");
        assert_eq!(decision.scroll_y, None);

        // Trimmed further, past them: the bar clamps the offset, and the
        // follower still says nothing.
        let decision = tail_follow_y(holding(false), extent(2600.0, 3000.0, visible), TAIL_SLACK);
        assert!(!decision.pinned);
        assert_eq!(decision.scroll_y, None);

        // A box that IS following walks back up with the trim, rather than
        // staying clamped against content that is no longer there.
        let decision = tail_follow_y(holding(true), extent(8600.0, 3700.0, visible), TAIL_SLACK);
        assert!(decision.pinned);
        assert_eq!(decision.scroll_y, Some(3300.0), "the new end, not the old one");
    }

    /// Content that fits has no end to be away from, so a reader who scrolled
    /// a box with nothing to scroll has not let go of anything — otherwise a
    /// pane that starts empty would be released before its first chunk.
    #[test]
    fn content_that_fits_has_no_end_to_be_away_from() {
        let decision = tail_follow_y(moved(true), extent(0.0, 300.0, 400.0), TAIL_SLACK);
        assert!(decision.pinned);
        assert_eq!(decision.scroll_y, Some(0.0), "nowhere to scroll to");

        // And the first chunk that overflows is followed.
        let decision = tail_follow_y(holding(true), extent(0.0, 900.0, 400.0), TAIL_SLACK);
        assert_eq!(decision.scroll_y, Some(500.0));
    }

    /// A box that was never asked to follow is never moved, whatever state it
    /// is handed — the default costs existing users nothing.
    #[test]
    fn a_box_that_was_not_asked_to_follow_never_moves() {
        for state in [
            TailState::default(),
            TailState {
                enabled: false,
                pinned: true,
                rearm: false,
            },
            TailState {
                enabled: false,
                pinned: true,
                rearm: true,
            },
        ] {
            let decision = tail_follow_y(state, extent(0.0, 9000.0, 400.0), TAIL_SLACK);
            assert!(!decision.pinned, "{state:?}");
            assert_eq!(decision.scroll_y, None, "{state:?}");
        }
    }

    /// The live field: off unless asked for, and a box asked for it starts out
    /// following, so the first chunk it is ever given is already at the bottom.
    #[test]
    fn auto_tail_is_off_unless_asked_for() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let plain = crate::script_eval!(vm, {use mod.widgets.* ScrollBars{}});
            let bars = ScrollBars::script_from_value(vm, plain);
            assert!(!bars.auto_tail, "off by default");
            assert!(!bars.is_tailing());

            let asked = crate::script_eval!(vm, {use mod.widgets.* ScrollBars{auto_tail: true}});
            let mut bars = ScrollBars::script_from_value(vm, asked);
            assert!(bars.auto_tail);
            assert!(bars.is_tailing(), "at birth the reader has not let go");

            bars.release_tail();
            assert!(!bars.is_tailing(), "a reader scroll lets go at once");
            bars.set_tailing(true);
            assert!(bars.is_tailing(), "and the host's own button takes hold again");
        });
    }
}
