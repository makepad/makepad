//! The command palette story: everything the app can do, found by typing at
//! it, over an overlay that covers the window.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Caption = Label{
        draw_text +: {color: theme.color_text_meta}
    }

    mod.stories.CommandPaletteOverview = StoryPage{
        StoryNote{text: "A menu is a map — you learn where a thing lives and reach for the same place every time. A palette is the other trade: you say the name and it comes to you. It is the only way back to a command whose name you remember and whose home you have forgotten."}

        StoryHeading{text: "Open it and type"}
        StoryNote{text: "The search field has the keyboard from the moment it opens, so the first keystroke filters rather than being thrown away. The arrows move the cursor and wrap, Enter runs what the cursor is on, Escape closes it, and a press on the dark ground outside closes it too."}
        StoryRow{
            open_palette := Button{text: "Open the palette"}
            ran_note := Caption{text: "nothing run yet"}
        }

        StoryHeading{text: "Try these"}
        StoryNote{text: "`tfs` finds Toggle Full Screen — the letters may be anywhere as long as they are in that order. `sel` and `dup` land on the Edit group. `zoom` marks the whole word. `qqq` matches nothing, and Enter then does nothing at all rather than running whatever was nearest."}

        StoryHeading{text: "The marks are the letters you typed"}
        StoryNote{text: "Every alignment is scored and the best one is marked, not the first one that fits: `fa` against \"Frame All\" marks the F and the A, never the a in the middle of Frame. Marking the wrong letters makes a palette look as though it matched by accident."}

        StoryHeading{text: "Ranked and grouped at the same time"}
        StoryNote{text: "Ranking and grouping usually fight. Here the commands are ranked inside their group and the groups are ranked by their own best command, so the first row of the first group is the best match in the whole list — the cursor starts on it — and the sheet still reads as the sheet somebody wrote. With nothing typed every score is zero and the table appears exactly as written."}

        StoryHeading{text: "A palette takes no room"}
        StoryNote{text: "The palette below is declared between the two chips. It claims no width and no height in the row, so the row is laid out as though it were not there. One that asked for `Fill` would quietly take a share of every spare point in that row whether or not it was ever opened."}
        StoryRow{
            Chip{text: "before"}
            palette := CommandPalette{
                commands: [
                    "# File"
                    "New document = ctrl+n"
                    "Open recent = ctrl+shift+o"
                    "Save a copy = ctrl+shift+s"
                    "Export as image = ctrl+e"
                    "Close the window = ctrl+w"
                    "# Edit"
                    "Undo = ctrl+z"
                    "Redo = ctrl+shift+z"
                    "Duplicate selection = ctrl+d"
                    "Find and replace = ctrl+h"
                    "Select nothing = ctrl+shift+a"
                    "# View"
                    "Zoom to fit = ctrl+0"
                    "Toggle full screen = f11"
                    "Show the grid = ctrl+quote"
                    "Split the view"
                    "# Window"
                    "Next tab = ctrl+tab"
                    "Previous tab = ctrl+shift+tab"
                    "Reopen closed tab = ctrl+shift+t"
                ]
            }
            Chip{text: "after"}
        }

        StoryHeading{text: "More lines than it shows"}
        StoryNote{text: "The list windows to `max_rows` lines around the cursor and the keyboard walks it; there is no scrollbar, because a bar to drag is furniture for a gesture nobody makes in a palette. The table above is twenty-one lines and a heading counts as one of them, so holding Down walks the window through it."}

        StoryHeading{text: "A command with no shortcut"}
        StoryNote{text: "\"Split the view\" is written with no `=` at all. It is something the app can do that has no chord yet, which is worth listing and worth searching for; its row simply has no keys on the right."}

        StoryHeading{text: "One table, two widgets"}
        StoryNote{text: "The lines are the format `ShortcutHelp` reads, so the same text can be the sheet and the palette. The sheet is for browsing — it lists everything and searches by chord as well as by name. The palette is for reaching one thing fast. Both are below over the same lines."}
        StoryRow{
            width: Fill
            sheet := ShortcutHelp{
                width: Fill
                search_height: 0.
                entries: [
                    "# Playback"
                    "Play or pause = space"
                    "Back a frame = left"
                    "Forward a frame = right"
                    "Set an in point = i"
                    "Set an out point = o"
                ]
            }
        }
        StoryRow{
            open_flat := Button{text: "The same lines as a palette"}
            flat_note := Caption{text: "nothing run yet"}
            flat := CommandPalette{
                hint_text: ""
                top_margin: 140.
                panel_width: 380.
                commands: [
                    "# Playback"
                    "Play or pause = space"
                    "Back a frame = left"
                    "Forward a frame = right"
                    "Set an in point = i"
                    "Set an out point = o"
                ]
            }
        }
        StoryNote{text: "That one has `hint_text: \"\"`, which leaves the quiet line under the list out — right once a person knows the keys, wrong while they are still learning them."}
    }
}

/// The last thing a palette did, said in a label. Asked of the actions rather
/// than tracked: a palette closes for reasons a page never hears about.
fn report(
    cx: &mut Cx,
    root: &WidgetRef,
    actions: &Actions,
    palette: &CommandPaletteRef,
    note: &[LiveId],
) {
    if let Some((index, label)) = palette.ran(actions) {
        let chord = palette.ran_shortcut(actions).unwrap_or_default();
        let said = if chord.is_empty() {
            format!("ran command {index} \u{2013} {label}, which has no chord")
        } else {
            format!("ran command {index} \u{2013} {label} ({chord})")
        };
        root.label(cx, note).set_text(cx, &said);
    } else if palette.dismissed(actions) {
        root.label(cx, note).set_text(cx, "closed without running anything");
    }
}

