//! Chip and Tag — the small pill that stands for ONE thing the user chose,
//! typed or was given: a filter that is on, a recipient in a field, a label
//! on a record.
//!
//! A chip is not a button. A button is a verb the user presses to make
//! something happen; a chip is a NOUN that is either there or not, and the
//! press toggles its being there. That difference decides everything below:
//! a chip carries a check when it is chosen, a cross when it can be taken
//! away, and it says its own name rather than an instruction.
//!
//! Three props cover what would otherwise be five widgets. `selectable`
//! makes the press a toggle and gives the chosen chip a tick, which is what
//! a filter is. `removable` puts a cross at the trailing edge with its own
//! hit area, which is what a recipient in a field is. Neither makes a chip
//! that only reads out a value, which is what a tag is, and that is the
//! `Tag` preset: `interactive: false`, so it takes no hover, no press and no
//! focus, because a pill that lights up under the pointer and then does
//! nothing is a promise the widget cannot keep.
//!
//! The colours come from the same role vocabulary the badge uses
//! ([`BadgeIntent`] and [`BadgePalette`]): a chip and a badge that both say
//! "error" must be the same red, and one palette is how that stays true
//! through a theme change. The APPEARANCE decides how loudly the role is
//! spoken — a solid fill, its quiet container tint, a bare outline, or ink
//! alone. There is deliberately no "elevated" appearance: a shadow says the
//! surface is lifted, which is a fact about the container a chip sits in,
//! not about the chip, so an elevated row of chips is a chip row inside an
//! `ElevatedView1`.
//!
//! Hover and press are drawn as STATE LAYERS: the ink colour laid over the
//! fill at `theme.state_hover_opacity` and `theme.state_press_opacity`,
//! mixed in the shader from two animated instances. That is the same
//! arithmetic every other control in the library uses for the same two
//! states, so a chip beside a button reacts with the same weight, and a
//! theme that wants a firmer press changes one token rather than every
//! widget.
//!
//! The cross of a removable chip is hit-tested BEFORE the body, and its
//! action is [`ChipAction::Removed`] rather than a click, because "take this
//! away" and "choose this" landing on the same pill a few points apart must
//! never be confused. Removing is the host's job: the chip reports, the host
//! decides, since only the host knows whether the thing behind the chip can
//! actually go.

use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    link_label::LinkLabelWidgetRefExt,
    view::View,
    badge::{measure, sized, BadgeIntent, BadgePalette, IntentColors},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

use crate::makepad_draw::DrawSvg;

/// How loudly a chip speaks its role: a solid fill, the quiet container
/// tint, a bare outline, or ink with nothing behind it.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ChipAppearance {
    /// The role's own colour, with its reading ink on top.
    #[pick]
    Filled = 0,
    /// The role's container colour: present, but quiet enough for a row.
    Tonal = 1,
    /// Nothing behind it, the role's colour as a stroke and as ink.
    Outline = 2,
    /// Ink alone: a chip that only becomes a shape when it is hovered.
    Ghost = 3,
}

/// The size ladder. A chip is a touch target as well as a label, so the
/// rungs are heights the finger can find, not font sizes.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ChipSize {
    Small = 0,
    #[pick]
    Medium = 1,
    Large = 2,
}

/// One rung, in layout points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipMetrics {
    /// Pill height.
    pub height: f64,
    /// Space between the pill's edge and its content.
    pub pad_x: f64,
    /// Space between the mark, the label and the cross.
    pub gap: f64,
    /// Side of the tick and the cross.
    pub mark: f64,
    /// Multiplier on the call site's font size.
    pub font_scale: f64,
}

impl ChipSize {
    pub fn metrics(self) -> ChipMetrics {
        match self {
            ChipSize::Small => ChipMetrics { height: 22.0, pad_x: 8.0, gap: 4.0, mark: 10.0, font_scale: 0.9 },
            ChipSize::Medium => ChipMetrics { height: 28.0, pad_x: 11.0, gap: 5.0, mark: 12.0, font_scale: 1.0 },
            ChipSize::Large => ChipMetrics { height: 34.0, pad_x: 14.0, gap: 6.0, mark: 14.0, font_scale: 1.1 },
        }
    }
}

/// How many chips in a group may be chosen at once.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ChipSelection {
    /// The group coordinates nothing; each chip is on its own.
    Any = 0,
    /// Choosing one puts every other one back.
    Single = 1,
    /// Any number at once.
    #[pick]
    Multi = 2,
}

/// What a group reports, on top of what its chips report themselves.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ChipGroupAction {
    /// The set of chosen chips changed, by a press or by the group putting
    /// another one back.
    Changed,
    /// Every chip was asked to go, by the "clear all" a `FilterSummary`
    /// carries.
    Cleared,
    #[default]
    None,
}

/// What a chip reports. The host owns the list behind the chips, so the
/// chip states what happened to it and changes nothing outside itself.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ChipAction {
    /// Pressed and released over the body. Raised for every chip, chosen
    /// or not.
    Clicked,
    /// A selectable chip's state after the press.
    Toggled(bool),
    /// The cross was pressed: the host should take this chip out of its
    /// list.
    Removed,
    #[default]
    None,
}

