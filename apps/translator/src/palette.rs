//! The translator's palette: ink and vermilion, one set for a light system
//! and one for a dark one. The app reads the system's choice once at start
//! (`AndroidParams::night_mode` on a phone; dark elsewhere) and every widget
//! below takes its colours from `mod.translator`.

use makepad_widgets::*;

script_mod! {
    // Light: warm paper, white cards, deep ink type.
    mod.translator_light = {
        bg: #f4f1eb
        surface: #ffffff
        surface_2: #ece7df
        line: #ddd6ca
        ink: #1b2130
        ink_dim: #686d79
        ink_faint: #a2a5ad
        ink_card: #1f2b46
        on_ink: #f6f1e8
        on_ink_dim: #a9b3c9
        accent_on_ink: #ff9b7a
        vermilion: #e8502f
        vermilion_down: #c93e20
        vermilion_deep: #8c2a14
        vermilion_soft: #fbe2d9
        on_vermilion: #ffffff
        // The other speaker's bubble in a conversation.
        bubble_them: #ffffff
        frame: #10141f
        // A microphone that is capturing; idle microphones are gray.
        mic_live: #e8231b
    }
    // Dark: near-black ink, lifted cards, the same vermilion a little brighter.
    mod.translator_dark = {
        bg: #111318
        surface: #1b1e25
        surface_2: #262a33
        line: #30353f
        ink: #ece7df
        ink_dim: #9a9fab
        ink_faint: #5f6470
        ink_card: #22304f
        on_ink: #f6f1e8
        on_ink_dim: #a9b3c9
        accent_on_ink: #ff9b7a
        vermilion: #ee5a38
        vermilion_down: #c94626
        vermilion_deep: #7a2210
        vermilion_soft: #3b2420
        on_vermilion: #ffffff
        bubble_them: #262a33
        frame: #07090d
        mic_live: #ff3b30
    }
}

/// Is the system in dark mode? Phones say so at start; elsewhere dark.
pub fn system_is_dark(cx: &Cx) -> bool {
    match cx.os_type() {
        OsType::Android(params) => params.night_mode,
        _ => true,
    }
}

/// Define both palettes and make the system's one `mod.translator`; the base
/// widget theme follows, so stock widgets (toggles, scroll bars, fields) match.
/// Call before `makepad_widgets::script_mod`.
pub fn set_base(vm: &mut ScriptVm) {
    let dark = system_is_dark(vm.cx_mut());
    set_base_theme(vm.cx_mut(), if dark { BaseTheme::Dark } else { BaseTheme::Light });
}

pub fn apply(vm: &mut ScriptVm) {
    script_mod(vm);
    if base_theme(vm.cx_mut()) == BaseTheme::Light {
        script_eval!(vm, {
            mod.translator = mod.translator_light
        });
    } else {
        script_eval!(vm, {
            mod.translator = mod.translator_dark
        });
    }
}
