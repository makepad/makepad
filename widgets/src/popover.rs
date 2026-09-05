//! Popover — any content, anchored to any widget, floating over everything.
//!
//! Why: every "hang this off that" in the tree was its own widget, and each
//! one carried its own copy of the same five jobs — take the pointer while
//! open, place the panel off the anchor's FINAL rect, flip and shift it to
//! stay on screen, close on Escape and on a press outside, hand the pointer
//! and the keyboard back. DropToggles, DropSlider, ComboBox and DropDown2
//! all spell those jobs a little differently, and a confirm bubble, a hover
//! card, a toggletip and a flyout toolbar would each have been a fifth,
//! sixth and seventh spelling. This module is the five jobs once, wrapped
//! around whatever content a call site puts in the `content :=` slot:
//!
//! ```text
//! Popover{
//!     open_btn := Button{ text: "Filters" }        // the anchor, laid out in place
//!     content := View{ ... anything ... }          // floats, never in layout
//! }
//! ```
//!
//! The wrapper derefs `View`, so its children are the anchor and are walked
//! exactly as they would be without the wrapper (the `Tip{}` idiom). The
//! one child named `content` is lifted OUT of the view's child list after
//! every apply and drawn on the popover's own overlay draw list instead,
//! sized by its own layout, placed by [`crate::overlay_place::place`] against
//! the anchor's final rect (captured on the event side, where positions are
//! honest), and shifted into place as one unit. It is put back for the
//! duration of each apply so a live edit updates the same instance in place
//! rather than building a fresh one.
//!
//! While a click, manual or context popover is open it holds the sweep lock
//! (`cx.sweep_lock`), the pairing every popup in this crate uses: the panel
//! floats over widgets that were walked before it this dispatch, so marking
//! a press handled by the time it reaches the popover is too late. The lock
//! turns every other hit test away. The content's own widgets use plain
//! `hits`, which the lock would turn away too, so the popover LIFTS its lock
//! around the content's dispatch and takes it again after. Locks nest, so a
//! popover inside a popover holds its own level, and Escape and an outside
//! press unwind one level per press: a popover only acts on them when no
//! inner overlay holds a lock. A hover popover never takes the lock — a
//! hover card that swallowed the next click would lose the click — so its
//! content answers presses only where nothing walked earlier lies beneath
//! it; interactive content wants a click popover.

use crate::{
    button::*,
    event::TouchState,
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{place, PlaceAlign, PlaceRequest, Placement, Side},
    view::*,
    widget::*,
    widget_tree::CxWidgetExt,
};

