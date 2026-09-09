//! Alerts, banners, inline tips and callouts: a message that sits IN the
//! page instead of over it.
//!
//! A toast interrupts and leaves; a dialog blocks. Neither is right for
//! "the upload failed, try again", "you are offline" or "here is how this
//! panel works": those belong next to the thing they talk about and must
//! stay until the reader has read them. This module is that one shape,
//! dressed four ways. The shape is an intent icon, a bold title, a
//! description, an optional action and an optional close cross, on a face
//! whose colour says how urgent it is. The dressings are presets of the
//! same widget: `Alert` for a message inside content, `Banner` for the full
//! width strip under a toolbar, `InlineTip` for guidance that can be sent
//! away for good, `Callout` for the card-shaped nudge with an accent bar.
//!
//! The intent is a PROPERTY, not a preset. The shaders carry the four intent
//! palettes and pick one at draw time, so an app can turn a green "saved"
//! into a red "failed" by setting `intent` and nothing else, and the
//! catalogue can drive it from a control. The appearance works the same way:
//! `Light` tints the face with the intent's container colour, `Filled` paints
//! it solid, `Outline` strokes it and leaves the page showing through.
//!
//! An alert that is closed does not vanish: it FOLDS. The close cross plays
//! the `open` track to `off`, which eases `shown` from one to zero; every
//! draw during that time clamps the height to `shown` times the height the
//! alert had when it was last whole and fades every layer by the same
//! fraction, so the content below slides up instead of jumping. Only when
//! the track has finished does the widget mark itself not visible, which is
//! what a test's `wait_hidden` sees and what a host's layout stops
//! reserving room for. `open` runs the same fold in reverse, which is how a
//! banner host re-shows a banner without a pop.
//!
//! Text reflows. While the title, the description and the actions fit on
//! one line of the width the alert has, they sit on one line with the
//! actions pushed to the far edge; when they do not, the text stacks and the
//! actions drop under it. The decision is measured on every draw from the
//! text's own single-line width and the slots' last drawn width, never from
//! a character count, so a change of font or of window width is reflected
//! on the next frame.
//!
//! The action slots take any widget the app puts there, a `Button` or a
//! `LinkLabel` as a rule. The alert watches for their `Clicked` and raises
//! [`AlertAction::Action`] so a host that only wants to know "the action was
//! taken" can listen in one place; a host that fills both slots reads the
//! buttons directly to tell them apart. A tip with a `dismiss_key` raises
//! [`AlertAction::Dismissed`] with that key when closed; persisting it is
//! the host's job, this widget only promises never to close silently.

use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    button::ButtonAction,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

/// What an alert can raise.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum AlertAction {
    /// The close cross was pressed: the alert is folding away.
    Closed,
    /// The widget in an action slot was clicked.
    Action,
    /// Closed while carrying a `dismiss_key`; the host stores the key so
    /// the tip stays away. Raised after `Closed`.
    Dismissed(String),
    #[default]
    None,
}

