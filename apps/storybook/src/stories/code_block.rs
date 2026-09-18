//! The code block story: a run of code as a block, with its lines counted
//! and a control that takes a copy, and the highlighted block beside it that
//! is an editor with the editing turned off.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CodeBlockOverview = StoryPage{
        StoryNote{text: "A run of code shown as a block: counted lines, the language's name, a control that takes a copy, and a band behind the lines worth looking at. It draws one ink — it is not a highlighter, and code that wants colour or a line too long to fit belongs in CodeView, which is the editor with the editing turned off."}

        StoryHeading{text: "A block of code"}
        StoryRow{
            width: Fill
            subject := CodeBlock{
                width: Fill
                language: "rust"
                highlight: "6"
                text: "fn body_top(&self) -> f64 {\n    self.header_h + self.pad_y\n}\n\nfn line_top(&self, index: usize) -> f64 {\n    self.body_top() + index as f64 * self.line_h\n}"
            }
        }
        StoryRow{
            copy_note := Label{text: "press Copy, or focus the block and use the copy key"}
        }

        StoryHeading{text: "What is drawn and what is copied"}
        StoryNote{text: "The numbers are drawn, never part of the text. That is the whole reason they are here: a copy gives back the code as it was written, so pasting it does not first mean deleting a column of numbers. Tabs are expanded to the next stop for drawing — the text engine has no tab stop and the indentation would otherwise vanish without trace — and a copy hands the tabs back."}
        StoryRow{
            width: Fill
            tabbed := CodeBlock{
                width: Fill
                language: "rust"
                text: "fn main() {\n\tlet room = 4;\n\tif room > 0 {\n\t\tstep(room);\n\t}\n}"
            }
        }

        StoryHeading{text: "An excerpt out of the middle of a file"}
        StoryNote{text: "first_line is the number the first line carries, and highlight is read in the numbers the gutter shows rather than in how far down the block a line happens to be."}
        StoryRow{
            width: Fill
            excerpt := CodeBlock{
                width: Fill
                language: "rust"
                first_line: 98
                highlight: "99-100"
                text: "let last = first + lines - 1;\nif to < first || from > last {\n    continue;\n}"
            }
        }

        StoryHeading{text: "Without the gutter"}
        StoryNote{text: "show_line_numbers: false for a block short enough that nobody will point at a line by number. A block with no copy control and no language label has no header either, and closes up round the code."}
        StoryRow{
            width: Fill
            bare := CodeBlock{
                width: Fill
                show_line_numbers: false
                show_copy: false
                text: "cx.push_clip_rect(rect);\ncx.pop_clip_rect();"
            }
        }

        StoryHeading{text: "A session"}
        StoryNote{text: "CodeTerminal puts a mark before each line in place of a number. The mark is drawn too, so a copy gives back the commands alone — the thing you actually wanted off the screen."}
        StoryRow{
            width: Fill
            session := CodeTerminal{
                width: Fill
                text: "build --release\ntest --all"
            }
        }

        StoryHeading{text: "A session with output in it"}
        StoryNote{text: "prompt_lines names the lines that carry the mark, so what came back is left plain. Empty — the default — marks every line, which is what a block of commands wants."}
        StoryRow{
            width: Fill
            transcript := CodeTerminal{
                width: Fill
                prompt_lines: "1"
                text: "test --all\nrunning 17 tests\ntest result: ok. 17 passed"
            }
        }

        StoryHeading{text: "As wide as the code"}
        StoryNote{text: "A line wider than the block is cut off at the trailing padding: it does not wrap, because a wrapped line stops lining up with the one above it. Give the block width: Fit where nothing may be cut, and it takes exactly the room its longest line needs."}
        StoryRow{
            Label{text: "260 wide — cut"}
            clipped := CodeBlock{
                width: 260.
                show_copy: false
                show_line_numbers: false
                text: "let position = resolve(align.to_position(axis, rect), room, min_a, min_b);"
            }
        }
        StoryRow{
            Label{text: "Fit — whole"}
            whole := CodeBlock{
                width: Fit
                show_copy: false
                show_line_numbers: false
                text: "let position = resolve(align.to_position(axis, rect), room, min_a, min_b);"
            }
        }

        StoryHeading{text: "A few words in a sentence"}
        StoryNote{text: "CodeInline is the same widget with no header, no gutter and only as wide as the words. It is a widget and not a text run, so it sits in a wrapping row beside labels; code inside a real paragraph belongs to Html and Markdown, which draw it through the text flow."}
        StoryRow{
            Label{text: "Give the block"}
            CodeInline{text: "width: Fit"}
            Label{text: "where nothing may be cut, and"}
            CodeInline{text: "show_copy: false"}
            Label{text: "where there is nothing worth taking away."}
        }

        StoryHeading{text: "The ladder"}
        StoryRow{
            Label{text: "CodeBlock"}
        }
        StoryRow{
            width: Fill
            CodeBlock{
                width: Fill
                language: "rust"
                text: "let one = 1;\nlet two = 2;"
            }
        }
        StoryRow{
            Label{text: "CodeTerminal"}
        }
        StoryRow{
            width: Fill
            CodeTerminal{
                width: Fill
                text: "build --release\ntest --all"
            }
        }
        StoryRow{
            Label{text: "CodeInline"}
            CodeInline{text: "let one = 1;"}
        }

        StoryHeading{text: "Highlighted, with an editor underneath"}
        StoryNote{text: "CodeView is the block to use when the code wants colour. It is the code editor wearing a preset: read only, no gutter, and Fit in height, so it takes exactly the room its lines need and sits in a column like a paragraph. Six places in this repository show one: a node's source, a log line's detail, a shader."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                highlighted := CodeView{
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

        StoryHeading{text: "The editing can be handed back"}
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

fn code_block_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.code_block(cx, ids!(subject)).copied(actions) {
        root.label(cx, ids!(copy_note))
            .set_text(cx, "the code is on the clipboard, numbers and all left behind");
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

/// One record has one handler: the block's copy note, then the view's edit
/// count.
fn code_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    code_block_actions(cx, root, actions);
    code_view_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "text/codeblock/overview",
    category: "Text",
    component: "CodeBlock",
    also: &["CodeTerminal", "CodeInline", "CodeView"],
    name: "Overview",
    dsl: "CodeBlockOverview",
    added: "2026-09-10",
    tags: &["new", "code", "snippet", "monospace", "copy", "terminal", "line numbers", "gutter"],
    doc: "# CodeBlock\n\nA run of code shown as a block, with its lines counted and a control that takes a copy. It lays the lines out itself: a gutter of numbers that can be turned off, a header carrying the language's name and the copy control, and a band behind the lines the writer wants looked at.\n\n## What it deliberately does not do\n\n**It does not highlight syntax.** Every line is one ink. Highlighting means a parser per language and this library already has one, in the editor: `CodeView` is that editor wearing a read-only preset, and code that wants colour, selection, or a line too long to fit belongs there rather than here. Running prose with a few words of code in it is served already too — `Html` and `Markdown` draw `<pre>` and `<code>` through the text flow — and this is not a replacement for either.\n\nIt does not wrap, scroll or edit. A line wider than the block is cut off at the trailing padding, because a wrapped line stops lining up with the one above it and indentation is how code is read. Give the block `width: Fit` where nothing may be cut.\n\n## What is drawn and what is copied\n\nThe numbers and the prompt marks are **drawn**. They are not part of `text`, and that is the whole reason they are here: a copy gives back the code exactly as it was handed in, so pasting it does not first mean deleting a column of numbers or a mark from the front of every line.\n\nTabs are the one place the two disagree. A tab is expanded to the next stop for drawing — the text engine has no tab stop, so an indented block would otherwise come out flush left with nothing to say it had lost anything — and a copy hands the tabs back untouched, because what is pasted has to compile.\n\n## Naming lines\n\n`highlight` and `prompt_lines` both take a list such as `\"3, 7-9\"`, read in the numbers the **gutter shows**: a block whose `first_line` is 98 is highlighted by what is on screen, not by how far down the block a line happens to be. A part that is not a number or a range of them is skipped rather than rejected, so one stray comma costs its own band and nothing else.\n\n## The three presets\n\n| Preset | For |\n|---|---|\n| `CodeBlock` | a listing: numbers, a language label, a copy control |\n| `CodeTerminal` | a session: a mark before each line in place of a number, on a darker face |\n| `CodeInline` | a few words inside a sentence: no header, no gutter, only as wide as the words |\n\n`prompt_lines` is what tells a command apart from what came back: name the command lines there and the output is left plain. Empty — the default — marks every line.\n\n## Copying\n\nPressing the control puts `text` on the clipboard and shows `copied_text` for `copied_secs`; `CodeBlockRef::copied` says when that happened. The block also takes key focus while it has a control, so the platform's own copy key works on it, and Return or Space press the control.\n\n## CodeView\n\nA block of highlighted code that sits in a page rather than in an editor pane. Six places in this repository show one.\n\nIt is not a separate widget from the editor. It is `CodeEditor` wearing a preset: `read_only`, no gutter, no word wrap, and `height: Fit` so it takes exactly the room its lines need instead of filling a pane. `text` sets the contents, and the document is built from it the first time the view draws.\n\n**`read_only` belongs to the editor inside, not to this widget.** `editor +: {read_only: false}` hands the editing back and the view raises `Changed` on every keystroke — one caller in this repository does precisely that. So the name describes the preset it ships with, not a restriction it enforces, and a reader who assumes otherwise will be surprised by a page they can type into.\n\n`word_wrap` is off, which is what you want for code: a wrapped line stops lining up with the one above it, and indentation is how code is read. Turn it on where the monospaced text is really prose — a log message, an error, a stack frame.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Language", target: "subject", kind: ControlKind::Text { prop: "language", default: "rust" } },
        Control { label: "Line numbers", target: "subject", kind: ControlKind::Bool { prop: "show_line_numbers", default: true } },
        Control { label: "First line", target: "subject", kind: ControlKind::Number { prop: "first_line", min: 1., max: 200., step: 1., default: 1. } },
        Control { label: "Highlighted", target: "subject", kind: ControlKind::Text { prop: "highlight", default: "6" } },
        Control { label: "Prompt", target: "subject", kind: ControlKind::Text { prop: "prompt", default: "" } },
        Control { label: "Copy control", target: "subject", kind: ControlKind::Bool { prop: "show_copy", default: true } },
        Control { label: "Line height", target: "subject", kind: ControlKind::Number { prop: "line_height", min: 10., max: 30., step: 0.5, default: 15. } },
        Control { label: "Tab size", target: "subject", kind: ControlKind::Number { prop: "tab_size", min: 1., max: 8., step: 1., default: 4. } },
    ],
    on_actions: Some(code_page_actions),
}];