/// What a popover reports to its host.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum PopoverAction {
    /// The content is now showing, whatever opened it.
    Opened,
    /// The content went away by the anchor, the API, a hover leaving, or
    /// the timer: every close that is not a dismissal.
    Closed,
    /// The operator sent it away: Escape, the back gesture, or a press
    /// outside the anchor and the content.
    Dismissed,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PopoverTrigger = set_type_default() do #(PopoverTrigger::script_api(vm))
    mod.widgets.splat(mod.widgets.PopoverTrigger)
    mod.widgets.PopoverPlacement = set_type_default() do #(PopoverPlacement::script_api(vm))
    mod.widgets.splat(mod.widgets.PopoverPlacement)

    mod.widgets.DrawPopoverArrowBase = #(DrawPopoverArrow::script_component(vm))
    set_type_default() do #(DrawPopoverArrow::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.PopoverBase = #(Popover::register_widget(vm))

    /** The flat popover: a surface-coloured panel with a one-pixel outline
     * and a level-two shadow; every other popover inherits from it. */
    mod.widgets.PopoverFlat = set_type_default() do mod.widgets.PopoverBase{
        width: Fit
        height: Fit
        // The enum variants are splatted into the widget module by this
        // same block, so here they are spelled in full; any other block
        // writes the bare `BottomStart` and `Click`.
        /** which edge of the anchor the content hangs off, and how it lines up */
        placement: mod.widgets.BottomStart
        /** draw a pointer from the panel to the anchor */
        arrow: false
        /** what opens it: a click on the anchor, hovering it, the API only, or a secondary press */
        trigger: mod.widgets.Click
        /** hover dwell before it opens, in seconds 0..2 step 0.05 */
        open_delay: 0.0
        /** grace after the pointer leaves anchor and content, in seconds 0..2 step 0.05 */
        close_delay: 0.0
        /** a press outside the anchor and the content closes it and is consumed */
        light_dismiss: true
        /** Tab and Shift+Tab cycle inside the content while it is open */
        trap_focus: false
        /** gap between the anchor's edge and the panel, in points 0..32 step 1 */
        offset: 4.0
        /** lay the content out at the anchor's width when it is narrower */
        match_anchor_width: false
        /** the arrow's height; its base is twice that 4..16 step 1 */
        arrow_size: 7.0
        /** room between the panel's edge and the content */
        panel_padding: theme.mspace_2

        /** The panel behind the content: a rounded box whose shadow is drawn
         * outside its rect, so the walk it takes stays the content's. */
        draw_panel +: {
            /** panel fill */
            color: uniform(theme.color_surface_container)
            /** outline at rest */
            border_color: uniform(theme.color_outline)
            /** second outline stop; a negative alpha keeps the outline flat */
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            /** outline width in pixels 0..4 step 0.5 */
            border_size: uniform(1.0)
            /** corner rounding 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius * 2.0)
            /** shadow ink */
            shadow_color: uniform(theme.color_elevation_2)
            /** shadow blur 0..48 step 1 */
            shadow_radius: uniform(theme.elevation_2_radius)
            /** shadow drop */
            shadow_offset: uniform(vec2(0.0, theme.elevation_2_offset_y))
            /** dither the outline gradient to hide banding 0..1 step 1 */
            color_dither: uniform(1.0)

            rect_size2: varying(vec2(0))
            rect_size3: varying(vec2(0))
            rect_pos2: varying(vec2(0))
            rect_shift: varying(vec2(0))
            sdf_rect_pos: varying(vec2(0))
            sdf_rect_size: varying(vec2(0))

            vertex: fn() {
                let min_offset = min(self.shadow_offset, vec2(0))
                self.rect_size2 = self.rect_size + 2.0 * vec2(self.shadow_radius)
                self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius)
                self.rect_shift = -min_offset
                return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    stroke_color = mix(self.border_color, self.border_color_2, self.pos.y + dither)
                }
                sdf.box(
                    self.sdf_rect_pos.x,
                    self.sdf_rect_pos.y,
                    self.sdf_rect_size.x,
                    self.sdf_rect_size.y,
                    max(1.0, self.border_radius)
                )
                if sdf.shape > -1.0 {
                    let m = self.shadow_radius
                    let o = self.shadow_offset + self.rect_shift
                    let v = GaussShadow.rounded_box_shadow(vec2(m) + o, self.rect_size2 + o, self.pos * (self.rect_size3 + vec2(m)), self.shadow_radius * 0.5, self.border_radius * 2.0)
                    sdf.clear(self.shadow_color * v)
                }
                sdf.fill_keep(self.color)
                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color, self.border_size)
                }
                return sdf.result
            }
        }

        /** The pointer: a triangle filled like the panel and outlined on its
         * two slanted edges, so it reads as part of the panel's outline. */
        draw_arrow +: {
            // `side` is the ONE instance here (it rides in the widget struct),
            // so every other prop is a uniform or the slots stop lining up.
            side: 0.0
            /** arrow fill */
            color: uniform(theme.color_surface_container)
            /** arrow outline */
            border_color: uniform(theme.color_outline)
            /** outline width in pixels 0..4 step 0.5 */
            border_size: uniform(1.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                // A square turned by a quarter turn, centred on the middle
                // of the arrow's base: the half inside this quad is the
                // triangle, the other half lies past the quad's edge and is
                // never painted. The base sits one pixel inside the quad, on
                // the row that overlaps the panel, so the fill covers the
                // panel's outline under it. Side 1 (content below the
                // anchor) points up and is the default; 0 points down, 2
                // points right, 3 points left.
                let mut c = vec2(w * 0.5, h - 1.0)
                let mut r = w * 0.5
                if self.side < 0.5 {
                    c = vec2(w * 0.5, 1.0)
                } else if self.side > 2.5 {
                    c = vec2(w - 1.0, h * 0.5)
                    r = h * 0.5
                } else if self.side > 1.5 {
                    c = vec2(1.0, h * 0.5)
                    r = h * 0.5
                }
                sdf.rotate(PI * 0.25, c.x, c.y)
                let s = r * 1.41421356
                sdf.rect(c.x - s * 0.5, c.y - s * 0.5, s, s)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }

        /** The floating content. Any View-based object, laid out by its own
         * walk, never by the anchor's; a Fill on either axis reads as Fit.
         * The margin around it is the panel's `panel_padding`. */
        content := View{
            width: Fit
            height: Fit
            flow: Down
        }
    }

    /** The bevelled popover: the flat panel with the theme's outset bevel
     * as a two-stop outline. */
    mod.widgets.Popover = mod.widgets.PopoverFlat{
        draw_panel +: {
            border_color: theme.color_bevel_outset_1
            border_color_2: theme.color_bevel_outset_2
        }
    }

    /** The popover with a pointer to its anchor. */
    mod.widgets.PopoverArrow = mod.widgets.Popover{
        arrow: true
    }

    /** The hover card: opens after a short dwell on the anchor, stays while
     * the pointer is over the anchor or the content, and closes a moment
     * after it leaves both — the grace is what lets the pointer travel from
     * the anchor into the card. Never takes the pointer, so a press
     * anywhere else still lands where it was aimed. */
    mod.widgets.PopoverHover = mod.widgets.Popover{
        trigger: mod.widgets.Hover
        open_delay: 0.35
        close_delay: 0.2
        light_dismiss: false
    }

    /** The toggletip: a click opens it and it stays until Escape or a press
     * outside; key focus moves into the content and Tab cycles there. */
    mod.widgets.PopoverToggle = mod.widgets.Popover{
        trigger: mod.widgets.Click
        trap_focus: true
    }

    mod.widgets.ConfirmPopoverBase = #(ConfirmPopover::register_widget(vm))

    /** A question with two answers, hung off the control that asks it: a
     * title and a cancel/confirm row. `danger` swaps the confirm button for
     * one in the theme's error colour. Reports Confirmed or Cancelled and
     * closes on either; Escape and an outside press dismiss it with no
     * answer, like the popover it is. */
    mod.widgets.ConfirmPopover = set_type_default() do mod.widgets.ConfirmPopoverBase{
        ..mod.widgets.Popover,
        arrow: true
        trap_focus: true
        /** the confirm button takes the theme's error colour */
        danger: false
        content := View{
            width: Fit
            height: Fit
            flow: Down
            spacing: theme.space_2
            /** the question */
            title := Label{
                text: "Are you sure?"
            }
            buttons := View{
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_2
                cancel := Button{
                    text: "Cancel"
                }
                confirm := Button{
                    text: "OK"
                }
                confirm_danger := Button{
                    visible: false
                    text: "OK"
                    draw_bg +: {
                        color: theme.color_error
                        color_hover: theme.color_error
                        color_down: theme.color_error
                        color_focus: theme.color_error
                    }
                }
            }
        }
    }

    /** A label with an (i) mark beside it that opens a hover card of
     * explanation. Set `label.text` for the word and `content.info.text`
     * for the explanation. */
    mod.widgets.InfoLabel = mod.widgets.PopoverHover{
        flow: Right
        spacing: theme.space_1
        align: Align{x: 0.0, y: 0.5}
        /** the word */
        label := Label{
            text: "Label"
        }
        /** the (i) mark */
        glyph := View{
            width: 14
            height: 14
            show_bg: true
            align: Center
            draw_bg +: {
                color: theme.color_outline
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5 - 1.0)
                    sdf.stroke(self.color, 1.0)
                    return sdf.result
                }
            }
            mark := Label{
                padding: 0
                text: "i"
                draw_text +: {
                    color: theme.color_outline
                    text_style: theme.font_bold{
                        font_size: 8
                    }
                }
            }
        }
        content := View{
            width: Fit
            height: Fit
            /** the explanation */
            info := Label{
                text: "Information"
            }
        }
    }
}

