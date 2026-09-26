//! The marked spans story: saying which word is wrong, under the words
//! themselves, in a flow and in a field.
use crate::makepad_widgets::text_flow::TextFlow;
use crate::makepad_widgets::text_input::TextMark;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryMarkedSpansBase = #(StoryMarkedSpans::register_widget(vm))

    mod.storybook.StoryMarkedSpans = set_type_default() do mod.storybook.StoryMarkedSpansBase{
        width: Fill
        height: Fit
    }

    mod.storybook.StoryMarkedFieldBase = #(StoryMarkedField::register_widget(vm))

    mod.storybook.StoryMarkedField = set_type_default() do mod.storybook.StoryMarkedFieldBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        field := TextInput{
            width: 320.
            text: "sales@exampel.com"
        }
    }

    mod.stories.TextFlowMarkedSpans = StoryPage{
        StoryNote{text: "A mark says which stretch of text is wrong, doubtful or worth noticing, and it draws under those words. A field that turns red says only that something in it is wrong; a mark says which word."}

        StoryHeading{text: "Three flavours"}
        StoryNote{text: "Error, warning and note. The colours are theme tokens, the wave is measured in pixels, and the last mark below runs past the end of a line: it comes back as one band per row, each hanging from that row's own baseline."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                subject := mod.storybook.StoryMarkedSpans{}
            }
        }

        StoryHeading{text: "In a field"}
        StoryNote{text: "The same call on a text field. Type in it and the mark goes: the library does no edit arithmetic, so a host re-validates and marks again rather than have the squiggle drift onto the wrong word."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                mod.storybook.StoryMarkedField{}
            }
        }
    }
}

/// The paragraph the flow above draws. The marks are found in it by search
/// rather than written as numbers, so editing the sentence cannot silently
/// move a squiggle onto the wrong word.
const PARAGRAPH: &str = "A form that colours the whole field can only say that \
something in it is wrong. This sentence has a mispelt word in it, a claim that \
is merely doubtful, and a phrase worth a second look that is long enough to run \
past the end of a line and carry its mark onto the next row with it.";

const MISSPELT: &str = "mispelt";
const DOUBTFUL: &str = "merely doubtful";
const NOTEWORTHY: &str = "long enough to run past the end of a line";

fn paragraph_marks() -> Vec<TextMark> {
    let mut marks = Vec::new();
    if let Some(at) = PARAGRAPH.find(MISSPELT) {
        marks.push(TextMark::error(at, at + MISSPELT.len()));
    }
    if let Some(at) = PARAGRAPH.find(DOUBTFUL) {
        marks.push(TextMark::warning(at, at + DOUBTFUL.len()));
    }
    if let Some(at) = PARAGRAPH.find(NOTEWORTHY) {
        marks.push(TextMark::note(at, at + NOTEWORTHY.len()));
    }
    marks
}

/// A flow with three marked spans in it.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryMarkedSpans {
    #[deref]
    flow: TextFlow,
    #[rust]
    marked: bool,
}

impl Widget for StoryMarkedSpans {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Marked before the pass rather than after it: a flow only records its
        // layout when something needs a byte range turned back into rects, so
        // marks set now are drawn on this frame rather than the next one.
        if !self.marked {
            self.marked = true;
            self.flow.set_marks(cx.cx.cx, paragraph_marks());
        }
        self.flow.begin(cx, walk);
        self.flow.draw_text(cx, PARAGRAPH);
        self.flow.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.flow.handle_event(cx, event, scope);
    }
}

/// The text the field above is declared with, mirrored here so the mark can be
/// found in it by search rather than written as a number.
const FIELD_TEXT: &str = "sales@exampel.com";
const FIELD_TYPO: &str = "exampel";

/// A field with one word in it marked wrong.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryMarkedField {
    #[deref]
    view: View,
    #[rust]
    marked: bool,
}

impl Widget for StoryMarkedField {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        // Marked after the first draw rather than before it: the field is
        // looked up by id, and there is nothing to look up until the view has
        // been walked once. Marking asks for a redraw.
        if !self.marked {
            let field = self.view.text_input(cx.cx.cx, ids!(field));
            if field.borrow().is_some() {
                self.marked = true;
                if let Some(at) = FIELD_TEXT.find(FIELD_TYPO) {
                    field.set_marks(cx.cx.cx, [TextMark::error(at, at + FIELD_TYPO.len())]);
                }
            }
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "text/textflow/marked-spans",
    category: "Text",
    component: "TextFlow",
    also: &["TextInput", "Markdown", "Html", "RichText"],
    name: "Marked spans",
    dsl: "TextFlowMarkedSpans",
    added: "2026-09-18",
    tags: &[],
    doc: "# Marked spans

A mark says that **this stretch of text** is wrong, doubtful or worth noticing, and it draws under those words rather than round the whole control.

That distinction is the whole feature. A field that turns red on a validation failure says only that something somewhere in it is wrong; an address list with one bad address in it turns red in its entirety and the reader has to find the bad one. A mark says which one.

## Setting one

`set_marks` replaces the marks; `add_mark` adds one; `clear_marks` drops them all. The offsets are **bytes into the widget's own text**, half open, and may be given either way round.

```
flow.set_marks(cx, [
    TextMark::error(at, at + word.len()),
    TextMark::warning(start, end),
]);
```

Three flavours: `error` for wrong, `warning` for doubtful, `note` for worth noticing. Error and warning draw a wave in `color_error` and `color_warning`; a note draws a dotted rule in `color_info`, so the three do not rely on colour alone to tell each other apart.

## What it reaches

`TextFlow` and `TextInput` carry the marks themselves, and `Markdown`, `Html` and `RichText` are built on a `TextFlow` they expose, so a mark set on that flow reaches all three. The decoration is one shader arm on the path that already draws the IME composition underline.

## Overlapping marks all draw

Two marks over the same word are two facts about it — a misspelling inside a sentence flagged as too long — and both squiggles draw, in the order they were given. Nothing is merged and nothing is silently dropped.

## Marks go when the text changes

Editing the text drops every mark on it. The library deliberately does no edit arithmetic: an offset that was right before an edit is a guess afterwards, and a squiggle under the wrong word is worse than no squiggle. The host has just changed the text, so it is about to validate it again anyway — and can mark it again then.

## The wave is in pixels

Amplitude, wavelength, thickness and the gap below the baseline are all pixel values on `DrawTextMark`. That is deliberate: a wave measured as a fraction of the row height gives the same word a different squiggle in a heading than in the caption under it, and stretches into a blur over a long run.

A mark that crosses a line break comes back as one band per row, each hanging from that row's own baseline — not one box spanning both.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: None,
}];