/// How urgent the message is; picks the palette.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum AlertIntent {
    #[pick]
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// How the face wears the intent's colour.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum AlertAppearance {
    /// A tinted face in the intent's container colour.
    #[pick]
    #[default]
    Light,
    /// A solid face in the intent colour, text in its on-colour.
    Filled,
    /// A stroke in the intent colour on a see-through face.
    Outline,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawAlertBgBase = #(DrawAlertBg::script_component(vm))
    set_type_default() do #(DrawAlertBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawAlertGlyphBase = #(DrawAlertGlyph::script_component(vm))
    set_type_default() do #(DrawAlertGlyph::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawAlertTextBase = #(DrawAlertText::script_component(vm))
    set_type_default() do #(DrawAlertText::script_shader(vm)){
        ..mod.draw.DrawText
    }

    mod.widgets.AlertIntent = #(AlertIntent::script_api(vm))
    mod.widgets.AlertAppearance = #(AlertAppearance::script_api(vm))

    mod.widgets.AlertBase = #(Alert::register_widget(vm))

    /** The flat alert: an intent icon, a title, a description, an action
     * slot and a close cross on a tinted face. Every other alert shape is
     * this widget with other defaults. */
    mod.widgets.AlertFlat = set_type_default() do mod.widgets.AlertBase{
        width: Fill
        height: Fit
        flow: Right
        /** gap between the icon, the text, the actions and the cross 0..24 step 1 */
        spacing: theme.space_2
        /** the face's inner padding */
        padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2 + 2., bottom: theme.space_2 + 2.}
        /** outer gap to neighbouring widgets */
        margin: 0.
        align: Align{x: 0., y: 0.}

        /** the title: one bold line; `text()` reports it */
        title: ""
        /** the description under, or beside, the title */
        description: ""
        /** longer guidance folded under a Show more link; empty means none */
        guidance: ""
        /** how urgent the message is: Info, Success, Warning or Error */
        intent: mod.widgets.AlertIntent.Info
        /** Light tints the face, Filled paints it solid, Outline strokes it */
        appearance: mod.widgets.AlertAppearance.Light
        /** draw the intent icon before the title */
        show_icon: true
        /** draw a close cross that folds the alert away */
        closable: false
        /** keep title, description and actions on one line while they fit */
        single_line: true
        /** the guidance starts unfolded */
        expanded: false
        /** a key the host persists when this is closed, so it stays away */
        dismiss_key: ""
        /** the fold link's label while the guidance is folded */
        more_text: "Show more"
        /** the fold link's label while the guidance is open */
        less_text: "Show less"
        /** the intent icon's box */
        icon_walk: Walk{width: 16., height: 16.}
        /** the close cross's box */
        close_walk: Walk{width: 12., height: 12., margin: Inset{left: theme.space_1, right: 0., top: 0., bottom: 0.}}
        /** the fold chevron's box */
        more_walk: Walk{width: 10., height: 10., margin: Inset{right: theme.space_1, left: 0., top: 0., bottom: 0.}}
        /** how far the alert is unfolded 0..1 step 0.01; the open track drives it */
        shown: 1.0
        /** how far the alert is dimmed 0..1 step 0.01; the disabled track drives it */
        dim: 0.0
        /** drawn at all; a closed alert clears this when its fold ends */
        visible: true

        /** The face: a rounded box filled and stroked by intent and
         * appearance, with an optional accent bar down its left edge. */
        draw_bg +: {
            /** the Info accent */
            color_info: uniform(theme.color_info)
            /** the Success accent */
            color_success: uniform(theme.color_success)
            /** the Warning accent */
            color_warning: uniform(theme.color_warning)
            /** the Error accent */
            color_error: uniform(theme.color_error)
            /** the Light face for Info */
            color_info_container: uniform(theme.color_info_container)
            /** the Light face for Success */
            color_success_container: uniform(theme.color_success_container)
            /** the Light face for Warning */
            color_warning_container: uniform(theme.color_warning_container)
            /** the Light face for Error */
            color_error_container: uniform(theme.color_error_container)
            /** the face behind an Outline alert; a card colour for a callout */
            color_fill_outline: uniform(theme.color_u_hidden)
            /** the stroke for Light and Filled faces; hidden here, a bevel on Alert */
            border_color: uniform(#0000)
            /** second bevel stop; a negative alpha keeps the stroke flat */
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            /** stroke thickness in pixels 0..4 step 0.5 */
            border_size: uniform(1.0)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.radius_m)
            /** width of the accent bar down the left edge; 0 draws none 0..8 step 0.5 */
            accent_size: uniform(0.0)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            intent_tone: fn() -> vec4 {
                let mut c = self.color_info
                if self.intent > 0.5 { c = self.color_success }
                if self.intent > 1.5 { c = self.color_warning }
                if self.intent > 2.5 { c = self.color_error }
                return c
            }
            intent_container: fn() -> vec4 {
                let mut c = self.color_info_container
                if self.intent > 0.5 { c = self.color_success_container }
                if self.intent > 1.5 { c = self.color_warning_container }
                if self.intent > 2.5 { c = self.color_error_container }
                return c
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let tone = self.intent_tone()
                let mut fill = self.intent_container()
                if self.appearance > 0.5 { fill = tone }
                if self.appearance > 1.5 { fill = self.color_fill_outline }
                let mut stroke = self.border_color
                if self.border_color_2.x > -0.5 {
                    stroke = mix(self.border_color, self.border_color_2, self.pos.y)
                }
                if self.appearance > 1.5 { stroke = tone }
                // A hidden stroke gets no inset either, or the face would
                // stop a pixel short of its box for no reason anyone can see.
                let mut bs = 0.0
                if stroke.w > 0.0 { bs = self.border_size }
                let r = max(1.0, self.border_radius)
                sdf.box(bs, bs, self.rect_size.x - bs * 2.0, self.rect_size.y - bs * 2.0, r)
                sdf.fill_keep(fill)
                if bs > 0.0 { sdf.stroke(stroke, bs) }
                if self.accent_size > 0.0 {
                    sdf.box(bs, bs, self.rect_size.x - bs * 2.0, self.rect_size.y - bs * 2.0, r)
                    sdf.rect(bs, bs, self.accent_size, self.rect_size.y - bs * 2.0)
                    sdf.intersect()
                    sdf.fill(tone)
                }
                return sdf.result * (self.shown * mix(1.0, self.disabled_alpha, self.dim))
            }
        }

        /** The intent icon: a disc, or a diamond for a warning, carrying a
         * mark that says the intent without its colour. Outline alerts get
         * a ring and a mark in the accent instead of a solid disc. */
        draw_icon +: {
            /** the Info accent */
            color_info: uniform(theme.color_info)
            /** the Success accent */
            color_success: uniform(theme.color_success)
            /** the Warning accent */
            color_warning: uniform(theme.color_warning)
            /** the Error accent */
            color_error: uniform(theme.color_error)
            /** the disc on a Filled Info face */
            color_on_info: uniform(theme.color_on_info)
            /** the disc on a Filled Success face */
            color_on_success: uniform(theme.color_on_success)
            /** the disc on a Filled Warning face */
            color_on_warning: uniform(theme.color_on_warning)
            /** the disc on a Filled Error face */
            color_on_error: uniform(theme.color_on_error)
            /** the mark on a Light Info disc */
            color_info_container: uniform(theme.color_info_container)
            /** the mark on a Light Success disc */
            color_success_container: uniform(theme.color_success_container)
            /** the mark on a Light Warning disc */
            color_warning_container: uniform(theme.color_warning_container)
            /** the mark on a Light Error disc */
            color_error_container: uniform(theme.color_error_container)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            intent_tone: fn() -> vec4 {
                let mut c = self.color_info
                if self.intent > 0.5 { c = self.color_success }
                if self.intent > 1.5 { c = self.color_warning }
                if self.intent > 2.5 { c = self.color_error }
                return c
            }
            intent_on_tone: fn() -> vec4 {
                let mut c = self.color_on_info
                if self.intent > 0.5 { c = self.color_on_success }
                if self.intent > 1.5 { c = self.color_on_warning }
                if self.intent > 2.5 { c = self.color_on_error }
                return c
            }
            intent_container: fn() -> vec4 {
                let mut c = self.color_info_container
                if self.intent > 0.5 { c = self.color_success_container }
                if self.intent > 1.5 { c = self.color_warning_container }
                if self.intent > 2.5 { c = self.color_error_container }
                return c
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let tone = self.intent_tone()
                let s = self.rect_size.x
                let c = s * 0.5
                let w = max(1.0, s * 0.1)
                let mut disc = tone
                let mut mark = self.intent_container()
                if self.appearance > 0.5 {
                    disc = self.intent_on_tone()
                    mark = tone
                }
                let mut ring = 0.0
                if self.appearance > 1.5 {
                    ring = 1.0
                    mark = tone
                }
                let mut warning = 0.0
                if self.intent > 1.5 { warning = 1.0 }
                if self.intent > 2.5 { warning = 0.0 }
                if warning > 0.5 {
                    // A diamond: a rounded box turned a quarter turn. The
                    // turn is undone straight after so the mark lands upright.
                    let h = s * 0.34
                    sdf.rotate(0.7853982, c, c)
                    sdf.box(c - h, c - h, h * 2.0, h * 2.0, s * 0.1)
                    sdf.rotate(-0.7853982, c, c)
                } else {
                    sdf.circle(c, c, s * 0.46)
                }
                if ring > 0.5 {
                    sdf.stroke(tone, w)
                } else {
                    sdf.fill(disc)
                }
                if self.intent < 0.5 {
                    sdf.circle(c, s * 0.31, w * 0.7)
                    sdf.fill(mark)
                    sdf.move_to(c, s * 0.46)
                    sdf.line_to(c, s * 0.72)
                    sdf.stroke(mark, w)
                }
                if self.intent > 0.5 {
                    if self.intent < 1.5 {
                        sdf.move_to(s * 0.3, s * 0.52)
                        sdf.line_to(s * 0.44, s * 0.66)
                        sdf.line_to(s * 0.71, s * 0.36)
                        sdf.stroke(mark, w)
                    }
                }
                if warning > 0.5 {
                    sdf.move_to(c, s * 0.3)
                    sdf.line_to(c, s * 0.56)
                    sdf.stroke(mark, w)
                    sdf.circle(c, s * 0.7, w * 0.7)
                    sdf.fill(mark)
                }
                if self.intent > 2.5 {
                    sdf.move_to(s * 0.35, s * 0.35)
                    sdf.line_to(s * 0.65, s * 0.65)
                    sdf.move_to(s * 0.65, s * 0.35)
                    sdf.line_to(s * 0.35, s * 0.65)
                    sdf.stroke(mark, w)
                }
                return sdf.result * (self.shown * mix(1.0, self.disabled_alpha, self.dim))
            }
        }

        /** The close cross, stroked in the text ink and grown under the pointer. */
        draw_close +: {
            /** the ink on a Light Info face */
            color_on_info_container: uniform(theme.color_on_info_container)
            /** the ink on a Light Success face */
            color_on_success_container: uniform(theme.color_on_success_container)
            /** the ink on a Light Warning face */
            color_on_warning_container: uniform(theme.color_on_warning_container)
            /** the ink on a Light Error face */
            color_on_error_container: uniform(theme.color_on_error_container)
            /** the ink on a Filled Info face */
            color_on_info: uniform(theme.color_on_info)
            /** the ink on a Filled Success face */
            color_on_success: uniform(theme.color_on_success)
            /** the ink on a Filled Warning face */
            color_on_warning: uniform(theme.color_on_warning)
            /** the ink on a Filled Error face */
            color_on_error: uniform(theme.color_on_error)
            /** the ink on an Outline face */
            color_text: uniform(theme.color_text)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            intent_ink: fn() -> vec4 {
                let mut c = self.color_on_info_container
                if self.intent > 0.5 { c = self.color_on_success_container }
                if self.intent > 1.5 { c = self.color_on_warning_container }
                if self.intent > 2.5 { c = self.color_on_error_container }
                let mut f = self.color_on_info
                if self.intent > 0.5 { f = self.color_on_success }
                if self.intent > 1.5 { f = self.color_on_warning }
                if self.intent > 2.5 { f = self.color_on_error }
                if self.appearance > 0.5 { c = f }
                if self.appearance > 1.5 { c = self.color_text }
                return c
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let s = self.rect_size.x
                let inset = s * (0.25 - self.hover * 0.06)
                sdf.move_to(inset, inset)
                sdf.line_to(s - inset, s - inset)
                sdf.move_to(s - inset, inset)
                sdf.line_to(inset, s - inset)
                let ink = self.intent_ink()
                let alpha = (0.7 + 0.3 * self.hover) * self.shown * mix(1.0, self.disabled_alpha, self.dim)
                sdf.stroke(vec4(ink.xyz, ink.w * alpha), 1.2)
                return sdf.result
            }
        }

        /** The fold chevron beside the Show more link: down while folded,
         * up while open. */
        draw_more +: {
            /** the ink on a Light Info face */
            color_on_info_container: uniform(theme.color_on_info_container)
            /** the ink on a Light Success face */
            color_on_success_container: uniform(theme.color_on_success_container)
            /** the ink on a Light Warning face */
            color_on_warning_container: uniform(theme.color_on_warning_container)
            /** the ink on a Light Error face */
            color_on_error_container: uniform(theme.color_on_error_container)
            /** the ink on a Filled Info face */
            color_on_info: uniform(theme.color_on_info)
            /** the ink on a Filled Success face */
            color_on_success: uniform(theme.color_on_success)
            /** the ink on a Filled Warning face */
            color_on_warning: uniform(theme.color_on_warning)
            /** the ink on a Filled Error face */
            color_on_error: uniform(theme.color_on_error)
            /** the ink on an Outline face */
            color_text: uniform(theme.color_text)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            intent_ink: fn() -> vec4 {
                let mut c = self.color_on_info_container
                if self.intent > 0.5 { c = self.color_on_success_container }
                if self.intent > 1.5 { c = self.color_on_warning_container }
                if self.intent > 2.5 { c = self.color_on_error_container }
                let mut f = self.color_on_info
                if self.intent > 0.5 { f = self.color_on_success }
                if self.intent > 1.5 { f = self.color_on_warning }
                if self.intent > 2.5 { f = self.color_on_error }
                if self.appearance > 0.5 { c = f }
                if self.appearance > 1.5 { c = self.color_text }
                return c
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let s = self.rect_size.x
                let top = mix(s * 0.35, s * 0.65, self.open)
                let bottom = mix(s * 0.65, s * 0.35, self.open)
                sdf.move_to(s * 0.2, top)
                sdf.line_to(s * 0.5, bottom)
                sdf.line_to(s * 0.8, top)
                let ink = self.intent_ink()
                let alpha = (0.7 + 0.3 * self.hover) * self.shown * mix(1.0, self.disabled_alpha, self.dim)
                sdf.stroke(vec4(ink.xyz, ink.w * alpha), 1.2)
                return sdf.result
            }
        }

        /** The title ink: bold, in the face's on-colour. */
        draw_title +: {
            /** the title typeface */
            text_style: theme.font_bold{
                /** title type size in points 6..32 step 0.5 */
                font_size: theme.font_size_p
            }
            /** the ink on a Light Info face */
            color_on_info_container: uniform(theme.color_on_info_container)
            /** the ink on a Light Success face */
            color_on_success_container: uniform(theme.color_on_success_container)
            /** the ink on a Light Warning face */
            color_on_warning_container: uniform(theme.color_on_warning_container)
            /** the ink on a Light Error face */
            color_on_error_container: uniform(theme.color_on_error_container)
            /** the ink on a Filled Info face */
            color_on_info: uniform(theme.color_on_info)
            /** the ink on a Filled Success face */
            color_on_success: uniform(theme.color_on_success)
            /** the ink on a Filled Warning face */
            color_on_warning: uniform(theme.color_on_warning)
            /** the ink on a Filled Error face */
            color_on_error: uniform(theme.color_on_error)
            /** the ink on an Outline face */
            color_text: uniform(theme.color_text)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            get_color: fn() {
                let mut c = self.color_on_info_container
                if self.intent > 0.5 { c = self.color_on_success_container }
                if self.intent > 1.5 { c = self.color_on_warning_container }
                if self.intent > 2.5 { c = self.color_on_error_container }
                let mut f = self.color_on_info
                if self.intent > 0.5 { f = self.color_on_success }
                if self.intent > 1.5 { f = self.color_on_warning }
                if self.intent > 2.5 { f = self.color_on_error }
                if self.appearance > 0.5 { c = f }
                if self.appearance > 1.5 { c = self.color_text }
                return vec4(c.xyz, c.w * self.shown * mix(1.0, self.disabled_alpha, self.dim))
            }
        }

        /** The description ink: regular weight, the same on-colour as the title. */
        draw_text +: {
            /** the description typeface */
            text_style: theme.font_regular{
                /** description type size in points 6..32 step 0.5 */
                font_size: theme.font_size_p
            }
            /** the ink on a Light Info face */
            color_on_info_container: uniform(theme.color_on_info_container)
            /** the ink on a Light Success face */
            color_on_success_container: uniform(theme.color_on_success_container)
            /** the ink on a Light Warning face */
            color_on_warning_container: uniform(theme.color_on_warning_container)
            /** the ink on a Light Error face */
            color_on_error_container: uniform(theme.color_on_error_container)
            /** the ink on a Filled Info face */
            color_on_info: uniform(theme.color_on_info)
            /** the ink on a Filled Success face */
            color_on_success: uniform(theme.color_on_success)
            /** the ink on a Filled Warning face */
            color_on_warning: uniform(theme.color_on_warning)
            /** the ink on a Filled Error face */
            color_on_error: uniform(theme.color_on_error)
            /** the ink on an Outline face */
            color_text: uniform(theme.color_text)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            get_color: fn() {
                let mut c = self.color_on_info_container
                if self.intent > 0.5 { c = self.color_on_success_container }
                if self.intent > 1.5 { c = self.color_on_warning_container }
                if self.intent > 2.5 { c = self.color_on_error_container }
                let mut f = self.color_on_info
                if self.intent > 0.5 { f = self.color_on_success }
                if self.intent > 1.5 { f = self.color_on_warning }
                if self.intent > 2.5 { f = self.color_on_error }
                if self.appearance > 0.5 { c = f }
                if self.appearance > 1.5 { c = self.color_text }
                return vec4(c.xyz, c.w * self.shown * mix(1.0, self.disabled_alpha, self.dim))
            }
        }

        /** The Show more link's ink: a small bold label in the on-colour,
         * brighter under the pointer. */
        draw_link +: {
            /** the link typeface */
            text_style: theme.font_bold{
                /** link type size in points 6..32 step 0.5 */
                font_size: theme.type_label_m_size
            }
            /** the ink on a Light Info face */
            color_on_info_container: uniform(theme.color_on_info_container)
            /** the ink on a Light Success face */
            color_on_success_container: uniform(theme.color_on_success_container)
            /** the ink on a Light Warning face */
            color_on_warning_container: uniform(theme.color_on_warning_container)
            /** the ink on a Light Error face */
            color_on_error_container: uniform(theme.color_on_error_container)
            /** the ink on a Filled Info face */
            color_on_info: uniform(theme.color_on_info)
            /** the ink on a Filled Success face */
            color_on_success: uniform(theme.color_on_success)
            /** the ink on a Filled Warning face */
            color_on_warning: uniform(theme.color_on_warning)
            /** the ink on a Filled Error face */
            color_on_error: uniform(theme.color_on_error)
            /** the ink on an Outline face */
            color_text: uniform(theme.color_text)
            /** the alpha everything settles to while disabled 0..1 step 0.01 */
            disabled_alpha: uniform(theme.state_disabled_content_opacity)

            get_color: fn() {
                let mut c = self.color_on_info_container
                if self.intent > 0.5 { c = self.color_on_success_container }
                if self.intent > 1.5 { c = self.color_on_warning_container }
                if self.intent > 2.5 { c = self.color_on_error_container }
                let mut f = self.color_on_info
                if self.intent > 0.5 { f = self.color_on_success }
                if self.intent > 1.5 { f = self.color_on_warning }
                if self.intent > 2.5 { f = self.color_on_error }
                if self.appearance > 0.5 { c = f }
                if self.appearance > 1.5 { c = self.color_text }
                return vec4(c.xyz, c.w * self.shown * mix(1.0, self.disabled_alpha, self.dim))
            }
        }

        /** the state machine folding the alert and dimming it */
        animator: Animator{
            /** open track: eases `shown`, which folds the height and fades every layer */
            open: {
                default: @on
                /** folding away: shown eases to 0 with the standard accelerate curve */
                off: AnimatorState{
                    from: {all: Forward{duration: theme.motion_medium_1}}
                    ease: theme.motion_ease_standard_accelerate
                    redraw: true
                    apply: {
                        shown: 0.0
                    }
                }
                /** unfolding: shown eases to 1 with the standard decelerate curve */
                on: AnimatorState{
                    from: {all: Forward{duration: theme.motion_medium_1}}
                    ease: theme.motion_ease_standard_decelerate
                    redraw: true
                    apply: {
                        shown: 1.0
                    }
                }
            }
            /** disabled track: eases `dim`, which fades every layer to the disabled alpha */
            disabled: {
                default: @off
                /** enabled: dim back to 0 */
                off: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_2}}
                    redraw: true
                    apply: {
                        dim: 0.0
                    }
                }
                /** disabled: dim to 1 */
                on: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_4}}
                    redraw: true
                    apply: {
                        dim: 1.0
                    }
                }
            }
        }
    }

    /** The standard alert: the flat face with the theme's bevel ramp as its stroke. */
    mod.widgets.Alert = mod.widgets.AlertFlat{
        draw_bg +: {
            border_color: theme.color_bevel_outset_1
            border_color_2: theme.color_bevel_outset_2
        }
    }

    /** The banner: a full-width strip with square corners that stays until
     * one of its actions is taken; it has no close cross by default. */
    mod.widgets.Banner = mod.widgets.AlertFlat{
        width: Fill
        closable: false
        padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_3, bottom: theme.space_3}
        draw_bg +: {
            border_radius: 0.0
        }
    }

    /** The inline tip: a guide banner with folded guidance and a media slot,
     * closable, on a low surface with the intent's stroke. */
    mod.widgets.InlineTip = mod.widgets.AlertFlat{
        appearance: mod.widgets.AlertAppearance.Outline
        closable: true
        single_line: false
        padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_3, bottom: theme.space_3}
        draw_bg +: {
            color_fill_outline: theme.color_surface_container_low
        }
    }

    /** The callout: the card nudge, an accent bar down the left of a raised
     * card, a title, an action and a close cross. */
    mod.widgets.Callout = mod.widgets.AlertFlat{
        appearance: mod.widgets.AlertAppearance.Outline
        closable: true
        show_icon: false
        single_line: false
        padding: Inset{left: theme.space_3 + 4., right: theme.space_3, top: theme.space_3, bottom: theme.space_3}
        draw_bg +: {
            color_fill_outline: theme.color_surface_container_high
            border_size: 0.0
            accent_size: 3.0
        }
    }

    mod.widgets.BannerHostBase = #(BannerHost::register_widget(vm))

    /** A place under a toolbar for the one banner that matters now. It holds
     * one Banner, hidden until `show` fills and unfolds it; a second `show`
     * replaces the text, so only the latest is ever on screen. */
    mod.widgets.BannerHost = set_type_default() do mod.widgets.BannerHostBase{
        width: Fill
        height: Fit
        flow: Down
        /** the banner the host fills; style it here */
        banner: mod.widgets.Banner{
            visible: false
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAlertBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    intent: f32,
    #[live]
    appearance: f32,
    #[live]
    shown: f32,
    #[live]
    dim: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAlertGlyph {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    intent: f32,
    #[live]
    appearance: f32,
    #[live]
    shown: f32,
    #[live]
    dim: f32,
    #[live]
    hover: f32,
    #[live]
    open: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAlertText {
    #[deref]
    draw_super: DrawText,
    #[live]
    intent: f32,
    #[live]
    appearance: f32,
    #[live]
    shown: f32,
    #[live]
    dim: f32,
}

/// The message-in-the-page widget behind Alert, Banner, InlineTip and Callout.
#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister, Animator)]
pub struct Alert {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The face; its area is the widget's area and its draw list the one
    /// every redraw goes through (a hand-written `WidgetNode` names it, the
    /// derive's `#[redraw]` being for structs the derive builds).
    #[live]
    draw_bg: DrawAlertBg,
    #[live]
    draw_icon: DrawAlertGlyph,
    #[live]
    draw_close: DrawAlertGlyph,
    #[live]
    draw_more: DrawAlertGlyph,
    #[live]
    draw_title: DrawAlertText,
    #[live]
    draw_text: DrawAlertText,
    #[live]
    draw_link: DrawAlertText,

    /// The action slot: a Button or a LinkLabel the app puts here.
    #[live]
    action: WidgetRef,
    /// A second action, for a banner that offers two.
    #[live]
    secondary: WidgetRef,
    /// A picture beside the text, for a tip.
    #[live]
    media: WidgetRef,

    #[live]
    icon_walk: Walk,
    #[live]
    close_walk: Walk,
    #[live]
    more_walk: Walk,

    #[live]
    pub title: String,
    #[live]
    pub description: String,
    #[live]
    pub guidance: String,
    #[live]
    pub intent: AlertIntent,
    #[live]
    pub appearance: AlertAppearance,
    #[live(true)]
    pub show_icon: bool,
    #[live]
    pub closable: bool,
    #[live(true)]
    pub single_line: bool,
    #[live]
    pub expanded: bool,
    #[live]
    pub dismiss_key: String,
    #[live]
    more_text: String,
    #[live]
    less_text: String,

    /// 1 unfolded, 0 folded away; the open track eases it.
    #[live(1.0)]
    shown: f64,
    /// 1 dimmed, 0 at full strength; the disabled track eases it.
    #[live]
    dim: f64,
    #[live(true)]
    visible: bool,

    /// The height of the last WHOLE draw, in layout points: what the fold
    /// scales. Zero until the alert has been drawn unfolded once.
    #[rust]
    full_height: f64,
    /// The close cross was pressed and the fold is running; when the open
    /// track stops, the alert hides.
    #[rust]
    closing: bool,
    #[rust]
    close_hover: bool,
    #[rust]
    more_hover: bool,
    /// The row layout the last draw settled on, so a hit test on the fold
    /// link can trust the link's area was drawn this frame.
    #[rust]
    has_link: bool,

    #[rust]
    action_data: WidgetActionData,
}

fn fixed(size: Size) -> f64 {
    match size {
        Size::Fixed(v) => v,
        _ => 0.0,
    }
}

fn outer_width(walk: &Walk) -> f64 {
    fixed(walk.width) + walk.margin.left + walk.margin.right
}

impl Alert {
    fn intent_index(&self) -> f32 {
        match self.intent {
            AlertIntent::Info => 0.0,
            AlertIntent::Success => 1.0,
            AlertIntent::Warning => 2.0,
            AlertIntent::Error => 3.0,
        }
    }

    fn appearance_index(&self) -> f32 {
        match self.appearance {
            AlertAppearance::Light => 0.0,
            AlertAppearance::Filled => 1.0,
            AlertAppearance::Outline => 2.0,
        }
    }

    /// Every layer reads the same four numbers; they are pushed once per
    /// draw so a layer can never disagree with the widget about its intent.
    fn push_shader_state(&mut self) {
        let intent = self.intent_index();
        let appearance = self.appearance_index();
        let shown = self.shown.clamp(0.0, 1.0) as f32;
        let dim = self.dim.clamp(0.0, 1.0) as f32;
        for glyph in [&mut self.draw_icon, &mut self.draw_close, &mut self.draw_more] {
            glyph.intent = intent;
            glyph.appearance = appearance;
            glyph.shown = shown;
            glyph.dim = dim;
        }
        self.draw_close.hover = if self.close_hover { 1.0 } else { 0.0 };
        self.draw_more.hover = if self.more_hover { 1.0 } else { 0.0 };
        self.draw_more.open = if self.expanded { 1.0 } else { 0.0 };
        for text in [&mut self.draw_title, &mut self.draw_text, &mut self.draw_link] {
            text.intent = intent;
            text.appearance = appearance;
            text.shown = shown;
            text.dim = dim;
        }
        self.draw_bg.intent = intent;
        self.draw_bg.appearance = appearance;
        self.draw_bg.shown = shown;
        self.draw_bg.dim = dim;
    }

    /// The width one line of this text takes in this ink, measured by the
    /// layouter rather than guessed from a character count.
    fn measure(cx: &mut Cx2d, draw: &DrawAlertText, text: &str) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        draw.prepare_single_line_run(cx, text)
            .map(|run| run.width_in_lpxs as f64)
            .unwrap_or(0.0)
    }

    /// The width a slot took the last time it was drawn, plus the gap before
    /// it; nothing for an empty or hidden slot.
    fn slot_width(cx: &Cx, slot: &WidgetRef, spacing: f64) -> f64 {
        if slot.is_empty() || !slot.visible() {
            return 0.0;
        }
        let w = slot.area().rect(cx).size.x;
        if w > 0.0 {
            w + spacing
        } else {
            0.0
        }
    }

    /// Whether the guidance fold link is drawn at all.
    fn shows_link(&self) -> bool {
        !self.guidance.is_empty()
    }

    /// Settle the fold when nothing is animating it: an ease can end shy of
    /// its keyframe and a lost frame can freeze a fraction, so a settled
    /// track's STATE is the truth, not the last number it wrote.
    fn settle(&mut self, cx: &Cx) {
        if !self.animator.is_track_animating(live_id!(open))
            && self.animator.groups.get(&live_id!(open)).is_some()
        {
            self.shown = if self.animator_in_state(cx, ids!(open.on)) {
                1.0
            } else {
                0.0
            };
        }
    }

    /// Unfold, or bring back an alert that was closed. The fold runs from
    /// zero so a returning alert grows into place rather than popping.
    pub fn open(&mut self, cx: &mut Cx) {
        self.closing = false;
        let was_visible = self.visible;
        self.visible = true;
        if !was_visible || !self.animator_in_state(cx, ids!(open.on)) {
            self.animator_cut(cx, ids!(open.off));
            self.animator_play(cx, ids!(open.on));
        }
        // A never-drawn alert has no area to redraw through; the parent has
        // to lay it out afresh, so ask for everything.
        if matches!(self.draw_bg.area(), Area::Empty) {
            cx.redraw_all();
        } else {
            self.draw_bg.redraw(cx);
        }
    }

    /// Fold away. Raises `Closed`, and `Dismissed(key)` when a dismiss key is set.
    pub fn close(&mut self, cx: &mut Cx) {
        if !self.visible || self.closing {
            return;
        }
        self.closing = true;
        self.animator_play(cx, ids!(open.off));
        let uid = self.widget_uid();
        cx.widget_action_with_data(&self.action_data, uid, AlertAction::Closed);
        if !self.dismiss_key.is_empty() {
            cx.widget_action_with_data(
                &self.action_data,
                uid,
                AlertAction::Dismissed(self.dismiss_key.clone()),
            );
        }
        self.draw_bg.redraw(cx);
    }

    /// Visible and not on its way out.
    pub fn is_open(&self) -> bool {
        self.visible && !self.closing
    }

    pub fn set_title(&mut self, cx: &mut Cx, title: &str) {
        if self.title != title {
            self.title = title.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_description(&mut self, cx: &mut Cx, description: &str) {
        if self.description != description {
            self.description = description.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_intent(&mut self, cx: &mut Cx, intent: AlertIntent) {
        if self.intent != intent {
            self.intent = intent;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_appearance(&mut self, cx: &mut Cx, appearance: AlertAppearance) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.draw_bg.redraw(cx);
        }
    }

    /// Unfold or fold the guidance under the description.
    pub fn set_expanded(&mut self, cx: &mut Cx, expanded: bool) {
        if self.expanded != expanded {
            self.expanded = expanded;
            self.draw_bg.redraw(cx);
        }
    }

    fn draw_wrapped(cx: &mut Cx2d, draw: &mut DrawAlertText, text: &str) {
        // The layouter only wraps inside a wrapping row flow, the way Label
        // arranges it; a Fill walk gives it the column's width to wrap to.
        cx.begin_turtle(
            Walk::fill_fit(),
            Layout {
                flow: Flow::right_wrap(),
                ..Layout::default()
            },
        );
        draw.draw_walk(cx, Walk::fill_fit(), Align::default(), text);
        cx.end_turtle();
    }

    fn draw_slots(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        for slot in [&self.action, &self.secondary] {
            if !slot.is_empty() && slot.visible() {
                let walk = slot.walk(cx);
                slot.draw_walk_all(cx, scope, walk);
            }
        }
    }
}

impl WidgetNode for Alert {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.draw_bg.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    /// The slots are children under their own names, so `ids!(action)`
    /// reaches the button an app put there. (A `#[find]` field would hand
    /// out the button's children and hide the button itself.)
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, slot) in [
            (live_id!(action), &self.action),
            (live_id!(secondary), &self.secondary),
            (live_id!(media), &self.media),
        ] {
            if !slot.is_empty() {
                visit(id, slot.clone());
            }
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for slot in [&self.action, &self.secondary, &self.media] {
            slot.find_widgets_from_point(cx, point, found);
        }
    }

    fn layer_areas(&self) -> Vec<(&'static str, Area)> {
        vec![
            ("draw_bg", self.draw_bg.area()),
            ("draw_icon", self.draw_icon.area()),
            ("draw_close", self.draw_close.area()),
            ("draw_more", self.draw_more.area()),
            ("draw_title", self.draw_title.area()),
            ("draw_text", self.draw_text.area()),
            ("draw_link", self.draw_link.area()),
        ]
    }

    fn set_action_data(&mut self, action_data: std::sync::Arc<dyn ActionTrait>) {
        self.action_data.set_box(action_data)
    }

    fn action_data(&self) -> Option<std::sync::Arc<dyn ActionTrait>> {
        self.action_data.clone_data()
    }

    fn visible(&self) -> bool {
        self.visible
    }

    /// Showing unfolds through `open`; hiding is immediate, for a host that
    /// wants the alert gone without the fold.
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if visible {
            self.open(cx);
        } else if self.visible {
            self.visible = false;
            self.closing = false;
            self.animator_cut(cx, ids!(open.off));
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for Alert {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.settle(cx);
        if self.closing && self.shown <= 0.0 {
            self.visible = false;
            self.closing = false;
            return DrawStep::done();
        }
        let folding = self.shown < 1.0 && self.full_height > 0.0;
        let mut walk = walk;
        let mut layout = self.layout;
        if folding {
            walk.height = Size::Fixed((self.full_height * self.shown).max(0.0));
            layout.clip_y = true;
        }
        self.push_shader_state();

        let spacing = layout.spacing;
        let outer = cx.turtle().next_walk_width(walk.width, walk.margin);
        let inner = outer - layout.padding.left - layout.padding.right;
        let icon_w = if self.show_icon {
            outer_width(&self.icon_walk) + spacing
        } else {
            0.0
        };
        let close_w = if self.closable {
            outer_width(&self.close_walk) + spacing
        } else {
            0.0
        };
        let actions_w = Self::slot_width(cx, &self.action, spacing)
            + Self::slot_width(cx, &self.secondary, spacing);
        let title_w = Self::measure(cx, &self.draw_title, &self.title);
        let text_w = Self::measure(cx, &self.draw_text, &self.description);
        let text_gap = if title_w > 0.0 && text_w > 0.0 {
            spacing
        } else {
            0.0
        };
        let plain = self.single_line && !self.shows_link() && self.media.is_empty();
        // A Fit-width alert has no width to reflow against: one line it is.
        let inline = plain
            && (inner.is_nan()
                || icon_w + title_w + text_gap + text_w + actions_w + close_w <= inner);
        layout.align = Align {
            x: 0.0,
            y: if inline { 0.5 } else { 0.0 },
        };
        self.has_link = false;

        self.draw_bg.begin(cx, walk, layout);
        if self.show_icon {
            self.draw_icon.draw_walk(cx, self.icon_walk);
        }
        if !self.media.is_empty() && self.media.visible() {
            let media_walk = self.media.walk(cx);
            self.media.draw_walk_all(cx, scope, media_walk);
        }
        if inline {
            if !self.title.is_empty() {
                self.draw_title
                    .draw_walk(cx, Walk::fit(), Align::default(), &self.title);
            }
            if !self.description.is_empty() {
                self.draw_text
                    .draw_walk(cx, Walk::fit(), Align::default(), &self.description);
            }
            // Push the actions and the cross to the far edge. The spacer is
            // a deferred fill: it takes whatever the row leaves over, and
            // has to be resolved before the row ends or the turtle cannot
            // place what came after it.
            let mut spacer = if inner.is_nan() {
                None
            } else {
                cx.defer_walk_turtle(Walk {
                    width: Size::fill(),
                    height: Size::Fixed(0.0),
                    ..Walk::default()
                })
            };
            self.draw_slots(cx, scope);
            if self.closable {
                self.draw_close.draw_walk(cx, self.close_walk);
            }
            if let Some(spacer) = spacer.as_mut() {
                let _ = spacer.resolve(cx);
            }
        } else {
            // The text column takes what is left after the icon and the
            // cross, worked out here rather than asked for as Fill: a Fill
            // turtle in a row takes the whole row and the cross would land
            // past the edge.
            let column_w = if inner.is_nan() {
                Size::fit()
            } else {
                let media_w = if self.media.is_empty() || !self.media.visible() {
                    0.0
                } else {
                    let w = self.media.area().rect(cx).size.x;
                    if w > 0.0 {
                        w + spacing
                    } else {
                        0.0
                    }
                };
                Size::Fixed((inner - icon_w - media_w - close_w).max(1.0))
            };
            cx.begin_turtle(
                Walk {
                    width: column_w,
                    height: Size::fit(),
                    ..Walk::default()
                },
                Layout {
                    flow: Flow::Down,
                    spacing: layout.padding.top.min(spacing),
                    ..Layout::default()
                },
            );
            if !self.title.is_empty() {
                Self::draw_wrapped(cx, &mut self.draw_title, &self.title);
            }
            if !self.description.is_empty() {
                Self::draw_wrapped(cx, &mut self.draw_text, &self.description);
            }
            if self.shows_link() {
                if self.expanded {
                    Self::draw_wrapped(cx, &mut self.draw_text, &self.guidance);
                }
                cx.begin_turtle(
                    Walk::fill_fit(),
                    Layout {
                        align: Align { x: 0.0, y: 0.5 },
                        ..Layout::flow_right()
                    },
                );
                self.draw_more.draw_walk(cx, self.more_walk);
                let label = if self.expanded {
                    &self.less_text
                } else {
                    &self.more_text
                };
                self.draw_link
                    .draw_walk(cx, Walk::fit(), Align::default(), label);
                cx.end_turtle();
                self.has_link = true;
            }
            if !self.action.is_empty() || !self.secondary.is_empty() {
                cx.begin_turtle(
                    Walk::fill_fit(),
                    Layout {
                        spacing,
                        align: Align { x: 0.0, y: 0.5 },
                        ..Layout::flow_right()
                    },
                );
                self.draw_slots(cx, scope);
                cx.end_turtle();
            }
            cx.end_turtle();
            if self.closable {
                self.draw_close.draw_walk(cx, self.close_walk);
            }
        }
        self.draw_bg.end(cx);
        // The height of a whole draw is what the fold scales. It is read
        // here, straight after the face has closed its turtle, because at
        // the top of a draw the face's area still points into the draw list
        // this frame has just cleared and reports nothing.
        if !folding {
            let height = self.draw_bg.area().rect(cx).size.y;
            if height > 0.0 {
                self.full_height = height;
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        // The fold has run its course: the alert is not there any more.
        if self.closing && !self.animator.is_track_animating(live_id!(open)) {
            self.closing = false;
            self.visible = false;
            self.draw_bg.redraw(cx);
        }
        if !self.visible {
            return;
        }
        for slot in [&self.action, &self.secondary, &self.media] {
            if !slot.is_empty() {
                slot.handle_event(cx, event, scope);
            }
        }
        let uid = self.widget_uid();
        if let Event::Actions(actions) = event {
            for slot in [&self.action, &self.secondary] {
                if slot.is_empty() {
                    continue;
                }
                if let Some(action) = actions.find_widget_action(slot.widget_uid()) {
                    if let ButtonAction::Clicked(_) = action.cast() {
                        cx.widget_action_with_data(&self.action_data, uid, AlertAction::Action);
                    }
                }
            }
        }
        if self.closable && !self.closing {
            match event.hits(cx, self.draw_close.area()) {
                Hit::FingerHoverIn(_) => {
                    self.close_hover = true;
                    cx.set_cursor(MouseCursor::Hand);
                    self.draw_bg.redraw(cx);
                }
                Hit::FingerHoverOut(_) => {
                    self.close_hover = false;
                    self.draw_bg.redraw(cx);
                }
                Hit::FingerDown(fe) if fe.is_primary_hit() => {
                    self.close_hover = false;
                    self.close(cx);
                }
                _ => {}
            }
        }
        if self.has_link {
            match event.hits(cx, self.draw_link.area()) {
                Hit::FingerHoverIn(_) => {
                    self.more_hover = true;
                    cx.set_cursor(MouseCursor::Hand);
                    self.draw_bg.redraw(cx);
                }
                Hit::FingerHoverOut(_) => {
                    self.more_hover = false;
                    self.draw_bg.redraw(cx);
                }
                Hit::FingerDown(fe) if fe.is_primary_hit() => {
                    let expanded = !self.expanded;
                    self.set_expanded(cx, expanded);
                }
                _ => {}
            }
        }
    }

    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_title(cx, v);
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
        for slot in [&self.action, &self.secondary] {
            if !slot.is_empty() {
                slot.set_disabled(cx, disabled);
            }
        }
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }
}

impl AlertRef {
    /// True when the close cross was pressed this pass.
    pub fn closed(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .is_some_and(|a| matches!(a.cast(), AlertAction::Closed))
    }

    /// True when a slot's action was clicked this pass.
    pub fn action(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions(self.widget_uid())
            .any(|a| matches!(a.cast(), AlertAction::Action))
    }

    /// The dismiss key, when this pass closed a tip that carries one.
    pub fn dismissed(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions(self.widget_uid()) {
            if let AlertAction::Dismissed(key) = action.cast() {
                return Some(key);
            }
        }
        None
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

    pub fn is_open(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_open())
    }

    pub fn set_title(&self, cx: &mut Cx, title: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_title(cx, title);
        }
    }

    pub fn set_description(&self, cx: &mut Cx, description: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_description(cx, description);
        }
    }

    pub fn set_intent(&self, cx: &mut Cx, intent: AlertIntent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_intent(cx, intent);
        }
    }

    pub fn set_appearance(&self, cx: &mut Cx, appearance: AlertAppearance) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_appearance(cx, appearance);
        }
    }

    pub fn set_expanded(&self, cx: &mut Cx, expanded: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_expanded(cx, expanded);
        }
    }
}

/// The one-at-a-time home for a banner.
#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct BannerHost {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    area: Area,
    /// The banner the host fills and shows; a `Banner` by default.
    #[live]
    banner: WidgetRef,
}

