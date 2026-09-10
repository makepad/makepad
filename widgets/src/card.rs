//! Card — a surface that holds a picture, a header, a body and a footer,
//! and knows which rung of the theme's elevation ladder it stands on.
//!
//! Three appearances, and they are three settings of ONE surface rather
//! than three drawings of a card. `Elevated` takes the low container tint
//! and rests on the ladder's first rung; `Filled` takes the highest
//! container tint and lies flat; `Outlined` takes the page's own colour
//! with a line around it, and lies flat as well. Every fill, every line
//! and every shadow comes out of [`CardPalette`], which reads the theme's
//! surface, outline and elevation tokens. A theme change therefore moves
//! all three appearances together, and a card cannot end up a shade
//! nothing else in the window is.
//!
//! **A pressable card rises under the pointer and comes back down under
//! the finger.** The lift is the ladder: hovering moves the card one rung
//! up, pressing returns it to the rung it rests on. Rising away from the
//! finger that is pressing it is the one thing about a card's shadow that
//! people notice, and it is always wrong. `pressable` is off by default,
//! because a surface that lights up under the pointer and then answers
//! nothing is a promise the widget cannot keep.
//!
//! **The media band bleeds to the card's edges.** A picture that stops
//! short of the rounding is not a card's picture, it is a picture in a
//! card. So the card holds no padding of its own on the outside — the
//! media is laid out first, at the full width, against the bare edge — and
//! the `padding` a caller writes belongs to the header, body and footer
//! together, which are laid out inside it. The band's own top corners are
//! rounded by [`CardMedia`], which draws its children into a texture and
//! samples that inside the curve; nothing else in the library clips a
//! child to anything but a rectangle. The card pushes its own radius into
//! that band every draw, so there is one radius and one place to change it.
//!
//! **What it is not.** It does not scroll: a card whose body scrolls is a
//! panel, and the widget for that already exists. It carries no title or
//! subtitle typography — the header slot takes whatever a caller puts in
//! it, and the library's headings are already a ladder of their own. It
//! has no expanded state, no swipe, and no fourth appearance: a card that
//! is a surface can be any of these three, and a card that needs to be
//! something else is not a card.
//!
//! One trap worth stating plainly: a pressable card whose footer holds
//! buttons only hears the presses that reach the surface. The buttons are
//! drawn after the card and sit on top of it, so a press on a button is
//! the button's and never the card's. That is the behaviour a person
//! expects and the reason a card's own action is worth having, but it does
//! mean a card is not a way to make its footer bigger.

use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    view::ViewWidgetRefExt,
    widget::*,
    CxWidgetExt,
};

/// How a card's surface is dressed. All three are settings of one shader:
/// a fill, an optional line, and a rung of the elevation ladder.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum CardAppearance {
    /// The low container tint, lifted off the page by a shadow.
    #[pick]
    Elevated = 0,
    /// The highest container tint, lying flat on the page.
    Filled = 1,
    /// The page's own colour with a line around it, lying flat.
    Outlined = 2,
}

/// What a card reports. The host owns whatever is behind the card, so the
/// card states what happened to it and changes nothing outside itself.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum CardAction {
    /// The surface was pressed. Raised on the way down, before anything is
    /// known about whether the press will finish here.
    Pressed,
    /// Pressed and released over the surface.
    Clicked,
    #[default]
    None,
}

/// One rung of the theme's elevation ladder: the ink of the shadow, how
/// far it blurs, and how far it drops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CardShadow {
    pub color: Vec4f,
    pub radius: f32,
    pub offset_y: f32,
}

/// Everything a card is dressed from, as plain numbers.
///
/// [`CardPalette`] is the script-visible half and reads the theme; this is
/// what it answers, so the arithmetic below can be exercised without a
/// script heap behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CardTokens {
    pub elevated: Vec4f,
    pub filled: Vec4f,
    pub outlined: Vec4f,
    pub outline: Vec4f,
    pub outline_size: f32,
    /// Text and icons on any of the three, and the ink of the state layer.
    pub ink: Vec4f,
    pub shadow_1: CardShadow,
    pub shadow_2: CardShadow,
    pub shadow_3: CardShadow,
}

