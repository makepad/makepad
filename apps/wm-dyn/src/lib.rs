//! Android super-app window manager: iOS-skinned desk, apps compiled on
//! demand to Rust `dylib`s and seated in-process. No statically linked
//! app crates — those live in the packaged checkout.
//!
//! This library is the Android cdylib (`cargo makepad android` builds the
//! package's `[lib]`); `src/main.rs` is the desktop binary. Both link the
//! engine dylib so every widgets/platform/std symbol in the process is the
//! one the on-demand app dylibs bind to.

// Never named below, but it has to be an upstream crate of this one for
// rustc to link widgets and friends from the engine `.so` instead of
// embedding them (an unnamed `--extern` is never loaded).
extern crate makepad_wm_engine;

use makepad_wm::{DesktopStyle, WmBuild};

/// The build: no modules linked, every tile a dylib compiled on demand.
pub fn wm_build() -> WmBuild {
    WmBuild {
        modules: vec![],
        modules_only: true,
        dynamic_dylibs: true,
        style: DesktopStyle::Ios,
        assistant: None,
        title: "wm dyn".to_string(),
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
        INTER_FONT_ASSET,
        ROBOTO_FLEX_FONT_ASSET,
    ],
    configure: |cx: &mut Cx| {
        cx.set_global(wm_build());
    }
);
