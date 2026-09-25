//! HamburgerMenu — the site navigation of a narrow window.
//!
//! A row of destinations shows inline while there is room. Below a width it
//! collapses into a burger button that opens the same destinations, either
//! in a side drawer or in a panel dropped under the button, and the bars turn
//! into a cross while the destinations are out. Choosing one, Escape and a
//! press outside all put them away.
//!
//! Every part already exists: the burger button, the nav list, the drawer
//! and the popover. What does not is the wiring each app would otherwise
//! write by hand: one model feeding three lists, a pick in any of them
//! lighting the other two, the burger following two different surfaces'
//! open and close, and a responsive switch that does not flicker when the
//! window rests on its breakpoint.
//!
//! **The three views are always built and only shown or hidden.** A variant
//! rebuilt from a template at every crossing of the breakpoint would throw
//! away the lit destination and an open panel each time the window was
//! dragged across the line, which is exactly when someone is looking.
//!
//! **Arrows look, a press chooses.** A nav list chooses as its arrows move,
//! so closing on every choice would put the panel away on the first arrow
//! and nobody could look down the list. What closes it is where the choice
//! came from, and that is only visible in the raw events, before the lists
//! turn them into a choice.
//!
//! **The menu moves the panel, not the surface.** A drawer slides on a curve
//! of its own and goes the moment it closes, and a popover does not move at
//! all, so neither could take the theme's easings or leave the way it came.
//! Each surface places a holder that rests where the panel belongs, and that
//! is what the surface hit-tests and hands the keyboard to. The panel inside
//! it moves along `enter_ease` over `enter_secs`. Once the surface has let
//! go, the menu draws the same panel on an overlay of its own along
//! `exit_ease` over `exit_secs`, where it takes no event, so the press that
//! follows a close lands where it was aimed. The bars turn on the same clock
//! and the same curve.
//!
//! **A press lands where the rows rest.** The panel is always laid out and
//! drawn where it rests, on a list of its own, and only that list's view
//! transform carries the pixels to where the travel has them: sliding in
//! from the edge, stretching past it on a spring, growing away from the
//! button. A hit test reads the rows' own rects, which the transform leaves
//! alone, so a press made while the panel is still coming out lands on the
//! row that will rest under it. Nothing clips the growing panel either, so
//! its shadow is whole.
//!
//! **The keyboard goes back to the burger.** A panel brought out from the
//! burger, by a press or by Return or Space on it, gives the keyboard back
//! to the burger when it goes away, so Return opens it again.

use crate::{
    animator::{Animate, Ease},
    button::ButtonWidgetRefExt,
    dialog::{Dialog, DialogSize, DialogWidgetRefExt, PanelEdge},
    event::TouchState,
    makepad_derive_widget::*,
    makepad_draw::*,
    nav_list::{Destination, NavListWidgetRefExt},
    overlay_place::Side,
    popover::{Popover, PopoverTrigger, PopoverWidgetRefExt},
    view::*,
    widget::*,
};

/// Where the collapsed destinations go.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum HamburgerSurface {
    /// A drawer from the side, over a scrim: for a window with no room to
    /// spare beside the work.
    #[pick]
    Drawer = 0,
    /// A panel dropped under the button: for a window too wide to want a
    /// drawer but too narrow for the row.
    Drop = 1,
}

/// Whether the destinations fold behind the button.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum HamburgerMode {
    /// Folded below `breakpoint`, in a row above it.
    #[pick]
    Responsive = 0,
    /// Always behind the button.
    Collapsed = 1,
    /// Always in a row.
    Inline = 2,
}

/// Which width a responsive menu compares with its breakpoint.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum HamburgerMeasure {
    /// The window's: right anywhere, including a bar slot that sizes itself
    /// and so has no room of its own to report.
    #[pick]
    Window = 0,
    /// The room the parent leaves the menu where it stands.
    Parent = 1,
}

/// Where a choice in an open panel came from, which decides whether the
/// panel goes away after it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PickSource {
    /// A press and release on the list.
    #[default]
    Pointer,
    /// An arrow, Home or End: the list chooses as these move, to let the
    /// reader look.
    Arrow,
    /// Return or Space on the list.
    Confirm,
}

/// What a hamburger menu reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum HamburgerAction {
    /// A destination was chosen, in whichever of the three lists; once.
    Selected(LiveId),
    /// The destinations came out, on either surface.
    Opened,
    /// They went away, whichever way: a pick, Escape, a press outside, the
    /// menu unfolding under an open panel, or the API.
    Closed,
    /// The menu folded (`true`) or unfolded (`false`).
    ModeChanged(bool),
    #[default]
    None,
}

/// The drawer's own slide. The menu moves the panel itself, so the drawer is
/// given the shortest slide it floors its own at, and what it slides is only
/// the holder the panel rests in: without this the hit test for its scrim
/// would trail the panel by the drawer's own curve.
const DRAWER_SLIDE_FLOOR_SECS: f64 = 0.01;

/// Whether a menu is folded behind its button at `width`.
///
/// A responsive menu that is already folded only unfolds past
/// `breakpoint + hysteresis`. A window resting on the breakpoint drifts a
/// point either side of it while it is being resized, and one line would
/// swap the row and the button on every one of those points.
pub fn next_collapsed(
    mode: HamburgerMode,
    was_collapsed: bool,
    width: f64,
    breakpoint: f64,
    hysteresis: f64,
) -> bool {
    match mode {
        HamburgerMode::Collapsed => true,
        HamburgerMode::Inline => false,
        HamburgerMode::Responsive => {
            // A negative band would unfold the menu before the width that
            // folded it, which is the flicker the band is there to stop.
            let band = hysteresis.max(0.0);
            if was_collapsed {
                width < breakpoint + band
            } else {
                width < breakpoint
            }
        }
    }
}

/// What opens the popover around the button.
///
/// The drop panel lets the popover open itself on the press, because that
/// is what gives it its focus trap, its pointer lock and its outside press.
/// Under a drawer the popover must never open, so it is only the API's.
pub fn popover_trigger_for(surface: HamburgerSurface) -> PopoverTrigger {
    match surface {
        HamburgerSurface::Drop => PopoverTrigger::Click,
        HamburgerSurface::Drawer => PopoverTrigger::Manual,
    }
}

/// Whether a pick from `source` puts the open panel away.
pub fn closes_on(close_on_pick: bool, source: PickSource) -> bool {
    close_on_pick && source != PickSource::Arrow
}

/// How far out a panel stands `elapsed` seconds into a travel: 0 away, 1 in
/// place.
///
/// It starts at `from`, which is wherever a travel it cut short had got to,
/// so a panel called back on its way out turns round where it is instead of
/// jumping. The spring passes `to` and comes back; that is left unclamped,
/// because the overshoot is the curve.
pub fn panel_travel(from: f64, to: f64, elapsed: f64, secs: f64, ease: &Ease, reduced: bool) -> f64 {
    // `!(secs > 0.0)` also catches a NaN a control might write.
    if reduced || !(secs > 0.0) || elapsed >= secs {
        return to;
    }
    from + (to - from) * ease.map((elapsed / secs).max(0.0))
}

/// The edge a menu's drawer comes from. `PanelEdge.Center` is the card in
/// the middle of the window, which is not a drawer at all, so a menu given
/// it keeps to the left, where a menu's drawer is looked for.
fn drawer_edge(side: PanelEdge) -> PanelEdge {
    if side.is_edge() {
        side
    } else {
        PanelEdge::Left
    }
}

/// The drawer's length along its axis in a pass of `pass`, as the drawer
/// works it out for the holder it places.
fn drawer_extent(side: PanelEdge, size: DialogSize, pass: DVec2) -> f64 {
    size.extent(side).unwrap_or(if side.is_column() { pass.x } else { pass.y })
}

/// Where the drawer's panel is drawn when it stands `v` of the way in, for a
/// drawer `extent` long in a pass of `pass`.
///
/// Short of its place the panel keeps its size and stands that far out past
/// its edge. Past its place, on an overshoot, it does not come off the edge,
/// which would open a gap along the window with the page showing through:
/// it stretches instead, its outer side still on the edge.
pub fn drawer_panel_rect(side: PanelEdge, extent: f64, pass: DVec2, v: f64) -> Rect {
    let along = extent * v.max(1.0);
    let short = extent * (1.0 - v.min(1.0));
    match side {
        PanelEdge::Left | PanelEdge::Center => Rect { pos: dvec2(-short, 0.0), size: dvec2(along, pass.y) },
        PanelEdge::Right => Rect { pos: dvec2(pass.x - along + short, 0.0), size: dvec2(along, pass.y) },
        PanelEdge::Top => Rect { pos: dvec2(0.0, -short), size: dvec2(pass.x, along) },
        PanelEdge::Bottom => Rect { pos: dvec2(0.0, pass.y - along + short), size: dvec2(pass.x, along) },
    }
}

/// The drop panel's offset in its holder and its height when it stands `v`
/// of the way out, for a panel `rest` points tall.
///
/// It grows away from the button: down from its top while it hangs below,
/// up from its bottom once the popover has flipped it above.
pub fn drop_panel_extent(rest: f64, v: f64, above: bool) -> (f64, f64) {
    let height = (rest * v).max(0.0);
    let offset = if above { rest - height } else { 0.0 };
    (offset, height)
}

/// The smallest scale a panel is drawn at: see `rest_to_drawn`.
const MIN_SCALE: f64 = 1e-3;

/// The view transform that carries a panel laid out at `rest` onto `drawn`:
/// scaled along each axis by how much bigger it is drawn, and moved so the
/// resting corner lands on the drawn one.
///
/// One formula for every way the panel travels. A drawer short of its place
/// is only moved; one stretching past it on a spring is scaled from the edge
/// it stays on; a drop panel growing from the button is scaled from the edge
/// by the button. Nothing is ever laid out at the drawn rect, so a hit test,
/// which reads the rects laid out, sees the panel where it rests.
///
/// A panel drawn to nothing is drawn a thousandth of its size instead: the
/// text shaders work from the transform, and a zero in it is a division by
/// zero there.
pub fn rest_to_drawn(rest: Rect, drawn: Rect) -> Mat4f {
    let scale = |drawn: f64, rest: f64| if rest > 0.0 { (drawn / rest).max(MIN_SCALE) } else { 1.0 };
    let sx = scale(drawn.size.x, rest.size.x);
    let sy = scale(drawn.size.y, rest.size.y);
    let mut m = Mat4f::identity();
    m.v[0] = sx as f32;
    m.v[5] = sy as f32;
    m.v[12] = (drawn.pos.x - rest.pos.x * sx) as f32;
    m.v[13] = (drawn.pos.y - rest.pos.y * sy) as f32;
    m
}

/// A walk that puts a part at `rect` in the pass, whatever turtle draws it.
fn walk_at(rect: Rect) -> Walk {
    Walk {
        abs_pos: Some(rect.pos),
        width: Size::Fixed(rect.size.x),
        height: Size::Fixed(rect.size.y),
        ..Walk::default()
    }
}

/// A panel on its way in or out.
#[derive(Clone, Copy, Debug)]
struct Travel {
    surface: HamburgerSurface,
    from: f64,
    to: f64,
    /// On the clock `Cx::seconds_since_app_start` reads, so a late frame
    /// lands where it belongs rather than where the last one stopped.
    started: f64,
    secs: f64,
    ease: Ease,
}

impl Travel {
    fn at(&self, now: f64) -> f64 {
        panel_travel(self.from, self.to, now - self.started, self.secs, &self.ease, false)
    }

    fn done(&self, now: f64) -> bool {
        now - self.started >= self.secs
    }

    fn leaving(&self) -> bool {
        self.to < 0.5
    }
}

/// The destinations a list of names makes, numbered from one the way a nav
/// list numbers its own, so a menu declared in markup and the lists inside
/// it agree on every id.
fn seed(labels: &[String]) -> Vec<Destination> {
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| Destination::new(LiveId(i as u64 + 1), label))
        .collect()
}

