//! The named text presets: one body scale, spoken by name instead of by
//! number.
//!
//! An app that wants slightly smaller text should not have to know a
//! number to get it. Every preset here is a label wearing a text style from
//! the theme's type scale, so `TextSmall` means "one rung below body" in
//! whichever theme is running, and a scale that is retuned once retunes
//! every screen written in these. A size written by hand into application
//! code is a decision taken once, by one person, in a place nobody looks at
//! again.
//!
//! `Text`, `TextSmall` and `TextLarge` are the three body rungs.
//! `TextStrong` is the same size in the bold face, `TextMuted` the same
//! size in the quieter of the two colours that read on a surface, and
//! `TextCode` the monospaced face for an identifier or a value quoted
//! inside a sentence.
//!
//! All six measure to their ink in both directions, so a preset can sit in
//! a row next to anything. They deliberately do NOT wrap. Copy that has to
//! wrap is `P` and `TextBox`, which fill their parent's width, and a Fill
//! inside a Fit resolves to nothing and paints nothing — so the wrapping
//! family and the inline family are kept apart rather than merged into one
//! preset that would be wrong half the time.
//!
//! The heading ladder `H1`..`H6` is deliberately NOT declared here. It
//! already ships with the label, and a second declaration of the same name
//! is not an error: whichever module registered last would quietly win, and
//! which one that is depends on the order of a list in `lib.rs`.
use crate::makepad_platform::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    /** Body text at the scale's own size: what the bulk of a screen is written in. */
    mod.widgets.Text = mod.widgets.Label{
        width: Fit
        height: Fit
        // A run of text carries no padding of its own. The space around a
        // run belongs to whatever laid the run out, and a preset that
        // brought its own would push every row it appears in out of line
        // with the rows that used a bare label.
        padding: 0.
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_body_m
        }
        text: "Text"
    }

    /** One rung down: captions, units, timestamps, the second line of a pair. */
    mod.widgets.TextSmall = mod.widgets.Text{
        draw_text +: {
            text_style: theme.font_body_s
        }
        text: "TextSmall"
    }

    /** One rung up: the lead sentence, or the one line on a screen that has to be read first. */
    mod.widgets.TextLarge = mod.widgets.Text{
        draw_text +: {
            text_style: theme.font_body_l
        }
        text: "TextLarge"
    }

    /** Body text in the quieter of the two colours that read on a surface: a hint, an aside, the half of a row that is context rather than content. */
    mod.widgets.TextMuted = mod.widgets.Text{
        draw_text +: {
            color: theme.color_on_surface_variant
        }
        text: "TextMuted"
    }

    /** Body text in the bold face at body size, for the word in a sentence that carries the fact. */
    mod.widgets.TextStrong = mod.widgets.Text{
        // The theme's label rung is the bold face at exactly the body
        // size, so taking it whole means emphasis is a change of weight
        // and never a change of size, with no number written here. Its
        // line spacing is the tighter of the two, so a strong run measures
        // a little shorter than the body run beside it; rows centre on the
        // cross axis, which is where that would otherwise show.
        draw_text +: {
            color: theme.color_text_hl
            text_style: theme.font_label_l
        }
        text: "TextStrong"
    }

    /** The monospaced face, for an identifier, a path or a value quoted inside a sentence. A face and not a chip: nothing is drawn behind it. */
    mod.widgets.TextCode = mod.widgets.Text{
        draw_text +: {
            text_style: theme.font_code
        }
        text: "TextCode"
    }
}

#[cfg(test)]
mod tests {
    /// The whole point of the family is that the sizes live in the theme.
    /// A size written here would be silent: the text still draws, at a
    /// size no theme can move, and nobody notices until two screens
    /// disagree. The needle is split so this test does not match itself.
    #[test]
    fn no_preset_writes_a_size_of_its_own() {
        let src = include_str!("typography.rs");
        let needle = concat!("font_", "size:");
        assert!(!src.contains(needle), "a preset set a size instead of naming a rung of the scale");
    }

    /// The heading ladder belongs to the label module. Declaring any of it
    /// again here would not fail to build and would not warn — the module
    /// that registers last simply wins.
    #[test]
    fn the_heading_ladder_is_left_where_it_is() {
        let src = include_str!("typography.rs");
        let label = include_str!("label.rs");
        for heading in ["H1", "H2", "H3", "H4", "H5", "H6"] {
            let decl = format!("mod.widgets.{heading} = ");
            assert!(label.contains(&decl), "the label module no longer declares {heading}");
            assert!(!src.contains(&decl), "this module must not shadow {heading}");
        }
    }

    /// One declaration per name, and every name in the family present.
    #[test]
    fn each_preset_is_declared_exactly_once() {
        let src = include_str!("typography.rs");
        for name in ["Text", "TextSmall", "TextLarge", "TextMuted", "TextStrong", "TextCode"] {
            let decl = format!("mod.widgets.{name} = ");
            assert_eq!(src.matches(&decl).count(), 1, "{name} is not declared exactly once");
        }
    }

    /// Every preset hangs off `Text`, so a change to the base reaches all
    /// of them and none of them can quietly drift onto a bare label with
    /// its own padding.
    #[test]
    fn the_variants_all_derive_from_the_base() {
        let src = include_str!("typography.rs");
        for name in ["TextSmall", "TextLarge", "TextMuted", "TextStrong", "TextCode"] {
            let decl = format!("mod.widgets.{name} = mod.widgets.Text{{");
            assert!(src.contains(&decl), "{name} does not derive from Text");
        }
    }
}