/// What opens a popover.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum PopoverTrigger {
    /// A primary press on the anchor opens it; another closes it.
    #[pick]
    #[default]
    Click,
    /// The pointer resting on the anchor opens it after `open_delay`; it
    /// stays while the pointer is over the anchor or the content and closes
    /// `close_delay` after the pointer leaves both. Never takes the pointer.
    Hover,
    /// Only the API opens it.
    Manual,
    /// A secondary press on the anchor opens it AT THE POINTER.
    Context,
}

/// The twelve places the content can hang: an edge of the anchor and an
/// alignment along it. Spelled as one word each because the widget module
/// exports enum variants as bare names, and `Top`, `Start` and `Center`
/// are already words the layout DSL uses.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum PopoverPlacement {
    TopStart,
    TopCenter,
    TopEnd,
    #[pick]
    #[default]
    BottomStart,
    BottomCenter,
    BottomEnd,
    LeftStart,
    LeftCenter,
    LeftEnd,
    RightStart,
    RightCenter,
    RightEnd,
}

impl PopoverPlacement {
    pub fn placement(self) -> Placement {
        match self {
            PopoverPlacement::TopStart => Placement::TOP_START,
            PopoverPlacement::TopCenter => Placement::TOP_CENTER,
            PopoverPlacement::TopEnd => Placement::TOP_END,
            PopoverPlacement::BottomStart => Placement::BOTTOM_START,
            PopoverPlacement::BottomCenter => Placement::BOTTOM_CENTER,
            PopoverPlacement::BottomEnd => Placement::BOTTOM_END,
            PopoverPlacement::LeftStart => Placement::LEFT_START,
            PopoverPlacement::LeftCenter => Placement::LEFT_CENTER,
            PopoverPlacement::LeftEnd => Placement::LEFT_END,
            PopoverPlacement::RightStart => Placement::RIGHT_START,
            PopoverPlacement::RightCenter => Placement::RIGHT_CENTER,
            PopoverPlacement::RightEnd => Placement::RIGHT_END,
        }
    }

    pub fn side(self) -> Side {
        self.placement().side
    }

