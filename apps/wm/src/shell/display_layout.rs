//! The saved display arrangement — order, main screen, per-screen modes
//! and the render-on GPU — as `~/.config/makepad/wm/display-layout`.
//!
//! This module is the pure model only: parsing, serializing and resolving
//! against this boot's [`LinuxDisplaySnapshot`]. It never touches the
//! filesystem; the worker reads and writes the file through
//! `display_settings_path` and `write_display_setting` (`system_linux.rs`)
//! the same way it does for the other display settings files.
//!
//! # File format
//!
//! ```text
//! makepad-display-layout 1
//! render-on 0000:01:00.0 10de:2b85
//! screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30
//! screen 0000:01:00.0 HDMI-A-1 main
//! ```
//!
//! Lines are whitespace-separated tokens. `#` starts a comment; blank
//! lines and unknown keywords are ignored. A screen's key is `(card PCI
//! address, connector without its cardN- prefix)`, which survives card
//! renumbering across boots: `LinuxDisplayOutput::pci` plus `name` minus
//! its `cardN-` prefix. `order` is the order of the `screen` lines, left
//! to right; `main` marks at most one screen as the main screen (the
//! first `main` wins when more than one line claims it); `mode=` is the
//! platform's mode syntax (`WxH`, `WxH@Hz`, `WxH-Hz`), validated the way
//! `mode_string_is_valid` does (`vulkan_linux.rs`, not public there, so
//! mirrored below); `render-on` has exactly `validate_gpu_choice`'s shape
//! (`<pci-address> <vendor>:<device>`). A duplicate screen key keeps its
//! first occurrence's position and flags.
//!
//! `display-layout` supersedes the older `display-source` and
//! `display-gpu` files; [`DisplayLayout::migrate`] reads them once to
//! build the first saved layout.
//!
//! # Applying the saved layout before start-up
//!
//! The renderer reads its environment before `Event::Startup`
//! (`platform/src/os/linux/vulkan_linux.rs::new_direct`), so the WM
//! cannot apply this file to its own process in time by calling into
//! itself. A session script run before the WM binary is exec'd
//! (`tools/linux/display-layout-env.sh`) turns the file into
//! `MAKEPAD_DISPLAY_ORDER`, `MAKEPAD_DRM_MODES` and
//! `MAKEPAD_VULKAN_COMPOSITOR_PCI`/`_UUID`, each exported only when not
//! already set from outside, with a matching `MAKEPAD_WM_ORDER_FROM_SAVED`
//! / `MAKEPAD_WM_MODES_FROM_SAVED` / `MAKEPAD_WM_GPU_FROM_SAVED` marker so
//! the running WM can tell "the script applied the saved value" from "this
//! was pinned from outside" (`SystemSnapshot::order_env`/`modes_env`/
//! `gpu_env`, `system_linux.rs`). An externally pinned, non-empty value
//! always wins over the file, both at session start (the script's own
//! check) and at runtime (`env_restore_plan`, above) — the WM's own
//! restore is a safety net for drift the script's one-shot export cannot
//! catch (e.g. the DRM/KMS state settling after the script ran).
//!
//! # Restart contract
//!
//! Saving a layout that moves the render-on GPU needs the renderer
//! restarted on the new choice, which the process cannot do to itself:
//! `restart_gate` (`linux_controls.rs`) asks the UI to confirm, and
//! `Event::Shutdown` then exits with code 75 (`EX_TEMPFAIL`) instead of
//! 0. A session manager
//! that runs the WM as a service treats that code as a request to relaunch
//! it, not a crash (`systemd`'s `RestartForceExitStatus=75`/
//! `SuccessExitStatus=75`, `tools/arch_usb/makepad-wm.service`); the next
//! launch re-reads `display-layout` through the same session script, so
//! the new GPU choice is already in the environment on the way up.
//!
//! Compiled for Linux only (the inner `cfg` below makes the module empty
//! elsewhere), so no other platform's behaviour changes.

#![cfg(all(target_os = "linux", not(target_env = "ohos")))]

use makepad_widgets::makepad_platform::linux_display::{LinuxDisplayOutput, LinuxDisplaySnapshot};
use makepad_widgets::makepad_platform::linux_wide_desktop::parse_mode_overrides;

use super::system_linux::validate_gpu_choice;

/// The header line's keyword.
const HEADER_KEYWORD: &str = "makepad-display-layout";
/// The header line's version, written on every save.
const FORMAT_VERSION: &str = "1";
/// The status the renderer gives a connected screen on another GPU whose
/// peer has not joined the wide desktop yet (`OTHER_GPU_STATUS` in
/// `vulkan_linux.rs`, private there, so mirrored). The renderer accepts
/// such a screen as the main screen and takes its mode before it starts.
pub const PEER_PENDING_STATUS: &str = "unsupported: driven by another GPU";

/// A screen's identity across reboots and card renumbering: the card's PCI
/// address (`LinuxDisplayOutput::pci`) and the connector name with its
/// `cardN-` prefix removed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScreenKey {
    pub pci: String,
    pub connector: String,
}

/// One `screen` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenEntry {
    pub key: ScreenKey,
    pub main: bool,
    pub mode: Option<String>,
}

/// The whole saved file: the render-on GPU (`None` is Auto) and the
/// screens in left-to-right order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplayLayout {
    pub render_on: Option<String>,
    pub screens: Vec<ScreenEntry>,
}

