//! The key-cap story: one cap, a chord parsed from a string, and a
//! searchable list of shortcuts.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Caption = Label{
        draw_text +: {color: theme.color_text_meta}
    }

    mod.stories.KbdOverview = StoryPage{
        StoryNote{text: "A chord written into a sentence reads as prose and scans as nothing. Drawn as caps it is a small picture of the keyboard, and the eye finds it in a paragraph without reading a word of it."}

        StoryHeading{text: "One cap"}
        StoryNote{text: "A glyph or a short word in a small rounded box. A cap never gets narrower than `min_width`, so a single letter is still a key and not a sliver."}
        StoryRow{
            subject := Kbd{text: "K"}
            Kbd{text: "Esc"}
            Kbd{text: "F5"}
            Kbd{text: "Tab"}
            Kbd{text: "Space"}
            Kbd{text: "Enter"}
        }

        StoryHeading{text: "Held down"}
        StoryNote{text: "The face of the cap drops onto the side of the key and takes its letter with it. The box does not move, so a row of caps never twitches when one of them is pressed."}
        StoryRow{
            Kbd{text: "A"}
            Kbd{text: "A" pressed: true}
            press_me := Kbd{text: "P"}
            press_toggle := Button{text: "Hold P"}
        }

        StoryHeading{text: "A chord"}
        StoryNote{text: "`KbdGroup` takes the whole chord as one string and draws the caps in order, with a separator between them."}
        StoryRow{
            KbdGroup{shortcut: "ctrl+shift+P"}
            KbdGroup{shortcut: "ctrl+s"}
            KbdGroup{shortcut: "alt+f4"}
        }
        StoryRow{
            KbdGroup{shortcut: "esc"}
            KbdGroup{shortcut: "ctrl+plus"}
            KbdGroup{shortcut: "shift+up"}
            KbdGroup{shortcut: "ctrl+pgdn"}
            KbdGroup{shortcut: "ctrl++"}
        }

        StoryHeading{text: "Written in any order, drawn in one"}
        StoryNote{text: "A shortcut table written by several hands still reads as one table: the modifiers are printed in this platform's own order however they were typed, and the keys follow in the order they were written."}
        StoryRow{
            Caption{text: "shift+ctrl+alt+k"}
            KbdGroup{shortcut: "shift+ctrl+alt+k"}
            Caption{text: "alt+k+ctrl+shift"}
            KbdGroup{shortcut: "alt+k+ctrl+shift"}
        }

        StoryHeading{text: "A key nobody here has heard of"}
        StoryNote{text: "It is printed as it was written. Dropping it would leave a chord with a cap missing and nothing to say why."}
        StoryRow{
            KbdGroup{shortcut: "ctrl+Frobnicate"}
            KbdGroup{shortcut: ""}
            Caption{text: "an empty shortcut draws nothing at all"}
        }

        StoryHeading{text: "What goes between the caps"}
        StoryRow{
            KbdGroup{shortcut: "ctrl+alt+delete"}
            KbdGroupTight{shortcut: "ctrl+alt+delete"}
            KbdGroup{shortcut: "ctrl+alt+delete" separator: "\u{2192}"}
        }

        StoryHeading{text: "The ladder"}
        StoryNote{text: "`KbdFlat` is the face on its own; `Kbd` adds the side of the key under it."}
        StoryRow{
            KbdFlat{text: "KbdFlat"}
            Kbd{text: "Kbd"}
        }

        StoryHeading{text: "A list of shortcuts"}
        StoryNote{text: "Each line is either a heading or a row. Type in the box to filter: a row matches on what it does or on the chord as it was written, and a heading comes back with the first row that survives under it."}
        StoryRow{
            width: Fill
            help := ShortcutHelp{
                width: Fill
                entries: [
                    "# Editing"
                    "Copy = ctrl+c"
                    "Cut = ctrl+x"
                    "Paste = ctrl+v"
                    "Undo = ctrl+z"
                    "Redo = ctrl+shift+z"
                    "# Moving about"
                    "Find = ctrl+f"
                    "Find next = f3"
                    "Go to line = ctrl+g"
                    "Word left = ctrl+left"
                    "Word right = ctrl+right"
                    "# The window"
                    "Command list = ctrl+shift+p"
                    "Full screen = f11"
                    "Close = ctrl+w"
                    "Split the view"
                ]
            }
        }
        StoryRow{
            picked := Label{text: "no row picked yet"}
        }

        StoryHeading{text: "Without the search field"}
        StoryNote{text: "`search_height: 0` for a panel whose host already has a box to type in; the host then calls `set_query`."}
        StoryRow{
            width: Fill
            ShortcutHelp{
                width: Fill
                search_height: 0.
                entries: [
                    "# Playback"
                    "Play or pause = space"
                    "Back a frame = left"
                    "Forward a frame = right"
                ]
            }
        }
    }
}

