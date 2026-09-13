//! The text flow story: the engine the markdown and html widgets are built
//! on, driven directly, and the maths renderer beside it.
use crate::makepad_widgets::text_flow::TextFlow;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryTextFlowBase = #(StoryTextFlow::register_widget(vm))

    mod.storybook.StoryTextFlow = set_type_default() do mod.storybook.StoryTextFlowBase{
        width: Fill
        height: Fit
        selectable: true
    }

    mod.stories.TextFlowOverview = StoryPage{
        StoryNote{text: "A paragraph is not a Label. A Label draws one string in one style; this lays out a run of text that wraps, changes style part way through, and can hold a quote or a block of code inside the same flow. It is what the markdown and html widgets are made of, and the pdf view too."}
        StoryNote{text: "Which one to use, against Markdown and Html, is set out on the Docs tab."}

        StoryHeading{text: "One paragraph, several styles"}
        StoryNote{text: "The styles below are pushed and popped around pieces of one continuous run — there is no separate widget per style. Drag across the text: the selection crosses the style boundaries because it is one flow, not four labels in a row."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                subject := mod.storybook.StoryTextFlow{}
            }
        }

        StoryHeading{text: "Mathematics"}
        StoryNote{text: "A separate widget with its own font, taking LaTeX and setting it. Fractions, roots, superscripts and the Greek letters; the size is the only knob most callers touch."}
        StoryRow{
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_2
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                MathView{text: "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}" font_size: 15.0}
                MathView{text: "e^{i\\pi} + 1 = 0" font_size: 15.0}
                MathView{text: "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}" font_size: 15.0}
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryTextFlow {
    #[deref]
    flow: TextFlow,
}

impl Widget for StoryTextFlow {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.flow.begin(cx, walk);

        self.flow.draw_text(cx, "This is one continuous run of text. ");
        // The style stacks are counters, not settings: push one around a
        // piece of the run and pop it, and the flow keeps laying out where
        // it left off rather than starting a new widget.
        self.flow.bold.push();
        self.flow.draw_text(cx, "This part is bold");
        self.flow.bold.pop();
        self.flow.draw_text(cx, ", ");
        self.flow.italic.push();
        self.flow.draw_text(cx, "this part is italic");
        self.flow.italic.pop();
        self.flow.draw_text(cx, ", and ");
        self.flow.inline_code.push();
        self.flow.draw_text(cx, "this part is code");
        self.flow.inline_code.pop();
        self.flow.draw_text(cx, ", and ");
        // A link is a widget the flow instantiates from a template and lays
        // out inline with the text around it - the one place in this widget
        // where a real child appears mid-paragraph.
        self.flow
            .draw_link(cx, live_id!(link), live_id!(story_link), "this part is a link");
        self.flow.draw_text(
            cx,
            ". They nest, so bold inside italic is a matter of pushing both. \
             The paragraph wraps at whatever width it is given, and none of \
             this is a separate widget: it is one flow, which is why a \
             selection can cross a style boundary.",
        );

        // A block does not begin its own line. The flow lays out where it
        // left off until the host breaks it, so markdown breaks the line
        // before every block it opens - and without this the quote runs on
        // at the end of the paragraph, which is exactly how this page first
        // drew it.
        self.flow.new_line_collapsed_with_spacing(cx, 8.0);
        self.flow.begin_quote(cx);
        self.flow.draw_text(cx, "A quote is a block within the same flow.");
        self.flow.end_quote(cx);

        self.flow.new_line_collapsed_with_spacing(cx, 8.0);
        self.flow.begin_code(cx);
        self.flow.draw_text(cx, "let flow = TextFlow;\nflow.bold.push();");
        self.flow.end_code(cx);
        self.flow.new_line_collapsed(cx);

        self.flow.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.flow.handle_event(cx, event, scope);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "text/textflow/overview",
    category: "Text",
    component: "TextFlow",
    also: &["MathView", "TextFlowLink"],
    name: "Overview",
    dsl: "TextFlowOverview",
    added: "2025-05-06",
    tags: &[],
    doc: "# TextFlow

A paragraph is not a `Label`. A label draws one string in one style. `TextFlow` lays out a run that **wraps, changes style part way through, and can hold a quote or a block of code inside the same flow**. It is the engine the markdown widget, the html widget and the pdf view are all built on.

**It has no `text` property.** Unlike almost everything else in the library it is driven from Rust: a host derefs it, calls `begin`, pushes text and style, and calls `end`. That is why there is a small widget behind this page rather than a declaration — the same shape markdown uses.

**The styles are counters, not settings.** `bold`, `italic`, `fixed`, `underline`, `strikethrough` and `inline_code` are each a stack: push one around a piece of the run, pop it, and the flow carries on laying out where it left off. They nest, so bold inside italic is a matter of pushing both. Nothing here creates a widget per style, which is what lets a selection cross a style boundary — try dragging across the paragraph.

`draw_link` is the one place a real child appears mid-paragraph: the flow instantiates a `TextFlowLink` from a template and lays it out inline with the text either side of it, which is how markdown and html put a link in a sentence.

`begin_quote`/`end_quote` and `begin_code`/`end_code` open blocks inside the same flow, and markdown's tables and list items work the same way — but **a block does not begin its own line.** The flow keeps laying out where it left off until the host breaks it with `new_line_collapsed_with_spacing`, which is why markdown breaks the line before every block it opens. Leave it out and the quote runs on at the end of the paragraph.

## Which one to use

| Want | Use |
|---|---|
| text written in markdown | `Markdown`, on the Markdown page |
| text written in html, or a widget of your own placed inline by a tag | `Html`, on the Html page |
| a run pushed from Rust by a widget of your own | `TextFlow` itself |

## MathView

A separate widget with its own maths font that takes LaTeX and sets it — fractions, roots, superscripts, sums and the Greek letters. `font_size` is the only knob most callers touch.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: None,
}];
