//! PillNav — a row of destinations inside one rounded bar.
//!
//! Some items are places to go. Others own a panel of links that grows out of
//! the bar when the pointer rests on the item, or when it is pressed or
//! reached by keyboard. A soft pill slides under whichever item the pointer,
//! the keyboard or the open panel is on, and comes to rest on the item that
//! says where the reader is. When the bar has no room it folds into a single
//! pill that opens every item as an accordion.
//!
//! The widget owns what a host should not have to: the hover intent, so a
//! sweep across the bar does not flash panels; the grace that lets the
//! pointer travel from an item into its panel; one keyboard model across the
//! bar and into the panel; and typed actions saying what was chosen.
//!
//! Panels are data, not child widgets. The panel's content is revealed and
//! faded row by row while its surface grows, the keyboard walks its columns,
//! and each row is reported to the test tree; all three need rows this widget
//! owns and measures itself. A host that needs arbitrary content in a panel
//! hangs a hover popover off a plain button instead.
//!
//! The arithmetic — stepping, the bar and panel layouts, the hover intent,
//! the bridge between an item and its panel, the morph and the folded list —
//! is free functions and a small reducer with no `Cx`, so every rule has a
//! unit test and the widget only wires them to events and draws.

use crate::{
    animator::Ease,
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::event::TouchState,
    menu_bar::{for_each_element, obj_bool, obj_field, obj_string},
    overlay_place::{claim_escape, orphan_sweep_locks, place_overlay, release_orphaned_sweep_locks, PlaceRequest, Placed, Side},
    popover::{PopoverPlacement, PopoverTrigger},
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawMorphSurfaceBase = #(DrawMorphSurface::script_component(vm))
    /** A rounded surface that can be part way between two shapes: the fill
     * moves from `color_from` to `color` and the shadow comes in with
     * `morph`, so a pill that grows into a panel lifts as it grows. A clear
     * fill draws only the outline, which is how a focus ring is drawn. */
    set_type_default() do #(DrawMorphSurface::script_shader(vm)){
        ..mod.draw.DrawQuad
        // Rust writes these per draw, so they are plain values.
        radius: 0.0
        morph: 1.0
        opacity: 1.0

        /** the fill once the surface has arrived */
        color: uniform(theme.color_surface_container)
        /** the fill it starts from */
        color_from: uniform(theme.color_surface_container)
        /** the outline */
        border_color: uniform(theme.color_outline_variant)
        /** outline width 0..4 step 0.5 */
        border_size: uniform(1.0)
        /** shadow ink */
        shadow_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        /** shadow blur 0..48 step 1 */
        shadow_radius: uniform(0.0)
        /** shadow drop */
        shadow_offset: uniform(vec2(0.0, 0.0))

        quad_shift: varying(vec2(0))
        quad_size: varying(vec2(0))

        vertex: fn() {
            // The quad reaches past the surface's rect by the shadow's reach
            // and its drop, the way the popover's panel does: a quad the
            // surface's own size would cut the shadow off at the corners.
            let reach = vec2(self.shadow_radius * 1.5)
            let lead = reach - min(self.shadow_offset, vec2(0))
            let trail = reach + max(self.shadow_offset, vec2(0))
            self.quad_shift = -lead
            self.quad_size = self.rect_size + lead + trail
            return self.clip_and_transform_vertex(self.rect_pos - lead, self.quad_size)
        }

        pixel: fn() {
            let p = self.pos * self.quad_size + self.quad_shift
            let b = self.border_size
            let inner = max(self.rect_size - vec2(b * 2.0), vec2(0.0))
            // The box's corner reaches twice the radius it is given, and
            // never past half the short side: a pill is a pill however the
            // morph stretches it.
            let r = clamp(self.radius - b, 0.0, min(inner.x, inner.y) * 0.5)
            let sdf = Sdf2d.viewport(p)
            sdf.box(b, b, inner.x, inner.y, r * 0.5)
            if self.shadow_radius > 0.0 {
                // The shade is read off the distance to the same box moved by
                // the drop, along the logistic curve the popover uses, so the
                // shadow wraps the rounded corners exactly.
                let shade = Sdf2d.viewport(p - self.shadow_offset)
                shade.box(b, b, inner.x, inner.y, r * 0.5)
                let sigma = max(self.shadow_radius * 0.5, 0.001)
                let v = 1.0 / (1.0 + exp(clamp(1.702 * (shade.shape - b) / sigma, -30.0, 30.0)))
                sdf.clear(vec4(self.shadow_color.rgb, self.shadow_color.a * v * self.morph))
            }
            let fill = mix(self.color_from, self.color, self.morph)
            if fill.a > 0.0 {
                sdf.fill_keep(fill)
            }
            if b > 0.0 {
                sdf.stroke(self.border_color, b)
            }
            return sdf.result * self.opacity
        }
    }

    /** Words in the bar and the panel: `hot` lifts the ink, `disabled` dims
     * it, and `opacity` fades it with the panel's reveal. */
    set_type_default() do #(DrawPillNavText::script_shader(vm)){
        ..mod.draw.DrawText
        hot: 0.0
        disabled: 0.0
        opacity: 1.0
        /** the ink under the pointer, the keyboard or on the current item */
        color_hot: uniform(theme.color_on_surface)
        /** ink alpha of a disabled item or link 0..1 step 0.05 */
        disabled_opacity: uniform(theme.state_disabled_content_opacity)
        get_color: fn() {
            let ink = mix(self.color, self.color_hot, self.hot)
            return vec4(ink.rgb, ink.a * mix(1.0, self.disabled_opacity, self.disabled) * self.opacity)
        }
    }

    /** The mark after an item that owns a panel, or the folded pill's three
     * bars. Drawn rather than typed: the text face carries no such glyph. */
    set_type_default() do #(DrawPillNavMark::script_shader(vm)){
        ..mod.draw.DrawQuad
        kind: 0.0
        open: 0.0
        hot: 0.0
        opacity: 1.0
        /** the mark at rest */
        color: uniform(theme.color_on_surface_variant)
        /** the mark on a hot item */
        color_hot: uniform(theme.color_on_surface)
        /** stroke width 0.5..4 step 0.25 */
        stroke: uniform(1.5)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            let ink = mix(self.color, self.color_hot, self.hot)
            if self.kind < 0.5 {
                // A chevron that turns half a turn as its panel opens, so
                // the mark says which way the panel went.
                let w = max(min(self.rect_size.x, self.rect_size.y) * 0.5 - self.stroke, 1.0)
                sdf.rotate(PI * self.open, c.x, c.y)
                sdf.move_to(c.x - w, c.y - w * 0.5)
                sdf.line_to(c.x, c.y + w * 0.5)
                sdf.line_to(c.x + w, c.y - w * 0.5)
                sdf.stroke(ink, self.stroke)
            } else {
                let t = 1.5
                sdf.rect(0.0, c.y - 4.0 - t * 0.5, self.rect_size.x, t)
                sdf.rect(0.0, c.y - t * 0.5, self.rect_size.x, t)
                sdf.rect(0.0, c.y + 4.0 - t * 0.5, self.rect_size.x, t)
                sdf.fill(ink)
            }
            return sdf.result * self.opacity
        }
    }

    mod.widgets.PillNavBase = #(PillNav::register_widget(vm))

    /** A row of destinations in one rounded bar; items may own a panel of
     * links that grows out of the bar. */
    mod.widgets.PillNav = set_type_default() do mod.widgets.PillNavBase{
        width: Fit
        height: Fit

        /** the destinations; `{id: @x label: "X" links: [...]}` owns a panel */
        items: []
        /** what opens a panel: Hover Click Manual */
        open_on: Hover
        /** which edge of the item the panel hangs off */
        panel_placement: BottomCenter
        /** choosing a plain item or a link makes its item current */
        track_current: true
        /** the pill rests on the current item when nothing else has it */
        highlight_current: true
        /** always draw the folded pill */
        compact: false
        /** fold when the bar is wider than the room its parent offers */
        compact_when_crowded: true
        /** the folded pill's word when no item is current */
        compact_label: "Menu"
        /** dimmed and inert, and no tab stop */
        disabled: false
        /** land every glide, grow and fade at once; delays still apply */
        reduced_motion: false

        /** height of an item pill 24..56 step 1 */
        item_height: 32.0
        /** room between the bar's edge and the item pills 0..16 step 1 */
        bar_pad: 4.0
        /** room either side of an item's word 4..32 step 1 */
        item_pad_x: 14.0
        /** gap between two item pills 0..16 step 1 */
        item_gap: 2.0
        /** where the items sit in a bar wider than they need 0..1 step 0.05 */
        items_align: 0.0
        /** the mark after an item that owns a panel; 0 hides it 0..16 step 1 */
        chevron_size: 8.0
        /** gap between the word and the mark 0..16 step 1 */
        chevron_gap: 6.0
        /** gap between the item's edge and the panel 0..24 step 1 */
        panel_gap: 8.0
        /** room inside the panel 0..24 step 1 */
        panel_pad: 8.0
        /** the panel's corner rounding 0..28 step 1 */
        panel_radius: theme.radius_xl
        /** gap between two link columns 0..32 step 1 */
        column_gap: 8.0
        /** a column never narrows past this 80..400 step 4 */
        column_min_width: 180.0
        /** a column never widens past this; longer words end in an ellipsis 120..600 step 4 */
        column_max_width: 320.0
        /** room either side of a link's words 4..24 step 1 */
        row_pad_x: 10.0
        /** room above and below a link's words 2..16 step 1 */
        row_pad_y: 6.0
        /** gap between two links 0..8 step 1 */
        row_gap: 2.0
        /** gap between a link's label and its hint 0..8 step 1 */
        hint_gap: 2.0
        /** the row a column heading takes 16..40 step 1 */
        group_height: 24.0
        /** seconds the pointer rests on an item before its panel opens 0..1 step 0.01 */
        open_delay: 0.1
        /** seconds on another item before an open panel moves to it 0..0.5 step 0.01 */
        switch_delay: 0.06
        /** grace after the pointer leaves the bar, the gap and the panel 0..1 step 0.01 */
        close_delay: 0.2
        // The pill and a panel moving to another item start and stop on
        // screen, which is what the standard curve is for. The pill coming
        // and going, and a panel growing and shrinking, arrive fast and
        // settle, and leave slowly then quickly: the theme's enter and exit
        // curves.
        /** the pill's glide between items 0..1 step 0.01 */
        glide_secs: theme.motion_short_4
        /** the curve the pill glides along; a theme easing */
        glide_ease: theme.motion_ease_standard
        /** the pill coming in where nothing had it 0..1 step 0.01 */
        highlight_enter_secs: theme.motion_short_4
        /** the curve the pill comes in along; a theme easing */
        highlight_enter_ease: theme.motion_ease_standard_decelerate
        /** the pill going once nothing has it 0..1 step 0.01 */
        highlight_exit_secs: theme.motion_short_3
        /** the curve the pill goes along; a theme easing */
        highlight_exit_ease: theme.motion_ease_standard_accelerate
        /** the panel growing out of the pill 0..1 step 0.01 */
        open_secs: theme.motion_medium_1
        /** the curve the panel grows along; a theme easing */
        open_ease: theme.motion_ease_emphasized_decelerate
        /** the panel shrinking back into it 0..1 step 0.01 */
        close_secs: theme.motion_short_3
        /** the curve it shrinks along; a theme easing */
        close_ease: theme.motion_ease_standard_accelerate
        /** an open panel moving to another item 0..1 step 0.01 */
        switch_secs: theme.motion_short_4
        /** the curve it moves along; a theme easing */
        switch_ease: theme.motion_ease_standard
        /** fill and mark alpha of a disabled bar 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity

        /** the bar */
        draw_bar +: {
            color: theme.color_surface_container
            color_from: theme.color_surface_container
            border_color: theme.color_outline_variant
            border_size: 1.0
            shadow_color: theme.color_elevation_1
            shadow_radius: theme.elevation_1_radius
            shadow_offset: vec2(0.0, theme.elevation_1_offset_y)
        }
        /** the pill that slides under an item */
        draw_highlight +: {
            color: theme.color_surface_container_highest
            color_from: theme.color_surface_container_highest
            border_size: 0.0
            shadow_radius: 0.0
            shadow_color: vec4(0.0, 0.0, 0.0, 0.0)
        }
        /** an item's word */
        draw_item_text +: {
            text_style: theme.font_regular{font_size: theme.font_size_p}
            color: theme.color_on_surface_variant
            color_hot: theme.color_on_surface
            disabled_opacity: theme.state_disabled_content_opacity
        }
        /** the current item's word, in the face every item is measured in */
        draw_item_text_current +: {
            text_style: theme.font_bold{font_size: theme.font_size_p}
            color: theme.color_on_surface
            color_hot: theme.color_on_surface
            disabled_opacity: theme.state_disabled_content_opacity
        }
        /** the chevron and the folded pill's bars */
        draw_mark +: {
            color: theme.color_on_surface_variant
            color_hot: theme.color_on_surface
            stroke: 1.5
        }
        /** the ring around the keyboard's item or link */
        draw_focus +: {
            color: vec4(0.0, 0.0, 0.0, 0.0)
            color_from: vec4(0.0, 0.0, 0.0, 0.0)
            border_color: theme.color_primary
            border_size: theme.size_focus_ring
            shadow_radius: 0.0
            shadow_color: vec4(0.0, 0.0, 0.0, 0.0)
        }
        /** the panel; it starts in the pill's grey and ends in its own */
        draw_panel +: {
            color: theme.color_surface_container
            color_from: theme.color_surface_container_highest
            border_color: theme.color_outline_variant
            border_size: 1.0
            shadow_color: theme.color_elevation_2
            shadow_radius: theme.elevation_2_radius
            shadow_offset: vec2(0.0, theme.elevation_2_offset_y)
        }
        /** the tint behind a hot or keyed link */
        draw_row +: {
            color: theme.color_surface_container_high
            color_from: theme.color_surface_container_high
            border_size: 0.0
            shadow_radius: 0.0
            shadow_color: vec4(0.0, 0.0, 0.0, 0.0)
        }
        /** a link's label */
        draw_link_text +: {
            text_style: theme.font_regular{font_size: theme.font_size_p}
            color: theme.color_on_surface
            color_hot: theme.color_on_surface
            disabled_opacity: theme.state_disabled_content_opacity
        }
        /** a link's one-line hint */
        draw_hint_text +: {
            text_style: theme.font_regular{font_size: theme.font_size_p}
            color: theme.color_on_surface_variant
            color_hot: theme.color_on_surface_variant
            disabled_opacity: theme.state_disabled_content_opacity
        }
        /** a column heading */
        draw_group_text +: {
            text_style: theme.font_bold{font_size: theme.font_size_p}
            color: theme.color_on_surface_variant
            color_hot: theme.color_on_surface_variant
            disabled_opacity: theme.state_disabled_content_opacity
        }
    }

    /** The bar whose panels open on a press rather than when the pointer rests. */
    mod.widgets.PillNavClick = mod.widgets.PillNav{
        open_on: Click
    }

    /** The bar folded into one pill that opens every item as an accordion. */
    mod.widgets.PillNavCompact = mod.widgets.PillNav{
        compact: true
    }
}

// ---------------------------------------------------------------------------
// draw layers
// ---------------------------------------------------------------------------

/// A rounded surface between two shapes. Shared with the line menu, whose
/// card, row tint and focus ring are the same three surfaces.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMorphSurface {
    #[deref]
    draw_super: DrawQuad,
    /// Corner radius in points; clamped to half the short side.
    #[live]
    pub radius: f32,
    /// 0 is the shape it grows from, 1 the shape it arrives at.
    #[live(1.0)]
    pub morph: f32,
    #[live(1.0)]
    pub opacity: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPillNavText {
    #[deref]
    draw_super: DrawText,
    #[live]
    pub hot: f32,
    #[live]
    pub disabled: f32,
    #[live(1.0)]
    pub opacity: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPillNavMark {
    #[deref]
    draw_super: DrawQuad,
    /// 0 a chevron, 1 three bars.
    #[live]
    pub kind: f32,
    #[live]
    pub open: f32,
    #[live]
    pub hot: f32,
    #[live(1.0)]
    pub opacity: f32,
}

// ---------------------------------------------------------------------------
// data
// ---------------------------------------------------------------------------

/// One link in an item's panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PillNavLink {
    pub id: LiveId,
    pub label: String,
    /// One line under the label, ellipsised; empty for none.
    pub hint: String,
    /// Links with the same group share a column under that heading; empty
    /// is a column with no heading.
    pub group: String,
    pub enabled: bool,
}

impl PillNavLink {
    pub fn new(id: LiveId, label: &str) -> Self {
        Self { id, label: label.to_string(), hint: String::new(), group: String::new(), enabled: true }
    }

    pub fn with_hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_string();
        self
    }

    pub fn in_group(mut self, group: &str) -> Self {
        self.group = group.to_string();
        self
    }
}

/// One destination in the bar: a place, or a group that owns a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PillNavItem {
    pub id: LiveId,
    pub label: String,
    pub enabled: bool,
    pub links: Vec<PillNavLink>,
}

impl PillNavItem {
    /// An item that is somewhere to go, with no panel.
    pub fn place(id: LiveId, label: &str) -> Self {
        Self { id, label: label.to_string(), enabled: true, links: Vec::new() }
    }

    /// An item that owns a panel of links.
    pub fn group(id: LiveId, label: &str, links: Vec<PillNavLink>) -> Self {
        Self { id, label: label.to_string(), enabled: true, links }
    }

    /// An item opens a panel only when there is something in it and it can
    /// be reached at all.
    pub fn has_panel(&self) -> bool {
        self.enabled && !self.links.is_empty()
    }
}

/// What the bar reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum PillNavAction {
    /// An item without a panel was chosen, by press or by Enter. With
    /// `open_on: Manual` an item with a panel reports this too, since
    /// pressing it opens nothing.
    Selected(LiveId),
    /// A link in a panel was chosen. The panel has started closing.
    Picked { item: LiveId, link: LiveId },
    /// A panel began to open, or moved to another item.
    Opened(LiveId),
    /// The open panel began to close, for any reason.
    Closed,
    #[default]
    None,
}

fn parse_items(vm: &mut ScriptVm, value: ScriptValue) -> Vec<PillNavItem> {
    let mut items = Vec::new();
    for_each_element(vm, value, &mut |vm, entry| {
        let Some(obj) = entry.as_object() else {
            return;
        };
        // An item with no id can never be reported, so it is not an item.
        let Some(id) = obj_field(vm, obj, id!(id)).as_id() else {
            return;
        };
        let links_value = obj_field(vm, obj, id!(links));
        let mut links = Vec::new();
        for_each_element(vm, links_value, &mut |vm, link| {
            let Some(link_obj) = link.as_object() else {
                return;
            };
            let Some(link_id) = obj_field(vm, link_obj, id!(id)).as_id() else {
                return;
            };
            links.push(PillNavLink {
                id: link_id,
                label: obj_string(vm, link_obj, id!(label)).unwrap_or_default(),
                hint: obj_string(vm, link_obj, id!(hint)).unwrap_or_default(),
                group: obj_string(vm, link_obj, id!(group)).unwrap_or_default(),
                enabled: obj_bool(vm, link_obj, id!(enabled)).unwrap_or(true),
            });
        });
        items.push(PillNavItem {
            id,
            label: obj_string(vm, obj, id!(label)).unwrap_or_default(),
            enabled: obj_bool(vm, obj, id!(enabled)).unwrap_or(true),
            links,
        });
    });
    items
}

// ---------------------------------------------------------------------------
// pure helpers
// ---------------------------------------------------------------------------

/// Inset kept between a panel and the window's edges, as the popover keeps.
const EDGE: f64 = 6.0;
/// The folded pill's three bars.
const COMPACT_MARK: f64 = 14.0;
/// Gap between the three bars and the folded pill's word.
const COMPACT_MARK_GAP: f64 = 8.0;
/// How far a focus ring stands off what it rings.
const RING_OUT: f64 = 2.0;

/// The next enabled entry from `from`, one step in the direction of `delta`,
/// wrapping. From nowhere a forward step lands on the first enabled entry
/// and a backward one on the last. None when nothing is enabled.
pub fn step_item(enabled: &[bool], from: Option<usize>, delta: isize) -> Option<usize> {
    let n = enabled.len();
    if n == 0 {
        return None;
    }
    let Some(start) = from.filter(|i| *i < n) else {
        return if delta < 0 {
            enabled.iter().rposition(|on| *on)
        } else {
            enabled.iter().position(|on| *on)
        };
    };
    let step = if delta < 0 { -1 } else { 1 };
    let mut index = start;
    for _ in 0..n {
        index = (index as isize + step).rem_euclid(n as isize) as usize;
        if enabled[index] {
            return Some(index);
        }
    }
    None
}

/// The bar's spacing, as the layout reads it.
#[derive(Clone, Copy, Debug)]
pub struct BarMetrics {
    pub bar_pad: f64,
    pub item_height: f64,
    pub item_gap: f64,
    pub items_align: f64,
}

