use crate::cursor::MouseCursor;
use crate::draw_list::DrawCallUniforms;
use crate::draw_list::DrawListUniforms;
use crate::draw_pass::DrawPassUniforms;
use crate::draw_pass::ScriptDrawPass;
use crate::window::MacosWindowChrome;
use crate::window::MacosWindowConfig;
use crate::window::MacosWindowKind;
use crate::window::MacosWindowLevel;
use crate::window::ScriptWindowHandle;
use crate::window::WindowBackdrop;
use crate::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    vm.new_handle_type(id!(geometry));

    let draw = vm.new_module(id!(draw));
    set_script_value_to_pod!(vm, draw.DrawCallUniforms);
    set_script_value_to_pod!(vm, draw.DrawListUniforms);
    set_script_value_to_pod!(vm, draw.DrawPassUniforms);
    set_script_value_to_api!(vm, draw.MouseCursor);
    set_script_value_to_api!(vm, draw.WindowBackdrop);
    set_script_value_to_api!(vm, draw.MacosWindowKind);
    set_script_value_to_api!(vm, draw.MacosWindowChrome);
    set_script_value_to_api!(vm, draw.MacosWindowLevel);
    set_script_value_to_api!(vm, draw.MacosWindowConfig);

    let pass_default = ScriptDrawPass::script_api(vm);
    vm.bx
        .heap
        .set_type_default(pass_default.as_object().unwrap());
    set_script_value!(vm, draw.ScriptDrawPass = pass_default);

    let window_default = ScriptWindowHandle::script_api(vm);
    vm.bx
        .heap
        .set_type_default(window_default.as_object().unwrap());
    set_script_value!(vm, draw.ScriptWindowHandle = window_default);

    NIL
}