impl DisplayLayout {
    /// Parses the file's text. Tolerant: a line that is empty, a `#`
    /// comment, an unknown keyword, or malformed for its keyword is
    /// skipped rather than failing the whole parse. A duplicate screen key
    /// keeps its first occurrence; a second `main` flag (on any screen) is
    /// dropped, so at most one screen ends up `main`. An invalid `mode=`
    /// or `render-on` value is dropped, leaving that field unset.
    pub fn parse(text: &str) -> DisplayLayout {
        let mut render_on = None;
        let mut screens: Vec<ScreenEntry> = Vec::new();
        let mut main_seen = false;
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut tokens = line.split_whitespace();
            let Some(keyword) = tokens.next() else { continue };
            match keyword {
                HEADER_KEYWORD => {}
                "render-on" => {
                    if render_on.is_some() {
                        continue;
                    }
                    let rest: Vec<&str> = tokens.collect();
                    let value = rest.join(" ");
                    if validate_gpu_choice(&value).is_ok() {
                        render_on = Some(value);
                    }
                }
                "screen" => {
                    let Some(pci) = tokens.next() else { continue };
                    let Some(connector) = tokens.next() else { continue };
                    if pci.is_empty() || connector.is_empty() {
                        continue;
                    }
                    let key = ScreenKey { pci: pci.to_string(), connector: connector.to_string() };
                    if screens.iter().any(|entry: &ScreenEntry| entry.key == key) {
                        continue; // duplicate key: first occurrence wins
                    }
                    let mut main = false;
                    let mut mode = None;
                    for token in tokens {
                        if token == "main" {
                            if !main_seen {
                                main = true;
                            }
                        } else if let Some(value) = token.strip_prefix("mode=") {
                            if mode_string_is_valid(value) {
                                mode = Some(value.to_string());
                            }
                        }
                    }
                    if main {
                        main_seen = true;
                    }
                    screens.push(ScreenEntry { key, main, mode });
                }
                _ => {}
            }
        }
        DisplayLayout { render_on, screens }
    }

    /// Serializes back to the file's text. Round-trips through
    /// [`DisplayLayout::parse`] for any layout this module itself built
    /// (at most one `main`, a valid `mode` and `render_on`, no duplicate
    /// keys — exactly what the mutating methods below maintain).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str(HEADER_KEYWORD);
        out.push(' ');
        out.push_str(FORMAT_VERSION);
        out.push('\n');
        if let Some(render_on) = &self.render_on {
            out.push_str("render-on ");
            out.push_str(render_on);
            out.push('\n');
        }
        for entry in &self.screens {
            out.push_str("screen ");
            out.push_str(&entry.key.pci);
            out.push(' ');
            out.push_str(&entry.key.connector);
            if entry.main {
                out.push_str(" main");
            }
            if let Some(mode) = &entry.mode {
                out.push_str(" mode=");
                out.push_str(mode);
            }
            out.push('\n');
        }
        out
    }

    /// The current arrangement, built straight from a snapshot rather than
    /// parsed: placed screens only (`desktop_position.is_some()`), ordered
    /// left to right by `desktop_position.x`, `main` from `primary`, mode
    /// from `mode_override`. An output without a resolvable `pci` is
    /// skipped (it could never be matched back on a later boot).
    pub fn from_snapshot(snap: &LinuxDisplaySnapshot, render_on: Option<String>) -> DisplayLayout {
        let mut placed: Vec<&LinuxDisplayOutput> =
            snap.outputs.iter().filter(|output| output.desktop_position.is_some()).collect();
        placed.sort_by_key(|output| output.desktop_position.map(|(x, _)| x).unwrap_or(0));
        let screens = placed
            .into_iter()
            .filter_map(|output| {
                screen_key(output).map(|key| ScreenEntry {
                    key,
                    main: output.primary,
                    mode: output.mode_override.clone(),
                })
            })
            .collect();
        DisplayLayout { render_on, screens }
    }

    /// Each saved screen that resolves against this boot's snapshot (its
    /// key matches an output's `pci` and connector suffix), as
    /// `(index into screens, that output's current name)`. A screen that
    /// does not resolve this boot is dropped — never matched by connector
    /// alone, so the same connector on another GPU is not mistaken for it.
    pub fn resolve(&self, snap: &LinuxDisplaySnapshot) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (index, entry) in self.screens.iter().enumerate() {
            if let Some(output) =
                snap.outputs.iter().find(|output| screen_key(output).as_ref() == Some(&entry.key))
            {
                out.push((index, output.name.clone()));
            }
        }
        out
    }

    /// This boot's output names in the saved left-to-right order, followed
    /// by any output the saved file did not place, in the snapshot's own
    /// (default) order — the shape `linux_set_display_order` wants.
    pub fn order_names(&self, snap: &LinuxDisplaySnapshot) -> Vec<String> {
        let resolved = self.resolve(snap);
        let mut names: Vec<String> = Vec::with_capacity(snap.outputs.len());
        for (_, name) in &resolved {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        for output in &snap.outputs {
            if !names.contains(&output.name) {
                names.push(output.name.clone());
            }
        }
        names
    }

    /// `(this boot's name, saved mode)` for every saved screen that both
    /// resolves and has a saved mode.
    pub fn modes(&self, snap: &LinuxDisplaySnapshot) -> Vec<(String, String)> {
        self.resolve(snap)
            .into_iter()
            .filter_map(|(index, name)| self.screens[index].mode.clone().map(|mode| (name, mode)))
            .collect()
    }

    /// The main screen's this-boot name, if the saved main screen (the
    /// first `screen` entry with `main` set) resolves.
    pub fn main_name(&self, snap: &LinuxDisplaySnapshot) -> Option<String> {
        let resolved = self.resolve(snap);
        let (main_index, _) = self.screens.iter().enumerate().find(|(_, entry)| entry.main)?;
        resolved.into_iter().find(|(index, _)| *index == main_index).map(|(_, name)| name)
    }

    /// Swaps the screen keyed by `key` with its left (`dir < 0`) or right
    /// (`dir > 0`) neighbour. `false`, no change, when the key is not
    /// found or the swap would run past an end.
    pub fn move_screen(&mut self, key: &ScreenKey, dir: i32) -> bool {
        let Some(index) = self.screens.iter().position(|entry| &entry.key == key) else {
            return false;
        };
        let target = index as i32 + dir;
        if target < 0 || target as usize >= self.screens.len() {
            return false;
        }
        self.screens.swap(index, target as usize);
        true
    }

    /// Marks the screen keyed by `key` as the (only) main screen. No-op
    /// when `key` is not one of the saved screens.
    pub fn set_main(&mut self, key: &ScreenKey) {
        if !self.screens.iter().any(|entry| &entry.key == key) {
            return;
        }
        for entry in &mut self.screens {
            entry.main = &entry.key == key;
        }
    }

    /// Sets the screen keyed by `key`'s mode, or clears it with `None`.
    /// A mode that fails `mode_string_is_valid` is treated as `None`
    /// instead of being saved. No-op when `key` is not one of the saved
    /// screens.
    pub fn set_mode(&mut self, key: &ScreenKey, mode: Option<String>) {
        let Some(entry) = self.screens.iter_mut().find(|entry| &entry.key == key) else {
            return;
        };
        entry.mode = mode.filter(|mode| mode_string_is_valid(mode));
    }

    /// Builds the first saved layout from the legacy files: `display-source`
    /// (a card-prefixed connector name such as `card1-HDMI-A-1`, resolved to
    /// a boot-stable key through `card_pci`, the caller's `cardN -> pci`
    /// lookup, becoming the one saved screen, `main`) and `display-gpu`
    /// (carried over to `render_on` as-is, when it has `validate_gpu_choice`'s
    /// shape). Either input can be absent or fail to resolve; the result is
    /// then missing that part rather than failing outright.
    pub fn migrate(
        display_source: Option<&str>,
        display_gpu: Option<&str>,
        card_pci: &dyn Fn(&str) -> Option<String>,
    ) -> DisplayLayout {
        let mut screens = Vec::new();
        if let Some(source) = display_source {
            if let Some((card, connector)) = split_card_prefix(source) {
                if let Some(pci) = card_pci(card) {
                    screens.push(ScreenEntry {
                        key: ScreenKey { pci, connector: connector.to_string() },
                        main: true,
                        mode: None,
                    });
                }
            }
        }
        let render_on =
            display_gpu.and_then(|gpu| validate_gpu_choice(gpu).ok().map(str::to_string));
        DisplayLayout { render_on, screens }
    }
}

