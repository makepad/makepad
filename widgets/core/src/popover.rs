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
//! sized by its own layout, placed by [`crate::overlay_place::place_overlay`] against
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
    overlay_place::{
        claim_escape, place_overlay, pointer_on_edge, slide_for_pointer, PlaceAlign, PlaceRequest, Placed,
        Placement, Pointer, Side,
    },
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

    mod.widgets.DrawPopoverPanelBase = #(DrawPopoverPanel::script_component(vm))
    set_type_default() do #(DrawPopoverPanel::script_shader(vm)){
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

        /** The panel behind the content and its pointer, drawn as ONE shape:
         * the rounded box unioned with the pointer, so the fill, the outline
         * and the shadow all come from the same distance field. The quad
         * reaches past the panel's rect for the shadow and the pointer, so
         * the walk it takes stays the content's. */
        draw_panel +: {
            /** panel fill */
            color: uniform(theme.color_surface_container)
            /** outline at rest */
            border_color: uniform(theme.color_outline)
            /** second outline stop; a negative alpha keeps the outline flat */
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            /** the pointer's outline when the outline has two stops */
            arrow_border_color: uniform(theme.color_outline)
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

            quad_shift: varying(vec2(0))
            quad_size: varying(vec2(0))

            vertex: fn() {
                // The panel's rect, grown by the shadow's reach all round, by
                // the drop on the side it falls to, and by the pointer (its
                // outline included) past the edge it stands out of. A quad
                // the panel's own size cuts the point off, shadow and all.
                // The reach is half as far again as the blur: the shade has
                // faded to well under a level there, so the quad's edge
                // leaves no step in it.
                let tip = self.arrow_tip
                let has_arrow = step(0.001, length(tip - self.arrow_base))
                let edge = vec2(self.border_size)
                let reach = vec2(self.shadow_radius * 1.5)
                let lead = reach - min(self.shadow_offset, vec2(0)) + max(edge - tip, vec2(0)) * has_arrow
                let trail = reach + max(self.shadow_offset, vec2(0)) + max(tip + edge - self.rect_size, vec2(0)) * has_arrow
                self.quad_shift = -lead
                self.quad_size = self.rect_size + lead + trail
                return self.clip_and_transform_vertex(self.rect_pos - lead, self.quad_size)
            }

            pixel: fn() {
                // Everything below is in the panel's own points, so the
                // pointer the widget placed lands where it was put.
                let p = self.pos * self.quad_size + self.quad_shift
                let inner = self.rect_size - vec2(self.border_size * 2.0)
                let radius = max(1.0, self.border_radius)
                let sdf = Sdf2d.viewport(p)
                sdf.box(self.border_size, self.border_size, inner.x, inner.y, radius)
                let panel_d = sdf.shape
                sdf.pointer(self.arrow_base.x, self.arrow_base.y, self.arrow_tip.x, self.arrow_tip.y)
                // How much of the outline here is the pointer's: none along
                // the panel's edges, all of it on the pointer's flanks, and
                // shading from one to the other over the point or so where
                // they meet. With no pointer its distance is out of reach
                // and this is nothing everywhere.
                let on_arrow = clamp((panel_d - sdf.dist) * 0.5 + 0.5, 0.0, 1.0)
                // The shadow is the same union, moved by the drop. A blurred
                // edge fades along the tail of the normal curve, so the shade
                // at a point is read off its distance to the shape: exact
                // along a straight edge, and it wraps the pointer, which the
                // shadow of a lone box cannot do.
                let shade = Sdf2d.viewport(p - self.shadow_offset)
                shade.box(self.border_size, self.border_size, inner.x, inner.y, radius)
                shade.pointer(self.arrow_base.x, self.arrow_base.y, self.arrow_tip.x, self.arrow_tip.y)
                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    // Measured over the panel and its shadow's reach, as it was
                    // before the pointer shared this quad: a pointer must not
                    // stretch the bevel of the panel it hangs off.
                    let t = clamp((p.y + self.shadow_radius) / (self.rect_size.y + 2.0 * self.shadow_radius), 0.0, 1.0)
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    // A bevel lights the top edge, and a pointer standing out
                    // of it in that light all but vanishes against a light
                    // ground, whose colour the fill is close to. So the
                    // pointer keeps an outline of its own.
                    stroke_color = mix(mix(self.border_color, self.border_color_2, t + dither), self.arrow_border_color, on_arrow)
                }
                if sdf.shape > -1.0 {
                    // The logistic curve standing in for the normal one, half
                    // way down at the outline's OUTER edge: the panel's
                    // silhouette ends there, not at the middle of the outline
                    // where the box that carries it is drawn.
                    let sigma = max(self.shadow_radius * 0.5, 0.001)
                    let v = 1.0 / (1.0 + exp(clamp(1.702 * (shade.shape - self.border_size) / sigma, -30.0, 30.0)))
                    sdf.clear(self.shadow_color * v)
                }
                sdf.fill_keep(self.color)
                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color, self.border_size)
                }
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