impl BannerHost {
    /// Put this message in the banner and unfold it. A banner already on
    /// screen takes the new text in place: only the latest is ever shown.
    pub fn show(&mut self, cx: &mut Cx, intent: AlertIntent, title: &str, description: &str) {
        if let Some(mut banner) = self.banner.borrow_mut::<Alert>() {
            banner.set_intent(cx, intent);
            banner.set_title(cx, title);
            banner.set_description(cx, description);
            banner.open(cx);
        }
        self.area.redraw(cx);
    }

    /// Fold the banner away.
    pub fn dismiss(&mut self, cx: &mut Cx) {
        if let Some(mut banner) = self.banner.borrow_mut::<Alert>() {
            banner.close(cx);
        }
    }

    pub fn is_showing(&self) -> bool {
        self.banner
            .borrow::<Alert>()
            .is_some_and(|banner| banner.is_open())
    }
}

impl WidgetNode for BannerHost {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if !self.banner.is_empty() {
            visit(live_id!(banner), self.banner.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.banner.find_widgets_from_point(cx, point, found);
    }
}

impl Widget for BannerHost {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        if !self.banner.is_empty() && self.banner.visible() {
            let banner_walk = self.banner.walk(cx);
            self.banner.draw_walk_all(cx, scope, banner_walk);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.banner.handle_event(cx, event, scope);
    }

