//! Drawer — the panel that comes in from an edge and goes back to it.
//!
//! A drawer is a dialog that has chosen a side. It stops the work the same
//! way — scrim, pointer taken, keyboard taken — but it arrives from an edge
//! and it is shaped by that edge: a left or right drawer is a column with a
//! width, a top or bottom one is a row with a height. Navigation, filters
//! and a long list of settings belong here rather than in a centred card,
//! because they are places rather than questions.
//!
//! **The side decides the shape, and the size means different things on
//! different sides.** `size` is a width for a drawer on the left or right
//! and a height for one on the top or bottom, which is why the rungs are
//! named for how much room they take rather than for a number.
//!
//! **A sheet is a drawer from the bottom with a grabber.** That is the only
//! difference the library makes, because it is the only difference that
//! matters: the grabber is the handle, and dragging it moves the panel
//! between three rungs — a peek, half the room, and the whole of what its
//! `size` asks for — while a drag below the lowest rung sends it back the
//! way it came. A drawer without a grabber has no rungs at all: it is open
//! or it is shut, and it should not draw a handle for a thing it cannot do.
//!
//! **It leaves the way it came.** The panel slides from its edge over
//! `theme.motion_medium_2` and slides back, rather than appearing; on a
//! panel this large the movement is what says where it came from, and a
//! drawer that simply appears reads as the page having been replaced.
//!
//! Escape and a press on the scrim both dismiss, through the same claim
//! every other overlay uses, so a menu or a popover raised inside a drawer
//! closes first and one press closes one thing.

use crate::{
    button::ButtonWidgetRefExt,
    label::LabelWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    modal::Modal,
    overlay_place::claim_escape,
    widget::*,
};

/// The edge a drawer comes in from.
///
/// Named `PanelEdge` rather than `DrawerSide` on purpose: the widget derive
/// treats any field whose TYPE name begins with "Draw" as a shader layer and
/// calls `area()` on it, so a `DrawerSide` field will not compile.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum PanelEdge {
    #[pick]
    Left = 0,
    Right = 1,
    Top = 2,
    Bottom = 3,
}

impl PanelEdge {
    /// Whether the drawer is a column (left or right) rather than a row.
    pub fn is_column(self) -> bool {
        matches!(self, PanelEdge::Left | PanelEdge::Right)
    }
}

/// How much room a drawer takes: a width on the left or right, a height on
/// the top or bottom.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum PanelSize {
    /// A list of a few things.
    Sm = 0,
    /// The usual: navigation, or a set of filters.
    #[pick]
    Md = 1,
    /// A working panel.
    Lg = 2,
    /// The whole edge to edge.
    Full = 3,
}

impl PanelSize {
    /// The extent in layout points, or `None` for one that takes the room
    /// it is given.
    pub fn extent(self, column: bool) -> Option<f64> {
        match (self, column) {
            (PanelSize::Sm, true) => Some(260.0),
            (PanelSize::Md, true) => Some(360.0),
            (PanelSize::Lg, true) => Some(520.0),
            (PanelSize::Sm, false) => Some(180.0),
            (PanelSize::Md, false) => Some(300.0),
            (PanelSize::Lg, false) => Some(460.0),
            (PanelSize::Full, _) => None,
        }
    }
}

/// Where a sheet rests between drags.
///
/// Only meaningful on a panel with a grabber: a drawer without one is open
/// or shut and has nothing in between. There is deliberately no `Hidden`
/// rung — a sheet dragged below the lowest one is dismissed through the
/// same path Escape and the scrim already use, so "closed" keeps one
/// meaning and one place that decides it, rather than two that can
/// disagree about a panel the user is looking at.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum SheetDetent {
    /// A peek: the grabber, the title, and the first of what is under them.
    Collapsed = 0,
    /// Half the room the window has.
    Half = 1,
    /// The whole extent this panel's own `size` asks for.
    #[pick]
    Expanded = 2,
}

/// How tall a collapsed sheet stands: enough for the grabber, the title and
/// the first line under them. Below this it reads as a bar rather than a
/// panel, which is a different thing and not what a sheet is for.
const SHEET_COLLAPSED: f64 = 96.0;