/// The panel and its pointer, drawn as one shape. The pointer rides in the
/// instance because every popover places its own; everything else about the
/// panel is a uniform its style sets.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPopoverPanel {
    #[deref]
    draw_super: DrawQuad,
    /// The middle of the pointer's base, in the panel's own points.
    #[live]
    pub arrow_base: Vec2f,
    /// The pointer's point, in the panel's own points; the same as the base
    /// when the panel has no pointer.
    #[live]
    pub arrow_tip: Vec2f,
}

/// How far a rounded corner of a panel of `size` reaches along its edges
/// from each end: the corner the panel shader draws, on a box inset by the
/// outline's width whose corners reach twice the radius it is given, never
/// past half the box.
fn panel_corner(size: DVec2, border_size: f64, border_radius: f64) -> f64 {
    let half = (size.x.min(size.y) * 0.5 - border_size).max(0.0);
    border_size + (2.0 * border_radius.max(1.0)).min(half)
}

/// What a popover's pointer is sized and placed with, for [`hang_panel`].
#[derive(Clone, Copy, Debug)]
struct PanelPointer {
    /** How far the point stands out; its base is twice that. */
    arrow_size: f64,
    /** The outline's width, read off the panel. */
    border_size: f64,
    /** The corner radius the panel's box is given, read off the panel. */
    border_radius: f64,
}