impl CardTokens {
    /// The rung at `level`. Level 0 is the ground — no ink, no blur, no
    /// drop — which is what a flat card rests on and what a card that has
    /// been switched off falls back to.
    pub fn rung(&self, level: u32) -> CardShadow {
        match level {
            0 => CardShadow::default(),
            1 => self.shadow_1,
            2 => self.shadow_2,
            _ => self.shadow_3,
        }
    }
}

/// The fill, the ink, the line and the shadow a card draws with, once its
/// appearance and its state have been resolved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CardSurface {
    pub fill: Vec4f,
    pub ink: Vec4f,
    pub border: Vec4f,
    pub border_size: f32,
    pub shadow: Vec4f,
    pub shadow_radius: f32,
    pub shadow_offset_y: f32,
}

/// The rung each appearance rests on.
///
/// Only the elevated card is off the ground. A filled card is a tint of
/// the page and an outlined one is a line drawn on it; giving either a
/// shadow makes all three the same card with different fills, which is
/// exactly what having three appearances is meant to avoid.
pub fn card_rest_level(appearance: CardAppearance) -> u32 {
    match appearance {
        CardAppearance::Elevated => 1,
        CardAppearance::Filled | CardAppearance::Outlined => 0,
    }
}

/// How far a card is raised, from the two state weights the animator
/// drives.
///
/// A card rises under the pointer and comes back DOWN under the finger.
/// Pressing something and watching it move away from the finger is the
/// wrong way round, and the shadow is where anyone would see it.
pub fn card_lift(hover: f32, down: f32) -> f32 {
    (hover - down).clamp(0.0, 1.0)
}

fn fade(color: Vec4f, opacity: f32) -> Vec4f {
    Vec4f { w: color.w * opacity, ..color }
}

fn lerp_color(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    Vec4f {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        z: a.z + (b.z - a.z) * t,
        w: a.w + (b.w - a.w) * t,
    }
}

