//! The window chrome story: the menu bar and the caption buttons, the two
//! pieces of a window that are not part of its contents.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.WindowChromeOverview = StoryPage{
        StoryNote{text: "The furniture of a window rather than anything inside it: the menus along the top, and the glyphs at the far end of the caption that mind the window itself."}

        StoryHeading{text: "The menu bar"}
        StoryNote{text: "Menus are declared, not built: a list of labels, each with items carrying an id, a label and a shortcut. It raises the id of whatever was chosen. Click a menu, then walk it with the arrow keys."}
        StoryRow{
            View{
                width: Fill height: Fit
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                bar := MenuBar{
                    menus: [
                        {label: "File" items: [
                            {id: @new_doc label: "New" shortcut: "Cmd+N"}
                            {id: @open_doc label: "Open…" shortcut: "Cmd+O"}
                            {sep: true}
                            {id: @close_doc label: "Close" shortcut: "Cmd+W"}
                        ]}
                        {label: "Edit" items: [
                            {id: @undo label: "Undo" shortcut: "Cmd+Z"}
                            {id: @redo label: "Redo" shortcut: "Shift+Cmd+Z"}
                            {sep: true}
                            {id: @nothing label: "Nothing to paste" enabled: false}
                        ]}
                        {label: "View" items: [
                            {id: @zoom_in label: "Zoom in" shortcut: "Cmd+Plus"}
                            {id: @zoom_out label: "Zoom out" shortcut: "Cmd+Minus"}
                        ]}
                    ]
                }
            }
        }
        StoryRow{
            chosen := Label{text: "nothing chosen"}
        }

        StoryHeading{text: "The caption buttons"}
        StoryNote{text: "Each is one glyph drawn by a shader rather than a font or an icon file, so it stays crisp at any size and takes its colours from the caption around it. button_type picks which glyph."}
        StoryRow{
            View{
                width: Fit height: Fit flow: Right
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                minimise := DesktopButton{
                    draw_bg.button_type: DesktopButtonType.WindowsMin
                    width: 40. height: 26.
                }
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.WindowsMax
                    width: 40. height: 26.
                }
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.WindowsMaxToggled
                    width: 40. height: 26.
                }
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.WindowsClose
                    width: 40. height: 26.
                }
            }
            View{
                width: Fit height: Fit flow: Right
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.Fullscreen
                    width: 40. height: 26.
                }
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.XRMode
                    width: 40. height: 26.
                }
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.RecordOff
                    width: 40. height: 26.
                }
                DesktopButton{
                    draw_bg.button_type: DesktopButtonType.RecordOn
                    width: 40. height: 26.
                }
            }
        }
    }
}

fn window_chrome_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(id) = root.menu_bar(cx, ids!(bar)).selected(actions) {
        root.label(cx, ids!(chosen))
            .set_text(cx, &format!("chose {id}"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/windowchrome/overview",
    category: "Navigation",
    component: "WindowChrome",
    also: &["MenuBar", "DesktopButton"],
    name: "Overview",
    dsl: "WindowChromeOverview",
    added: "2025-05-06",
    tags: &[],
    doc: "# Window chrome

The furniture of a window rather than anything inside it.

## MenuBar

Menus are **declared, not built**: `menus` takes a list of `{label, items}`, and each item carries an `id`, a `label`, an optional `shortcut` and an optional `enabled: false`. `{sep: true}` draws a rule. It raises the `id` of whatever was chosen, so a host matches on ids rather than on positions.

`set_menus` replaces the lot at runtime and `set_enabled` greys one item, which is how a host keeps *Undo* dead until there is something to undo.

**A shortcut string is not the same key press everywhere.** `Cmd` means the logo modifier on macOS and Ctrl on every other platform — the same chord, a different key, resolved by the bar rather than by the caller. And a focused text editor takes an editing shortcut *before* the menu bar does, so Cmd+Z in a text field undoes the typing rather than the document.

## DesktopButton

One glyph each, drawn by a shader rather than loaded from a font or an SVG, so they stay crisp at any size and take their colours from the caption around them. `button_type` picks which: minimise, maximise, the toggled maximise, close, fullscreen, the XR mode, and a recording lamp with an off and an on face.

They are ordinary buttons otherwise — the window's own behaviour is the host's to wire.",
    subject: "minimise",
    feature: None,
    controls: &[],
    on_actions: Some(window_chrome_actions),
}];
