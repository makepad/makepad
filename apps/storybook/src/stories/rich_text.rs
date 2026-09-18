//! The rich text story: a document of blocks and marked runs, typed into.
use crate::makepad_widgets::rich_text::{BlockKind, Mark, RichTextEditorWidgetRefExt};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RichTextEditorOverview = StoryPage{
        StoryNote{text: "Text with marks in it. A document is a list of blocks — paragraph, heading, bullet, numbered, quote, code — each a list of runs, and a run is a string with a set of marks over it. Click into it and type; the buttons act on whatever is selected."}

        StoryHeading{text: "The marks"}
        StoryNote{text: "Select a stretch of the text below and press one. A mark over part of a run splits the run in three; taking it off again merges what is left back into one, so the document never grows a run per keystroke. With nothing selected a button arms the next thing typed instead."}
        StoryRow{
            View{
                width: Fill height: Fit flow: Right spacing: theme.space_1
                bold := ButtonFlat{text: "Bold"}
                italic := ButtonFlat{text: "Italic"}
                underline := ButtonFlat{text: "Underline"}
                strike := ButtonFlat{text: "Strike"}
                code := ButtonFlat{text: "Code"}
                link := ButtonFlat{text: "Link"}
                unlink := ButtonFlat{text: "Unlink"}
            }
        }

        StoryHeading{text: "The block kinds"}
        StoryNote{text: "These act on every block the selection touches. Return splits a block, Return on an empty list item ends the list, and Backspace at the head of a block takes its kind off before it takes any text."}
        StoryRow{
            View{
                width: Fill height: Fit flow: Right spacing: theme.space_1
                as_para := ButtonFlat{text: "Paragraph"}
                as_h1 := ButtonFlat{text: "Title"}
                as_h2 := ButtonFlat{text: "Heading"}
                as_bullet := ButtonFlat{text: "Bullets"}
                as_number := ButtonFlat{text: "Numbers"}
                as_quote := ButtonFlat{text: "Quote"}
                as_code := ButtonFlat{text: "Code block"}
            }
        }
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                subject := mod.widgets.RichTextEditor{
                    text: "# A document\n\nThis is one **paragraph** with *italic*, <u>underlined</u> and ~~struck~~ text in it, a piece of `inline code`, and [a link](handbook/rich-text) that goes somewhere.\n\n- a bullet\n- and another one\n\n1. first\n1. second\n\n> A quote is a block of its own.\n\n```\nfn main() {\n    let doc = RichDoc::new();\n}\n```"
                }
            }
        }
        StoryRow{
            caret_note := Label{text: "the caret is in a paragraph, carrying nothing"}
        }

        StoryHeading{text: "What the document says"}
        StoryNote{text: "The same document written back out as markup after every edit. It is what `text` holds, so a host can keep a document in a string, a field or a file and hand it back later."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                markup_note := Label{
                    width: Fill
                    text: "..."
                    draw_text +: {text_style: theme.font_code{font_size: theme.font_size_p}}
                }
            }
        }

        StoryHeading{text: "The same document, read only"}
        StoryNote{text: "`RichTextView` is the same widget with the typing turned off: it takes no keystrokes and draws no caret, the text can still be selected and copied, and a click on a link is reported rather than swallowed. This one follows the editor above."}
        StoryRow{
            View{
                width: Fill height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                mirror := mod.widgets.RichTextView{
                    text: "# A document\n\nThis is one **paragraph** with *italic*, <u>underlined</u> and ~~struck~~ text in it, a piece of `inline code`, and [a link](handbook/rich-text) that goes somewhere.\n\n- a bullet\n- and another one\n\n1. first\n1. second\n\n> A quote is a block of its own.\n\n```\nfn main() {\n    let doc = RichDoc::new();\n}\n```"
                }
            }
        }
        StoryRow{
            link_note := Label{text: "no link followed yet"}
        }

        StoryHeading{text: "An empty one"}
        StoryNote{text: "A document always has one block, so there is always somewhere to put the caret. Click in the strip below and start typing."}
        StoryRow{
            View{
                width: Fill height: 60.
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                mod.widgets.RichTextEditor{text: ""}
            }
        }

        StoryHeading{text: "What it does not do"}
        StoryNote{text: "No images, tables, rules or nested lists, and no phone-keyboard composition: it edits from a hardware keyboard. The clipboard carries plain text both ways — the marks do not travel with it — and a run that is both struck through and underlined draws only the strike, because the flow underneath draws one decoration per run."}
    }
}

