//! What the window manager asks of the platform it runs on — and the line
//! between the desk, which runs everywhere, and the PROCESS host, which
//! only a native build has.
//!
//! The desk (layout, tiles, the shell, the AI pane, the module host, the
//! bus) is one program on every target. What differs is around it: a
//! native build hosts other apps as child processes over a localhost hub,
//! forks samplers for the bar, reads themes from `~/.makepad`, and can
//! tell a child its theme through the environment; the web build has none
//! of that — no processes, no sockets, no files, no environment, and a
//! clock that is the browser's, not `std::time`'s (which traps on wasm).
//! Every one of those differences is asked through here, so the rest of
//! the crate never branches on the target itself.

use makepad_widgets::*;

/// Seconds since the program started, monotonic, on every platform. The
/// only clock the desk keeps time with: `std::time::Instant` is not
/// implemented on wasm32 and traps on first use.
pub fn now() -> f64 {
    Cx::monotonic_now()
}

/// Seconds since the Unix epoch, as the platform knows them.
pub fn wall_now() -> f64 {
    Cx::time_now()
}

/// The two clocks, as `makepad_app_host::HostedApp` takes them.
pub const CLOCK: makepad_app_host::Clock = makepad_app_host::Clock { now, wall_now };

/// Whether this PLATFORM can host apps as child PROCESSES: a hub to accept
/// them, a spawner to start them, a shared framebuffer per tile, a pool to
/// keep them warm. The web, iOS, tvOS and Android have none of that:
/// those builds host their linked modules in-process and nothing else.
/// Whether a build that could host processes WANTS to is the build's say
/// (`WmBuild::modules_only`, asked through `App::processes`).
pub const fn processes_available() -> bool {
    cfg!(not(any(
        target_arch = "wasm32",
        target_os = "ios",
        target_os = "tvos",
        target_os = "android"
    )))
}

/// Windows: a program the WM starts runs without a console window. wm is a
/// windowed program with no console of its own, so every console program
/// it starts (cargo, an app linked as a console program, curl) would
/// otherwise get a console window of its own. Children talk to the WM over
/// pipes and the hub, never a console; with this flag each console child
/// gets a console that has no window, which its own children (rustc under
/// cargo) share.
pub fn no_console_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Whether this build runs ON a phone: the OS draws the status bar, the
/// island and the home indicator, owns rotation and appearance, and
/// reports the safe-area insets the shell lays out against
/// (`mobile::PhoneChrome`). A desktop window playing a phone (the iOS and
/// Android skins) draws all of that itself.
pub const fn device_phone() -> bool {
    cfg!(any(target_os = "ios", target_os = "android"))
}

/// Hand a child process a setting through its environment (the theme
/// file every hosted app styles itself from). Nothing where there are no
/// child processes: on wasm32 `std::env::set_var` panics outright.
pub fn set_child_env(key: &str, value: &std::ffi::OsStr) {
    #[cfg(not(target_arch = "wasm32"))]
    std::env::set_var(key, value);
    #[cfg(target_arch = "wasm32")]
    let _ = (key, value);
}

/// The storage namespace the desk keeps its own small state in (the
/// theme choice), on the Cx storage API — files under the makepad home
/// natively, the browser's store on the web.
pub const STORAGE: &str = "wm";

/// The key the chosen theme's name is kept under.
pub const THEME_KEY: &str = "theme";
/// The key the desktop behaviour (`warp` or `classic`) is kept under.
pub const DESKTOP_MODE_KEY: &str = "desktop.mode";
/// The key the desktop's look (`<sheet id> <dark|light>`) is kept under.
pub const DESKTOP_LOOK_KEY: &str = "desktop.look";
/// The key the dock auto-hide choice (`on` or `off`) is kept under.
pub const DOCK_AUTOHIDE_KEY: &str = "dock.autohide";
/// The key the shelf auto-hide choice (`left`, `right`, both or empty) is kept under.
pub const SHELF_AUTOHIDE_KEY: &str = "shelf.autohide";
/// The key the home page's arranged icon order is kept under (mobile_tiles
/// `HomeOrder::to_document`).
pub const HOME_ORDER_KEY: &str = "home.order";
/// The phone shell's look (`android-light|android-dark|ios-light|ios-dark`),
/// set by the home screen's switcher.
pub const SHELL_LOOK_KEY: &str = "shell.look";

