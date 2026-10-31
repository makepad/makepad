//! Stage's house look under the window manager's sheet.
//!
//! Why (user, 149): "why are all the stage highlights blue on 149? I thought
//! they were orange". The WM (look: macOS dark) hands every app its sheet as
//! `MAKEPAD_WIDGET_STYLE`; Stage's accent is the house orange only with no
//! sheet on, and picking "Stage (house)" took the sheet off just until the
//! next read put the environment's back. Its own test process: it sets that
//! variable.

use makepad_look::palette;
use makepad_widgets::*;

fn accent(cx: &mut Cx) -> (u32, u32, u32) {
    let rgba = palette::get(cx).accent;
    (rgba >> 24, (rgba >> 16) & 0xff, (rgba >> 8) & 0xff)
}

/// Stage's order: the library, then the palette over it.
fn load(vm: &mut ScriptVm) {
    makepad_widgets::script_mod(vm);
    palette::script_mod(vm);
}

#[test]
fn the_house_pick_is_orange_under_the_wms_macos_dark_sheet() {
    std::env::set_var("MAKEPAD_WIDGET_STYLE", "macos-dark");
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();

    // No pick: the system's look, whose accent is blue.
    cx.with_vm(load);
    let (r, _, b) = accent(&mut cx);
    assert!(b > r, "macOS dark's accent is blue, got r={r} b={b}");

    // "Stage (house)" picked, as the picker and a saved `house` do: the next
    // module run -- the style reload the pick asks for -- is orange.
    cx.with_vm(|vm| {
        desktop_style::uninstall(vm);
        vm.with_reload(load);
    });
    let (r, g, b) = accent(&mut cx);
    assert!(r > b, "the house accent is orange (R>B), got r={r} g={g} b={b}");
    assert_eq!((r, g, b), (0xff, 0x5c, 0x39), "the house accent is Makepad orange");

    // And it stays: a later reload reads the sheet again.
    cx.with_vm(|vm| vm.with_reload(load));
    let (r, _, b) = accent(&mut cx);
    assert!(r > b, "the next reload put the WM's sheet back over the house pick: r={r} b={b}");
}
