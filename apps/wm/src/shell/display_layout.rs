//! The saved display settings, `~/.config/makepad/wm/display-layout`: the
//! render-on GPU, the virtual desktop, and one display profile per set of
//! connected monitors (`linux_display::display_profiles`): their screens left to
//! right, which monitors each shows (two or more: a mirror) and which one it is
//! optimised for, the main screen, each monitor's mode, the ones that are off,
//! and the pacing.
//!
//! This module is the pure model only: parsing, serializing and capturing a
//! profile from the renderer's [`LinuxDisplaySnapshot`]. It never touches the
//! filesystem; the worker reads and writes the file through
//! `display_settings_path` and `write_display_setting` (`system_linux.rs`).
//!
//! # File format
//!
//! ```text
//! makepad-display-layout 2
//! render-on 0000:01:00.0 1234:5678
//! virtual 7680x2160@120 panels=mirror scale=200 follow=on
//! profile 00000000000000a1,00000000000000b2
//! screen main
//! output 0000:01:00.0 DP-1 00000000000000a1 mode=2560x1440@144
//! screen
//! output 0000:01:00.0 DP-2 00000000000000b2
//! pace auto
//! ```
//!
//! The WM's own lines come first: `render-on` has exactly
//! `validate_gpu_choice`'s shape (`<pci-address> <vendor>:<device>`);
//! `virtual <WxH>[@Hz] [panels=mirror|off] [scale=<hundredths>] [follow=on|off]`
//! (absent: the Panel desktop) makes the virtual display the desktop: the size
//! is the fixed size, or the fallback while `follow=on` (the Desk Link
//! viewer's window size decides once a viewer has reported); `panels=` is what
//! the physical panels show meanwhile (default `mirror`: scaled to fit; `off`:
//! not driven); `scale=` is the desktop's scale while virtual (default 200 =
//! 2.00x, clamped to the display scale range); `follow=` defaults to off. The
//! first valid line of each wins; malformed ones are skipped. Then the
//! profiles, in `DisplayProfiles`' own format.
//!
//! # Who applies what
//!
//! The direct backend applies the profiles itself: it reads this file (at
//! `display_profiles::layout_path`, the WM's config dir unless
//! `MAKEPAD_DISPLAY_LAYOUT` names another for tests) at start-up
//! and whenever the set of connected monitors changes (plugging in a monitor
//! applies the profile of the new set, without a restart).
//! `MAKEPAD_DISPLAY_ARRANGEMENT` and `MAKEPAD_DRM_MODES` set from outside pin
//! the arrangement or a monitor's mode over any profile. The WM only writes:
//! a profile is captured whenever a change the person applied is in place
//! (kept, for one that changes a mode). The render-on GPU is read by the
//! session script before the WM starts (a GPU change needs a restart:
//! `restart_gate` in `linux_controls.rs`); the virtual desktop is restored by
//! the WM ([`virtual_restore`]).
//!
//! A version-1 file's `screen`, `mirror` and `pace` lines (the old global
//! order, main screen, modes, Extended / Mirror switch and pacing) are not
//! read: that part of the saved layout resets. `display-gpu`, older still,
//! is read once into `render_on` when there is no `display-layout`.
//!
//! # Restart contract
//!
//! Saving a layout that moves the render-on GPU needs the renderer
//! restarted on the new choice, which the process cannot do to itself:
//! `restart_gate` (`linux_controls.rs`) asks the UI to confirm, and
//! `Event::Shutdown` then exits with code 75 (`EX_TEMPFAIL`) instead of
//! 0. A session manager that runs the WM as a service treats that code as a
//! request to relaunch it, not a crash (`systemd`'s
//! `RestartForceExitStatus=75`/`SuccessExitStatus=75`,
//! `tools/arch_usb/makepad-wm.service`); the next launch re-reads
//! `display-layout` through the same session script.
//!
//! Compiled for Linux only (the inner `cfg` below makes the module empty
//! elsewhere), so no other platform's behaviour changes.

#![cfg(target_os = "linux")]

use makepad_widgets::makepad_platform::linux_display::display_profiles::{DisplayProfile, DisplayProfiles, OutputKey, ProfileSettings};
use makepad_widgets::makepad_platform::linux_display::{LinuxDisplayOutput, LinuxDisplaySnapshot, VirtualDisplay, VirtualPanels};

use super::system_linux::{validate_gpu_choice, DISPLAY_SCALE_MAX, DISPLAY_SCALE_MIN};