    pub fn align(self) -> PlaceAlign {
        self.placement().align
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPopoverArrow {
    #[deref]
    draw_super: DrawQuad,
    /// 0 points down, 1 up, 2 right, 3 left: toward the anchor.
    #[live]
    side: f32,
}

/// Inset kept between the content and the window's edges.
const EDGE: f64 = 6.0;
/// Room the overlay root keeps around the panel before the shift, so the
/// arrow and the shadow never start at a negative coordinate.
const ROOT_MARGIN: f64 = 48.0;

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct Popover {
    #[source]
    source: ScriptObjectRef,
    /// The anchor: the wrapper's children, walked in place.
    #[deref]
    view: View,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_arrow: DrawPopoverArrow,

    /// Which edge of the anchor the content hangs off, and how it lines up.
    #[live]
    pub placement: PopoverPlacement,
    /// Draw a pointer from the panel to the anchor.
    #[live(false)]
    pub arrow: bool,
    /// What opens it.
    #[live]
    pub trigger: PopoverTrigger,
    /// Hover dwell before a hover popover opens, in seconds.
    #[live(0.0)]
    pub open_delay: f64,
    /// Grace after the pointer leaves anchor and content, in seconds.
    #[live(0.0)]
    pub close_delay: f64,
    /// A press outside the anchor and the content closes it and is
    /// consumed. Hover popovers close on any outside press regardless, and
    /// never consume it.
    #[live(true)]
    pub light_dismiss: bool,
    /// Tab and Shift+Tab cycle inside the content while it is open, and
    /// key focus moves into the content on open.
    #[live(false)]
    pub trap_focus: bool,
    /// Gap between the anchor's edge and the panel (the arrow's tip, when
    /// there is one), in points.
    #[live(4.0)]
    pub offset: f64,
    /// Lay the content out at the anchor's width when it is narrower; a
    /// content with a wider fixed width keeps it.
    #[live(false)]
    pub match_anchor_width: bool,
    /// The arrow's height; its base is twice that.
    #[live(7.0)]
    pub arrow_size: f64,
    /// Room between the panel's edge and the content. Kept on the panel
    /// rather than the content slot, so a call site's own `content :=`
    /// object, which arrives with the view's zero padding, still sits
    /// inside a margin.
    #[live]
    pub panel_padding: Inset,

    /// The `content :=` child, lifted out of the view after every apply.
    #[rust]
    content: WidgetRef,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    open: bool,
    /// The anchor's FINAL rect, captured on the event side. Mid-draw the
    /// wrapper only knows its pre-alignment position; the event side reads
    /// the place it was actually drawn.
    #[rust]
    anchor_rect: Rect,
    /// A rect handed in by `open_at`, used instead of the anchor's while
    /// this open lasts.
    #[rust]
    pointer_anchor: Option<Rect>,
    /// The panel's rect of the last draw, window-absolute, for event-side
    /// containment tests.
    #[rust]
    panel_rect: Rect,
    /// The side the last draw took, for hosts that want to know.
    #[rust]
    placed_side: Option<Side>,
    #[rust]
    open_timer: Timer,
    #[rust]
    close_timer: Timer,
    /// Where key focus was when this opened; it goes back there on close.
    #[rust]
    focus_before: Area,
    /// Focus is to move into the content at the next draw, when its areas
    /// exist.
    #[rust]
    focus_pending: bool,
    /// The sweep lock is owed but not yet taken: this opened on a press,
    /// and the anchor's own widget must still see the release of that
    /// press, or it stays drawn as held. The lock is taken on the release.
    #[rust]
    lock_pending: bool,
    #[rust]
    focus_trap: FocusTrap,
}

impl ScriptHook for Popover {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    /// Hand the content slot back to the view for the duration of the
    /// apply, so a live edit finds the child by id and updates the SAME
    /// instance in place rather than building a second one.
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if !self.content.is_empty()
            && !self
                .view
                .children
                .iter()
                .any(|(id, _)| *id == id!(content))
        {
            self.view.children.push((id!(content), self.content.clone()));
        }
    }

    /// Lift `content` out of the view's child list: the view neither lays
    /// it out nor hands it events; the popover does both, on the overlay.
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if let Some(pos) = self
            .view
            .children
            .iter()
            .position(|(id, _)| *id == id!(content))
        {
            let (_, widget) = self.view.children.remove(pos);
            self.content = widget;
        }
        let uid = self.view.widget_uid();
        let cx = vm.cx_mut();
        cx.widget_tree_mark_dirty(uid);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }
}

impl WidgetNode for Popover {
    fn widget_uid(&self) -> WidgetUid {
        self.view.widget_uid()
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.view.walk(cx)
    }

