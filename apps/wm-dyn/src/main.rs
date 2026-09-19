//! Desktop binary of the Android super-app (`src/lib.rs` is the Android
//! cdylib): the same desk, apps compiled on demand to Rust `dylib`s.

use makepad_wm::makepad_widgets::*;
use makepad_wm::App;

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
        cx.set_global(makepad_wm_dyn::wm_build());
    }
);

// On Android the app is the library's cdylib (`app_main!` there exports the
// JNI entry and defines no `main`); this binary only exists for the desktop.
#[cfg(target_os = "android")]
fn main() {}