/// The fill, the ink and the stroke a chip draws with, once its role,
/// appearance and state have been resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipColors {
    pub fill: Vec4f,
    pub ink: Vec4f,
    pub border: Vec4f,
    pub border_size: f32,
}

/// The colours for one appearance of one role. A chosen chip is drawn in
/// the loud form of whatever appearance it was given, so choosing reads as
/// a step up in weight rather than a change of colour.
pub fn chip_colors(family: IntentColors, appearance: ChipAppearance, selected: bool) -> ChipColors {
    let clear = Vec4f::default();
    if selected {
        return match appearance {
            ChipAppearance::Filled | ChipAppearance::Tonal => ChipColors {
                fill: family.base,
                ink: family.on_base,
                border: clear,
                border_size: 0.0,
            },
            ChipAppearance::Outline | ChipAppearance::Ghost => ChipColors {
                fill: family.container,
                ink: family.on_container,
                border: family.base,
                border_size: 1.0,
            },
        };
    }
    match appearance {
        ChipAppearance::Filled => ChipColors {
            fill: family.base,
            ink: family.on_base,
            border: clear,
            border_size: 0.0,
        },
        ChipAppearance::Tonal => ChipColors {
            fill: family.container,
            ink: family.on_container,
            border: clear,
            border_size: 0.0,
        },
        ChipAppearance::Outline => ChipColors {
            fill: clear,
            ink: family.base,
            border: family.base,
            border_size: 1.0,
        },
        ChipAppearance::Ghost => ChipColors {
            fill: clear,
            ink: family.base,
            border: clear,
            border_size: 0.0,
        },
    }
}

fn dimmed(color: Vec4f, opacity: f32) -> Vec4f {
    Vec4f { w: color.w * opacity, ..color }
}

/// The pill: a rounded box, its stroke, and the hover and press layers the
/// animator drives.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChip {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_2: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    radius: f32,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    /// Lit on the one chip the group's arrow keys are standing on.
    #[live]
    keyed: f32,
}