    fn area(&self) -> Area {
        self.view.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        self.view.children(visit);
        if !self.content.is_empty() {
            visit(id!(content), self.content.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.view.find_widgets_from_point(cx, point, found);
        if self.open {
            self.content.find_widgets_from_point(cx, point, found);
        }
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.view.set_visible(cx, visible)
    }

    fn visible(&self) -> bool {
        self.view.visible()
    }

    fn set_scroll_pos(&mut self, cx: &mut Cx, v: DVec2) {
        self.view.set_scroll_pos(cx, v)
    }
}

impl Popover {
    /// A hover popover never takes the pointer: a hover card that swallowed
    /// the next click would lose the click.
    fn owns_pointer(&self) -> bool {
        !matches!(self.trigger, PopoverTrigger::Hover)
    }

    /// The rect the content hangs off: the pointer rect while an `open_at`
    /// lasts, else the anchor's final rect.
    fn anchor(&self) -> Rect {
        self.pointer_anchor.unwrap_or(self.anchor_rect)
    }

    fn refresh_anchor(&mut self, cx: &Cx) {
        let rect = self.view.area().rect(cx);
        if rect.size.x > 0.0 || rect.size.y > 0.0 {
            self.anchor_rect = rect;
        }
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    fn stop_timers(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.open_timer);
        cx.stop_timer(self.close_timer);
        self.open_timer = Timer::empty();
        self.close_timer = Timer::empty();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The side the content was last drawn on, after any flip.
    pub fn placed_side(&self) -> Option<Side> {
        self.placed_side
    }

    /// The floating content, for lookups into it.
    pub fn content(&self) -> WidgetRef {
        self.content.clone()
    }

    pub fn open(&mut self, cx: &mut Cx) {
        self.begin_open(cx, false);
    }

    /// Open, taking the sweep lock now or, when `from_press`, on the
    /// release of the press that opened it.
    fn begin_open(&mut self, cx: &mut Cx, from_press: bool) {
        if self.open {
            return;
        }
        self.stop_timers(cx);
        self.refresh_anchor(cx);
        self.open = true;
        self.focus_before = cx.key_focus();
        if self.owns_pointer() {
            if from_press {
                self.lock_pending = true;
            } else {
                cx.sweep_lock(self.view.area());
            }
        }
        if self.trap_focus {
            self.focus_trap.begin(cx, self.content.area());
            self.focus_pending = true;
        }
        self.redraw_all(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, PopoverAction::Opened);
    }

    /// Open hanging off `rect` instead of the anchor: a pointer position
    /// for a context popover, a cell of a grid, a word in a text.
    pub fn open_at(&mut self, cx: &mut Cx, rect: Rect) {
        self.pointer_anchor = Some(rect);
        if self.open {
            self.redraw_all(cx);
        } else {
            self.open(cx);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.close_with(cx, PopoverAction::Closed);
    }

    fn close_with(&mut self, cx: &mut Cx, action: PopoverAction) {
        if !self.open {
            return;
        }
        self.stop_timers(cx);
        self.open = false;
        self.pointer_anchor = None;
        self.focus_pending = false;
        if self.owns_pointer() {
            if !self.lock_pending {
                cx.sweep_unlock(self.view.area());
            }
            self.lock_pending = false;
            // Key focus goes back where it came from, or to the anchor when
            // nothing held it.
            self.focus_trap.end(cx);
            let back = if self.focus_before.is_empty() {
                self.view.area()
            } else {
                self.focus_before
            };
            cx.set_key_focus(back);
        }
        self.redraw_all(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        if self.open {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    fn dismiss(&mut self, cx: &mut Cx) {
        self.close_with(cx, PopoverAction::Dismissed);
    }

    /// A press, from a mouse button or a touch. `on_anchor` is whether it
    /// landed on the anchor, decided by the caller from the claim the
    /// anchor's subtree made or, failing a claim, from the anchor's rect.
    /// Returns true when the popover consumed it.
    fn press_at(
        &mut self,
        cx: &mut Cx,
        abs: DVec2,
        primary: bool,
        secondary: bool,
        on_anchor: bool,
        inner_held: bool,
    ) -> bool {
        if self.open {
            if self.panel_rect.contains(abs) {
                // The content answered it; nothing walked after may.
                return true;
            }
            if on_anchor {
                match self.trigger {
                    PopoverTrigger::Hover => {}
                    _ => self.close(cx),
                }
                return self.owns_pointer();
            }
            if !self.owns_pointer() {
                // A hover card hides under a press like a tip does, and
                // lets the press through to whatever it was for.
                self.close(cx);
                return false;
            }
            if self.light_dismiss && !inner_held {
                self.dismiss(cx);
                return true;
            }
            return false;
        }
        if !on_anchor {
            return false;
        }
        match self.trigger {
            PopoverTrigger::Click if primary => {
                self.begin_open(cx, true);
                true
            }
            PopoverTrigger::Context if secondary => {
                self.pointer_anchor = Some(Rect {
                    pos: abs,
                    size: dvec2(1.0, 1.0),
                });
                self.begin_open(cx, true);
                true
            }
            _ => false,
        }
    }

    /// The pointer moved. Only a hover popover cares: over the anchor or
    /// the content it opens (after the dwell) and stays; away from both it
    /// closes after the grace.
    fn pointer_at(&mut self, cx: &mut Cx, abs: Option<DVec2>) {
        if !matches!(self.trigger, PopoverTrigger::Hover) {
            return;
        }
        let over = match abs {
            Some(abs) => {
                self.anchor().contains(abs) || (self.open && self.panel_rect.contains(abs))
            }
            None => false,
        };
        if over {
            if !self.close_timer.is_empty() {
                cx.stop_timer(self.close_timer);
                self.close_timer = Timer::empty();
            }
            if !self.open && self.open_timer.is_empty() {
                if self.open_delay > 0.0 {
                    self.open_timer = cx.start_timeout(self.open_delay);
                } else {
                    self.open(cx);
                }
            }
        } else {
            if !self.open_timer.is_empty() {
                cx.stop_timer(self.open_timer);
                self.open_timer = Timer::empty();
            }
            if self.open && self.close_timer.is_empty() {
                if self.close_delay > 0.0 {
                    self.close_timer = cx.start_timeout(self.close_delay);
                } else {
                    self.close(cx);
                }
            }
        }
    }

    /// Where the content walk comes from: the content's own, with Fill read
    /// as Fit on both axes — a popover has no room to hand out, and a plain
    /// `content := View{}` arrives with the view's Fill defaults, which a
    /// Fit panel would resolve to nothing — then widened to the anchor when
    /// asked and the content is not wider already.
    fn content_walk(&mut self, cx: &mut Cx) -> Walk {
        let mut walk = self.content.walk(cx);
        if matches!(walk.width, Size::Fill { .. }) {
            walk.width = Size::fit();
        }
        if matches!(walk.height, Size::Fill { .. }) {
            walk.height = Size::fit();
        }
        if self.match_anchor_width {
            let anchor_w = self.anchor().size.x;
            let keep = match walk.width {
                Size::Fixed(w) if w > anchor_w => true,
                _ => false,
            };
            if !keep && anchor_w > 0.0 {
                walk.width = Size::Fixed(anchor_w);
            }
        }
        walk
    }

    fn arrow_rect(&self, side: Side, panel: Rect, arrow_at: DVec2) -> Rect {
        let a = self.arrow_size;
        let ov = 1.0;
        // Keep the tip clear of the rounded corners.
        let inset = a * 1.5;
        match side {
            Side::Bottom => {
                let x = arrow_at
                    .x
                    .max(panel.pos.x + inset)
                    .min(panel.pos.x + panel.size.x - inset);
                Rect {
                    pos: dvec2(x - a, panel.pos.y - a),
                    size: dvec2(a * 2.0, a + ov),
                }
            }
            Side::Top => {
                let x = arrow_at
                    .x
                    .max(panel.pos.x + inset)
                    .min(panel.pos.x + panel.size.x - inset);
                Rect {
                    pos: dvec2(x - a, panel.pos.y + panel.size.y - ov),
                    size: dvec2(a * 2.0, a + ov),
                }
            }
            Side::Right => {
                let y = arrow_at
                    .y
                    .max(panel.pos.y + inset)
                    .min(panel.pos.y + panel.size.y - inset);
                Rect {
                    pos: dvec2(panel.pos.x - a, y - a),
                    size: dvec2(a + ov, a * 2.0),
                }
            }
            Side::Left => {
                let y = arrow_at
                    .y
                    .max(panel.pos.y + inset)
                    .min(panel.pos.y + panel.size.y - inset);
                Rect {
                    pos: dvec2(panel.pos.x + panel.size.x - ov, y - a),
                    size: dvec2(a + ov, a * 2.0),
                }
            }
        }
    }
}

impl Widget for Popover {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The anchor, in layout, exactly as it would be without the wrapper.
        self.view.draw_walk(cx, scope, walk)?;

        // The overlay list is begun on every draw, open or not: a list that
        // is not begun keeps showing what it showed last. The handle is
        // taken out of `self` for the duration so the borrow does not pin
        // every other field.
        let Some(mut draw_list) = self.draw_list.take() else {
            return DrawStep::done();
        };
        draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(
            pass,
            Layout {
                padding: Inset {
                    left: ROOT_MARGIN,
                    top: ROOT_MARGIN,
                    right: ROOT_MARGIN,
                    bottom: ROOT_MARGIN,
                },
                ..Layout::flow_down()
            },
        );
        if self.open && !self.content.is_empty() {
            // The PROVEN popup idiom: draw the panel as turtle content at the
            // overlay root, measure it, then SHIFT the whole list into place.
            // Sizes are honest mid-draw; only positions lie, and the anchor's
            // position comes from the event side.
            let content_walk = {
                let cx: &mut Cx = cx;
                self.content_walk(cx)
            };
            // Unclipped: the panel's shadow is painted outside its rect, and
            // a turtle that clipped to the rect would cut it to the corners.
            self.draw_panel.begin(
                cx,
                Walk::fit(),
                Layout {
                    padding: self.panel_padding,
                    clip_x: false,
                    clip_y: false,
                    ..Layout::default()
                },
            );
            self.content.draw_walk_all(cx, scope, content_walk);
            self.draw_panel.end(cx);
            let panel = self.draw_panel.area().rect(cx);

            let anchor = {
                let a = self.anchor();
                if a.size.x > 0.0 || a.size.y > 0.0 {
                    a
                } else {
                    self.view.area().rect(cx)
                }
            };
            let gap = self.offset + if self.arrow { self.arrow_size } else { 0.0 };
            let placed = place(&PlaceRequest {
                anchor,
                size: panel.size,
                bounds: Rect {
                    pos: dvec2(EDGE, EDGE),
                    size: pass - dvec2(EDGE * 2.0, EDGE * 2.0),
                },
                gap,
                placement: self.placement.placement(),
                match_anchor_width: false,
            });
            // The helper may shorten a popup to the room; the panel keeps
            // its drawn size and overruns instead, the lesser fault.
            let placed_rect = Rect {
                pos: placed.rect.pos,
                size: panel.size,
            };
            if self.arrow {
                self.draw_arrow.side = match placed.side {
                    Side::Top => 0.0,
                    Side::Bottom => 1.0,
                    Side::Left => 2.0,
                    Side::Right => 3.0,
                };
                // Root-local: the panel sits at `panel.pos` before the shift.
                let local_at = placed.arrow_at - placed_rect.pos + panel.pos;
                let rect = self.arrow_rect(placed.side, panel, local_at);
                self.draw_arrow.draw_abs(cx, rect);
            }
            self.panel_rect = placed_rect;
            self.placed_side = Some(placed.side);
            if self.focus_pending {
                // Retarget first: on a first open the trap was begun with
                // an area the content had not drawn yet, and the stops live
                // under the draw list this draw just filled.
                self.focus_pending = false;
                let content_area = self.content.area();
                self.focus_trap.retarget(content_area);
                let first = self.focus_trap.first_stop(cx);
                cx.set_key_focus(first.unwrap_or(content_area));
            }
            cx.end_pass_sized_turtle_with_shift(Area::Empty, placed_rect.pos - panel.pos);
        } else {
            cx.end_pass_sized_turtle();
        }
        draw_list.end(cx);
        self.draw_list = Some(draw_list);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.open_timer.is_event(event).is_some() {
            self.open_timer = Timer::empty();
            self.open(cx);
        }
        if self.close_timer.is_event(event).is_some() {
            self.close_timer = Timer::empty();
            self.close(cx);
        }

        let area = self.view.area();
        let mut inner_held = false;
        if self.open {
            if self.pointer_anchor.is_none() {
                self.refresh_anchor(cx);
            }
            // The content's widgets use plain `hits`, which this popover's
            // own lock would turn away: lift it around their dispatch. What
            // is left on the stack meanwhile is an inner overlay's — and an
            // inner overlay gets Escape and the outside press first.
            let held = self.owns_pointer() && !self.lock_pending;
            if held {
                cx.sweep_unlock(area);
            }
            inner_held = cx.sweep_lock_area().is_some();
            if self.trap_focus {
                self.focus_trap.handle_event(cx, event);
            }
            self.content.handle_event(cx, event, scope);
            if held {
                cx.sweep_lock(area);
            }
        } else if !event.requires_visibility() && !self.content.is_empty() {
            self.content.handle_event(cx, event, scope);
        }

        // The anchor subtree, with the claim snapshot around it (the
        // documented `pointer_claimed_area` idiom): a claim that appeared
        // across it is the anchor under the pointer, whatever child made it.
        let claimed_before = event.pointer_claimed_area();
        self.view.handle_event(cx, event, scope);
        let claimed_after = event.pointer_claimed_area();
        let child_claimed = claimed_after != claimed_before && !claimed_after.is_empty();

        match event {
            Event::MouseDown(me) => {
                if !self.open {
                    self.refresh_anchor(cx);
                }
                let on_anchor = child_claimed
                    || (claimed_before.is_empty() && self.anchor().contains(me.abs));
                if self.press_at(
                    cx,
                    me.abs,
                    me.button.is_primary(),
                    me.button.is_secondary(),
                    on_anchor,
                    inner_held,
                ) && me.handled.get().is_empty()
                {
                    me.handled.set(area);
                }
            }
            // Touch never becomes a MouseDown: without this arm a popover
            // opens on a phone and then answers nothing at all.
            Event::TouchUpdate(te) => {
                if self.open
                    && self.lock_pending
                    && te.touches.iter().any(|t| t.state == TouchState::Stop)
                {
                    self.lock_pending = false;
                    cx.sweep_lock(area);
                }
                if let Some(touch) = te.touches.iter().find(|t| t.state == TouchState::Start) {
                    if !self.open {
                        self.refresh_anchor(cx);
                    }
                    let on_anchor = child_claimed
                        || (claimed_before.is_empty() && self.anchor().contains(touch.abs));
                    if self.press_at(cx, touch.abs, true, false, on_anchor, inner_held)
                        && touch.handled.get().is_empty()
                    {
                        touch.handled.set(area);
                    }
                }
            }
            // The release of the press that opened this: the anchor's own
            // widget has just seen it (the lock was not held yet), so the
            // lock owed since the press is taken now.
            Event::MouseUp(_) => {
                if self.open && self.lock_pending {
                    self.lock_pending = false;
                    cx.sweep_lock(area);
                }
            }
            Event::MouseMove(me) => {
                if !self.open {
                    self.refresh_anchor(cx);
                }
                self.pointer_at(cx, Some(me.abs));
            }
            Event::MouseLeave(_) | Event::ClearHover => {
                self.pointer_at(cx, None);
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                if self.open && !inner_held {
                    self.dismiss(cx);
                }
            }
            _ => {}
        }
        if self.open && !inner_held && event.back_pressed() {
            self.dismiss(cx);
        }
    }
}

impl PopoverRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn open_at(&self, cx: &mut Cx, rect: Rect) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_at(cx, rect);
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
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// The floating content, for lookups into it.
    pub fn content(&self) -> WidgetRef {
        self.borrow()
            .map(|inner| inner.content())
            .unwrap_or_default()
    }

    fn action(&self, actions: &Actions, wanted: PopoverAction) -> bool {
        let uid = self.widget_uid();
        actions.iter().any(|action| {
            action
                .as_widget_action()
                .map(|wa| wa.widget_uid == uid && wa.cast::<PopoverAction>() == wanted)
                .unwrap_or(false)
        })
    }

    pub fn opened(&self, actions: &Actions) -> bool {
        self.action(actions, PopoverAction::Opened)
    }

    pub fn closed(&self, actions: &Actions) -> bool {
        self.action(actions, PopoverAction::Closed)
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        self.action(actions, PopoverAction::Dismissed)
    }
}

/// What a confirm popover reports, besides the popover actions it also
/// sends: which of its two answers was taken.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ConfirmPopoverAction {
    /// The confirm button was pressed; the popover has closed.
    Confirmed,
    /// The cancel button was pressed; the popover has closed.
    Cancelled,
    #[default]
    None,
}

/// A popover whose content is a question and two buttons. It derefs the
/// popover for everything but the answer: the buttons are found in the
/// content by id (`confirm`, `confirm_danger`, `cancel`), `danger` decides
/// which of the two confirm buttons is the visible one, and a press on
/// either answer closes the popover and reports it.
#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct ConfirmPopover {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    popover: Popover,
    /// The confirm button takes the theme's error colour: the answer
    /// destroys something.
    #[live(false)]
    pub danger: bool,
}

impl ConfirmPopover {
    /// Show the confirm button that matches `danger` and hide the other.
    /// Two buttons rather than one recoloured at runtime: a colour is a
    /// shader uniform the button owns, and a preset is the one honest way
    /// to give it a second face.
    fn apply_danger(&mut self, cx: &mut Cx) {
        let danger = self.danger;
        let content = self.popover.content();
        content
            .button(cx, ids!(confirm))
            .set_visible(cx, !danger);
        content
            .button(cx, ids!(confirm_danger))
            .set_visible(cx, danger);
    }

    pub fn set_danger(&mut self, cx: &mut Cx, danger: bool) {
        if self.danger != danger {
            self.danger = danger;
            self.apply_danger(cx);
        }
    }
}

impl ScriptHook for ConfirmPopover {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let cx = vm.cx_mut();
        self.apply_danger(cx);
    }
}

impl WidgetNode for ConfirmPopover {
    fn widget_uid(&self) -> WidgetUid {
        self.popover.widget_uid()
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.popover.walk(cx)
    }

    fn area(&self) -> Area {
        self.popover.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.popover.redraw(cx)
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        self.popover.children(visit)
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.popover.find_widgets_from_point(cx, point, found)
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.popover.set_visible(cx, visible)
    }

    fn visible(&self) -> bool {
        self.popover.visible()
    }

    fn set_scroll_pos(&mut self, cx: &mut Cx, v: DVec2) {
        self.popover.set_scroll_pos(cx, v)
    }
}

impl Widget for ConfirmPopover {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.popover.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let content = self.popover.content();
            let uid = self.widget_uid();
            if content.button(cx, ids!(confirm)).clicked(actions)
                || content.button(cx, ids!(confirm_danger)).clicked(actions)
            {
                cx.widget_action(uid, ConfirmPopoverAction::Confirmed);
                self.popover.close(cx);
            } else if content.button(cx, ids!(cancel)).clicked(actions) {
                cx.widget_action(uid, ConfirmPopoverAction::Cancelled);
                self.popover.close(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.popover.draw_walk(cx, scope, walk)
    }
}

impl ConfirmPopoverRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.popover.open(cx);
        }
    }