impl DisplayLayout {
    /// Makes sure every placed screen of `snap` that has a key is one of
    /// the saved screens, so an edit (`set_main`, `set_mode`,
    /// `move_screen`) on a live screen is never a no-op. When one is
    /// missing (nothing saved yet, or a layout migrated from the old
    /// single-name `display-source`), the screens become the live
    /// arrangement left to right as [`DisplayLayout::from_snapshot`]
    /// builds it, followed by the saved screens that are not placed now
    /// (not connected this boot, or a peer's screen still to join), kept
    /// with their modes (their `main` dropped when a live screen is main).
    /// A layout that already covers every live screen is left exactly as
    /// saved.
    pub fn include_snapshot(&mut self, snap: &LinuxDisplaySnapshot) {
        let live = DisplayLayout::from_snapshot(snap, None).screens;
        if live.iter().all(|entry| self.screens.iter().any(|saved| saved.key == entry.key)) {
            return;
        }
        let mut screens = live;
        let rest: Vec<ScreenEntry> = self
            .screens
            .iter()
            .filter(|saved| !screens.iter().any(|live| live.key == saved.key))
            .cloned()
            .collect();
        for mut entry in rest {
            if screens.iter().any(|other| other.main) {
                entry.main = false;
            }
            screens.push(entry);
        }
        self.screens = screens;
    }
}

impl DisplayLayout {
    /// Merges the file's start-up read into a working layout this session
    /// edited before the read landed. Untouched parts come from the read.
    /// When the screens were edited (`screens_edited`: a main pick, an
    /// order or mode change), the edited entries stand, and every saved
    /// entry they do not have is appended (including screens not connected
    /// now), so a whole-file save never drops them; an appended entry
    /// loses `main` when an edited one has it. `render_on_edited` keeps the
    /// person's render-on GPU.
    pub fn merge_loaded(&mut self, loaded: &DisplayLayout, screens_edited: bool, render_on_edited: bool) {
        if screens_edited {
            let has_main = self.screens.iter().any(|entry| entry.main);
            for saved in &loaded.screens {
                if !self.screens.iter().any(|entry| entry.key == saved.key) {
                    let mut entry = saved.clone();
                    if has_main {
                        entry.main = false;
                    }
                    self.screens.push(entry);
                }
            }
        } else {
            self.screens = loaded.screens.clone();
        }
        if !render_on_edited {
            self.render_on = loaded.render_on.clone();
        }
    }
}

impl DisplayLayout {
    /// Puts the screens keyed by `keys` in that order, each into one of
    /// the slots those screens held, so every other saved screen (one not
    /// connected now, a peer's screen still to join) keeps its place
    /// between them. Keys that are not saved screens are ignored.
    pub fn reorder(&mut self, keys: &[ScreenKey]) {
        let current: Vec<ScreenKey> = self.screens.iter().map(|entry| entry.key.clone()).collect();
        let order = fill_slots(&current, keys);
        let mut screens = Vec::with_capacity(self.screens.len());
        for key in order {
            if let Some(index) = self.screens.iter().position(|entry| entry.key == key) {
                screens.push(self.screens.remove(index));
            }
        }
        screens.append(&mut self.screens);
        self.screens = screens;
    }
}

/// The Display panel's screen rows: indices into `snap.outputs`, the
/// placed screens left to right (`desktop_position.x`) first, then the
/// ones outside the desktop in the snapshot's default order; with how
/// many of them are placed (the ones that can be moved).
pub fn screen_rows(snap: &LinuxDisplaySnapshot) -> (Vec<usize>, usize) {
    let mut placed: Vec<usize> =
        (0..snap.outputs.len()).filter(|&i| snap.outputs[i].desktop_position.is_some()).collect();
    placed.sort_by_key(|&i| snap.outputs[i].desktop_position.map(|(x, _)| x).unwrap_or(0));
    let placed_count = placed.len();
    placed.extend((0..snap.outputs.len()).filter(|&i| snap.outputs[i].desktop_position.is_none()));
    (placed, placed_count)
}

/// `order` with the item at `row` swapped with its left (`dir < 0`) or
/// right (`dir > 0`) neighbour; `None` past either end.
pub fn swap_neighbour<T: Clone>(order: &[T], row: usize, dir: i32) -> Option<Vec<T>> {
    if row >= order.len() || dir == 0 {
        return None;
    }
    let target = if dir < 0 { row.checked_sub(1)? } else { row + 1 };
    if target >= order.len() {
        return None;
    }
    let mut out = order.to_vec();
    out.swap(row, target);
    Some(out)
}

/// `list` with the items that `wanted` names put in `wanted`'s order,
/// each into one of the slots those items held; every other item stays
/// where it was. Items of `wanted` not in `list` are ignored.
pub fn fill_slots<T: Clone + PartialEq>(list: &[T], wanted: &[T]) -> Vec<T> {
    let mut queue = wanted.iter().filter(|item| list.contains(item));
    list.iter()
        .map(|item| {
            if wanted.contains(item) {
                queue.next().cloned().unwrap_or_else(|| item.clone())
            } else {
                item.clone()
            }
        })
        .collect()
}

/// Whether the desktop's screens (placed, or on their way: [`is_joining`])
/// are driven by more than one card. The renderer cannot switch its GPU
/// live then, so "Render on" only takes effect at the next start.
pub fn screens_span_gpus(snap: &LinuxDisplaySnapshot) -> bool {
    let mut cards = snap
        .outputs
        .iter()
        .filter(|output| output.desktop_position.is_some() || is_joining(output))
        .map(|output| output.card.as_str());
    match cards.next() {
        Some(first) => cards.any(|card| card != first),
        None => false,
    }
}

/// One step of bringing the running desktop in line with the saved
/// layout: the `Cx::linux_set_display_*` call it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreOp {
    /// `linux_set_display_order` with these this-boot names.
    Order(Vec<String>),
    /// `linux_set_display_mode(name, mode)`.
    Mode(String, Option<String>),
    /// `linux_set_display_source(name)`: make it the main screen.
    Main(String),
}

