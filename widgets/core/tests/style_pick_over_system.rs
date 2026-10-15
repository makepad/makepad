//! An app's own theme pick stands over the window system's sheet.
//!
//! The WM hands every app it starts its look as `MAKEPAD_WIDGET_STYLE` and
//! sends changes as `makepad_style` events. On 149 (WM look macOS dark) Stage's
//! orange accent showed blue: picking "Stage (house)" took the sheet off, and
//! the next read put the environment's sheet straight back. This file is its
//! own test process because it sets that variable.

use makepad_widgets_core::desktop_style::{self, DesktopStyle, StyleSheet};
use makepad_widgets_core::*;

fn cx_under_system_sheet() -> Cx {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    cx.with_vm(|vm| makepad_widgets_core::script_mod(vm));
    cx
}

fn worn(cx: &mut Cx) -> Option<String> {
    cx.with_vm(desktop_style::current_name)
}

fn wm_sends(cx: &mut Cx, id: &str) {
    let sheet = StyleSheet::named(id).expect("a catalogue sheet");
    desktop_style::handle_event(cx, &Event::Custom(sheet.to_json()));
}

#[test]
fn a_pick_stands_over_the_window_systems_sheet() {
    std::env::set_var("MAKEPAD_WIDGET_STYLE", "macos-dark");

    // No pick: the app wears the system's sheet and follows its changes.
    let mut cx = cx_under_system_sheet();
    assert_eq!(worn(&mut cx).as_deref(), Some("macos-dark"));
    wm_sends(&mut cx, "macos");
    assert_eq!(worn(&mut cx).as_deref(), Some("macos"), "the system's new look restyles an app that never picked");

    // The plain theme, picked: it stays plain on every later read and over
    // the system's next look.
    let mut cx = cx_under_system_sheet();
    cx.with_vm(desktop_style::uninstall);
    assert_eq!(worn(&mut cx), None, "the plain theme, picked, is worn");
    assert!(cx.with_vm(desktop_style::current).is_none(), "the environment's sheet comes back over the plain pick");
    assert_eq!(cx.with_vm(desktop_style::current_style), DesktopStyle::default());
    // The window controls stay the window system's (Stage lost its caption buttons
    // under the WM when they followed its plain pick).
    assert_eq!(desktop_style::system_style(&mut cx), Some(DesktopStyle::Macos));
    wm_sends(&mut cx, "windows");
    assert_eq!(worn(&mut cx), None, "the system's look replaced the app's pick");
    assert_eq!(desktop_style::system_style(&mut cx), Some(DesktopStyle::Windows), "the controls follow the system's new look");

    // A sheet, picked: the system's look does not replace it either.
    let mut cx = cx_under_system_sheet();
    cx.with_vm(|vm| desktop_style::install(vm, StyleSheet::named("macos").unwrap()));
    wm_sends(&mut cx, "macos-dark");
    assert_eq!(worn(&mut cx).as_deref(), Some("macos"));
}