/// Where each item pill sits, relative to the bar's origin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarLayout {
    /// The bar's natural size: exactly what the items and their gaps need.
    pub size: DVec2,
    pub items: Vec<Rect>,
}

/// The size a bar needs for items of `widths`.
pub fn bar_natural_size(widths: &[f64], m: &BarMetrics) -> DVec2 {
    let gaps = m.item_gap * widths.len().saturating_sub(1) as f64;
    dvec2(m.bar_pad * 2.0 + widths.iter().sum::<f64>() + gaps, m.bar_pad * 2.0 + m.item_height)
}

/// Lay the item pills out in a bar `given` points big. A bar larger than it
/// needs places the row by `items_align` and centres it vertically; a
/// smaller one lets the row run over rather than squeezing words.
pub fn bar_layout(widths: &[f64], m: &BarMetrics, given: DVec2) -> BarLayout {
    let size = bar_natural_size(widths, m);
    let extra = dvec2((given.x - size.x).max(0.0), (given.y - size.y).max(0.0));
    let mut x = m.bar_pad + extra.x * m.items_align.clamp(0.0, 1.0);
    let y = m.bar_pad + extra.y * 0.5;
    let mut items = Vec::with_capacity(widths.len());
    for width in widths {
        items.push(Rect { pos: dvec2(x, y), size: dvec2(*width, m.item_height) });
        x += width + m.item_gap;
    }
    BarLayout { size, items }
}

/// A bar folds only when its parent says how much room there is and that is
/// less than it needs. A parent that fits its children gives no room figure,
/// and a bar that folded there would fold for no reason.
pub fn is_crowded(natural_width: f64, room: Option<f64>) -> bool {
    room.map_or(false, |room| natural_width > room + 0.5)
}

/// A link's drawn size: its label and its hint, each one line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowMeasure {
    pub label: DVec2,
    pub hint: DVec2,
}

/// The panel's spacing, as the layout reads it.
#[derive(Clone, Copy, Debug)]
pub struct PanelMetrics {
    pub panel_pad: f64,
    pub column_gap: f64,
    pub column_min_width: f64,
    pub column_max_width: f64,
    pub row_pad_x: f64,
    pub row_pad_y: f64,
    pub row_gap: f64,
    pub hint_gap: f64,
    pub group_height: f64,
}

/// One column of a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnLayout {
    /// The heading and its row, for a column with a group.
    pub heading: Option<(String, Rect)>,
    pub x: f64,
    pub width: f64,
    /// The links in the column, top to bottom, as indices into the links.
    pub rows: Vec<usize>,
}

/// A panel's columns and rows, relative to the panel's origin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelLayout {
    pub size: DVec2,
    pub columns: Vec<ColumnLayout>,
    /// One rect per link, in link order.
    pub rows: Vec<Rect>,
}

/// The groups in the order each first appears, each with its links in the
/// order they were written.
pub fn panel_columns(links: &[PillNavLink]) -> Vec<(String, Vec<usize>)> {
    let mut columns: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, link) in links.iter().enumerate() {
        match columns.iter_mut().find(|(group, _)| *group == link.group) {
            Some((_, rows)) => rows.push(i),
            None => columns.push((link.group.clone(), vec![i])),
        }
    }
    columns
}

/// How tall one link's row is: its label, and its hint under a small gap.
pub fn row_height(measure: RowMeasure, m: &PanelMetrics) -> f64 {
    let hint = if measure.hint.y > 0.0 { m.hint_gap + measure.hint.y } else { 0.0 };
    m.row_pad_y * 2.0 + measure.label.y + hint
}

/// Lay a panel out. `heading_widths` is one width per column from
/// [`panel_columns`], read for the columns that have a heading.
pub fn panel_layout(
    links: &[PillNavLink],
    measures: &[RowMeasure],
    heading_widths: &[f64],
    m: &PanelMetrics,
) -> PanelLayout {
    let groups = panel_columns(links);
    let mut rows = vec![Rect::default(); links.len()];
    let mut columns = Vec::with_capacity(groups.len());
    let mut x = m.panel_pad;
    let mut tallest: f64 = 0.0;
    let max_width = m.column_max_width.max(m.column_min_width);
    for (c, (group, members)) in groups.into_iter().enumerate() {
        let heading_width = if group.is_empty() { 0.0 } else { heading_widths.get(c).copied().unwrap_or(0.0) };
        let content = members
            .iter()
            .map(|i| {
                let measure = measures.get(*i).copied().unwrap_or_default();
                measure.label.x.max(measure.hint.x)
            })
            .fold(heading_width, f64::max);
        let width = (content + m.row_pad_x * 2.0).max(m.column_min_width).min(max_width);
        let mut y = m.panel_pad;
        let heading = if group.is_empty() {
            None
        } else {
            let rect = Rect { pos: dvec2(x, y), size: dvec2(width, m.group_height) };
            y += m.group_height;
            Some((group, rect))
        };
        for (k, i) in members.iter().enumerate() {
            if k > 0 {
                y += m.row_gap;
            }
            let height = row_height(measures.get(*i).copied().unwrap_or_default(), m);
            rows[*i] = Rect { pos: dvec2(x, y), size: dvec2(width, height) };
            y += height;
        }
        tallest = tallest.max(y - m.panel_pad);
        columns.push(ColumnLayout { heading, x, width, rows: members });
        x += width + m.column_gap;
    }
    let widths: f64 = columns.iter().map(|c| c.width).sum();
    let gaps = m.column_gap * columns.len().saturating_sub(1) as f64;
    PanelLayout {
        size: dvec2(m.panel_pad * 2.0 + widths + gaps, m.panel_pad * 2.0 + tallest),
        columns,
        rows,
    }
}

/// A key pressed while the keyboard is on a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKey {
    Down,
    Up,
    Right,
    Left,
    Home,
    End,
}

/// Where a key sends the keyboard from a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStep {
    To(usize),
    /// Up from the top of a column: back to the item in the bar.
    Bar,
    /// Left past the first column.
    PrevItem,
    /// Right past the last column.
    NextItem,
    Stay,
}

/// The links in reading order: column by column, top to bottom.
pub fn reading_order(layout: &PanelLayout) -> Vec<usize> {
    layout.columns.iter().flat_map(|c| c.rows.iter().copied()).collect()
}

/// Walk a panel by keyboard. Up and Down stay in a column; Left and Right
/// land on the row of the neighbouring column whose middle is nearest, so
/// crossing columns keeps the eye at the same height.
pub fn step_link(layout: &PanelLayout, enabled: &[bool], from: Option<usize>, key: LinkKey) -> LinkStep {
    let on = |i: &usize| enabled.get(*i).copied().unwrap_or(false);
    let order = reading_order(layout);
    let first = || order.iter().copied().find(|i| on(i)).map_or(LinkStep::Stay, LinkStep::To);
    let last = || order.iter().rev().copied().find(|i| on(i)).map_or(LinkStep::Stay, LinkStep::To);
    let Some(from) = from else {
        return match key {
            LinkKey::End => last(),
            LinkKey::Up => LinkStep::Bar,
            _ => first(),
        };
    };
    let Some(col) = layout.columns.iter().position(|c| c.rows.contains(&from)) else {
        return first();
    };
    let column = &layout.columns[col];
    let at = column.rows.iter().position(|r| *r == from).unwrap_or(0);
    match key {
        LinkKey::Down => column.rows[at + 1..].iter().copied().find(|i| on(i)).map_or(LinkStep::Stay, LinkStep::To),
        LinkKey::Up => column.rows[..at].iter().rev().copied().find(|i| on(i)).map_or(LinkStep::Bar, LinkStep::To),
        LinkKey::Right | LinkKey::Left => {
            let centre = layout.rows.get(from).map_or(0.0, |r| r.center().y);
            let step = if key == LinkKey::Right { 1 } else { -1 };
            let mut c = col as isize;
            loop {
                c += step;
                if c < 0 {
                    return LinkStep::PrevItem;
                }
                let Some(next) = layout.columns.get(c as usize) else {
                    return LinkStep::NextItem;
                };
                // `min_by` keeps the first of equals, so a tie goes to the
                // higher row.
                let nearest = next.rows.iter().copied().filter(|i| on(i)).min_by(|a, b| {
                    let da = (layout.rows[*a].center().y - centre).abs();
                    let db = (layout.rows[*b].center().y - centre).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
                if let Some(nearest) = nearest {
                    return LinkStep::To(nearest);
                }
            }
        }
        LinkKey::Home => first(),
        LinkKey::End => last(),
    }
}

/// What the pointer is over, as the hover intent reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Over {
    Nothing,
    /// The bar, but no enabled item.
    Bar,
    Item(usize),
    /// The gap between the open item and its panel.
    Bridge,
    Panel,
}

/// A change the intent is waiting to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pending {
    Open(usize),
    Switch(usize),
    Close,
}

/// A change the intent made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntentChange {
    Opened(usize),
    Switched(usize),
    Closed,
}

/// The timings the intent is read with.
#[derive(Clone, Copy, Debug)]
pub struct IntentTimes {
    pub open_on: PopoverTrigger,
    pub open_delay: f64,
    pub switch_delay: f64,
    pub close_delay: f64,
}

/// Which panel is open and what is about to happen to it. Pure: the widget
/// feeds it where the pointer is, presses and the clock, and does what the
/// returned change says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NavIntent {
    pub open: Option<usize>,
    pub pending: Option<(Pending, f64)>,
}

impl NavIntent {
    fn fire(&mut self, pending: Pending) -> Option<IntentChange> {
        self.pending = None;
        match pending {
            Pending::Open(i) => {
                self.open = Some(i);
                Some(IntentChange::Opened(i))
            }
            Pending::Switch(i) => {
                self.open = Some(i);
                Some(IntentChange::Switched(i))
            }
            Pending::Close => self.open.take().map(|_| IntentChange::Closed),
        }
    }

    /// The pointer is over `over` at `now`.
    pub fn pointer(
        &mut self,
        over: Over,
        now: f64,
        t: &IntentTimes,
        has_panel: &dyn Fn(usize) -> bool,
    ) -> Option<IntentChange> {
        let want = match t.open_on {
            PopoverTrigger::Hover => match (self.open, over) {
                (None, Over::Item(i)) if has_panel(i) => Some((Pending::Open(i), t.open_delay)),
                (None, _) => None,
                (Some(o), Over::Item(i)) if i == o => None,
                (Some(_), Over::Bar | Over::Bridge | Over::Panel) => None,
                (Some(_), Over::Item(i)) if has_panel(i) => Some((Pending::Switch(i), t.switch_delay)),
                (Some(_), Over::Item(_)) => Some((Pending::Close, t.switch_delay)),
                (Some(_), Over::Nothing) => Some((Pending::Close, t.close_delay)),
            },
            // A panel opened by a press stays until something closes it on
            // purpose, but once one is open, moving along the bar still
            // moves it, the way an application menu slides.
            PopoverTrigger::Click | PopoverTrigger::Context => {
                self.pending = None;
                return match (self.open, over) {
                    (Some(o), Over::Item(i)) if i != o && has_panel(i) => {
                        self.open = Some(i);
                        Some(IntentChange::Switched(i))
                    }
                    _ => None,
                };
            }
            PopoverTrigger::Manual => return None,
        };
        let Some((pending, delay)) = want else {
            self.pending = None;
            return None;
        };
        if delay <= 0.0 {
            return self.fire(pending);
        }
        // Kept, not refreshed: a deadline pushed back by every mouse move
        // would never arrive while the pointer is moving.
        if self.pending.map(|(p, _)| p) != Some(pending) {
            self.pending = Some((pending, now + delay));
        }
        None
    }

    /// A press on `item`.
    pub fn press(&mut self, item: usize, t: &IntentTimes, has_panel: &dyn Fn(usize) -> bool) -> Option<IntentChange> {
        self.pending = None;
        if self.open == Some(item) {
            self.open = None;
            return Some(IntentChange::Closed);
        }
        if has_panel(item) && t.open_on != PopoverTrigger::Manual {
            let switched = self.open.is_some();
            self.open = Some(item);
            return Some(if switched { IntentChange::Switched(item) } else { IntentChange::Opened(item) });
        }
        None
    }

    /// The clock reached `now`.
    pub fn tick(&mut self, now: f64) -> Option<IntentChange> {
        match self.pending {
            Some((pending, at)) if at <= now => self.fire(pending),
            _ => None,
        }
    }

    /// When the pending change is due.
    pub fn deadline(&self) -> Option<f64> {
        self.pending.map(|(_, at)| at)
    }

    pub fn force_close(&mut self) -> Option<IntentChange> {
        self.pending = None;
        self.open.take().map(|_| IntentChange::Closed)
    }

    pub fn force_open(&mut self, item: usize) -> Option<IntentChange> {
        self.pending = None;
        match self.open {
            Some(open) if open == item => None,
            Some(_) => {
                self.open = Some(item);
                Some(IntentChange::Switched(item))
            }
            None => {
                self.open = Some(item);
                Some(IntentChange::Opened(item))
            }
        }
    }
}

/// The gap between an item and its panel, from the anchor's facing edge to
/// the panel's, as wide as the two together. Counting it as inside is what
/// lets the pointer cross from an item into its panel without the panel
/// deciding it has been left. `side` is the side of the anchor the panel
/// was placed on.
pub fn bridge_rect(anchor: Rect, panel: Rect, side: Side) -> Rect {
    let left = anchor.pos.x.min(panel.pos.x);
    let right = (anchor.pos.x + anchor.size.x).max(panel.pos.x + panel.size.x);
    let top = anchor.pos.y.min(panel.pos.y);
    let bottom = (anchor.pos.y + anchor.size.y).max(panel.pos.y + panel.size.y);
    let span = |a: f64, b: f64| (a, (b - a).max(0.0));
    match side {
        Side::Bottom => {
            let (y, h) = span(anchor.pos.y + anchor.size.y, panel.pos.y);
            Rect { pos: dvec2(left, y), size: dvec2(right - left, h) }
        }
        Side::Top => {
            let (y, h) = span(panel.pos.y + panel.size.y, anchor.pos.y);
            Rect { pos: dvec2(left, y), size: dvec2(right - left, h) }
        }
        Side::Right => {
            let (x, w) = span(anchor.pos.x + anchor.size.x, panel.pos.x);
            Rect { pos: dvec2(x, top), size: dvec2(w, bottom - top) }
        }
        Side::Left => {
            let (x, w) = span(panel.pos.x + panel.size.x, anchor.pos.x);
            Rect { pos: dvec2(x, top), size: dvec2(w, bottom - top) }
        }
    }
}

/// An ease that starts and stops with no speed and no acceleration, so a
/// grow never visibly kicks at either end.
pub fn smootherstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    // Clamped again: a hair under 1 comes out a hair over it, and a surface
    // at an opacity above 1 brightens what it is drawn over.
    (t * t * t * (t * (t * 6.0 - 15.0) + 10.0)).clamp(0.0, 1.0)
}

pub fn lerp_rect(a: Rect, b: Rect, t: f64) -> Rect {
    Rect::from_lerp(a, b, t)
}

/// A panel part way through growing out of its pill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Morph {
    pub rect: Rect,
    pub radius: f64,
    /// How far the surface has come, held to 0..1, which its fill and shadow
    /// read: a fill mixed past its own colour is some other colour.
    pub morph: f64,
    /// How much of the panel's content shows.
    pub content_alpha: f64,
}

/// The content fades in over the last part of the grow only: words drawn
/// into a surface still the size of a pill are clipped to nonsense. Read off
/// how far the surface has come rather than off the clock, so the words keep
/// pace with the surface whatever curve it grows along.
pub fn content_alpha(reached: f64) -> f64 {
    smootherstep((reached - 0.55) / 0.45)
}

/// The panel `shape` of the way from the pill to where it settles. `shape`
/// is the eased value, which a spring carries past 1 and back, so the
/// surface grows past the panel and settles onto it. `reached` is the
/// furthest the grow has come, which the words fade by: a bounce that dips
/// back must not dim words it has already shown.
pub fn morph_at(shape: f64, reached: f64, pill: Rect, panel: Rect, pill_radius: f64, panel_radius: f64) -> Morph {
    let rect = lerp_rect(pill, panel, shape);
    let held = shape.clamp(0.0, 1.0);
    Morph {
        // A shrink that swings below the pill would turn the size inside out.
        rect: Rect { pos: rect.pos, size: dvec2(rect.size.x.max(0.0), rect.size.y.max(0.0)) },
        radius: pill_radius + (panel_radius - pill_radius) * held,
        morph: held,
        content_alpha: content_alpha(reached),
    }
}

/// The alphas of the panel's old and new content while it moves to another
/// item: the old words are gone before the new ones start, so the two never
/// read as one jumble.
pub fn switch_alphas(progress: f64) -> (f64, f64) {
    let old = 1.0 - smootherstep(progress / 0.35);
    let new = smootherstep((progress - 0.35) / 0.65);
    (old, new)
}

/// A row in a folded bar's list, or a link in a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactRow {
    Item(usize),
    Link(usize, usize),
}

/// The folded list: every item, and under the expanded one, its links.
pub fn compact_rows(items: &[PillNavItem], expanded: Option<usize>) -> Vec<CompactRow> {
    let mut rows = Vec::new();
    for (i, item) in items.iter().enumerate() {
        rows.push(CompactRow::Item(i));
        if expanded == Some(i) {
            rows.extend((0..item.links.len()).map(|j| CompactRow::Link(i, j)));
        }
    }
    rows
}

/// A row can be reached when it and its item are both enabled.
pub fn row_enabled(items: &[PillNavItem], row: CompactRow) -> bool {
    match row {
        CompactRow::Item(i) => items.get(i).map_or(false, |item| item.enabled),
        CompactRow::Link(i, j) => items
            .get(i)
            .and_then(|item| item.enabled.then(|| item.links.get(j)).flatten())
            .map_or(false, |link| link.enabled),
    }
}

/// A key pressed in the folded list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

/// Walk the folded list. Answers the expanded item and the keyboard's row
/// after the key. Right opens an item's links in place and Left closes them,
/// from the item or from any of its links, so the accordion is never more
/// than one key away from where the keyboard is.
pub fn step_compact(
    items: &[PillNavItem],
    expanded: Option<usize>,
    at: Option<CompactRow>,
    key: CompactKey,
) -> (Option<usize>, Option<CompactRow>) {
    let rows = compact_rows(items, expanded);
    let on = |row: &CompactRow| row_enabled(items, *row);
    let pos = at.and_then(|at| rows.iter().position(|row| *row == at));
    match key {
        CompactKey::Down => {
            let next = match pos {
                Some(p) => rows[p + 1..].iter().copied().find(|r| on(r)),
                None => rows.iter().copied().find(|r| on(r)),
            };
            (expanded, next.or(at))
        }
        CompactKey::Up => {
            let next = match pos {
                Some(p) => rows[..p].iter().rev().copied().find(|r| on(r)),
                None => rows.iter().rev().copied().find(|r| on(r)),
            };
            (expanded, next.or(at))
        }
        CompactKey::Home => (expanded, rows.iter().copied().find(|r| on(r)).or(at)),
        CompactKey::End => (expanded, rows.iter().rev().copied().find(|r| on(r)).or(at)),
        CompactKey::Right => match at {
            Some(CompactRow::Item(i)) if items.get(i).map_or(false, |item| item.has_panel()) => (Some(i), at),
            _ => (expanded, at),
        },
        CompactKey::Left => match at {
            Some(CompactRow::Link(i, _)) => (expanded, Some(CompactRow::Item(i))),
            Some(CompactRow::Item(i)) if expanded == Some(i) => (None, at),
            _ => (expanded, at),
        },
    }
}

/// The current item after the items change: kept only when its id is still
/// there and still a place, because a current item that has turned into a
/// group says the reader is somewhere that is now only a heading.
pub fn keep_current(old: &[PillNavItem], current: Option<usize>, new: &[PillNavItem]) -> Option<usize> {
    let id = old.get(current?)?.id;
    new.iter().position(|item| item.id == id && item.links.is_empty())
}

/// One line of text's drawn size. Free-standing so the caller can keep its
/// own `&mut self` while measuring against one of its draw layers.
fn measure(dt: &DrawText, cx: &mut Cx, text: &str) -> DVec2 {
    if text.is_empty() {
        return dvec2(0.0, 0.0);
    }
    let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = dt.font_scale as f64;
    dvec2(laidout.size_in_lpxs.width as f64 * scale, laidout.size_in_lpxs.height as f64 * scale)
}

/// `text` cut to `max_w` with an ellipsis. Links are data with no width of
/// their own, so a long word has to be shortened here, not by a layout.
fn elide(dt: &DrawText, cx: &mut Cx, text: &str, max_w: f64) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let full = measure(dt, cx, text).x;
    if full <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = ((chars.len() as f64) * (max_w / full.max(1.0))).floor() as usize;
    keep = keep.min(chars.len());
    while keep > 0 {
        let mut cut: String = chars[..keep].iter().collect();
        cut.push('\u{2026}');
        if measure(dt, cx, &cut).x <= max_w {
            return cut;
        }
        keep -= 1;
    }
    String::new()
}