fn palette_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let palette = root.command_palette(cx, ids!(palette));
    let flat = root.command_palette(cx, ids!(flat));
    if root.button(cx, ids!(open_palette)).clicked(actions) {
        palette.open(cx);
    }
    if root.button(cx, ids!(open_flat)).clicked(actions) {
        flat.open(cx);
    }
    report(cx, root, actions, &palette, ids!(ran_note));
    report(cx, root, actions, &flat, ids!(flat_note));
}

pub const STORIES: &[Story] = &[Story {
    key: "overlay/commandpalette/overview",
    category: "Overlay",
    component: "CommandPalette",
    also: &[],
    name: "Overview",
    dsl: "CommandPaletteOverview",
    added: "2026-09-10",
    tags: &["new", "overlay", "command", "palette", "fuzzy", "search", "keyboard", "shortcut", "quick open"],
    doc: "# CommandPalette

A search field over the commands an app can run, on an overlay that covers the window. A menu is a map — you learn where a thing lives and reach for the same place every time — and a palette is the other trade: you say the name and it comes to you. An app wants both, and this one is shaped so that the *same table of lines* can feed the palette and the shortcut sheet.

## The table

Each line of `commands` is either `\"# Heading\"` or `\"What it does = ctrl+p\"`, which is exactly what `ShortcutHelp` reads. The **first** `=` splits the line, so a chord may contain one of its own. A line with no `=` is something the app can do that has no chord yet — worth listing, and worth searching for.

Every row draws its chord as key caps through a `KbdGroup`, so somebody who arrived by typing leaves knowing the keys, and stops needing the palette for that command.

## Matching, and where the marks land

Typing narrows the list by a subsequence match: the letters may be anywhere as long as they are in that order, so `tfs` finds *Toggle Full Screen*. Every alignment is scored and the **best** one is marked, not the first one that fits — `fa` against *Frame All* marks the F and the A, never the a in the middle of *Frame*. A greedy scan is cheaper and marks the wrong letters, which makes the palette look as though it matched by accident.

Word starts score, runs of adjacent letters score, and a match near the front of a label beats the same match further in. A change of case counts as a word start too, so a label with no spaces in it still matches by its initials.

## Ranked and grouped at once

Ranking and grouping usually fight: rank the whole list and the groups interleave into nonsense, keep the groups and the best match is buried three headings down. Both are ranked here. Inside a group the commands are ordered by score; the groups are ordered by their own best command. The first row of the first group is therefore the best match in the whole list — the cursor starts on it and Enter runs it — and the sheet still reads as the sheet somebody wrote. A group that lost every command loses its heading with them. With nothing typed every score is zero, ties fall back to the written order, and the table appears exactly as written.

## Keyboard

| Key | Result |
|---|---|
| anything printable | filters, and the cursor returns to the best match |
| Up / Down | moves the cursor, wrapping |
| PageUp / PageDown | moves by a windowful, stopping at the ends |
| Enter | runs the command under the cursor; **nothing matched, nothing happens** |
| Escape | closes, and reports that it was dismissed |

Home and End are left to the caret. The query is a few characters long and the arrows wrap, so the list loses nothing by leaving a text field the keys a text field is expected to have.

The field has the keyboard from the moment the palette opens, so the first keystroke filters instead of being thrown away. It always opens with an empty query: one that reopened holding the last search would answer that first keystroke with the wrong list.

## Reading it

`ran` reports the command chosen — its ordinal **among the command rows as written**, which is what a host's own array is indexed by, and does not move when the ranking does — together with its label. `dismissed` reports a close with nothing run, so a host that keeps a toggle beside the palette can put it back.

## What it does not do

It does not bind the chord that opens it. `open`, `close` and `toggle` are called by the host from wherever it keeps its bindings; a widget that bound its own opener would quietly become the place that binding is defined, which is the last place anyone would look for it.

It does not run anything either — choosing a row raises an action and the host does the work. The palette has no idea what any of these lines mean.

It has no scrollbar: the list windows to `max_rows` lines around the cursor and the keyboard walks it. And it searches labels, not chords, because the marks have to land on the letters the person typed — searching by chord is what the shortcut sheet is for.

**A palette takes no room.** It is an overlay: the walk it reports to its parent is empty and the overlay itself is sized by the pass, which is why it can be declared in the middle of a row without moving anything in that row.",
    subject: "palette",
    feature: None,
    controls: &[
        Control { label: "Panel width", target: "palette", kind: ControlKind::Number { prop: "panel_width", min: 240., max: 900., step: 10., default: 520. } },
        Control { label: "Down from the top", target: "palette", kind: ControlKind::Number { prop: "top_margin", min: 0., max: 400., step: 4., default: 96. } },
        Control { label: "Lines shown", target: "palette", kind: ControlKind::Number { prop: "max_rows", min: 3., max: 30., step: 1., default: 12. } },
        Control { label: "Row height", target: "palette", kind: ControlKind::Number { prop: "row_height", min: 16., max: 48., step: 1., default: 26. } },
        Control { label: "Key cap height", target: "palette", kind: ControlKind::Number { prop: "cap_height", min: 12., max: 40., step: 1., default: 20. } },
        Control { label: "Nothing matched", target: "palette", kind: ControlKind::Text { prop: "empty_text", default: "No command matches" } },
        Control { label: "Dimming", target: "palette", kind: ControlKind::Number { prop: "draw_scrim.dim", min: 0., max: 1., step: 0.05, default: 0.55 } },
        Control { label: "Corner radius", target: "palette", kind: ControlKind::Number { prop: "draw_panel.radius", min: 0., max: 24., step: 0.5, default: 8. } },
    ],
    on_actions: Some(palette_actions),
}];