/// The surface for one appearance at one height.
///
/// `lift` moves the card between the rung it rests on and the one above,
/// so hovering is a movement along the theme's ladder rather than a second
/// shadow invented for the occasion. `disabled` carries the opacity a
/// switched-off card fades its line and its ink by.
pub fn card_surface(
    tokens: CardTokens,
    appearance: CardAppearance,
    lift: f32,
    disabled: Option<f32>,
) -> CardSurface {
    // Nothing inert is raised. A card that has been switched off lies flat
    // and takes no lift, whatever the pointer is doing over it.
    let lift = if disabled.is_some() { 0.0 } else { lift.clamp(0.0, 1.0) };
    let rest = if disabled.is_some() { 0 } else { card_rest_level(appearance) };
    let from = tokens.rung(rest);
    let to = tokens.rung(rest + 1);
    let mut out = CardSurface {
        fill: match appearance {
            CardAppearance::Elevated => tokens.elevated,
            CardAppearance::Filled => tokens.filled,
            CardAppearance::Outlined => tokens.outlined,
        },
        ink: tokens.ink,
        border: match appearance {
            CardAppearance::Outlined => tokens.outline,
            _ => Vec4f::default(),
        },
        border_size: match appearance {
            CardAppearance::Outlined => tokens.outline_size,
            _ => 0.0,
        },
        shadow: lerp_color(from.color, to.color, lift),
        shadow_radius: from.radius + (to.radius - from.radius) * lift,
        shadow_offset_y: from.offset_y + (to.offset_y - from.offset_y) * lift,
    };
    if let Some(opacity) = disabled {
        // The fill is left alone on purpose. A surface that fades out
        // stops being a surface, and it is the content standing on it that
        // has to read as unavailable — which the slots do for themselves,
        // because the card hands each of them the same flag.
        out.border = fade(out.border, opacity);
        out.ink = fade(out.ink, opacity);
    }
    out
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Registered before the `use` below, because a block's `use` is a
    // snapshot of the namespace as it stood when the block began. The bare
    // names a call site writes (`appearance: Outlined`) resolve against the
    // property's own type, so these sit beside the chip's `Filled` without
    // changing what it means there.
    mod.widgets.CardAppearance = set_type_default() do #(CardAppearance::script_api(vm))
    mod.widgets.splat(mod.widgets.CardAppearance)

    /** Every colour a card is dressed from: the fill of each appearance,
     * the line an outlined one draws, the ink of its state layer, and three
     * rungs of the theme's elevation ladder. One place, so the three
     * appearances cannot drift apart. */
    mod.widgets.CardPalette = set_type_default() do #(CardPalette::script_api(vm)){
        /** the fill of an elevated card */
        elevated: theme.color_surface_container_low
        /** the fill of a filled card */
        filled: theme.color_surface_container_highest
        /** the fill of an outlined card: the page's own colour */
        outlined: theme.color_surface
        /** the line an outlined card draws */
        outline: theme.color_outline_variant
        /** that line's width in pixels 0..4 step 0.5 */
        outline_size: 1.0
        /** ink on any of the three; the state layer is this colour */
        ink: theme.color_on_surface
        /** the resting rung: a card lying on the page */
        shadow_1: theme.color_elevation_1
        shadow_1_radius: theme.elevation_1_radius
        shadow_1_offset_y: theme.elevation_1_offset_y
        /** the raised rung: a card under the pointer */
        shadow_2: theme.color_elevation_2
        shadow_2_radius: theme.elevation_2_radius
        shadow_2_offset_y: theme.elevation_2_offset_y
        /** the rung above that, for a host that lifts a card itself */
        shadow_3: theme.color_elevation_3
        shadow_3_radius: theme.elevation_3_radius
        shadow_3_offset_y: theme.elevation_3_offset_y
    }

    use mod.widgets.*

    mod.widgets.DrawCardBase = #(DrawCard::script_component(vm))
    set_type_default() do #(DrawCard::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.CardBase = #(Card::register_widget(vm))
    /** A surface holding a picture, a header, a body and a footer. */
    mod.widgets.Card = set_type_default() do mod.widgets.CardBase{
        // Fit here would collapse the content turtle, which asks for the
        // card's width: a card is a surface something is laid out on, so it
        // takes the width it is given and finds its own height.
        width: Fill
        height: Fit
        flow: Down
        // The padding of the HEADER, BODY and FOOTER together. The media is
        // laid out before it and reaches the card's edges.
        padding: theme.mspace_2
        spacing: theme.space_2
        /** how the surface is dressed: Elevated Filled Outlined */
        appearance: Elevated
        /** the whole surface answers a press */
        pressable: false
        /** dimmed and inert; the flag reaches all four slots */
        disabled: false
        /** the line's and the ink's alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        /** corner rounding in points; the media band follows it 0..40 step 0.5 */
        // This is the radius, not draw_bg.border_radius: the card writes
        // this one into the shader and into the media band on every draw,
        // so a value set on the shader directly lasts a single frame.
        radius: theme.radius_l
        /** drawn at all */
        visible: true
        /** Every colour and every rung the three appearances are made of. */
        palette: mod.widgets.CardPalette{}

        /** The surface: a rounded box, its line, its shadow, and the state
         * layer the animator drives.
         *
         * The two opacities are uniforms because the shader alone owns
         * them. Everything below them is PLAIN, and every one of those has
         * a field on the draw struct behind it, written from Rust on every
         * draw: a shader value with a Rust field behind it has to be
         * declared plain, or the runtime refuses the binding and the value
         * never arrives. */
        draw_bg +: {
            /** the state layer's strength while hovered 0..1 step 0.01 */
            hover_opacity: uniform(theme.state_hover_opacity)
            /** the state layer's strength while pressed 0..1 step 0.01 */
            press_opacity: uniform(theme.state_press_opacity)

            hover: 0.0
            down: 0.0

            color: theme.color_surface_container_low
            ink: theme.color_on_surface
            border_color: theme.color_outline_variant
            border_size: 0.0
            border_radius: theme.radius_l
            shadow_color: theme.color_elevation_1
            shadow_radius: theme.elevation_1_radius
            shadow_offset_y: theme.elevation_1_offset_y

            rect_size2: varying(vec2(0))
            rect_size3: varying(vec2(0))
            rect_pos2: varying(vec2(0))
            rect_shift: varying(vec2(0))
            sdf_rect_pos: varying(vec2(0))
            sdf_rect_size: varying(vec2(0))

            vertex: fn() {
                // The shadow is drawn OUTSIDE the card's rect: the quad
                // grows by the blur and the drop, and the card's own box is
                // placed back inside it. The turtle never sees any of that,
                // so the card still takes exactly the room its content
                // asked for and a shadow costs no layout.
                let offset = vec2(0.0, self.shadow_offset_y)
                let min_offset = min(offset, vec2(0.0))
                self.rect_size2 = self.rect_size + 2.0 * vec2(self.shadow_radius)
                self.rect_size3 = self.rect_size2 + abs(offset)
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius)
                self.rect_shift = -min_offset
                return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
                // sdf.box draws twice the radius it is given, so the radius
                // a caller writes is halved here and means points.
                let r = min(self.border_radius, min(self.sdf_rect_size.x, self.sdf_rect_size.y) * 0.5)
                sdf.box(
                    self.sdf_rect_pos.x,
                    self.sdf_rect_pos.y,
                    self.sdf_rect_size.x,
                    self.sdf_rect_size.y,
                    r * 0.5
                )
                // A flat card asks for a shadow of no blur, and the
                // gaussian divides by that blur. Without the guard a filled
                // or an outlined card at rest would be shaded with whatever
                // a division by zero gives on the machine it is running on.
                if self.shadow_radius > 0.0 && sdf.shape > -1.0 {
                    let m = self.shadow_radius
                    let o = vec2(0.0, self.shadow_offset_y) + self.rect_shift
                    let v = GaussShadow.rounded_box_shadow(
                        vec2(m) + o,
                        self.rect_size2 + o,
                        self.pos * (self.rect_size3 + vec2(m)),
                        self.shadow_radius * 0.5,
                        r
                    )
                    sdf.clear(self.shadow_color * v)
                }
                // Hover and press are the ink laid over the fill at the
                // theme's opacities: the same arithmetic every other
                // control in the library uses, so a card answers the
                // pointer with a button's weight.
                let layer = self.hover * self.hover_opacity + self.down * self.press_opacity
                sdf.fill_keep(mix(self.color, vec4(self.ink.xyz, 1.0), layer * self.ink.a))
                if self.border_size > 0.0 {
                    sdf.stroke(self.border_color, self.border_size)
                }
                return sdf.result
            }
        }

        // Bare slots, not named instances: a slot takes a value, so a
        // caller writes `header: CardHeader{...}` and not
        // `header := CardHeader{...}`, which would make a child and leave
        // the slot empty. They start invisible so that a card given only a
        // body does not pay three lots of spacing for the two it has not
        // got; anything a caller puts in a slot is visible by default.
        media: View{width: Fill height: Fit visible: false}
        header: View{width: Fill height: Fit visible: false}
        body: View{width: Fill height: Fit visible: false}
        footer: View{width: Fill height: Fit visible: false}

        animator: Animator{
            /** pointer state: drives the state layer and the lift together */
            hover: {
                default: @off
                /** pointer away: the card settles back onto its own rung */
                off: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_3}}
                    ease: theme.motion_ease_standard
                    apply: {draw_bg: {hover: 0.0, down: 0.0}}
                }
                /** pointer over: one rung up, and the hover layer */
                on: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_2}}
                    ease: theme.motion_ease_standard_decelerate
                    apply: {draw_bg: {hover: 1.0, down: 0.0}}
                }
                /** pressed: back down to the resting rung, taken at once */
                down: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_1}}
                    apply: {draw_bg: {hover: 1.0, down: 1.0}}
                }
            }
        }
    }

    /** The card that lies on the page and casts a shadow. */
    mod.widgets.ElevatedCard = mod.widgets.Card{
        appearance: Elevated
    }

    /** The card that is a tint of the page rather than a lift off it, for
     * a grid of cards where a field of shadows would be noise. */
    mod.widgets.FilledCard = mod.widgets.Card{
        appearance: Filled
    }

    /** The card drawn as a line on the page, for a dense list where even a
     * tint is too much weight. */
    mod.widgets.OutlinedCard = mod.widgets.Card{
        appearance: Outlined
    }

    /** The card the whole of which answers a press. */
    mod.widgets.PressableCard = mod.widgets.Card{
        pressable: true
    }

    /** The band a card's picture sits in.
     *
     * It reaches the card's edges and rounds its own top corners to match,
     * by drawing its children into a texture and sampling that inside the
     * curve — nothing else in the library clips a child to anything but a
     * rectangle. The bottom corners stay square because the body of the
     * card is directly under them. `border_radius` is written by the card
     * on every draw, so a caller changes one radius, not two. */
    mod.widgets.CardMedia = mod.widgets.CachedRoundedView{
        width: Fill
        height: Fit
        draw_bg +: {
            /** the top corners' radius; the card writes its own here 0..40 step 0.5 */
            border_radius: uniform(theme.radius_l)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box_y(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius * 0.5,
                    0.0
                )
                let color = self.image.sample(self.pos * self.scale + self.shift)
                sdf.fill_keep_premul(color)
                return sdf.result
            }
        }
    }

    /** A card's top band: the title and whatever stands beside it. */
    mod.widgets.CardHeader = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    /** A card's text. */
    mod.widgets.CardBody = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    /** A card's actions, gathered at its trailing edge. */
    mod.widgets.CardFooter = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 1.0 y: 0.5}
    }
}