/// The tick a chosen chip carries and the cross a removable one carries,
/// drawn rather than loaded: they must follow the ink colour exactly, at
/// any size, on any theme.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChipMark {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    /// 0 draws nothing, 1 the tick, 2 the cross.
    #[live]
    mark: f32,
    /// Lifted while the pointer is on the cross itself, so the target
    /// answers before it is pressed.
    #[live]
    hover: f32,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Registered before the `use` below, because a block's `use` is a
    // snapshot taken when it runs. The bare variant names a call site
    // writes (`appearance: Outline`, `size: Large`) resolve against the
    // property's own type, so these live beside the badge's without
    // shadowing what it means there.
    mod.widgets.ChipAppearance = set_type_default() do #(ChipAppearance::script_api(vm))
    mod.widgets.splat(mod.widgets.ChipAppearance)
    mod.widgets.ChipSize = set_type_default() do #(ChipSize::script_api(vm))
    mod.widgets.splat(mod.widgets.ChipSize)
    mod.widgets.ChipSelection = set_type_default() do #(ChipSelection::script_api(vm))
    mod.widgets.splat(mod.widgets.ChipSelection)

    use mod.widgets.*

    mod.widgets.DrawChipBase = #(DrawChip::script_component(vm))
    set_type_default() do #(DrawChip::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawChipMarkBase = #(DrawChipMark::script_component(vm))
    set_type_default() do #(DrawChipMark::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ChipBase = #(Chip::register_widget(vm))
    /** A pill standing for one chosen thing: a filter, a recipient, a label. */
    mod.widgets.ChipFlat = set_type_default() do mod.widgets.ChipBase{
        width: Fit
        height: Fit
        /** the name the chip stands for */
        text: ""
        /** what the chip means: Neutral Primary Secondary Tertiary Error Warning Success Info */
        intent: Neutral
        /** how loudly: Filled Tonal Outline Ghost */
        appearance: Tonal
        /** Small Medium Large */
        size: Medium
        /** the press toggles the chip instead of only reporting a click */
        selectable: false
        /** the chip is chosen; only meaningful while selectable */
        selected: false
        /** a cross at the trailing edge asks the host to take the chip away */
        removable: false
        /** show the tick a chosen chip carries 0..1 */
        show_check: true
        /** no hover, no press, no focus: a chip that only reads out a value */
        interactive: true
        /** corner radius; the default is a full pill 0..24 step 0.5 */
        radius: 999.
        /** drawn at all; a host that removes a chip can hide it instead */
        visible: true
        /** dimmed and inert */
        disabled: false
        /** ink and fill alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        /** The colour of every role, shared with the badge so one meaning is one colour. */
        palette: mod.widgets.BadgePalette{}
        /** the icon's box before the label; draw_icon paints the SVG into it */
        icon_walk: Walk{/** icon width in pixels 8..40 step 1 */ width: 0., height: Fit}

        // The fill, the second gradient stop, the ink, the stroke, its
        // width, the radius and the two state layers are the INSTANCES,
        // written every draw from role, appearance and state; everything
        // else here is a uniform or the instance slots stop lining up.
        draw_bg +: {
            /** the state layer's strength while hovered 0..1 step 0.01 */
            hover_opacity: uniform(theme.state_hover_opacity)
            /** the state layer's strength while pressed 0..1 step 0.01 */
            press_opacity: uniform(theme.state_press_opacity)
            /** bevel strength: 0 flat, 1 the full two-stop stroke 0..1 step 0.05 */
            bevel: uniform(0.0)
            /** bevel stroke, top stop */
            color_bevel_1: uniform(theme.color_bevel_outset_1)
            /** bevel stroke, bottom stop */
            color_bevel_2: uniform(theme.color_bevel_outset_2)
            /** 0 flat, 1 shade the fill left to right, 2 top to bottom 0..2 step 1 */
            gradient: uniform(0.0)
            /** the ring marking where the arrow keys are standing */
            color_keyed: uniform(theme.color_bevel_focus)
            keyed: 0.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // `sdf.box` draws twice the radius it is given, and a radius
                // past half the height is simply a pill.
                let r = min(self.radius, self.rect_size.y * 0.5)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, r * 0.5)
                // Branch-free so the shader has no assignment to a
                // binding: pick the axis, then blend the gradient in or out.
                let t = mix(self.pos.x, self.pos.y, step(1.5, self.gradient))
                let fill = mix(self.color, mix(self.color, self.color_2, t), step(0.5, self.gradient))
                // Hover and press are the ink laid over the fill at the
                // theme's opacities: the same arithmetic every control uses,
                // so a chip reacts with a button's weight.
                let layer = self.hover * self.hover_opacity + self.down * self.press_opacity
                sdf.fill_keep(mix(fill, vec4(self.ink.xyz, 1.0), layer * self.ink.a))
                if self.border_size > 0.0 {
                    sdf.stroke_keep(self.border_color, self.border_size)
                }
                // The bevel follows the fill's alpha: an outline or a ghost
                // chip has no face to bevel.
                if self.bevel > 0.0 {
                    sdf.stroke_keep(mix(self.color_bevel_1, self.color_bevel_2, self.pos.y) * self.bevel * self.color.a, 1.0)
                }
                // Where the arrow keys are standing. Drawn as a ring around
                // the pill rather than a change to its face, because the
                // face already means chosen-or-not and a keyboard place is
                // not an answer. Without it the arrows moved and nothing on
                // screen moved with them.
                if self.keyed > 0.0 {
                    sdf.stroke(vec4(self.color_keyed.xyz, self.color_keyed.a * self.keyed), 1.5)
                }
                return sdf.result
            }
        }
        draw_mark +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let r = min(self.rect_size.x, self.rect_size.y) * 0.5
                if self.mark < 0.5 {
                    return sdf.result
                }
                if self.mark < 1.5 {
                    // The tick: two strokes, the short one down into the
                    // corner, the long one up across.
                    sdf.move_to(c.x - r * 0.72, c.y)
                    sdf.line_to(c.x - r * 0.18, c.y + r * 0.55)
                    sdf.line_to(c.x + r * 0.74, c.y - r * 0.58)
                    sdf.stroke(self.color, max(1.25, r * 0.30))
                    return sdf.result
                }
                // The cross, with a soft disc behind it while the pointer is
                // on the cross itself: the target says so before it is hit.
                if self.hover > 0.0 {
                    sdf.circle(c.x, c.y, r)
                    sdf.fill(vec4(self.color.xyz, 0.18 * self.hover))
                }
                sdf.move_to(c.x - r * 0.5, c.y - r * 0.5)
                sdf.line_to(c.x + r * 0.5, c.y + r * 0.5)
                sdf.move_to(c.x + r * 0.5, c.y - r * 0.5)
                sdf.line_to(c.x - r * 0.5, c.y + r * 0.5)
                sdf.stroke(self.color, max(1.1, r * 0.26))
                return sdf.result
            }
        }
        draw_text +: {
            text_style: theme.font_regular{}
            color: theme.color_text
        }
        draw_icon +: {
            color: theme.color_text
        }
        animator: Animator{
            /** pointer state: drives the hover and press layers */
            hover: {
                default: @off
                /** pointer away: both layers fade out together */
                off: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_3}}
                    ease: theme.motion_ease_standard
                    apply: {draw_bg: {hover: 0.0, down: 0.0}}
                }
                /** pointer over: the hover layer only */
                on: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_2}}
                    ease: theme.motion_ease_standard_decelerate
                    apply: {draw_bg: {hover: 1.0, down: 0.0}}
                }
                /** pressed: the press layer, taken instantly */
                down: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_1}}
                    apply: {draw_bg: {hover: 1.0, down: 1.0}}
                }
            }
        }
    }

    /** The chip with the outset bevel, to sit beside the bevelled buttons. */
    mod.widgets.Chip = mod.widgets.ChipFlat{
        draw_bg +: {
            bevel: uniform(1.0)
        }
    }

    /** The chip whose fill shades left to right. */
    mod.widgets.ChipGradientX = mod.widgets.Chip{
        draw_bg +: {
            gradient: uniform(1.0)
        }
    }

    /** The chip whose fill shades top to bottom. */
    mod.widgets.ChipGradientY = mod.widgets.Chip{
        draw_bg +: {
            gradient: uniform(2.0)
        }
    }

    /** The chip carrying only its leading icon. */
    mod.widgets.ChipIcon = mod.widgets.Chip{
        /** icon only: the label is empty */
        text: ""
        icon_walk: Walk{width: 14., height: Fit}
    }

    mod.widgets.ChipGroupBase = #(ChipGroup::register_widget(vm))
    /** A row of chips that agree on what "chosen" means, wrapping when the
     * row runs out and moving the keyboard focus along itself. */
    mod.widgets.ChipGroup = set_type_default() do mod.widgets.ChipGroupBase{
        width: Fill
        height: Fit
        flow: Right{wrap: true}
        spacing: theme.space_1
        /** how many chips may be chosen at once: Any Single Multi */
        selection: Multi
        /** Delete or Backspace on a focused removable chip asks for its removal */
        remove_on_delete: true
    }

    /** The chips a filter is currently made of, with a way to drop them all. */
    mod.widgets.FilterSummary = mod.widgets.ChipGroup{
        align: Align{y: 0.5}
        clear := LinkLabel{text: "Clear all"}
    }

    /** A label on a record: it states a fact and answers nothing, so it
     * takes no hover, no press and no focus. */
    mod.widgets.Tag = mod.widgets.ChipFlat{
        /** a tag reads out a value; it is not a control */
        interactive: false
        /** the quiet corner of a record, not a pill to press */
        radius: theme.radius_s
        size: Small
    }
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct Chip {
    #[rust]
    keyed: bool,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    pub draw_bg: DrawChip,
    #[live]
    draw_mark: DrawChipMark,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[apply_default]
    animator: Animator,

    /// The name the chip stands for.
    #[live]
    pub text: String,
    #[live]
    pub intent: BadgeIntent,
    #[live]
    pub appearance: ChipAppearance,
    #[live]
    pub size: ChipSize,
    /// The press toggles the chip instead of only reporting a click.
    #[live]
    pub selectable: bool,
    /// The chip is chosen; only meaningful while `selectable`.
    #[live]
    pub selected: bool,
    /// A cross at the trailing edge asks the host to take the chip away.
    #[live]
    pub removable: bool,
    /// Show the tick a chosen chip carries.
    #[live(true)]
    pub show_check: bool,
    /// No hover, no press, no focus: a chip that only reads out a value.
    #[live(true)]
    pub interactive: bool,
    #[live(999.0)]
    pub radius: f64,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live]
    pub palette: BadgePalette,
    #[live(true)]
    #[visible]
    visible: bool,

    /// Dimmed and inert. A story or a form can set it in the DSL, and
    /// `set_disabled` moves the same flag.
    #[live]
    pub disabled: bool,
    /// The cross's rect from the last draw, so the pointer can be tested
    /// against the cross before the body.
    #[rust]
    close_rect: Rect,
    /// True while the pointer is over the cross, so the body's hover layer
    /// stays off and the cross's disc comes on.
    #[rust]
    over_close: bool,
}