/// A number on its way to one end of 0..1 along one of the theme's easings:
/// the panel's grow and the pill's fade here, the line menu's reveal. The
/// clock runs straight and the ease shapes what is read off it, so a spring
/// or a bounce can carry the value past its end and back while the clock
/// still says how much of the run is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitTween {
    from: f64,
    to: f64,
    /// 0 when the run starts, 1 once it is over.
    t: f64,
    secs: f64,
    ease: Ease,
    /// The furthest the value has come toward `to` this run, held to 0..1.
    peak: f64,
}

impl Default for UnitTween {
    fn default() -> Self {
        Self::at(0.0)
    }
}

impl UnitTween {
    /// At rest on `value`.
    pub fn at(value: f64) -> Self {
        Self { from: value, to: value, t: 1.0, secs: 0.0, ease: Ease::Linear, peak: value.clamp(0.0, 1.0) }
    }

    /// Head for `to` along `ease`. A run from one end to the other takes
    /// `secs` and a run from part way takes that share of it, so turning back
    /// mid-run starts from where the value is and takes only as long as the
    /// way back is. Aiming where it already heads changes nothing, so a
    /// caller can aim on every frame.
    pub fn aim(&mut self, to: f64, secs: f64, ease: Ease) {
        if to == self.to {
            return;
        }
        let now = self.value();
        self.secs = secs.max(0.0) * (to - now).abs().min(1.0);
        self.from = now;
        self.to = to;
        self.ease = ease;
        self.peak = now.clamp(0.0, 1.0);
        self.t = 0.0;
        if self.secs <= 0.0 {
            self.settle();
        }
    }

    /// Advance the clock by `dt` seconds. Answers whether the run goes on.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.t >= 1.0 {
            return false;
        }
        if self.secs <= 0.0 {
            self.settle();
            return false;
        }
        self.t = (self.t + dt / self.secs).min(1.0);
        if self.t >= 1.0 {
            self.settle();
            return false;
        }
        let now = self.value().clamp(0.0, 1.0);
        self.peak = if self.to >= self.from { self.peak.max(now) } else { self.peak.min(now) };
        true
    }

    /// End the run on its target now. For motion that has been switched off.
    pub fn settle(&mut self) {
        *self = Self::at(self.to);
    }

    /// Where the value is now, past either end while an overshooting ease
    /// carries it. Exactly the target once the run is over: a curve read at
    /// its last sample can land a hair off.
    pub fn value(&self) -> f64 {
        if self.t >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * self.ease.map(self.t)
    }

    /// The furthest the value has come this run, held to 0..1 and never going
    /// back: what a fade reads, so it does not flicker while a bounce dips.
    pub fn reached(&self) -> f64 {
        self.peak
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    /// Resting on `value`, with nothing of the run left.
    pub fn is_at(&self, value: f64) -> bool {
        self.t >= 1.0 && self.to == value
    }
}

/// A rect on its way to another along one of the theme's easings: the pill
/// that slides under the items, and a panel moving to another item. The
/// button group's sliding indicator does the same on a fixed curve; this one
/// takes the curve as a property and lets an overshooting one carry the rect
/// past where it is going.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EasedGlide {
    from: Rect,
    to: Rect,
    /// 0 at the start of the glide, 1 once it has arrived.
    t: f64,
    secs: f64,
    ease: Ease,
}

impl Default for EasedGlide {
    fn default() -> Self {
        Self { from: Rect::default(), to: Rect::default(), t: 1.0, secs: 0.0, ease: Ease::Linear }
    }
}

impl EasedGlide {
    /// Aim at `target`. A first target is taken at once, there being nowhere
    /// to slide from; a new one starts from wherever the rect is drawn now,
    /// so a second aim mid-flight bends the path instead of jumping back.
    pub fn aim(&mut self, target: Rect, secs: f64, ease: Ease) {
        if self.to == target {
            return;
        }
        if self.is_empty() {
            self.from = target;
            self.to = target;
            self.t = 1.0;
        } else {
            self.from = self.current();
            self.to = target;
            self.t = 0.0;
        }
        self.secs = secs.max(0.0);
        self.ease = ease;
    }

    /// Advance the glide by `dt` seconds. Answers whether it is still
    /// moving, so a caller knows whether to ask for another frame.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.t >= 1.0 {
            return false;
        }
        if self.secs <= 0.0 {
            self.t = 1.0;
            return false;
        }
        self.t = (self.t + dt / self.secs).min(1.0);
        self.t < 1.0
    }

    /// Where the rect is drawn now, past its target while an overshooting
    /// ease carries it, and never a negative size.
    pub fn current(&self) -> Rect {
        if self.t >= 1.0 {
            return self.to;
        }
        let rect = lerp_rect(self.from, self.to, self.ease.map(self.t));
        Rect { pos: rect.pos, size: dvec2(rect.size.x.max(0.0), rect.size.y.max(0.0)) }
    }

    /// Where the rect comes to rest.
    pub fn resting(&self) -> Rect {
        self.to
    }

    /// True while it has never been aimed at anything.
    pub fn is_empty(&self) -> bool {
        self.to.size.x == 0.0 && self.to.size.y == 0.0
    }

    /// How far along the glide's clock is, 0 at its start and 1 once it has
    /// arrived. A fade timed against the glide reads this rather than a
    /// second clock that could drift from the first.
    pub fn progress(&self) -> f64 {
        self.t
    }

    /// Land on the target now. For motion that has been switched off.
    pub fn settle(&mut self) {
        self.t = 1.0;
    }
}

/// `rect` moved inside `room`, and cut down only where it is bigger than the
/// room. A spring or a bounce swings a surface past where it settles; this
/// keeps the swing inside what the surface belongs to, so the window's edge
/// never cuts a rounded corner off mid-swing and a pill never leaves its bar.
pub fn hold_inside(rect: Rect, room: Rect) -> Rect {
    let axis = |pos: f64, size: f64, start: f64, len: f64| {
        let size = size.min(len).max(0.0);
        (pos.min(start + len - size).max(start), size)
    };
    let (x, w) = axis(rect.pos.x, rect.size.x, room.pos.x, room.size.x);
    let (y, h) = axis(rect.pos.y, rect.size.y, room.pos.y, room.size.y);
    Rect { pos: dvec2(x, y), size: dvec2(w, h) }
}

// ---------------------------------------------------------------------------
// the widget
// ---------------------------------------------------------------------------

/// The panel that is showing, opening, closing or moving.
#[derive(Clone, Debug, Default)]
struct PanelState {
    /// The item whose panel it is; 0 and unused for the folded list.
    item: usize,
    /// 0 is the pill, 1 the settled panel, along `open_ease` or `close_ease`.
    grow: UnitTween,
    closing: bool,
    /// Where the settled panel is going: aimed at the placed rect each draw,
    /// so a switch or a folded list changing height glides.
    glide: EasedGlide,
    /// The item whose content is fading out while the panel moves.
    fading: Option<usize>,
    /// The item the glide was last aimed for, so a fade ends only once the
    /// glide has actually been aimed at the new place.
    aimed_item: Option<usize>,
}

/// A row as the panel draws it.
#[derive(Clone, Debug)]
struct ContentRow {
    row: CompactRow,
    /// Relative to the panel's settled origin, before any scroll.
    rect: Rect,
    label: String,
    hint: String,
    label_h: f64,
    enabled: bool,
    indent: f64,
    /// A folded item that owns links: whether they are showing.
    chevron: Option<bool>,
}

#[derive(Clone, Debug)]
struct PanelContent {
    anchor: Rect,
    placed: Placed,
    size: DVec2,
    rows: Vec<ContentRow>,
    headings: Vec<(String, Rect)>,
}

/// A panel row's target on the event side.
#[derive(Clone, Debug)]
struct PanelHit {
    rect: Rect,
    row: CompactRow,
    enabled: bool,
    label: String,
}

#[derive(Script, Widget)]
pub struct PillNav {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,

    #[redraw]
    #[live]
    draw_bar: DrawMorphSurface,
    #[live]
    draw_highlight: DrawMorphSurface,
    #[live]
    draw_item_text: DrawPillNavText,
    #[live]
    draw_item_text_current: DrawPillNavText,
    #[live]
    draw_mark: DrawPillNavMark,
    #[live]
    draw_focus: DrawMorphSurface,
    #[live]
    draw_panel: DrawMorphSurface,
    #[live]
    draw_row: DrawMorphSurface,
    #[live]
    draw_link_text: DrawPillNavText,
    #[live]
    draw_hint_text: DrawPillNavText,
    #[live]
    draw_group_text: DrawPillNavText,

    /// The items as written in the DSL. Parsed on apply when the raw value
    /// changed, so an unrelated re-apply keeps a runtime `set_items`.
    #[live]
    items: ScriptValue,
    /// The item that says where the reader is, as written in the DSL.
    #[live]
    current: LiveId,
    #[live]
    pub open_on: PopoverTrigger,
    #[live]
    pub panel_placement: PopoverPlacement,
    #[live(true)]
    pub track_current: bool,
    #[live(true)]
    pub highlight_current: bool,
    #[live(false)]
    pub compact: bool,
    #[live(true)]
    pub compact_when_crowded: bool,
    #[live]
    pub compact_label: String,
    #[live(false)]
    pub disabled: bool,
    #[live(false)]
    pub reduced_motion: bool,
    #[live(32.0)]
    pub item_height: f64,
    #[live(4.0)]
    pub bar_pad: f64,
    #[live(14.0)]
    pub item_pad_x: f64,
    #[live(2.0)]
    pub item_gap: f64,
    #[live(0.0)]
    pub items_align: f64,
    #[live(8.0)]
    pub chevron_size: f64,
    #[live(6.0)]
    pub chevron_gap: f64,
    #[live(8.0)]
    pub panel_gap: f64,
    #[live(8.0)]
    pub panel_pad: f64,
    #[live(14.0)]
    pub panel_radius: f64,
    #[live(8.0)]
    pub column_gap: f64,
    #[live(180.0)]
    pub column_min_width: f64,
    #[live(320.0)]
    pub column_max_width: f64,
    #[live(10.0)]
    pub row_pad_x: f64,
    #[live(6.0)]
    pub row_pad_y: f64,
    #[live(2.0)]
    pub row_gap: f64,
    #[live(2.0)]
    pub hint_gap: f64,
    #[live(24.0)]
    pub group_height: f64,
    #[live(0.1)]
    pub open_delay: f64,
    #[live(0.06)]
    pub switch_delay: f64,
    #[live(0.2)]
    pub close_delay: f64,
    #[live(0.2)]
    pub glide_secs: f64,
    #[live(Ease::Bezier { cp0: 0.2, cp1: 0.0, cp2: 0.0, cp3: 1.0 })]
    pub glide_ease: Ease,
    #[live(0.2)]
    pub highlight_enter_secs: f64,
    #[live(Ease::Bezier { cp0: 0.0, cp1: 0.0, cp2: 0.0, cp3: 1.0 })]
    pub highlight_enter_ease: Ease,
    #[live(0.15)]
    pub highlight_exit_secs: f64,
    #[live(Ease::Bezier { cp0: 0.3, cp1: 0.0, cp2: 1.0, cp3: 1.0 })]
    pub highlight_exit_ease: Ease,
    #[live(0.25)]
    pub open_secs: f64,
    #[live(Ease::Bezier { cp0: 0.05, cp1: 0.7, cp2: 0.1, cp3: 1.0 })]
    pub open_ease: Ease,
    #[live(0.15)]
    pub close_secs: f64,
    #[live(Ease::Bezier { cp0: 0.3, cp1: 0.0, cp2: 1.0, cp3: 1.0 })]
    pub close_ease: Ease,
    #[live(0.2)]
    pub switch_secs: f64,
    #[live(Ease::Bezier { cp0: 0.2, cp1: 0.0, cp2: 0.0, cp3: 1.0 })]
    pub switch_ease: Ease,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    defs: Vec<PillNavItem>,
    #[rust]
    items_source: ScriptValue,
    #[rust]
    current_source: LiveId,
    #[rust]
    current_index: Option<usize>,
    /// The enabled item under the pointer, from the bar's own hits.
    #[rust]
    hover: Option<usize>,
    #[rust]
    bar_hovered: bool,
    /// The item a press went down on, so its release can choose it.
    #[rust]
    pressed: Option<usize>,
    /// An item a press just closed: hovering it does not reopen the panel
    /// until the pointer has been somewhere else.
    #[rust]
    hover_suppressed: Option<usize>,
    #[rust]
    key_item: Option<usize>,
    #[rust]
    key_link: Option<usize>,
    /// The folded list's keyboard row, and the item whose links show.
    #[rust]
    compact_key: Option<CompactRow>,
    #[rust]
    expanded: Option<usize>,
    #[rust]
    hot_row: Option<CompactRow>,
    /// Set by a key the bar handles, cleared by the pointer: the focus ring
    /// shows only for someone using the keys.
    #[rust]
    keyboard_mode: bool,
    #[rust]
    focused: bool,
    /// The focus that is about to arrive came from a press, not from Tab.
    #[rust]
    focus_by_press: bool,
    #[rust]
    intent: NavIntent,
    #[rust]
    panel: Option<PanelState>,
    /// The pill, in the bar's own points: a glide measured in window points
    /// would glide every time the page scrolled the bar.
    #[rust]
    highlight: EasedGlide,
    /// The pill coming in when something claims it, along its enter curve,
    /// and going when nothing does, along its exit curve.
    #[rust]
    highlight_fade: UnitTween,
    /// The bar's rect as the event side reads it: mid-draw positions come
    /// before alignment and lie.
    #[rust]
    bar_rect: Rect,
    /// Each item pill, relative to the bar's origin, from the last draw.
    #[rust]
    item_rects: Vec<Rect>,
    /// The folded pill, relative to the bar's origin.
    #[rust]
    pill_rect: Rect,
    #[rust]
    drawn_compact: bool,
    /// Where the panel settles. Everything that hits against the panel reads
    /// this and never the surface as drawn, which is smaller while it grows
    /// and larger while a spring swings it.
    #[rust]
    panel_rect: Rect,
    #[rust]
    panel_anchor: Rect,
    #[rust]
    panel_side: Option<Side>,
    #[rust]
    panel_layout_cache: Option<(usize, PanelLayout)>,
    #[rust]
    panel_rows: Vec<(CompactRow, Rect)>,
    #[rust]
    panel_visible_h: f64,
    #[rust]
    panel_hits: Vec<PanelHit>,
    #[rust]
    panel_scroll: f64,
    #[rust]
    panel_scroll_max: f64,
    #[rust]
    locked: bool,
    #[rust]
    frame: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    timer: Timer,
    #[rust]
    timer_deadline: f64,
    #[rust]
    overlay: Option<DrawList2d>,
}

impl ScriptHook for PillNav {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        let parsed = if self.items.raw() != self.items_source.raw() {
            self.items_source = self.items;
            Some(parse_items(vm, self.items))
        } else {
            None
        };
        let cx = vm.cx_mut();
        if let Some(items) = parsed {
            self.set_items(cx, items);
        }
        if self.current != self.current_source {
            self.current_source = self.current;
            let id = self.current;
            self.current_index = self.defs.iter().position(|item| item.id == id && item.links.is_empty());
        }
        if self.disabled || !self.visible {
            self.close(cx);
        }
        self.repaint(cx);
    }
}

impl PillNav {
    // -- reading ----------------------------------------------------------

    fn instant(&self) -> bool {
        self.reduced_motion
    }

    fn bar_metrics(&self) -> BarMetrics {
        BarMetrics {
            bar_pad: self.bar_pad,
            item_height: self.item_height,
            item_gap: self.item_gap,
            items_align: self.items_align,
        }
    }

    fn panel_metrics(&self) -> PanelMetrics {
        PanelMetrics {
            panel_pad: self.panel_pad,
            column_gap: self.column_gap,
            column_min_width: self.column_min_width,
            column_max_width: self.column_max_width,
            row_pad_x: self.row_pad_x,
            row_pad_y: self.row_pad_y,
            row_gap: self.row_gap,
            hint_gap: self.hint_gap,
            group_height: self.group_height,
        }
    }

    fn times(&self, touch: bool) -> IntentTimes {
        // Touch has no hover, so a panel that would open on a rest opens
        // on a tap instead.
        let open_on = match self.open_on {
            PopoverTrigger::Hover if touch => PopoverTrigger::Click,
            other => other,
        };
        IntentTimes {
            open_on,
            open_delay: self.open_delay,
            switch_delay: self.switch_delay,
            close_delay: self.close_delay,
        }
    }

    fn enabled_items(&self) -> Vec<bool> {
        self.defs.iter().map(|item| item.enabled).collect()
    }

    fn item_abs(&self, index: usize) -> Option<Rect> {
        self.item_rects.get(index).map(|r| r.translate(self.bar_rect.pos))
    }

    fn item_at(&self, abs: DVec2) -> Option<usize> {
        if self.drawn_compact {
            return None;
        }
        self.item_rects.iter().position(|r| r.translate(self.bar_rect.pos).contains(abs))
    }

    fn panel_showing(&self) -> Option<&PanelState> {
        self.panel.as_ref().filter(|panel| !panel.closing)
    }

    fn compact_word(&self) -> String {
        self.current_index
            .and_then(|i| self.defs.get(i))
            .map(|item| item.label.clone())
            .unwrap_or_else(|| self.compact_label.clone())
    }

    /// The panel's layout for keyboard steps: the one last drawn when it is
    /// for this item, else one with every row the same height, which keeps
    /// the column order and near enough the heights for a first key.
    fn layout_for_keys(&self, item: usize) -> PanelLayout {
        if let Some((drawn, layout)) = &self.panel_layout_cache {
            if *drawn == item {
                return layout.clone();
            }
        }
        let links = self.defs.get(item).map(|d| d.links.clone()).unwrap_or_default();
        let measures = vec![RowMeasure::default(); links.len()];
        panel_layout(&links, &measures, &[], &self.panel_metrics())
    }

    fn link_enabled(&self, item: usize) -> Vec<bool> {
        self.defs
            .get(item)
            .map(|d| d.links.iter().map(|l| d.enabled && l.enabled).collect())
            .unwrap_or_default()
    }

    fn first_link(&self, item: usize) -> Option<usize> {
        match step_link(&self.layout_for_keys(item), &self.link_enabled(item), None, LinkKey::Home) {
            LinkStep::To(link) => Some(link),
            _ => None,
        }
    }

    /// Which item the pill should be under, in order of who has the most
    /// claim to it: the keys, the open panel, the pointer, the current item.
    fn highlight_target(&self) -> Option<Rect> {
        if self.drawn_compact {
            let active = self.bar_hovered || self.panel_showing().is_some() || (self.keyboard_mode && self.focused);
            return active.then_some(self.pill_rect);
        }
        let index = if self.keyboard_mode && self.focused && self.key_item.is_some() {
            self.key_item
        } else if let Some(panel) = self.panel_showing() {
            Some(panel.item)
        } else if self.hover.is_some() {
            self.hover
        } else if self.highlight_current {
            self.current_index
        } else {
            None
        };
        index.and_then(|i| self.item_rects.get(i).copied())
    }

    /// The pill where it is drawn, in the bar's points. An overshooting curve
    /// carries it past the item it is going to, but not out of the bar: past
    /// the bar's end it would be a pill floating over the page.
    fn highlight_now(&self, bar_size: DVec2) -> Rect {
        let bar = Rect { pos: dvec2(0.0, 0.0), size: bar_size };
        hold_inside(self.highlight.current(), bar.hull(self.highlight.resting()))
    }

    fn key_row(&self) -> Option<CompactRow> {
        if !(self.keyboard_mode && self.focused) {
            return None;
        }
        if self.drawn_compact {
            self.compact_key
        } else {
            Some(CompactRow::Link(self.panel_showing()?.item, self.key_link?))
        }
    }

    // -- motion -----------------------------------------------------------

    fn repaint(&self, cx: &mut Cx) {
        self.draw_bar.redraw(cx);
        if let Some(list) = &self.overlay {
            list.redraw(cx);
        }
    }

    /// Something started to move: ask for a frame and a redraw.
    fn kick(&mut self, cx: &mut Cx) {
        self.frame = cx.new_next_frame();
        self.repaint(cx);
    }

