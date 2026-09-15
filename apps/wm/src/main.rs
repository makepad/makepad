//! wm — the desktop window manager binary: the library's `App` with the
//! default build (processes over the client hub, no linked modules, the
//! Omarchy desk, the assistant as a child process). The all-in-one
//! builds are the same `App` with another `WmBuild` (apps/wm-all).

use makepad_wm::makepad_widgets::*;
use makepad_wm::App;

app_main!(
    App,
    font_set: International,
    font_assets: [
        "makepad_widgets/resources/jetbrains_mono_variable.ttf",
        "makepad_widgets/resources/NotoColorEmoji.ttf",
    ]
);