/// How far open a sheet stands at one rung, given the extent its own `size`
/// asks for and the room the window has.
///
/// `Half` is clamped into the other two rather than taken literally: on a
/// short window half the room can be less than a peek, and on a tall one it
/// can be more than the panel ever asked for, and in both cases the rung
/// that survives is the one the sheet can actually stand at.
fn detent_extent(detent: SheetDetent, base: f64, room: f64) -> f64 {
    let collapsed = SHEET_COLLAPSED.min(base);
    match detent {
        SheetDetent::Collapsed => collapsed,
        SheetDetent::Half => (room * 0.5).clamp(collapsed, base),
        SheetDetent::Expanded => base,
    }
}

/// Which rung a dragged sheet settles at, or `None` when the drag went far
/// enough below the lowest one to mean "let go of me".
///
/// Nearest rung wins. The dismiss threshold is half the collapsed rung
/// rather than zero, so letting go of a sheet that has been pulled most of
/// the way down closes it instead of leaving a sliver on screen that the
/// pointer has already left.
fn settle_detent(extent: f64, collapsed: f64, half: f64, expanded: f64) -> Option<SheetDetent> {
    if extent < collapsed * 0.5 {
        return None;
    }
    [
        (SheetDetent::Collapsed, collapsed),
        (SheetDetent::Half, half),
        (SheetDetent::Expanded, expanded),
    ]
    .into_iter()
    .min_by(|a, b| (a.1 - extent).abs().total_cmp(&(b.1 - extent).abs()))
    .map(|(rung, _)| rung)
}

