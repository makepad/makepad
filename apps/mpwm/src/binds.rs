//! The Omarchy keymap (`default/hypr/bindings/*.lua`, read from source),
//! carried over binding-for-binding where the host OS allows.
//!
//! SUPER mapping: the Logo key is the direct spelling on every desktop.
//! When mpwm is nested inside another window manager, that manager commonly
//! consumes Logo-key chords first, so **Ctrl+Alt** is accepted as a fallback;
//! Omarchy's SUPER+CTRL layer then maps to **Ctrl+Alt+Logo**.
//!
//! That leaves Omarchy's SUPER+ALT layer with no bits of its own —
//! `KeyModifiers` carries exactly four (shift/control/alt/logo) and SUPER
//! has eaten two of them. Two ways in, both live:
//!
//! 1. **Cmd+Alt** (and Cmd+Alt+Shift for SUPER+SHIFT+ALT). Stateless, and
//!    it keeps the SUPER and SUPER+CTRL laws untouched. macOS eats a couple
//!    of these itself (Cmd+Alt+Space is Finder search, Cmd+Alt+D the dock),
//!    which is what the second way is for.
//! 2. **SUPER+A as a one-shot prefix**: press it and the *next* key is read
//!    on the SUPER+ALT layer. SUPER+A is unbound in every Omarchy binding
//!    file, so nothing is lost.
//!
//! The cheat sheet (SUPER+K) renders the real combos for the current OS.

use crate::layout::{Axis, Dir, FullscreenMode};
use makepad_widgets::*;

#[derive(Clone, Debug, PartialEq)]
pub enum WmAction {
    // Applications (bindings/applications.lua).
    LaunchTerminal,
    /// SUPER+SHIFT+RETURN / SUPER+SHIFT+B (applications.lua) —
    /// launch-or-focus the browser.
    LaunchBrowser,
    // Window management (bindings/tiling.lua).
    CloseWindow,
    CloseAllWindows,
    ToggleSplit,
    TogglePseudo,
    ToggleFloat,
    PopOut,
    Fullscreen(FullscreenMode),
    /// SUPER+CTRL+F — `fullscreenstate 0 2`: report-only, no layout change.
    TiledFullscreen,
    FocusDir(Dir),
    SwapDir(Dir),
    /// Move the nearest divider on `axis` by `px` (positive = right/down).
    ResizePx {
        axis: Axis,
        px: f64,
    },
    CycleFocus(bool),
    // Groups.
    ToggleGroup,
    MoveOutOfGroup,
    MoveIntoGroup(Dir),
    GroupNext,
    GroupPrev,
    GroupActive(usize),
    // Workspaces.
    Workspace(usize),
    MoveToWorkspace(usize),
    MoveToWorkspaceSilent(usize),
    WorkspaceNext,
    WorkspacePrev,
    WorkspaceFormer,
    ToggleScratchpad,
    MoveToScratchpad,
    /// SUPER+L — `omarchy-hyprland-workspace-layout-toggle`: flip this
    /// workspace between dwindle and the side-scrolling layout.
    ToggleWorkspaceLayout,
    // Utilities (bindings/utilities.lua).
    Menu,
    AppsMenu,
    SystemMenu,
    /// A jsonc route opened straight from a keybinding
    /// (`omarchy-menu toggle capture` and friends).
    MenuRoute(&'static str),
    Keybindings,
    ToggleBar,
    ThemeMenu,
    BackgroundNext,
    /// Nested-mode only: arm the SUPER+ALT layer for the next key.
    ArmAltLayer,
}

/// Which Omarchy modifier layer a bind lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Super,
    SuperShift,
    SuperCtrl,
    SuperAlt,
    SuperShiftCtrl,
    SuperShiftAlt,
    /// Plain Alt (ALT+TAB window cycling — the same on every OS).
    Alt,
    AltShift,
    /// CTRL+ALT+DELETE (close all windows). Nested on macOS this is the
    /// same chord as SUPER, which is harmless: Delete is bound nowhere else.
    CtrlAlt,
}

/// The layer expressed as Omarchy's own modifier bits on top of SUPER.
struct LayerBits {
    shift: bool,
    ctrl: bool,
    alt: bool,
}