impl Chip {
    fn colors(&self) -> ChipColors {
        let mut colors = chip_colors(
            self.palette.family(self.intent),
            self.appearance,
            self.selectable && self.selected,
        );
        if self.disabled {
            colors.fill = dimmed(colors.fill, self.disabled_opacity);
            colors.ink = dimmed(colors.ink, self.disabled_opacity);
            colors.border = dimmed(colors.border, self.disabled_opacity);
        }
        colors
    }

    /// A tick is shown only where it means something: a selectable chip
    /// that is chosen, and only while the host wants it.
    fn shows_check(&self) -> bool {
        self.selectable && self.selected && self.show_check
    }

    fn measure_text(&mut self, cx: &mut Cx2d, scale: f64) -> f64 {
        if self.text.is_empty() {
            return 0.0;
        }
        let rest = self.draw_text.text_style.font_size;
        self.draw_text.text_style.font_size = rest * scale as f32;
        let width = measure(&self.draw_text, cx, &self.text);
        self.draw_text.text_style.font_size = rest;
        width
    }

    /// The chip's own size: the content laid out left to right inside the
    /// pill's padding, and never shorter than its height, so a chip with
    /// one letter is still a pill and not a circle squeezed flat.
    fn extent(&mut self, cx: &mut Cx2d) -> (f64, f64) {
        let m = self.size.metrics();
        let icon_w = match self.icon_walk.width {
            Size::Fixed(w) if w > 0.0 => w,
            _ => 0.0,
        };
        let text_w = self.measure_text(cx, m.font_scale);
        let mut w = m.pad_x * 2.0;
        let mut parts = 0.0;
        if self.shows_check() {
            w += m.mark;
            parts += 1.0;
        }
        if icon_w > 0.0 {
            w += icon_w;
            parts += 1.0;
        }
        if text_w > 0.0 {
            w += text_w;
            parts += 1.0;
        }
        if self.removable {
            w += m.mark;
            parts += 1.0;
        }
        if parts > 1.0 {
            w += m.gap * (parts - 1.0);
        }
        (w.max(m.height), m.height)
    }

    pub fn set_selected(&mut self, cx: &mut Cx, selected: bool) {
        if self.selected != selected {
            self.selected = selected;
            self.draw_bg.redraw(cx);
        }
    }

    /// Mark this chip as the one a group's arrow keys are standing on.
    /// The group owns this: a chip on its own has no arrows to stand under.
    pub fn set_keyed(&mut self, cx: &mut Cx, keyed: bool) {
        if self.keyed != keyed {
            self.keyed = keyed;
            self.draw_bg.redraw(cx);
        }
    }