/// The state as the test tree spells it.
fn format_snapshot(collapsed: bool, open: Option<HamburgerSurface>) -> &'static str {
    match (collapsed, open) {
        (false, _) => "inline",
        (true, None) => "collapsed",
        (true, Some(HamburgerSurface::Drawer)) => "collapsed open drawer",
        (true, Some(HamburgerSurface::Drop)) => "collapsed open drop",
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Not splatted: `Drawer` and `Window` are widget names and `Collapsed`
    // is a word other widgets want, so these are always written out as
    // `HamburgerSurface.Drawer`, the way `PanelEdge` is.
    mod.widgets.HamburgerSurface = set_type_default() do #(HamburgerSurface::script_api(vm))
    mod.widgets.HamburgerMode = set_type_default() do #(HamburgerMode::script_api(vm))
    mod.widgets.HamburgerMeasure = set_type_default() do #(HamburgerMeasure::script_api(vm))

    use mod.widgets.*

    // The part of the menu's panel that moves: drawn where it rests, on a
    // list of its own, carried to where the travel has it by that list's
    // view transform. Kept to this module rather than put under
    // `mod.widgets`: it is the menu's own plumbing, and a name there is one
    // an app could build on.
    let HamburgerMotionBase = #(HamburgerMotion::register_widget(vm))
    let HamburgerMotion = set_type_default() do HamburgerMotionBase{
        width: Fill
        height: Fit
        flow: Down
        clip_x: false
        clip_y: false
    }

    mod.widgets.HamburgerMenuBase = #(HamburgerMenu::register_widget(vm))

    /** A row of destinations that collapses behind a burger button when the
     * window is narrow, into a side drawer or a panel under the button. */
    mod.widgets.HamburgerMenu = set_type_default() do mod.widgets.HamburgerMenuBase{
        width: Fit
        height: Fit
        flow: Right
        align: Align{y: 0.5}
        spacing: theme.space_2

        /** the destinations, by name */
        labels: []
        /** where the folded destinations go: HamburgerSurface.Drawer Drop */
        surface: HamburgerSurface.Drawer
        /** HamburgerMode.Responsive folds below the breakpoint; Collapsed always, Inline never */
        mode: HamburgerMode.Responsive
        /** the width compared: HamburgerMeasure.Window or the room the Parent leaves */
        measure: HamburgerMeasure.Window
        /** below this width the destinations fold behind the button 240..1600 step 10 */
        breakpoint: 720.
        /** a folded menu unfolds only this far past the breakpoint 0..96 step 1 */
        hysteresis: 16.
        /** a press or Return in the panel puts it away; arrows never do */
        close_on_pick: true
        /** which edge the drawer comes from: PanelEdge.Left Right Top Bottom */
        drawer_side: PanelEdge.Left
        /** how much room the drawer takes: Xs Sm Md Lg Xl Full */
        drawer_size: Sm
        /** the drawer's title */
        drawer_title: ""
        /** the drop panel's width 160..480 step 10 */
        drop_width: 240.
        /** how long the panel takes to come out, in seconds 0..1 step 0.01 */
        enter_secs: theme.motion_medium_2
        /** the curve the panel comes out on and the bars turn to a cross on: one of the theme's motion_ease tokens */
        enter_ease: theme.motion_ease_emphasized_decelerate
        /** how long the panel takes to go away, in seconds 0..1 step 0.01 */
        exit_secs: theme.motion_short_4
        /** the curve the panel goes away on and the cross turns back to bars on: one of the theme's motion_ease tokens */
        exit_ease: theme.motion_ease_standard_accelerate
        /** the panel and the bars land at once, without movement */
        reduced_motion: false

        burger_pop := PopoverToggle{
            placement: BottomStart
            offset: 4.
            // The button's own margin, moved out to the popover: the popover
            // hangs its panel off its own rect, and with the margin inside
            // it the panel hung 7 points below the button instead of 4.
            margin: theme.mspace_v_1
            // The popover's own panel is left clear and flat. The panel on
            // screen is the sheet inside the holder, which the menu can also
            // draw on its way out, after the popover has stopped drawing.
            panel_padding: Inset{top: 0. right: 0. bottom: 0. left: 0.}
            draw_panel +: {
                color: #0000
                border_size: 0.
                shadow_color: #0000
                shadow_radius: 0.
            }
            burger := BurgerButton{margin: 0.}
            // The holder: the popover places it, hit-tests it and keeps the
            // keyboard in it, where the panel rests.
            content := View{
                width: 240.
                height: Fit
                flow: Down
                clip_x: false
                clip_y: false
                drop_motion := HamburgerMotion{
                    // The popover's panel, as a sheet: its fill, bevelled
                    // outline, corners and level-two shadow.
                    drop_sheet := RoundedShadowView{
                        width: Fill
                        height: Fit
                        flow: Down
                        padding: Inset{top: theme.space_2 + 4. right: theme.space_2 bottom: theme.space_2 + 4. left: theme.space_2}
                        draw_bg +: {
                            color: theme.color_surface_container
                            border_size: 1.
                            border_radius: theme.corner_radius * 2.
                            border_color: theme.color_bevel_outset_1
                            border_color_2: theme.color_bevel_outset_2
                            shadow_color: theme.color_elevation_2
                            shadow_radius: theme.elevation_2_radius
                            shadow_offset: vec2(0., theme.elevation_2_offset_y)
                        }
                        drop_nav := NavList{
                            width: Fill
                            height: Fit
                        }
                    }
                }
            }
        }

        inline_nav := NavBar{
            width: Fit
            height: Fit
        }

        drawer := Drawer{
            side: PanelEdge.Left
            size: Sm
            // The holder: the drawer places it where the panel rests, tests
            // its scrim against it and hands it the keyboard. The drawer's
            // own panel, header and body and all, sits inside it as the
            // sheet, placed by the menu.
            content := View{
                width: 260.
                height: Fill
                flow: Down
                clip_x: false
                clip_y: false
                drawer_motion := HamburgerMotion{
                    height: Fill
                    drawer_sheet := Drawer.content{
                        body +: {
                            drawer_nav := NavList{
                                width: Fill
                                height: Fit
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The part of a panel that moves. Its children are laid out and drawn
/// where they rest, on a list of its own, and `transform` is written to that
/// list's view transform on every draw: the pixels go where the travel has
/// them and every rect a hit test reads stays where it rests.
#[derive(Script, Widget)]
pub struct HamburgerMotion {
    #[deref]
    view: View,
    #[rust]
    list: Option<DrawList2d>,
    #[rust(Mat4f::identity())]
    transform: Mat4f,
}

impl ScriptHook for HamburgerMotion {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.list = Some(DrawList2d::script_new(vm));
    }
}

impl HamburgerMotion {
    /// Empty the list when this draw event did not draw it. A list that is
    /// not drawn again keeps what it last held, and every rect in it still
    /// reads as valid: a panel put away would go on answering the test tree
    /// and every lookup where it last rested.
    fn forget_if_not_drawn(&mut self, cx: &mut Cx) {
        let Some(list) = &self.list else {
            return;
        };
        let (id, redraw_id) = (list.id(), cx.redraw_id);
        let held = &cx.draw_lists[id];
        if held.redraw_id == redraw_id || (held.draw_items.len() == 0 && held.rect_areas.is_empty()) {
            return;
        }
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        let held = &mut cx.draw_lists[id];
        held.clear_draw_items(redraw_id, recording_gen, uniforms_gen);
        held.draw_items.finish_recording();
    }

    fn set_transform(&mut self, cx: &mut Cx, transform: Mat4f) {
        if self.transform.v != transform.v {
            self.transform = transform;
            if let Some(list) = &self.list {
                list.set_view_transform(cx, &self.transform);
            }
        }
    }
}

impl Widget for HamburgerMotion {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(mut list) = self.list.take() else {
            self.view.draw_walk_all(cx, scope, walk);
            return DrawStep::done();
        };
        list.begin_always(cx);
        // Before the children draw as well as after: text reads its list's
        // transform while it is drawn, and a transform written only after
        // the draw would leave the names a frame behind the panel.
        list.set_view_transform_self_only(cx, &self.transform);
        self.view.draw_walk_all(cx, scope, walk);
        list.end(cx);
        list.set_view_transform(cx, &self.transform);
        self.list = Some(list);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct HamburgerMenu {
    #[source]
    source: ScriptObjectRef,
    /// The named parts: the button's popover, the row, and the drawer. The
    /// menu draws them itself, because a nav list has no visible flag of its
    /// own and the row must take no room while the menu is folded.
    #[deref]
    view: View,

    /// The destinations, by name, for a menu declared in markup.
    #[live]
    pub labels: Vec<String>,
    #[live]
    pub surface: HamburgerSurface,
    #[live]
    pub mode: HamburgerMode,
    #[live]
    pub measure: HamburgerMeasure,
    #[live(720.0)]
    pub breakpoint: f64,
    #[live(16.0)]
    pub hysteresis: f64,
    #[live(true)]
    pub close_on_pick: bool,
    #[live(PanelEdge::Left)]
    pub drawer_side: PanelEdge,
    #[live(DialogSize::Sm)]
    pub drawer_size: DialogSize,
    #[live]
    pub drawer_title: String,
    #[live(240.0)]
    pub drop_width: f64,
    #[live(0.3)]
    pub enter_secs: f64,
    #[live(Ease::OutCubic)]
    pub enter_ease: Ease,
    #[live(0.2)]
    pub exit_secs: f64,
    #[live(Ease::InCubic)]
    pub exit_ease: Ease,
    #[live]
    pub reduced_motion: bool,

    /// The menu's own rect. The view is never drawn as a view, so its area
    /// would stay empty and nothing could redraw or find the menu.
    #[rust]
    area: Area,
    #[rust]
    destinations: Vec<Destination>,
    /// Whether a host has handed destinations over, which `labels` then
    /// never overrides.
    #[rust]
    from_rust: bool,
    #[rust]
    seeded: bool,
    #[rust]
    selected: Option<LiveId>,
    #[rust]
    collapsed: bool,
    /// Whether a draw has decided `collapsed` from a real width yet.
    #[rust]
    measured: bool,
    /// The surface that is out, if one is. Kept rather than read from the
    /// surfaces, so a close is noticed however it happened and reported once.
    #[rust]
    open_surface: Option<HamburgerSurface>,
    /// A close owed to a pick, spent on the next frame.
    #[rust]
    close_frame: Option<NextFrame>,
    /// The drawer's list takes the keyboard at the first draw that gives it
    /// an area.
    #[rust]
    focus_nav_pending: bool,
    /// The panel that is out came out from the burger, so the keyboard goes
    /// back to the burger when it goes away.
    #[rust]
    focus_to_burger: bool,
    /// The press now down began on the open panel's list.
    #[rust]
    press_in_list: bool,
    /// The panel on its way, while it moves.
    #[rust]
    travel: Option<Travel>,
    #[rust]
    travel_frame: NextFrame,
    /// The overlay a panel is drawn on once its surface has let it go.
    #[rust]
    leaving: Option<DrawList2d>,
    /// The drop panel's height at rest, read off its list at every draw
    /// that shows it: what it grows to, and the holder's height meanwhile.
    #[rust]
    drop_rest_height: Option<f64>,
    /// Where the drop panel's holder last rested, to draw it leaving there.
    #[rust]
    drop_rest: Option<Rect>,
    /// The popover last hung the panel above the button.
    #[rust]
    drop_above: bool,
    /// The turn last written to the bars, so a draw at rest writes nothing.
    #[rust(f64::NAN)]
    turn_written: f64,
}

impl ScriptHook for HamburgerMenu {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.leaving = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // An edit of the markup may have changed the names; a control
        // writing one property has not, and re-reading them there would do
        // nothing but cost a comparison.
        if !apply.is_eval() {
            self.seeded = false;
            // An edit rebuilds the bars at their markup's turn, which is not
            // the one last written to them.
            self.turn_written = f64::NAN;
        }
        // Hidden by markup rather than through `set_visible`: the same reason
        // to put an open panel away at once, since no draw is coming to do it.
        if !self.view.visible && self.open_surface.is_some() {
            vm.with_cx_mut(|cx| self.put_away(cx, Animate::No));
        }
        if !self.view.visible {
            self.travel = None;
        }
    }
}

impl WidgetNode for HamburgerMenu {
    fn widget_uid(&self) -> WidgetUid {
        self.view.widget_uid()
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        self.view.redraw(cx);
        if let Some(leaving) = &self.leaving {
            leaving.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        self.view.children(visit);
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.view.find_widgets_from_point(cx, point, found);
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        // Put away here rather than on the next draw: a view does not draw a
        // hidden child, and a hidden menu hears no press, so a panel left out
        // would keep its grab on the pointer with nothing able to dismiss it.
        if !visible {
            self.put_away(cx, Animate::No);
            // A panel already leaving goes too: a view does not draw a
            // hidden child, so nothing would draw the rest of its way out.
            self.travel = None;
        }
        self.view.set_visible(cx, visible);
        self.area.redraw(cx);
    }

    fn visible(&self) -> bool {
        self.view.visible()
    }
}

impl HamburgerMenu {
    /// A direct part, by name, without a walk of the tree.
    fn part(&self, id: LiveId) -> WidgetRef {
        self.view
            .children
            .iter()
            .find(|(child, _)| *child == id)
            .map(|(_, widget)| widget.clone())
            .unwrap_or_default()
    }

    fn burger(&self, cx: &Cx) -> WidgetRef {
        self.widget(cx, ids!(burger))
    }

    /// The row, the drawer's list and the drop panel's list.
    fn lists(&self, cx: &Cx) -> [WidgetRef; 3] {
        [
            self.part(live_id!(inline_nav)),
            self.widget(cx, ids!(drawer_nav)),
            self.widget(cx, ids!(drop_nav)),
        ]
    }

    /// The list in the panel that is out, if one is.
    fn panel_list(&self, cx: &Cx) -> WidgetRef {
        match self.open_surface {
            Some(HamburgerSurface::Drawer) => self.widget(cx, ids!(drawer_nav)),
            Some(HamburgerSurface::Drop) => self.widget(cx, ids!(drop_nav)),
            None => WidgetRef::empty(),
        }
    }

    fn animate(&self) -> Animate {
        if self.reduced_motion {
            Animate::No
        } else {
            Animate::Yes
        }
    }

    fn emit(&self, cx: &mut Cx, action: HamburgerAction) {
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
    }

    /// The destinations, to all three lists. The lit one stays lit if it is
    /// still there, and otherwise the first takes over, as a nav list does.
    pub fn set_destinations(&mut self, cx: &mut Cx, destinations: Vec<Destination>) {
        self.from_rust = true;
        self.seeded = true;
        self.hand_out(cx, destinations);
    }

    fn hand_out(&mut self, cx: &mut Cx, destinations: Vec<Destination>) {
        let keep = self
            .selected
            .filter(|id| destinations.iter().any(|d| d.id == *id))
            .or_else(|| destinations.first().map(|d| d.id));
        // The same vector to each list, and the same lit one after it: three
        // lists that each fell back on their own could light three different
        // destinations when the old one went away.
        for list in self.lists(cx) {
            let list = list.as_nav_list();
            list.set_destinations(cx, destinations.clone());
            if let Some(id) = keep {
                list.select(cx, id);
            }
        }
        self.destinations = destinations;
        self.selected = keep;
        self.redraw(cx);
    }

    /// Turn `labels` into destinations, once per edit of the markup, unless
    /// a host has handed some over.
    fn ensure_seeded(&mut self, cx: &mut Cx) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        if self.from_rust {
            return;
        }
        let destinations = seed(&self.labels);
        if destinations != self.destinations {
            self.hand_out(cx, destinations);
        }
    }

    /// Light a destination in all three lists, reporting nothing, as a nav
    /// list's own `select` does.
    pub fn select(&mut self, cx: &mut Cx, id: LiveId) {
        self.ensure_seeded(cx);
        if !self.destinations.iter().any(|d| d.id == id) {
            return;
        }
        self.selected = Some(id);
        for list in self.lists(cx) {
            list.as_nav_list().select(cx, id);
        }
        self.redraw(cx);
    }

    pub fn selected(&self) -> Option<LiveId> {
        self.selected
    }

    fn selected_label(&self) -> Option<String> {
        let id = self.selected?;
        self.destinations
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.label.clone())
    }

    /// Whether the destinations are behind the button. Before the first draw
    /// has measured anything this is what the mode alone says, so a host
    /// asking early is not told a responsive menu is folded.
    pub fn is_collapsed(&self) -> bool {
        if self.measured {
            self.collapsed
        } else {
            self.mode == HamburgerMode::Collapsed
        }
    }

    pub fn is_open(&self) -> bool {
        self.open_surface.is_some()
    }

    /// Bring the destinations out on the menu's surface. Nothing happens
    /// while they are showing in the row: there is no panel to open.
    pub fn open(&mut self, cx: &mut Cx) {
        // A host opening the menu while the burger has the keyboard is
        // opening it from the burger as much as Return is.
        let from_burger = cx.has_key_focus(self.burger(cx).area());
        self.open_from(cx, from_burger);
    }

    fn open_from(&mut self, cx: &mut Cx, from_burger: bool) {
        if !self.is_collapsed() || self.open_surface.is_some() {
            return;
        }
        self.ensure_seeded(cx);
        match self.surface {
            HamburgerSurface::Drawer => self.part(live_id!(drawer)).as_dialog().open(cx),
            HamburgerSurface::Drop => self.part(live_id!(burger_pop)).as_popover().open(cx),
        }
        self.follow_surfaces_from(cx, from_burger);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.put_away(cx, self.animate());
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        if self.is_open() {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    /// Close whichever surface is out, turning the cross back at `animate`.
    fn put_away(&mut self, cx: &mut Cx, animate: Animate) {
        let open = self.open_surface;
        match open {
            Some(HamburgerSurface::Drawer) => {
                let drawer = self.part(live_id!(drawer)).as_dialog();
                // Asked only when open: a drawer's close hands the keyboard
                // back to where it was when it opened, and doing that for a
                // drawer already shut takes it from wherever it is now.
                if drawer.is_open() {
                    drawer.close(cx);
                }
            }
            Some(HamburgerSurface::Drop) => self.part(live_id!(burger_pop)).as_popover().close(cx),
            None => return,
        }
        self.mark_closed(cx, animate);
    }

    fn mark_opened(&mut self, cx: &mut Cx, surface: HamburgerSurface, from_burger: bool) {
        self.open_surface = Some(surface);
        self.focus_to_burger = from_burger;
        // The popover moves the keyboard into its own content; a drawer
        // gives it to the panel, which is one stop short of the list the
        // arrows are for.
        self.focus_nav_pending = surface == HamburgerSurface::Drawer;
        self.press_in_list = false;
        // Cut, never played: see `turn_burger`.
        self.burger(cx).as_button().set_open_with(cx, true, Animate::No);
        self.set_off(cx, surface, 1.0, self.animate());
        self.redraw(cx);
        self.emit(cx, HamburgerAction::Opened);
    }

    fn mark_closed(&mut self, cx: &mut Cx, animate: Animate) {
        let Some(surface) = self.open_surface.take() else {
            return;
        };
        self.focus_nav_pending = false;
        self.press_in_list = false;
        self.close_frame = None;
        // After the surface's own close, which hands the keyboard to where
        // it was when the surface opened: for a drop panel opened by a press
        // that is not the burger, whose press had not given it the keyboard
        // yet when the popover opened. A menu that has unfolded or been
        // hidden has no burger on screen to give it to.
        if std::mem::take(&mut self.focus_to_burger) && self.is_collapsed() && self.view.visible {
            let burger = self.burger(cx).area();
            if !burger.is_empty() {
                cx.set_key_focus(burger);
            }
        }
        self.burger(cx).as_button().set_open_with(cx, false, Animate::No);
        self.set_off(cx, surface, 0.0, animate);
        self.redraw(cx);
        self.emit(cx, HamburgerAction::Closed);
    }

    /// Send the panel of `surface` on its way to `to` (1 in place, 0 away)
    /// from wherever a travel it cuts short had got to, or put it there at
    /// once when there is to be no movement.
    fn set_off(&mut self, cx: &mut Cx, surface: HamburgerSurface, to: f64, animate: Animate) {
        let (secs, ease) = if to > 0.5 {
            (self.enter_secs, self.enter_ease)
        } else {
            (self.exit_secs, self.exit_ease)
        };
        if matches!(animate, Animate::No) || self.reduced_motion || !(secs > 0.0) {
            self.travel = None;
            return;
        }
        let now = cx.seconds_since_app_start();
        let from = match self.travel {
            Some(travel) if travel.surface == surface => travel.at(now),
            _ => 1.0 - to,
        };
        self.travel = Some(Travel { surface, from, to, started: now, secs, ease });
        self.travel_frame = cx.new_next_frame();
    }

    /// One frame of the travel: draw the panel where it now stands, and ask
    /// for the next frame until it has arrived.
    fn step_travel(&mut self, cx: &mut Cx) {
        let Some(travel) = self.travel else {
            return;
        };
        if travel.done(cx.seconds_since_app_start()) {
            self.travel = None;
        } else {
            self.travel_frame = cx.new_next_frame();
        }
        self.redraw(cx);
        self.part(live_id!(drawer)).redraw(cx);
        self.part(live_id!(burger_pop)).redraw(cx);
    }

    /// How far out the panel of `surface` stands at `now`: where its travel
    /// has got to while it moves, else in place while its surface is open.
    fn shown(&self, surface: HamburgerSurface, now: f64) -> f64 {
        match self.travel {
            Some(travel) if travel.surface == surface => travel.at(now),
            _ if self.open_surface == Some(surface) => 1.0,
            _ => 0.0,
        }
    }

    fn moving(&self, surface: HamburgerSurface, now: f64) -> bool {
        self.travel
            .is_some_and(|travel| travel.surface == surface && !travel.done(now))
    }

    /// Where the drop panel is drawn when it stands `v` of the way out from
    /// `rest`, where it rests.
    fn drop_drawn(&self, rest: Rect, v: f64) -> Rect {
        let (offset, height) = drop_panel_extent(rest.size.y, v, self.drop_above);
        Rect { pos: rest.pos + dvec2(0.0, offset), size: dvec2(rest.size.x, height) }
    }

    /// Lay both panels out where they rest, and give each the transform
    /// that carries it to where its travel has it, for this draw.
    fn place_panels(&mut self, cx: &mut Cx, pass: DVec2, now: f64) {
        // Always at a rect of the pass, never filling the holder: the drawer
        // slides the holder in on its first frame, and a panel that followed
        // it would arrive a frame late.
        let side = drawer_edge(self.drawer_side);
        let extent = drawer_extent(side, self.drawer_size, pass);
        let rest = drawer_panel_rect(side, extent, pass, 1.0);
        let drawn = drawer_panel_rect(side, extent, pass, self.shown(HamburgerSurface::Drawer, now));
        let drawer = self.part(live_id!(drawer));
        if let Some(mut sheet) = drawer.widget(cx, ids!(drawer_sheet)).borrow_mut::<View>() {
            sheet.walk = walk_at(rest);
        }
        if let Some(mut motion) = drawer.widget(cx, ids!(drawer_motion)).borrow_mut::<HamburgerMotion>() {
            motion.set_transform(cx, rest_to_drawn(rest, drawn));
        }

        let holder = self.part(live_id!(burger_pop)).as_popover().content();
        if let Some(mut holder) = holder.borrow_mut::<View>() {
            holder.walk.width = Size::Fixed(self.drop_width);
        }
        let transform = match (self.moving(HamburgerSurface::Drop, now), self.drop_rest) {
            (true, Some(rest)) => rest_to_drawn(rest, self.drop_drawn(rest, self.shown(HamburgerSurface::Drop, now))),
            // The first frame of an opening has no resting place measured
            // yet, and the panel has not begun to grow.
            (true, None) => rest_to_drawn(Rect { pos: DVec2::default(), size: dvec2(1.0, 1.0) }, Rect::default()),
            (false, _) => Mat4f::identity(),
        };
        if let Some(mut motion) = holder.widget(cx, ids!(drop_motion)).borrow_mut::<HamburgerMotion>() {
            motion.set_transform(cx, transform);
        }
    }

    /// Read the drop panel's resting height and place off the frame the
    /// popover last drew it in.
    ///
    /// Read before this draw begins, from the last one: the popover hangs
    /// its panel by moving everything it drew once it has measured it, and
    /// the holder's rect only shows where it went after that draw is over.
    /// A holder the popover did not draw last frame says nothing.
    fn measure_drop(&mut self, cx: &Cx) {
        let pop = self.part(live_id!(burger_pop));
        let Some(popover) = pop.borrow::<Popover>() else {
            return;
        };
        let holder = popover.content();
        let above = popover.placed_side() == Some(Side::Top);
        drop(popover);
        let held = holder.area();
        let sheet = holder.widget(cx, ids!(drop_sheet));
        let list = sheet.widget(cx, ids!(drop_nav)).area();
        if !held.is_valid(cx) || !list.is_valid(cx) {
            return;
        }
        // The list's own rect and the sheet's padding round it: the height
        // the sheet has at rest.
        let padding = sheet.borrow::<View>().map(|sheet| sheet.layout.padding).unwrap_or_default();
        let height = list.rect(cx).size.y + padding.top + padding.bottom;
        let rect = held.rect(cx);
        self.drop_rest_height = Some(height);
        self.drop_above = above;
        self.drop_rest = Some(Rect { pos: rect.pos, size: dvec2(rect.size.x, height) });
    }

    /// Draw the panel on its way out, on the menu's own overlay, once its
    /// surface has let it go. Nothing there hears an event, so the panel
    /// takes neither the press that follows the close nor the keyboard. It
    /// is laid out where it rested, as it was on its way in, and the
    /// overlay's transform carries it out.
    fn draw_leaving(&mut self, cx: &mut Cx2d, scope: &mut Scope, pass: DVec2, now: f64) {
        // Begun on every draw, as the popover begins its own: a list that is
        // not begun keeps showing what it showed last.
        let Some(mut leaving) = self.leaving.take() else {
            return;
        };
        leaving.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(Layout::flow_down());
        let travel = self.travel.filter(|travel| {
            travel.leaving() && !travel.done(now) && self.open_surface != Some(travel.surface)
        });
        let mut transform = Mat4f::identity();
        if let Some(travel) = travel {
            let v = travel.at(now);
            match travel.surface {
                HamburgerSurface::Drawer => {
                    let side = drawer_edge(self.drawer_side);
                    let extent = drawer_extent(side, self.drawer_size, pass);
                    let rest = drawer_panel_rect(side, extent, pass, 1.0);
                    transform = rest_to_drawn(rest, drawer_panel_rect(side, extent, pass, v));
                    // Before the draw too, for the text: see `HamburgerMotion`.
                    leaving.set_view_transform_self_only(cx, &transform);
                    let sheet = self.part(live_id!(drawer)).widget(cx, ids!(drawer_sheet));
                    let _ = sheet.draw_walk(cx, scope, walk_at(rest));
                }
                HamburgerSurface::Drop => {
                    if let Some(rest) = self.drop_rest {
                        transform = rest_to_drawn(rest, self.drop_drawn(rest, v));
                        leaving.set_view_transform_self_only(cx, &transform);
                        let holder = self.part(live_id!(burger_pop)).as_popover().content();
                        let sheet = holder.widget(cx, ids!(drop_sheet));
                        let _ = sheet.draw_walk(cx, scope, walk_at(rest));
                    }
                }
            }
        }
        cx.end_pass_sized_turtle();
        leaving.end(cx);
        leaving.set_view_transform(cx, &transform);
        self.leaving = Some(leaving);
    }

    /// Write the bars' turn from the panel's clock.
    ///
    /// `set_open_with` only chooses whether the button plays its own open
    /// track, and that track's length and curve are fixed in the button's
    /// markup, where nothing the button offers can change them. So the menu
    /// only ever cuts the track, which keeps the button's state right, and
    /// writes the turn itself: along the panel's curve, from where the panel
    /// is, so a spring turns the bars past the cross and back with it.
    fn turn_burger(&mut self, cx: &mut Cx, now: f64) {
        let turn = match self.travel {
            Some(travel) => travel.at(now),
            None if self.open_surface.is_some() => 1.0,
            None => 0.0,
        };
        if turn == self.turn_written {
            return;
        }
        self.turn_written = turn;
        let mut burger = self.burger(cx);
        script_apply_eval!(cx, burger, {
            draw_bg +: {open: #(turn)}
        });
    }

    /// Bring the menu's idea of what is out in line with the surfaces.
    ///
    /// Read from the surfaces rather than from their reports, so a close is
    /// followed however it came about: the scrim, Escape, the close mark, the
    /// back gesture, or a host that shut the drawer or the popover itself.
    fn follow_surfaces(&mut self, cx: &mut Cx) {
        // A surface that opened itself did so on a press on its anchor,
        // which is the burger.
        self.follow_surfaces_from(cx, true);
    }

    fn follow_surfaces_from(&mut self, cx: &mut Cx, from_burger: bool) {
        let drawer_open = self.part(live_id!(drawer)).as_dialog().is_open();
        let pop_open = self.part(live_id!(burger_pop)).as_popover().is_open();
        let open = self.open_surface;
        match open {
            Some(HamburgerSurface::Drawer) if !drawer_open => self.mark_closed(cx, self.animate()),
            Some(HamburgerSurface::Drop) if !pop_open => self.mark_closed(cx, self.animate()),
            None if pop_open => self.mark_opened(cx, HamburgerSurface::Drop, from_burger),
            None if drawer_open => self.mark_opened(cx, HamburgerSurface::Drawer, from_burger),
            _ => {}
        }
    }

    /// Where a choice about to be made in the open panel is coming from,
    /// read off the raw event before the list turns it into a choice.
    fn pick_source(&mut self, cx: &Cx, event: &Event) -> Option<PickSource> {
        if self.open_surface.is_none() {
            self.press_in_list = false;
            return None;
        }
        let list = self.panel_list(cx);
        if list.is_empty() {
            return None;
        }
        let area = list.area();
        // Clipped, as a hit test is: the drawer's body scrolls, and a row
        // scrolled out from under the header must not count a release on the
        // header as a press on the list.
        let inside = |abs: DVec2| {
            let rect = area.clipped_rect(cx);
            area.is_valid(cx) && rect.size.x > 0.0 && rect.contains(abs)
        };
        match event {
            Event::KeyDown(ke) if cx.has_key_focus(area) => match ke.key_code {
                KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::Home
                | KeyCode::End => Some(PickSource::Arrow),
                KeyCode::ReturnKey | KeyCode::Space if !ke.is_repeat => Some(PickSource::Confirm),
                _ => None,
            },
            Event::MouseDown(me) => {
                self.press_in_list = me.button.is_primary() && inside(me.abs);
                None
            }
            Event::MouseUp(me) => {
                let began_inside = std::mem::take(&mut self.press_in_list);
                (began_inside && inside(me.abs)).then_some(PickSource::Pointer)
            }
            Event::TouchUpdate(te) => {
                let mut source = None;
                for touch in &te.touches {
                    match touch.state {
                        TouchState::Start => self.press_in_list = inside(touch.abs),
                        TouchState::Stop => {
                            if std::mem::take(&mut self.press_in_list) && inside(touch.abs) {
                                source = Some(PickSource::Pointer);
                            }
                        }
                        _ => {}
                    }
                }
                source
            }
            _ => None,
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Only the drawer is opened from the button's click. The drop panel
        // opened itself on the press, and answering the click as well would
        // shut it again on the release.
        if self.surface == HamburgerSurface::Drawer
            && self.is_collapsed()
            && self.open_surface.is_none()
            && self.burger(cx).as_button().clicked(actions)
        {
            self.open_from(cx, true);
        }

        let lists = self.lists(cx);
        let chosen = lists
            .iter()
            .enumerate()
            .find_map(|(at, list)| list.as_nav_list().chosen(actions).map(|id| (at, id)));
        if let Some((from, id)) = chosen {
            for (at, list) in lists.iter().enumerate() {
                if at != from {
                    list.as_nav_list().select(cx, id);
                }
            }
            self.selected = Some(id);
            self.redraw(cx);
            self.emit(cx, HamburgerAction::Selected(id));
        }
    }

    /// The width the breakpoint is compared with.
    /// How much room this menu has to fill -- not how wide it would like to
    /// be, which is what `Widget::measure_width` answers.
    fn available_width(&self, cx: &Cx2d, walk: Walk) -> f64 {
        match self.measure {
            HamburgerMeasure::Window => cx.current_pass_size().x,
            HamburgerMeasure::Parent => cx
                .peek_walk_turtle(Walk {
                    width: Size::fill(),
                    ..walk
                })
                .size
                .x,
        }
    }

    /// Fold or unfold for this draw, before anything is drawn, so the frame
    /// that crosses the breakpoint already shows the new mode.
    fn decide(&mut self, cx: &mut Cx, width: f64) {
        let was = self.is_collapsed();
        // A width of nothing is a pass not yet sized, or a parent that has no
        // room to give; it says nothing about the room, and folding on it
        // would swap the row for the button on the first frame of every
        // window.
        let known = width.is_finite() && width > 0.0;
        let collapsed = match self.mode {
            HamburgerMode::Responsive if !known => was,
            mode => next_collapsed(mode, was, width, self.breakpoint, self.hysteresis),
        };
        self.measured = true;
        self.collapsed = collapsed;
        if collapsed != was {
            self.emit(cx, HamburgerAction::ModeChanged(collapsed));
        }
    }

    /// Write the menu's properties into the parts that carry them.
    fn dress(&mut self, cx: &mut Cx) {
        let pop = self.part(live_id!(burger_pop));
        if let Some(mut pop) = pop.borrow_mut::<Popover>() {
            pop.trigger = popover_trigger_for(self.surface);
        }
        let drawer = self.part(live_id!(drawer));
        if let Some(mut drawer) = drawer.borrow_mut::<Dialog>() {
            drawer.side = drawer_edge(self.drawer_side);
            drawer.size = self.drawer_size;
            if drawer.title != self.drawer_title {
                drawer.set_text(cx, &self.drawer_title);
            }
            drawer.slide_secs = DRAWER_SLIDE_FLOOR_SECS;
        };
    }
}

impl Widget for HamburgerMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_seeded(cx.cx.cx);
        let width = self.available_width(cx, walk);
        self.decide(cx.cx.cx, width);
        let visible = self.view.visible;
        // A panel left out over a menu that has unfolded, or been switched
        // to the other surface, belongs to nothing on screen. It goes at
        // once, the cross with it: there is no button left to watch turning
        // back. A hidden menu is only drawn by a parent that draws children
        // whatever their flag (a view skips them), so hiding puts the panel
        // away in `set_visible`, and this is the same rule for that parent.
        if let Some(open) = self.open_surface {
            if !visible || !self.collapsed || open != self.surface {
                self.put_away(cx.cx.cx, Animate::No);
            }
        }
        // A panel still leaving a surface the menu no longer shows has
        // nothing on screen to leave from, and goes with it at once.
        let (folded, surface) = (self.collapsed, self.surface);
        if self
            .travel
            .is_some_and(|travel| !visible || !folded || travel.surface != surface)
        {
            self.travel = None;
        }
        self.dress(cx.cx.cx);
        let now = cx.seconds_since_app_start();
        let pass = cx.current_pass_size();
        self.measure_drop(cx);
        self.place_panels(cx.cx.cx, pass, now);
        self.turn_burger(cx.cx.cx, now);

        let collapsed = self.collapsed;
        // Hidden through its own flag rather than skipped: a popover begins
        // its overlay list on every draw, and a list that is not begun keeps
        // showing its last frame.
        self.part(live_id!(burger_pop))
            .set_visible(cx.cx.cx, visible && collapsed);
        let children = self.view.children.clone();
        if visible {
            cx.begin_turtle(walk, self.view.layout);
        }
        for (id, child) in children.iter() {
            // The row has no visible flag, so it is simply not drawn: it takes
            // no room, its areas go stale, and nothing can press it.
            if *id == live_id!(inline_nav) && (collapsed || !visible) {
                continue;
            }
            // The drawer and the popover draw on overlays of their own and
            // take no room here, so they are drawn even while hidden, to
            // clear what they last showed.
            if !visible && *id != live_id!(drawer) && *id != live_id!(burger_pop) {
                continue;
            }
            let _ = child.draw_all(cx, scope);
        }
        if visible {
            cx.end_turtle_with_area(&mut self.area);
        }
        self.draw_leaving(cx, scope, pass, now);
        // A panel its surface did not draw this time is not on screen.
        let motions = [
            self.part(live_id!(drawer)).widget(cx, ids!(drawer_motion)),
            self.part(live_id!(burger_pop)).as_popover().content().widget(cx, ids!(drop_motion)),
        ];
        for motion in motions {
            if let Some(mut motion) = motion.borrow_mut::<HamburgerMotion>() {
                motion.forget_if_not_drawn(cx.cx.cx);
            }
        }

        if self.focus_nav_pending && self.open_surface == Some(HamburgerSurface::Drawer) {
            // After the drawer has drawn, so this wins over the modal, which
            // gives the keyboard to its whole panel on the same draw.
            let area = self.widget(cx, ids!(drawer_nav)).area();
            if area.is_valid(cx) {
                self.focus_nav_pending = false;
                cx.set_key_focus(area);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(frame) = self.close_frame {
            if frame.is_event(event).is_some() {
                self.close_frame = None;
                self.close(cx);
            }
        }
        if self.travel_frame.is_event(event).is_some() {
            self.step_travel(cx);
        }
        if !self.view.visible && event.requires_visibility() {
            return;
        }

        // Read before the children see the event. Once a drawer has closed
        // it forwards nothing, so a close made on the release itself would
        // swallow the click the row was about to raise, and the destination
        // would never be chosen: the close is owed to the next frame, by
        // which time the choice has been reported.
        if let Some(source) = self.pick_source(cx, event) {
            if closes_on(self.close_on_pick, source) {
                self.close_frame = Some(cx.new_next_frame());
            }
        }

        let collapsed = self.is_collapsed();
        let needs_sight = event.requires_visibility();
        let children = self.view.children.clone();
        // Last drawn first, as a view does: the drawer lies over everything,
        // so it must have a press before the button under its scrim does.
        for (id, child) in children.iter().rev() {
            let shown = if *id == live_id!(inline_nav) {
                !collapsed
            } else if *id == live_id!(burger_pop) {
                // A hidden popover still holds the rect its button had, and
                // would open on a press where the button used to be.
                collapsed || child.as_popover().is_open()
            } else {
                true
            };
            if shown || !needs_sight {
                child.handle_event(cx, event, scope);
            }
        }

        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }

        // The button has no key activation of its own.
        if let Event::KeyDown(ke) = event {
            if collapsed
                && !ke.is_repeat
                && matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space)
                && cx.has_key_focus(self.burger(cx).area())
            {
                if self.is_open() {
                    self.close(cx);
                } else {
                    self.open_from(cx, true);
                }
            }
        }

        self.follow_surfaces(cx);
    }

    /// Where you are, so a test reads the menu in one line.
    fn text(&self) -> String {
        self.selected_label().unwrap_or_default()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format_snapshot(self.is_collapsed(), self.open_surface).to_string())
    }

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        self.selected_label()
    }
}

impl HamburgerMenuRef {
    pub fn set_destinations(&self, cx: &mut Cx, destinations: Vec<Destination>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_destinations(cx, destinations);
        }
    }

    pub fn select(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.select(cx, id);
        }
    }

    pub fn selected(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.selected())
    }

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

    pub fn toggle(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_open())
    }

    pub fn is_collapsed(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_collapsed())
    }

    /// Every report this menu made in `actions`, in order. Several can
    /// arrive together — a pick and the close it causes — so a lookup that
    /// stops at the first would miss the rest.
    fn reports(&self, actions: &Actions) -> Vec<HamburgerAction> {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == uid)
            .map(|action| action.cast::<HamburgerAction>())
            .collect()
    }

    /// The destination chosen this pass, if one was.
    pub fn chosen(&self, actions: &Actions) -> Option<LiveId> {
        self.reports(actions).into_iter().find_map(|report| match report {
            HamburgerAction::Selected(id) => Some(id),
            _ => None,
        })
    }

    pub fn opened(&self, actions: &Actions) -> bool {
        self.reports(actions).contains(&HamburgerAction::Opened)
    }

    pub fn closed(&self, actions: &Actions) -> bool {
        self.reports(actions).contains(&HamburgerAction::Closed)
    }

    /// Whether the menu folded (`Some(true)`) or unfolded this pass.
    pub fn mode_changed(&self, actions: &Actions) -> Option<bool> {
        self.reports(actions).into_iter().find_map(|report| match report {
            HamburgerAction::ModeChanged(collapsed) => Some(collapsed),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use crate::makepad_script::trap::NoTrap;
    use crate::makepad_script::{script, ScriptMod};
    use crate::nav_list::{NavAction, NavList};
    use std::{cell::Cell, collections::HashSet};

    /// The theme's easings, in the order Foundations > Motion plays them.
    const EASE_TOKENS: [&str; 8] = [
        "motion_ease_standard",
        "motion_ease_standard_decelerate",
        "motion_ease_standard_accelerate",
        "motion_ease_emphasized_decelerate",
        "motion_ease_emphasized_accelerate",
        "motion_ease_linear",
        "motion_ease_spring",
        "motion_ease_bounce",
    ];

    /// The easing a theme token names, read out of the loaded theme the way
    /// the Foundations page reads it, so these checks follow the theme and
    /// never restate it.
    fn theme_ease(cx: &mut Cx, token: &str) -> Ease {
        cx.with_vm(|vm| {
            let theme = vm.module(id!(theme));
            let value = vm.bx.heap.value(theme, LiveId::from_str(token).into(), NoTrap);
            Ease::script_from_value(vm, value)
        })
    }

    fn theme_number(cx: &mut Cx, token: &str) -> f64 {
        cx.with_vm(|vm| {
            let theme = vm.module(id!(theme));
            vm.bx
                .heap
                .value(theme, LiveId::from_str(token).into(), NoTrap)
                .as_f64()
                .unwrap_or_else(|| panic!("the theme has no number {token}"))
        })
    }

    /// A travel follows the theme's own evaluation of every easing: into
    /// place, away, and back from part of the way. It arrives exactly, and
    /// with motion off, or no time to take, it is there at once.
    #[test]
    fn the_panel_travels_on_every_theme_easing() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        for token in EASE_TOKENS {
            let ease = theme_ease(&mut cx, token);
            // One second, so the fraction handed to the ease is the one
            // checked here and not a rounding of it.
            for x in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9] {
                let eased = ease.map(x);
                assert_eq!(panel_travel(0.0, 1.0, x, 1.0, &ease, false), eased, "{token} in at {x}");
                assert_eq!(panel_travel(1.0, 0.0, x, 1.0, &ease, false), 1.0 - eased, "{token} out at {x}");
                let back = panel_travel(0.4, 1.0, x, 1.0, &ease, false);
                assert!((back - (0.4 + 0.6 * eased)).abs() < 1e-12, "{token} back from part way at {x}");
                assert_eq!(panel_travel(0.0, 1.0, x, 1.0, &ease, true), 1.0, "{token} with motion off");
            }
            assert_eq!(panel_travel(0.0, 1.0, 1.0, 1.0, &ease, false), 1.0, "{token} arrives");
            assert_eq!(panel_travel(1.0, 0.0, 9.0, 1.0, &ease, false), 0.0, "{token} has left");
            assert_eq!(panel_travel(0.0, 1.0, 0.0, 0.0, &ease, false), 1.0, "{token} with no time");
            assert_eq!(panel_travel(0.0, 1.0, 0.0, f64::NAN, &ease, false), 1.0, "{token} with no number");
        }
        let widest = |ease: &Ease| {
            (1..200)
                .map(|i| panel_travel(0.0, 1.0, i as f64 * 0.005, 1.0, ease, false))
                .fold(0.0, f64::max)
        };
        assert!(widest(&theme_ease(&mut cx, "motion_ease_spring")) > 1.0, "a spring swings past its place");
        assert!(widest(&theme_ease(&mut cx, "motion_ease_bounce")) <= 1.0, "a bounce never does");
        });
    }

    /// Short of its place the drawer's panel stands out past its edge at its
    /// own size; past it, on a spring, it stretches with its outer side
    /// still on the edge, so no gap opens along the window.
    #[test]
    fn the_drawer_panel_keeps_to_its_edge() {
        crate::on_test_cx(|| {
        let pass = dvec2(800.0, 600.0);
        let rect = |x: f64, y: f64, w: f64, h: f64| Rect { pos: dvec2(x, y), size: dvec2(w, h) };

        let at = |v| drawer_panel_rect(PanelEdge::Left, 260.0, pass, v);
        assert_eq!(at(1.0), rect(0.0, 0.0, 260.0, 600.0));
        assert_eq!(at(0.0), rect(-260.0, 0.0, 260.0, 600.0));
        assert_eq!(at(0.5), rect(-130.0, 0.0, 260.0, 600.0));
        assert_eq!(at(1.25), rect(0.0, 0.0, 325.0, 600.0));

        let at = |v| drawer_panel_rect(PanelEdge::Right, 260.0, pass, v);
        assert_eq!(at(1.0), rect(540.0, 0.0, 260.0, 600.0));
        assert_eq!(at(0.0), rect(800.0, 0.0, 260.0, 600.0));
        assert_eq!(at(0.5), rect(670.0, 0.0, 260.0, 600.0));
        assert_eq!(at(1.25), rect(475.0, 0.0, 325.0, 600.0));

        let at = |v| drawer_panel_rect(PanelEdge::Top, 180.0, pass, v);
        assert_eq!(at(1.0), rect(0.0, 0.0, 800.0, 180.0));
        assert_eq!(at(0.0), rect(0.0, -180.0, 800.0, 180.0));
        assert_eq!(at(1.5), rect(0.0, 0.0, 800.0, 270.0));

        let at = |v| drawer_panel_rect(PanelEdge::Bottom, 180.0, pass, v);
        assert_eq!(at(1.0), rect(0.0, 420.0, 800.0, 180.0));
        assert_eq!(at(0.0), rect(0.0, 600.0, 800.0, 180.0));
        assert_eq!(at(1.5), rect(0.0, 330.0, 800.0, 270.0));

        // A spring on the way out goes further out, never back across.
        assert!(drawer_panel_rect(PanelEdge::Left, 260.0, pass, -0.1).pos.x < -260.0);
        // A drawer the full size of the window takes the window's length.
        assert_eq!(drawer_extent(PanelEdge::Left, DialogSize::Full, pass), 800.0);
        assert_eq!(drawer_extent(PanelEdge::Bottom, DialogSize::Full, pass), 600.0);
        assert_eq!(drawer_extent(PanelEdge::Right, DialogSize::Sm, pass), 260.0);
        });
    }

    /// The drop panel grows away from the button, whichever side the popover
    /// hung it on, and never to less than nothing.
    #[test]
    fn the_drop_panel_grows_away_from_the_button() {
        crate::on_test_cx(|| {
        assert_eq!(drop_panel_extent(120.0, 0.25, false), (0.0, 30.0));
        assert_eq!(drop_panel_extent(120.0, 0.25, true), (90.0, 30.0));
        assert_eq!(drop_panel_extent(120.0, 1.0, true), (0.0, 120.0));
        assert_eq!(drop_panel_extent(120.0, 1.1, false).1, 132.0);
        assert_eq!(drop_panel_extent(120.0, -0.2, true), (120.0, 0.0));
        });
    }

    /// Resting on the line does not flicker: folded, the menu comes back out
    /// only past the band, and a menu that is out folds on the line itself.
    #[test]
    fn the_switch_has_a_band_so_it_does_not_flicker() {
        crate::on_test_cx(|| {
        let at = |was, width| next_collapsed(HamburgerMode::Responsive, was, width, 640.0, 16.0);
        assert!(at(false, 639.0));
        assert!(!at(false, 640.0));
        assert!(at(true, 650.0));
        assert!(at(true, 655.9));
        assert!(!at(true, 656.0));
        for width in [0.0, 320.0, 640.0, 4000.0] {
            for was in [false, true] {
                assert!(next_collapsed(HamburgerMode::Collapsed, was, width, 640.0, 16.0));
                assert!(!next_collapsed(HamburgerMode::Inline, was, width, 640.0, 16.0));
            }
        }
        // A negative band is read as none, so the menu never unfolds below
        // the width that folded it.
        assert!(next_collapsed(HamburgerMode::Responsive, true, 630.0, 640.0, -32.0));
        });
    }

    /// Arrows choose as they move, so they must never put the panel away.
    #[test]
    fn arrows_never_close_the_panel() {
        crate::on_test_cx(|| {
        assert!(!closes_on(true, PickSource::Arrow));
        assert!(closes_on(true, PickSource::Pointer));
        assert!(closes_on(true, PickSource::Confirm));
        for source in [PickSource::Pointer, PickSource::Arrow, PickSource::Confirm] {
            assert!(!closes_on(false, source));
        }
        });
    }

    /// The drop panel is the popover's to open; under a drawer it must not
    /// open at all.
    #[test]
    fn the_drop_surface_lets_the_popover_open_itself() {
        crate::on_test_cx(|| {
        assert_eq!(popover_trigger_for(HamburgerSurface::Drop), PopoverTrigger::Click);
        assert_eq!(popover_trigger_for(HamburgerSurface::Drawer), PopoverTrigger::Manual);
        });
    }

    /// The three lists are handed equal vectors with the ids a nav list
    /// would give its own names, and seeding again gives an equal vector, so
    /// each list's "same list, no rebuild" short-circuit holds.
    #[test]
    fn three_lists_are_seeded_the_same() {
        crate::on_test_cx(|| {
        let labels: Vec<String> = ["Home", "Library", "History"].iter().map(|s| s.to_string()).collect();
        let once = seed(&labels);
        let ids: Vec<LiveId> = once.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![LiveId(1), LiveId(2), LiveId(3)]);
        assert_eq!(once[1].label, "Library");
        assert_eq!(once, seed(&labels));
        assert!(seed(&[]).is_empty());
        });
    }

    #[test]
    fn snapshot_value_spells_the_state() {
        crate::on_test_cx(|| {
        assert_eq!(format_snapshot(false, None), "inline");
        assert_eq!(format_snapshot(true, None), "collapsed");
        assert_eq!(format_snapshot(true, Some(HamburgerSurface::Drawer)), "collapsed open drawer");
        assert_eq!(format_snapshot(true, Some(HamburgerSurface::Drop)), "collapsed open drop");
        });
    }

    /// Composed of a drawer, which is a dialog on an edge, a popover and a
    /// nav list, so it must register after all three.
    #[test]
    fn it_registers_after_the_dialog() {
        crate::on_test_cx(|| {
        let calls = crate::widgets_mod_source();
        let at = calls
            .find("crate::hamburger_menu::script_mod(vm);")
            .expect("the menu is registered");
        for base in ["dialog", "popover", "nav_list", "button"] {
            let call = format!("crate::{base}::script_mod(vm);");
            let base_at = calls.find(&call).unwrap_or_else(|| panic!("{call} is not registered"));
            assert!(base_at < at, "{call} must register before the menu");
        }
        });
    }

    /// Splatted, the variants would be exported bare, and `Drawer` and
    /// `Window` are already widgets.
    #[test]
    fn no_enum_is_splatted() {
        crate::on_test_cx(|| {
        let source = include_str!("hamburger_menu.rs");
        for name in ["HamburgerSurface", "HamburgerMode", "HamburgerMeasure"] {
            // Built at run time, so this test's own text does not match.
            let needle = format!("splat(mod.widgets.{name})");
            assert!(!source.contains(&needle), "{name} is splatted");
            let registered = format!("mod.widgets.{name} = set_type_default() do #({name}::script_api(vm))");
            assert!(source.contains(&registered), "{name} is not registered");
        }
        });
    }

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    fn menu(cx: &mut Cx, source: ScriptMod) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = vm.eval(source);
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// One frame of `root` into a window-less pass of `size`. The drawer and
    /// the popover each draw on an overlay list of their own, and those
    /// hang off the window's overlay, so the target keeps one as a window
    /// does.
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

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef, size: DVec2) {
            self.pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn reports(actions: &Actions) -> Vec<HamburgerAction> {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .map(|action| action.cast::<HamburgerAction>())
            .filter(|action| *action != HamburgerAction::None)
            .collect()
    }

    /// The markup is read by nothing the compiler checks: building it is
    /// what proves the parts exist under the names the menu looks them up
    /// by, and that the defaults are the ones the markup documents.
    #[test]
    fn the_menu_builds_its_parts_under_the_names_it_looks_for() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{labels: ["Home" "Library" "History"]}
        });
        let timing = [
            theme_number(&mut cx, "motion_medium_2"),
            theme_number(&mut cx, "motion_short_4"),
        ];
        let eases = [
            theme_ease(&mut cx, "motion_ease_emphasized_decelerate"),
            theme_ease(&mut cx, "motion_ease_standard_accelerate"),
        ];
        let inner = root.borrow::<HamburgerMenu>().expect("a hamburger menu");
        assert_eq!([inner.enter_secs, inner.exit_secs], timing, "the panel is timed by the theme");
        assert_eq!([inner.enter_ease, inner.exit_ease], eases, "the panel is eased by the theme");
        assert_eq!(inner.surface, HamburgerSurface::Drawer);
        assert_eq!(inner.mode, HamburgerMode::Responsive);
        assert_eq!(inner.measure, HamburgerMeasure::Window);
        assert_eq!(inner.breakpoint, 720.0);
        assert_eq!(inner.hysteresis, 16.0);
        assert!(inner.close_on_pick);
        assert_eq!(inner.drawer_side, PanelEdge::Left);
        assert_eq!(inner.drawer_size, DialogSize::Sm);
        assert_eq!(inner.drop_width, 240.0);
        assert!(!inner.reduced_motion);
        for name in [ids!(burger), ids!(drawer_nav), ids!(drop_nav)] {
            assert!(!inner.widget(&cx, name).is_empty(), "no part {name:?}");
        }
        for name in [live_id!(burger_pop), live_id!(inline_nav), live_id!(drawer)] {
            assert!(!inner.part(name).is_empty(), "no part {name:?}");
        }
        assert!(inner.part(live_id!(drawer)).borrow::<Dialog>().is_some());
        assert!(inner.part(live_id!(burger_pop)).borrow::<Popover>().is_some());
        for list in inner.lists(&cx) {
            assert!(list.borrow::<NavList>().is_some(), "a list is not a NavList");
        }
        // Each surface holds its panel in a holder with nothing else in it:
        // the markup's holder replaced the surface's own content rather than
        // merging into it, and the drawer's panel kept all its parts. Between
        // holder and panel is the part that moves, which clips nothing.
        let only_child = |holder: &WidgetRef, motion: LiveId, sheet: LiveId| {
            {
                let holder = holder.borrow::<View>().expect("a holder is a view");
                let children: Vec<LiveId> = holder.children.iter().map(|(id, _)| *id).collect();
                assert_eq!(children, vec![motion], "the holder holds only the part that moves");
                assert!(!holder.show_bg, "the holder draws nothing of its own");
                assert!(!holder.layout.clip_x && !holder.layout.clip_y, "the holder clips the panel's shadow");
            }
            let moving = holder.widget(&cx, &[motion]);
            let moving = moving.borrow::<HamburgerMotion>().expect("the part that moves is a HamburgerMotion");
            let children: Vec<LiveId> = moving.view.children.iter().map(|(id, _)| *id).collect();
            assert_eq!(children, vec![sheet], "the part that moves holds only the panel");
            assert!(!moving.view.layout.clip_x && !moving.view.layout.clip_y, "the part that moves clips the panel's shadow");
        };
        let holder = inner.part(live_id!(drawer)).widget(&cx, ids!(content));
        only_child(&holder, live_id!(drawer_motion), live_id!(drawer_sheet));
        let sheet = holder.widget(&cx, ids!(drawer_sheet));
        for name in [ids!(title_label), ids!(close), ids!(body), ids!(drawer_nav)] {
            assert!(!sheet.widget(&cx, name).is_empty(), "the drawer's panel has no {name:?}");
        }
        let holder = inner.part(live_id!(burger_pop)).as_popover().content();
        only_child(&holder, live_id!(drop_motion), live_id!(drop_sheet));
        let sheet = holder.widget(&cx, ids!(drop_sheet));
        assert!(!sheet.widget(&cx, ids!(drop_nav)).is_empty());
        let sheet = sheet.borrow::<View>().expect("the drop panel is a view");
        assert!(!sheet.layout.clip_x && !sheet.layout.clip_y, "the drop panel clips its own shadow");
        });
    }

    /// The transform carries the resting rect onto the drawn one, corner to
    /// corner, on every path a panel takes; a panel drawn to nothing is drawn
    /// very small rather than to a zero the text shaders divide by.
    #[test]
    fn a_panel_is_carried_from_where_it_rests_to_where_it_is_drawn() {
        crate::on_test_cx(|| {
        let pass = dvec2(800.0, 600.0);
        let close = |a: Rect, b: Rect| (a.pos - b.pos).length() < 1e-3 && (a.size - b.size).length() < 1e-3;
        for side in [PanelEdge::Left, PanelEdge::Right, PanelEdge::Top, PanelEdge::Bottom] {
            let extent = drawer_extent(side, DialogSize::Sm, pass);
            let rest = drawer_panel_rect(side, extent, pass, 1.0);
            assert_eq!(rest_to_drawn(rest, rest).v, Mat4f::identity().v, "{side:?} at rest");
            for v in [0.0, 0.3, 0.5, 1.2] {
                let drawn = drawer_panel_rect(side, extent, pass, v);
                let got = carried(&rest_to_drawn(rest, drawn), rest);
                assert!(close(got, drawn), "{side:?} at {v}: {got:?} is not {drawn:?}");
            }
        }
        let rest = Rect { pos: dvec2(40.0, 100.0), size: dvec2(240.0, 138.0) };
        for above in [false, true] {
            for v in [0.25, 0.5, 1.1] {
                let (offset, height) = drop_panel_extent(rest.size.y, v, above);
                let drawn = Rect { pos: rest.pos + dvec2(0.0, offset), size: dvec2(rest.size.x, height) };
                let got = carried(&rest_to_drawn(rest, drawn), rest);
                assert!(close(got, drawn), "a drop panel {v} out, above {above}: {got:?} is not {drawn:?}");
            }
        }
        let flat = rest_to_drawn(rest, Rect { pos: rest.pos, size: dvec2(rest.size.x, 0.0) });
        assert!((flat.v[5] as f64 - MIN_SCALE).abs() < 1e-9, "drawn to nothing, drawn very small: {}", flat.v[5]);
        assert!(flat.v.iter().all(|value| value.is_finite()));
        });
    }

    /// A wide window shows the row and no button; a narrow one the button
    /// and no row, decided on the draw itself, and the three lists hold the
    /// same destinations whichever is showing.
    #[test]
    fn the_width_decides_between_the_row_and_the_button() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{labels: ["Home" "Library" "History" "Settings"]}
        });
        let mut target = Target::new(&mut cx);
        let actions = cx.capture_actions(|cx| target.draw(cx, &root, dvec2(1000.0, 400.0)));
        assert!(reports(&actions).is_empty(), "a menu that starts in a row has not changed mode");
        {
            let inner = root.borrow::<HamburgerMenu>().unwrap();
            assert!(!inner.is_collapsed());
            let row = inner.part(live_id!(inline_nav)).area();
            assert!(row.is_valid(&cx) && row.rect(&cx).size.x > 0.0, "the row is drawn");
            assert!(!inner.burger(&cx).area().is_valid(&cx), "the button is not");
            for list in inner.lists(&cx) {
                assert_eq!(list.as_nav_list().count(), 4);
                assert_eq!(list.as_nav_list().selected(), Some(LiveId(1)));
            }
            assert_eq!(inner.text(), "Home");
            assert_eq!(inner.snapshot_value(&cx).as_deref(), Some("inline"));
        }

        let actions = cx.capture_actions(|cx| target.draw(cx, &root, dvec2(500.0, 400.0)));
        assert_eq!(reports(&actions), vec![HamburgerAction::ModeChanged(true)]);
        let inner = root.borrow::<HamburgerMenu>().unwrap();
        assert!(inner.is_collapsed());
        let burger = inner.burger(&cx).area();
        assert!(burger.is_valid(&cx) && burger.rect(&cx).size.x > 0.0, "the button is drawn");
        assert!(!inner.part(live_id!(inline_nav)).area().is_valid(&cx), "the row is not");
        assert_eq!(inner.snapshot_value(&cx).as_deref(), Some("collapsed"));
        });
    }

    /// A pick in the drawer's list lights the row and the drop list too,
    /// and the menu reports it once, under its own name.
    #[test]
    fn a_pick_in_one_list_lights_the_other_two() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed labels: ["Home" "Library" "History"]}
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(800.0, 400.0));
        let drawer_nav = root.borrow::<HamburgerMenu>().unwrap().widget(&cx, ids!(drawer_nav));
        let pick = Event::Actions(vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(NavAction::Selected(LiveId(3))),
            widget_uid: drawer_nav.widget_uid(),
            group: None,
        })]);
        let actions = cx.capture_actions(|cx| root.handle_event(cx, &pick, &mut Scope::empty()));
        assert_eq!(reports(&actions), vec![HamburgerAction::Selected(LiveId(3))]);
        let inner = root.borrow::<HamburgerMenu>().unwrap();
        assert_eq!(inner.selected(), Some(LiveId(3)));
        assert_eq!(inner.text(), "History");
        let [row, _, drop] = inner.lists(&cx);
        assert_eq!(row.as_nav_list().selected(), Some(LiveId(3)));
        assert_eq!(drop.as_nav_list().selected(), Some(LiveId(3)));
        });
    }

    /// Opening follows the surface and the cross follows the opening, both
    /// ways, with one report each.
    #[test]
    fn the_burger_turns_with_the_surface_it_opened() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed reduced_motion: true labels: ["Home" "Library"]}
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(800.0, 400.0));
        let menu = root.as_hamburger_menu();

        let actions = cx.capture_actions(|cx| menu.open(cx));
        assert_eq!(reports(&actions), vec![HamburgerAction::Opened]);
        {
            let inner = root.borrow::<HamburgerMenu>().unwrap();
            assert!(inner.part(live_id!(drawer)).as_dialog().is_open());
            assert!(inner.burger(&cx).as_button().open(), "the bars are a cross");
            assert_eq!(inner.snapshot_value(&cx).as_deref(), Some("collapsed open drawer"));
            // The drawer's own slide is cut to its floor: the menu moves the panel.
            assert_eq!(inner.part(live_id!(drawer)).borrow::<Dialog>().unwrap().slide_secs, DRAWER_SLIDE_FLOOR_SECS);
        }
        let actions = cx.capture_actions(|cx| menu.close(cx));
        assert_eq!(reports(&actions), vec![HamburgerAction::Closed]);
        {
            let inner = root.borrow::<HamburgerMenu>().unwrap();
            assert!(!inner.part(live_id!(drawer)).as_dialog().is_open());
            assert!(!inner.burger(&cx).as_button().open(), "the cross is bars again");
        }
        // A close with nothing out reports nothing.
        let actions = cx.capture_actions(|cx| menu.close(cx));
        assert!(reports(&actions).is_empty());

        if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
            inner.surface = HamburgerSurface::Drop;
        }
        target.draw(&mut cx, &root, dvec2(800.0, 400.0));
        let actions = cx.capture_actions(|cx| menu.open(cx));
        assert_eq!(reports(&actions), vec![HamburgerAction::Opened]);
        let inner = root.borrow::<HamburgerMenu>().unwrap();
        assert!(inner.part(live_id!(burger_pop)).as_popover().is_open());
        assert_eq!(inner.snapshot_value(&cx).as_deref(), Some("collapsed open drop"));
        assert!(!inner.part(live_id!(drawer)).as_dialog().is_open(), "the drawer stayed shut");
        });
    }

    /// A panel that is out when the window grows past the breakpoint goes
    /// away on the draw that unfolds the menu, and says so.
    #[test]
    fn unfolding_puts_an_open_panel_away() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{labels: ["Home" "Library"]}
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(500.0, 400.0));
        let menu = root.as_hamburger_menu();
        assert!(menu.is_collapsed());
        menu.open(&mut cx);
        assert!(menu.is_open());

        // Inside the band: still folded, still open.
        target.draw(&mut cx, &root, dvec2(730.0, 400.0));
        assert!(menu.is_collapsed() && menu.is_open());

        let actions = cx.capture_actions(|cx| target.draw(cx, &root, dvec2(1000.0, 400.0)));
        let reported = reports(&actions);
        assert!(reported.contains(&HamburgerAction::ModeChanged(false)), "{reported:?}");
        assert!(reported.contains(&HamburgerAction::Closed), "{reported:?}");
        assert!(!menu.is_collapsed() && !menu.is_open());
        let inner = root.borrow::<HamburgerMenu>().unwrap();
        assert!(!inner.part(live_id!(drawer)).as_dialog().is_open());
        assert!(!inner.burger(&cx).as_button().open());
        });
    }

    /// Moves the key focus the way the event loop does between events.
    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    /// One event to `root`, then the actions it raised, round after round
    /// as the event loop delivers them, with the focus settled after; the
    /// menu's own reports from all of it.
    fn deliver(cx: &mut Cx, root: &WidgetRef, event: &Event) -> Vec<HamburgerAction> {
        let mut reported = Vec::new();
        let mut actions = cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()));
        while !actions.is_empty() {
            reported.extend(reports(&actions));
            let round = Event::Actions(actions);
            actions = cx.capture_actions(|cx| root.handle_event(cx, &round, &mut Scope::empty()));
        }
        settle_focus(cx);
        reported
    }

    fn key(key_code: KeyCode) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 0.0,
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

    fn next_frame(frame: NextFrame) -> Event {
        Event::NextFrame(NextFrameEvent { frame: 1, time: 0.0, set: HashSet::from([frame]) })
    }

    /// Put a travel `x` of the way through by moving when it started: a test
    /// cannot wait out a real duration.
    fn travel_to(cx: &mut Cx, root: &WidgetRef, x: f64) {
        let now = cx.seconds_since_app_start();
        if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
            if let Some(travel) = inner.travel.as_mut() {
                travel.started = now - travel.secs * x;
            }
        }
    }

    /// Long travels, so the time a test takes between moving the clock and
    /// drawing moves the panel by nothing worth measuring.
    fn slow(root: &WidgetRef) {
        if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
            inner.enter_secs = 1000.0;
            inner.exit_secs = 1000.0;
        }
    }

    /// The travel's next frame, delivered once it has arrived.
    fn finish_travel(cx: &mut Cx, root: &WidgetRef) {
        travel_to(cx, root, 1.0);
        let frame = root.borrow::<HamburgerMenu>().unwrap().travel_frame;
        deliver(cx, root, &next_frame(frame));
        assert!(root.borrow::<HamburgerMenu>().unwrap().travel.is_none(), "the travel is over");
    }

    /// Whether a part was drawn at the last draw, where, and on which list.
    fn drawn(cx: &Cx, part: &WidgetRef) -> Option<(Rect, DrawListId)> {
        match part.area() {
            Area::Instance(instance) if part.area().is_valid(cx) => Some((part.area().rect(cx), instance.draw_list_id)),
            _ => None,
        }
    }

    fn leaving_list(root: &WidgetRef) -> DrawListId {
        root.borrow::<HamburgerMenu>().unwrap().leaving.as_ref().expect("a leaving overlay").draw_list_id()
    }

    fn turn(root: &WidgetRef) -> f64 {
        root.borrow::<HamburgerMenu>().unwrap().turn_written
    }

    /// Where a transform draws `rect`.
    fn carried(m: &Mat4f, rect: Rect) -> Rect {
        let (sx, sy) = (m.v[0] as f64, m.v[5] as f64);
        Rect {
            pos: dvec2(rect.pos.x * sx + m.v[12] as f64, rect.pos.y * sy + m.v[13] as f64),
            size: dvec2(rect.size.x * sx, rect.size.y * sy),
        }
    }

    /// The transform the part named `motion` carries its panel with.
    fn motion_transform(cx: &Cx, root: &WidgetRef, motion: &[LiveId]) -> Mat4f {
        let motion = root.borrow::<HamburgerMenu>().unwrap().widget(cx, motion);
        let transform = motion.borrow::<HamburgerMotion>().expect("a HamburgerMotion").transform;
        transform
    }

    fn leaving_transform(cx: &Cx, root: &WidgetRef) -> Mat4f {
        root.borrow::<HamburgerMenu>().unwrap().leaving.as_ref().expect("a leaving overlay").get_view_transform(cx)
    }

    /// The drawer's panel comes in along the enter easing with the bars
    /// turning on the same curve. On a close the drawer lets go at once and
    /// the panel leaves along the exit easing on the menu's own overlay,
    /// where a press goes straight past it, and it is gone once it has left.
    #[test]
    fn the_drawer_panel_comes_and_goes_on_the_menus_easings() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed labels: ["Home" "Library" "History"]}
        });
        slow(&root);
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, size);
        let menu = root.as_hamburger_menu();
        let (enter, exit) = {
            let inner = root.borrow::<HamburgerMenu>().unwrap();
            (inner.enter_ease, inner.exit_ease)
        };
        let sheet = root.borrow::<HamburgerMenu>().unwrap().widget(&cx, ids!(drawer_sheet));
        let rest = drawer_panel_rect(PanelEdge::Left, 260.0, size, 1.0);

        menu.open(&mut cx);
        travel_to(&mut cx, &root, 0.5);
        target.draw(&mut cx, &root, size);
        let v = enter.map(0.5);
        let want = drawer_panel_rect(PanelEdge::Left, 260.0, size, v);
        let (rect, list) = drawn(&cx, &sheet).expect("the drawer draws its panel");
        assert_eq!(rect, rest, "laid out where it rests, for the hit tests");
        let shown = carried(&motion_transform(&cx, &root, ids!(drawer_motion)), rect);
        assert!((shown.pos.x - want.pos.x).abs() < 0.01 && (shown.size - want.size).length() < 0.01, "drawn at {shown:?}, not {want:?}");
        assert!(shown.pos.x < 0.0 && shown.pos.x > -260.0, "half way along the curve is part way in");
        assert_ne!(list, leaving_list(&root), "an open drawer draws its own panel");
        assert!((turn(&root) - v).abs() < 1e-3, "the bars turn with the panel: {} against {v}", turn(&root));
        assert!(root.borrow::<HamburgerMenu>().unwrap().burger(&cx).as_button().open());

        travel_to(&mut cx, &root, 1.0);
        target.draw(&mut cx, &root, size);
        assert_eq!(drawn(&cx, &sheet).map(|(rect, _)| rect), Some(rest), "in place");
        assert_eq!(motion_transform(&cx, &root, ids!(drawer_motion)).v, Mat4f::identity().v, "drawn where it rests");
        assert_eq!(turn(&root), 1.0, "a cross");

        menu.close(&mut cx);
        assert!(!root.borrow::<HamburgerMenu>().unwrap().part(live_id!(drawer)).as_dialog().is_open(), "the drawer let go at once");
        travel_to(&mut cx, &root, 0.5);
        target.draw(&mut cx, &root, size);
        let v = 1.0 - exit.map(0.5);
        let want = drawer_panel_rect(PanelEdge::Left, 260.0, size, v);
        let (rect, list) = drawn(&cx, &sheet).expect("the panel is drawn on its way out");
        assert_eq!(rect, rest, "laid out where it rested");
        let shown = carried(&leaving_transform(&cx, &root), rect);
        assert!((shown.pos.x - want.pos.x).abs() < 0.01, "drawn at {shown:?}, not {want:?}");
        assert_eq!(list, leaving_list(&root), "on the menu's own overlay");
        assert!((turn(&root) - v).abs() < 1e-3, "the cross turns back with the panel");
        let rect = shown;

        // A press on the leaving panel is not taken by it.
        let press_event = press(dvec2(rect.pos.x + rect.size.x - 20.0, 300.0));
        let actions = cx.capture_actions(|cx| root.handle_event(cx, &press_event, &mut Scope::empty()));
        assert!(reports(&actions).is_empty());
        if let Event::MouseDown(me) = &press_event {
            assert!(me.handled.get().is_empty(), "the leaving panel took the press");
        }
        deliver(&mut cx, &root, &release(dvec2(rect.pos.x + rect.size.x - 20.0, 300.0)));

        finish_travel(&mut cx, &root);
        target.draw(&mut cx, &root, size);
        assert!(drawn(&cx, &sheet).is_none(), "gone once it has left");
        assert_eq!(turn(&root), 0.0, "bars");
        assert!(!root.borrow::<HamburgerMenu>().unwrap().burger(&cx).as_button().open());
        });
    }

    /// The drop panel grows from the button to its resting height while the
    /// popover holds that height for placing and hit-testing, and shrinks
    /// back on the menu's overlay after the popover has closed.
    #[test]
    fn the_drop_panel_grows_from_the_button_and_shrinks_back() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{
                mode: HamburgerMode.Collapsed
                surface: HamburgerSurface.Drop
                labels: ["Home" "Library" "History"]
            }
        });
        slow(&root);
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, size);
        let menu = root.as_hamburger_menu();
        let (enter, exit) = {
            let inner = root.borrow::<HamburgerMenu>().unwrap();
            (inner.enter_ease, inner.exit_ease)
        };
        let holder = root.borrow::<HamburgerMenu>().unwrap().part(live_id!(burger_pop)).as_popover().content();
        let sheet = holder.widget(&cx, ids!(drop_sheet));

        menu.open(&mut cx);
        // The first frame lays the rows out; the next reads their height
        // before it places the panel.
        target.draw(&mut cx, &root, size);
        travel_to(&mut cx, &root, 0.5);
        target.draw(&mut cx, &root, size);
        let rest = root.borrow::<HamburgerMenu>().unwrap().drop_rest_height.expect("the resting height was read");
        assert!(rest > 3.0 * 16.0, "three rows and the padding: {rest}");
        let (laid, _) = drawn(&cx, &sheet).expect("the popover draws the panel");
        let held = holder.area().rect(&cx);
        assert!((laid.size.y - rest).abs() < 0.01, "laid out at its resting height: {}", laid.size.y);
        let grown = carried(&motion_transform(&cx, &root, ids!(drop_motion)), laid);
        assert!((grown.size.y - rest * enter.map(0.5)).abs() < 0.01, "{} is not {}", grown.size.y, rest * enter.map(0.5));
        assert!((held.size.y - rest).abs() < 0.01, "the popover holds the resting height: {}", held.size.y);
        assert!((grown.pos.y - held.pos.y).abs() < 0.01, "it grows down from the button");
        let burger = root.borrow::<HamburgerMenu>().unwrap().burger(&cx).area().rect(&cx);
        assert!(held.pos.y >= burger.pos.y + burger.size.y, "it hangs below the button");

        travel_to(&mut cx, &root, 1.0);
        target.draw(&mut cx, &root, size);
        let (full, _) = drawn(&cx, &sheet).unwrap();
        assert!((full.size.y - rest).abs() < 0.01, "at rest it is as tall as its rows: {}", full.size.y);
        assert_eq!(motion_transform(&cx, &root, ids!(drop_motion)).v, Mat4f::identity().v, "drawn where it rests");

        menu.close(&mut cx);
        assert!(!root.borrow::<HamburgerMenu>().unwrap().part(live_id!(burger_pop)).as_popover().is_open(), "the popover let go at once");
        travel_to(&mut cx, &root, 0.5);
        target.draw(&mut cx, &root, size);
        let (laid, list) = drawn(&cx, &sheet).expect("the panel is drawn on its way out");
        assert_eq!(list, leaving_list(&root), "on the menu's own overlay");
        assert_eq!(laid.pos, full.pos, "laid out where it rested");
        let leaving = carried(&leaving_transform(&cx, &root), laid);
        assert!((leaving.size.y - rest * (1.0 - exit.map(0.5))).abs() < 0.01);
        assert!((leaving.pos - full.pos).length() < 0.01, "shrinking back to the button");

        finish_travel(&mut cx, &root);
        target.draw(&mut cx, &root, size);
        assert!(drawn(&cx, &sheet).is_none(), "gone once it has left");
        });
    }

    /// Opened again on its way out, the panel turns round from where it had
    /// got to, and the drawer draws it again.
    #[test]
    fn a_panel_called_back_on_its_way_out_turns_round_where_it_is() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed labels: ["Home" "Library"]}
        });
        slow(&root);
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, size);
        let menu = root.as_hamburger_menu();
        menu.open(&mut cx);
        finish_travel(&mut cx, &root);
        target.draw(&mut cx, &root, size);
        menu.close(&mut cx);
        travel_to(&mut cx, &root, 0.5);
        let now = cx.seconds_since_app_start();
        let out = root.borrow::<HamburgerMenu>().unwrap().travel.unwrap().at(now);
        assert!(out > 0.0 && out < 1.0, "part of the way out: {out}");

        menu.open(&mut cx);
        let travel = root.borrow::<HamburgerMenu>().unwrap().travel.expect("on its way back");
        assert_eq!((travel.surface, travel.to), (HamburgerSurface::Drawer, 1.0));
        assert!((travel.from - out).abs() < 1e-3, "from {} rather than {out}", travel.from);
        target.draw(&mut cx, &root, size);
        let sheet = root.borrow::<HamburgerMenu>().unwrap().widget(&cx, ids!(drawer_sheet));
        let (rect, list) = drawn(&cx, &sheet).expect("drawn");
        assert_ne!(list, leaving_list(&root), "the open drawer draws it again");
        let want = drawer_panel_rect(PanelEdge::Left, 260.0, size, out);
        let shown = carried(&motion_transform(&cx, &root, ids!(drawer_motion)), rect);
        assert!((shown.pos.x - want.pos.x).abs() < 0.5, "{shown:?} is not {want:?}");
        });
    }

    /// While the panel is still coming out, a press where a row will rest
    /// lands on that row: in a drawer sliding in from the right, whose rows
    /// are drawn well short of where they rest, and in a drop panel whose
    /// last row has not grown into view yet.
    #[test]
    fn a_press_lands_on_the_row_where_it_rests_while_the_panel_comes_out() {
        crate::on_test_cx(|| {
        for surface in [HamburgerSurface::Drawer, HamburgerSurface::Drop] {
            let mut cx = cx();
            let root = menu(&mut cx, script! {
                use mod.prelude.widgets.*
                HamburgerMenu{mode: HamburgerMode.Collapsed labels: ["Home" "Library" "History"]}
            });
            slow(&root);
            if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
                inner.surface = surface;
                inner.drawer_side = PanelEdge::Right;
            }
            let size = dvec2(800.0, 600.0);
            let mut target = Target::new(&mut cx);
            target.draw(&mut cx, &root, size);
            let menu = root.as_hamburger_menu();
            let _ = cx.capture_actions(|cx| menu.open(cx));
            target.draw(&mut cx, &root, size);
            // Early in the travel, when the panel is drawn far short of where
            // it rests.
            travel_to(&mut cx, &root, 0.02);
            target.draw(&mut cx, &root, size);
            settle_focus(&mut cx);

            let nav = match surface {
                HamburgerSurface::Drawer => ids!(drawer_nav),
                HamburgerSurface::Drop => ids!(drop_nav),
            };
            let list = root.borrow::<HamburgerMenu>().unwrap().widget(&cx, nav).area().rect(&cx);
            let (at, id, drawn_short) = match surface {
                // The middle row, half way across where the drawer rests.
                HamburgerSurface::Drawer => {
                    let rest = drawer_panel_rect(PanelEdge::Right, 260.0, size, 1.0);
                    let at = dvec2(rest.pos.x + 40.0, list.pos.y + list.size.y * 0.5);
                    let drawn = carried(&motion_transform(&cx, &root, ids!(drawer_motion)), rest);
                    (at, LiveId(2), drawn.pos.x > at.x)
                }
                // The last row, below where the growing panel reaches.
                HamburgerSurface::Drop => {
                    let at = list.pos + dvec2(list.size.x * 0.5, list.size.y * 5.0 / 6.0);
                    let holder = root.borrow::<HamburgerMenu>().unwrap().part(live_id!(burger_pop)).as_popover().content();
                    let drawn = carried(&motion_transform(&cx, &root, ids!(drop_motion)), holder.area().rect(&cx));
                    (at, LiveId(3), drawn.pos.y + drawn.size.y < at.y)
                }
            };
            assert!(drawn_short, "{surface:?}: the panel is drawn short of the row that will rest at {at:?}");
            deliver(&mut cx, &root, &press(at));
            let reported = deliver(&mut cx, &root, &release(at));
            assert_eq!(reported, vec![HamburgerAction::Selected(id)], "{surface:?}: the press did not land on the resting row");
            assert!(root.borrow::<HamburgerMenu>().unwrap().close_frame.is_some(), "{surface:?}: a pick on the list owes a close");
            assert!(menu.is_open(), "{surface:?}: not taken for a press outside");
        }
        });
    }

    /// The drop panel hangs its 4 points below the button itself, not below
    /// the button's margin.
    #[test]
    fn the_drop_panel_hangs_four_points_below_the_button() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed surface: HamburgerSurface.Drop reduced_motion: true labels: ["Home" "Library"]}
        });
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, size);
        let menu = root.as_hamburger_menu();
        menu.open(&mut cx);
        target.draw(&mut cx, &root, size);
        target.draw(&mut cx, &root, size);
        let inner = root.borrow::<HamburgerMenu>().unwrap();
        let burger = inner.burger(&cx).area().rect(&cx);
        let holder = inner.part(live_id!(burger_pop)).as_popover().content().area().rect(&cx);
        assert!(burger.size.y > 0.0 && holder.size.y > 0.0);
        let gap = holder.pos.y - (burger.pos.y + burger.size.y);
        assert!((gap - 4.0).abs() < 0.01, "the panel hangs {gap} points below the button");
        let wrapper = inner.part(live_id!(burger_pop)).area().rect(&cx);
        assert_eq!(wrapper, burger, "the popover's rect is the button's");
        });
    }

    /// A panel brought out from the burger, by a press on it or by Return,
    /// gives the keyboard back to the burger when it goes away, though the
    /// page has been drawn again meanwhile, so Return brings it out again.
    #[test]
    fn the_keyboard_is_back_on_the_burger_after_the_panel_goes_away() {
        crate::on_test_cx(|| {
        for surface in [HamburgerSurface::Drawer, HamburgerSurface::Drop] {
            let mut cx = cx();
            let root = menu(&mut cx, script! {
                use mod.prelude.widgets.*
                HamburgerMenu{mode: HamburgerMode.Collapsed reduced_motion: true labels: ["Home" "Library" "History"]}
            });
            if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
                inner.surface = surface;
            }
            let size = dvec2(800.0, 600.0);
            let mut target = Target::new(&mut cx);
            target.draw(&mut cx, &root, size);
            let menu = root.as_hamburger_menu();
            let burger = || root.borrow::<HamburgerMenu>().unwrap().burger(&cx).area();
            let on_burger = {
                let rect = burger().rect(&cx);
                rect.pos + rect.size * 0.5
            };

            deliver(&mut cx, &root, &press(on_burger));
            deliver(&mut cx, &root, &release(on_burger));
            assert!(menu.is_open(), "{surface:?}: a press on the burger brings the panel out");
            target.draw(&mut cx, &root, size);
            settle_focus(&mut cx);
            root.redraw(&mut cx);
            target.draw(&mut cx, &root, size);
            settle_focus(&mut cx);
            assert!(!cx.has_key_focus(root.borrow::<HamburgerMenu>().unwrap().burger(&cx).area()), "{surface:?}: the panel has the keyboard");

            deliver(&mut cx, &root, &key(KeyCode::ReturnKey));
            let frame = root.borrow::<HamburgerMenu>().unwrap().close_frame.expect("Return in the list owes a close");
            deliver(&mut cx, &root, &next_frame(frame));
            assert!(!menu.is_open(), "{surface:?}: Return put the panel away");
            target.draw(&mut cx, &root, size);
            settle_focus(&mut cx);
            let burger_now = root.borrow::<HamburgerMenu>().unwrap().burger(&cx).area();
            assert!(cx.has_key_focus(burger_now), "{surface:?}: the keyboard is at {:?}, not the burger {burger_now:?}", cx.key_focus());

            deliver(&mut cx, &root, &key(KeyCode::ReturnKey));
            assert!(menu.is_open(), "{surface:?}: Return on the burger brings the panel out again");
            target.draw(&mut cx, &root, size);
            settle_focus(&mut cx);
            menu.close(&mut cx);
            settle_focus(&mut cx);
            let burger_now = root.borrow::<HamburgerMenu>().unwrap().burger(&cx).area();
            assert!(cx.has_key_focus(burger_now), "{surface:?}: opened with Return, the keyboard comes back too");
        }
        });
    }

    /// A page rebuilt while its menu's drawer is out (a theme switch) takes
    /// the drawer with it; the next event gives the pointer and the wheel
    /// back to the page that replaced it.
    #[test]
    fn a_menu_rebuilt_with_its_drawer_out_leaves_the_pointer_and_the_wheel_free() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let source = || script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed reduced_motion: true labels: ["Home" "Library"]}
        };
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        let old = menu(&mut cx, source());
        target.draw(&mut cx, &old, size);
        old.as_hamburger_menu().open(&mut cx);
        target.draw(&mut cx, &old, size);
        assert!(cx.sweep_lock_area().is_some(), "the open drawer holds the pointer");
        drop(old);

        let new = menu(&mut cx, source());
        target.draw(&mut cx, &new, size);
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        crate::modal::release_orphaned_scroll_blocks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the drawer that went still holds the pointer");
        let burger = new.borrow::<HamburgerMenu>().unwrap().burger(&cx).area();
        assert!(cx.is_scrolling_allowed_within(&burger), "the drawer that went still blocks the wheel");
        });
    }

    /// With motion reduced the panel and the bars land at once, on both
    /// surfaces, and nothing is left leaving after a close.
    #[test]
    fn reduced_motion_lands_the_panel_and_the_bars_at_once() {
        crate::on_test_cx(|| {
        for surface in [HamburgerSurface::Drawer, HamburgerSurface::Drop] {
            let mut cx = cx();
            let root = menu(&mut cx, script! {
                use mod.prelude.widgets.*
                HamburgerMenu{mode: HamburgerMode.Collapsed reduced_motion: true labels: ["Home" "Library"]}
            });
            if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
                inner.surface = surface;
            }
            let size = dvec2(800.0, 600.0);
            let mut target = Target::new(&mut cx);
            target.draw(&mut cx, &root, size);
            let menu = root.as_hamburger_menu();
            let sheet = match surface {
                HamburgerSurface::Drawer => root.borrow::<HamburgerMenu>().unwrap().widget(&cx, ids!(drawer_sheet)),
                HamburgerSurface::Drop => root
                    .borrow::<HamburgerMenu>()
                    .unwrap()
                    .part(live_id!(burger_pop))
                    .as_popover()
                    .content()
                    .widget(&cx, ids!(drop_sheet)),
            };

            menu.open(&mut cx);
            assert!(root.borrow::<HamburgerMenu>().unwrap().travel.is_none(), "{surface:?}: no travel in");
            target.draw(&mut cx, &root, size);
            assert_eq!(turn(&root), 1.0, "{surface:?}: a cross at once");
            let (rect, _) = drawn(&cx, &sheet).expect("drawn");
            if surface == HamburgerSurface::Drawer {
                assert_eq!(rect, drawer_panel_rect(PanelEdge::Left, 260.0, size, 1.0), "in place at once");
            } else {
                target.draw(&mut cx, &root, size);
                let rest = root.borrow::<HamburgerMenu>().unwrap().drop_rest_height.expect("the resting height was read");
                assert!((rect.size.y - rest).abs() < 0.01, "at its full height at once");
            }

            menu.close(&mut cx);
            assert!(root.borrow::<HamburgerMenu>().unwrap().travel.is_none(), "{surface:?}: no travel out");
            target.draw(&mut cx, &root, size);
            assert!(drawn(&cx, &sheet).is_none(), "{surface:?}: nothing left leaving");
            assert_eq!(turn(&root), 0.0, "{surface:?}: bars at once");
        }
        });
    }

    /// In the drawer the arrows reach the list at once, move the lit
    /// destination and leave the drawer out; Return puts it away, on the
    /// frame after, with the choice the arrows made kept.
    #[test]
    fn arrows_look_through_the_drawer_and_return_puts_it_away() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Collapsed labels: ["Home" "Library" "History"]}
        });
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, size);
        let menu = root.as_hamburger_menu();
        let _ = cx.capture_actions(|cx| menu.open(cx));
        // The draw that gives the drawer's list an area, and so the keyboard.
        target.draw(&mut cx, &root, size);
        settle_focus(&mut cx);
        let nav = root.borrow::<HamburgerMenu>().unwrap().widget(&cx, ids!(drawer_nav));
        assert!(cx.has_key_focus(nav.area()), "the drawer's list holds the keyboard");

        let reported = deliver(&mut cx, &root, &key(KeyCode::ArrowDown));
        assert_eq!(reported, vec![HamburgerAction::Selected(LiveId(2))]);
        assert!(menu.is_open(), "an arrow looks; it does not put the drawer away");
        assert!(root.borrow::<HamburgerMenu>().unwrap().close_frame.is_none());
        let row = root.borrow::<HamburgerMenu>().unwrap().part(live_id!(inline_nav));
        assert_eq!(row.as_nav_list().selected(), Some(LiveId(2)), "the row followed");

        let reported = deliver(&mut cx, &root, &key(KeyCode::ReturnKey));
        assert!(reported.is_empty(), "{reported:?}");
        let frame = root.borrow::<HamburgerMenu>().unwrap().close_frame.expect("Return owes a close");
        assert!(menu.is_open(), "the close waits for the next frame");
        let reported = deliver(&mut cx, &root, &next_frame(frame));
        assert_eq!(reported, vec![HamburgerAction::Closed]);
        assert!(!menu.is_open());
        assert_eq!(menu.selected(), Some(LiveId(2)));

        // With close on pick off, Return leaves it out.
        if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
            inner.close_on_pick = false;
        }
        let _ = cx.capture_actions(|cx| menu.open(cx));
        target.draw(&mut cx, &root, size);
        settle_focus(&mut cx);
        deliver(&mut cx, &root, &key(KeyCode::ReturnKey));
        assert!(root.borrow::<HamburgerMenu>().unwrap().close_frame.is_none());
        assert!(menu.is_open());
        });
    }

    /// A press and release on the drop panel's list chooses and puts the
    /// panel away on the frame after; a press outside puts it away at once.
    #[test]
    fn a_press_on_the_list_puts_the_drop_panel_away_after_the_choice() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{
                mode: HamburgerMode.Collapsed
                surface: HamburgerSurface.Drop
                labels: ["Home" "Library" "History"]
            }
        });
        let size = dvec2(800.0, 600.0);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, size);
        let menu = root.as_hamburger_menu();
        let _ = cx.capture_actions(|cx| menu.open(cx));
        target.draw(&mut cx, &root, size);
        settle_focus(&mut cx);
        assert!(menu.is_open());
        // Grown all the way, so the rows are not cut by the growing edge.
        travel_to(&mut cx, &root, 1.0);
        target.draw(&mut cx, &root, size);

        let list = root.borrow::<HamburgerMenu>().unwrap().widget(&cx, ids!(drop_nav)).area().rect(&cx);
        assert!(list.size.x > 0.0, "the drop panel's list is drawn");
        let on_list = list.pos + list.size * 0.5;
        deliver(&mut cx, &root, &press(on_list));
        let reported = deliver(&mut cx, &root, &release(on_list));
        // The middle of three rows is the second destination.
        assert_eq!(reported, vec![HamburgerAction::Selected(LiveId(2))], "the press chose");
        let frame = root.borrow::<HamburgerMenu>().unwrap().close_frame.expect("a press on the list owes a close");
        assert!(menu.is_open(), "the close waits for the choice to be reported");
        let reported = deliver(&mut cx, &root, &next_frame(frame));
        assert_eq!(reported, vec![HamburgerAction::Closed]);
        assert!(!menu.is_open());
        assert!(!root.borrow::<HamburgerMenu>().unwrap().part(live_id!(burger_pop)).as_popover().is_open());

        let _ = cx.capture_actions(|cx| menu.open(cx));
        target.draw(&mut cx, &root, size);
        settle_focus(&mut cx);
        let reported = deliver(&mut cx, &root, &press(dvec2(size.x - 10.0, size.y - 10.0)));
        assert_eq!(reported, vec![HamburgerAction::Closed], "a press outside puts it away at once");
        assert!(!menu.is_open());
        assert!(root.borrow::<HamburgerMenu>().unwrap().close_frame.is_none());
        });
    }

    /// While the row is showing there is no panel to open.
    #[test]
    fn a_menu_in_a_row_does_not_open() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = menu(&mut cx, script! {
            use mod.prelude.widgets.*
            HamburgerMenu{mode: HamburgerMode.Inline labels: ["Home" "Library"]}
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(300.0, 400.0));
        let menu = root.as_hamburger_menu();
        let actions = cx.capture_actions(|cx| menu.open(cx));
        assert!(reports(&actions).is_empty());
        assert!(!menu.is_open());
        });
    }

    /// Hidden while a panel is out, the menu puts the panel away at once. A
    /// view draws no hidden child and a hidden menu hears no press, so a
    /// panel left out would hold the pointer with nothing to dismiss it.
    #[test]
    fn hiding_an_open_menu_puts_its_panel_away() {
        crate::on_test_cx(|| {
        for surface in [HamburgerSurface::Drawer, HamburgerSurface::Drop] {
            let mut cx = cx();
            let root = menu(&mut cx, script! {
                use mod.prelude.widgets.*
                HamburgerMenu{mode: HamburgerMode.Collapsed labels: ["Home" "Library"]}
            });
            if let Some(mut inner) = root.borrow_mut::<HamburgerMenu>() {
                inner.surface = surface;
            }
            let mut target = Target::new(&mut cx);
            target.draw(&mut cx, &root, dvec2(800.0, 400.0));
            let menu = root.as_hamburger_menu();
            menu.open(&mut cx);
            target.draw(&mut cx, &root, dvec2(800.0, 400.0));
            assert!(menu.is_open(), "{surface:?}");

            let actions = cx.capture_actions(|cx| root.set_visible(cx, false));
            assert_eq!(reports(&actions), vec![HamburgerAction::Closed], "{surface:?}");
            assert!(!menu.is_open(), "{surface:?}");
            {
                let inner = root.borrow::<HamburgerMenu>().unwrap();
                assert!(!inner.part(live_id!(drawer)).as_dialog().is_open(), "{surface:?}");
                assert!(!inner.part(live_id!(burger_pop)).as_popover().is_open(), "{surface:?}");
                assert!(!inner.burger(&cx).as_button().open(), "{surface:?}: the cross is bars again");
            }
            assert_eq!(cx.sweep_lock_area(), None, "{surface:?}: nothing is left holding the pointer");
        }
        });
    }
}