impl Layer {
    fn bits(self) -> LayerBits {
        let (shift, ctrl, alt) = match self {
            Layer::Super => (false, false, false),
            Layer::SuperShift => (true, false, false),
            Layer::SuperCtrl => (false, true, false),
            Layer::SuperAlt => (false, false, true),
            Layer::SuperShiftCtrl => (true, true, false),
            Layer::SuperShiftAlt => (true, false, true),
            Layer::Alt | Layer::AltShift | Layer::CtrlAlt => (false, false, false),
        };
        LayerBits { shift, ctrl, alt }
    }
}

pub struct Bind {
    pub layer: Layer,
    pub key: KeyCode,
    pub action: WmAction,
    pub help: &'static str,
}

pub fn keymap() -> Vec<Bind> {
    use KeyCode::*;
    use Layer::{
        Alt, AltShift, CtrlAlt, Super, SuperAlt, SuperCtrl, SuperShift, SuperShiftAlt,
        SuperShiftCtrl,
    };
    let mut binds = vec![
        // ---- applications.lua
        Bind { layer: Super, key: ReturnKey, action: WmAction::LaunchTerminal, help: "Terminal" },
        Bind { layer: SuperShift, key: ReturnKey, action: WmAction::LaunchBrowser, help: "Browser" },
        Bind { layer: SuperShift, key: KeyB, action: WmAction::LaunchBrowser, help: "Browser" },
        // ---- tiling.lua
        Bind { layer: Super, key: KeyW, action: WmAction::CloseWindow, help: "Close window" },
        Bind { layer: Super, key: KeyQ, action: WmAction::CloseWindow, help: "Close window" },
        Bind { layer: CtrlAlt, key: Delete, action: WmAction::CloseAllWindows, help: "Close all windows" },
        Bind { layer: Super, key: KeyJ, action: WmAction::ToggleSplit, help: "Toggle window split" },
        Bind { layer: Super, key: KeyP, action: WmAction::TogglePseudo, help: "Pseudo window" },
        Bind { layer: Super, key: KeyT, action: WmAction::ToggleFloat, help: "Toggle window floating/tiling" },
        Bind { layer: Super, key: KeyF, action: WmAction::Fullscreen(FullscreenMode::Fullscreen), help: "Full screen" },
        Bind { layer: SuperCtrl, key: KeyF, action: WmAction::TiledFullscreen, help: "Tiled full screen" },
        Bind { layer: SuperAlt, key: KeyF, action: WmAction::Fullscreen(FullscreenMode::Maximized), help: "Full width" },
        Bind { layer: Super, key: KeyO, action: WmAction::PopOut, help: "Pop window out (float & pin)" },
        Bind { layer: Super, key: ArrowLeft, action: WmAction::FocusDir(Dir::Left), help: "Focus on left window" },
        Bind { layer: Super, key: ArrowRight, action: WmAction::FocusDir(Dir::Right), help: "Focus on right window" },
        Bind { layer: Super, key: ArrowUp, action: WmAction::FocusDir(Dir::Up), help: "Focus on above window" },
        Bind { layer: Super, key: ArrowDown, action: WmAction::FocusDir(Dir::Down), help: "Focus on below window" },
        Bind { layer: Super, key: KeyS, action: WmAction::ToggleScratchpad, help: "Toggle scratchpad" },
        Bind { layer: SuperAlt, key: KeyS, action: WmAction::MoveToScratchpad, help: "Move window to scratchpad" },
        Bind { layer: Super, key: KeyL, action: WmAction::ToggleWorkspaceLayout, help: "Toggle workspace layout" },
        Bind { layer: Super, key: Backtick, action: WmAction::ToggleScratchpad, help: "Toggle scratchpad" },
        Bind { layer: SuperShift, key: Backtick, action: WmAction::MoveToScratchpad, help: "Move window to scratchpad" },
        Bind { layer: Super, key: Tab, action: WmAction::WorkspaceNext, help: "Next workspace" },
        Bind { layer: SuperShift, key: Tab, action: WmAction::WorkspacePrev, help: "Previous workspace" },
        Bind { layer: SuperCtrl, key: Tab, action: WmAction::WorkspaceFormer, help: "Former workspace" },
        Bind { layer: SuperShift, key: ArrowLeft, action: WmAction::SwapDir(Dir::Left), help: "Swap window to the left" },
        Bind { layer: SuperShift, key: ArrowRight, action: WmAction::SwapDir(Dir::Right), help: "Swap window to the right" },
        Bind { layer: SuperShift, key: ArrowUp, action: WmAction::SwapDir(Dir::Up), help: "Swap window up" },
        Bind { layer: SuperShift, key: ArrowDown, action: WmAction::SwapDir(Dir::Down), help: "Swap window down" },
        Bind { layer: Alt, key: Tab, action: WmAction::CycleFocus(true), help: "Focus on next window" },
        Bind { layer: AltShift, key: Tab, action: WmAction::CycleFocus(false), help: "Focus on previous window" },
        // Omarchy: SUPER+code:20/21 = minus/equals, resize({x|y = ±N}).
        Bind { layer: Super, key: Minus, action: WmAction::ResizePx { axis: Axis::Horizontal, px: -100.0 }, help: "Expand window left" },
        Bind { layer: Super, key: Equals, action: WmAction::ResizePx { axis: Axis::Horizontal, px: 100.0 }, help: "Shrink window left" },
        Bind { layer: SuperShift, key: Minus, action: WmAction::ResizePx { axis: Axis::Vertical, px: -100.0 }, help: "Shrink window up" },
        Bind { layer: SuperShift, key: Equals, action: WmAction::ResizePx { axis: Axis::Vertical, px: 100.0 }, help: "Expand window down" },
        Bind { layer: SuperAlt, key: Minus, action: WmAction::ResizePx { axis: Axis::Horizontal, px: -25.0 }, help: "Expand window left a little" },
        Bind { layer: SuperAlt, key: Equals, action: WmAction::ResizePx { axis: Axis::Horizontal, px: 25.0 }, help: "Shrink window left a little" },
        Bind { layer: SuperShiftAlt, key: Minus, action: WmAction::ResizePx { axis: Axis::Vertical, px: -25.0 }, help: "Shrink window up a little" },
        Bind { layer: SuperShiftAlt, key: Equals, action: WmAction::ResizePx { axis: Axis::Vertical, px: 25.0 }, help: "Expand window down a little" },
        Bind { layer: SuperCtrl, key: Minus, action: WmAction::ResizePx { axis: Axis::Horizontal, px: -300.0 }, help: "Expand window left a lot" },
        Bind { layer: SuperCtrl, key: Equals, action: WmAction::ResizePx { axis: Axis::Horizontal, px: 300.0 }, help: "Shrink window left a lot" },
        Bind { layer: SuperShiftCtrl, key: Minus, action: WmAction::ResizePx { axis: Axis::Vertical, px: -300.0 }, help: "Shrink window up a lot" },
        Bind { layer: SuperShiftCtrl, key: Equals, action: WmAction::ResizePx { axis: Axis::Vertical, px: 300.0 }, help: "Expand window down a lot" },
        // Groups.
        Bind { layer: Super, key: KeyG, action: WmAction::ToggleGroup, help: "Toggle window grouping" },
        Bind { layer: SuperAlt, key: KeyG, action: WmAction::MoveOutOfGroup, help: "Move active window out of group" },
        Bind { layer: SuperAlt, key: ArrowLeft, action: WmAction::MoveIntoGroup(Dir::Left), help: "Move window to group on left" },
        Bind { layer: SuperAlt, key: ArrowRight, action: WmAction::MoveIntoGroup(Dir::Right), help: "Move window to group on right" },
        Bind { layer: SuperAlt, key: ArrowUp, action: WmAction::MoveIntoGroup(Dir::Up), help: "Move window to group on top" },
        Bind { layer: SuperAlt, key: ArrowDown, action: WmAction::MoveIntoGroup(Dir::Down), help: "Move window to group on bottom" },
        Bind { layer: SuperAlt, key: Tab, action: WmAction::GroupNext, help: "Next window in group" },
        Bind { layer: SuperShiftAlt, key: Tab, action: WmAction::GroupPrev, help: "Previous window in group" },
        Bind { layer: SuperCtrl, key: ArrowLeft, action: WmAction::GroupPrev, help: "Move grouped window focus left" },
        Bind { layer: SuperCtrl, key: ArrowRight, action: WmAction::GroupNext, help: "Move grouped window focus right" },
        // ---- utilities.lua
        Bind { layer: Super, key: Space, action: WmAction::Menu, help: "Omarchy menu" },
        Bind { layer: SuperAlt, key: Space, action: WmAction::AppsMenu, help: "Apps menu" },
        Bind { layer: Super, key: Escape, action: WmAction::SystemMenu, help: "System menu" },
        Bind { layer: Super, key: KeyK, action: WmAction::Keybindings, help: "Keybindings" },
        Bind { layer: SuperCtrl, key: KeyC, action: WmAction::MenuRoute("trigger.capture"), help: "Capture menu" },
        Bind { layer: SuperCtrl, key: KeyO, action: WmAction::MenuRoute("trigger.toggle"), help: "Toggle menu" },
        Bind { layer: SuperCtrl, key: KeyH, action: WmAction::MenuRoute("trigger.hardware"), help: "Hardware menu" },
        Bind { layer: SuperShift, key: Space, action: WmAction::ToggleBar, help: "Toggle top bar" },
        Bind { layer: SuperCtrl, key: Space, action: WmAction::BackgroundNext, help: "Background switcher" },
        Bind { layer: SuperShiftCtrl, key: Space, action: WmAction::ThemeMenu, help: "Theme menu" },
        // ---- nested-mode helper (see the module note).
        Bind { layer: Super, key: KeyA, action: WmAction::ArmAltLayer, help: "Arm the SUPER+ALT layer for one key" },
    ];
    // SUPER+1..0 workspaces; +SHIFT moves the window along, +SHIFT+ALT
    // moves it silently. SUPER+ALT+1..5 selects the nth group member.
    let digits = [
        KeyCode::Key1,
        KeyCode::Key2,
        KeyCode::Key3,
        KeyCode::Key4,
        KeyCode::Key5,
        KeyCode::Key6,
        KeyCode::Key7,
        KeyCode::Key8,
        KeyCode::Key9,
        KeyCode::Key0,
    ];
    for (i, key) in digits.into_iter().enumerate() {
        binds.push(Bind {
            layer: Layer::Super,
            key,
            action: WmAction::Workspace(i),
            help: "Switch to workspace N",
        });
        binds.push(Bind {
            layer: Layer::SuperShift,
            key,
            action: WmAction::MoveToWorkspace(i),
            help: "Move window to workspace N",
        });
        binds.push(Bind {
            layer: Layer::SuperShiftAlt,
            key,
            action: WmAction::MoveToWorkspaceSilent(i),
            help: "Move window silently to workspace N",
        });
        if i < 5 {
            binds.push(Bind {
                layer: Layer::SuperAlt,
                key,
                action: WmAction::GroupActive(i + 1),
                help: "Switch to group window N",
            });
        }
    }
    binds
}