/// The surface: a rounded box, its line, its shadow, and the two state
/// layers the animator drives.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCard {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    color: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    shadow_color: Vec4f,
    #[live]
    shadow_radius: f32,
    /// Cards drop straight down, so the offset is one number rather than a
    /// vector: a card lit from the side is a card lit differently from
    /// every menu and dialog in the same window.
    #[live]
    shadow_offset_y: f32,
}

/// Every colour and every rung the three appearances are made of.
#[derive(Script, ScriptHook)]
pub struct CardPalette {
    #[live]
    pub elevated: Vec4f,
    #[live]
    pub filled: Vec4f,
    #[live]
    pub outlined: Vec4f,
    #[live]
    pub outline: Vec4f,
    #[live(1.0)]
    pub outline_size: f32,
    #[live]
    pub ink: Vec4f,
    #[live]
    pub shadow_1: Vec4f,
    #[live]
    pub shadow_1_radius: f32,
    #[live]
    pub shadow_1_offset_y: f32,
    #[live]
    pub shadow_2: Vec4f,
    #[live]
    pub shadow_2_radius: f32,
    #[live]
    pub shadow_2_offset_y: f32,
    #[live]
    pub shadow_3: Vec4f,
    #[live]
    pub shadow_3_radius: f32,
    #[live]
    pub shadow_3_offset_y: f32,
}