/// What the running desktop still needs to match `layout`, as order, then
/// modes, then main. Only saved screens that resolve this boot count;
/// the rest are ignored. Idempotent: a snapshot that already shows the
/// layout yields nothing.
///
/// * `Order(order_names)` only when two or more saved screens resolve,
///   and then when those of them that are placed are not in their saved
///   relative order left to right (`desktop_position.x`), or when one of
///   them is still joining ([`is_joining`]): the order is stored before
///   it joins, so it joins on its saved side. Screens the file does not
///   list never decide the order, so a one-screen layout (one migrated
///   from `display-source`, or a laptop's when a dock adds a screen)
///   leaves the arrangement alone. The full `order_names` is what the
///   platform gets.
/// * `Mode(name, Some(mode))` per resolved screen whose saved mode is not
///   its `mode_override`. A screen saved without `mode=` leaves the
///   running mode alone (an amendment to the brief's "mode_override
///   differs"): at start-up an override with no saved mode can only come
///   from the environment, an external `MAKEPAD_DRM_MODES` wins, and a
///   layout migrated from `display-source` knows no modes. Choosing
///   "Automatic" in the panel calls `linux_set_display_mode(None)`
///   itself, so nothing depends on the plan clearing a mode.
/// * `Main(name)` when the saved main screen resolves and is not
///   `primary`.
pub fn restore_plan(layout: &DisplayLayout, snap: &LinuxDisplaySnapshot) -> Vec<RestoreOp> {
    let mut ops = Vec::new();
    let resolved = layout.resolve(snap);
    if resolved.is_empty() {
        return ops;
    }
    let output_named =
        |name: &str| snap.outputs.iter().find(|output| output.name == name);
    if resolved.len() >= 2 {
        let mut placed: Vec<&LinuxDisplayOutput> =
            snap.outputs.iter().filter(|output| output.desktop_position.is_some()).collect();
        placed.sort_by_key(|output| output.desktop_position.map(|(x, _)| x).unwrap_or(0));
        let saved: Vec<&str> = resolved.iter().map(|(_, name)| name.as_str()).collect();
        // Ties (an overlapping, still settling desktop) count as waiting
        // below, never as "in order".
        let placed_saved: Vec<&str> =
            placed.iter().map(|output| output.name.as_str()).filter(|name| saved.contains(name)).collect();
        let wanted: Vec<&str> =
            saved.iter().copied().filter(|name| placed_saved.contains(name)).collect();
        if wanted != placed_saved || restore_waiting(layout, snap) {
            ops.push(RestoreOp::Order(layout.order_names(snap)));
        }
    }
    for (index, name) in &resolved {
        let Some(mode) = layout.screens[*index].mode.as_ref() else { continue };
        if output_named(name).is_some_and(|output| output.mode_override.as_ref() != Some(mode)) {
            ops.push(RestoreOp::Mode(name.clone(), Some(mode.clone())));
        }
    }
    if let Some(main) = layout.main_name(snap) {
        if output_named(&main).is_some_and(|output| !output.primary) {
            ops.push(RestoreOp::Main(main));
        }
    }
    ops
}

