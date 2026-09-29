// Included by application unit tests to validate the real registration path,
// including app-local Splash modules and aliases, through every sheet in the catalogue.
use super::*;

#[test]
fn application_styles_reload_without_script_errors() {
    use makepad_widgets::desktop_style::{catalogue, install, StyleSheet};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    cx.with_vm(|vm| {
        makepad_widgets::makepad_platform::script::script_mod(vm);
        App::script_mod(vm);
        for entry in catalogue() {
            install(vm, StyleSheet::load(entry));
            vm.bx.captured_errors = Some(Vec::new());
            vm.with_reload(App::script_mod);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{}: {errors:?}", entry.id);
        }
    });
}
