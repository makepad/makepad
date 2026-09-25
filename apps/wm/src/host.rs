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

/// Whether this PLATFORM can host apps as child PROCESSES: a hub to accept
/// them, a spawner to start them, a shared framebuffer per tile, a pool to
/// keep them warm. The web, iOS, tvOS, Android and OHOS have none of that:
/// those builds host their linked modules in-process and nothing else.
/// Whether a build that could host processes WANTS to is the build's say
/// (`WmBuild::modules_only`, asked through `App::processes`).
pub const fn processes_available() -> bool {
    cfg!(not(any(
        target_arch = "wasm32",
        target_os = "ios",
        target_os = "tvos",
        target_env = "ohos"
    )))
}

/// Android: the APK's extracted native-library directory, where the child
/// launcher (`libmakepad_launch.so`) and every hostable app's library
/// (`libapp_<bin>.so`) live — the directory this library was loaded from.
#[cfg(target_os = "android")]
pub fn android_native_lib_dir() -> Option<std::path::PathBuf> {
    use std::ffi::{c_char, c_int, c_void, CStr};
    #[repr(C)]
    struct DlInfo {
        dli_fname: *const c_char,
        dli_fbase: *mut c_void,
        dli_sname: *const c_char,
        dli_saddr: *mut c_void,
    }
    extern "C" {
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
    }
    let mut info = DlInfo {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    let found = unsafe { dladdr(android_native_lib_dir as *const c_void, &mut info) };
    if found == 0 || info.dli_fname.is_null() {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy().into_owned();
    std::path::Path::new(&path).parent().map(|dir| dir.to_path_buf())
}

/// Android: the launcher and the library of the app whose binary name is
/// `bin`, when the APK ships it.
#[cfg(target_os = "android")]
pub fn android_app_binary(bin: &str) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let dir = android_native_lib_dir()?;
    let launcher = dir.join("libmakepad_launch.so");
    let lib = dir.join(format!("libapp_{}.so", bin.replace('-', "_")));
    (launcher.is_file() && lib.is_file()).then_some((launcher, lib))
}

/// Android: whether hosted apps are built on the phone before they run
/// (`MAKEPAD_WM_ONDEVICE_BUILD`, set by the APK's entry from the
/// `debug.makepad.wm.ondevice` property; clients.rs `launch_argv`).
#[cfg(target_os = "android")]
pub fn android_ondevice_builds() -> bool {
    std::env::var("MAKEPAD_WM_ONDEVICE_BUILD").map(|v| v == "1").unwrap_or(false)
}

/// Android: what a hosted child needs from the WM's environment — the APK
/// its assets are in, the app's data and cache directories, the density,
/// and the socket its shared frames come from (os/linux/android/
/// android_hosted.rs). Called once, before the first child starts.
#[cfg(target_os = "android")]
pub fn android_prepare_children(cx: &Cx) {
    use makepad_widgets::makepad_platform::os::linux::android::android_hosted as hosted;
    if let OsType::Android(params) = cx.os_type() {
        set_child_env(hosted::DATA_PATH_ENV, params.data_path.as_ref());
        set_child_env(hosted::CACHE_PATH_ENV, params.cache_path.as_ref());
        set_child_env(hosted::DENSITY_ENV, params.density.to_string().as_ref());
    }
    // nativeLibraryDir is <app dir>/lib/<abi>; the APK is <app dir>/base.apk.
    if let Some(apk) = android_native_lib_dir()
        .and_then(|dir| dir.parent().and_then(|lib| lib.parent()).map(|app| app.join("base.apk")))
    {
        set_child_env(hosted::APK_ENV, apk.as_os_str());
    }
    hosted::start_frame_server();
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
                target_os = "android",
                target_env = "ohos"
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
