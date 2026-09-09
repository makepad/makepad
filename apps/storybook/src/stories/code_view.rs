//! The code view story: a block of code that is an editor with the editing
//! turned off.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CodeViewOverview = StoryPage{
        StoryNote{text: "A block of code, highlighted, in a page rather than in an editor pane. Six places in this repository show one: a node's source, a log line's detail, a shader."}

        StoryHeading{text: "A block of code"}
        StoryNote{text: "Read only, no gutter, and Fit in height, so it takes exactly the room its lines need and sits in a column like a paragraph."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                subject := CodeView{
                    editor +: {width: Fill}
                    text: "fn resolve(collapse: Collapse, room: f64, bar: f64) -> f64 {\n    match collapse {\n        Collapse::A => 0.0,\n        Collapse::B => (room - bar).max(0.0),\n        Collapse::None => clamp(room),\n    }\n}"
                }
            }
        }

        StoryHeading{text: "Long lines scroll, they do not wrap"}
        StoryNote{text: "word_wrap is off, which is right for code: a wrapped line stops lining up with the one above it. Turn it on where the text is prose that happens to be monospaced — a log message, an error."}
        StoryRow{
            View{
                width: 420. height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                scrolls := CodeView{
                    editor +: {width: Fill}
                    text: "let position = resolve_split_position(align.to_position(axis, rect), room, min_a, min_b, collapse, size);"
                }
            }
            View{
                width: 420. height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                wraps := CodeView{
                    editor +: {width: Fill word_wrap: true}
                    text: "let position = resolve_split_position(align.to_position(axis, rect), room, min_a, min_b, collapse, size);"
                }
            }
        }

        StoryHeading{text: "It is an editor with the editing turned off"}
        StoryNote{text: "read_only is a property of the editor inside, not of this widget, so a caller can hand it back. Type in this one: it takes the keystroke and says so. One place in this repository does exactly that, which is why the name is about the preset rather than about a restriction."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                typable := CodeView{
                    editor +: {width: Fill read_only: false}
                    text: "// type here"
                }
            }
        }
        StoryRow{
            edited := Label{text: "not edited yet"}
        }
    }
}

fn code_view_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    use makepad_code_editor::code_view::CodeViewAction;
    let typable = root.widget(cx, ids!(typable));
    let uid = typable.widget_uid();
    if actions
        .find_widget_action(uid)
        .is_some_and(|a| matches!(a.cast::<CodeViewAction>(), CodeViewAction::Changed))
    {
        let n = typable.text().len();
        root.label(cx, ids!(edited))
            .set_text(cx, &format!("edited, now {n} characters"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "text/codeview/overview",
    category: "Text",
    component: "CodeView",
    also: &[],
    name: "Overview",
    dsl: "CodeViewOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# CodeView

A block of highlighted code that sits in a page rather than in an editor pane. Six places in this repository show one.

It is not a separate widget from the editor. It is `CodeEditor` wearing a preset: `read_only`, no gutter, no word wrap, and `height: Fit` so it takes exactly the room its lines need instead of filling a pane. `text` sets the contents, and the document is built from it the first time the view draws.

**`read_only` belongs to the editor inside, not to this widget.** `editor +: {read_only: false}` hands the editing back and the view raises `Changed` on every keystroke — one caller in this repository does precisely that. So the name describes the preset it ships with, not a restriction it enforces, and a reader who assumes otherwise will be surprised by a page they can type into.

`word_wrap` is off, which is what you want for code: a wrapped line stops lining up with the one above it, and indentation is how code is read. Turn it on where the monospaced text is really prose — a log message, an error, a stack frame.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: Some(code_view_actions),
}];
