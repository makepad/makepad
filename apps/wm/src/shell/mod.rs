//! The omarchy-shell desktop UI, ported from the Quickshell/QML original
//! (`local/vendor/omarchy/shell/`) to splash/makepad.
//!
//! Everything here is read from that source: the spacing scale and type
//! scale of `Commons/Style.qml`, the surface roles of `Commons/Color.qml`,
//! the per-surface token contract of `default/themed/shell.toml.tpl`, the
//! bar layout of `config/omarchy/shell.json`, and each plugin's own QML.
//!
//! # The token contract
//!
//! At runtime the ONLY source is the splash theme: `mod.wm_theme.shell`,
//! an object with one section per surface, mirroring `shell.toml.tpl`
//! key-for-key (dashes become underscores, `[section] key` becomes
//! `section: {key: ...}`). `theme.rs` fills it in from the theme's palette
//! the way `omarchy-theme-set-templates` fills the `.tpl`, and a theme may
//! ship its own `shell: { ... }` block to override the lot.
//!
//! Sizes are in PIXELS, like the QML. Makepad's `font_size` is in points,
//! so text is drawn at `px * 0.75` (see `ShellDraw::px_to_pt`).

use makepad_widgets::*;

pub use makepad_shell_kit::ui;
pub use makepad_shell_kit::{alpha, darker, fade, rgb, CtrlState, MenuTokens, ShellTokens};

pub mod ai_pane;
pub mod bar;
pub mod blackshark;
pub mod file_picker;
pub mod gallery;
pub mod game_controllers;
pub mod launcher;
pub mod menu;
pub mod notifications;
pub mod osd;
pub mod panels;
/// The temperature tray's sensors, history and warning (read on Linux).
pub mod temps;
/// The Wi-Fi dropdown's iwd backend: Linux desktop only;
/// no other platform compiles or references it.
#[cfg(target_os = "linux")]
pub mod wifi_linux;
/// The Wi-Fi panel's hotspot mode (logic) and the QR encoder its join code
/// uses; same gating as the Wi-Fi backend.
#[cfg(target_os = "linux")]
pub mod hotspot_linux;
#[cfg(target_os = "linux")]
pub mod qr;
#[cfg(target_os = "linux")]
pub mod system_linux;
/// The saved display arrangement (order, main screen, modes, render-on
/// GPU): pure parser/serializer/resolver model, no file I/O. Linux only,
/// same gate as `system_linux` (it uses `LinuxDisplaySnapshot`).
#[cfg(target_os = "linux")]
pub mod display_layout;
/// The Display menu's model: resolutions and rates per screen, and the
/// try / keep / revert state machine of a mode or arrangement change.
#[cfg(target_os = "linux")]
pub mod display_menu;

/// Register every shell widget. Called from `AppMain::script_mod` AFTER
/// the theme has been evaluated (the DSL below reads `mod.wm_theme`).
pub fn script_mod(vm: &mut ScriptVm) {
    makepad_shell_kit::script_mod(vm);
    ai_pane::script_mod(vm);
    bar::script_mod(vm);
    menu::script_mod(vm);
    osd::script_mod(vm);
    notifications::script_mod(vm);
    panels::script_mod(vm);
    gallery::script_mod(vm);
    file_picker::script_mod(vm);
}