    /// Advance every animation by `dt`. Answers whether anything still moves.
    fn advance(&mut self, dt: f64) -> bool {
        let instant = self.instant();
        let mut moving = false;

        let (glide_secs, glide_ease) = (self.glide_secs, self.glide_ease);
        let target = self.highlight_target();
        if let Some(rect) = target {
            self.highlight.aim(rect, glide_secs, glide_ease);
        }
        if instant {
            self.highlight.settle();
        } else if self.highlight.step(dt) {
            moving = true;
        }
        // Coming and going are two motions with a curve and a time each; the
        // glide's are for moving between items.
        let (want, fade_secs, fade_ease) = if target.is_some() {
            (1.0, self.highlight_enter_secs, self.highlight_enter_ease)
        } else {
            (0.0, self.highlight_exit_secs, self.highlight_exit_ease)
        };
        self.highlight_fade.aim(want, fade_secs, fade_ease);
        if instant {
            self.highlight_fade.settle();
        } else if self.highlight_fade.step(dt) {
            moving = true;
        }
        if self.highlight_fade.is_at(0.0) {
            // Taken instantly next time rather than gliding in from a
            // place it left long ago.
            self.highlight = EasedGlide::default();
        }

        let open = (1.0, self.open_secs, self.open_ease);
        let close = (0.0, self.close_secs, self.close_ease);
        let mut drop_panel = false;
        if let Some(panel) = &mut self.panel {
            // Aimed on every frame: a close during the grow, or an open
            // during the shrink, turns back from where the surface is.
            let (goal, secs, ease) = if panel.closing { close } else { open };
            panel.grow.aim(goal, secs, ease);
            if instant {
                panel.grow.settle();
            } else if panel.grow.step(dt) {
                moving = true;
            }
            if instant {
                panel.glide.settle();
            } else if panel.glide.step(dt) {
                moving = true;
            }
            if panel.fading.is_some() {
                if panel.aimed_item == Some(panel.item) && panel.glide.progress() >= 1.0 {
                    panel.fading = None;
                } else {
                    moving = true;
                }
            }
            drop_panel = panel.closing && panel.grow.is_at(0.0);
        }
        if drop_panel {
            self.panel = None;
        }
        moving
    }

    fn arm_timer(&mut self, cx: &mut Cx) {
        match self.intent.deadline() {
            Some(deadline) if deadline == self.timer_deadline && !self.timer.is_empty() => {}
            Some(deadline) => {
                cx.stop_timer(self.timer);
                let now = cx.seconds_since_app_start();
                self.timer = cx.start_timeout((deadline - now).max(0.001));
                self.timer_deadline = deadline;
            }
            None => {
                if !self.timer.is_empty() {
                    cx.stop_timer(self.timer);
                    self.timer = Timer::empty();
                }
            }
        }
    }

    fn lock(&mut self, cx: &mut Cx) {
        if !self.locked {
            self.locked = true;
            // A lock a dropped overlay left behind goes first. The window
            // does this on every event; this is for a tree with no window
            // above it, where that lock would otherwise sit under this one
            // and outlive it.
            release_orphaned_sweep_locks(cx);
            // A bar that has never drawn has no area to own the lock with,
            // and a lock held by nothing turns every press in the window
            // away. The draw takes it once there is an area.
            let area = self.draw_bar.area();
            if !area.is_empty() {
                cx.sweep_lock(area);
            }
        }
    }

    fn unlock(&mut self, cx: &mut Cx) {
        if self.locked {
            self.locked = false;
            cx.sweep_unlock(self.draw_bar.area());
        }
    }

    // -- opening and closing ---------------------------------------------

    fn apply_change(&mut self, cx: &mut Cx, change: Option<IntentChange>) {
        match change {
            Some(IntentChange::Opened(item)) | Some(IntentChange::Switched(item)) => self.show_panel(cx, item),
            Some(IntentChange::Closed) => self.hide_panel(cx),
            None => {}
        }
        self.arm_timer(cx);
    }

    fn show_panel(&mut self, cx: &mut Cx, item: usize) {
        let instant = self.instant();
        let mut moved = true;
        match &mut self.panel {
            // A panel part way shut grows back from where it is.
            Some(panel) if panel.item == item => {
                moved = panel.closing;
                panel.closing = false;
            }
            Some(panel) => {
                panel.fading = if instant || panel.closing { None } else { Some(panel.item) };
                panel.item = item;
                panel.closing = false;
            }
            None => {
                self.panel = Some(PanelState { item, grow: UnitTween::at(if instant { 1.0 } else { 0.0 }), ..PanelState::default() });
            }
        }
        if !moved {
            return;
        }
        self.panel_scroll = 0.0;
        self.hot_row = None;
        if self.key_link.is_some() && !self.drawn_compact {
            self.key_link = None;
        }
        self.lock(cx);
        let id = if self.drawn_compact { None } else { self.defs.get(item).map(|d| d.id) };
        let uid = self.widget_uid();
        cx.widget_action(uid, PillNavAction::Opened(id.unwrap_or_else(menu_id)));
        self.kick(cx);
    }

    fn hide_panel(&mut self, cx: &mut Cx) {
        let instant = self.instant();
        let Some(panel) = &mut self.panel else {
            return;
        };
        if panel.closing {
            return;
        }
        panel.closing = true;
        panel.fading = None;
        if instant {
            self.panel = None;
        }
        self.hot_row = None;
        self.key_link = None;
        self.compact_key = None;
        // Released as the close starts, not when it ends: the page answers
        // the next press at once rather than after the animation.
        self.unlock(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, PillNavAction::Closed);
        self.kick(cx);
    }

    fn select(&mut self, cx: &mut Cx, index: usize) {
        let Some(item) = self.defs.get(index) else {
            return;
        };
        let id = item.id;
        if self.track_current && item.links.is_empty() {
            self.current_index = Some(index);
        }
        let uid = self.widget_uid();
        cx.widget_action(uid, PillNavAction::Selected(id));
        self.kick(cx);
    }

    fn activate_row(&mut self, cx: &mut Cx, row: CompactRow) {
        if !row_enabled(&self.defs, row) {
            return;
        }
        match row {
            CompactRow::Link(i, j) => {
                let (item, link) = (self.defs[i].id, self.defs[i].links[j].id);
                if self.track_current {
                    self.current_index = Some(i);
                }
                let uid = self.widget_uid();
                cx.widget_action(uid, PillNavAction::Picked { item, link });
                self.close(cx);
            }
            CompactRow::Item(i) if !self.defs[i].links.is_empty() => {
                // The accordion rule: one item's links at a time.
                self.expanded = if self.expanded == Some(i) { None } else { Some(i) };
                self.compact_key = Some(row);
                self.kick(cx);
            }
            CompactRow::Item(i) => {
                self.select(cx, i);
                self.close(cx);
            }
        }
    }

    /// Move the keyboard along the bar. An open panel follows: it moves to
    /// an item with links and closes on one without.
    fn move_key_item(&mut self, cx: &mut Cx, to: Option<usize>, into_panel: bool) {
        let Some(to) = to else {
            return;
        };
        self.key_item = Some(to);
        if self.panel_showing().is_some() {
            if self.defs.get(to).map_or(false, |d| d.has_panel()) {
                let change = self.intent.force_open(to);
                self.apply_change(cx, change);
                self.key_link = if into_panel { self.first_link(to) } else { None };
            } else {
                let change = self.intent.force_close();
                self.apply_change(cx, change);
            }
        }
        self.kick(cx);
    }

    fn keep_key_row_visible(&mut self) {
        if self.panel_scroll_max <= 0.0 {
            return;
        }
        let Some(row) = self.key_row() else {
            return;
        };
        let Some((_, rect)) = self.panel_rows.iter().find(|(r, _)| *r == row) else {
            return;
        };
        let top = rect.pos.y - self.panel_pad;
        let bottom = top + rect.size.y;
        if top < self.panel_scroll {
            self.panel_scroll = top;
        } else if bottom > self.panel_scroll + self.panel_visible_h {
            self.panel_scroll = bottom - self.panel_visible_h;
        }
        self.panel_scroll = self.panel_scroll.clamp(0.0, self.panel_scroll_max);
    }

    // -- the public surface -----------------------------------------------

    /// Replace the items. Closes any panel at once and keeps the current
    /// item only when it is still there and still a place.
    pub fn set_items(&mut self, cx: &mut Cx, items: Vec<PillNavItem>) {
        let was_open = self.panel_showing().is_some();
        self.current_index = keep_current(&self.defs, self.current_index, &items);
        self.defs = items;
        self.panel = None;
        self.intent = NavIntent::default();
        self.arm_timer(cx);
        self.unlock(cx);
        self.hover = None;
        self.pressed = None;
        self.hover_suppressed = None;
        self.key_item = None;
        self.key_link = None;
        self.compact_key = None;
        self.expanded = None;
        self.hot_row = None;
        self.keyboard_mode = false;
        self.panel_layout_cache = None;
        self.panel_hits.clear();
        if was_open {
            let uid = self.widget_uid();
            cx.widget_action(uid, PillNavAction::Closed);
        }
        self.repaint(cx);
    }

    pub fn items(&self) -> &[PillNavItem] {
        &self.defs
    }

    /// Make the item with `id` current, or none. Raises no action. An id
    /// that is not a place in the bar is ignored.
    pub fn set_current(&mut self, cx: &mut Cx, id: Option<LiveId>) {
        match id {
            None => self.current_index = None,
            Some(id) => match self.defs.iter().position(|item| item.id == id && item.links.is_empty()) {
                Some(index) => self.current_index = Some(index),
                None => return,
            },
        }
        self.kick(cx);
    }

    pub fn current(&self) -> Option<LiveId> {
        self.current_index.and_then(|i| self.defs.get(i)).map(|item| item.id)
    }

    /// Open the panel of the item with `id`. Moves no focus. On a folded bar
    /// the list opens with that item's links showing.
    pub fn open(&mut self, cx: &mut Cx, id: LiveId) {
        let Some(index) = self.defs.iter().position(|item| item.id == id && item.has_panel()) else {
            return;
        };
        if self.drawn_compact {
            self.expanded = Some(index);
            let change = self.intent.force_open(0);
            self.apply_change(cx, change);
            self.kick(cx);
        } else {
            let change = self.intent.force_open(index);
            self.apply_change(cx, change);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        let change = self.intent.force_close();
        self.apply_change(cx, change);
        // A panel the intent had already let go of can still be showing.
        self.hide_panel(cx);
    }

    /// Open or opening, not closing.
    pub fn is_open(&self) -> bool {
        self.panel_showing().is_some()
    }

    pub fn open_item(&self) -> Option<LiveId> {
        let panel = self.panel_showing()?;
        if self.drawn_compact {
            self.expanded.and_then(|i| self.defs.get(i)).map(|item| item.id)
        } else {
            self.defs.get(panel.item).map(|item| item.id)
        }
    }

    /// Enable or disable every item and link carrying `id`.
    pub fn set_enabled(&mut self, cx: &mut Cx, id: LiveId, enabled: bool) {
        for item in &mut self.defs {
            if item.id == id {
                item.enabled = enabled;
            }
            for link in &mut item.links {
                if link.id == id {
                    link.enabled = enabled;
                }
            }
        }
        self.repaint(cx);
    }

    /// Whether the last draw folded the bar.
    pub fn is_compact(&self) -> bool {
        self.drawn_compact
    }

    // -- drawing ----------------------------------------------------------

    fn draw_overlay(&mut self, cx: &mut Cx2d, fallback_bar: Rect) {
        let Some(mut list) = self.overlay.take() else {
            return;
        };
        // Begun on every draw, open or not: a list that is not begun keeps
        // showing what it showed last.
        list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());
        if self.panel.is_some() {
            self.draw_panel_now(cx, pass, fallback_bar);
        } else {
            self.panel_hits.clear();
            self.panel_rows.clear();
            self.panel_side = None;
        }
        cx.end_pass_sized_turtle();
        list.end(cx);
        self.overlay = Some(list);
    }

    fn place(&self, anchor: Rect, size: DVec2, pass: DVec2) -> Placed {
        place_overlay(&PlaceRequest {
            anchor,
            size,
            bounds: Rect { pos: dvec2(EDGE, EDGE), size: pass - dvec2(EDGE * 2.0, EDGE * 2.0) },
            gap: self.panel_gap,
            placement: self.panel_placement.placement(),
            match_anchor_width: false,
        })
    }

    fn full_content(&mut self, cx: &mut Cx2d, item: usize, bar: Rect, pass: DVec2) -> Option<PanelContent> {
        let def = self.defs.get(item)?.clone();
        let item_rect = *self.item_rects.get(item)?;
        let m = self.panel_metrics();
        let measures: Vec<RowMeasure> = def
            .links
            .iter()
            .map(|link| RowMeasure {
                label: measure(&self.draw_link_text, cx, &link.label),
                hint: measure(&self.draw_hint_text, cx, &link.hint),
            })
            .collect();
        let heading_widths: Vec<f64> = panel_columns(&def.links)
            .iter()
            .map(|(group, _)| measure(&self.draw_group_text, cx, group).x)
            .collect();
        let layout = panel_layout(&def.links, &measures, &heading_widths, &m);
        // The item's width over the bar's full height, so `panel_gap` is
        // measured from the bar's edge and every panel hangs at the same
        // height whatever `bar_pad` is.
        let anchor = Rect {
            pos: dvec2(bar.pos.x + item_rect.pos.x, bar.pos.y),
            size: dvec2(item_rect.size.x, bar.size.y),
        };
        let placed = self.place(anchor, layout.size, pass);
        let rows = def
            .links
            .iter()
            .enumerate()
            .map(|(j, link)| ContentRow {
                row: CompactRow::Link(item, j),
                rect: layout.rows[j],
                label: link.label.clone(),
                hint: link.hint.clone(),
                label_h: measures[j].label.y,
                enabled: def.enabled && link.enabled,
                indent: 0.0,
                chevron: None,
            })
            .collect();
        let headings = layout.columns.iter().filter_map(|c| c.heading.clone()).collect();
        let size = layout.size;
        self.panel_layout_cache = Some((item, layout));
        Some(PanelContent { anchor, placed, size, rows, headings })
    }

    fn compact_content(&mut self, cx: &mut Cx2d, bar: Rect, pass: DVec2) -> PanelContent {
        let m = self.panel_metrics();
        let mut rows = Vec::new();
        let mut y = m.panel_pad;
        let mut widest: f64 = 0.0;
        let mark = if self.chevron_size > 0.0 { self.chevron_gap + self.chevron_size } else { 0.0 };
        for (k, row) in compact_rows(&self.defs, self.expanded).into_iter().enumerate() {
            if k > 0 {
                y += m.row_gap;
            }
            let (label, hint, indent, chevron) = match row {
                CompactRow::Item(i) => {
                    let item = &self.defs[i];
                    let chevron = (!item.links.is_empty()).then_some(self.expanded == Some(i));
                    (item.label.clone(), String::new(), 0.0, chevron)
                }
                CompactRow::Link(i, j) => {
                    let link = &self.defs[i].links[j];
                    (link.label.clone(), link.hint.clone(), m.row_pad_x, None)
                }
            };
            let label_size = measure(&self.draw_link_text, cx, &label);
            let hint_size = measure(&self.draw_hint_text, cx, &hint);
            let height = row_height(RowMeasure { label: label_size, hint: hint_size }, &m);
            let marks = if chevron.is_some() { mark } else { 0.0 };
            widest = widest.max(indent + label_size.x.max(hint_size.x) + marks);
            rows.push(ContentRow {
                row,
                rect: Rect { pos: dvec2(m.panel_pad, y), size: dvec2(0.0, height) },
                label,
                hint,
                label_h: label_size.y,
                enabled: row_enabled(&self.defs, row),
                indent,
                chevron,
            });
            y += height;
        }
        let width = (widest + m.row_pad_x * 2.0).max(m.column_min_width).min(m.column_max_width.max(m.column_min_width));
        for row in &mut rows {
            row.rect.size.x = width;
        }
        let size = dvec2(width + m.panel_pad * 2.0, y + m.panel_pad);
        let anchor = Rect {
            pos: dvec2(bar.pos.x + self.pill_rect.pos.x, bar.pos.y),
            size: dvec2(self.pill_rect.size.x, bar.size.y),
        };
        let placed = self.place(anchor, size, pass);
        PanelContent { anchor, placed, size, rows, headings: Vec::new() }
    }

    fn draw_panel_now(&mut self, cx: &mut Cx2d, pass: DVec2, fallback_bar: Rect) {
        let Some(mut panel) = self.panel.clone() else {
            return;
        };
        let bar = if self.bar_rect.size.x > 0.0 { self.bar_rect } else { fallback_bar };
        let compact = self.drawn_compact;
        let content = if compact {
            Some(self.compact_content(cx, bar, pass))
        } else {
            self.full_content(cx, panel.item, bar, pass)
        };
        let Some(content) = content else {
            return;
        };
        let placed = content.placed.rect;
        self.panel_rect = placed;
        self.panel_anchor = content.anchor;
        self.panel_side = Some(content.placed.side);
        self.panel_visible_h = (placed.size.y - self.panel_pad * 2.0).max(0.0);
        self.panel_scroll_max = (content.size.y - placed.size.y).max(0.0);
        self.panel_scroll = self.panel_scroll.clamp(0.0, self.panel_scroll_max);
        self.panel_rows = content.rows.iter().map(|r| (r.row, r.rect)).collect();

        // The settled panel glides to where it belongs: to another item's
        // place on a switch, to a new height when a folded list changes.
        let (glide_secs, glide_ease) = if compact { (self.glide_secs, self.glide_ease) } else { (self.switch_secs, self.switch_ease) };
        panel.glide.aim(placed, glide_secs, glide_ease);
        if self.instant() {
            panel.glide.settle();
        }
        panel.aimed_item = Some(panel.item);
        if panel.glide.progress() < 1.0 {
            self.frame = cx.new_next_frame();
        }

        // It grows out of its own item's pill, and shrinks back into wherever
        // the sliding pill is by then. Opening from the sliding pill would
        // move the growth's origin sideways whenever the panel opens before
        // the pill has finished gliding to the item, which the default open
        // delay, shorter than the glide, makes the usual case.
        let pill = if panel.closing && self.highlight_fade.value() > 0.5 && !self.highlight.is_empty() {
            self.highlight_now(bar.size).translate(bar.pos)
        } else if compact {
            self.pill_rect.translate(bar.pos)
        } else {
            self.item_abs(panel.item).unwrap_or(content.anchor)
        };
        // A spring or a bounce swings the surface past where it settles and
        // back. The swing is kept on the screen; nothing is hit against it,
        // so it can neither keep a panel open nor let a press through.
        let screen = Rect { pos: dvec2(0.0, 0.0), size: pass };
        let settling = hold_inside(panel.glide.current(), screen.hull(placed));
        let morph = morph_at(panel.grow.value(), panel.grow.reached(), pill, settling, self.item_height * 0.5, self.panel_radius);
        let surface = hold_inside(morph.rect, screen.hull(pill).hull(settling));
        let dim = if self.disabled { self.disabled_opacity } else { 1.0 };
        self.draw_panel.radius = morph.radius as f32;
        self.draw_panel.morph = morph.morph as f32;
        self.draw_panel.opacity = dim;
        self.draw_panel.draw_abs(cx, surface);

        // The content is drawn where it will settle and clipped to the
        // growing surface: the panel reveals its words rather than squashing
        // them.
        let clip = Rect {
            pos: surface.pos + dvec2(1.0, 1.0),
            size: dvec2((surface.size.x - 2.0).max(0.0), (surface.size.y - 2.0).max(0.0)),
        };
        cx.begin_turtle(Walk::abs_rect(clip), Layout { clip_x: true, clip_y: true, ..Layout::default() });
        let (old_alpha, new_alpha) = match panel.fading {
            Some(_) => switch_alphas(panel.glide.progress()),
            None => (0.0, 1.0),
        };
        let visible = Rect {
            pos: dvec2(placed.pos.x, placed.pos.y + self.panel_pad),
            size: dvec2(placed.size.x, self.panel_visible_h),
        };
        let origin = placed.pos - dvec2(0.0, self.panel_scroll);
        let hits = self.draw_rows(cx, &content, origin, morph.content_alpha * new_alpha, visible);
        if let Some(old) = panel.fading {
            if let Some(old_content) = self.full_content(cx, old, bar, pass) {
                let old_origin = old_content.placed.rect.pos;
                let _ = self.draw_rows(cx, &old_content, old_origin, morph.content_alpha * old_alpha, visible);
                // The cache is for the keys, which are on the new panel.
                self.full_content(cx, panel.item, bar, pass);
            }
        }
        cx.end_turtle();

        // The ring stands outside the clip, and only on a settled panel: a
        // ring on a row that is still arriving points at nothing.
        let settled = panel.grow.is_at(1.0) && panel.fading.is_none() && panel.glide.progress() >= 1.0;
        if settled {
            if let Some(key) = self.key_row() {
                if let Some(hit) = hits.iter().find(|h| h.row == key && h.rect.size.y > 0.0) {
                    let ring = hit.rect.add_margin(dvec2(RING_OUT, RING_OUT));
                    self.draw_focus.radius = (self.panel_radius * 0.5 + RING_OUT) as f32;
                    self.draw_focus.opacity = 1.0;
                    self.draw_focus.draw_abs(cx, ring);
                }
            }
        }
        self.panel_hits = if panel.closing { Vec::new() } else { hits };
        // Only the glide's aim is written back: `p` and the rest belong to
        // the frame handler.
        if let Some(live) = &mut self.panel {
            live.glide = panel.glide;
            live.aimed_item = panel.aimed_item;
        }
    }

