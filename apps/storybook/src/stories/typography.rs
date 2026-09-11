//! The typography stories: the type scale with names on it, and the
//! heading ladder those names sit under.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TypographyOverview = StoryPage{
        StoryHeading{text: "Body"}
        StoryNote{text: "Three rungs, named rather than numbered. Each takes its size from the theme's type scale, so a theme that retunes the scale retunes every screen written in these."}
        StoryHeading{text: "The specimen, under the controls"}
        StoryNote{text: "One string through the whole family, so what is being compared is the rungs and not the sentences. A pangram judges the letterforms, the figures show whether numbers stay legible at the small end, and the long word shows where each rung starts to crowd."}
        StoryRow{
            pangram := Button{text: "Pangram"}
            figures := Button{text: "Figures"}
            long := Button{text: "Long word"}
        }
        large := TextLarge{text: "The quick brown fox jumps over the lazy dog"}
        sample := Text{text: "The quick brown fox jumps over the lazy dog"}
        small := TextSmall{text: "The quick brown fox jumps over the lazy dog"}
        strong := TextStrong{text: "The quick brown fox jumps over the lazy dog"}
        muted := TextMuted{text: "The quick brown fox jumps over the lazy dog"}
        code := TextCode{text: "The quick brown fox jumps over the lazy dog"}
        TextLarge{text: "TextLarge, the line that has to be read first"}
        Text{text: "Text, the size the rest of a screen is written in"}
        TextSmall{text: "TextSmall, for captions, units and timestamps"}

        StoryHeading{text: "Emphasis"}
        StoryNote{text: "Strong changes the weight and muted changes the colour. Neither moves a word up or down the scale, so a sentence built out of all three keeps one rhythm."}
        StoryRow{
            Text{text: "the last frame took"}
            TextStrong{text: "18 ms"}
            TextMuted{text: "(median over the last hundred)"}
        }
        TextStrong{text: "TextStrong"}
        TextMuted{text: "TextMuted"}

        StoryHeading{text: "Code"}
        StoryNote{text: "The monospaced face for an identifier, a path or a value quoted inside a sentence. It is a face and not a chip: nothing is drawn behind it."}
        StoryRow{
            Text{text: "read the scale from"}
            TextCode{text: "window.dpi_factor"}
        }

        StoryHeading{text: "Headings"}
        StoryNote{text: "The heading ladder runs all the way to H6. H5 and H6 are the two quiet rungs, for the fourth and fifth level of a document that has that many."}
        H1{text: "H1"}
        H2{text: "H2"}
        H3{text: "H3"}
        H4{text: "H4"}
        H5{text: "H5"}
        H6{text: "H6"}

        StoryHeading{text: "Wrapping"}
        StoryNote{text: "The presets above measure to their ink and do not wrap, which is what lets them stand in a row beside anything. Copy that has to wrap is P and TextBox, which fill their parent's width instead."}
        P{text: "A paragraph fills the width it is given and breaks where it runs out, so the same text reads as one block at any window size. Drag the window narrower and this line rewraps; the row of presets above it does not, because each of those is only as wide as the letters in it."}
    }

}

const PANGRAM: &str = "The quick brown fox jumps over the lazy dog";
const FIGURES: &str = "0123456789 1,234.56 -42 100% 03:17:04";
const LONG_WORD: &str = "counterrevolutionaries";

/// All six specimens take the same string at once: a family read against
/// six different sentences tells you nothing about the family.
fn typography_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let specimen = if root.button(cx, ids!(pangram)).clicked(actions) {
        PANGRAM
    } else if root.button(cx, ids!(figures)).clicked(actions) {
        FIGURES
    } else if root.button(cx, ids!(long)).clicked(actions) {
        LONG_WORD
    } else {
        return;
    };
    for path in [
        ids!(large),
        ids!(sample),
        ids!(small),
        ids!(strong),
        ids!(muted),
        ids!(code),
    ] {
        root.label(cx, path).set_text(cx, specimen);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "text/text/overview",
        category: "Text",
        component: "Text",
        also: &["H5", "H6", "TextCode", "TextLarge", "TextMuted", "TextSmall", "TextStrong"],
        name: "Overview",
        dsl: "TypographyOverview",
        added: "2026-09-10",
        tags: &["new", "type"],
        doc: "# Text

The type scale with names on it. `Text` is the body size, `TextSmall` and `TextLarge` the rungs either side of it, and every one of them takes its size from a theme token rather than a number written where it is used. Retune the scale in the theme and every screen written in these moves with it.

`TextStrong` is the bold face at exactly the body size, so emphasis is a change of weight and not a change of size. `TextMuted` is body in the quieter of the two colours that read on a surface, for the half of a row that is context rather than content. `TextCode` is the monospaced face for an identifier or a value quoted inside a sentence — a face, not a chip: nothing is drawn behind it.

All six measure to their ink and none of them wrap, which is what lets them stand in a row beside a button or an icon. Copy that has to wrap is `P` or `TextBox`, which fill their parent's width; the two families are kept apart because a Fill inside a Fit resolves to nothing and paints nothing.

The heading ladder is the label module's and is shown here whole, including `H5` and `H6`, the two rungs no page documented before.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: Some(typography_actions),
    },];