    /// Draw at `walk` and answer the rect drawn. Set, draw, RESTORE: the
    /// text's colour and size belong to the call site, so the role's ink
    /// and the size rung are laid over them per draw rather than written
    /// into them.
    pub fn draw_chip(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        self.draw_bg.keyed = if self.keyed { 1.0 } else { 0.0 };
        let colors = self.colors();
        let m = self.size.metrics();
        let (w, h) = self.extent(cx);
        self.draw_bg.color = colors.fill;
        self.draw_bg.color_2 = dimmed(colors.fill, 0.55);
        self.draw_bg.ink = colors.ink;
        self.draw_bg.border_color = colors.border;
        self.draw_bg.border_size = colors.border_size;
        self.draw_bg.radius = self.radius as f32;

        let rect = self.draw_bg.draw_walk(cx, sized(walk, w, h));

        let mut x = rect.pos.x + m.pad_x;
        let cy = rect.pos.y + rect.size.y * 0.5;
        self.draw_mark.color = colors.ink;
        if self.shows_check() {
            self.draw_mark.mark = 1.0;
            self.draw_mark.hover = 0.0;
            self.draw_mark.draw_abs(
                cx,
                Rect { pos: dvec2(x, cy - m.mark * 0.5), size: dvec2(m.mark, m.mark) },
            );
            x += m.mark + m.gap;
        }
        if let Size::Fixed(icon_w) = self.icon_walk.width {
            if icon_w > 0.0 {
                self.draw_icon.color = colors.ink;
                self.draw_icon.draw_abs(
                    cx,
                    Rect { pos: dvec2(x, cy - icon_w * 0.5), size: dvec2(icon_w, icon_w) },
                );
                x += icon_w + m.gap;
            }
        }
        if !self.text.is_empty() {
            let rest_color = self.draw_text.color;
            let rest_size = self.draw_text.text_style.font_size;
            self.draw_text.color = colors.ink;
            self.draw_text.text_style.font_size = rest_size * m.font_scale as f32;
            let text_w = measure(&self.draw_text, cx, &self.text);
            let baseline = cy - (rest_size * m.font_scale as f32) as f64 * 0.7;
            self.draw_text.draw_abs(cx, dvec2(x, baseline), &self.text);
            self.draw_text.color = rest_color;
            self.draw_text.text_style.font_size = rest_size;
            // The cross is measured from the trailing edge, so nothing
            // after the label needs the running x.
            let _ = text_w;
        }
        self.close_rect = Rect::default();
        if self.removable {
            let close = Rect {
                pos: dvec2(rect.pos.x + rect.size.x - m.pad_x - m.mark, cy - m.mark * 0.5),
                size: dvec2(m.mark, m.mark),
            };
            self.draw_mark.mark = 2.0;
            self.draw_mark.hover = if self.over_close { 1.0 } else { 0.0 };
            self.draw_mark.draw_abs(cx, close);
            // The hit area is grown to the pill's height: a cross drawn at
            // ten points is not a target at ten points.
            self.close_rect = Rect {
                pos: dvec2(close.pos.x - m.gap * 0.5, rect.pos.y),
                size: dvec2(m.mark + m.gap * 0.5 + m.pad_x, rect.size.y),
            };
        }
        rect
    }
}

impl Widget for Chip {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_chip(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        if !self.interactive || self.disabled {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(MouseCursor::Hand);
                let over_close = self.close_rect.contains(fe.abs);
                if over_close != self.over_close {
                    self.over_close = over_close;
                    self.draw_bg.redraw(cx);
                }
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Default);
                if self.over_close {
                    self.over_close = false;
                    self.draw_bg.redraw(cx);
                }
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.over_close = self.close_rect.contains(fe.abs);
                self.animator_play(cx, ids!(hover.down));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if fe.is_over {
                    self.animator_play(cx, ids!(hover.on));
                    if fe.was_tap() {
                        // The cross first: "take this away" and "choose
                        // this" land a few points apart and must never be
                        // confused for one another.
                        if self.removable && self.close_rect.contains(fe.abs) {
                            cx.widget_action(uid, ChipAction::Removed);
                        } else {
                            if self.selectable {
                                self.selected = !self.selected;
                                cx.widget_action(uid, ChipAction::Toggled(self.selected));
                            }
                            cx.widget_action(uid, ChipAction::Clicked);
                        }
                    }
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    /// A selectable chip answers the test tree the way a checkbox does.
    fn snapshot_checked(&self, _cx: &Cx) -> Option<bool> {
        self.selectable.then_some(self.selected)
    }
}

impl ChipRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<ChipAction>(), ChipAction::Clicked);
        }
        false
    }

    /// The chip's state after a press that changed it.
    pub fn toggled(&self, actions: &Actions) -> Option<bool> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<ChipAction>() {
            ChipAction::Toggled(on) => Some(on),
            _ => None,
        }
    }

    pub fn removed(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<ChipAction>(), ChipAction::Removed);
        }
        false
    }

    pub fn selected(&self) -> bool {
        self.borrow().map(|inner| inner.selected).unwrap_or(false)
    }

    pub fn set_selected(&self, cx: &mut Cx, selected: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, selected);
        }
    }
}