/// How SUPER is spelled on this keyboard. Every desktop accepts both: the
/// Logo key (⌘ on macOS) and Ctrl+Alt, which is the way in for chords the
/// host desktop or compositor keeps for itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spelling {
    /// SUPER = the Logo key; the other three modifiers mean themselves.
    Logo,
    /// SUPER = Ctrl+Alt; Cmd carries the CTRL bit and the SUPER+A prefix
    /// carries the ALT bit.
    CtrlAlt,
}

/// Every spelling this OS understands, best first.
///
/// Linux needs the Ctrl+Alt spelling too when mpwm is nested inside a real
/// desktop compositor: Hyprland consumes its SUPER bindings before the key
/// event can reach mpwm. Keeping Logo first preserves direct/session use,
/// while Ctrl+Alt provides an unclaimed way to operate the inner WM.
pub fn spellings() -> &'static [Spelling] {
    &[Spelling::Logo, Spelling::CtrlAlt]
}

/// True when the event's modifiers form the given layer, read in one
/// spelling. `alt_armed` is the one-shot SUPER+A prefix.
fn layer_matches(layer: Layer, m: &KeyModifiers, alt_armed: bool, spelling: Spelling) -> bool {
    let (sup, ctrl_bit, alt_bit) = match spelling {
        Spelling::Logo => (m.logo, m.control, m.alt),
        Spelling::CtrlAlt => {
            let ctrl_alt = m.control && m.alt;
            (ctrl_alt, m.logo, ctrl_alt && alt_armed)
        }
    };

    match layer {
        Layer::Alt => m.alt && !m.control && !m.logo && !m.shift,
        Layer::AltShift => m.alt && !m.control && !m.logo && m.shift,
        Layer::CtrlAlt => m.control && m.alt && !m.logo && !m.shift,
        other => {
            let want = other.bits();
            sup && m.shift == want.shift && ctrl_bit == want.ctrl && alt_bit == want.alt
        }
    }
}