    fn draw_rows(&mut self, cx: &mut Cx2d, content: &PanelContent, origin: DVec2, alpha: f64, visible: Rect) -> Vec<PanelHit> {
        let mut hits = Vec::with_capacity(content.rows.len());
        let alpha = alpha as f32;
        let (row_pad_x, row_pad_y, hint_gap) = (self.row_pad_x, self.row_pad_y, self.hint_gap);
        let (chevron_size, chevron_gap) = (self.chevron_size, self.chevron_gap);
        let row_radius = (self.panel_radius - self.panel_pad).max(4.0);
        let key = self.key_row();
        let top = visible.pos.y;
        let bottom = visible.pos.y + visible.size.y;
        for (title, rect) in &content.headings {
            let at = rect.translate(origin);
            if at.pos.y + at.size.y < top || at.pos.y > bottom {
                continue;
            }
            let size = measure(&self.draw_group_text, cx, title);
            self.draw_group_text.hot = 0.0;
            self.draw_group_text.disabled = 0.0;
            self.draw_group_text.opacity = alpha;
            let text = elide(&self.draw_group_text, cx, title, at.size.x - row_pad_x * 2.0);
            self.draw_group_text.draw_abs(cx, dvec2(at.pos.x + row_pad_x, at.pos.y + (at.size.y - size.y) * 0.5), &text);
        }
        for row in &content.rows {
            let at = row.rect.translate(origin);
            if at.pos.y + at.size.y < top || at.pos.y > bottom {
                // Wholly scrolled out: no pixels and no target.
                hits.push(PanelHit { rect: Rect::default(), row: row.row, enabled: row.enabled, label: row.label.clone() });
                continue;
            }
            let hot = row.enabled && self.hot_row == Some(row.row);
            let keyed = key == Some(row.row);
            if hot || keyed {
                self.draw_row.radius = row_radius as f32;
                self.draw_row.morph = 1.0;
                self.draw_row.opacity = alpha;
                self.draw_row.draw_abs(cx, at);
            }
            let marks = if row.chevron.is_some() && chevron_size > 0.0 { chevron_gap + chevron_size } else { 0.0 };
            let room = at.size.x - row_pad_x * 2.0 - row.indent - marks;
            let x = at.pos.x + row_pad_x + row.indent;
            let disabled = if row.enabled { 0.0 } else { 1.0 };
            self.draw_link_text.hot = if hot { 1.0 } else { 0.0 };
            self.draw_link_text.disabled = disabled;
            self.draw_link_text.opacity = alpha;
            let label = elide(&self.draw_link_text, cx, &row.label, room);
            self.draw_link_text.draw_abs(cx, dvec2(x, at.pos.y + row_pad_y), &label);
            if !row.hint.is_empty() {
                self.draw_hint_text.hot = if hot { 1.0 } else { 0.0 };
                self.draw_hint_text.disabled = disabled;
                self.draw_hint_text.opacity = alpha;
                let hint = elide(&self.draw_hint_text, cx, &row.hint, room);
                self.draw_hint_text.draw_abs(cx, dvec2(x, at.pos.y + row_pad_y + row.label_h + hint_gap), &hint);
            }
            if let (Some(open), true) = (row.chevron, chevron_size > 0.0) {
                self.draw_mark.kind = 0.0;
                self.draw_mark.open = if open { 1.0 } else { 0.0 };
                self.draw_mark.hot = if hot { 1.0 } else { 0.0 };
                self.draw_mark.opacity = alpha * if row.enabled { 1.0 } else { self.disabled_opacity };
                self.draw_mark.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(at.pos.x + at.size.x - row_pad_x - chevron_size, at.pos.y + (at.size.y - chevron_size) * 0.5),
                        size: dvec2(chevron_size, chevron_size),
                    },
                );
            }
            let clipped = at.clip((dvec2(visible.pos.x, top), dvec2(visible.pos.x + visible.size.x, bottom)));
            hits.push(PanelHit { rect: clipped, row: row.row, enabled: row.enabled, label: row.label.clone() });
        }
        hits
    }

    // -- events -----------------------------------------------------------

    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        if ke.key_code == KeyCode::Escape {
            // Read raw, so a panel opened by hover can be sent away without
            // the bar ever having had the focus.
            return;
        }
        if matches!(ke.key_code, KeyCode::Tab) {
            self.close(cx);
            return;
        }
        let was = self.keyboard_mode;
        self.keyboard_mode = true;
        if self.drawn_compact {
            self.handle_compact_key(cx, ke);
        } else {
            self.handle_bar_key(cx, ke);
        }
        if !was {
            self.kick(cx);
        }
    }

    fn handle_bar_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let enabled = self.enabled_items();
        let open_item = self.panel_showing().map(|p| p.item);
        if let (Some(item), Some(link)) = (open_item, self.key_link) {
            let key = match ke.key_code {
                KeyCode::ArrowDown => Some(LinkKey::Down),
                KeyCode::ArrowUp => Some(LinkKey::Up),
                KeyCode::ArrowRight => Some(LinkKey::Right),
                KeyCode::ArrowLeft => Some(LinkKey::Left),
                KeyCode::Home => Some(LinkKey::Home),
                KeyCode::End => Some(LinkKey::End),
                _ => None,
            };
            if let Some(key) = key {
                match step_link(&self.layout_for_keys(item), &self.link_enabled(item), Some(link), key) {
                    LinkStep::To(to) => {
                        self.key_link = Some(to);
                        self.keep_key_row_visible();
                    }
                    LinkStep::Bar => self.key_link = None,
                    LinkStep::PrevItem => {
                        let to = step_item(&enabled, Some(item), -1);
                        self.move_key_item(cx, to, true);
                    }
                    LinkStep::NextItem => {
                        let to = step_item(&enabled, Some(item), 1);
                        self.move_key_item(cx, to, true);
                    }
                    LinkStep::Stay => {}
                }
                self.kick(cx);
                return;
            }
            if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) {
                self.activate_row(cx, CompactRow::Link(item, link));
            }
            return;
        }
        match ke.key_code {
            KeyCode::ArrowRight | KeyCode::ArrowLeft => {
                let delta = if ke.key_code == KeyCode::ArrowRight { 1 } else { -1 };
                let from = self.key_item.or(open_item).or(self.current_index);
                let to = step_item(&enabled, from, delta);
                self.move_key_item(cx, to, false);
            }
            KeyCode::Home => {
                let to = step_item(&enabled, None, 1);
                self.move_key_item(cx, to, false);
            }
            KeyCode::End => {
                let to = step_item(&enabled, None, -1);
                self.move_key_item(cx, to, false);
            }
            KeyCode::ArrowDown | KeyCode::ReturnKey | KeyCode::Space => {
                let Some(index) = self
                    .key_item
                    .or(self.current_index)
                    .filter(|i| enabled.get(*i).copied().unwrap_or(false))
                    .or_else(|| step_item(&enabled, None, 1))
                else {
                    return;
                };
                self.key_item = Some(index);
                let has_panel = self.defs[index].has_panel() && self.open_on != PopoverTrigger::Manual;
                if has_panel {
                    let change = self.intent.force_open(index);
                    self.apply_change(cx, change);
                    self.key_link = self.first_link(index);
                    self.kick(cx);
                } else if ke.key_code != KeyCode::ArrowDown {
                    self.select(cx, index);
                }
            }
            _ => {}
        }
    }

    fn handle_compact_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        if self.panel_showing().is_none() {
            if matches!(ke.key_code, KeyCode::ArrowDown | KeyCode::ReturnKey | KeyCode::Space) {
                let change = self.intent.force_open(0);
                self.apply_change(cx, change);
                self.compact_key = step_compact(&self.defs, self.expanded, None, CompactKey::Down).1;
                self.kick(cx);
            }
            return;
        }
        let key = match ke.key_code {
            KeyCode::ArrowDown => Some(CompactKey::Down),
            KeyCode::ArrowUp => Some(CompactKey::Up),
            KeyCode::ArrowRight => Some(CompactKey::Right),
            KeyCode::ArrowLeft => Some(CompactKey::Left),
            KeyCode::Home => Some(CompactKey::Home),
            KeyCode::End => Some(CompactKey::End),
            _ => None,
        };
        if let Some(key) = key {
            let (expanded, at) = step_compact(&self.defs, self.expanded, self.compact_key, key);
            self.expanded = expanded;
            self.compact_key = at;
            self.keep_key_row_visible();
            self.kick(cx);
        } else if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) {
            if let Some(row) = self.compact_key {
                self.activate_row(cx, row);
            }
        }
    }

    /// The pointer moved to `abs`, or left the window.
    fn pointer_moved(&mut self, cx: &mut Cx, abs: Option<DVec2>) {
        if self.drawn_compact {
            return;
        }
        if !(self.bar_hovered || self.intent.open.is_some() || self.intent.pending.is_some()) {
            return;
        }
        let over = match abs {
            None => Over::Nothing,
            Some(abs) => self.classify(abs),
        };
        let over = match over {
            Over::Item(i) if self.hover_suppressed == Some(i) => Over::Bar,
            other => {
                if !matches!(other, Over::Item(_)) {
                    self.hover_suppressed = None;
                }
                other
            }
        };
        let now = cx.seconds_since_app_start();
        let times = self.times(false);
        let defs = &self.defs;
        let has_panel = |i: usize| defs.get(i).map_or(false, |d| d.has_panel());
        let change = self.intent.pointer(over, now, &times, &has_panel);
        self.apply_change(cx, change);
    }

    fn classify(&self, abs: DVec2) -> Over {
        if let Some(i) = self.item_at(abs) {
            // Something laid over the bar turns its hits away, and the bar
            // must not open a panel under it.
            if !self.bar_hovered {
                return Over::Nothing;
            }
            return if self.defs.get(i).map_or(false, |d| d.enabled) { Over::Item(i) } else { Over::Bar };
        }
        if self.bar_rect.contains(abs) {
            return if self.bar_hovered { Over::Bar } else { Over::Nothing };
        }
        if self.panel_showing().is_some() {
            if self.panel_rect.contains(abs) {
                return Over::Panel;
            }
            if let Some(side) = self.panel_side {
                if bridge_rect(self.panel_anchor, self.panel_rect, side).contains(abs) {
                    return Over::Bridge;
                }
            }
        }
        Over::Nothing
    }

    fn inside_union(&self, abs: DVec2) -> bool {
        self.bar_rect.contains(abs)
            || self.panel_rect.contains(abs)
            || self
                .panel_side
                .map_or(false, |side| bridge_rect(self.panel_anchor, self.panel_rect, side).contains(abs))
    }

    fn row_at(&self, abs: DVec2) -> Option<&PanelHit> {
        self.panel_hits.iter().find(|hit| hit.rect.size.y > 0.0 && hit.rect.contains(abs))
    }

    fn handle_panel_hits(&mut self, cx: &mut Cx, event: &Event) {
        if self.panel_showing().is_none() {
            return;
        }
        // The bar owns the lock, and the panel is another area of the same
        // widget: it hits with the bar as its sweep area.
        match event.hits_with_sweep_area(cx, self.draw_panel.area(), self.draw_bar.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.row_at(fe.abs).filter(|hit| hit.enabled).map(|hit| hit.row);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hot != self.hot_row {
                    self.hot_row = hot;
                    self.repaint(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot_row.take().is_some() {
                    self.repaint(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.keyboard_mode = false;
            }
            Hit::FingerScroll(fe) if self.panel_scroll_max > 0.0 => {
                let next = (self.panel_scroll + fe.scroll.y).clamp(0.0, self.panel_scroll_max);
                if next != self.panel_scroll {
                    self.panel_scroll = next;
                    self.repaint(cx);
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && fe.is_over => {
                let row = self.row_at(fe.abs).filter(|hit| hit.enabled).map(|hit| hit.row);
                if let Some(row) = row {
                    let touch = fe.device.is_touch();
                    self.activate_row(cx, row);
                    if touch {
                        self.hover = None;
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_bar_hits(&mut self, cx: &mut Cx, event: &Event) {
        // The bar is its own sweep area. A plain `hits` has none, and the
        // lock turns away every area whose sweep area is not the owner, the
        // owner's own included: without this a press on the pill cannot
        // close the list it opened.
        let bar = self.draw_bar.area();
        match event.hits_with_sweep_area(cx, bar, bar) {
            Hit::KeyFocus(_) => {
                self.focused = true;
                if !self.focus_by_press {
                    // Arrived by Tab: the keys are in use, so show where
                    // they are.
                    self.keyboard_mode = true;
                    let enabled = self.enabled_items();
                    self.key_item = self
                        .current_index
                        .filter(|i| enabled.get(*i).copied().unwrap_or(false))
                        .or_else(|| step_item(&enabled, None, 1));
                }
                self.focus_by_press = false;
                self.kick(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.focused = false;
                self.keyboard_mode = false;
                self.close(cx);
                self.kick(cx);
            }
            Hit::KeyDown(ke) => self.handle_key(cx, &ke),
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hover = self.item_at(fe.abs).filter(|i| self.defs.get(*i).map_or(false, |d| d.enabled));
                let over_pill = self.drawn_compact && self.pill_rect.translate(self.bar_rect.pos).contains(fe.abs);
                cx.set_cursor(if hover.is_some() || over_pill { MouseCursor::Hand } else { MouseCursor::Default });
                if !self.bar_hovered || hover != self.hover {
                    self.bar_hovered = true;
                    self.hover = hover;
                    self.kick(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.bar_hovered = false;
                if self.hover.take().is_some() || self.drawn_compact {
                    self.kick(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.focus_by_press = !self.focused;
                cx.set_key_focus(self.draw_bar.area());
                self.keyboard_mode = false;
                let touch = fe.device.is_touch();
                if self.drawn_compact {
                    if self.pill_rect.translate(self.bar_rect.pos).contains(fe.abs) {
                        let change = if self.panel_showing().is_some() {
                            self.intent.force_close()
                        } else {
                            self.intent.force_open(0)
                        };
                        self.apply_change(cx, change);
                    }
                    return;
                }
                let Some(index) = self.item_at(fe.abs).filter(|i| self.defs[*i].enabled) else {
                    return;
                };
                self.pressed = Some(index);
                if touch {
                    self.hover = Some(index);
                }
                let times = self.times(touch);
                let defs = &self.defs;
                let has_panel = |i: usize| defs.get(i).map_or(false, |d| d.has_panel());
                let change = self.intent.press(index, &times, &has_panel);
                if change == Some(IntentChange::Closed) {
                    self.hover_suppressed = Some(index);
                }
                self.apply_change(cx, change);
                self.kick(cx);
            }
            Hit::FingerUp(fe) => {
                let Some(index) = self.pressed.take() else {
                    return;
                };
                if fe.is_primary_hit() && fe.is_over && fe.was_tap() && self.item_at(fe.abs) == Some(index) {
                    let item = &self.defs[index];
                    if item.links.is_empty() || self.open_on == PopoverTrigger::Manual {
                        self.select(cx, index);
                    }
                }
                if fe.device.is_touch() {
                    self.hover = None;
                    self.kick(cx);
                }
            }
            _ => {}
        }
    }

    fn handle_raw(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::MouseMove(me) => {
                if self.keyboard_mode {
                    self.keyboard_mode = false;
                    self.kick(cx);
                }
                self.pointer_moved(cx, Some(me.abs));
            }
            Event::MouseLeave(_) | Event::ClearHover => self.pointer_moved(cx, None),
            Event::TouchUpdate(te) => {
                if self.keyboard_mode {
                    self.keyboard_mode = false;
                    self.kick(cx);
                }
                if self.panel_showing().is_some()
                    && !te.touches.is_empty()
                    && te.touches.iter().all(|t| t.state == TouchState::Start && !self.inside_union(t.abs))
                {
                    self.close(cx);
                }
            }
            Event::MouseDown(me) if self.panel_showing().is_some() => {
                if !self.inside_union(me.abs) {
                    // The lock has already refused this press to everything
                    // walked before the bar; marking it spent refuses it to
                    // everything after, so a dismissing press does one thing.
                    if me.handled.get().is_empty() {
                        me.handled.set(self.draw_bar.area());
                    }
                    self.close(cx);
                }
            }
            // The bar is about to move with the page, and a panel left
            // where the bar was is wrong.
            Event::Scroll(se) if self.panel_showing().is_some() => {
                if !self.panel_rect.contains(se.abs) {
                    self.close(cx);
                }
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape && self.panel_showing().is_some() => {
                let top = cx.sweep_lock_area();
                let mine = top.is_none() || (self.locked && top == Some(self.draw_bar.area()));
                if mine && claim_escape(cx) {
                    if !self.drawn_compact {
                        self.key_item = self.panel_showing().map(|p| p.item);
                    }
                    self.key_link = None;
                    self.close(cx);
                }
            }
            Event::WindowLostFocus(_) => self.close(cx),
            _ => {}
        }
        if self.panel_showing().is_some() && event.back_pressed() {
            self.close(cx);
        }
    }
}

impl Drop for PillNav {
    fn drop(&mut self) {
        // Dropped holding the pointer, as when its page is swapped for
        // another while a panel or the folded list is out: nothing else would
        // ever let go of the lock, and every press in the window would go on
        // being turned away. A drop has no `Cx`, so the lock is left for the
        // next event to release.
        if self.locked {
            orphan_sweep_locks(&[self.draw_bar.area()]);
        }
    }
}

/// The folded pill's id, in the test tree and in `Opened`. Interned, so the
/// tree prints the word a test looks the pill up by rather than a hash.
fn menu_id() -> LiveId {
    LiveId::from_str_with_lut("menu").unwrap_or(live_id!(menu))
}

impl Widget for PillNav {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            self.draw_overlay(cx, Rect::default());
            return DrawStep::done();
        }
        let m = self.bar_metrics();
        let labels: Vec<String> = self.defs.iter().map(|item| item.label.clone()).collect();
        // Every word is measured in the bold face, so the row never shuffles
        // when the current item, drawn bold, moves.
        let label_widths: Vec<f64> = labels.iter().map(|label| measure(&self.draw_item_text_current, cx, label).x).collect();
        let widths: Vec<f64> = self
            .defs
            .iter()
            .zip(&label_widths)
            .map(|(item, w)| {
                let mark = if !item.links.is_empty() && self.chevron_size > 0.0 { self.chevron_gap + self.chevron_size } else { 0.0 };
                self.item_pad_x * 2.0 + w + mark
            })
            .collect();
        let natural = bar_natural_size(&widths, &m);
        let room = if self.compact_when_crowded { cx.turtle().max_width(Walk { width: Size::fill(), ..walk }) } else { None };
        let compact = self.compact || is_crowded(natural.x, room);
        if compact != self.drawn_compact && self.panel.is_some() {
            // The panel belongs to the shape the bar had.
            self.panel = None;
            self.intent = NavIntent::default();
            self.unlock(cx);
            let uid = self.widget_uid();
            cx.widget_action(uid, PillNavAction::Closed);
        }
        self.drawn_compact = compact;

        let word = self.compact_word();
        let word_w = if compact { measure(&self.draw_item_text_current, cx, &word).x } else { 0.0 };
        let wanted = if compact {
            dvec2(
                self.bar_pad * 2.0 + self.item_pad_x * 2.0 + COMPACT_MARK + COMPACT_MARK_GAP + word_w,
                self.bar_pad * 2.0 + self.item_height,
            )
        } else {
            natural
        };
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(wanted.x),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(wanted.y),
                other => other,
            },
            ..walk
        };
        let rect = cx.walk_turtle(walk);
        let dim = if self.disabled { self.disabled_opacity } else { 1.0 };
        self.draw_bar.radius = (rect.size.y * 0.5) as f32;
        self.draw_bar.morph = 1.0;
        self.draw_bar.opacity = dim;
        self.draw_bar.draw_abs(cx, rect);

        if compact {
            let y = self.bar_pad + (rect.size.y - wanted.y).max(0.0) * 0.5;
            self.pill_rect = Rect {
                pos: dvec2(self.bar_pad, y),
                size: dvec2((rect.size.x - self.bar_pad * 2.0).max(0.0), self.item_height),
            };
            self.item_rects.clear();
        } else {
            self.item_rects = bar_layout(&widths, &m, rect.size).items;
        }

        // The pill is aimed here too, so a first draw shows it where it
        // belongs; only the frame handler moves it along.
        let target = self.highlight_target();
        if let Some(target) = target {
            self.highlight.aim(target, self.glide_secs, self.glide_ease);
            if self.highlight.progress() < 1.0 || !self.highlight_fade.is_at(1.0) {
                self.frame = cx.new_next_frame();
            }
        } else if !self.highlight_fade.is_at(0.0) {
            self.frame = cx.new_next_frame();
        }
        // An opacity past 1 brightens what the pill is drawn over, so the
        // fade's swing is cut at both ends.
        let opacity = self.highlight_fade.value().clamp(0.0, 1.0);
        if opacity > 0.0 && !self.highlight.is_empty() {
            self.draw_highlight.radius = (self.item_height * 0.5) as f32;
            self.draw_highlight.morph = 1.0;
            self.draw_highlight.opacity = opacity as f32 * dim;
            let pill = self.highlight_now(rect.size).translate(rect.pos);
            self.draw_highlight.draw_abs(cx, pill);
        }
        let lit = target;
        // The chevron turns with the grow, swing and all.
        let open_morph = self.panel.as_ref().map(|p| (p.item, p.grow.value() as f32));

        if compact {
            let pill = self.pill_rect.translate(rect.pos);
            let hot = if lit.is_some() { 1.0 } else { 0.0 };
            self.draw_mark.kind = 1.0;
            self.draw_mark.open = 0.0;
            self.draw_mark.hot = hot;
            self.draw_mark.opacity = dim;
            self.draw_mark.draw_abs(
                cx,
                Rect {
                    pos: dvec2(pill.pos.x + self.item_pad_x, pill.pos.y + (pill.size.y - COMPACT_MARK) * 0.5),
                    size: dvec2(COMPACT_MARK, COMPACT_MARK),
                },
            );
            let x = pill.pos.x + self.item_pad_x + COMPACT_MARK + COMPACT_MARK_GAP;
            let dt = &mut self.draw_item_text_current;
            dt.hot = 1.0;
            dt.disabled = if self.disabled { 1.0 } else { 0.0 };
            dt.opacity = 1.0;
            let size = measure(dt, cx, &word);
            dt.draw_abs(cx, dvec2(x, pill.pos.y + (pill.size.y - size.y) * 0.5), &word);
        } else {
            let (pad, chevron_size) = (self.item_pad_x, self.chevron_size);
            let lit_index = lit.and_then(|r| self.item_rects.iter().position(|i| *i == r));
            for i in 0..self.defs.len() {
                let item_rect = self.item_rects[i].translate(rect.pos);
                let item = &self.defs[i];
                let has_links = !item.links.is_empty();
                let enabled = item.enabled && !self.disabled;
                let is_current = self.current_index == Some(i);
                let hot = if is_current || lit_index == Some(i) { 1.0 } else { 0.0 };
                let label = labels[i].clone();
                let dt = if is_current { &mut self.draw_item_text_current } else { &mut self.draw_item_text };
                dt.hot = hot;
                dt.disabled = if enabled { 0.0 } else { 1.0 };
                dt.opacity = 1.0;
                let size = measure(dt, cx, &label);
                // Centred in the room the bold word takes, so the regular
                // word sits where the bold one will.
                let x = item_rect.pos.x + pad + (label_widths[i] - size.x).max(0.0) * 0.5;
                dt.draw_abs(cx, dvec2(x, item_rect.pos.y + (item_rect.size.y - size.y) * 0.5), &label);
                if has_links && chevron_size > 0.0 {
                    self.draw_mark.kind = 0.0;
                    self.draw_mark.open = open_morph.filter(|(item, _)| *item == i).map_or(0.0, |(_, m)| m);
                    self.draw_mark.hot = hot;
                    self.draw_mark.opacity = if enabled { 1.0 } else { self.disabled_opacity };
                    self.draw_mark.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(
                                item_rect.pos.x + item_rect.size.x - pad - chevron_size,
                                item_rect.pos.y + (item_rect.size.y - chevron_size) * 0.5,
                            ),
                            size: dvec2(chevron_size, chevron_size),
                        },
                    );
                }
            }
        }

        // The ring goes around the keyboard's item only for someone using
        // the keys, and only while the keys are in the bar rather than in
        // the panel.
        if self.focused && self.keyboard_mode && self.key_row().is_none() {
            let ringed = if compact {
                Some(self.pill_rect)
            } else {
                self.key_item.and_then(|i| self.item_rects.get(i).copied())
            };
            if let Some(ringed) = ringed {
                self.draw_focus.radius = (self.item_height * 0.5 + RING_OUT) as f32;
                self.draw_focus.opacity = 1.0;
                self.draw_focus.draw_abs(cx, ringed.translate(rect.pos).add_margin(dvec2(RING_OUT, RING_OUT)));
            }
        }

        if !self.disabled {
            cx.add_nav_stop(self.draw_bar.area(), NavRole::TextInput, Inset::default());
        }
        if self.locked {
            // The area is a fresh handle after every draw; the lock keeps one
            // entry per owner and takes the newest.
            cx.sweep_lock(self.draw_bar.area());
        }

        self.draw_overlay(cx, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.frame.is_event(event) {
            let dt = if self.last_time > 0.0 { (ne.time - self.last_time).clamp(0.0, 0.1) } else { 0.0 };
            let moving = self.advance(dt);
            self.repaint(cx);
            if moving {
                self.last_time = ne.time;
                self.frame = cx.new_next_frame();
            } else {
                self.last_time = 0.0;
            }
        }
        if self.timer.is_event(event).is_some() {
            self.timer = Timer::empty();
            let now = cx.seconds_since_app_start();
            let change = self.intent.tick(now);
            self.apply_change(cx, change);
        }

        let rect = self.draw_bar.area().rect(cx);
        if rect.size.x > 0.0 || rect.size.y > 0.0 {
            self.bar_rect = rect;
        }

        if self.disabled || !self.visible {
            if self.panel_showing().is_some() {
                self.close(cx);
            }
            return;
        }

        self.handle_bar_hits(cx, event);
        self.handle_panel_hits(cx, event);
        self.handle_raw(cx, event);
    }

    /// The current item's label.
    fn text(&self) -> String {
        self.current_index.and_then(|i| self.defs.get(i)).map(|item| item.label.clone()).unwrap_or_default()
    }

    /// Make current the item whose label or id is `v`.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        let id = LiveId::from_str(v);
        if let Some(item) = self.defs.iter().find(|item| item.label == v || item.id == id) {
            let id = item.id;
            self.set_current(cx, Some(id));
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            if disabled {
                self.close(cx);
            }
            self.repaint(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        self.current_index.and_then(|i| self.defs.get(i)).map(|item| item.label.clone())
    }

    /// The open item's label while a panel is open or opening, so a test
    /// can wait for it before pressing a row that is still growing.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let Some(panel) = self.panel_showing() else {
            return Some(String::new());
        };
        if self.drawn_compact {
            return Some(self.compact_word());
        }
        Some(self.defs.get(panel.item).map(|item| item.label.clone()).unwrap_or_default())
    }

    fn snapshot_parts(&self, cx: &Cx) -> Vec<SnapshotPart> {
        let area = self.draw_bar.area();
        // Not drawn this frame (another tab, a page scrolled away): a stale
        // area reads as a zero rect, and the cached item rects moved by it
        // would send a test to press empty space by the window's corner.
        if !area.is_valid(cx) {
            return Vec::new();
        }
        let bar = area.rect(cx);
        // The bar's items as they are on screen: an ancestor that clips the
        // bar clips them. The panel is on an overlay, which nothing clips.
        let shown = area.clipped_rect(cx);
        let on_screen = |rect: Rect| rect.clip((shown.pos, shown.pos + shown.size));
        let mut parts = Vec::new();
        if self.drawn_compact {
            parts.push(SnapshotPart {
                id: menu_id(),
                widget_type: "PillNavMenu",
                rect: on_screen(self.pill_rect.translate(bar.pos)),
                text: self.compact_word(),
                selected: false,
                enabled: !self.disabled,
            });
        } else {
            for (i, item) in self.defs.iter().enumerate() {
                let Some(rect) = self.item_rects.get(i) else {
                    continue;
                };
                parts.push(SnapshotPart {
                    id: item.id,
                    widget_type: "PillNavItem",
                    rect: on_screen(rect.translate(bar.pos)),
                    text: item.label.clone(),
                    selected: self.current_index == Some(i),
                    enabled: item.enabled && !self.disabled,
                });
            }
        }
        // Rows only once the panel has settled: a test must not press a row
        // that is still arriving.
        let Some(panel) = self.panel_showing() else {
            return parts;
        };
        if !panel.grow.is_at(1.0) || panel.fading.is_some() || panel.glide.progress() < 1.0 {
            return parts;
        }
        if !self.drawn_compact {
            if let Some(item) = self.defs.get(panel.item) {
                parts.push(SnapshotPart {
                    id: item.id,
                    widget_type: "PillNavPanel",
                    rect: self.panel_rect,
                    text: item.label.clone(),
                    selected: false,
                    enabled: true,
                });
            }
        }
        for hit in &self.panel_hits {
            if hit.rect.size.x <= 0.0 || hit.rect.size.y <= 0.0 {
                continue;
            }
            let (id, widget_type, selected) = match hit.row {
                CompactRow::Item(i) => (self.defs[i].id, "PillNavItem", self.current_index == Some(i)),
                CompactRow::Link(i, j) => (self.defs[i].links[j].id, "PillNavLink", false),
            };
            parts.push(SnapshotPart { id, widget_type, rect: hit.rect, text: hit.label.clone(), selected, enabled: hit.enabled });
        }
        parts
    }
}

impl PillNavRef {
    fn each_action(&self, actions: &Actions) -> Vec<PillNavAction> {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == uid)
            .map(|action| action.cast::<PillNavAction>())
            .collect()
    }

    /// An item without a panel chosen this pass.
    pub fn selected(&self, actions: &Actions) -> Option<LiveId> {
        self.each_action(actions).into_iter().find_map(|a| match a {
            PillNavAction::Selected(id) => Some(id),
            _ => None,
        })
    }

    /// A link chosen this pass, with its item. `Closed` arrives in the same
    /// pass, so this looks past it.
    pub fn picked(&self, actions: &Actions) -> Option<(LiveId, LiveId)> {
        self.each_action(actions).into_iter().find_map(|a| match a {
            PillNavAction::Picked { item, link } => Some((item, link)),
            _ => None,
        })
    }

    pub fn opened(&self, actions: &Actions) -> Option<LiveId> {
        self.each_action(actions).into_iter().find_map(|a| match a {
            PillNavAction::Opened(id) => Some(id),
            _ => None,
        })
    }

    pub fn closed(&self, actions: &Actions) -> bool {
        self.each_action(actions).into_iter().any(|a| a == PillNavAction::Closed)
    }

    pub fn set_items(&self, cx: &mut Cx, items: Vec<PillNavItem>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_items(cx, items);
        }
    }

    pub fn items(&self) -> Vec<PillNavItem> {
        self.borrow().map(|inner| inner.items().to_vec()).unwrap_or_default()
    }

    pub fn set_current(&self, cx: &mut Cx, id: Option<LiveId>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_current(cx, id);
        }
    }

    pub fn current(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.current())
    }

    pub fn open(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx, id);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map_or(false, |inner| inner.is_open())
    }

    pub fn open_item(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.open_item())
    }

    pub fn set_enabled(&self, cx: &mut Cx, id: LiveId, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, id, enabled);
        }
    }

    pub fn is_compact(&self) -> bool {
        self.borrow().map_or(false, |inner| inner.is_compact())
    }

    /// The label of the item with `id`, or of the link with `id`, for a host
    /// turning a report into words.
    pub fn label_of(&self, id: LiveId) -> String {
        let Some(inner) = self.borrow() else {
            return String::new();
        };
        for item in inner.items() {
            if item.id == id {
                return item.label.clone();
            }
            if let Some(link) = item.links.iter().find(|link| link.id == id) {
                return link.label.clone();
            }
        }
        String::new()
    }
}