/// The header line's keyword.
const HEADER_KEYWORD: &str = "makepad-display-layout";
/// The header line's version, written on every save.
const FORMAT_VERSION: &str = "2";
/// The status the renderer gives a connected screen on another GPU whose
/// peer has not joined the wide desktop yet (`OTHER_GPU_STATUS` in the
/// direct backend, private there, so mirrored).
pub const PEER_PENDING_STATUS: &str = "unsupported: driven by another GPU";

/// The whole saved file.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplayLayout {
    /// The render-on GPU; `None` is Auto.
    pub render_on: Option<String>,
    /// The virtual desktop; `None` is the Panel desktop.
    pub virtual_display: Option<SavedVirtual>,
    /// One per set of connected monitors.
    pub profiles: DisplayProfiles,
}

/// The `virtual` line: the virtual desktop's saved choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedVirtual {
    /// The fixed size and refresh (the fallback while `follow`) and what
    /// the panels show.
    pub mode: VirtualDisplay,
    /// The desktop scale while virtual, in hundredths.
    pub scale: u32,
    /// The Desk Link viewer's window size decides once it has reported.
    pub follow: bool,
}

impl SavedVirtual {
    /// The defaults when Virtual is first picked.
    pub const DEFAULT_SCALE: u32 = 200;

    pub fn first_pick() -> SavedVirtual {
        SavedVirtual {
            mode: VirtualDisplay { width: 7680, height: 2160, refresh_mhz: 120_000, panels: VirtualPanels::Mirror },
            scale: Self::DEFAULT_SCALE,
            follow: true,
        }
    }

    /// The saved choices with the size picker's `size` applied: Follow
    /// keeps the fixed size as the fallback, a fixed size turns Follow off.
    pub fn with_size(&self, size: VirtualSize) -> SavedVirtual {
        let mut out = *self;
        match size {
            VirtualSize::Follow => out.follow = true,
            VirtualSize::Fixed { width, height, refresh_mhz } => {
                out.follow = false;
                out.mode.width = clamp_even(width);
                out.mode.height = clamp_even(height);
                out.mode.refresh_mhz = refresh_mhz.clamp(1000, 1_000_000);
            }
        }
        out
    }

    /// The virtual display the desktop should have: the Desk Link viewer's
    /// window (`width, height, refresh_mhz`) while following and one has
    /// reported, else the saved size. Sizes are clamped and even.
    pub fn target(&self, viewer: Option<(u32, u32, u32)>) -> VirtualDisplay {
        match viewer.filter(|_| self.follow) {
            Some((width, height, refresh_mhz)) => VirtualDisplay {
                width: clamp_even(width),
                height: clamp_even(height),
                refresh_mhz: if (1000..=1_000_000).contains(&refresh_mhz) { refresh_mhz } else { self.mode.refresh_mhz },
                panels: self.mode.panels,
            },
            None => self.mode,
        }
    }

    fn parse(mut tokens: std::str::SplitWhitespace<'_>) -> Option<SavedVirtual> {
        let mode = tokens.next()?;
        let mut panels = VirtualPanels::Mirror;
        let mut scale = Self::DEFAULT_SCALE;
        let mut follow = false;
        for token in tokens {
            if let Some(value) = token.strip_prefix("panels=") {
                if let Some(parsed) = VirtualPanels::parse(value) {
                    panels = parsed;
                }
            } else if let Some(value) = token.strip_prefix("scale=") {
                if let Ok(parsed) = value.parse::<u32>() {
                    scale = parsed.clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX);
                }
            } else if let Some(value) = token.strip_prefix("follow=") {
                match value {
                    "on" => follow = true,
                    "off" => follow = false,
                    _ => {}
                }
            }
        }
        let mode = VirtualDisplay::parse(mode, panels.as_str()).ok()?;
        Some(SavedVirtual { mode, scale, follow })
    }

    fn line(&self) -> String {
        format!(
            "virtual {} panels={} scale={} follow={}\n",
            self.mode.mode_string(),
            self.mode.panels.as_str(),
            self.scale,
            if self.follow { "on" } else { "off" }
        )
    }
}

/// What the Size picker chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualSize {
    /// The Desk Link viewer's window decides.
    Follow,
    /// A preset or custom size.
    Fixed { width: u32, height: u32, refresh_mhz: u32 },
}

/// `value` within the virtual display's limits, rounded down to even.
fn clamp_even(value: u32) -> u32 {
    (value.clamp(VirtualDisplay::MIN_SIZE, VirtualDisplay::MAX_SIZE)) & !1
}