/// `plan` with every `Order`/`Mode` op removed that an external,
/// non-saved env var already pins for this boot: all of `Order` while
/// `order_env` is `Some` (any non-empty, externally pinned
/// `MAKEPAD_DISPLAY_ORDER` — the whole order is one call, so there is no
/// partial order to keep), and a `Mode(name, _)` for any screen `name`
/// appears in `modes_env`'s own `name=mode,name=mode` text (reparsed with
/// the platform's own [`parse_mode_overrides`], never a second copy of
/// that grammar). `Main` is never filtered: the main screen has no env
/// var of its own, and `restart_gate`/the panel's own live edits are
/// unaffected (a person's `DisplayOrder`/`DisplayMode` action calls
/// `Cx::linux_set_display_*` directly, never through this plan).
///
/// This is the runtime half of the constraint "an externally set
/// non-empty env var always wins over the file": the session helper
/// already defers to a pinned var at session start, and without this the
/// WM's own restore did not, so a pinned env and a disagreeing file
/// produced two layouts per boot (the env's for the first frame, then the
/// file's once the restore ran).
pub fn env_restore_plan(plan: Vec<RestoreOp>, order_env: Option<&str>, modes_env: Option<&str>) -> Vec<RestoreOp> {
    let order_pinned = order_env.is_some_and(|value| !value.is_empty());
    let mode_pinned: Vec<String> = modes_env
        .filter(|value| !value.is_empty())
        .map(|text| parse_mode_overrides(text).into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default();
    plan.into_iter()
        .filter(|op| match op {
            RestoreOp::Order(_) => !order_pinned,
            RestoreOp::Mode(name, _) => !mode_pinned.contains(name),
            RestoreOp::Main(_) => true,
        })
        .collect()
}

/// A connected screen on another GPU whose peer has not been created yet:
/// the renderer already accepts it as the main screen.
pub fn is_peer_pending(output: &LinuxDisplayOutput) -> bool {
    output.status.starts_with(PEER_PENDING_STATUS)
}

/// A listed screen that is not part of the desktop yet but is on its way:
/// not placed, and neither failed, lost, unsupported (other than a peer
/// GPU's screen still to be created) nor left out by the GPU's limits.
/// Covers both a peer GPU's row before its peer exists and the peer's own
/// row between its creation and its first frame, which has an ordinary
/// status.
pub fn is_joining(output: &LinuxDisplayOutput) -> bool {
    if output.desktop_position.is_some() {
        return false;
    }
    if is_peer_pending(output) {
        return true;
    }
    let status = output.status.as_str();
    !(status.starts_with("failed")
        || status.starts_with("lost")
        || status.starts_with("unsupported")
        || status.starts_with("active: not in the wide desktop"))
}

/// Whether the desktop is not settled enough to judge the saved order: a
/// saved screen that resolves this boot is still joining ([`is_joining`]),
/// or the placed screens overlap ([`placed_overlap`]). The order restore
/// stays open (within the caller's bound), and the order is seeded, so the
/// screen is laid out on its saved side.
pub fn restore_waiting(layout: &DisplayLayout, snap: &LinuxDisplaySnapshot) -> bool {
    placed_overlap(snap)
        || layout.resolve(snap).iter().any(|(_, name)| {
            snap.outputs.iter().find(|output| &output.name == name).is_some_and(is_joining)
        })
}

/// Whether two screens of the desktop overlap: a layout still settling
/// (a peer's screen reported before the wide desktop gave it its slice
/// sits at its own (0, 0)), which says nothing about the saved order yet.
pub fn placed_overlap(snap: &LinuxDisplaySnapshot) -> bool {
    let rects: Vec<(u64, u64, u64, u64)> = snap
        .outputs
        .iter()
        .filter_map(|output| {
            let (x, y) = output.desktop_position?;
            Some((x as u64, y as u64, output.width.max(1) as u64, output.height.max(1) as u64))
        })
        .collect();
    rects.iter().enumerate().any(|(i, a)| {
        rects[i + 1..].iter().any(|b| a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3)
    })
}

/// `output`'s screen key, or `None` when it has no resolvable `pci` (it
/// could never be matched back on a later boot, so it is not worth
/// saving).
pub fn screen_key(output: &LinuxDisplayOutput) -> Option<ScreenKey> {
    let pci = output.pci.clone()?;
    Some(ScreenKey { pci, connector: connector_suffix(&output.name) })
}

/// `name` minus its `cardN-` prefix, or `name` unchanged when it does not
/// have that shape.
pub fn connector_suffix(name: &str) -> String {
    match split_card_prefix(name) {
        Some((_, connector)) => connector.to_string(),
        None => name.to_string(),
    }
}

/// Splits `cardN-<connector>` into `("cardN", "<connector>")`, or `None`
/// when `name` does not start with `card` followed by digits then `-`.
fn split_card_prefix(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("card")?;
    let dash = rest.find('-')?;
    let digits = &rest[..dash];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let card_len = "card".len() + dash;
    Some((&name[..card_len], &name[card_len + 1..]))
}

/// Whether `mode` is `WxH`, `WxH@Hz` or `WxH-Hz` with digits only — the
/// forms `MAKEPAD_DRM_MODE`, `MAKEPAD_DRM_MODES` and
/// `direct_request_display_mode` accept. Mirrors the platform's
/// `mode_string_is_valid` (`platform/src/os/linux/vulkan_linux.rs`), which
/// is private and `cfg(linux_direct)`-gated, so it is not reachable from
/// here.
fn mode_string_is_valid(mode: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|byte| byte.is_ascii_digit());
    let resolution = match mode.split_once('@').or_else(|| mode.split_once('-')) {
        Some((resolution, rate)) => {
            if !digits(rate) {
                return false;
            }
            resolution
        }
        None => mode,
    };
    match resolution.split_once('x') {
        Some((width, height)) => digits(width) && digits(height),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str, pci: &str) -> LinuxDisplayOutput {
        LinuxDisplayOutput { name: name.to_string(), pci: Some(pci.to_string()), ..Default::default() }
    }

    fn placed(name: &str, pci: &str, x: u32, primary: bool) -> LinuxDisplayOutput {
        LinuxDisplayOutput {
            desktop_position: Some((x, 0)),
            primary,
            ..output(name, pci)
        }
    }

    fn snapshot(outputs: Vec<LinuxDisplayOutput>) -> LinuxDisplaySnapshot {
        LinuxDisplaySnapshot { direct: true, outputs }
    }

    #[test]
    fn round_trip() {
        let layout = DisplayLayout {
            render_on: Some("0000:01:00.0 10de:2b85".to_string()),
            screens: vec![
                ScreenEntry {
                    key: ScreenKey { pci: "0000:00:02.0".to_string(), connector: "HDMI-A-2".to_string() },
                    main: false,
                    mode: Some("3840x2160@30".to_string()),
                },
                ScreenEntry {
                    key: ScreenKey { pci: "0000:01:00.0".to_string(), connector: "HDMI-A-1".to_string() },
                    main: true,
                    mode: None,
                },
            ],
        };
        let text = layout.serialize();
        assert_eq!(DisplayLayout::parse(&text), layout);
    }

    #[test]
    fn junk_and_comments_are_ignored() {
        let text = "\
            # a comment\n\
            \n\
            makepad-display-layout 1\n\
            mystery-key some value\n\
            screen\n\
            screen 0000:00:02.0\n\
            screen 0000:01:00.0 HDMI-A-1 main\n\
            # another comment\n\
        ";
        let layout = DisplayLayout::parse(text);
        assert_eq!(layout.render_on, None);
        assert_eq!(layout.screens.len(), 1);
        assert_eq!(layout.screens[0].key.connector, "HDMI-A-1");
        assert!(layout.screens[0].main);
    }

    #[test]
    fn card_renumbering_resolves_by_pci_and_connector_suffix() {
        let layout = DisplayLayout::parse("screen 0000:00:02.0 HDMI-A-2\n");
        let snap = snapshot(vec![output("card1-HDMI-A-2", "0000:00:02.0")]);
        assert_eq!(layout.resolve(&snap), vec![(0, "card1-HDMI-A-2".to_string())]);
    }

    #[test]
    fn same_connector_on_two_gpus_resolves_by_pci() {
        let layout = DisplayLayout::parse("screen 0000:01:00.0 HDMI-A-1\n");
        let snap = snapshot(vec![
            output("card0-HDMI-A-1", "0000:00:02.0"),
            output("card1-HDMI-A-1", "0000:01:00.0"),
        ]);
        assert_eq!(layout.resolve(&snap), vec![(0, "card1-HDMI-A-1".to_string())]);
    }

    #[test]
    fn missing_screen_is_dropped() {
        let layout = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-2\nscreen 0000:ff:00.0 DP-9\n",
        );
        let snap = snapshot(vec![output("card0-HDMI-A-2", "0000:00:02.0")]);
        assert_eq!(layout.resolve(&snap), vec![(0, "card0-HDMI-A-2".to_string())]);
        assert_eq!(layout.order_names(&snap), vec!["card0-HDMI-A-2".to_string()]);
    }

    #[test]
    fn two_main_flags_keep_the_first() {
        let layout = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-2 main\nscreen 0000:01:00.0 HDMI-A-1 main\n",
        );
        assert!(layout.screens[0].main);
        assert!(!layout.screens[1].main);
    }

    #[test]
    fn move_screen_at_both_ends() {
        let mut layout = DisplayLayout::parse(
            "screen 0000:00:02.0 A\nscreen 0000:00:03.0 B\nscreen 0000:00:04.0 C\n",
        );
        let a = ScreenKey { pci: "0000:00:02.0".to_string(), connector: "A".to_string() };
        let c = ScreenKey { pci: "0000:00:04.0".to_string(), connector: "C".to_string() };
        assert!(!layout.move_screen(&a, -1));
        assert!(!layout.move_screen(&c, 1));
        assert!(layout.move_screen(&a, 1));
        assert_eq!(layout.screens[0].key.connector, "B");
        assert_eq!(layout.screens[1].key.connector, "A");
    }

    #[test]
    fn invalid_mode_is_dropped() {
        let layout = DisplayLayout::parse("screen 0000:00:02.0 HDMI-A-2 mode=garbage\n");
        assert_eq!(layout.screens[0].mode, None);
        let layout = DisplayLayout::parse("screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30\n");
        assert_eq!(layout.screens[0].mode, Some("3840x2160@30".to_string()));
    }

    #[test]
    fn from_snapshot_orders_by_desktop_position_x_and_skips_unplaced_and_no_pci() {
        let mut unplaced = output("card0-DP-9", "0000:00:09.0");
        unplaced.desktop_position = None;
        let mut no_pci = placed("card0-DP-8", "0000:00:08.0", 0, false);
        no_pci.pci = None;
        let snap = snapshot(vec![
            placed("card1-HDMI-A-1", "0000:01:00.0", 1920, true),
            placed("card0-HDMI-A-2", "0000:00:02.0", 0, false),
            unplaced,
            no_pci,
        ]);
        let layout = DisplayLayout::from_snapshot(&snap, None);
        assert_eq!(layout.screens.len(), 2);
        assert_eq!(layout.screens[0].key.connector, "HDMI-A-2");
        assert!(!layout.screens[0].main);
        assert_eq!(layout.screens[1].key.connector, "HDMI-A-1");
        assert!(layout.screens[1].main);
    }

    #[test]
    fn migrate_maps_display_source_and_display_gpu() {
        let card_pci = |card: &str| -> Option<String> {
            match card {
                "card1" => Some("0000:01:00.0".to_string()),
                _ => None,
            }
        };
        let layout = DisplayLayout::migrate(
            Some("card1-HDMI-A-1"),
            Some("0000:01:00.0 10de:2b85"),
            &card_pci,
        );
        assert_eq!(layout.render_on, Some("0000:01:00.0 10de:2b85".to_string()));
        assert_eq!(layout.screens.len(), 1);
        assert!(layout.screens[0].main);
        assert_eq!(
            layout.screens[0].key,
            ScreenKey { pci: "0000:01:00.0".to_string(), connector: "HDMI-A-1".to_string() }
        );
    }

    #[test]
    fn migrate_drops_unresolvable_source_and_invalid_gpu() {
        let card_pci = |_: &str| -> Option<String> { None };
        let layout = DisplayLayout::migrate(Some("card1-HDMI-A-1"), Some("not a gpu choice"), &card_pci);
        assert_eq!(layout.screens.len(), 0);
        assert_eq!(layout.render_on, None);
    }

    fn peer_pending(name: &str, pci: &str) -> LinuxDisplayOutput {
        LinuxDisplayOutput {
            status: format!("{PEER_PENDING_STATUS} (card1); it joins the wide desktop as a peer"),
            ..output(name, pci)
        }
    }

    fn key(pci: &str, connector: &str) -> ScreenKey {
        ScreenKey { pci: pci.to_string(), connector: connector.to_string() }
    }

    #[test]
    fn restore_plan_reorders_a_fresh_snapshot_in_default_order() {
        // Default order puts HDMI-A-1 left; the saved file wants it right.
        let snap = snapshot(vec![
            placed("card0-HDMI-A-1", "0000:00:02.0", 0, true),
            placed("card0-HDMI-A-2", "0000:00:02.0", 1920, false),
        ]);
        let layout = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-2\nscreen 0000:00:02.0 HDMI-A-1 main\n",
        );
        assert_eq!(
            restore_plan(&layout, &snap),
            vec![RestoreOp::Order(vec!["card0-HDMI-A-2".to_string(), "card0-HDMI-A-1".to_string()])]
        );
    }

    #[test]
    fn restore_plan_sets_a_mode_that_differs() {
        let snap = snapshot(vec![placed("card0-HDMI-A-2", "0000:00:02.0", 0, true)]);
        let layout = DisplayLayout::parse("screen 0000:00:02.0 HDMI-A-2 main mode=3840x2160@30\n");
        assert_eq!(
            restore_plan(&layout, &snap),
            vec![RestoreOp::Mode("card0-HDMI-A-2".to_string(), Some("3840x2160@30".to_string()))]
        );
    }

    #[test]
    fn restore_plan_leaves_a_running_mode_alone_when_none_is_saved() {
        let mut out = placed("card0-HDMI-A-2", "0000:00:02.0", 0, true);
        out.mode_override = Some("3840x2160@30".to_string());
        let layout = DisplayLayout::parse("screen 0000:00:02.0 HDMI-A-2 main\n");
        assert_eq!(restore_plan(&layout, &snapshot(vec![out])), vec![]);
    }

    #[test]
    fn restore_plan_is_empty_once_applied() {
        let mut left = placed("card1-HDMI-A-2", "0000:00:02.0", 0, false);
        left.mode_override = Some("3840x2160@30".to_string());
        let right = placed("card0-HDMI-A-1", "0000:01:00.0", 3840, true);
        // Snapshot lists in default order (by name), not left to right.
        let snap = snapshot(vec![right, left]);
        let layout = DisplayLayout::parse(
            "render-on 0000:01:00.0 10de:2b85\n\
             screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30\n\
             screen 0000:01:00.0 HDMI-A-1 main\n",
        );
        assert_eq!(restore_plan(&layout, &snap), vec![]);
        // And the snapshot's own arrangement plans nothing either.
        assert_eq!(restore_plan(&DisplayLayout::from_snapshot(&snap, None), &snap), vec![]);
    }

    #[test]
    fn restore_plan_asks_for_a_main_screen_on_a_peer_not_yet_active() {
        // The iGPU screen is up; the 5090's is listed as driven by another
        // GPU and has not joined yet. Order is seeded so it joins on the
        // saved side; main is asked for at once.
        let snap = snapshot(vec![
            placed("card1-HDMI-A-2", "0000:00:02.0", 0, true),
            peer_pending("card0-HDMI-A-1", "0000:01:00.0"),
        ]);
        let layout = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-2\nscreen 0000:01:00.0 HDMI-A-1 main\n",
        );
        assert_eq!(
            restore_plan(&layout, &snap),
            vec![
                RestoreOp::Order(vec!["card1-HDMI-A-2".to_string(), "card0-HDMI-A-1".to_string()]),
                RestoreOp::Main("card0-HDMI-A-1".to_string()),
            ]
        );
        assert!(is_peer_pending(&snap.outputs[1]));
        assert!(!is_peer_pending(&snap.outputs[0]));
    }

    #[test]
    fn restore_plan_ignores_unresolved_screens() {
        // Saved screens on a card that is gone, and one whose connector
        // exists only on the other GPU: neither is matched.
        let snap = snapshot(vec![
            placed("card0-HDMI-A-1", "0000:00:02.0", 0, true),
            placed("card0-DP-1", "0000:00:02.0", 1920, false),
        ]);
        let layout = DisplayLayout::parse(
            "screen 0000:ff:00.0 DP-9 main mode=1920x1080\nscreen 0000:01:00.0 HDMI-A-1 mode=1280x720\n",
        );
        assert_eq!(restore_plan(&layout, &snap), vec![]);
    }

    #[test]
    fn include_snapshot_adds_live_screens_and_keeps_unresolved_ones() {
        let snap = snapshot(vec![
            placed("card0-HDMI-A-1", "0000:00:02.0", 1920, true),
            placed("card0-DP-1", "0000:00:02.0", 0, false),
        ]);
        // Migrated: only the main screen, and it is not connected now.
        let mut layout = DisplayLayout::parse("screen 0000:01:00.0 HDMI-A-1 main\n");
        layout.include_snapshot(&snap);
        let keys: Vec<&ScreenKey> = layout.screens.iter().map(|entry| &entry.key).collect();
        assert_eq!(
            keys,
            vec![&key("0000:00:02.0", "DP-1"), &key("0000:00:02.0", "HDMI-A-1"), &key("0000:01:00.0", "HDMI-A-1")]
        );
        assert_eq!(layout.screens.iter().filter(|entry| entry.main).count(), 1);
        assert!(layout.screens[1].main);
        layout.set_main(&key("0000:00:02.0", "DP-1"));
        assert!(layout.screens[0].main && !layout.screens[1].main);
        // A layout that already covers every live screen is left alone.
        let mut covered = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-1 mode=1280x720\nscreen 0000:00:02.0 DP-1 main\n",
        );
        let before = covered.clone();
        covered.include_snapshot(&snap);
        assert_eq!(covered, before);
    }

    #[test]
    fn restore_plan_one_saved_screen_never_reorders() {
        // Migrated display-source: only the main screen, on the right.
        let snap = snapshot(vec![
            placed("card1-HDMI-A-2", "0000:00:02.0", 0, true),
            placed("card0-HDMI-A-1", "0000:01:00.0", 3840, false),
        ]);
        let layout = DisplayLayout::parse("screen 0000:01:00.0 HDMI-A-1 main\n");
        assert_eq!(restore_plan(&layout, &snap), vec![RestoreOp::Main("card0-HDMI-A-1".to_string())]);
        // The same main screen on a peer still to join: Main only.
        let snap = snapshot(vec![
            placed("card1-HDMI-A-2", "0000:00:02.0", 0, true),
            peer_pending("card0-HDMI-A-1", "0000:01:00.0"),
        ]);
        assert_eq!(restore_plan(&layout, &snap), vec![RestoreOp::Main("card0-HDMI-A-1".to_string())]);
    }

    #[test]
    fn restore_plan_ignores_unlisted_screens_for_order() {
        // Two saved screens in their saved order, a third (unlisted) one
        // placed between them: the saved relative order holds, no Order.
        let snap = snapshot(vec![
            placed("card0-DP-1", "0000:00:02.0", 0, true),
            placed("card0-DP-2", "0000:00:02.0", 1920, false),
            placed("card0-DP-3", "0000:00:02.0", 3840, false),
        ]);
        let layout = DisplayLayout::parse("screen 0000:00:02.0 DP-1 main\nscreen 0000:00:02.0 DP-3\n");
        assert_eq!(restore_plan(&layout, &snap), vec![]);
    }

    #[test]
    fn restore_plan_waits_for_a_peer_row_with_an_ordinary_status() {
        // The peer exists but has not presented: its own row is unplaced,
        // inactive, with a non-peer status. Still joining, so Order is
        // seeded and Main asked for; a failed screen is not joining.
        let mut starting = output("card0-HDMI-A-1", "0000:01:00.0");
        starting.status = "starting".to_string();
        let snap = snapshot(vec![placed("card1-HDMI-A-2", "0000:00:02.0", 0, true), starting]);
        let layout = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-2\nscreen 0000:01:00.0 HDMI-A-1 main\n",
        );
        assert!(restore_waiting(&layout, &snap));
        assert!(is_joining(&snap.outputs[1]));
        assert_eq!(
            restore_plan(&layout, &snap),
            vec![
                RestoreOp::Order(vec!["card1-HDMI-A-2".to_string(), "card0-HDMI-A-1".to_string()]),
                RestoreOp::Main("card0-HDMI-A-1".to_string()),
            ]
        );
        let mut failed = snap.clone();
        failed.outputs[1].status = "failed: no free display plane".to_string();
        assert!(!restore_waiting(&layout, &failed));
        assert!(!is_joining(&failed.outputs[1]));
        assert_eq!(restore_plan(&layout, &failed), vec![RestoreOp::Main("card0-HDMI-A-1".to_string())]);
    }

    // env_restore_plan: env present/absent/empty crossed with the plan
    // having something to filter, per var.

    fn full_plan() -> Vec<RestoreOp> {
        vec![
            RestoreOp::Order(vec!["card0-HDMI-A-2".to_string(), "card1-HDMI-A-1".to_string()]),
            RestoreOp::Mode("card0-HDMI-A-2".to_string(), Some("3840x2160@30".to_string())),
            RestoreOp::Main("card1-HDMI-A-1".to_string()),
        ]
    }

    #[test]
    fn env_restore_plan_absent_changes_nothing() {
        assert_eq!(env_restore_plan(full_plan(), None, None), full_plan());
    }

    #[test]
    fn env_restore_plan_order_pin_drops_only_order() {
        let plan = env_restore_plan(full_plan(), Some("whatever,non-empty"), None);
        assert_eq!(
            plan,
            vec![
                RestoreOp::Mode("card0-HDMI-A-2".to_string(), Some("3840x2160@30".to_string())),
                RestoreOp::Main("card1-HDMI-A-1".to_string()),
            ]
        );
    }

    #[test]
    fn env_restore_plan_modes_pin_drops_only_the_named_screens_mode() {
        let plan = env_restore_plan(full_plan(), None, Some("card0-HDMI-A-2=1920x1080"));
        assert_eq!(
            plan,
            vec![
                RestoreOp::Order(vec!["card0-HDMI-A-2".to_string(), "card1-HDMI-A-1".to_string()]),
                RestoreOp::Main("card1-HDMI-A-1".to_string()),
            ]
        );
        // A pinned var that does not name this screen leaves its Mode op
        // alone: the pin is per screen, not per variable.
        let plan = env_restore_plan(full_plan(), None, Some("card9-DP-9=1920x1080"));
        assert!(plan.iter().any(|op| matches!(op, RestoreOp::Mode(name, _) if name == "card0-HDMI-A-2")));
    }

    #[test]
    fn env_restore_plan_both_pinned_drops_order_and_mode_keeps_main() {
        assert_eq!(
            env_restore_plan(full_plan(), Some("x"), Some("card0-HDMI-A-2=1920x1080")),
            vec![RestoreOp::Main("card1-HDMI-A-1".to_string())]
        );
    }

    #[test]
    fn env_restore_plan_empty_string_does_not_pin() {
        // `order_env`/`modes_env` never hand this fn an empty string (the
        // `Option` is already `None` then, same as `gpu_env`), but the
        // pure fn is still defined for it: empty means nothing to pin.
        assert_eq!(env_restore_plan(full_plan(), Some(""), Some("")), full_plan());
    }

    #[test]
    fn merge_loaded_keeps_saved_screens_behind_an_early_main_pick() {
        // A main pick before the read: the working layout only knows the
        // live screen. The file also holds a docked screen not connected now.
        let mut working = DisplayLayout::parse("screen 0000:00:02.0 eDP-1 main\n");
        let loaded = DisplayLayout::parse(
            "render-on 0000:01:00.0 10de:2b85\n\
             screen 0000:01:00.0 DP-3 main mode=2560x1440@144\n\
             screen 0000:00:02.0 eDP-1 mode=1920x1200\n",
        );
        working.merge_loaded(&loaded, true, false);
        assert_eq!(working.render_on, loaded.render_on);
        assert_eq!(working.screens.len(), 2);
        assert_eq!(working.screens[0].key, key("0000:00:02.0", "eDP-1"));
        assert!(working.screens[0].main);
        assert_eq!(working.screens[0].mode, None);
        assert_eq!(working.screens[1].key, key("0000:01:00.0", "DP-3"));
        assert!(!working.screens[1].main);
        assert_eq!(working.screens[1].mode, Some("2560x1440@144".to_string()));
    }

    #[test]
    fn merge_loaded_takes_untouched_parts_from_the_read() {
        let mut working = DisplayLayout { render_on: Some("0000:01:00.0 10de:2b85".to_string()), screens: vec![] };
        let loaded = DisplayLayout::parse("render-on 0000:00:02.0 8086:a780\nscreen 0000:00:02.0 HDMI-A-2 main\n");
        working.merge_loaded(&loaded, false, true);
        assert_eq!(working.render_on, Some("0000:01:00.0 10de:2b85".to_string()));
        assert_eq!(working.screens, loaded.screens);
    }

    #[test]
    fn screen_key_is_none_without_pci() {
        let mut no_pci = output("card0-HDMI-A-2", "0000:00:02.0");
        no_pci.pci = None;
        assert_eq!(screen_key(&no_pci), None);
        let with_pci = output("card0-HDMI-A-2", "0000:00:02.0");
        assert_eq!(
            screen_key(&with_pci),
            Some(ScreenKey { pci: "0000:00:02.0".to_string(), connector: "HDMI-A-2".to_string() })
        );
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn screen_rows_run_left_to_right_then_unplaced_in_default_order() {
        // Default (snapshot) order is not left to right.
        let mut failed = output("card0-DP-1", "0000:00:02.0");
        failed.status = "failed: no mode".to_string();
        let snap = snapshot(vec![
            placed("card0-eDP-1", "0000:00:02.0", 3840, false),
            failed,
            placed("card1-HDMI-A-1", "0000:01:00.0", 0, true),
            peer_pending("card1-DP-3", "0000:01:00.0"),
            placed("card0-HDMI-A-2", "0000:00:02.0", 1920, false),
        ]);
        let (rows, placed_count) = screen_rows(&snap);
        let row_names: Vec<&str> = rows.iter().map(|&i| snap.outputs[i].name.as_str()).collect();
        assert_eq!(
            row_names,
            ["card1-HDMI-A-1", "card0-HDMI-A-2", "card0-eDP-1", "card0-DP-1", "card1-DP-3"]
        );
        assert_eq!(placed_count, 3);
        assert_eq!(screen_rows(&snapshot(Vec::new())), (Vec::new(), 0));
    }

    #[test]
    fn neighbour_swap_moves_one_place_and_refuses_the_ends() {
        let order = names(&["A", "B", "C"]);
        assert_eq!(swap_neighbour(&order, 1, -1), Some(names(&["B", "A", "C"])));
        assert_eq!(swap_neighbour(&order, 1, 1), Some(names(&["A", "C", "B"])));
        assert_eq!(swap_neighbour(&order, 0, -1), None);
        assert_eq!(swap_neighbour(&order, 2, 1), None);
        assert_eq!(swap_neighbour(&order, 3, -1), None);
        assert_eq!(swap_neighbour(&names(&["A"]), 0, 1), None);
    }

    #[test]
    fn fill_slots_keeps_the_others_where_they_were() {
        // The platform order with an unplaced saved screen (X) between two
        // placed ones and an unlisted one at the end.
        let full = names(&["A", "X", "B", "C", "Z"]);
        assert_eq!(fill_slots(&full, &names(&["B", "A", "C"])), names(&["B", "X", "A", "C", "Z"]));
        // Unknown names in the new order are ignored.
        assert_eq!(fill_slots(&full, &names(&["C", "Q", "B", "A"])), names(&["C", "X", "B", "A", "Z"]));
    }

    #[test]
    fn reorder_puts_the_given_keys_in_their_slots() {
        let mut l = DisplayLayout::parse(
            "screen 0000:00:02.0 HDMI-A-2 mode=3840x2160@30\nscreen 0000:01:00.0 DP-3 main\nscreen 0000:01:00.0 HDMI-A-1\n",
        );
        l.reorder(&[key("0000:01:00.0", "HDMI-A-1"), key("0000:00:02.0", "HDMI-A-2"), key("0000:09:00.0", "DP-1")]);
        let order: Vec<&str> = l.screens.iter().map(|e| e.key.connector.as_str()).collect();
        assert_eq!(order, ["HDMI-A-1", "DP-3", "HDMI-A-2"]);
        assert_eq!(l.screens[2].mode, Some("3840x2160@30".to_string()));
        assert!(l.screens[1].main);
    }

    #[test]
    fn screens_span_gpus_counts_placed_and_joining_screens() {
        let one_gpu = snapshot(vec![
            placed("card0-eDP-1", "0000:00:02.0", 0, true),
            placed("card0-HDMI-A-2", "0000:00:02.0", 1920, false),
        ]);
        assert!(!screens_span_gpus(&one_gpu));
        let mut failed = output("card1-DP-1", "0000:01:00.0");
        failed.card = "card1".to_string();
        failed.status = "failed: no mode".to_string();
        let mut with_failed = one_gpu.clone();
        with_failed.outputs.push(failed);
        assert!(!screens_span_gpus(&with_failed));
        let mut with_peer = one_gpu.clone();
        let mut peer = peer_pending("card1-HDMI-A-1", "0000:01:00.0");
        peer.card = "card1".to_string();
        with_peer.outputs.push(peer);
        assert!(screens_span_gpus(&with_peer));
        let mut two_placed = one_gpu;
        let mut other = placed("card1-HDMI-A-1", "0000:01:00.0", 3840, false);
        other.card = "card1".to_string();
        two_placed.outputs.push(other);
        assert!(screens_span_gpus(&two_placed));
    }

    #[test]
    fn restore_plan_orders_a_desktop_whose_screens_still_overlap() {
        // A peer's screen reported before the wide desktop gave it its
        // slice sits at (0, 0) on top of the rendering GPU's screen: the
        // tie must not read as "already in the saved order".
        let mut peer = placed("card0-HDMI-A-2", "0000:0c:00.0", 0, false);
        peer.width = 3840;
        peer.height = 2160;
        let mut own = placed("card1-HDMI-A-1", "0000:01:00.0", 0, true);
        own.width = 3840;
        own.height = 2160;
        let snap = snapshot(vec![peer, own]);
        let layout = DisplayLayout::parse(
            "screen 0000:0c:00.0 HDMI-A-2 mode=3840x2160@30\nscreen 0000:01:00.0 HDMI-A-1 main\n",
        );
        assert!(placed_overlap(&snap));
        assert!(restore_waiting(&layout, &snap));
        assert_eq!(
            restore_plan(&layout, &snap),
            vec![
                RestoreOp::Order(names(&["card0-HDMI-A-2", "card1-HDMI-A-1"])),
                RestoreOp::Mode("card0-HDMI-A-2".to_string(), Some("3840x2160@30".to_string())),
            ]
        );
        // Laid out side by side: no overlap, nothing to wait for.
        let mut apart = snap.clone();
        apart.outputs[1].desktop_position = Some((3840, 0));
        assert!(!placed_overlap(&apart));
        assert!(!restore_waiting(&layout, &apart));
    }
}
