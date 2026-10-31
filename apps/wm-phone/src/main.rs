//! wm-phone -- the phone's window manager. The same `App` as the desktop `wm`, in the Android
//! style (launcher, dock, one app at a time); the apps it offers start as domains of this process
//! (`relax run --privileged`), so nothing here links an app.

use makepad_app_wm::makepad_widgets::*;
use makepad_app_wm::{App, DesktopStyle, WmBuild};

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
        cx.set_global(WmBuild {
            style: DesktopStyle::Android,
            title: "makepad".to_string(),
            ..Default::default()
        });
    }
);
