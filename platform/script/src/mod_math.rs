use crate::heap::*;
use crate::makepad_live_id::live_id::*;
use crate::native::*;
use crate::numeric::NumericValue;
use crate::shader_builtins::*;
use crate::value::*;
use crate::*;

pub fn define_math_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let math = heap.new_module(id!(math));
    define_shader_builtins(heap, math, native);
}

/// What a call of `mod` runs: `mod(x, y)` is the modulo of shading
/// languages, `x - y * floor(x / y)` (the result takes y's sign, as
/// `wrap(x, y)`), component-wise on vectors. `mod` stays the name of the
/// module root, so `mod.math` and `use mod.x.*` mean what they did: only a
/// call of it, which was always an error, reaches this function
/// (`handle_call_args`). Not in any module, so `use mod.math.*` brings no
/// `mod` into scope.
pub fn define_mod_call(heap: &mut ScriptHeap, native: &mut ScriptNative) -> ScriptObject {
    let fn_obj = native.add_fn(heap, script_args!(x = 0.0, y = 0.0), |vm, args| {
        let x_val = vm
            .bx
            .heap
            .value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm
            .bx
            .heap
            .value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let ip = vm.bx.threads.cur_ref().trap.ip;
        let x_nv = NumericValue::from_script_value_heap(&vm.bx.heap, x_val, ip);
        let y_nv = NumericValue::from_script_value_heap(&vm.bx.heap, y_val, ip);
        if let (NumericValue::F64(x), NumericValue::F64(y)) = (&x_nv, &y_nv) {
            return ScriptValue::from_f64(x - y * (x / y).floor());
        }
        x_nv.zip_f32(y_nv, |x, y| x - y * (x / y).floor())
            .to_script_value_heap(&mut vm.bx.heap, &vm.bx.code)
    });
    heap.set_static(fn_obj.into());
    fn_obj
}