/// Whether `MAKEPAD_VIRTUAL_DISPLAY` was pinned from outside: set and
/// non-empty, and not the session script's export of the saved line
/// (`MAKEPAD_WM_VIRTUAL_FROM_SAVED=1`).
pub fn virtual_env_pinned(value: Option<&str>, from_saved: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty()) && from_saved != Some("1")
}

impl DisplayLayout {
    /// Parses the file's text. Tolerant: an empty line, a `#` comment, an
    /// unknown keyword or a malformed line is skipped rather than failing
    /// the whole parse.
    pub fn parse(text: &str) -> DisplayLayout {
        let mut render_on = None;
        let mut virtual_display = None;
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut tokens = line.split_whitespace();
            match tokens.next() {
                // The profiles follow; they are `DisplayProfiles`' to read.
                Some("profile") => break,
                Some("render-on") if render_on.is_none() => {
                    let value = tokens.collect::<Vec<_>>().join(" ");
                    if validate_gpu_choice(&value).is_ok() {
                        render_on = Some(value);
                    }
                }
                Some("virtual") if virtual_display.is_none() => virtual_display = SavedVirtual::parse(tokens),
                _ => {}
            }
        }
        DisplayLayout { render_on, virtual_display, profiles: DisplayProfiles::parse(text) }
    }

    /// Serializes back to the file's text; round-trips through
    /// [`DisplayLayout::parse`].
    pub fn serialize(&self) -> String {
        let mut out = format!("{HEADER_KEYWORD} {FORMAT_VERSION}\n");
        if let Some(render_on) = &self.render_on {
            out.push_str(&format!("render-on {render_on}\n"));
        }
        if let Some(saved) = &self.virtual_display {
            out.push_str(&saved.line());
        }
        out.push_str(&self.profiles.text());
        out
    }

    /// The first saved layout when there is no `display-layout`: the older
    /// `display-gpu` (carried over to `render_on` when it has
    /// `validate_gpu_choice`'s shape).
    pub fn migrate(display_gpu: Option<&str>) -> DisplayLayout {
        let render_on = display_gpu.and_then(|gpu| validate_gpu_choice(gpu).ok().map(str::to_string));
        DisplayLayout { render_on, ..Default::default() }
    }

    /// Merges the file's start-up read into a working layout this session
    /// edited before the read landed: the edited parts stand (a profile
    /// saved this session replaces the read one for its monitors), the rest
    /// comes from the read.
    pub fn merge_loaded(&mut self, loaded: &DisplayLayout, render_on_edited: bool, virtual_edited: bool) {
        let mine = std::mem::replace(&mut self.profiles, loaded.profiles.clone());
        for profile in mine.profiles {
            self.profiles.set(profile);
        }
        if !render_on_edited {
            self.render_on = loaded.render_on.clone();
        }
        if !virtual_edited {
            self.virtual_display = loaded.virtual_display;
        }
    }

    /// Saves `settings` as the profile of the monitors `snap` lists. `false`
    /// (nothing saved) when no output's card has a PCI address, so no
    /// monitor could be found again.
    pub fn save_profile(&mut self, settings: &ProfileSettings, snap: &LinuxDisplaySnapshot) -> bool {
        let connected = connected_keys(snap);
        if connected.is_empty() {
            return false;
        }
        self.profiles.set(DisplayProfile::capture(settings, &connected));
        true
    }
}

/// Each listed output (the virtual display aside) with its key across boots;
/// one whose card has no PCI address is left out.
pub fn connected_keys(snap: &LinuxDisplaySnapshot) -> Vec<(String, OutputKey)> {
    snap.outputs.iter().filter_map(|o| Some((o.name.clone(), OutputKey::of(o)?))).collect()
}

/// What the renderer runs now, as a profile's settings.
pub fn running_settings(snap: &LinuxDisplaySnapshot) -> ProfileSettings {
    ProfileSettings {
        arrangement: snap.arrangement.clone(),
        modes: snap.outputs.iter().map(|o| (o.name.clone(), o.mode_override.clone())).collect(),
        main: snap.main_screen().and_then(|s| s.outputs.first().cloned()),
        pacing: snap.pacing.clone(),
    }
}