#[cfg(test)]
mod tests {

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }
    use super::*;
    use crate::makepad_script::script;
    use crate::makepad_script::trap::NoTrap;

    fn id(name: &str) -> LiveId {
        LiveId::from_str(name)
    }

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

    /// The easing a theme token names, read out of the loaded theme as the
    /// Foundations page reads it, so these checks follow the theme and never
    /// restate it.
    fn theme_ease(vm: &mut ScriptVm, token: &str) -> Ease {
        let theme = vm.module(id!(theme));
        let value = vm.bx.heap.value(theme, LiveId::from_str(token).into(), NoTrap);
        Ease::script_from_value(vm, value)
    }

    fn theme_number(vm: &mut ScriptVm, token: &str) -> f64 {
        let theme = vm.module(id!(theme));
        vm.bx
            .heap
            .value(theme, LiveId::from_str(token).into(), NoTrap)
            .as_f64()
            .unwrap_or_else(|| panic!("the theme has no number {token}"))
    }

    /// Every theme easing by token, from a VM with the widgets loaded.
    fn theme_eases() -> Vec<(&'static str, Ease)> {
        let mut cx = test_cx();
        cx.with_vm(|vm| EASE_TOKENS.iter().map(|token| (*token, theme_ease(vm, token))).collect())
    }

    fn named(eases: &[(&'static str, Ease)], token: &str) -> Ease {
        eases.iter().find(|(name, _)| *name == token).map(|(_, ease)| *ease).expect("a theme easing")
    }

    fn metrics() -> PanelMetrics {
        PanelMetrics {
            panel_pad: 8.0,
            column_gap: 8.0,
            column_min_width: 180.0,
            column_max_width: 320.0,
            row_pad_x: 10.0,
            row_pad_y: 6.0,
            row_gap: 2.0,
            hint_gap: 2.0,
            group_height: 24.0,
        }
    }

    fn times(open_on: PopoverTrigger) -> IntentTimes {
        IntentTimes { open_on, open_delay: 0.1, switch_delay: 0.06, close_delay: 0.2 }
    }

    /// Items 1 and 2 own panels; 0 and 3 are places.
    fn has_panel(i: usize) -> bool {
        i == 1 || i == 2
    }

    /// Two columns: Make holds links 0, 1 and 2, Ship holds 3 and 4. Every
    /// row carries a hint, so every row is 44 points tall.
    fn build_panel() -> (Vec<PillNavLink>, PanelLayout) {
        let links = vec![
            PillNavLink::new(id("editor"), "Editor").with_hint("Write").in_group("Make"),
            PillNavLink::new(id("debugger"), "Debugger").with_hint("Step").in_group("Make"),
            PillNavLink::new(id("profiler"), "Profiler").with_hint("Time").in_group("Make"),
            PillNavLink::new(id("hosting"), "Hosting").with_hint("Serve").in_group("Ship"),
            PillNavLink::new(id("updates"), "Updates").with_hint("Send").in_group("Ship"),
        ];
        let measures = vec![RowMeasure { label: dvec2(60.0, 16.0), hint: dvec2(100.0, 14.0) }; links.len()];
        let layout = panel_layout(&links, &measures, &[40.0, 40.0], &metrics());
        (links, layout)
    }

    fn sample_items() -> Vec<PillNavItem> {
        vec![
            PillNavItem::place(id("overview"), "Overview"),
            PillNavItem::group(
                id("build"),
                "Build",
                vec![PillNavLink::new(id("editor"), "Editor"), PillNavLink::new(id("hosting"), "Hosting")],
            ),
            PillNavItem::group(id("learn"), "Learn", vec![PillNavLink::new(id("guides"), "Guides")]),
            PillNavItem::place(id("pricing"), "Pricing"),
        ]
    }

    /// The DSL only fails at eval time, so the gate is a real registration:
    /// build the bar from `mod.widgets.PillNav` with the items a host writes
    /// and read back what the apply parsed.
    #[test]
    fn the_dsl_registers_and_the_items_parse() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let nav = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                PillNav{
                    current: @overview
                    items: [
                        {id: @overview label: "Overview"}
                        {id: @build label: "Build" links: [
                            {id: @editor label: "Editor" hint: "Write and run code in one place" group: "Make"}
                            {id: @hosting label: "Hosting" group: "Ship" enabled: false}
                            {label: "A link with no id"}
                        ]}
                        {label: "An item with no id"}
                        {id: @pricing label: "Pricing" enabled: false}
                    ]
                }
            });
            PillNav::script_from_value(vm, value)
        });
        assert_eq!(nav.defs.len(), 3, "the item without an id is dropped");
        assert_eq!(nav.defs[0].id, id("overview"));
        assert_eq!(nav.defs[0].label, "Overview");
        assert!(nav.defs[0].links.is_empty());
        assert_eq!(nav.defs[1].links.len(), 2, "the link without an id is dropped");
        assert_eq!(nav.defs[1].links[0].hint, "Write and run code in one place");
        assert_eq!(nav.defs[1].links[0].group, "Make");
        assert!(nav.defs[1].links[0].enabled);
        assert!(!nav.defs[1].links[1].enabled);
        assert!(!nav.defs[2].enabled);
        assert_eq!(nav.current(), Some(id("overview")));
        assert_eq!(nav.open_on, PopoverTrigger::Hover);
        assert_eq!(nav.panel_placement, PopoverPlacement::BottomCenter);
        assert_eq!(nav.compact_label, "Menu");

        let (click, folded) = cx.with_vm(|vm| {
            let click = vm.eval(script! {
                use mod.prelude.widgets.*
                PillNavClick{}
            });
            let folded = vm.eval(script! {
                use mod.prelude.widgets.*
                PillNavCompact{}
            });
            (PillNav::script_from_value(vm, click), PillNav::script_from_value(vm, folded))
        });
        assert_eq!(click.open_on, PopoverTrigger::Click);
        assert!(folded.compact);
        });
    }

    /// Every layer is compiled here, because a shader error would otherwise
    /// only show as a bar with nothing in it.
    #[test]
    fn the_bar_and_its_panel_compile() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for (name, value) in [
                ("surface", crate::script_eval!(vm, {
                    mod.shader.test_compile_draw_source(mod.widgets.PillNav.draw_panel, "glsl", false)
                })),
                ("text", crate::script_eval!(vm, {
                    mod.shader.test_compile_draw_source(mod.widgets.PillNav.draw_link_text, "glsl", false)
                })),
                ("mark", crate::script_eval!(vm, {
                    mod.shader.test_compile_draw_source(mod.widgets.PillNav.draw_mark, "glsl", false)
                })),
            ] {
                let text = vm
                    .bx
                    .heap
                    .string_with(value, |_heap, text| text.to_string())
                    .expect("the compiler answers with source");
                assert!(!text.starts_with("ERRORS:"), "{name} did not compile: {text}");
                assert!(!text.is_empty(), "{name} compiled to nothing");
            }
        });
        });
    }

    #[test]
    fn both_presets_are_declared_once() {
        crate::on_test_cx(|| {
        let source = include_str!("pill_nav.rs");
        // Spelled in pieces, so this test's own text is not what it finds.
        let click = ["mod.widgets.PillNavClick = ", "mod.widgets.PillNav{"].concat();
        let compact = ["mod.widgets.PillNavCompact = ", "mod.widgets.PillNav{"].concat();
        assert_eq!(source.matches(&click).count(), 1);
        assert_eq!(source.matches(&compact).count(), 1);
        // The trigger and placement words are the popover's; an enum of
        // look-alikes splatted here would overwrite them in the module.
        assert!(!source.contains(&["mod.widgets.", "splat("].concat()));
        });
    }

    #[test]
    fn stepping_items_skips_disabled_and_wraps() {
        crate::on_test_cx(|| {
        let enabled = [true, false, true, true];
        assert_eq!(step_item(&enabled, Some(0), 1), Some(2));
        assert_eq!(step_item(&enabled, Some(3), 1), Some(0));
        assert_eq!(step_item(&enabled, Some(0), -1), Some(3));
        assert_eq!(step_item(&enabled, Some(2), -1), Some(0));
        assert_eq!(step_item(&[false, true], Some(1), 1), Some(1), "a lone stop stays put");
        });
    }

    #[test]
    fn stepping_from_nothing_starts_at_the_first_enabled_item() {
        crate::on_test_cx(|| {
        assert_eq!(step_item(&[false, true, true], None, 1), Some(1));
        assert_eq!(step_item(&[false, true, true], None, -1), Some(2));
        assert_eq!(step_item(&[false, false], None, 1), None);
        assert_eq!(step_item(&[], None, 1), None);
        });
    }

    fn bar_metrics(items_align: f64) -> BarMetrics {
        BarMetrics { bar_pad: 4.0, item_height: 32.0, item_gap: 2.0, items_align }
    }

    #[test]
    fn a_bar_is_as_wide_as_its_items_and_their_gaps() {
        crate::on_test_cx(|| {
        let layout = bar_layout(&[60.0, 80.0, 50.0], &bar_metrics(0.0), dvec2(0.0, 0.0));
        assert_eq!(layout.size, dvec2(4.0 * 2.0 + 190.0 + 2.0 * 2.0, 40.0));
        let xs: Vec<f64> = layout.items.iter().map(|r| r.pos.x).collect();
        assert_eq!(xs, vec![4.0, 66.0, 148.0]);
        assert!(layout.items.iter().all(|r| r.pos.y == 4.0 && r.size.y == 32.0));
        assert_eq!(layout.items[1].size.x, 80.0);
        });
    }

    #[test]
    fn extra_width_places_items_by_items_align() {
        crate::on_test_cx(|| {
        let widths = [60.0, 80.0, 50.0];
        let given = dvec2(302.0, 60.0);
        assert_eq!(bar_layout(&widths, &bar_metrics(0.0), given).items[0].pos, dvec2(4.0, 14.0));
        assert_eq!(bar_layout(&widths, &bar_metrics(0.5), given).items[0].pos.x, 54.0);
        assert_eq!(bar_layout(&widths, &bar_metrics(1.0), given).items[0].pos.x, 104.0);
        // A bar narrower than its items runs over rather than squeezing.
        assert_eq!(bar_layout(&widths, &bar_metrics(1.0), dvec2(100.0, 0.0)).items[0].pos.x, 4.0);
        });
    }

    #[test]
    fn the_bar_is_crowded_only_when_its_parent_is_narrower() {
        crate::on_test_cx(|| {
        assert!(!is_crowded(200.0, None), "a parent that fits gives no room figure");
        assert!(is_crowded(200.0, Some(199.0)));
        assert!(!is_crowded(200.0, Some(200.0)));
        assert!(!is_crowded(200.0, Some(199.6)), "half a point is rounding, not a crowd");
        });
    }

    #[test]
    fn columns_follow_the_first_appearance_of_each_group() {
        crate::on_test_cx(|| {
        let links = vec![
            PillNavLink::new(id("a"), "A").in_group("Ship"),
            PillNavLink::new(id("b"), "B").in_group("Make"),
            PillNavLink::new(id("c"), "C").in_group("Ship"),
            PillNavLink::new(id("d"), "D"),
        ];
        let columns = panel_columns(&links);
        assert_eq!(
            columns,
            vec![("Ship".to_string(), vec![0, 2]), ("Make".to_string(), vec![1]), (String::new(), vec![3])]
        );
        let layout = panel_layout(&links, &vec![RowMeasure::default(); 4], &[0.0; 3], &metrics());
        assert_eq!(layout.columns.len(), 3);
        assert_eq!(layout.columns[0].rows, vec![0, 2]);
        assert!(layout.columns[0].x < layout.columns[1].x && layout.columns[1].x < layout.columns[2].x);
        assert_eq!(layout.rows[2].pos.x, layout.rows[0].pos.x, "a group's links share a column");
        assert!(layout.columns[2].heading.is_none());
        });
    }

    #[test]
    fn a_column_is_clamped_between_its_min_and_max_width() {
        crate::on_test_cx(|| {
        let m = metrics();
        let one = |label_w: f64, heading_w: f64| {
            let links = vec![PillNavLink::new(id("a"), "A").in_group("G")];
            let measures = vec![RowMeasure { label: dvec2(label_w, 16.0), hint: dvec2(0.0, 0.0) }];
            panel_layout(&links, &measures, &[heading_w], &m).columns[0].width
        };
        assert_eq!(one(40.0, 0.0), 180.0);
        assert_eq!(one(500.0, 0.0), 320.0);
        assert_eq!(one(200.0, 0.0), 220.0);
        assert_eq!(one(40.0, 250.0), 270.0, "a heading wider than its rows widens the column");
        });
    }

    #[test]
    fn a_row_with_a_hint_is_taller_by_the_hint_and_its_gap() {
        crate::on_test_cx(|| {
        let m = metrics();
        let plain = row_height(RowMeasure { label: dvec2(60.0, 16.0), hint: dvec2(0.0, 0.0) }, &m);
        let hinted = row_height(RowMeasure { label: dvec2(60.0, 16.0), hint: dvec2(90.0, 14.0) }, &m);
        assert_eq!(plain, 28.0);
        assert_eq!(hinted, plain + 2.0 + 14.0);
        });
    }

    #[test]
    fn a_panel_without_groups_is_one_column_without_a_heading() {
        crate::on_test_cx(|| {
        let links = vec![PillNavLink::new(id("a"), "A"), PillNavLink::new(id("b"), "B")];
        let measures = vec![RowMeasure { label: dvec2(60.0, 16.0), hint: dvec2(0.0, 0.0) }; 2];
        let layout = panel_layout(&links, &measures, &[], &metrics());
        assert_eq!(layout.columns.len(), 1);
        assert!(layout.columns[0].heading.is_none());
        assert_eq!(layout.rows[0].pos, dvec2(8.0, 8.0), "no heading row above the first link");
        assert_eq!(layout.rows[1].pos.y, 8.0 + 28.0 + 2.0);
        assert_eq!(layout.size, dvec2(8.0 * 2.0 + 180.0, 8.0 * 2.0 + 28.0 * 2.0 + 2.0));
        });
    }

    #[test]
    fn down_walks_a_column_and_up_from_its_top_returns_to_the_bar() {
        crate::on_test_cx(|| {
        let (links, layout) = build_panel();
        let on = vec![true; links.len()];
        assert_eq!(step_link(&layout, &on, None, LinkKey::Down), LinkStep::To(0));
        assert_eq!(step_link(&layout, &on, Some(0), LinkKey::Down), LinkStep::To(1));
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::Down), LinkStep::To(2));
        assert_eq!(step_link(&layout, &on, Some(2), LinkKey::Down), LinkStep::Stay, "a column has a bottom");
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::Up), LinkStep::To(0));
        assert_eq!(step_link(&layout, &on, Some(0), LinkKey::Up), LinkStep::Bar);
        assert_eq!(step_link(&layout, &on, Some(3), LinkKey::Up), LinkStep::Bar, "every column's top leads back");
        });
    }

    #[test]
    fn right_lands_on_the_nearest_row_of_the_next_column() {
        crate::on_test_cx(|| {
        let (links, layout) = build_panel();
        let on = vec![true; links.len()];
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::Right), LinkStep::To(4));
        assert_eq!(step_link(&layout, &on, Some(2), LinkKey::Right), LinkStep::To(4));
        assert_eq!(step_link(&layout, &on, Some(0), LinkKey::Right), LinkStep::To(3));
        assert_eq!(step_link(&layout, &on, Some(4), LinkKey::Left), LinkStep::To(1));
        });
    }

    #[test]
    fn right_past_the_last_column_moves_to_the_next_item() {
        crate::on_test_cx(|| {
        let (links, layout) = build_panel();
        let on = vec![true; links.len()];
        assert_eq!(step_link(&layout, &on, Some(3), LinkKey::Right), LinkStep::NextItem);
        assert_eq!(step_link(&layout, &on, Some(0), LinkKey::Left), LinkStep::PrevItem);
        });
    }

    #[test]
    fn disabled_links_are_never_a_keyboard_stop() {
        crate::on_test_cx(|| {
        let (_, layout) = build_panel();
        let on = [false, true, false, false, true];
        assert_eq!(step_link(&layout, &on, None, LinkKey::Down), LinkStep::To(1));
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::Down), LinkStep::Stay);
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::Up), LinkStep::Bar);
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::Home), LinkStep::To(1));
        assert_eq!(step_link(&layout, &on, Some(1), LinkKey::End), LinkStep::To(4));
        // A column with nothing enabled in it is crossed, not landed in.
        let only_make = [true, true, true, false, false];
        assert_eq!(step_link(&layout, &only_make, Some(0), LinkKey::Right), LinkStep::NextItem);
        });
    }

    #[test]
    fn hovering_an_item_opens_its_panel_only_after_the_open_delay() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent::default();
        assert_eq!(intent.pointer(Over::Item(1), 0.0, &t, &has_panel), None);
        assert_eq!(intent.deadline(), Some(0.1));
        assert_eq!(intent.tick(0.05), None);
        assert_eq!(intent.pointer(Over::Item(1), 0.08, &t, &has_panel), None);
        assert_eq!(intent.deadline(), Some(0.1), "moving on the item does not push the deadline back");
        assert_eq!(intent.tick(0.1), Some(IntentChange::Opened(1)));
        assert_eq!(intent.open, Some(1));
        // A place never opens a panel, however long the pointer rests.
        let mut idle = NavIntent::default();
        assert_eq!(idle.pointer(Over::Item(0), 0.0, &t, &has_panel), None);
        assert_eq!(idle.deadline(), None);
        });
    }

    #[test]
    fn sweeping_across_the_bar_opens_nothing() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent::default();
        intent.pointer(Over::Item(1), 0.0, &t, &has_panel);
        intent.pointer(Over::Item(2), 0.05, &t, &has_panel);
        intent.pointer(Over::Bar, 0.08, &t, &has_panel);
        assert_eq!(intent.tick(0.2), None);
        assert_eq!(intent.open, None);
        });
    }

    #[test]
    fn an_open_panel_moves_to_another_item_after_the_switch_delay() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent { open: Some(1), pending: None };
        assert_eq!(intent.pointer(Over::Item(2), 1.0, &t, &has_panel), None);
        assert_eq!(intent.pointer(Over::Item(2), 1.03, &t, &has_panel), None);
        assert_eq!(intent.tick(1.05), None);
        assert_eq!(intent.tick(1.06), Some(IntentChange::Switched(2)));
        assert_eq!(intent.open, Some(2));
        });
    }

    #[test]
    fn reaching_the_panel_before_the_switch_fires_cancels_it() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent { open: Some(1), pending: None };
        intent.pointer(Over::Item(2), 0.0, &t, &has_panel);
        intent.pointer(Over::Panel, 0.03, &t, &has_panel);
        assert_eq!(intent.pending, None);
        assert_eq!(intent.tick(0.2), None);
        assert_eq!(intent.open, Some(1));
        });
    }

    #[test]
    fn leaving_everything_closes_after_the_grace_and_returning_cancels_it() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent { open: Some(1), pending: None };
        intent.pointer(Over::Nothing, 0.0, &t, &has_panel);
        assert_eq!(intent.deadline(), Some(0.2));
        intent.pointer(Over::Bar, 0.1, &t, &has_panel);
        assert_eq!(intent.tick(0.3), None, "coming back inside the grace cancels the close");
        intent.pointer(Over::Nothing, 0.3, &t, &has_panel);
        assert_eq!(intent.tick(0.49), None);
        assert_eq!(intent.tick(0.5), Some(IntentChange::Closed));
        assert_eq!(intent.open, None);
        });
    }

    #[test]
    fn the_gap_between_item_and_panel_counts_as_inside() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent { open: Some(1), pending: None };
        intent.pointer(Over::Nothing, 0.0, &t, &has_panel);
        intent.pointer(Over::Bridge, 0.05, &t, &has_panel);
        assert_eq!(intent.pending, None);
        let anchor = Rect { pos: dvec2(100.0, 0.0), size: dvec2(80.0, 40.0) };
        let panel = Rect { pos: dvec2(60.0, 48.0), size: dvec2(200.0, 150.0) };
        let bridge = bridge_rect(anchor, panel, Side::Bottom);
        assert!(bridge.contains(dvec2(150.0, 44.0)));
        assert!(bridge.contains(dvec2(62.0, 44.0)), "as wide as the panel where the panel is wider");
        });
    }

    #[test]
    fn resting_on_a_plain_item_closes_the_open_panel() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent { open: Some(1), pending: None };
        assert_eq!(intent.pointer(Over::Item(3), 0.0, &t, &has_panel), None);
        assert_eq!(intent.deadline(), Some(0.06));
        assert_eq!(intent.tick(0.06), Some(IntentChange::Closed));
        });
    }

    #[test]
    fn a_press_toggles_and_clears_anything_pending() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Hover);
        let mut intent = NavIntent::default();
        intent.pointer(Over::Item(2), 0.0, &t, &has_panel);
        assert_eq!(intent.press(1, &t, &has_panel), Some(IntentChange::Opened(1)));
        assert_eq!(intent.pending, None);
        assert_eq!(intent.press(2, &t, &has_panel), Some(IntentChange::Switched(2)));
        assert_eq!(intent.press(2, &t, &has_panel), Some(IntentChange::Closed));
        assert_eq!(intent.press(0, &t, &has_panel), None, "a place opens nothing; the widget reports it");
        });
    }

    #[test]
    fn click_mode_never_opens_on_hover_but_slides_once_open() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Click);
        let mut intent = NavIntent::default();
        assert_eq!(intent.pointer(Over::Item(1), 0.0, &t, &has_panel), None);
        assert_eq!(intent.tick(1.0), None);
        assert_eq!(intent.open, None);
        assert_eq!(intent.press(1, &t, &has_panel), Some(IntentChange::Opened(1)));
        assert_eq!(intent.pointer(Over::Item(2), 1.0, &t, &has_panel), Some(IntentChange::Switched(2)));
        assert_eq!(intent.pointer(Over::Nothing, 2.0, &t, &has_panel), None);
        assert_eq!(intent.tick(9.0), None, "a pressed-open panel does not close on leave");
        assert_eq!(intent.open, Some(2));
        });
    }

    #[test]
    fn manual_mode_opens_nothing_by_itself() {
        crate::on_test_cx(|| {
        let t = times(PopoverTrigger::Manual);
        let mut intent = NavIntent::default();
        assert_eq!(intent.pointer(Over::Item(1), 0.0, &t, &has_panel), None);
        assert_eq!(intent.tick(1.0), None);
        assert_eq!(intent.press(1, &t, &has_panel), None);
        assert_eq!(intent.force_open(1), Some(IntentChange::Opened(1)));
        assert_eq!(intent.force_open(1), None);
        assert_eq!(intent.force_open(2), Some(IntentChange::Switched(2)));
        assert_eq!(intent.force_close(), Some(IntentChange::Closed));
        assert_eq!(intent.force_close(), None);
        });
    }

    #[test]
    fn a_zero_delay_fires_inside_the_pointer_call() {
        crate::on_test_cx(|| {
        let t = IntentTimes { open_on: PopoverTrigger::Hover, open_delay: 0.0, switch_delay: 0.0, close_delay: 0.0 };
        let mut intent = NavIntent::default();
        assert_eq!(intent.pointer(Over::Item(1), 0.0, &t, &has_panel), Some(IntentChange::Opened(1)));
        assert_eq!(intent.pointer(Over::Item(2), 0.0, &t, &has_panel), Some(IntentChange::Switched(2)));
        assert_eq!(intent.pointer(Over::Nothing, 0.0, &t, &has_panel), Some(IntentChange::Closed));
        assert_eq!(intent.deadline(), None);
        });
    }

    #[test]
    fn the_bridge_spans_from_item_to_panel_on_each_side() {
        crate::on_test_cx(|| {
        let anchor = Rect { pos: dvec2(100.0, 100.0), size: dvec2(80.0, 40.0) };
        let below = Rect { pos: dvec2(90.0, 148.0), size: dvec2(200.0, 100.0) };
        assert_eq!(bridge_rect(anchor, below, Side::Bottom), Rect { pos: dvec2(90.0, 140.0), size: dvec2(200.0, 8.0) });
        let above = Rect { pos: dvec2(120.0, -8.0), size: dvec2(40.0, 100.0) };
        assert_eq!(bridge_rect(anchor, above, Side::Top), Rect { pos: dvec2(100.0, 92.0), size: dvec2(80.0, 8.0) });
        let right = Rect { pos: dvec2(188.0, 80.0), size: dvec2(120.0, 60.0) };
        assert_eq!(bridge_rect(anchor, right, Side::Right), Rect { pos: dvec2(180.0, 80.0), size: dvec2(8.0, 60.0) });
        let left = Rect { pos: dvec2(0.0, 110.0), size: dvec2(92.0, 50.0) };
        assert_eq!(bridge_rect(anchor, left, Side::Left), Rect { pos: dvec2(92.0, 100.0), size: dvec2(8.0, 60.0) });
        });
    }

    #[test]
    fn smootherstep_is_flat_at_both_ends() {
        crate::on_test_cx(|| {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert!((smootherstep(0.5) - 0.5).abs() < 1e-12);
        assert!(smootherstep(0.01) < 1e-4, "no speed at the start");
        assert!(1.0 - smootherstep(0.99) < 1e-4, "no speed at the end");
        assert_eq!(smootherstep(-1.0), 0.0);
        assert_eq!(smootherstep(2.0), 1.0);
        });
    }

    #[test]
    fn the_morph_starts_on_the_pill_and_ends_on_the_panel() {
        crate::on_test_cx(|| {
        let pill = Rect { pos: dvec2(100.0, 4.0), size: dvec2(80.0, 32.0) };
        let panel = Rect { pos: dvec2(40.0, 48.0), size: dvec2(400.0, 200.0) };
        let start = morph_at(0.0, 0.0, pill, panel, 16.0, 14.0);
        assert_eq!(start.rect, pill);
        assert_eq!(start.radius, 16.0);
        assert_eq!(start.content_alpha, 0.0);
        assert_eq!(start.morph, 0.0);
        let middle = morph_at(0.55, 0.55, pill, panel, 16.0, 14.0);
        assert!(middle.rect.size.x > pill.size.x && middle.rect.size.x < panel.size.x);
        assert!(middle.rect.pos.y > pill.pos.y && middle.rect.pos.y < panel.pos.y);
        assert_eq!(middle.content_alpha, 0.0);
        let end = morph_at(1.0, 1.0, pill, panel, 16.0, 14.0);
        assert_eq!(end.rect, panel);
        assert_eq!(end.radius, 14.0);
        assert_eq!(end.content_alpha, 1.0);
        assert_eq!(end.morph, 1.0);
        });
    }

    /// A spring grows the surface past the panel, but its fill, its corner
    /// and its words stop where the panel's do; a shrink that swings below
    /// the pill never turns the surface inside out.
    #[test]
    fn a_swing_past_the_panel_moves_only_the_surface() {
        crate::on_test_cx(|| {
        let pill = Rect { pos: dvec2(100.0, 4.0), size: dvec2(80.0, 32.0) };
        let panel = Rect { pos: dvec2(40.0, 48.0), size: dvec2(400.0, 200.0) };
        let past = morph_at(1.3, 1.0, pill, panel, 16.0, 14.0);
        assert!(past.rect.size.x > panel.size.x && past.rect.size.y > panel.size.y, "the surface swings past the panel");
        assert_eq!((past.morph, past.radius, past.content_alpha), (1.0, 14.0, 1.0));
        let under = morph_at(-0.5, 0.0, pill, panel, 16.0, 14.0);
        assert_eq!(under.rect.size, dvec2(0.0, 0.0));
        assert_eq!((under.morph, under.radius, under.content_alpha), (0.0, 16.0, 0.0));
        // The words fade by how far the grow has come, not where it is now.
        assert_eq!(morph_at(0.8, 1.0, pill, panel, 16.0, 14.0).content_alpha, 1.0);
        });
    }

    /// The curves and the times are the theme's tokens, and a host can name
    /// another token for any of them.
    #[test]
    fn the_motion_takes_its_curves_and_times_from_the_theme() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                PillNav{}
            });
            let nav = PillNav::script_from_value(vm, value);
            for (ease, token) in [
                (nav.glide_ease, "motion_ease_standard"),
                (nav.open_ease, "motion_ease_emphasized_decelerate"),
                (nav.close_ease, "motion_ease_standard_accelerate"),
                (nav.switch_ease, "motion_ease_standard"),
            ] {
                assert_eq!(ease, theme_ease(vm, token), "{token}");
            }
            for (ease, token) in [
                (nav.highlight_enter_ease, "motion_ease_standard_decelerate"),
                (nav.highlight_exit_ease, "motion_ease_standard_accelerate"),
            ] {
                assert_eq!(ease, theme_ease(vm, token), "the pill's fade: {token}");
            }
            for (secs, token) in [
                (nav.glide_secs, "motion_short_4"),
                (nav.open_secs, "motion_medium_1"),
                (nav.close_secs, "motion_short_3"),
                (nav.switch_secs, "motion_short_4"),
                (nav.highlight_enter_secs, "motion_short_4"),
                (nav.highlight_exit_secs, "motion_short_3"),
            ] {
                assert_eq!(secs, theme_number(vm, token), "{token}");
            }
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                PillNav{open_ease: theme.motion_ease_spring close_ease: theme.motion_ease_bounce glide_ease: theme.motion_ease_linear}
            });
            let sprung = PillNav::script_from_value(vm, value);
            assert_eq!(sprung.open_ease, theme_ease(vm, "motion_ease_spring"));
            assert_eq!(sprung.close_ease, theme_ease(vm, "motion_ease_bounce"));
            assert_eq!(sprung.glide_ease, theme_ease(vm, "motion_ease_linear"));
            assert_eq!(sprung.switch_ease, nav.switch_ease, "a curve not named keeps its default");
        });
        });
    }

    /// The pill comes in where nothing had it along its enter curve and
    /// time, and goes once nothing has it along its exit ones, neither of
    /// them the glide's; reduced motion lands it at once.
    #[test]
    fn the_pill_comes_in_and_goes_on_its_own_curves_and_times() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let (mut nav, spring, accelerate) = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                PillNav{
                    highlight_current: false
                    glide_ease: theme.motion_ease_bounce
                    glide_secs: 5.0
                    highlight_enter_ease: theme.motion_ease_spring
                    highlight_enter_secs: 0.4
                    highlight_exit_ease: theme.motion_ease_emphasized_accelerate
                    highlight_exit_secs: 0.1
                    items: [
                        {id: @overview label: "Overview"}
                        {id: @pricing label: "Pricing"}
                    ]
                }
            });
            (
                PillNav::script_from_value(vm, value),
                theme_ease(vm, "motion_ease_spring"),
                theme_ease(vm, "motion_ease_emphasized_accelerate"),
            )
        });
        nav.item_rects = vec![
            Rect { pos: dvec2(4.0, 4.0), size: dvec2(80.0, 32.0) },
            Rect { pos: dvec2(90.0, 4.0), size: dvec2(70.0, 32.0) },
        ];
        nav.hover = Some(0);
        assert!(nav.advance(0.0), "the pointer claims the pill");
        assert!(nav.advance(0.1));
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(near(nav.highlight_fade.value(), spring.map(0.25)), "in along the enter curve, a quarter of its time in");
        while nav.advance(0.05) {}
        assert!(nav.highlight_fade.is_at(1.0));
        nav.hover = None;
        assert!(nav.advance(0.0), "nothing claims it");
        assert!(nav.advance(0.05));
        assert!(near(nav.highlight_fade.value(), 1.0 - accelerate.map(0.5)), "out along the exit curve, half its time in");
        while nav.advance(0.05) {}
        assert!(nav.highlight_fade.is_at(0.0));
        nav.reduced_motion = true;
        nav.hover = Some(1);
        assert!(!nav.advance(0.0));
        assert!(nav.highlight_fade.is_at(1.0), "reduced motion brings it in at once");
        });
    }

    /// Each theme easing is read straight off the tween's clock, both ways:
    /// toward 1 the value is the curve, toward 0 it is the curve turned over.
    #[test]
    fn a_tween_reads_every_theme_easing_off_its_clock() {
        crate::on_test_cx(|| {
        for (token, ease) in theme_eases() {
            let mut enter = UnitTween::at(0.0);
            enter.aim(1.0, 0.4, ease);
            let mut exit = UnitTween::at(1.0);
            exit.aim(0.0, 0.4, ease);
            assert_eq!((enter.value(), exit.value()), (ease.map(0.0), 1.0 - ease.map(0.0)), "{token} at the start");
            let mut clock = 0.0;
            for share in [0.25, 0.5, 0.75] {
                let dt = share * 0.4 - clock;
                clock += dt;
                assert!(enter.step(dt) && exit.step(dt), "{token} is still running at {share}");
                let at = clock / 0.4;
                assert!((enter.value() - ease.map(at)).abs() < 1e-9, "{token} in at {share}");
                assert!((exit.value() - (1.0 - ease.map(at))).abs() < 1e-9, "{token} out at {share}");
            }
            assert!(!enter.step(0.2) && !exit.step(0.2), "{token} ends on time");
            assert!(enter.is_at(1.0) && exit.is_at(0.0));
            assert_eq!((enter.value(), exit.value()), (1.0, 0.0), "{token} lands exactly on its end");
        }
        });
    }

    #[test]
    fn a_spring_carries_the_value_past_its_end_and_lands_on_it() {
        crate::on_test_cx(|| {
        let spring = named(&theme_eases(), "motion_ease_spring");
        let mut grow = UnitTween::at(0.0);
        grow.aim(1.0, 1.0, spring);
        let mut highest: f64 = 0.0;
        let mut reached = 0.0;
        while grow.step(0.01) {
            highest = highest.max(grow.value());
            assert!(grow.reached() >= reached && grow.reached() <= 1.0, "what a fade reads never goes back or past 1");
            reached = grow.reached();
        }
        assert!(highest > 1.2, "the spring swings past 1: {highest}");
        assert!(grow.is_at(1.0));
        assert_eq!((grow.value(), grow.reached()), (1.0, 1.0));
        });
    }

    #[test]
    fn a_bounce_dips_the_surface_but_never_the_words() {
        crate::on_test_cx(|| {
        let bounce = named(&theme_eases(), "motion_ease_bounce");
        let mut grow = UnitTween::at(0.0);
        grow.aim(1.0, 1.0, bounce);
        let mut deepest_dip: f64 = 0.0;
        let mut alpha = 0.0;
        while grow.step(0.005) {
            assert!(grow.value() <= 1.0 + 1e-9, "a bounce never goes past its end");
            deepest_dip = deepest_dip.max(grow.reached() - grow.value());
            let now = content_alpha(grow.reached());
            assert!(now >= alpha, "the words never fade back while the surface dips");
            alpha = now;
        }
        assert!(deepest_dip > 0.1, "the surface does dip back: {deepest_dip}");
        });
    }

    /// Turning back mid-run starts where the value is, takes the share of
    /// the time the way back needs, and aiming the same way again is free.
    #[test]
    fn a_reversal_turns_back_from_where_the_value_is() {
        crate::on_test_cx(|| {
        let eases = theme_eases();
        let mut grow = UnitTween::at(0.0);
        grow.aim(1.0, 0.4, named(&eases, "motion_ease_emphasized_decelerate"));
        assert!(grow.step(0.05));
        let there = grow.value();
        assert!(there > 0.0 && there < 1.0);
        grow.aim(0.0, 0.2, named(&eases, "motion_ease_standard_accelerate"));
        assert_eq!(grow.value(), there, "no jump at the turn");
        assert_eq!(grow.target(), 0.0);
        let before = grow;
        grow.aim(0.0, 5.0, named(&eases, "motion_ease_linear"));
        assert_eq!(grow, before, "aiming where it already heads changes nothing");
        let mut spent = 0.0;
        while grow.step(0.001) {
            spent += 0.001;
        }
        assert!((spent - there * 0.2).abs() < 0.002, "the way back took {spent}, not {} of a full run", there);
        assert!(grow.is_at(0.0));
        });
    }

    #[test]
    fn a_zero_time_or_a_settle_lands_at_once() {
        crate::on_test_cx(|| {
        let spring = named(&theme_eases(), "motion_ease_spring");
        let mut grow = UnitTween::at(0.0);
        grow.aim(1.0, 0.0, spring);
        assert!(grow.is_at(1.0));
        grow.aim(0.0, 1.0, spring);
        assert!(grow.step(0.1));
        grow.settle();
        assert!(grow.is_at(0.0) && grow.value() == 0.0 && !grow.step(0.1));
        });
    }

    #[test]
    fn a_glide_takes_its_first_place_at_once_and_follows_its_curve_to_the_next() {
        crate::on_test_cx(|| {
        let spring = named(&theme_eases(), "motion_ease_spring");
        let first = Rect { pos: dvec2(4.0, 4.0), size: dvec2(80.0, 32.0) };
        let next = Rect { pos: dvec2(200.0, 4.0), size: dvec2(60.0, 32.0) };
        let mut glide = EasedGlide::default();
        glide.aim(first, 1.0, spring);
        assert_eq!(glide.current(), first, "nowhere to slide from");
        assert!(!glide.step(0.016));
        glide.aim(next, 1.0, spring);
        assert_eq!(glide.current(), first);
        assert!(glide.step(0.15));
        let e = spring.map(0.15);
        assert!(e > 1.0);
        assert!((glide.current().pos.x - (4.0 + 196.0 * e)).abs() < 1e-9, "the curve, read off the clock");
        assert!(glide.current().pos.x > next.pos.x, "a spring runs past the item");
        assert_eq!(glide.resting(), next, "while where it rests stays put");
        while glide.step(0.01) {}
        assert_eq!(glide.current(), next);
        });
    }

    #[test]
    fn a_swing_is_moved_back_inside_its_room_and_cut_only_when_bigger() {
        crate::on_test_cx(|| {
        let room = Rect { pos: dvec2(0.0, 0.0), size: dvec2(400.0, 300.0) };
        let inside = Rect { pos: dvec2(10.0, 10.0), size: dvec2(100.0, 50.0) };
        assert_eq!(hold_inside(inside, room), inside);
        let past = Rect { pos: dvec2(350.0, 280.0), size: dvec2(100.0, 50.0) };
        assert_eq!(hold_inside(past, room), Rect { pos: dvec2(300.0, 250.0), size: dvec2(100.0, 50.0) });
        let wide = Rect { pos: dvec2(-20.0, 10.0), size: dvec2(500.0, 50.0) };
        assert_eq!(hold_inside(wide, room), Rect { pos: dvec2(0.0, 10.0), size: dvec2(400.0, 50.0) });
        let before = Rect { pos: dvec2(-30.0, -5.0), size: dvec2(20.0, 20.0) };
        assert_eq!(hold_inside(before, room), Rect { pos: dvec2(0.0, 0.0), size: dvec2(20.0, 20.0) });
        });
    }

    #[test]
    fn content_stays_hidden_for_the_first_half_of_the_grow() {
        crate::on_test_cx(|| {
        for step in 0..=55 {
            assert_eq!(content_alpha(step as f64 / 100.0), 0.0, "shown at {step}%");
        }
        let mut last = 0.0;
        for step in 56..=100 {
            let alpha = content_alpha(step as f64 / 100.0);
            assert!(alpha > last, "the reveal never goes back");
            last = alpha;
        }
        });
    }

    #[test]
    fn a_switch_lets_the_old_words_go_before_the_new_ones_come() {
        crate::on_test_cx(|| {
        assert_eq!(switch_alphas(0.0), (1.0, 0.0));
        assert_eq!(switch_alphas(0.35), (0.0, 0.0), "a moment with neither, never both");
        assert_eq!(switch_alphas(1.0), (0.0, 1.0));
        });
    }

    #[test]
    fn compact_rows_show_links_only_under_the_expanded_item() {
        crate::on_test_cx(|| {
        let items = sample_items();
        assert_eq!(
            compact_rows(&items, None),
            vec![CompactRow::Item(0), CompactRow::Item(1), CompactRow::Item(2), CompactRow::Item(3)]
        );
        assert_eq!(
            compact_rows(&items, Some(1)),
            vec![
                CompactRow::Item(0),
                CompactRow::Item(1),
                CompactRow::Link(1, 0),
                CompactRow::Link(1, 1),
                CompactRow::Item(2),
                CompactRow::Item(3),
            ]
        );
        });
    }

    #[test]
    fn compact_left_from_a_link_goes_to_its_item_and_again_collapses_it() {
        crate::on_test_cx(|| {
        let items = sample_items();
        let (expanded, at) = step_compact(&items, None, Some(CompactRow::Item(1)), CompactKey::Right);
        assert_eq!((expanded, at), (Some(1), Some(CompactRow::Item(1))));
        let (expanded, at) = step_compact(&items, expanded, at, CompactKey::Down);
        assert_eq!(at, Some(CompactRow::Link(1, 0)));
        let (expanded, at) = step_compact(&items, expanded, at, CompactKey::Left);
        assert_eq!((expanded, at), (Some(1), Some(CompactRow::Item(1))));
        let (expanded, at) = step_compact(&items, expanded, at, CompactKey::Left);
        assert_eq!((expanded, at), (None, Some(CompactRow::Item(1))));
        let (_, at) = step_compact(&items, expanded, at, CompactKey::Down);
        assert_eq!(at, Some(CompactRow::Item(2)), "collapsed links are not walked");
        let (_, at) = step_compact(&items, None, Some(CompactRow::Item(0)), CompactKey::Right);
        assert_eq!(at, Some(CompactRow::Item(0)), "a place has nothing to open");
        });
    }

    #[test]
    fn changing_items_keeps_current_only_if_it_is_still_a_place() {
        crate::on_test_cx(|| {
        let old = sample_items();
        let moved = vec![PillNavItem::place(id("pricing"), "Pricing"), PillNavItem::place(id("overview"), "Overview")];
        assert_eq!(keep_current(&old, Some(0), &moved), Some(1), "kept by id, wherever it went");
        assert_eq!(keep_current(&old, Some(0), &[PillNavItem::place(id("pricing"), "Pricing")]), None);
        let grown = vec![PillNavItem::group(id("overview"), "Overview", vec![PillNavLink::new(id("a"), "A")])];
        assert_eq!(keep_current(&old, Some(0), &grown), None, "a place that became a group is not where you are");
        assert_eq!(keep_current(&old, None, &moved), None);
        });
    }

    /// The bar holds the lock while its list is out, and the lock turns away
    /// every area whose sweep area is not its owner, the owner's own plain
    /// hits included. The pill's press has to get through, or pressing it
    /// could never put the list away. Opened from the keys: a press the
    /// bar had already captured would be handed back past the lock, and a
    /// window-less test has no event loop to release that capture.
    #[test]
    fn a_press_on_the_pill_puts_away_the_list_the_bar_holds_the_lock_for() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    nav := PillNavCompact{
                        reduced_motion: true
                        items: [
                            {id: @overview label: "Overview"}
                            {id: @build label: "Build" links: [
                                {id: @editor label: "Editor"}
                                {id: @hosting label: "Hosting"}
                            ]}
                        ]
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let size = dvec2(800.0, 600.0);
        let pass = DrawPass::new(&mut cx);
        let mut list = DrawList2d::new(&mut cx);
        let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
        let mut draw = |cx: &mut Cx| {
            pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = crate::makepad_draw::cx_draw::CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            list.begin_always(&mut cx2d);
            overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            overlay.end(&mut cx2d);
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        };
        draw(&mut cx);
        let nav = root.widget(&cx, ids!(nav));
        let (pill, bar) = {
            let inner = nav.borrow::<PillNav>().expect("nav is a pill nav");
            let parts = inner.snapshot_parts(&cx);
            let pill = parts.iter().find(|part| part.widget_type == "PillNavMenu").expect("the folded pill").rect.center();
            (pill, inner.draw_bar.area())
        };
        cx.set_key_focus(bar);
        cx.action(());
        cx.handle_actions();
        let enter = Event::KeyDown(KeyEvent { key_code: KeyCode::ReturnKey, ..Default::default() });
        root.handle_event(&mut cx, &enter, &mut Scope::empty());
        draw(&mut cx);
        assert!(nav.as_pill_nav().is_open(), "Return brought the list out");
        assert!(cx.sweep_lock_area().is_some(), "and the bar holds the pointer");
        let press = Event::MouseDown(MouseDownEvent {
            abs: pill,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        });
        root.handle_event(&mut cx, &press, &mut Scope::empty());
        assert!(!nav.as_pill_nav().is_open(), "the press on the pill put it away");
        });
    }

    /// A pass, a list and an overlay for a window-less draw.
    struct Screen {
        pass: DrawPass,
        list: DrawList2d,
        overlay: Overlay,
    }

    impl Screen {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Screen { pass: DrawPass::new(cx), list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            let size = dvec2(800.0, 600.0);
            self.pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = crate::makepad_draw::cx_draw::CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    /// A bar of two items, the second owning a panel, in a tree of its own.
    fn lone_bar(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    nav := PillNav{
                        items: [
                            {id: @overview label: "Overview"}
                            {id: @build label: "Build" links: [
                                {id: @editor label: "Editor"}
                                {id: @hosting label: "Hosting"}
                            ]}
                        ]
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// A bar dropped with its panel out, as when the page holding it is
    /// swapped for another mid-open, cannot let go of the pointer itself: a
    /// drop has no `Cx`. A lock nobody holds turns every press in the window
    /// away, so the bar leaves it for the next event to release, and a bar
    /// taking a lock of its own in a tree with no window above it releases
    /// what was left first.
    #[test]
    fn a_bar_dropped_open_leaves_its_lock_for_the_next_event() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let first = lone_bar(&mut cx);
        let mut first_screen = Screen::new(&mut cx);
        first_screen.draw(&mut cx, &first);
        first.widget(&cx, ids!(nav)).as_pill_nav().open(&mut cx, id("build"));
        first_screen.draw(&mut cx, &first);
        assert!(first.widget(&cx, ids!(nav)).as_pill_nav().is_open());
        assert!(cx.sweep_lock_area().is_some(), "the open bar holds the pointer");
        drop(first);
        assert!(cx.sweep_lock_area().is_some(), "nothing could let go of it on drop");
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the next event does");

        let dropped = lone_bar(&mut cx);
        let survivor = lone_bar(&mut cx);
        let (mut dropped_screen, mut survivor_screen) = (Screen::new(&mut cx), Screen::new(&mut cx));
        dropped_screen.draw(&mut cx, &dropped);
        survivor_screen.draw(&mut cx, &survivor);
        dropped.widget(&cx, ids!(nav)).as_pill_nav().open(&mut cx, id("build"));
        assert!(cx.sweep_lock_area().is_some());
        drop(dropped);
        let nav = survivor.widget(&cx, ids!(nav));
        nav.as_pill_nav().open(&mut cx, id("build"));
        assert!(nav.as_pill_nav().is_open());
        nav.as_pill_nav().close(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the other bar let go of its own lock and of the one left behind");
        });
    }

    /// The folded pill goes into the test tree under the id the spec gives
    /// it, as the word `menu` a test can look it up by, not a hash.
    #[test]
    fn the_folded_pill_is_named_menu_in_the_test_tree() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    nav := PillNavCompact{
                        items: [{id: @overview label: "Overview"}]
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut screen = Screen::new(&mut cx);
        screen.draw(&mut cx, &root);
        let nav = root.widget(&cx, ids!(nav));
        let inner = nav.borrow::<PillNav>().expect("nav is a pill nav");
        let parts = inner.snapshot_parts(&cx);
        let pill = parts.iter().find(|part| part.widget_type == "PillNavMenu").expect("the folded pill");
        assert_eq!(pill.id, live_id!(menu));
        assert_eq!(crate::widget_tree::live_id_token(pill.id), "menu");
        });
    }

    /// While a spring swings the growing panel past where it settles, the
    /// pointer and a press read the placed panel and the rows where they
    /// rest: a point the swing covers below the panel is outside it, the
    /// rows are hit where they will be, and a test is not told of rows that
    /// are still arriving. Reduced motion lands the same spring at once.
    #[test]
    fn the_pointer_reads_the_placed_panel_while_a_spring_swings_the_surface() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    padding: Inset{left: 200. top: 40.}
                    nav := PillNav{
                        open_ease: theme.motion_ease_spring
                        open_secs: 1.0
                        items: [
                            {id: @overview label: "Overview"}
                            {id: @build label: "Build" links: [
                                {id: @editor label: "Editor" hint: "Write"}
                                {id: @hosting label: "Hosting" hint: "Serve"}
                            ]}
                        ]
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut screen = Screen::new(&mut cx);
        screen.draw(&mut cx, &root);
        let nav = root.widget(&cx, ids!(nav));
        {
            let mut bar = nav.borrow_mut::<PillNav>().expect("nav is a pill nav");
            bar.bar_rect = bar.draw_bar.area().rect(&cx);
            assert!(bar.bar_rect.size.x > 0.0, "the bar was drawn");
            bar.open(&mut cx, id("build"));
            assert!(bar.advance(0.0), "the grow has started");
        }
        screen.draw(&mut cx, &root);
        let (placed, rows) = {
            let bar = nav.borrow::<PillNav>().unwrap();
            assert!(bar.panel_rect.size.y > 0.0, "the panel was placed");
            (bar.panel_rect, bar.panel_hits.iter().map(|hit| hit.rect).collect::<Vec<_>>())
        };
        assert_eq!(rows.len(), 2);

        nav.borrow_mut::<PillNav>().unwrap().advance(0.15);
        screen.draw(&mut cx, &root);
        {
            let bar = nav.borrow::<PillNav>().unwrap();
            let panel = bar.panel.as_ref().expect("the panel is growing");
            let swing = panel.grow.value();
            assert!(swing > 1.2, "the spring carries the surface past the panel: {swing}");
            let pill = bar.item_abs(1).expect("the item");
            let surface = morph_at(swing, panel.grow.reached(), pill, placed, bar.item_height * 0.5, bar.panel_radius).rect;
            let below = dvec2(placed.center().x, placed.pos.y + placed.size.y + 4.0);
            assert!(surface.contains(below), "the swing reaches below the panel");
            assert_eq!(bar.classify(below), Over::Nothing, "but the pointer there is not in the panel");
            assert!(!bar.inside_union(below), "and a press there is outside it");
            assert_eq!(bar.panel_rect, placed, "the placed rect does not swing");
            assert_eq!(bar.panel_hits.iter().map(|hit| hit.rect).collect::<Vec<_>>(), rows, "the rows are hit where they rest");
            assert!(
                bar.snapshot_parts(&cx).iter().all(|part| part.widget_type != "PillNavLink"),
                "rows still arriving are not reported"
            );
        }

        nav.borrow_mut::<PillNav>().unwrap().advance(1.0);
        screen.draw(&mut cx, &root);
        {
            let bar = nav.borrow::<PillNav>().unwrap();
            assert!(bar.panel.as_ref().unwrap().grow.is_at(1.0));
            let links: Vec<Rect> = bar.snapshot_parts(&cx).iter().filter(|part| part.widget_type == "PillNavLink").map(|part| part.rect).collect();
            assert_eq!(links, rows, "settled, the rows are reported where they were hit all along");
        }

        let mut bar = nav.borrow_mut::<PillNav>().unwrap();
        bar.reduced_motion = true;
        bar.close(&mut cx);
        assert!(bar.panel.is_none(), "reduced motion takes the panel away at once");
        bar.open(&mut cx, id("build"));
        assert!(bar.panel.as_ref().unwrap().grow.is_at(1.0), "and brings it back whole, spring or not");
        assert!(!bar.advance(0.0), "with nothing left to move");
        });
    }
}
