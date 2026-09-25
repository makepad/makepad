//! The hotkey stories: the keymap as an editable list, and the keyboard
//! drawn whole with what each key does.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Caption = Label{
        draw_text +: {color: theme.color_text_meta}
    }

    mod.stories.HotkeyEditorOverview = StoryPage{
        StoryNote{text: "Every command the app can run from the keyboard, with the chord it answers to now, the chord it shipped with, and a Reset where the two differ. The list is the app's one keymap, so a change made here is a change everywhere that keymap is read."}

        StoryHeading{text: "Bind a chord"}
        StoryNote{text: "Press a chord cell and it listens: hold the modifiers, press the key, and that is the new binding. The cell shows the modifiers as they go down. Escape cancels, Backspace clears the binding. While a cell listens nothing else runs, so pressing the old chord of another command only binds it."}
        StoryRow{
            bind_save := Button{text: "Bind Save\u{2026}"}
            changed_note := Caption{text: "nothing changed yet"}
        }
        StoryRow{
            width: Fill
            subject := HotkeyEditor{
                hotkeys: [
                    {id: @save label: "Save" chord: "Mod+S"}
                    {id: @save_as label: "Save as" chord: "Mod+Shift+S"}
                    {id: @open label: "Open" chord: "Mod+O"}
                    {id: @go_to_file label: "Go to file" chord: "Mod+P"}
                    {id: @print label: "Print" chord: "Mod+P"}
                    {id: @find label: "Find" chord: "Mod+F"}
                    {id: @find_in_notes label: "Find in notes" chord: "Mod+F" scope: @notes}
                    {id: @command_list label: "Command list" chord: "Mod+Shift+P"}
                    {id: @toggle_sidebar label: "Toggle sidebar" chord: "Mod+B"}
                    {id: @zoom_in label: "Zoom in" chord: "Mod+="}
                    {id: @zoom_out label: "Zoom out" chord: "Mod+-"}
                    {id: @full_screen label: "Full screen" chord: "F11"}
                    {id: @quick_note label: "Quick note" chord: "N"}
                    {id: @split label: "Split the view"}
                ]
            }
        }

        StoryHeading{text: "Clashes are marked, not refused"}
        StoryNote{text: "Go to file and Print ship on the same chord. Both rows carry the mark and the cells are outlined; rest the pointer on the mark to see what else the chord runs. A clash is allowed on purpose: swapping two chords passes through one."}

        StoryHeading{text: "Which command a key press means"}
        StoryNote{text: "Press a chord anywhere on this page and the line below names the command it resolves to. Find in notes is bound to the same chord as Find but only inside the notes field: with the caret there, Mod+F means Find in notes. While the caret is in the field the keys that type are the field's, so N types an n instead of running Quick note — and Mod+S still saves."}
        StoryRow{
            notes := TextInput{width: 260 empty_text: "notes (a focus scope)"}
            resolved_note := Caption{text: "no chord pressed yet"}
        }
        StoryRow{
            width: Fill
            map := KeyboardMap{
                show_navigation: false
            }
        }
        StoryNote{text: "The keyboard lights the chord of the row under the pointer."}
    }

    mod.stories.KeyboardMapOverview = StoryPage{
        StoryNote{text: "The keyboard drawn whole, with what each key does on it. A list answers \"what is the chord for this command\"; the map answers the question the other way round — what does this key do, and which keys are still free."}

        StoryHeading{text: "Tints, bars and held keys"}
        StoryNote{text: "A bound key is tinted by the modifiers of its binding and carries one bar per modifier set bound on it. Hold a modifier and the map narrows to the bindings made with exactly the modifiers held. The keys you hold light up. Rest the pointer on a key for its bindings; press it to name them below."}
        StoryRow{
            width: Fill
            subject := KeyboardMap{
                show_numpad: true
                hotkeys: [
                    {id: @save label: "Save" chord: "Mod+S"}
                    {id: @save_as label: "Save as" chord: "Mod+Shift+S"}
                    {id: @open label: "Open" chord: "Mod+O"}
                    {id: @go_to_file label: "Go to file" chord: "Mod+P"}
                    {id: @print label: "Print" chord: "Mod+P"}
                    {id: @find label: "Find" chord: "Mod+F"}
                    {id: @command_list label: "Command list" chord: "Mod+Shift+P"}
                    {id: @toggle_sidebar label: "Toggle sidebar" chord: "Mod+B"}
                    {id: @zoom_in label: "Zoom in" chord: "Mod+="}
                    {id: @zoom_out label: "Zoom out" chord: "Mod+-"}
                    {id: @full_screen label: "Full screen" chord: "F11"}
                    {id: @quick_note label: "Quick note" chord: "N"}
                    {id: @next_tab label: "Next tab" chord: "Ctrl+Tab"}
                    {id: @word_back label: "Word back" chord: "Alt+Left"}
                ]
            }
        }
        StoryRow{
            key_note := Caption{text: "no key pressed yet"}
        }

        StoryHeading{text: "ISO"}
        StoryNote{text: "`iso: true` draws the ISO main block: the tall Enter over two rows and the extra key beside the left Shift."}
        StoryRow{
            width: Fill
            KeyboardMap{
                iso: true
                show_navigation: false
                show_legend: false
                unit: 28.
            }
        }
    }
}