/// What a drawer reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum DrawerAction {
    Opened,
    /// Escape, the close mark, or a press on the scrim.
    Dismissed,
    /// A sheet was dragged to a new rung and let go there.
    DetentChanged(SheetDetent),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.PanelEdge = set_type_default() do #(PanelEdge::script_api(vm))
    mod.widgets.PanelSize = set_type_default() do #(PanelSize::script_api(vm))
    mod.widgets.splat(mod.widgets.PanelSize)
    // Not splatted, unlike PanelSize: a splatted variant is exported as a
    // bare name, and Collapsed/Half/Expanded are words a widget could
    // plausibly want. Written out as `SheetDetent.Half`, the way PanelEdge
    // is, it cannot shadow anything.
    mod.widgets.SheetDetent = set_type_default() do #(SheetDetent::script_api(vm))

    use mod.widgets.*

    mod.widgets.DrawerBase = #(Drawer::register_widget(vm))
    /** A panel that comes in from an edge: navigation, filters, settings. */
    mod.widgets.Drawer = set_type_default() do mod.widgets.DrawerBase{
        // The modal's own chrome, spelled out: a widget that derefs another
        // does not inherit that widget's DSL.
        flow: Overlay
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
                // The scrim is a colour and a strength: `color_scrim` alone
                // is opaque black and would hide the page rather than dim it.
                color: uniform(theme.color_scrim)
                dim: uniform(theme.state_scrim_opacity)
                pixel: fn() {
                    return vec4(self.color.xyz, self.color.a * self.dim)
                }
            }
        }

        /** which edge it comes in from: PanelEdge.Left Right Top Bottom */
        side: PanelEdge.Left
        /** how much room it takes: Sm Md Lg Full */
        size: Md
        /** Escape, the close mark and a press on the scrim all dismiss it */
        dismissable: true
        /** a grabber at the leading edge, for a panel that can be dragged */
        grabber: false
        /** which rung a sheet opens at: SheetDetent.Collapsed Half Expanded */
        detent: SheetDetent.Expanded
        /** the title, also what `text()` answers */
        title: ""
        /** how long it takes to come in and go back 0..1 step 0.01 */
        slide_secs: theme.motion_medium_2

        content := RoundedShadowView{
            width: 360.
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

            grab := View{
                width: Fill
                height: Fit
                align: Align{x: 0.5}
                padding: Inset{top: 8. bottom: 2.}
                visible: false
                RoundedView{
                    width: 36.
                    height: 4.
                    draw_bg +: {
                        color: theme.color_outline
                        border_radius: 2.
                    }
                }
            }

            header := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{y: 0.5}
                padding: Inset{left: 20. right: 12. top: 16. bottom: 8.}
                spacing: theme.space_2
                title_label := Label{
                    width: Fill
                    draw_text +: {
                        text_style: theme.font_title_s
                        color: theme.color_on_surface
                    }
                }
                close := ButtonFlat{
                    width: 24.
                    height: 24.
                    text: "\u{00d7}"
                }
            }

            body := View{
                width: Fill
                height: Fill
                flow: Down
                spacing: theme.space_2
                padding: Inset{left: 20. right: 20. top: 0. bottom: 16.}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
            }
        }
    }

    /** A drawer from the right, for a detail panel beside the work. */
    mod.widgets.SideSheet = mod.widgets.Drawer{
        side: PanelEdge.Right
    }

    /** A panel from the bottom with a grabber: the shape a phone uses for
     * everything, and a good one for a short set of choices anywhere. */
    mod.widgets.BottomSheet = mod.widgets.Drawer{
        side: PanelEdge.Bottom
        grabber: true
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Drawer {
    #[deref]
    modal: Modal,
    #[live]
    pub side: PanelEdge,
    #[live]
    pub size: PanelSize,
    #[live(true)]
    pub dismissable: bool,
    #[live]
    pub grabber: bool,
    #[live(SheetDetent::Expanded)]
    pub detent: SheetDetent,
    #[live]
    pub title: String,
    #[live(0.3)]
    pub slide_secs: f64,
    /// Where the keyboard was before this drawer took it.
    #[rust]
    restore: Area,
    /// Whether the chrome has been written from the props this open.
    #[rust]
    dressed: bool,
    /// How far open the panel stands right now, along its own axis. Only a
    /// sheet moves this; without a grabber the panel's `size` decides.
    #[rust]
    live_extent: f64,
    /// Whether `live_extent` has been read from `detent` yet, this open.
    #[rust]
    extent_dressed: bool,
    /// The room the window had at the last draw, along this panel's axis.
    /// Kept because a drag arrives on an event, where the pass is not.
    #[rust]
    pass_extent: f64,
    /// Where the pointer took hold, and how far open the panel was then.
    #[rust]
    drag_from: Option<(f64, f64)>,
    /// How far in the panel is, 0 at its edge and 1 in place.
    #[rust]
    slide: f64,
    #[rust]
    sliding: bool,
    #[rust]
    last_t: f64,
    #[rust]
    next_frame: NextFrame,
}

impl Drawer {
    pub fn open_drawer(&mut self, cx: &mut Cx) {
        self.restore = cx.key_focus();
        self.dressed = false;
        // A sheet opens at its declared rung every time, rather than where
        // the last drag left it: reopening is a new question being asked,
        // not the same panel coming back.
        self.extent_dressed = false;
        self.drag_from = None;
        self.slide = 0.0;
        self.sliding = true;
        self.last_t = cx.seconds_since_app_start();
        self.next_frame = cx.new_next_frame();
        self.modal.open(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, DrawerAction::Opened);
    }

    pub fn close_drawer(&mut self, cx: &mut Cx) {
        // Straight out rather than sliding back: a drawer that animates
        // away has to keep taking the pointer while it does, and a panel
        // that is leaving must not swallow the press that follows it.
        self.modal.close(cx);
        self.sliding = false;
        cx.set_key_focus(self.restore);
    }

    pub fn is_open(&self) -> bool {
        self.modal.is_open()
    }

    /// How far in the panel is: 0 at its edge, 1 in place.
    pub fn slide(&self) -> f64 {
        self.slide
    }

    /// Which rung the sheet rests at.
    pub fn detent(&self) -> SheetDetent {
        self.detent
    }

    /// Move the sheet to a rung, the way a drag would leave it there.
    pub fn set_detent(&mut self, cx: &mut Cx, detent: SheetDetent) {
        self.detent = detent;
        if self.pass_extent > 0.0 {
            let column = self.side.is_column();
            let base = self.size.extent(column).unwrap_or(self.pass_extent);
            self.live_extent = detent_extent(detent, base, self.pass_extent);
            self.extent_dressed = true;
        } else {
            // Nothing has measured the window yet — this is a rung being
            // set in the same breath as the open, before a single draw —
            // so there is no room to compute against. Leave it for the
            // draw that is about to happen, which will have the pass.
            // Computing here would resolve every rung to nothing and then
            // mark the answer as final, which is a sheet that opens
            // invisible and stays that way.
            self.extent_dressed = false;
        }
        self.modal.redraw(cx);
    }

    fn dress(&mut self, cx: &mut Cx) {
        let content = self.modal.widget(cx, ids!(content));
        content.label(cx, ids!(title_label)).set_text(cx, &self.title);
        content.widget(cx, ids!(close)).set_visible(cx, self.dismissable);
        content.widget(cx, ids!(grab)).set_visible(cx, self.grabber);
    }

    fn answer(&mut self, cx: &mut Cx, action: DrawerAction) {
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
        self.close_drawer(cx);
    }

    /// Advance the slide. Answers whether it is still moving.
    fn step(&mut self, cx: &mut Cx) -> bool {
        if !self.sliding {
            return false;
        }
        let now = cx.seconds_since_app_start();
        let dt = (now - self.last_t).clamp(0.0, 0.1);
        self.last_t = now;
        let secs = self.slide_secs.max(0.01);
        self.slide = (self.slide + dt / secs).min(1.0);
        if self.slide >= 1.0 {
            self.sliding = false;
        }
        self.sliding
    }

    /// The walk the panel takes, and where it sits while it is arriving.
    fn panel_walk(&self, pass: DVec2) -> (Walk, DVec2) {
        let column = self.side.is_column();
        // A sheet stands where it has been dragged to; every other drawer
        // stands where its size says, on exactly the path it always took.
        let extent = if self.grabber {
            Some(self.live_extent)
        } else {
            self.size.extent(column)
        };
        let (w, h) = if column {
            (extent.map(Size::Fixed).unwrap_or(Size::fill()), Size::fill())
        } else {
            (Size::fill(), extent.map(Size::Fixed).unwrap_or(Size::fill()))
        };
        // Eased out: a panel this large arriving at a constant speed reads
        // as being dragged rather than as having been sent for.
        let t = {
            let inv = 1.0 - self.slide.clamp(0.0, 1.0);
            1.0 - inv * inv * inv
        };
        let size = match (column, extent) {
            (true, Some(e)) => dvec2(e, pass.y),
            (true, None) => dvec2(pass.x, pass.y),
            (false, Some(e)) => dvec2(pass.x, e),
            (false, None) => dvec2(pass.x, pass.y),
        };
        let off = 1.0 - t;
        let pos = match self.side {
            PanelEdge::Left => dvec2(-size.x * off, 0.0),
            PanelEdge::Right => dvec2(pass.x - size.x + size.x * off, 0.0),
            PanelEdge::Top => dvec2(0.0, -size.y * off),
            PanelEdge::Bottom => dvec2(0.0, pass.y - size.y + size.y * off),
        };
        (Walk { width: w, height: h, ..Walk::default() }, pos)
    }
}

impl Widget for Drawer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.is_open() {
            if !self.dressed {
                self.dressed = true;
                self.dress(cx.cx.cx);
            }
            let pass = cx.current_pass_size();
            let column = self.side.is_column();
            self.pass_extent = if column { pass.x } else { pass.y };
            if self.grabber && !self.extent_dressed {
                self.extent_dressed = true;
                let base = self.size.extent(column).unwrap_or(self.pass_extent);
                self.live_extent = detent_extent(self.detent, base, self.pass_extent);
            }
            let (panel_walk, pos) = self.panel_walk(pass);
            let content = self.modal.widget(cx.cx.cx, ids!(content));
            let mut view = content.borrow_mut::<crate::view::View>();
            if let Some(view) = view.as_mut() {
                view.walk = panel_walk.with_abs_pos(pos);
            }
            drop(view);
        }
        self.modal.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.is_open() {
            return;
        }
        if self.next_frame.is_event(event).is_some() && self.step(cx) {
            self.next_frame = cx.new_next_frame();
            self.modal.redraw(cx);
        } else if self.next_frame.is_event(event).is_some() {
            self.modal.redraw(cx);
        }
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape && self.dismissable && claim_escape(cx) {
                self.answer(cx, DrawerAction::Dismissed);
                return;
            }
        }
        self.modal.handle_event(cx, event, scope);
        // The grabber, and the only place the panel's extent is moved by
        // hand. It runs BEFORE the scrim check below and returns from every
        // arm that did something: a release that ends outside the panel it
        // has just shrunk must not also read as a press on the scrim, which
        // would send the sheet back when the user only let go of it.
        if self.grabber {
            let column = self.side.is_column();
            let along = |p: DVec2| if column { p.x } else { p.y };
            let base = self.size.extent(column).unwrap_or(self.pass_extent);
            match event {
                Event::MouseDown(me) if self.drag_from.is_none() => {
                    let grab = self
                        .modal
                        .widget(cx, ids!(content))
                        .widget(cx, ids!(grab))
                        .area()
                        .rect(cx);
                    if grab.size.x > 0.0 && grab.contains(me.abs) {
                        self.drag_from = Some((along(me.abs), self.live_extent));
                    }
                }
                Event::MouseMove(me) => {
                    if let Some((held_at, held_extent)) = self.drag_from {
                        // Which way makes the panel bigger depends on the
                        // edge it came from: a bottom sheet grows upward.
                        let delta = along(me.abs) - held_at;
                        let toward_open = match self.side {
                            PanelEdge::Bottom | PanelEdge::Right => -delta,
                            PanelEdge::Top | PanelEdge::Left => delta,
                        };
                        self.live_extent =
                            (held_extent + toward_open).clamp(0.0, base.max(self.pass_extent));
                        cx.set_cursor(if column {
                            MouseCursor::ColResize
                        } else {
                            MouseCursor::RowResize
                        });
                        self.modal.redraw(cx);
                        return;
                    }
                }
                Event::MouseUp(_) => {
                    if self.drag_from.take().is_some() {
                        let collapsed = SHEET_COLLAPSED.min(base);
                        let half = (self.pass_extent * 0.5).clamp(collapsed, base);
                        match settle_detent(self.live_extent, collapsed, half, base) {
                            Some(rung) => {
                                self.detent = rung;
                                self.live_extent = detent_extent(rung, base, self.pass_extent);
                                let uid = self.widget_uid();
                                cx.widget_action(uid, DrawerAction::DetentChanged(rung));
                                self.modal.redraw(cx);
                            }
                            None => self.answer(cx, DrawerAction::Dismissed),
                        }
                        return;
                    }
                }
                _ => {}
            }
        }
        // A press on the scrim sends the drawer back. The modal has already
        // stopped it reaching the page underneath.
        if self.dismissable {
            if let Event::MouseUp(me) = event {
                let panel = self.modal.widget(cx, ids!(content)).area().rect(cx);
                if panel.size.x > 0.0 && !panel.contains(me.abs) {
                    self.answer(cx, DrawerAction::Dismissed);
                    return;
                }
            }
        }
        if let Event::Actions(actions) = event {
            let content = self.modal.widget(cx, ids!(content));
            if content.widget(cx, ids!(close)).as_button().clicked(actions) {
                self.answer(cx, DrawerAction::Dismissed);
            }
        }
    }

    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.title != v {
            self.title = v.to_string();
            self.dressed = false;
            self.modal.redraw(cx);
        }
    }
}