    pub fn open_at(&self, cx: &mut Cx, rect: Rect) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.popover.open_at(cx, rect);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.popover.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.popover.is_open()).unwrap_or(false)
    }

    pub fn set_danger(&self, cx: &mut Cx, danger: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_danger(cx, danger);
        }
    }

    fn answer(&self, actions: &Actions, wanted: ConfirmPopoverAction) -> bool {
        let uid = self.widget_uid();
        actions.iter().any(|action| {
            action
                .as_widget_action()
                .map(|wa| wa.widget_uid == uid && wa.cast::<ConfirmPopoverAction>() == wanted)
                .unwrap_or(false)
        })
    }

    pub fn confirmed(&self, actions: &Actions) -> bool {
        self.answer(actions, ConfirmPopoverAction::Confirmed)
    }

    pub fn cancelled(&self, actions: &Actions) -> bool {
        self.answer(actions, ConfirmPopoverAction::Cancelled)
    }
}

/// Keeps Tab and Shift+Tab inside one area's draw list, and hands key
/// focus back where it was when the trap is released.
///
/// The window's `NavControl` walks the nav stops of the whole pass on Tab
/// and picks the next one before any widget sees the key. This trap runs
/// after it in the same dispatch and picks again, this time among the
/// stops under the trapped area's draw list only; key focus is applied
/// after the dispatch, so the later choice is the one that lands. The cycle
/// is a true one — Tab from the last stop lands on the first, Shift+Tab
/// from the first on the last — because the stops are collected into a
/// list first. Only widgets that register nav stops (text inputs, drop
/// downs, sliders today) take part; with no stop at all Tab is still
/// consumed, so focus cannot leave the area through it.
#[derive(Default)]
pub struct FocusTrap {
    area: Area,
    restore: Area,
    active: bool,
}