/// The label of a command, from the app's keymap.
fn label_of(cx: &mut Cx, id: LiveId) -> String {
    cx.global::<Hotkeys>()
        .get(id)
        .map(|h| h.label.clone())
        .unwrap_or_else(|| id.to_string())
}

fn editor_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let editor = root.hotkey_editor(cx, ids!(subject));
    let map = root.keyboard_map(cx, ids!(map));

    // The notes field is the `notes` focus scope; tying it to its area
    // every pass keeps the area current.
    let notes = root.widget(cx, ids!(notes)).area();
    cx.global::<Hotkeys>().set_scope_area(live_id!(notes), notes);

    if root.button(cx, ids!(bind_save)).clicked(actions) {
        editor.start_capture(cx, live_id!(save));
    }
    for (id, chord) in editor.changes(actions) {
        let label = label_of(cx, id);
        let text = match chord {
            Some(chord) => format!("{label} is now {}", chord.format()),
            None => format!("{label} is unbound"),
        };
        root.label(cx, ids!(changed_note)).set_text(cx, &text);
    }
    if let Some(row) = editor.hovered(actions) {
        let chord = row.and_then(|id| cx.global::<Hotkeys>().bindings_for(id));
        map.set_highlight(cx, chord);
    }
    if let Some(id) = map.resolved(actions) {
        let text = format!("that ran {}", label_of(cx, id));
        root.label(cx, ids!(resolved_note)).set_text(cx, &text);
    }
}