/// Remember the theme the person chose: the omarchy-style state file
/// beside the themes natively (what the next start and every child reads),
/// and the desk's storage namespace everywhere, so the web build keeps
/// the choice across reloads too.
pub fn persist_theme_choice(cx: &mut Cx, name: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-theme"), name);
    }
    let _ = cx.storage(STORAGE).set(cx, THEME_KEY, name.as_bytes().to_vec());
}

/// Remember the desktop behaviour: a state file beside the theme's natively,
/// and the desk's storage namespace everywhere.
pub fn persist_desktop_mode(cx: &mut Cx, mode: crate::spacewarp::Mode) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-desktop-mode"), mode.name());
    }
    let _ = cx.storage(STORAGE).set(cx, DESKTOP_MODE_KEY, mode.name().as_bytes().to_vec());
}

/// The saved desktop behaviour (native; the web reads the storage when it starts).
pub fn load_desktop_mode() -> Option<crate::spacewarp::Mode> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = std::fs::read_to_string(crate::theme::themes_dir().join("../current-desktop-mode")).ok()?;
        return crate::spacewarp::Mode::from_name(&text);
    }
    #[cfg(target_arch = "wasm32")]
    None
}

/// Remember the desktop's look: the style sheet the person picked and
/// whether it is dark, beside the theme's state file natively and in the
/// desk's storage namespace everywhere.
pub fn persist_desktop_look(cx: &mut Cx, sheet_id: &str, dark: bool) {
    let text = format!("{} {}", sheet_id, if dark { "dark" } else { "light" });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-desktop-look"), &text);
    }
    let _ = cx.storage(STORAGE).set(cx, DESKTOP_LOOK_KEY, text.into_bytes());
}

/// The saved desktop look (native): the sheet id and the dark flag.
pub fn load_desktop_look() -> Option<(String, bool)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = std::fs::read_to_string(crate::theme::themes_dir().join("../current-desktop-look")).ok()?;
        let (id, look) = text.trim().split_once(' ')?;
        return Some((id.to_string(), look == "dark"));
    }
    #[cfg(target_arch = "wasm32")]
    None
}

/// Remember the dock auto-hide choice, beside the theme's state file
/// natively and in the desk's storage namespace everywhere.
pub fn persist_dock_autohide(cx: &mut Cx, on: bool) {
    let text = if on { "on" } else { "off" };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-dock-autohide"), text);
    }
    let _ = cx.storage(STORAGE).set(cx, DOCK_AUTOHIDE_KEY, text.as_bytes().to_vec());
}

/// The saved dock auto-hide choice (native; off when nothing was saved).
pub fn load_dock_autohide() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        return std::fs::read_to_string(crate::theme::themes_dir().join("../current-dock-autohide"))
            .is_ok_and(|text| text.trim() == "on");
    }
    #[cfg(target_arch = "wasm32")]
    false
}

/// Remember which shelves (left, right) auto-hide, beside the theme's state
/// file natively and in the desk's storage namespace everywhere.
pub fn persist_shelf_autohide(cx: &mut Cx, sides: [bool; 2]) {
    let text = [("left", sides[0]), ("right", sides[1])].iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect::<Vec<_>>().join(" ");
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-shelf-autohide"), &text);
    }
    let _ = cx.storage(STORAGE).set(cx, SHELF_AUTOHIDE_KEY, text.into_bytes());
}

/// Remember the touchpad's natural scrolling choice beside the theme's state
/// file (the direct backend reads the touchpad itself; elsewhere the OS owns it).
pub fn persist_natural_scroll(on: bool) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-natural-scroll"), if on { "on" } else { "off" });
    }
    let _ = on;
}

/// The saved natural scrolling choice: on (as on a Mac) unless turned off.
pub fn load_natural_scroll() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        return std::fs::read_to_string(crate::theme::themes_dir().join("../current-natural-scroll"))
            .map_or(true, |text| text.trim() != "off");
    }
    #[cfg(target_arch = "wasm32")]
    true
}

/// Applies the natural scrolling choice to the direct backend's touchpad.
pub fn apply_natural_scroll(on: bool) {
    #[cfg(target_os = "linux")]
    makepad_widgets::makepad_platform::linux_input::set_natural_scroll(on);
    let _ = on;
}