fn kbd_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let cap = root.kbd(cx, ids!(press_me));
    if root.button(cx, ids!(press_toggle)).clicked(actions) {
        let held = cap.pressed();
        cap.set_pressed(cx, !held);
    }
    if let Some((label, shortcut)) = root.shortcut_help(cx, ids!(help)).picked(actions) {
        let said = if shortcut.is_empty() {
            format!("{label} has no shortcut")
        } else {
            format!("{label} \u{2013} {shortcut}")
        };
        root.label(cx, ids!(picked)).set_text(cx, &said);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/kbd/overview",
    category: "Data display",
    component: "Kbd",
    also: &["KbdFlat", "KbdGroup", "KbdGroupTight", "ShortcutHelp"],
    name: "Overview",
    dsl: "KbdOverview",
    added: "2026-09-10",
    tags: &["new", "keyboard", "shortcut", "chord", "key cap", "hotkey", "help", "cheatsheet"],
    doc: "# Kbd\n\nOne key cap: a glyph or a short word in a small rounded box. `Kbd` carries the side of the key under the face, `KbdFlat` is the face alone. `pressed` drops the face onto the side and takes the letter with it; the box keeps its size, so a row of caps never twitches when one goes down.\n\n## KbdGroup\n\nA whole chord from one string — `shortcut: \"ctrl+shift+P\"` — split on `+`, with the plus key itself written `\"ctrl++\"` or `\"ctrl+plus\"`. The modifiers are drawn first in a single order however they were typed, and the other keys follow in the order they were written, so a shortcut table written by several hands still reads as one table. A key this widget does not know is printed exactly as it was written rather than dropped: a missing cap with no explanation is worse than an odd-looking one. `separator` is what goes between the caps and `KbdGroupTight` leaves it out.\n\n## The names belong to the platform\n\nOne platform prints its modifiers on the keys as symbols; every other spells them as words, and the two do not even read them in the same order. The call site writes one string and each build turns it into the caps its own keyboard has on it — names and order both, chosen at compile time. Nothing at the call site changes between builds.\n\n## ShortcutHelp\n\nA list of chords and what they do. Each line of `entries` is either `\"# Heading\"` or `\"What it does = ctrl+c\"`; the FIRST `=` splits the line, so a chord may contain one of its own (`\"Zoom in = ctrl+=\"`). A line with no `=` is something the app can do that has no shortcut yet, which is worth listing and worth searching.\n\nThe search matches a row on its label or on the chord AS WRITTEN, so typing `ctrl` finds the row whether this build draws that cap as a word or as a symbol. A heading survives only with the first row that survives under it, so a filtered list never shows a group title with nothing beneath it; a query that matches the heading keeps the whole group. `search_height: 0` leaves the field out for a host that filters from its own box with `set_query`.\n\nA row answers a press with its label and its chord, for a list that also runs what it lists.\n\n## What these do not do\n\nThey do not bind anything, and they do not check that the host bound what they draw. A cap is a picture of a key: nothing here listens for the chord it shows, because binding belongs with the thing being bound — the menu, the command list — and a widget that both drew and bound would quietly become the place shortcuts are defined.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Cap", target: "subject", kind: ControlKind::Text { prop: "text", default: "K" } },
        Control { label: "Held down", target: "subject", kind: ControlKind::Bool { prop: "pressed", default: false } },
        Control { label: "Least width", target: "subject", kind: ControlKind::Number { prop: "min_width", min: 8., max: 80., step: 1., default: 22. } },
        Control { label: "Side padding", target: "subject", kind: ControlKind::Number { prop: "pad_x", min: 0., max: 24., step: 0.5, default: 7. } },
        Control { label: "Key side", target: "subject", kind: ControlKind::Number { prop: "shelf", min: 0., max: 8., step: 0.5, default: 2. } },
        Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.radius", min: 0., max: 12., step: 0.5, default: 4. } },
    ],
    on_actions: Some(kbd_actions),
}];