fn map_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let map = root.keyboard_map(cx, ids!(subject));
    if let Some(code) = map.key_clicked(actions) {
        let name = key_cap_label(code, KeyPlatform::current());
        let bound: Vec<String> = cx
            .global::<Hotkeys>()
            .bindings_on_key(code)
            .into_iter()
            .map(|h| {
                let chord = h.chord.map(|c| c.format()).unwrap_or_default();
                format!("{} ({chord})", h.label)
            })
            .collect();
        let text = if bound.is_empty() {
            format!("{name} is free")
        } else {
            format!("{name}: {}", bound.join(", "))
        };
        root.label(cx, ids!(key_note)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "inputs/hotkeys/hotkey-editor",
        category: "Inputs",
        component: "Hotkeys",
        also: &["HotkeyEditor", "KeyboardMap"],
        name: "Hotkey editor",
        dsl: "HotkeyEditorOverview",
        added: "2026-09-25",
        tags: &["new", "keyboard", "hotkey", "shortcut", "keymap", "rebind", "chord", "conflict"],
        doc: "# HotkeyEditor\n\nThe app's keymap as rows of command, current chord (drawn as key caps), default chord and Reset. The rows are the `Hotkeys` registry held on the `Cx` (`cx.global::<Hotkeys>()`), read on every draw, so a change made anywhere shows here.\n\n## Declaring commands\n\n```\nHotkeyEditor{\n    hotkeys: [\n        {id: @save label: \"Save\" chord: \"Mod+S\"}\n        {id: @find_in_notes label: \"Find in notes\" chord: \"Mod+F\" scope: @notes}\n    ]\n}\n```\n\nor from Rust with `Hotkeys::register`. `Mod` is Command on Apple and Control elsewhere; `Cmd` stands in for Control where there is no Command key; `Meta`/`Super`/`Win` name the logo key. Chords print as `Cmd+Shift+P` on Apple and `Ctrl+Shift+P` elsewhere.\n\n## Binding\n\nPress a chord cell and it listens for the next key combination. Escape cancels, Backspace or Delete clears, moving the focus away cancels. While it listens the registry resolves nothing. A clash is kept and marked on both rows; the mark's tip names the other command. Reset puts one row back, Reset all every row, and Undo takes back the last change (the registry keeps the replaced chord). Every change arrives as `HotkeyEditorAction::Changed(id, chord)`.\n\n## Routing\n\n`Hotkeys::resolve` decides what a key press means: a binding scoped to the focused part of the window beats a global one on the same chord, and while a text field has the caret the chords that type or edit text are the field's. Saving is `Hotkeys::save` / `load` through `cx.storage`, as `id=chord` lines of what the person changed.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Row height", target: "subject", kind: ControlKind::Number { prop: "row_height", min: 20., max: 56., step: 1., default: 30. } },
            Control { label: "Chord column", target: "subject", kind: ControlKind::Number { prop: "chord_width", min: 80., max: 320., step: 1., default: 190. } },
            Control { label: "Default column", target: "subject", kind: ControlKind::Number { prop: "default_width", min: 0., max: 240., step: 1., default: 130. } },
            Control { label: "Headings", target: "subject", kind: ControlKind::Number { prop: "header_height", min: 0., max: 40., step: 1., default: 22. } },
        ],
        on_actions: Some(editor_actions),
    },
    Story {
        key: "inputs/hotkeys/keyboard-map",
        category: "Inputs",
        component: "Hotkeys",
        also: &["KeyboardMap"],
        name: "Keyboard map",
        dsl: "KeyboardMapOverview",
        added: "2026-09-25",
        tags: &["new", "keyboard", "hotkey", "shortcut", "keymap", "layout", "ansi", "iso"],
        doc: "# KeyboardMap\n\nThe keyboard drawn whole from layout tables — the main block in ANSI or ISO shape, the function row, the navigation cluster and the number pad, widths in key units — with the app's bindings on it.\n\n* A bound key is tinted by the modifiers of its binding (`tint_plain`, `tint_ctrl`, `tint_alt`, `tint_shift`, `tint_logo`; several modifiers mix) and carries one bar per modifier set bound on it.\n* Holding modifiers narrows the map to the bindings made with exactly those (`follow_modifiers`).\n* Held keys light up; the key under the pointer lights up and its bindings come up in a tip.\n* `set_highlight(chord)` lights a chord for a host, as the hotkey editor page does for the row under the pointer.\n\nPressing a key raises `KeyboardMapAction::KeyClicked(code)`. A key press anywhere in the window that resolves to a command raises `KeyboardMapAction::Resolved(id)`.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "ISO", target: "subject", kind: ControlKind::Bool { prop: "iso", default: false } },
            Control { label: "Function row", target: "subject", kind: ControlKind::Bool { prop: "show_function_row", default: true } },
            Control { label: "Navigation", target: "subject", kind: ControlKind::Bool { prop: "show_navigation", default: true } },
            Control { label: "Number pad", target: "subject", kind: ControlKind::Bool { prop: "show_numpad", default: true } },
            Control { label: "Follow modifiers", target: "subject", kind: ControlKind::Bool { prop: "follow_modifiers", default: true } },
            Control { label: "Key unit", target: "subject", kind: ControlKind::Number { prop: "unit", min: 16., max: 64., step: 1., default: 34. } },
            Control { label: "Key gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 8., step: 0.5, default: 3. } },
        ],
        on_actions: Some(map_actions),
    },
];