impl DrawerRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_drawer(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close_drawer(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<DrawerAction>(), DrawerAction::Dismissed);
        }
        false
    }

    /// Which rung the sheet rests at.
    pub fn detent(&self) -> SheetDetent {
        self.borrow()
            .map(|inner| inner.detent())
            .unwrap_or(SheetDetent::Expanded)
    }

    /// Move the sheet to a rung.
    pub fn set_detent(&self, cx: &mut Cx, detent: SheetDetent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_detent(cx, detent);
        }
    }

    /// The rung a drag just left the sheet at, if one did.
    pub fn detent_changed(&self, actions: &Actions) -> Option<SheetDetent> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<DrawerAction>() {
            DrawerAction::DetentChanged(rung) => Some(rung),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A side decides whether the drawer is a column or a row, and the
    /// size means the other thing on each.
    #[test]
    fn the_side_decides_what_the_size_means() {
        assert!(PanelEdge::Left.is_column());
        assert!(PanelEdge::Right.is_column());
        assert!(!PanelEdge::Top.is_column());
        assert!(!PanelEdge::Bottom.is_column());
        // A column is wider than a row is tall at the same rung: a list of
        // things needs width, a set of choices needs less height.
        assert!(PanelSize::Md.extent(true).unwrap() > PanelSize::Md.extent(false).unwrap());
        assert_eq!(PanelSize::Full.extent(true), None);
    }

    /// The rungs climb on both axes.
    #[test]
    fn the_size_rungs_climb() {
        for column in [true, false] {
            let sizes: Vec<f64> = [PanelSize::Sm, PanelSize::Md, PanelSize::Lg]
                .iter()
                .map(|s| s.extent(column).unwrap())
                .collect();
            assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?} must climb");
        }
    }

    /// A sheet's three rungs climb — on a window whose half actually falls
    /// between the other two. A 300pt sheet only has three distinct rungs
    /// in a window between 192 and 600 tall; taller than that and half the
    /// room is past the panel itself, which the clamp test below covers.
    #[test]
    fn the_sheet_rungs_climb() {
        let base = PanelSize::Md.extent(false).unwrap();
        let room = 500.0;
        let peek = detent_extent(SheetDetent::Collapsed, base, room);
        let half = detent_extent(SheetDetent::Half, base, room);
        let full = detent_extent(SheetDetent::Expanded, base, room);
        assert!(peek < half && half < full, "{peek} {half} {full} must climb");
        assert_eq!(full, base, "expanded is what the panel's own size asks for");
    }

    /// Half the room is clamped into the two rungs either side of it: on a
    /// short window half of it is less than a peek, and on a tall one it is
    /// more than the panel ever asked for.
    #[test]
    fn the_half_rung_is_clamped_into_the_ones_it_sits_between() {
        let base = 300.0;
        assert_eq!(
            detent_extent(SheetDetent::Half, base, 60.0),
            SHEET_COLLAPSED,
            "a short window: half of it is below the peek, so the peek wins"
        );
        assert_eq!(
            detent_extent(SheetDetent::Half, base, 4000.0),
            base,
            "a tall window: half of it is past the panel, so the panel wins"
        );
    }

    /// A panel smaller than a peek never claims to be taller than it is.
    #[test]
    fn a_panel_shorter_than_a_peek_is_still_only_itself() {
        let base = 40.0;
        for rung in [SheetDetent::Collapsed, SheetDetent::Half, SheetDetent::Expanded] {
            assert!(
                detent_extent(rung, base, 900.0) <= base,
                "{rung:?} must not exceed the panel's own size"
            );
        }
    }

    /// Letting go settles at the nearest rung.
    #[test]
    fn a_drag_settles_at_the_rung_it_is_nearest() {
        let (peek, half, full) = (96.0, 450.0, 300.0_f64.max(450.0));
        assert_eq!(settle_detent(100.0, peek, half, full), Some(SheetDetent::Collapsed));
        assert_eq!(settle_detent(440.0, peek, half, full), Some(SheetDetent::Half));
        assert_eq!(settle_detent(peek, peek, half, full), Some(SheetDetent::Collapsed));
    }

    /// Dragged most of the way down, letting go closes it rather than
    /// leaving a sliver on screen the pointer has already left.
    #[test]
    fn a_drag_below_the_lowest_rung_lets_the_sheet_go() {
        assert_eq!(settle_detent(10.0, 96.0, 450.0, 600.0), None);
        assert_eq!(settle_detent(0.0, 96.0, 450.0, 600.0), None);
        assert!(
            settle_detent(48.0, 96.0, 450.0, 600.0).is_some(),
            "exactly at the threshold still settles: dismissing is the far side of it"
        );
    }
}