/// Hang a panel of `size` off `anchor` in a pass of `pass`: where it goes,
/// and the pointer it draws when it has one, in its own points.
///
/// The placement comes first, then the slide off an anchor too small for
/// the pointer to reach its middle from where the panel lines up, then the
/// pointer, placed against the outline and the corners the panel's shape is
/// drawn with. The draw goes through here, and so do the tests.
fn hang_panel(
    anchor: Rect,
    size: DVec2,
    pass: DVec2,
    placement: Placement,
    offset: f64,
    pointer: Option<PanelPointer>,
) -> (Placed, Option<Pointer>) {
    let bounds = Rect {
        pos: dvec2(EDGE, EDGE),
        size: pass - dvec2(EDGE * 2.0, EDGE * 2.0),
    };
    let gap = offset + pointer.map_or(0.0, |p| p.arrow_size);
    let mut placed = place_overlay(&PlaceRequest {
        anchor,
        size,
        bounds,
        gap,
        placement,
        match_anchor_width: false,
    });
    // The helper may shorten a popup to the room; the panel keeps its drawn
    // size and overruns instead, the lesser fault.
    placed.rect.size = size;
    let Some(p) = pointer else {
        return (placed, None);
    };
    let corner = panel_corner(size, p.border_size, p.border_radius);
    let placed = slide_for_pointer(placed, anchor, bounds, corner + p.arrow_size.max(0.0));
    let at = placed.arrow_at - placed.rect.pos;
    let pointer = pointer_on_edge(placed.side, size, at, p.arrow_size, p.border_size, corner);
    (placed, Some(pointer))
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
    draw_panel: DrawPopoverPanel,

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
            // No pointer until this draw has placed one: the instance is
            // written when the panel begins, before it has a size or a place.
            self.draw_panel.arrow_base = Vec2f::default();
            self.draw_panel.arrow_tip = Vec2f::default();
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
            // The pointer is placed against the outline and the corners the
            // panel's shape is drawn with, so it reads them off the panel.
            let pointer = self.arrow.then(|| {
                let mut border_size = [0.0f32];
                let mut border_radius = [0.0f32];
                self.draw_panel.get_uniform(cx, live_id!(border_size), &mut border_size);
                self.draw_panel.get_uniform(cx, live_id!(border_radius), &mut border_radius);
                PanelPointer {
                    arrow_size: self.arrow_size,
                    border_size: border_size[0] as f64,
                    border_radius: border_radius[0] as f64,
                }
            });
            let (placed, pointer) =
                hang_panel(anchor, panel.size, pass, self.placement.placement(), self.offset, pointer);
            let placed_rect = placed.rect;
            if let Some(pointer) = pointer {
                // The pointer is part of the panel's own shape, so it goes
                // into the instance the panel has already written.
                self.draw_panel.arrow_base = pointer.base.into();
                self.draw_panel.arrow_tip = pointer.tip.into();
                self.draw_panel.update_instance_area_value(cx, ids!(arrow_base));
                self.draw_panel.update_instance_area_value(cx, ids!(arrow_tip));
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
        // A closed popover holds no lock, so a lock held now is another
        // overlay's: a modal or drawer whose scrim lies over this anchor, or
        // a menu open elsewhere. The press and the pointer are that overlay's,
        // the way `hits` turns them away from the anchor's own widgets. The
        // anchor test below reads the raw rect, and without this it opened
        // the popover under a drawer's scrim. An overlay whose content holds
        // this popover lifts its own lock while it walks that content, so a
        // popover inside a modal or another popover still opens.
        let locked_out = !self.open && cx.sweep_lock_area().is_some();

        match event {
            Event::MouseDown(me) => {
                if !self.open {
                    self.refresh_anchor(cx);
                }
                let on_anchor = !locked_out
                    && (child_claimed
                        || (claimed_before.is_empty() && self.anchor().contains(me.abs)));
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
                    let on_anchor = !locked_out
                        && (child_claimed
                            || (claimed_before.is_empty() && self.anchor().contains(touch.abs)));
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
            // ...or the opening mouse press itself taken away: its release
            // will not come. A cancel of anything else leaves the lock owed.
            Event::MouseUp(_) => {
                if self.open && self.lock_pending {
                    self.lock_pending = false;
                    cx.sweep_lock(area);
                }
            }
            Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) => {
                if self.open && self.lock_pending {
                    self.lock_pending = false;
                    cx.sweep_lock(area);
                }
            }
            Event::MouseMove(me) => {
                if !self.open {
                    self.refresh_anchor(cx);
                }
                self.pointer_at(cx, (!locked_out).then_some(me.abs));
            }
            Event::MouseLeave(_) | Event::ClearHover => {
                self.pointer_at(cx, None);
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                // Nothing locked above this popover, and no other overlay
                // has taken this press: both, or the press unwinds more
                // than one level.
                if self.open && !inner_held && claim_escape(cx) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The pending sweep lock belongs to the press that opened the popover:
    /// only that mouse press taken away discharges it. Another finger's
    /// cancel, or a cancel of some other capture, leaves it owed.
    #[test]
    fn only_the_opening_press_taken_away_takes_the_pending_lock() {
        crate::on_test_cx(|| {
        let mut cx = crate::checkout_test_cx();
        let mut popover = cx.with_vm(|vm| {
            crate::script_mod(vm);
            Popover::script_new_with_default(vm)
        });
        popover.open = true;
        popover.lock_pending = true;
        let touch: crate::event::DigitId = live_id_num!(touch, 5).into();
        let mouse: crate::event::DigitId = live_id!(mouse).into();
        let other = finger_cancel(&mut cx, touch, crate::event::DigitDevice::Touch { uid: 5 }, true);
        popover.handle_event(&mut cx, &other, &mut Scope::empty());
        assert!(popover.lock_pending, "another finger's cancel took the lock");
        let partial = finger_cancel(&mut cx, mouse, crate::event::DigitDevice::Mouse { button: MouseButton::PRIMARY }, false);
        popover.handle_event(&mut cx, &partial, &mut Scope::empty());
        assert!(popover.lock_pending, "a cancel of another capture took the lock");
        let own = finger_cancel(&mut cx, mouse, crate::event::DigitDevice::Mouse { button: MouseButton::PRIMARY }, true);
        popover.handle_event(&mut cx, &own, &mut Scope::empty());
        assert!(!popover.lock_pending, "the opening press taken away left the lock owed");
        });
    }

    /// A `FingerCancel` for `digit`; with `taken_away` the press itself was
    /// cancelled first (`cancel_digit`), as a host or the OS does.
    fn finger_cancel(cx: &mut Cx, digit: crate::event::DigitId, device: crate::event::DigitDevice, taken_away: bool) -> Event {
        if taken_away {
            cx.fingers.cancel_digit(digit);
        }
        Event::FingerCancel(crate::event::FingerCancelEvent {
            window_id: WindowId(1, 1),
            digit_id: digit,
            device,
            abs: dvec2(1.0, 1.0),
            time: 1.0,
            modifiers: KeyModifiers::default(),
        })
    }


    const PASS: DVec2 = DVec2 { x: 800.0, y: 600.0 };
    const OFFSET: f64 = 4.0;
    const ARROW: f64 = 7.0;
    const BORDER: f64 = 1.0;
    const RADIUS: f64 = 5.0;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    const POINTER: PanelPointer = PanelPointer {
        arrow_size: ARROW,
        border_size: BORDER,
        border_radius: RADIUS,
    };

    /// Hang a panel with a pointer off `anchor`, and give back where it
    /// went, the side it took, and its pointer's point, window-absolute.
    fn hang(anchor: Rect, size: DVec2, placement: Placement) -> (Rect, Side, DVec2) {
        let (placed, pointer) = hang_panel(anchor, size, PASS, placement, OFFSET, Some(POINTER));
        (placed.rect, placed.side, placed.rect.pos + pointer.unwrap().tip)
    }

    #[test]
    fn popover_pointer_aims_at_the_anchor_it_is_centred_on() {
        let (rect, side, tip) = hang(r(300.0, 100.0, 80.0, 30.0), dvec2(200.0, 60.0), Placement::BOTTOM_CENTER);
        assert_eq!(side, Side::Bottom);
        assert_eq!(rect.pos, dvec2(240.0, 141.0));
        // Under the anchor's middle; the outline's outer edge round the point
        // is `offset` from the anchor, so the point itself is a border's
        // width further out.
        assert_eq!(tip, dvec2(340.0, 130.0 + OFFSET + BORDER));
    }

    #[test]
    fn popover_pointer_slides_along_the_edge_when_the_window_pushes_the_panel() {
        // Too near the left edge to centre: the panel is pushed right and the
        // pointer moves left along it, still under the anchor.
        let anchor = r(20.0, 100.0, 80.0, 30.0);
        let (rect, _, tip) = hang(anchor, dvec2(200.0, 60.0), Placement::BOTTOM_CENTER);
        assert_eq!(rect.pos.x, EDGE);
        assert_eq!(tip.x, anchor.center().x);
        // The right edge, above.
        let anchor = r(720.0, 300.0, 60.0, 30.0);
        let (rect, side, tip) = hang(anchor, dvec2(200.0, 60.0), Placement::TOP_CENTER);
        assert_eq!(side, Side::Top);
        assert_eq!(rect.pos.x, PASS.x - EDGE - 200.0);
        assert_eq!(tip, dvec2(anchor.center().x, 300.0 - OFFSET - BORDER));
        // A side placement pushed up off the bottom edge slides the same way.
        let anchor = r(100.0, 560.0, 80.0, 30.0);
        let (rect, side, tip) = hang(anchor, dvec2(160.0, 120.0), Placement::RIGHT_CENTER);
        assert_eq!(side, Side::Right);
        assert_eq!(rect.pos.y, PASS.y - EDGE - 120.0);
        assert_eq!(tip, dvec2(180.0 + OFFSET + BORDER, anchor.center().y));
    }

    #[test]
    fn popover_pointer_stops_where_the_corner_starts() {
        // The anchor's middle lies off the panel's straight edge: the pointer
        // stops with its whole base on the straight part, the corner's reach
        // plus its own half base in from the panel's end.
        let (rect, _, tip) = hang(r(0.0, 100.0, 10.0, 30.0), dvec2(200.0, 60.0), Placement::BOTTOM_CENTER);
        assert_eq!(rect.pos.x, EDGE);
        assert_eq!(tip.x, EDGE + BORDER + RADIUS * 2.0 + ARROW);
        let (rect, _, tip) = hang(r(790.0, 100.0, 10.0, 30.0), dvec2(200.0, 60.0), Placement::BOTTOM_CENTER);
        assert_eq!(rect.pos.x + rect.size.x, PASS.x - EDGE);
        assert_eq!(tip.x, PASS.x - EDGE - BORDER - RADIUS * 2.0 - ARROW);
    }

    #[test]
    fn popover_pointer_follows_a_flip_and_every_side() {
        // No room above: the panel flips below and its pointer points up.
        let anchor = r(300.0, 10.0, 80.0, 30.0);
        let (rect, side, tip) = hang(anchor, dvec2(200.0, 60.0), Placement::TOP_CENTER);
        assert_eq!(side, Side::Bottom);
        assert!(tip.y < rect.pos.y);
        assert_eq!(tip.x, anchor.center().x);
        // Beside a control taller than the corners' reach, so its middle is
        // on the straight part for both ends.
        let anchor = r(300.0, 300.0, 80.0, 74.0);
        let (_, side, tip) = hang(anchor, dvec2(160.0, 120.0), Placement::LEFT_START);
        assert_eq!(side, Side::Left);
        assert_eq!(tip, dvec2(300.0 - OFFSET - BORDER, anchor.center().y));
        let (_, side, tip) = hang(anchor, dvec2(160.0, 120.0), Placement::RIGHT_END);
        assert_eq!(side, Side::Right);
        assert_eq!(tip, dvec2(380.0 + OFFSET + BORDER, anchor.center().y));
    }

    #[test]
    fn popover_pointer_corner_never_reaches_past_half_the_panel() {
        // A radius bigger than a squat panel can hold saturates the way the
        // shader's box does, so the pointer still finds the straight part.
        let size = dvec2(200.0, 24.0);
        let corner = panel_corner(size, 1.0, 40.0);
        let p = pointer_on_edge(Side::Right, size, dvec2(0.0, 0.0), 7.0, 1.0, corner);
        assert_eq!(p.base, dvec2(1.0, 12.0));
        let p = pointer_on_edge(Side::Bottom, size, dvec2(0.0, 0.0), 7.0, 1.0, corner);
        assert_eq!(p.tip, dvec2(1.0 + 11.0 + 7.0, 1.0 - 7.0));
    }

    #[test]
    fn popover_moves_off_a_small_control_so_its_pointer_reaches_the_middle() {
        // Narrower than the pointer and a corner twice over: lined up with
        // the control's start or end, the point would land past its middle,
        // so the panel moves out past that end instead.
        let anchor = r(300.0, 100.0, 20.0, 20.0);
        let (rect, side, tip) = hang(anchor, dvec2(200.0, 60.0), Placement::BOTTOM_START);
        assert_eq!(side, Side::Bottom);
        assert_eq!(tip.x, anchor.center().x);
        assert!(rect.pos.x < anchor.pos.x);
        let (rect, _, tip) = hang(anchor, dvec2(200.0, 60.0), Placement::TOP_END);
        assert_eq!(tip.x, anchor.center().x);
        assert!(rect.pos.x + rect.size.x > anchor.pos.x + anchor.size.x);
        // Beside it: a panel tall enough for the pointer between its corners
        // slides up, and one too short for that is centred on the control.
        let (rect, side, tip) = hang(anchor, dvec2(160.0, 60.0), Placement::RIGHT_START);
        assert_eq!(side, Side::Right);
        assert_eq!(tip.y, anchor.center().y);
        assert!(rect.pos.y < anchor.pos.y);
        let (rect, _, tip) = hang(anchor, dvec2(90.0, 30.0), Placement::LEFT_END);
        assert_eq!(tip.y, anchor.center().y);
        assert_eq!(rect.center().y, anchor.center().y);
    }

    #[test]
    fn popover_opened_at_a_press_points_at_the_press() {
        // A secondary press anchors the panel to the one point pressed.
        let press = r(400.0, 300.0, 1.0, 1.0);
        let (_, side, tip) = hang(press, dvec2(200.0, 60.0), Placement::BOTTOM_START);
        assert_eq!(side, Side::Bottom);
        assert_eq!(tip.x, press.center().x);
    }

    #[test]
    fn popover_without_a_pointer_lines_up_with_the_control() {
        // No pointer, no slide: a small control still gets the placement it
        // asked for, and the gap is the offset alone.
        let anchor = r(300.0, 100.0, 20.0, 20.0);
        let (placed, pointer) = hang_panel(anchor, dvec2(200.0, 60.0), PASS, Placement::BOTTOM_START, OFFSET, None);
        assert!(pointer.is_none());
        assert_eq!(placed.rect.pos, dvec2(300.0, 120.0 + OFFSET));
    }
}
