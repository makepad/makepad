//! The window manager as an Android app whose apps are real processes.
//!
//! This library is the Android cdylib (`cargo makepad android` builds the
//! package's `[lib]`): the plain `makepad-wm` desk in its Android skin, with
//! processes. Every app it opens runs in a process of its own — the APK's
//! launcher (`launch/`) runs the app's library there — and reaches the desk
//! over its hub, exactly like a desktop child (apps/wm/src/clients.rs,
//! platform/src/os/linux/android/android_hosted.rs).

use makepad_wm::{DesktopStyle, WmBuild};

/// The build: no linked modules, every app a hosted child process.
pub fn wm_build() -> WmBuild {
    WmBuild {
        modules: vec![],
        modules_only: false,
        dynamic_dylibs: false,
        style: DesktopStyle::Android,
        assistant: None,
        title: "wm".to_string(),
    }
}

#[cfg(target_os = "android")]
use makepad_wm::makepad_widgets::*;
#[cfg(target_os = "android")]
use makepad_wm::App;

#[cfg(target_os = "android")]
app_main!(
    App,
    font_set: International,
    font_assets: [
        "makepad_widgets/resources/jetbrains_mono_variable.ttf",
        "makepad_widgets/resources/NotoColorEmoji.ttf",
        "makepad_widgets/resources/Inter.ttf",
    ],
    configure: |cx: &mut Cx| {
        // `adb shell setprop debug.makepad.wm.test_app calculator`: open an
        // app at start (the WM's MAKEPAD_WM_TEST_APP; a phone has no shell
        // environment to set it in).
        let test_app = system_property("debug.makepad.wm.test_app");
        if !test_app.is_empty() {
            std::env::set_var("MAKEPAD_WM_TEST_APP", test_app);
        }
        // `adb shell setprop debug.makepad.wm.ondevice 1`: build each app on
        // the phone before it runs (an APK packed with `--proc-toolchain`).
        if system_property("debug.makepad.wm.ondevice") == "1" {
            std::env::set_var("MAKEPAD_WM_ONDEVICE_BUILD", "1");
        }
        makepad_wm::android_prepare_children(cx);
        cx.set_global(wm_build());
    }
);

#[cfg(target_os = "android")]
fn system_property(name: &str) -> String {
    use std::ffi::{c_char, c_int, CString};
    extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    let Ok(name) = CString::new(name) else { return String::new() };
    let mut value = [0 as c_char; 92];
    let len = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    if len <= 0 {
        return String::new();
    }
    let bytes: Vec<u8> = value[..len as usize].iter().map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