    fn text(&self) -> String {
        self.banner.text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.banner.set_text(cx, v);
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.banner.set_disabled(cx, disabled);
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.banner.disabled(cx)
    }
}

impl BannerHostRef {
    pub fn show(&self, cx: &mut Cx, intent: AlertIntent, title: &str, description: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show(cx, intent, title, description);
        }
    }

    pub fn dismiss(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.dismiss(cx);
        }
    }

    pub fn is_showing(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_showing())
    }

    /// The banner inside, for a host that wants to read its actions.
    pub fn banner(&self) -> AlertRef {
        match self.borrow() {
            Some(inner) => inner.banner.as_alert(),
            None => WidgetRef::empty().as_alert(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn alert_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let alert = include_str!("alert.rs");
        assert!(lib.contains("pub mod alert;"));
        assert!(lib.contains("alert::*"));
        assert!(lib.contains("crate::alert::script_mod(vm);"));
        // The action slots default to a Button or a LinkLabel, and the host
        // template names a Banner, so those must be registered first.
        let button = lib.find("crate::button::script_mod(vm);").unwrap();
        let link = lib.find("crate::link_label::script_mod(vm);").unwrap();
        let here = lib.find("crate::alert::script_mod(vm);").unwrap();
        assert!(button < here && link < here);
        // The needles are assembled at run time so this test's own text,
        // which `include_str!` brings along, cannot satisfy them.
        let base = format!("mod.widgets.{} = #({}::register_widget(vm))", "AlertBase", "Alert");
        let flat = format!("mod.widgets.{} = set_type_default() do mod.widgets.{}", "AlertFlat", "AlertBase");
        let host = format!("mod.widgets.{} = #({}::register_widget(vm))", "BannerHostBase", "BannerHost");
        assert!(alert.contains(&base));
        assert!(alert.contains(&flat));
        assert!(alert.contains(&host));
        let one = format!("set_type_default() do mod.widgets.{}", "AlertBase");
        assert_eq!(alert.matches(&one).count(), 1, "one type default per widget");
    }
}