impl FocusTrap {
    /// Start trapping inside `area`, remembering where focus is now.
    pub fn begin(&mut self, cx: &mut Cx, area: Area) {
        self.restore = cx.key_focus();
        self.area = area;
        self.active = true;
    }

    /// Point the trap at a fresher area for the same content — the one a
    /// later draw produced.
    pub fn retarget(&mut self, area: Area) {
        self.area = area;
    }

    /// Release the trap and put key focus back where `begin` found it.
    pub fn end(&mut self, cx: &mut Cx) {
        if !self.active {
            return;
        }
        self.active = false;
        if !self.restore.is_empty() {
            cx.set_key_focus(self.restore);
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Every nav stop under the trapped area's draw list, in tab order.
    pub fn stops(&self, cx: &mut Cx) -> Vec<Area> {
        let Some(root) = self.area.draw_list_id() else {
            return Vec::new();
        };
        let mut stops = Vec::new();
        CxDraw::iterate_nav_stops(cx, root, |_, stop| {
            stops.push(stop.area);
            None
        });
        stops
    }

    /// The first stop under the trapped area, if there is one.
    pub fn first_stop(&self, cx: &mut Cx) -> Option<Area> {
        self.stops(cx).into_iter().next()
    }

    /// Cycle on Tab / Shift+Tab. Returns true when the key was consumed.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if !self.active {
            return false;
        }
        let Event::KeyDown(ke) = event else {
            return false;
        };
        if ke.key_code != KeyCode::Tab {
            return false;
        }
        let stops = self.stops(cx);
        let focus = cx.key_focus();
        let next = if stops.is_empty() {
            focus
        } else {
            let len = stops.len();
            let at = stops.iter().position(|a| *a == focus);
            match (at, ke.modifiers.shift) {
                (Some(i), false) => stops[(i + 1) % len],
                (Some(i), true) => stops[(i + len - 1) % len],
                (None, false) => stops[0],
                (None, true) => stops[len - 1],
            }
        };
        cx.set_key_focus(next);
        true
    }
}