/// The saved virtual desktop, when the renderer does not show it yet: what
/// to send. While following, the viewer's size may differ from the saved
/// fallback, so only the panels word is compared; a fixed size must match.
pub fn virtual_restore(layout: &DisplayLayout, snap: &LinuxDisplaySnapshot) -> Option<VirtualDisplay> {
    let saved = layout.virtual_display?;
    let holds = snap.virtual_display.is_some_and(|v| {
        v.panels == saved.mode.panels
            && (saved.follow || (v.width, v.height, v.refresh_mhz) == (saved.mode.width, saved.mode.height, saved.mode.refresh_mhz))
    });
    (!holds).then_some(saved.mode)
}

/// Whether the desktop's screens (shown, or on their way: [`is_joining`])
/// are driven by more than one card. The renderer cannot switch its GPU
/// live then, so "Render on" only takes effect at the next start.
pub fn screens_span_gpus(snap: &LinuxDisplaySnapshot) -> bool {
    let mut cards = snap
        .outputs
        .iter()
        .filter(|output| snap.screen_of(&output.name).is_some() || is_joining(output))
        .map(|output| output.card.as_str());
    match cards.next() {
        Some(first) => cards.any(|card| card != first),
        None => false,
    }
}

/// A connected screen on another GPU whose peer has not been created yet.
pub fn is_peer_pending(output: &LinuxDisplayOutput) -> bool {
    output.status.starts_with(PEER_PENDING_STATUS)
}

/// A listed output that is not on a screen of the desktop yet but is on its
/// way: neither off, failed, lost nor unsupported (other than a peer GPU's
/// output still to be created). Covers both a peer GPU's row before its peer
/// exists and the peer's own row before its first frame.
pub fn is_joining(output: &LinuxDisplayOutput) -> bool {
    if is_peer_pending(output) {
        return true;
    }
    let status = output.status.as_str();
    !output.active
        && !(status == "off"
            || status.starts_with("not driven")
            || status.starts_with("failed")
            || status.starts_with("lost")
            || status.starts_with("unsupported")
            || status.starts_with("active: not in the wide desktop"))
}