/// A key's name for the cheat sheet.
pub fn key_text(key: KeyCode) -> String {
    use KeyCode::*;
    let s = match key {
        ReturnKey => "Return",
        Space => "Space",
        Tab => "Tab",
        Escape => "Esc",
        Delete => "Delete",
        Backspace => "Backspace",
        Backtick => "`",
        Minus => "-",
        Equals => "=",
        ArrowLeft => "Left",
        ArrowRight => "Right",
        ArrowUp => "Up",
        ArrowDown => "Down",
        other => {
            let raw = format!("{:?}", other);
            return raw
                .strip_prefix("Key")
                .map(|s| s.to_string())
                .unwrap_or(raw);
        }
    };
    s.to_string()
}

/// The modifier prefix of a layer in one spelling.
fn prefix_text(layer: Layer, spelling: Spelling) -> String {
    let (sup, ctrl, alt) = match spelling {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        Spelling::Logo => ("Cmd", "Ctrl", "Cmd+Alt"),
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Spelling::Logo => ("Super", "Ctrl", "Super+Alt"),
        Spelling::CtrlAlt => ("Ctrl+Alt", "Cmd", "Cmd+Alt"),
    };
    match layer {
        Layer::Super => sup.to_string(),
        Layer::SuperShift => format!("{}+Shift", sup),
        Layer::SuperCtrl => format!("{}+{}", sup, ctrl),
        Layer::SuperAlt => alt.to_string(),
        Layer::SuperShiftCtrl => format!("{}+Shift+{}", sup, ctrl),
        Layer::SuperShiftAlt => format!("{}+Shift", alt),
        Layer::Alt => "Alt".to_string(),
        Layer::AltShift => "Alt+Shift".to_string(),
        Layer::CtrlAlt => "Ctrl+Alt".to_string(),
    }
}