/// A row of chips that agree on what "chosen" means.
///
/// The group owns exactly two things a chip cannot own by itself: the RULE
/// (choosing one may put the others back) and the keyboard's PLACE in the
/// row. Everything else stays with the chip, so a chip outside a group
/// behaves exactly as one inside it.
///
/// The row is ONE tab stop, not one per chip: Tab reaches the group, then
/// the arrows walk it. That is what a row of filters should be — someone
/// working by keyboard who wants the next control should not have to press
/// Tab nine times to get past nine filters.
#[derive(Script, ScriptHook, Widget)]
pub struct ChipGroup {
    #[rust]
    area: Area,
    #[deref]
    view: View,
    /// How many chips may be chosen at once.
    #[live]
    pub selection: ChipSelection,
    /// Delete or Backspace on the focused chip asks for its removal.
    #[live(true)]
    pub remove_on_delete: bool,
    /// Which chip the keyboard is on, as an index into the chips the group
    /// can see.
    #[rust]
    focused: usize,
}

impl ChipGroup {
    /// The chips under this group, in layout order. Only direct children
    /// count: a chip inside a nested view belongs to that view's own group,
    /// if it has one.
    fn chips(&self) -> Vec<WidgetRef> {
        let mut found = Vec::new();
        self.view.children(&mut |_id, child| {
            if child.borrow::<Chip>().is_some() {
                found.push(child);
            }
        });
        found
    }

    /// Put every chip but `keep` back. Answers whether anything moved, so
    /// the group only reports a change that actually happened.
    fn put_the_others_back(&mut self, cx: &mut Cx, keep: WidgetUid) -> bool {
        let mut changed = false;
        for chip in self.chips() {
            let Some(mut inner) = chip.borrow_mut::<Chip>() else {
                continue;
            };
            if inner.widget_uid() != keep && inner.selected {
                inner.set_selected(cx, false);
                changed = true;
            }
        }
        changed
    }

    fn move_focus(&mut self, cx: &mut Cx, delta: isize) {
        let chips = self.chips();
        let len = chips.len() as isize;
        if len == 0 {
            return;
        }
        // Step PAST what cannot be chosen, the way menu.rs already does.
        // Landing on a disabled chip is a dead stop: the ring says the keys
        // are there and Return then does nothing.
        let usable = |i: isize| -> bool {
            chips
                .get(i as usize)
                .and_then(|c| c.borrow::<Chip>())
                .map(|c| c.interactive && !c.disabled)
                .unwrap_or(false)
        };
        let mut at = self.focused as isize;
        for _ in 0..len {
            at = (at + delta).rem_euclid(len);
            if usable(at) {
                break;
            }
        }
        self.focused = at as usize;
        self.view.redraw(cx);
    }

    /// The labels of the chips that are chosen.
    pub fn chosen(&self) -> Vec<String> {
        self.chips()
            .iter()
            .filter_map(|chip| chip.borrow::<Chip>())
            .filter(|chip| chip.selected)
            .map(|chip| chip.text.clone())
            .collect()
    }

    /// Ask every removable chip to go. Each reports its own removal, so a
    /// host that already listens for the cross needs no second path.
    fn ask_all_to_go(&mut self, cx: &mut Cx) {
        for chip in self.chips() {
            let Some(inner) = chip.borrow::<Chip>() else {
                continue;
            };
            let (removable, chip_uid) = (inner.removable, inner.widget_uid());
            drop(inner);
            if removable {
                cx.widget_action(chip_uid, ChipAction::Removed);
            }
        }
    }

    fn toggle_focused(&mut self, cx: &mut Cx) -> bool {
        let Some(chip) = self.chips().get(self.focused).cloned() else {
            return false;
        };
        let Some(mut inner) = chip.borrow_mut::<Chip>() else {
            return false;
        };
        // The mouse refuses a disabled chip; the keyboard was toggling it.
        if !inner.selectable || inner.disabled || !inner.interactive {
            return false;
        }
        let on = !inner.selected;
        inner.set_selected(cx, on);
        let keep = inner.widget_uid();
        drop(inner);
        if on && self.selection == ChipSelection::Single {
            self.put_the_others_back(cx, keep);
        }
        true
    }

    /// Which chip this round's actions came from, if any.
    fn pressed_index(&self, actions: &Actions) -> Option<usize> {
        self.chips().iter().position(|chip| {
            chip.borrow::<Chip>()
                .map(|inner| actions.find_widget_action(inner.widget_uid()).is_some())
                .unwrap_or(false)
        })
    }

    fn remove_focused(&mut self, cx: &mut Cx) {
        let Some(chip) = self.chips().get(self.focused).cloned() else {
            return;
        };
        let Some(inner) = chip.borrow::<Chip>() else {
            return;
        };
        let (removable, chip_uid) = (inner.removable, inner.widget_uid());
        drop(inner);
        if removable {
            cx.widget_action(chip_uid, ChipAction::Removed);
        }
    }
}

impl Widget for ChipGroup {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Marked BEFORE the chips draw. Set after, the flag lands on widgets
        // that have already painted and the ring waits for a redraw that
        // nothing asks for - the arrows moved and the row sat still.
        //
        // Exactly one chip carries it, and only while the row holds the
        // keyboard: where the arrows are standing means nothing to someone
        // using the mouse, and a ring left behind reads as a second kind of
        // chosen.
        let keyed = if cx.cx.cx.has_key_focus(self.area) {
            Some(self.focused)
        } else {
            None
        };
        for (i, chip) in self.chips().into_iter().enumerate() {
            if let Some(mut inner) = chip.borrow_mut::<Chip>() {
                inner.set_keyed(cx.cx.cx, keyed == Some(i));
            }
        }