/// `name` minus its `cardN-` prefix, or `name` unchanged when it does not
/// have that shape.
pub fn connector_suffix(name: &str) -> String {
    makepad_widgets::makepad_platform::linux_display::display_profiles::connector_suffix(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::linux_display::{DisplayArrangement, LinuxDisplayScreen};

    fn output(name: &str, pci: &str, fingerprint: u64) -> LinuxDisplayOutput {
        LinuxDisplayOutput { name: name.to_string(), pci: Some(pci.to_string()), fingerprint, card: name.split('-').next().unwrap_or("").to_string(), active: true, status: "active".into(), ..Default::default() }
    }

    fn screen(name: &str, outputs: &[&str], primary: bool) -> LinuxDisplayScreen {
        LinuxDisplayScreen { name: name.into(), outputs: outputs.iter().map(|o| o.to_string()).collect(), optimise_for: outputs[0].into(), primary, ..Default::default() }
    }

    /// The person's desktop: [B] [A main] [C+D].
    fn users() -> LinuxDisplaySnapshot {
        let names = ["card1-DP-1", "card1-DP-2", "card1-DP-3", "card1-DP-4"];
        LinuxDisplaySnapshot {
            direct: true,
            outputs: names.iter().zip([0xa1, 0xb2, 0xc3, 0xd4]).map(|(n, fp)| output(n, "0000:01:00.0", fp)).collect(),
            screens: vec![screen("card1-DP-2", &["card1-DP-2"], false), screen("card1-DP-1", &["card1-DP-1"], true), screen("card1-DP-3", &["card1-DP-3", "card1-DP-4"], false)],
            arrangement: DisplayArrangement::parse("card1-DP-2,card1-DP-1,card1-DP-3+*card1-DP-4").unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn a_layout_round_trips_with_its_profiles_and_skips_the_old_lines() {
        let snap = users();
        let mut layout = DisplayLayout { render_on: Some("0000:01:00.0 1234:5678".into()), ..Default::default() };
        assert!(layout.save_profile(&running_settings(&snap), &snap));
        let text = layout.serialize();
        assert!(text.starts_with("makepad-display-layout 2\nrender-on 0000:01:00.0 1234:5678\nprofile 00000000000000a1,"), "{text}");
        assert_eq!(DisplayLayout::parse(&text), layout);
        let settings = layout.profiles.profiles[0].resolve(&connected_keys(&snap));
        assert_eq!(settings.arrangement, snap.arrangement);
        assert_eq!(settings.main.as_deref(), Some("card1-DP-1"), "the main screen in the middle");
        // A version-1 file: render-on and virtual stay, the old screen lines reset.
        let old = DisplayLayout::parse("makepad-display-layout 1\nrender-on 0000:01:00.0 1234:5678\nscreen 0000:01:00.0 HDMI-A-1 main mode=3840x2160@30\nmirror 0000:01:00.0 HDMI-A-1\npace slowest\nvirtual 1920x1080\n");
        assert_eq!(old.render_on.as_deref(), Some("0000:01:00.0 1234:5678"));
        assert!(old.virtual_display.is_some());
        assert!(old.profiles.profiles.is_empty());
        assert_eq!(DisplayLayout::parse("render-on bogus\n# note\n\n").render_on, None);
        assert_eq!(DisplayLayout::migrate(Some("0000:01:00.0 1234:5678")).render_on.as_deref(), Some("0000:01:00.0 1234:5678"));
        assert_eq!(DisplayLayout::migrate(Some("nope")), DisplayLayout::default());
    }

    #[test]
    fn a_profile_saved_this_session_outlives_a_late_read() {
        let snap = users();
        let mut working = DisplayLayout::default();
        working.save_profile(&running_settings(&snap), &snap);
        let mut other = snap.clone();
        other.outputs.truncate(1);
        let mut loaded = DisplayLayout { render_on: Some("0000:01:00.0 1234:5678".into()), ..Default::default() };
        loaded.save_profile(&ProfileSettings::default(), &other);
        let mut stale = snap.clone();
        stale.arrangement = DisplayArrangement::default();
        loaded.save_profile(&running_settings(&stale), &stale);
        working.merge_loaded(&loaded, false, false);
        assert_eq!(working.render_on, loaded.render_on);
        assert_eq!(working.profiles.profiles.len(), 2, "the read's other set stays");
        let mine = working.profiles.find(&[0xa1, 0xb2, 0xc3, 0xd4]).unwrap();
        assert_eq!(mine.screens.len(), 3, "this session's [B] [A] [C+D] wins over the read");
        assert!(!working.save_profile(&ProfileSettings::default(), &LinuxDisplaySnapshot::default()));
    }

    #[test]
    fn screens_span_gpus_counts_shown_and_joining_outputs() {
        let mut snap = users();
        assert!(!screens_span_gpus(&snap));
        let mut failed = output("card0-eDP-1", "0000:00:02.0", 1);
        failed.active = false;
        failed.status = "failed: no mode".into();
        snap.outputs.push(failed);
        assert!(!screens_span_gpus(&snap));
        let mut peer = output("card0-HDMI-A-2", "0000:00:02.0", 2);
        peer.active = false;
        peer.status = PEER_PENDING_STATUS.into();
        snap.outputs.push(peer);
        assert!(screens_span_gpus(&snap));
        let mut off = output("card0-DP-9", "0000:00:02.0", 3);
        off.active = false;
        off.status = "off".into();
        assert!(!is_joining(&off));
    }

    fn saved_virtual() -> SavedVirtual {
        SavedVirtual {
            mode: VirtualDisplay { width: 7680, height: 2160, refresh_mhz: 120_000, panels: VirtualPanels::Off },
            scale: 200,
            follow: true,
        }
    }

    #[test]
    fn virtual_line_round_trips_and_parses_tolerantly() {
        let layout = DisplayLayout { virtual_display: Some(saved_virtual()), ..Default::default() };
        let text = layout.serialize();
        assert!(text.contains("\nvirtual 7680x2160@120 panels=off scale=200 follow=on\n"), "{text}");
        assert_eq!(DisplayLayout::parse(&text), layout);
        assert!(!DisplayLayout::default().serialize().contains("virtual"));
        // Defaults: mirror, 2.00x, follow off; fractional refresh; scale clamped.
        let bare = DisplayLayout::parse("virtual 1920x1080@59.94\n").virtual_display.unwrap();
        assert_eq!(bare.mode, VirtualDisplay { width: 1920, height: 1080, refresh_mhz: 59_940, panels: VirtualPanels::Mirror });
        assert_eq!((bare.scale, bare.follow), (200, false));
        assert_eq!(DisplayLayout::parse(&DisplayLayout { virtual_display: Some(bare), ..Default::default() }.serialize()).virtual_display, Some(bare));
        assert_eq!(DisplayLayout::parse("virtual 1920x1080 scale=900\n").virtual_display.unwrap().scale, DISPLAY_SCALE_MAX);
        assert_eq!(DisplayLayout::parse("virtual 1920x1080 panels=sideways scale=x follow=maybe bogus\n").virtual_display.map(|v| (v.mode.panels, v.scale, v.follow)), Some((VirtualPanels::Mirror, 200, false)));
        // Malformed or out of range lines are skipped; the first valid one wins.
        assert_eq!(DisplayLayout::parse("virtual\nvirtual banana\nvirtual 10x10\n").virtual_display, None);
        let two = DisplayLayout::parse("virtual banana\nvirtual 3840x2160@120\nvirtual 1920x1080\n").virtual_display.unwrap();
        assert_eq!(two.mode.width, 3840);
    }

    #[test]
    fn virtual_size_choices_and_viewer_target() {
        let saved = saved_virtual();
        let fixed = saved.with_size(VirtualSize::Fixed { width: 2561, height: 1441, refresh_mhz: 144_000 });
        assert!(!fixed.follow);
        assert_eq!((fixed.mode.width, fixed.mode.height, fixed.mode.refresh_mhz, fixed.mode.panels), (2560, 1440, 144_000, VirtualPanels::Off));
        let follow = fixed.with_size(VirtualSize::Follow);
        assert!(follow.follow);
        assert_eq!(follow.mode, fixed.mode, "the fixed size stays as the fallback");
        // No viewer yet: the fallback; a viewer: its window, even, clamped.
        assert_eq!(follow.target(None), fixed.mode);
        let viewer = follow.target(Some((3841, 2161, 60_000)));
        assert_eq!((viewer.width, viewer.height, viewer.refresh_mhz, viewer.panels), (3840, 2160, 60_000, VirtualPanels::Off));
        assert_eq!(follow.target(Some((99999, 10, 0))).width, VirtualDisplay::MAX_SIZE);
        assert_eq!(follow.target(Some((99999, 10, 0))).height, VirtualDisplay::MIN_SIZE);
        assert_eq!(follow.target(Some((99999, 10, 0))).refresh_mhz, 144_000, "no refresh reported: the fallback's");
        // Not following: the viewer is ignored.
        assert_eq!(fixed.target(Some((3840, 2160, 60_000))), fixed.mode);
    }

    #[test]
    fn virtual_restore_compares_the_panels_and_a_fixed_size() {
        let layout = DisplayLayout { virtual_display: Some(saved_virtual()), ..Default::default() };
        let panel_snap = users();
        assert_eq!(virtual_restore(&layout, &panel_snap), Some(saved_virtual().mode));
        let on = |v: VirtualDisplay| LinuxDisplaySnapshot { virtual_display: Some(v), ..panel_snap.clone() };
        assert_eq!(virtual_restore(&layout, &on(saved_virtual().mode)), None);
        // Following: the viewer's size stands; the wrong panels word does not.
        let viewer = VirtualDisplay { width: 3840, height: 2160, ..saved_virtual().mode };
        assert_eq!(virtual_restore(&layout, &on(viewer)), None);
        let mirror = VirtualDisplay { panels: VirtualPanels::Mirror, ..saved_virtual().mode };
        assert_eq!(virtual_restore(&layout, &on(mirror)), Some(saved_virtual().mode));
        // Fixed: the size must match too.
        let fixed = DisplayLayout { virtual_display: Some(SavedVirtual { follow: false, ..saved_virtual() }), ..Default::default() };
        assert_eq!(virtual_restore(&fixed, &on(viewer)), Some(saved_virtual().mode));
        assert_eq!(virtual_restore(&DisplayLayout::default(), &on(viewer)), None);
    }

    #[test]
    fn virtual_env_is_pinned_only_from_outside() {
        assert!(virtual_env_pinned(Some("7680x2160@120 off"), None));
        assert!(!virtual_env_pinned(Some("7680x2160@120 off"), Some("1")));
        assert!(!virtual_env_pinned(Some(""), None));
        assert!(!virtual_env_pinned(None, None));
    }

    #[test]
    fn merge_loaded_keeps_an_edited_virtual_choice() {
        let loaded = DisplayLayout { virtual_display: Some(saved_virtual()), ..Default::default() };
        let mut working = DisplayLayout::default();
        working.merge_loaded(&loaded, false, true);
        assert_eq!(working.virtual_display, None);
        working.merge_loaded(&loaded, false, false);
        assert_eq!(working.virtual_display, Some(saved_virtual()));
    }
}