/// Human-readable combo for the cheat sheet: the ⌘ spelling first, the
/// Ctrl+Alt fallback in parentheses where they differ.
pub fn combo_text(bind: &Bind) -> String {
    let key = key_text(bind.key);
    let first = format!("{}+{}", prefix_text(bind.layer, Spelling::Logo), key);
    if spellings().len() < 2 {
        return first;
    }
    let second = format!("{}+{}", prefix_text(bind.layer, Spelling::CtrlAlt), key);
    if second == first {
        return first;
    }
    format!("{}  ({})", first, second)
}

/// Match a key event against the keymap.
#[cfg(test)]
pub fn match_bind(key_code: KeyCode, modifiers: &KeyModifiers) -> Option<WmAction> {
    match_bind_armed(key_code, modifiers, false)
}

/// As `match_bind`, with the one-shot SUPER+ALT prefix state.
pub fn match_bind_armed(
    key_code: KeyCode,
    modifiers: &KeyModifiers,
    alt_armed: bool,
) -> Option<WmAction> {
    let binds = keymap();
    for spelling in spellings() {
        for bind in &binds {
            if bind.key == key_code
                && layer_matches(bind.layer, modifiers, alt_armed, *spelling)
            {
                return Some(bind.action.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, control: bool, alt: bool, logo: bool) -> KeyModifiers {
        KeyModifiers {
            shift,
            control,
            alt,
            logo,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn sup() -> KeyModifiers {
        mods(false, true, true, false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn sup() -> KeyModifiers {
        mods(false, false, false, true)
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn sup_alt() -> KeyModifiers {
        mods(false, false, true, true)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn sup_alt() -> KeyModifiers {
        mods(false, false, true, true)
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn sup_ctrl() -> KeyModifiers {
        mods(false, true, true, true)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn sup_ctrl() -> KeyModifiers {
        mods(false, true, false, true)
    }

    fn fallback_sup() -> KeyModifiers {
        mods(false, true, true, false)
    }

    #[test]
    fn the_three_fullscreen_layers_are_distinct() {
        assert_eq!(
            match_bind(KeyCode::KeyF, &sup()),
            Some(WmAction::Fullscreen(FullscreenMode::Fullscreen))
        );
        assert_eq!(
            match_bind(KeyCode::KeyF, &sup_alt()),
            Some(WmAction::Fullscreen(FullscreenMode::Maximized))
        );
        assert_eq!(
            match_bind(KeyCode::KeyF, &sup_ctrl()),
            Some(WmAction::TiledFullscreen)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn cmd_is_super_and_ctrl_alt_still_is() {
        // ⌘ IS Hyprland's SUPER (the user unbound Spotlight).
        let cmd = mods(false, false, false, true);
        assert_eq!(match_bind(KeyCode::Space, &cmd), Some(WmAction::Menu));
        assert_eq!(
            match_bind(KeyCode::ReturnKey, &cmd),
            Some(WmAction::LaunchTerminal)
        );
        assert_eq!(match_bind(KeyCode::Key1, &cmd), Some(WmAction::Workspace(0)));
        assert_eq!(match_bind(KeyCode::KeyW, &cmd), Some(WmAction::CloseWindow));
        // ⌘⌥ is the ALT layer, ⌘⌃ the CTRL layer, ⌘⇧ the SHIFT layer.
        assert_eq!(
            match_bind(KeyCode::Space, &mods(false, false, true, true)),
            Some(WmAction::AppsMenu)
        );
        assert_eq!(
            match_bind(KeyCode::Space, &mods(false, true, false, true)),
            Some(WmAction::BackgroundNext)
        );
        assert_eq!(
            match_bind(KeyCode::Space, &mods(true, false, false, true)),
            Some(WmAction::ToggleBar)
        );
        // The Ctrl+Alt spelling keeps working for the chords macOS eats.
        assert_eq!(
            match_bind(KeyCode::Space, &mods(false, true, true, false)),
            Some(WmAction::Menu)
        );
        // The cheat sheet shows both.
        let bind = keymap()
            .into_iter()
            .find(|b| b.key == KeyCode::Space && b.action == WmAction::Menu)
            .unwrap();
        let text = combo_text(&bind);
        assert!(text.starts_with("Cmd+Space"), "{}", text);
        assert!(text.contains("Ctrl+Alt+Space"), "{}", text);
    }

    #[test]
    fn the_alt_prefix_reaches_the_alt_layer() {
        // Without the prefix, SUPER+S toggles the scratchpad...
        assert_eq!(
            match_bind(KeyCode::KeyS, &fallback_sup()),
            Some(WmAction::ToggleScratchpad)
        );
        // ...with it, the same chord moves the window there.
        assert_eq!(
            match_bind_armed(KeyCode::KeyS, &fallback_sup(), true),
            Some(WmAction::MoveToScratchpad)
        );
    }

    #[test]
    fn ctrl_alt_is_a_nested_super_fallback() {
        assert_eq!(
            match_bind(KeyCode::KeyQ, &fallback_sup()),
            Some(WmAction::CloseWindow)
        );
        assert_eq!(
            match_bind(KeyCode::ArrowLeft, &fallback_sup()),
            Some(WmAction::FocusDir(Dir::Left))
        );
    }

    #[test]
    fn alt_tab_is_not_a_super_bind() {
        let alt = mods(false, false, true, false);
        assert_eq!(
            match_bind(KeyCode::Tab, &alt),
            Some(WmAction::CycleFocus(true))
        );
        let alt_shift = mods(true, false, true, false);
        assert_eq!(
            match_bind(KeyCode::Tab, &alt_shift),
            Some(WmAction::CycleFocus(false))
        );
    }

    #[test]
    fn ctrl_alt_delete_closes_everything() {
        let ca = mods(false, true, true, false);
        assert_eq!(
            match_bind(KeyCode::Delete, &ca),
            Some(WmAction::CloseAllWindows)
        );
    }

    #[test]
    fn workspace_digits_and_group_digits_do_not_collide() {
        assert_eq!(match_bind(KeyCode::Key3, &sup()), Some(WmAction::Workspace(2)));
        assert_eq!(
            match_bind(KeyCode::Key3, &sup_alt()),
            Some(WmAction::GroupActive(3))
        );
    }

    #[test]
    fn every_bind_renders_a_combo() {
        for bind in keymap() {
            let text = combo_text(&bind);
            assert!(!text.is_empty());
            assert!(!bind.help.is_empty());
        }
    }

    #[test]
    fn no_two_binds_share_a_chord() {
        let binds = keymap();
        for (i, a) in binds.iter().enumerate() {
            for b in binds.iter().skip(i + 1) {
                if a.layer == b.layer && a.key == b.key {
                    assert_eq!(
                        a.action, b.action,
                        "chord clash: {} does two things",
                        combo_text(a)
                    );
                }
            }
        }
    }
}