        let step = self.view.draw_walk(cx, scope, walk);
        // The view hands out a fresh area every redraw and migrates nothing,
        // so a key focus pointed at it stops matching the moment anything
        // repaints - which is why the arrows had never once answered. The
        // group keeps its own handle and moves the focus along with it.
        self.area = cx.update_area_refs(self.area, self.view.area());
        // The doc above this type has always said the row is one tab stop.
        // Nothing registered one, so Tab walked straight past the group and
        // the arrows only answered someone who had already clicked into it.
        if !self.disabled(cx.cx.cx) {
            cx.add_nav_stop(self.area, NavRole::TextInput, Inset::default());
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        // The group deliberately does NOT hit-test its own area: that area
        // covers its chips, and taking the hit would capture the pointer
        // before a chip ever saw it. Keys are read straight off the event
        // instead, and only while the row holds the key focus.
        if let Event::KeyDown(ke) = event {
            if cx.has_key_focus(self.area) {
                match ke.key_code {
                    KeyCode::ArrowRight | KeyCode::ArrowDown => self.move_focus(cx, 1),
                    KeyCode::ArrowLeft | KeyCode::ArrowUp => self.move_focus(cx, -1),
                    KeyCode::Home => {
                        self.focused = 0;
                        self.view.redraw(cx);
                    }
                    KeyCode::End => {
                        self.focused = self.chips().len().saturating_sub(1);
                        self.view.redraw(cx);
                    }
                    KeyCode::ReturnKey | KeyCode::Space => {
                        if self.toggle_focused(cx) {
                            cx.widget_action(uid, ChipGroupAction::Changed);
                        }
                    }
                    KeyCode::Delete | KeyCode::Backspace if self.remove_on_delete => {
                        self.remove_focused(cx);
                    }
                    _ => {}
                }
            }
        }
        self.view.handle_event(cx, event, scope);

        // Taken on any press inside the row, not only on one that changed an
        // answer. A chip that is already on does not toggle, so it reports
        // nothing, so the focus would stay on whatever the hand had touched
        // last and the arrows would answer nobody.
        //
        // THE POINTER-CAPTURE RULE. This reads the raw press instead of a
        // hit, so nothing has told it that the pointer may already be locked
        // to a control -- and nothing can: a widget that ALREADY holds the
        // mouse is handed its press straight off the capture list without
        // marking the event at all, so a press arriving mid-drag looks
        // exactly like a fresh one from here. While something else owns the
        // pointer the row takes no focus from it.
        //
        // The row's OWN areas are not "something else". Its chips are its
        // answers, and a press on one is still the row's to answer -- that
        // is the whole point of the arm. Only the mouse locks, so a finger
        // held on a control elsewhere leaves this alone.
        if let Event::MouseDown(e) = event {
            let mut mine = vec![self.area];
            mine.extend(
                self.chips()
                    .iter()
                    .filter_map(|chip| chip.borrow::<Chip>().map(|inner| inner.draw_bg.area())),
            );
            if !cx.fingers.is_mouse_held_outside(&mine)
                && !self.area.is_empty()
                && self.area.rect(cx).contains(e.abs)
            {
                cx.set_key_focus(self.area);
            }
        }

        // The ring is decided at draw time from who holds the keyboard, so
        // the row has to be told when that changed - otherwise the focus
        // leaves and the ring stays behind, still claiming a place the
        // arrows no longer answer from.
        if let Event::KeyFocus(kf) = event {
            if kf.prev == self.area || kf.focus == self.area {
                self.view.redraw(cx);
            }
        }

        if let Event::Actions(actions) = event {
            let mut changed = false;
            for chip in self.chips() {
                let Some(inner) = chip.borrow::<Chip>() else {
                    continue;
                };
                let chip_uid = inner.widget_uid();
                let selected = inner.selected;
                drop(inner);
                let Some(action) = actions.find_widget_action(chip_uid) else {
                    continue;
                };
                if let ChipAction::Toggled(_) = action.cast::<ChipAction>() {
                    changed = true;
                    if selected && self.selection == ChipSelection::Single {
                        self.put_the_others_back(cx, chip_uid);
                    }
                }
            }
            if changed {
                // A press moves the keyboard's place to the chip that was
                // pressed, so the arrows carry on from where the hand left
                // off rather than from wherever they were.
                if let Some(index) = self.pressed_index(actions) {
                    self.focused = index;
                }
                cx.set_key_focus(self.area);
                cx.widget_action(uid, ChipGroupAction::Changed);
            }
            // The clear link is found by name rather than by type: a host
            // may put any widget there, and only its click matters.
            let clear = self.view.widget(cx, ids!(clear));
            if !clear.is_empty() && clear.as_link_label().clicked(actions) {
                self.ask_all_to_go(cx);
                cx.widget_action(uid, ChipGroupAction::Cleared);
            }
        }
    }

    /// The chosen chips, comma separated: what the row is filtering by, in
    /// one line a test can wait on.
    fn text(&self) -> String {
        self.chosen().join(", ")
    }
}