/// What the caret carries, in words, for the line under the editor.
fn describe(marks: &[&str], kind: BlockKind) -> String {
    let where_ = match kind {
        BlockKind::Paragraph => "a paragraph",
        BlockKind::Heading(1) => "a title",
        BlockKind::Heading(_) => "a heading",
        BlockKind::Bullet => "a bullet",
        BlockKind::Numbered => "a numbered item",
        BlockKind::Quote => "a quote",
        BlockKind::Code => "a code block",
    };
    if marks.is_empty() {
        format!("the caret is in {where_}, carrying nothing")
    } else {
        format!("the caret is in {where_}, carrying {}", marks.join(" + "))
    }
}

fn rich_text_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.rich_text_editor(cx, ids!(subject));

    if root.button(cx, ids!(bold)).clicked(actions) {
        subject.toggle_mark(cx, Mark::Bold);
    }
    if root.button(cx, ids!(italic)).clicked(actions) {
        subject.toggle_mark(cx, Mark::Italic);
    }
    if root.button(cx, ids!(underline)).clicked(actions) {
        subject.toggle_mark(cx, Mark::Underline);
    }
    if root.button(cx, ids!(strike)).clicked(actions) {
        subject.toggle_mark(cx, Mark::Strike);
    }
    if root.button(cx, ids!(code)).clicked(actions) {
        subject.toggle_mark(cx, Mark::Code);
    }
    // A page has nowhere to ask for a target, so it hands over a fixed one.
    // The widget takes any string: it never follows a link itself.
    if root.button(cx, ids!(link)).clicked(actions) {
        subject.set_link(cx, Some("handbook/rich-text"));
    }
    if root.button(cx, ids!(unlink)).clicked(actions) {
        subject.set_link(cx, None);
    }

    if root.button(cx, ids!(as_para)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Paragraph);
    }
    if root.button(cx, ids!(as_h1)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Heading(1));
    }
    if root.button(cx, ids!(as_h2)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Heading(2));
    }
    if root.button(cx, ids!(as_bullet)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Bullet);
    }
    if root.button(cx, ids!(as_number)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Numbered);
    }
    if root.button(cx, ids!(as_quote)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Quote);
    }
    if root.button(cx, ids!(as_code)).clicked(actions) {
        subject.set_kind(cx, BlockKind::Code);
    }

    let changed = subject.changed(actions);
    if changed || subject.selection_changed(actions) {
        let marks = subject.marks_at_caret();
        let mut names: Vec<&str> = Vec::new();
        for (mark, name) in [
            (Mark::Bold, "bold"),
            (Mark::Italic, "italic"),
            (Mark::Underline, "underline"),
            (Mark::Strike, "strikethrough"),
            (Mark::Code, "code"),
        ] {
            if marks.has(mark) {
                names.push(name);
            }
        }
        if marks.link().is_some() {
            names.push("a link");
        }
        let note = describe(&names, subject.kind_at_caret());
        root.label(cx, ids!(caret_note)).set_text(cx, &note);
    }
    if changed {
        let markup = subject.markup();
        root.label(cx, ids!(markup_note)).set_text(cx, &markup);
        // The read-only one below is handed the same document, which is the
        // whole of what a host has to do to render one.
        root.rich_text_editor(cx, ids!(mirror)).set_markup(cx, &markup);
    }

    if let Some(href) = root.rich_text_editor(cx, ids!(mirror)).link_clicked(actions) {
        root.label(cx, ids!(link_note))
            .set_text(cx, &format!("followed: {href}"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "text/richtexteditor/overview",
    category: "Text",
    component: "RichTextEditor",
    also: &["RichTextView"],
    name: "Overview",
    dsl: "RichTextEditorOverview",
    added: "2026-09-10",
    tags: &[
        "new",
        "text",
        "rich text",
        "editor",
        "document",
        "marks",
        "bold",
        "italic",
        "formatting",
    ],
    doc: "# RichTextEditor\n\nText with marks in it, and the model that holds them.\n\nA document is a list of blocks. A block is a **kind** — paragraph, heading, bullet, numbered, quote, code — and a list of **runs**. A run is a string and the set of **marks** over the whole of it: bold, italic, underline, strikethrough, inline code, and a link with a target. That model is the widget: applying a mark, splitting a block or merging two of them are methods on it, and every one of them is tested without a window.\n\n## Runs split and merge; they are never a list of styled characters\n\nApplying bold to the middle of a run leaves three runs, and taking it off the middle of a bold run leaves three again. The other school keeps a style per character and rebuilds the runs at draw time; it is easier to write and it loses the one thing worth having — the invariant that **two adjacent runs never carry the same marks**. That invariant is what makes \"does the whole selection have this mark\" a cheap question, what stops the document growing a run per keystroke, and what keeps the tests short enough to read.\n\nA link is a mark with a target rather than a sixth flag, so two links side by side stay two runs: runs merge only when their marks — target included — are equal. Typing at the right edge of a link is not part of the link, which is the one place the left-inherit rule is broken on purpose.\n\n## It draws through the text flow\n\nDrawing is `TextFlow`, the same engine the markdown widget uses, driven a block at a time; the flow's own text tracker is what turns a click into a character index. The caret is drawn **inside** the flow: the run it sits in is drawn in two halves with a thin quad walked between them, so the flow puts the caret exactly where the next glyph would go, on the right line, with no geometry arithmetic in the widget at all.\n\n## The keys\n\n| Key | What it does |\n|---|---|\n| arrows | move by a character, or by a word with alt or control |\n| up / down | the line above or below, asked of the flow |\n| Home / End | the ends of the block, or of the document with the primary key |\n| Return | split the block; on an empty list item, end the list |\n| Backspace | at the head of a block, take its kind off before its text |\n| primary + B / I / U / E | bold, italic, underline, inline code |\n| primary + shift + X | strikethrough |\n| primary + Z, primary + shift + Z | undo, redo — a run of keystrokes is one undo |\n\n## Markup\n\n`text` is the document written out in a small, common markup — `**bold**`, `*italic*`, `<u>underline</u>`, `~~struck~~`, `` `code` ``, `[text](target)`, `#` headings, `-` and `1.` items, `>` quotes and fenced code blocks. Set it to load a document; it is rewritten after every edit, so it is always what the document says.\n\n## RichTextView\n\nThe same widget with `editable: false`: no caret and no keystrokes, but the text is still selectable and a click on a link is reported as an action instead of moving the caret.\n\n## What it deliberately does not do\n\nNo images, tables, rules, nested lists or footnotes. No phone-keyboard composition — it edits from a hardware keyboard. The clipboard carries plain text both ways, so marks do not travel with it, and the caret does not blink. A run that is both struck through and underlined draws only the strike, because the flow underneath draws one decoration per run.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Editable", target: "subject", kind: ControlKind::Bool { prop: "editable", default: true } },
        Control { label: "Block spacing", target: "subject", kind: ControlKind::Number { prop: "block_spacing", min: 0., max: 48., step: 1., default: 12. } },
        Control { label: "Heading scale", target: "subject", kind: ControlKind::Number { prop: "heading_scale", min: 1., max: 3., step: 0.05, default: 1.8 } },
        Control { label: "Caret width", target: "subject", kind: ControlKind::Number { prop: "caret_width", min: 1., max: 4., step: 0.5, default: 1.5 } },
        Control { label: "Caret height", target: "subject", kind: ControlKind::Number { prop: "caret_height", min: 1., max: 2.5, step: 0.05, default: 1.55 } },
    ],
    on_actions: Some(rich_text_actions),
}];
