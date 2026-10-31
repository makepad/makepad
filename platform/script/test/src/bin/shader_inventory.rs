//! Text-only shader sweep; no OS initialization, event loop, or GPU context.
use makepad_widgets::*;

fn main() {
    let directory = std::env::args_os().nth(1).expect("usage: shader-inventory OUTPUT_DIRECTORY");
    makepad_script_test::prepare_shader_inventory();
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        vm.capture_errors();
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "widget registration failed:\n{}", errors.join("\n"));
        makepad_script_test::write_shader_inventory(vm, "widgets", std::path::Path::new(&directory)).unwrap();
    });
}