impl ChipGroupRef {
    /// The labels of the chips that are chosen.
    pub fn chosen(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.chosen()).unwrap_or_default()
    }

    pub fn changed(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<ChipGroupAction>(), ChipGroupAction::Changed);
        }
        false
    }

    pub fn cleared(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<ChipGroupAction>(), ChipGroupAction::Cleared);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Choosing a chip is a step up in weight within the appearance it was
    /// given, never a change of colour: the role has to survive the state.
    #[test]
    fn choosing_a_chip_raises_its_weight_and_keeps_its_role() {
        let family = IntentColors {
            base: Vec4f { x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
            on_base: Vec4f { x: 1.0, y: 1.0, z: 1.0, w: 1.0 },
            container: Vec4f { x: 0.4, y: 0.0, z: 0.0, w: 1.0 },
            on_container: Vec4f { x: 0.9, y: 0.9, z: 0.9, w: 1.0 },
        };
        let tonal = chip_colors(family, ChipAppearance::Tonal, false);
        assert_eq!(tonal.fill, family.container);
        let tonal_on = chip_colors(family, ChipAppearance::Tonal, true);
        assert_eq!(tonal_on.fill, family.base, "a chosen tonal chip fills with its role");
        let outline = chip_colors(family, ChipAppearance::Outline, false);
        assert_eq!(outline.fill.w, 0.0, "an outline chip has no face");
        assert_eq!(outline.border, family.base);
        let outline_on = chip_colors(family, ChipAppearance::Outline, true);
        assert_eq!(outline_on.fill, family.container, "choosing gives the outline a face");
        assert_eq!(outline_on.border, family.base, "and keeps its stroke");
    }

    /// The ladder is heights a finger can find, and it only ever goes up.
    #[test]
    fn the_size_ladder_climbs() {
        let small = ChipSize::Small.metrics();
        let medium = ChipSize::Medium.metrics();
        let large = ChipSize::Large.metrics();
        assert!(small.height < medium.height && medium.height < large.height);
        assert!(small.pad_x < medium.pad_x && medium.pad_x < large.pad_x);
        assert!(small.mark < medium.mark && medium.mark < large.mark);
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to a row of chips.
///
/// The row moves the keyboard onto itself from a RAW press, so that the arrows
/// answer from wherever the hand last touched. Nothing tells a raw press that
/// the pointer may already be locked to a control -- a widget that already
/// holds the mouse is handed its press off the capture list without marking
/// the event at all -- so the row has to ask the capture list itself, and take
/// no focus from a pointer that is not free.
#[cfg(test)]
mod pointer_capture_tests {
    #![allow(dead_code)]
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    /// One press, handed to `widget` alone -- the way the tree hands a press
    /// to one branch at a time. Dispatching through the whole root instead
    /// would let the button that is holding the pointer answer the SECOND
    /// press as well (it is handed its own FingerDown off the capture list)
    /// and take the keyboard back, which would hide what is being tested.
    fn send(cx: &mut Cx, widget: &WidgetRef, at: Vec2d) {
        widget.handle_event(cx, &press(at), &mut Scope::empty());
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> Vec2d {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    grab := Button{width: 100. height: 40. text: "grab"}
                    row := ChipGroup{
                        flow: Right
                        one := Chip{text: "one" selectable: true}
                        two := Chip{text: "two" selectable: true}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// `set_key_focus` only records the request; the focus moves when the
    /// action cycle runs. A bare press raises no action in this scene, so one
    /// is pushed to turn the handle.
    fn settle(cx: &mut Cx) {
        cx.action(ChipGroupAction::None);
        cx.handle_actions();
    }

    /// Presses the first chip, with the pointer already locked to the button
    /// above the row when `held`. Answers (where the keyboard ended up, the
    /// row's own area).
    fn press_a_chip(held: bool) -> (Area, Area) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let grab = root.widget(&cx, ids!(grab));
        let row = root.widget(&cx, ids!(row));
        let one = root.widget(&cx, ids!(row.one));
        let row_area = row.borrow::<ChipGroup>().unwrap().area;
        let on_chip = middle(&cx, &one);

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        if held {
            let on_grab = middle(&cx, &grab);
            send(&mut cx, &grab, on_grab);
            assert!(
                cx.fingers.is_mouse_held_outside(&[row_area]),
                "the button did not take the pointer, so this test proves nothing"
            );
        }
        send(&mut cx, &row, on_chip);
        cx.fingers.first_mouse_button = None;
        settle(&mut cx);
        (cx.key_focus(), row_area)
    }

    /// The control: a press on a chip with nothing else holding the pointer is
    /// the row's, and the keyboard follows the hand onto it.
    #[test]
    fn a_press_nothing_else_holds_moves_the_keyboard_onto_the_row() {
        let (focus, row) = press_a_chip(false);
        assert_eq!(focus, row, "the row stopped taking the keyboard from its own press");
    }

    /// The rule: a press that arrives while another control holds the mouse is
    /// that control's, and the row takes no keyboard from it.
    #[test]
    fn a_press_another_control_holds_moves_no_keyboard() {
        let (focus, row) = press_a_chip(true);
        assert_ne!(focus, row, "the row pulled the keyboard off a control mid-drag");
    }
}
