# wm keyboard shortcuts — how they work

wm is Omarchy's tiling window manager rebuilt in makepad, and it keeps
Omarchy's keymap (read from `omarchy/default/hypr/bindings/*.lua`). Omarchy
builds every shortcut from one modifier, **SUPER** — the Logo key: the
Windows key on a PC keyboard, **⌘ Cmd** on a Mac — plus optional
Shift / Ctrl / Alt. wm accepts ⌘ as SUPER exactly like Linux, and also a
second spelling, **Ctrl+Alt**, for the few chords macOS keeps for itself
(⌘Tab, ⌘⇧3/4/5). If you still have Spotlight on ⌘Space, unbind it in System
Settings → Keyboard → Shortcuts, or use Ctrl+Alt+Space.

| Omarchy layer      | Press on macOS        | Fallback spelling        |
|--------------------|-----------------------|--------------------------|
| SUPER              | **⌘**                 | Ctrl+Alt                 |
| SUPER+SHIFT        | ⌘+**Shift**           | Ctrl+Alt+Shift           |
| SUPER+CTRL         | ⌘+**Ctrl**            | Ctrl+Alt+Cmd *(legacy)*  |
| SUPER+ALT          | ⌘+**Alt** (⌥ Option)  | Ctrl+Alt+**A**, then key |
| SUPER+SHIFT+CTRL   | ⌘+Shift+Ctrl          | Ctrl+Alt+Shift+Cmd       |
| SUPER+SHIFT+ALT    | ⌘+Shift+Alt           | —                        |
| ALT / ALT+SHIFT    | Alt / Alt+Shift       | (same on every OS)       |

Quitting wm itself is **⌘⇧Q** (the app menu) — plain ⌘Q closes the focused
window, as in Omarchy. Press **⌘K** at any time for the in-app cheat sheet;
it prints the exact chords for the OS you are on.

## Starting things

| Chord              | Does                                                             |
|--------------------|------------------------------------------------------------------|
| ⌘ **Return**       | New terminal, opened in the focused terminal's directory         |
| ⌘ **Space**        | The Omarchy menu (Apps, Learn, Trigger, Style, Setup, … System)  |
| ⌘⌥ **Space**       | Apps menu straight away (the launcher)                           |
| ⌘ **Esc**          | System menu (lock, suspend, logout, reboot, shutdown)            |
| ⌘ **K**            | Keybindings cheat sheet                                          |
| ⌘⇧⌃ **Space**      | Theme menu                                                       |
| ⌘⌃ **Space**       | Next wallpaper                                                   |
| ⌘⇧ **Space**       | Show / hide the top bar                                          |

With the mouse: the Omarchy button at the far left of the bar opens the menu
(right-click it for a new terminal); the focused window's title in the bar
focuses it on left-click and **closes it on middle- or right-click**.

Inside a menu: type to filter, **Enter** or **Right** activates, **Backspace**
or **Left** goes back a level (so does clicking the header), **Esc** clears
the filter and then closes; hover selects, click activates, click outside
closes.

## Moving around the tiles

New windows split the focused tile (dwindle layout: the longer side splits).

| Chord                      | Does                                          |
|----------------------------|-----------------------------------------------|
| ⌘ **Arrows**               | Focus the window left / right / up / down     |
| ⌘⇧ **Arrows**              | Swap the window in that direction             |
| Alt+**Tab** / Alt+Shift+Tab| Next / previous window                        |
| ⌘ **J**                    | Toggle the split direction of the focused pair|
| ⌘ **W** or **Q**           | Close the window                              |
| Ctrl+Alt+**Delete**        | Close all windows, back to workspace 1        |
| ⌘ **L**                    | Toggle the workspace layout (dwindle / scrolling) |

Mouse: ⌘ + left-drag moves a window (a tiled one swaps with the tile you
drop it on), ⌘ + right-drag resizes it from the corner nearest your grab.

## Sizing

| Chord                   | Does                                  |
|-------------------------|---------------------------------------|
| ⌘ **-** / **=**         | Move the nearest divider left / right by 100 px |
| ⌘⇧ **-** / **=**        | …up / down by 100 px                  |
| ⌘⌥ **-** / **=**        | …by 25 px                             |
| ⌘⇧⌥ **-** / **=**       | …by 25 px vertically                  |
| ⌘⌃ **-** / **=**        | …by 300 px                            |
| ⌘⇧⌃ **-** / **=**       | …by 300 px vertically                 |

## Window modes

| Chord     | Does                                                              |
|-----------|-------------------------------------------------------------------|
| ⌘ **F**   | Full screen (the window covers the desk, bar hidden)              |
| ⌘⌥ **F**  | Full width (maximized inside the gaps, bar stays)                 |
| ⌘⌃ **F**  | Tiled full screen (the app is told it is fullscreen, tile stays)  |
| ⌘ **T**   | Toggle floating / tiled                                           |
| ⌘ **O**   | Pop the window out: float it at 1300×900, center, pin on top on every workspace; again to tile it back |
| ⌘ **P**   | Pseudo-tile: keep the app's own size centered inside its tile     |

## Workspaces and the scratchpad

| Chord                      | Does                                                |
|----------------------------|-----------------------------------------------------|
| ⌘ **1**…**0**              | Go to workspace 1…10                                |
| ⌘⇧ **1**…**0**             | Move the window there and follow it                 |
| ⌘⇧⌥ **1**…**0**            | Move it there silently (stay where you are)         |
| ⌘ **Tab** / ⌘⇧ Tab         | Next / previous workspace                           |
| ⌘⌃ **Tab**                 | The workspace you came from                         |
| ⌘ + mouse wheel            | Scroll through workspaces                           |
| ⌘ **S** or **`**           | Show / hide the scratchpad (a drop-down console over the top half) |
| ⌘⌥ **S**, ⌘⇧ **`**         | Send the window to the scratchpad                   |

## Groups (tabbed windows)

| Chord                    | Does                                              |
|--------------------------|---------------------------------------------------|
| ⌘ **G**                  | Turn the window into a group (tabs) / dissolve it |
| ⌘⌥ **Arrows**            | Move the window into the group next to it         |
| ⌘⌥ **G**                 | Move the window out of its group                  |
| ⌘⌥ **Tab** / ⌘⇧⌥ Tab     | Next / previous tab in the group                  |
| ⌘⌃ **Left/Right**        | Same, Omarchy's other spelling                    |
| ⌘⌥ **1**…**5**           | Jump to the nth tab                               |

## When a chord does nothing

macOS swallowed it. Use the Ctrl+Alt spelling, or for the SUPER+ALT layer
press **Ctrl+Alt+A**, let go, then press the key. Everything above is
generated from `apps/wm/src/binds.rs`; the cheat sheet (⌘K) is always the
authoritative list.