impl CardPalette {
    /// The palette as plain numbers, for the arithmetic that dresses a
    /// card.
    pub fn tokens(&self) -> CardTokens {
        CardTokens {
            elevated: self.elevated,
            filled: self.filled,
            outlined: self.outlined,
            outline: self.outline,
            outline_size: self.outline_size,
            ink: self.ink,
            shadow_1: CardShadow {
                color: self.shadow_1,
                radius: self.shadow_1_radius,
                offset_y: self.shadow_1_offset_y,
            },
            shadow_2: CardShadow {
                color: self.shadow_2,
                radius: self.shadow_2_radius,
                offset_y: self.shadow_2_offset_y,
            },
            shadow_3: CardShadow {
                color: self.shadow_3,
                radius: self.shadow_3_radius,
                offset_y: self.shadow_3_offset_y,
            },
        }
    }
}

#[derive(Script, Widget, Animator)]
pub struct Card {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    /// The layout of the CONTENT, not of the card: the media is laid out
    /// before this padding is opened.
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    pub draw_bg: DrawCard,
    #[apply_default]
    animator: Animator,

    /// The picture at the top, reaching the card's edges.
    #[find]
    #[live]
    pub media: WidgetRef,
    /// The title band.
    #[find]
    #[live]
    pub header: WidgetRef,
    /// What the card is about.
    #[find]
    #[live]
    pub body: WidgetRef,
    /// The actions, at the trailing edge.
    #[find]
    #[live]
    pub footer: WidgetRef,

