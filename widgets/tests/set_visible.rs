//! `set_visible` on the input widgets: the trait default is a no-op that keeps reporting
//! visible, so a widget that does not implement it ignores `set_visible(cx, false)`.
use makepad_widgets::makepad_script::script;
use makepad_widgets::*;

fn new_cx() -> Cx {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| makepad_widgets::script_mod(vm));
    cx.with_vm(|vm| vm.capture_errors());
    cx
}

const IDS: [LiveId; 5] = [
    live_id!(slider),
    live_id!(text_input),
    live_id!(drop_down),
    live_id!(value_input),
    live_id!(drop_slider),
];

fn children(w: &WidgetRef) -> Vec<(LiveId, WidgetRef)> {
    let mut kids = Vec::new();
    w.children(&mut |id, c| kids.push((id, c)));
    kids
}

#[test]
fn input_widgets_hide_and_show() {
    let mut cx = new_cx();
    let doc = cx.with_vm(|vm| {
        vm.eval(script! {
            use mod.prelude.widgets.*
            View{
                slider := Slider{}
                text_input := TextInput{}
                drop_down := DropDown{ labels: ["a" "b"] }
                value_input := ValueInput{}
                drop_slider := DropSlider{}
            }
        })
    });
    let root = cx.with_vm(|vm| WidgetRef::script_from_value(vm, doc));
    let errs = cx.with_vm(|vm| vm.take_errors());
    assert!(errs.is_empty(), "{errs:?}");

    let kids = children(&root);
    assert_eq!(kids.iter().map(|(id, _)| *id).collect::<Vec<_>>(), IDS);
    for (id, w) in kids {
        assert!(w.visible(), "{id} starts visible");
        w.set_visible(&mut cx, false);
        assert!(!w.visible(), "{id} hides");
        w.set_visible(&mut cx, true);
        assert!(w.visible(), "{id} shows again");
    }
}

#[test]
fn input_widgets_start_hidden_from_script() {
    let mut cx = new_cx();
    let doc = cx.with_vm(|vm| {
        vm.eval(script! {
            use mod.prelude.widgets.*
            View{
                slider := Slider{ visible: false }
                text_input := TextInput{ visible: false }
                drop_down := DropDown{ visible: false }
                value_input := ValueInput{ visible: false }
                drop_slider := DropSlider{ visible: false }
            }
        })
    });
    let root = cx.with_vm(|vm| WidgetRef::script_from_value(vm, doc));
    let errs = cx.with_vm(|vm| vm.take_errors());
    assert!(errs.is_empty(), "{errs:?}");
    for (id, w) in children(&root) {
        assert!(!w.visible(), "{id} reads `visible: false`");
    }
}