/// Remember the touchpad's tap to click choice beside natural scrolling's.
pub fn persist_tap_to_click(on: bool) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-tap-to-click"), if on { "on" } else { "off" });
    }
    let _ = on;
}

/// The saved tap to click choice: off unless turned on.
pub fn load_tap_to_click() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        return std::fs::read_to_string(crate::theme::themes_dir().join("../current-tap-to-click"))
            .map_or(false, |text| text.trim() == "on");
    }
    #[cfg(target_arch = "wasm32")]
    false
}

/// Applies tap to click to the direct backend's touchpad.
pub fn apply_tap_to_click(on: bool) {
    #[cfg(target_os = "linux")]
    makepad_widgets::makepad_platform::linux_input::set_tap_to_click(on);
    let _ = on;
}

/// The saved shelf auto-hide choice (native; both off when nothing was saved).
pub fn load_shelf_autohide() -> [bool; 2] {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = std::fs::read_to_string(crate::theme::themes_dir().join("../current-shelf-autohide")).unwrap_or_default();
        return [text.split_whitespace().any(|w| w == "left"), text.split_whitespace().any(|w| w == "right")];
    }
    #[cfg(target_arch = "wasm32")]
    [false, false]
}

/// Remember each app's half height on the shelf, beside the theme's state file
/// natively and in the desk's storage namespace everywhere.
pub fn persist_shelf_half(cx: &mut Cx, heights: &std::collections::HashMap<String, f64>) {
    let mut rows: Vec<String> = heights.iter().map(|(app, h)| format!("{app} {:.0}", h)).collect();
    rows.sort();
    let text = rows.join("\n");
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(crate::theme::themes_dir().join("../current-shelf-half"), &text);
    }
    let _ = cx.storage(STORAGE).set(cx, "shelf.half", text.into_bytes());
}

/// The saved half heights (native).
pub fn load_shelf_half() -> std::collections::HashMap<String, f64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = std::fs::read_to_string(crate::theme::themes_dir().join("../current-shelf-half")).unwrap_or_default();
        return text
            .lines()
            .filter_map(|l| {
                let (app, h) = l.rsplit_once(' ')?;
                Some((app.to_string(), h.parse().ok()?))
            })
            .collect();
    }
    #[cfg(target_arch = "wasm32")]
    Default::default()
}

/// The wall clock as the bar shows it when the platform's own sampler
/// (`date`, forked on a thread) has nothing to say: the web has no
/// processes, so the bar's clock is formatted here from the platform's
/// epoch seconds. UTC — the page has no time zone of its own yet.
pub fn fallback_clock(alt: bool) -> String {
    let secs = wall_now().max(0.0) as i64;
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let (hours, minutes) = (day_secs / 3600, (day_secs % 3600) / 60);
    // 1970-01-01 was a Thursday.
    const WEEKDAYS: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
    if alt {
        let (year, month, day) = civil_from_days(days);
        const MONTHS: [&str; 12] = [
            "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
        ];
        format!("{} {} {}", day, MONTHS[(month - 1) as usize], year)
    } else {
        format!("{} {:02}:{:02}", weekday, hours, minutes)
    }
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian
/// (Howard Hinnant's civil-from-days).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_civil_calendar_is_right_at_the_edges() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2000-02-29 is day 11016; 2026-09-02 is day 20698.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_698), (2026, 9, 2));
    }

    #[test]
    fn the_desk_knows_where_it_runs() {
        assert_eq!(
            processes_available(),
            cfg!(not(any(
                target_arch = "wasm32",
                target_os = "ios",
                target_os = "tvos",
                target_os = "android"
            )))
        );
        assert!(now() >= 0.0);
    }
}

/// Where the makepad home goes for a process with no home directory at
/// all: the platform's temp dir natively; on the web a virtual root —
/// there is no filesystem, every read of it fails cleanly and every
/// bundled fallback applies, whereas `std::env::temp_dir` PANICS there.
pub fn homeless_root() -> std::path::PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::temp_dir()
    }
    #[cfg(target_arch = "wasm32")]
    {
        std::path::PathBuf::from("/makepad")
    }
}