    #[live]
    pub appearance: CardAppearance,
    /// The whole surface answers a press.
    #[live]
    pub pressable: bool,
    /// Dimmed and inert. A story or a form can set it in the DSL, and
    /// `set_disabled` moves the same flag.
    #[live]
    pub disabled: bool,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(8.0)]
    pub radius: f64,
    #[live]
    pub palette: CardPalette,
    #[live(true)]
    #[visible]
    visible: bool,
}

impl ScriptHook for Card {
    /// A `disabled: true` written in the DSL sets the field directly and
    /// never reaches `set_disabled`, so without this the surface would go
    /// flat while the buttons in the footer stayed live — the card would
    /// look switched off and still answer.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        if self.disabled {
            vm.with_cx_mut(|cx| {
                for slot in [&self.media, &self.header, &self.body, &self.footer] {
                    slot.set_disabled(cx, true);
                }
            });
        }
    }
}

impl Card {
    /// The surface this card draws with right now.
    ///
    /// The lift is read back out of the shader's own animated values, so
    /// the shadow climbs the ladder at exactly the rate the state layer
    /// fades in. Tracking hover and press in Rust as well would give two
    /// clocks for one movement, and they would not agree.
    pub fn surface(&self) -> CardSurface {
        let lift = card_lift(self.draw_bg.hover, self.draw_bg.down);
        let disabled = self.disabled.then_some(self.disabled_opacity);
        card_surface(self.palette.tokens(), self.appearance, lift, disabled)
    }
}

impl Widget for Card {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let surface = self.surface();
        self.draw_bg.color = surface.fill;
        self.draw_bg.ink = surface.ink;
        self.draw_bg.border_color = surface.border;
        self.draw_bg.border_size = surface.border_size;
        self.draw_bg.border_radius = self.radius as f32;
        self.draw_bg.shadow_color = surface.shadow;
        self.draw_bg.shadow_radius = surface.shadow_radius;
        self.draw_bg.shadow_offset_y = surface.shadow_offset_y;

        // The band's rounding has to be the card's, or the picture squares
        // off the two corners the card has just rounded. Pushed rather than
        // written twice: one radius, one place to change it. A media slot
        // that is not a view of its own carries no such uniform and the
        // write goes nowhere, which is the right answer for a slot that has
        // no corners to round.
        self.media
            .as_view()
            .set_uniform(cx.cx.cx, live_id!(border_radius), &[self.radius as f32]);

        // No padding on the outer turtle. The media has to reach the card's
        // edges, and a container that pads everything cannot have one child
        // that escapes; so the media is laid out here against the bare
        // edge, and the padding a caller wrote is opened below it.
        //
        // Nor any clipping. The shadow is drawn outside the card's rect,
        // and a turtle that clips to that rect cuts it off — which is why
        // every other lifted surface in the library switches clipping off
        // as well.
        self.draw_bg.begin(
            cx,
            walk,
            Layout {
                flow: Flow::Down,
                clip_x: false,
                clip_y: false,
                ..Layout::default()
            },
        );
        cx.widget_tree_insert_child(self.uid, live_id!(media), self.media.clone());
        let media_walk = self.media.walk(cx.cx.cx);
        let _ = self.media.draw_walk(cx, scope, media_walk);

        cx.begin_turtle(Walk::fill_fit(), self.layout);
        for (name, slot) in [
            (live_id!(header), &self.header),
            (live_id!(body), &self.body),
            (live_id!(footer), &self.footer),
        ] {
            // The slots are drawn here rather than by a container, so
            // nothing else puts them in the tree: without this a host could
            // not reach ids!(card.footer) at all.
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
            let slot_walk = slot.walk(cx.cx.cx);
            let _ = slot.draw_walk(cx, scope, slot_walk);
        }
        cx.end_turtle();
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        // The slots first, always — even on a card that takes no presses
        // itself, because a button in the footer is still a button.
        for slot in [&self.media, &self.header, &self.body, &self.footer] {
            slot.handle_event(cx, event, scope);
        }
        if !self.pressable || self.disabled {
            return;
        }
        if let Event::ClearHover = event {
            self.animator_cut(cx, ids!(hover.off));
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Default);
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.animator_play(cx, ids!(hover.down));
                cx.widget_action(uid, CardAction::Pressed);
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if fe.is_over {
                    self.animator_play(cx, ids!(hover.on));
                    if fe.was_tap() {
                        cx.widget_action(uid, CardAction::Clicked);
                    }
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => {}
        }
    }

    /// One `disabled` reaches all four slots, rather than each caller
    /// remembering to set four. A slot honours it if it has a disabled
    /// state; a plain `Label` has none.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            for slot in [&self.media, &self.header, &self.body, &self.footer] {
                slot.set_disabled(cx, disabled);
            }
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }
}

impl CardRef {
    /// Whether the surface was pressed and released this pass.
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<CardAction>(), CardAction::Clicked);
        }
        false
    }

    /// Whether the surface was pressed this pass, on the way down.
    pub fn pressed(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<CardAction>(), CardAction::Pressed);
        }
        false
    }

    /// Dress the card differently at runtime.
    pub fn set_appearance(&self, cx: &mut Cx, appearance: CardAppearance) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.appearance != appearance {
                inner.appearance = appearance;
                inner.draw_bg.redraw(cx);
            }
        }
    }

    /// Whether the whole surface answers a press.
    pub fn set_pressable(&self, cx: &mut Cx, pressable: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.pressable != pressable {
                inner.pressable = pressable;
                inner.draw_bg.redraw(cx);
            }
        }
    }

    /// The surface the card would draw with right now, for a host that
    /// wants to match something else to it.
    pub fn surface(&self) -> CardSurface {
        self.borrow().map(|inner| inner.surface()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(v: f32) -> Vec4f {
        Vec4f { x: v, y: v, z: v, w: 1.0 }
    }

    /// A ladder whose rungs are far enough apart that a wrong rung cannot
    /// pass for the right one.
    ///
    /// Every value is a multiple of an eighth, so the mixing between two
    /// rungs is exact and a test can compare colours outright instead of
    /// within a tolerance that would hide a rung being off by a little.
    fn tokens() -> CardTokens {
        CardTokens {
            elevated: color(0.125),
            filled: color(0.25),
            outlined: color(0.375),
            outline: color(0.5),
            outline_size: 1.0,
            ink: color(0.625),
            shadow_1: CardShadow { color: color(0.75), radius: 10.0, offset_y: 1.0 },
            shadow_2: CardShadow { color: color(0.875), radius: 20.0, offset_y: 2.0 },
            shadow_3: CardShadow { color: color(1.0), radius: 30.0, offset_y: 3.0 },
        }
    }

    /// Level 0 is the ground. A flat card must ask for no blur at all,
    /// because that is what the shader's guard tests to decide whether to
    /// run the gaussian.
    #[test]
    fn the_ground_is_no_shadow_at_all() {
        let rung = tokens().rung(0);
        assert_eq!(rung, CardShadow::default());
        assert_eq!(rung.radius, 0.0);
    }

    /// Only the elevated card is off the ground: a filled card is a tint of
    /// the page and an outlined one is a line drawn on it.
    #[test]
    fn only_the_elevated_card_rests_off_the_ground() {
        let t = tokens();
        let elevated = card_surface(t, CardAppearance::Elevated, 0.0, None);
        let filled = card_surface(t, CardAppearance::Filled, 0.0, None);
        let outlined = card_surface(t, CardAppearance::Outlined, 0.0, None);
        assert_eq!(elevated.shadow_radius, 10.0);
        assert_eq!(filled.shadow_radius, 0.0);
        assert_eq!(outlined.shadow_radius, 0.0);
    }

    /// Three appearances, three fills, and exactly one line. A stroke on a
    /// filled card would make it an outlined card with a different fill,
    /// which is the collapse having three appearances exists to prevent.
    #[test]
    fn only_the_outlined_card_draws_a_line() {
        let t = tokens();
        let elevated = card_surface(t, CardAppearance::Elevated, 0.0, None);
        let filled = card_surface(t, CardAppearance::Filled, 0.0, None);
        let outlined = card_surface(t, CardAppearance::Outlined, 0.0, None);
        assert_eq!(elevated.fill, t.elevated);
        assert_eq!(filled.fill, t.filled);
        assert_eq!(outlined.fill, t.outlined);
        assert_eq!(elevated.border_size, 0.0);
        assert_eq!(filled.border_size, 0.0);
        assert_eq!(outlined.border_size, 1.0);
        assert_eq!(outlined.border, t.outline);
    }

    /// A full lift lands exactly on the next rung of the theme's ladder,
    /// not on a shadow invented for the occasion.
    #[test]
    fn a_lift_lands_on_the_next_rung() {
        let t = tokens();
        let raised = card_surface(t, CardAppearance::Elevated, 1.0, None);
        assert_eq!(raised.shadow_radius, t.shadow_2.radius);
        assert_eq!(raised.shadow_offset_y, t.shadow_2.offset_y);
        assert_eq!(raised.shadow, t.shadow_2.color);

        // A flat card that can be pressed rises from the ground to the
        // first rung, so hovering it means the same thing it means anywhere
        // else: one step up.
        let flat_raised = card_surface(t, CardAppearance::Filled, 1.0, None);
        assert_eq!(flat_raised.shadow_radius, t.shadow_1.radius);
    }

    /// Half a lift is half way between the two rungs, so the shadow moves
    /// with the state layer rather than jumping when it crosses.
    #[test]
    fn a_part_lift_sits_between_the_rungs() {
        let t = tokens();
        let half = card_surface(t, CardAppearance::Elevated, 0.5, None);
        assert_eq!(half.shadow_radius, 15.0);
        assert_eq!(half.shadow_offset_y, 1.5);
    }

    /// The rule the shadow makes visible: a card rises under the pointer
    /// and comes back down under the finger.
    #[test]
    fn a_press_puts_the_card_back_down() {
        assert_eq!(card_lift(0.0, 0.0), 0.0);
        assert_eq!(card_lift(1.0, 0.0), 1.0);
        assert_eq!(card_lift(1.0, 1.0), 0.0, "pressed is back on its own rung");
        // The animator can run the two tracks at different speeds, and a
        // press taken instantly against a hover still easing in briefly
        // gives a down weight above the hover weight.
        assert_eq!(card_lift(0.3, 1.0), 0.0, "never below the resting rung");
        let t = tokens();
        let pressed = card_surface(t, CardAppearance::Elevated, card_lift(1.0, 1.0), None);
        assert_eq!(pressed.shadow_radius, t.shadow_1.radius);
    }

    /// Nothing inert is raised, and nothing inert answers the pointer.
    #[test]
    fn a_disabled_card_lies_flat_however_it_is_hovered() {
        let t = tokens();
        let off = card_surface(t, CardAppearance::Elevated, 1.0, Some(0.5));
        assert_eq!(off.shadow_radius, 0.0);
        assert_eq!(off.shadow, CardShadow::default().color);
        // The fill is untouched: a surface that fades out stops being a
        // surface, and the content standing on it carries the message.
        assert_eq!(off.fill, t.elevated);
        assert_eq!(off.ink.w, t.ink.w * 0.5);
    }

    /// A switched-off outlined card keeps its line and only quietens it —
    /// dropping the line would leave nothing on the page at all.
    #[test]
    fn a_disabled_outlined_card_keeps_its_line() {
        let t = tokens();
        let off = card_surface(t, CardAppearance::Outlined, 0.0, Some(0.25));
        assert_eq!(off.border_size, 1.0);
        assert_eq!(off.border.w, t.outline.w * 0.25);
        assert_eq!(off.border.x, t.outline.x, "quieter, not a different colour");
    }

    /// A lift outside 0..1 cannot walk the card off the end of the ladder.
    #[test]
    fn a_lift_is_clamped_to_the_two_rungs() {
        let t = tokens();
        assert_eq!(
            card_surface(t, CardAppearance::Elevated, 4.0, None).shadow_radius,
            t.shadow_2.radius
        );
        assert_eq!(
            card_surface(t, CardAppearance::Elevated, -4.0, None).shadow_radius,
            t.shadow_1.radius
        );
        assert_eq!(card_lift(4.0, 0.0), 1.0);
    }
}
