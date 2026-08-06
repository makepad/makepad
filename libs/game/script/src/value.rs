//! Script value <-> engine type conversions.
//!
//! Ported faithfully from gamemaker's game_view.rs helpers — the semantics
//! here are load-bearing (a missing option must read as NIL, not as an error
//! value that NaNs every downstream cast).

use makepad_widgets::*;
use makepad_widgets::makepad_script::numeric::NumericValue;


/// Positional argument, normalized so a missing slot reads as NIL.
pub fn arg(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    let v = vm.bx.heap.vec_value(args, index, NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

pub fn arg_f32(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> f32 {
    let v = arg(vm, args, index);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    vm.bx.heap.cast_to_f64(v, ip) as f32
}

pub fn arg_f64(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> f64 {
    let v = arg(vm, args, index);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    vm.bx.heap.cast_to_f64(v, ip)
}

pub fn arg_id(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> u64 {
    arg_f32(vm, args, index) as u64
}

pub fn arg_string(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> String {
    let v = arg(vm, args, index);
    vm.bx.heap.temp_string_with(|heap, out| {
        heap.cast_to_string(v, out);
        out.to_string()
    })
}

pub fn value_f32(vm: &mut ScriptVm, v: ScriptValue) -> f32 {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    vm.bx.heap.cast_to_f64(v, ip) as f32
}

pub fn value_bool(vm: &mut ScriptVm, v: ScriptValue) -> bool {
    if let Some(b) = v.as_bool() {
        return b;
    }
    value_f32(vm, v) != 0.0
}

pub fn value_string(vm: &mut ScriptVm, v: ScriptValue) -> String {
    vm.bx.heap.temp_string_with(|heap, out| {
        heap.cast_to_string(v, out);
        out.to_string()
    })
}

/// Report a wrong-typed option to the agent instead of coercing it silently.
///
/// A silent fallback is the most expensive failure mode we have: `size: [1,2,3]`
/// became `vec3(0,0,0)` and `color: "#ff0000"` became grey, so a generated game
/// looked correct in source and rendered an empty world with nothing to explain
/// why. The engine already hard-fails unknown verbs with a did-you-mean; typed
/// options deserve the same courtesy. NIL is exempt — a missing option is not a
/// mistake, it is a default (that distinction is load-bearing, see the module
/// docs).
fn wrong_type(vm: &mut ScriptVm, expected: &str, hint: &str) {
    let loc = vm
        .bx
        .code
        .ip_to_loc(vm.bx.threads.cur_ref().trap.ip)
        .map(|l| format!("{l}: "))
        .unwrap_or_default();
    let msg = format!("{loc}expected {expected} — {hint}");
    if let Some(sink) = vm.bx.captured_errors.as_mut() {
        sink.push(msg);
    }
}

pub fn value_vec3(vm: &mut ScriptVm, v: ScriptValue) -> Vec3f {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
        NumericValue::Vec3(v) => v,
        NumericValue::F64(f) => vec3f(f as f32, f as f32, f as f32),
        _ => {
            if !v.is_nil() {
                wrong_type(vm, "a vec3", "write vec3(x, y, z); an array like [x, y, z] is not a position");
            }
            vec3f(0.0, 0.0, 0.0)
        }
    }
}

pub fn value_color(vm: &mut ScriptVm, v: ScriptValue) -> Vec4f {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
        NumericValue::Color(c) => c,
        NumericValue::Vec4(c) => c,
        _ => {
            if !v.is_nil() {
                wrong_type(vm, "a color literal", "write #ff8800 (or #xff88ee when a digit is followed by e); a quoted \"#ff8800\" string is not a color");
            }
            vec4(0.8, 0.8, 0.8, 1.0)
        }
    }
}

pub fn vec3_value(vm: &mut ScriptVm, v: Vec3f) -> ScriptValue {
    NumericValue::Vec3(v).to_script_value_heap(&mut vm.bx.heap, &vm.bx.code)
}

/// Missing option keys come back as error values, not NIL — normalize both to
/// NIL so `is_nil()` means "not provided".
pub fn opts_value(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> ScriptValue {
    let v = vm.bx.heap.value(opts, key.into(), NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

pub fn opt_f32(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: f32) -> f32 {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        default
    } else {
        value_f32(vm, v)
    }
}

pub fn opt_bool(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: bool) -> bool {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        default
    } else {
        value_bool(vm, v)
    }
}

pub fn opt_vec3(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: Vec3f) -> Vec3f {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        default
    } else {
        value_vec3(vm, v)
    }
}

pub fn opt_color(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: Vec4f) -> Vec4f {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        default
    } else {
        value_color(vm, v)
    }
}

pub fn opt_string(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> Option<String> {
    let v = opts_value(vm, opts, key);
    if v.is_nil() {
        None
    } else {
        Some(value_string(vm, v))
    }
}

pub fn fn_ref(vm: &mut ScriptVm, v: ScriptValue) -> Option<ScriptObjectRef> {
    let obj = v.as_object()?;
    Some(vm.bx.heap.new_object_ref(obj))
}

/// Script `[...]` literals are ScriptArrays; some paths hand us vec-objects.
/// Accept either.
pub fn list_len(vm: &ScriptVm, v: ScriptValue) -> usize {
    if let Some(a) = v.as_array() {
        vm.bx.heap.array_len(a)
    } else if let Some(o) = v.as_object() {
        vm.bx.heap.vec_len(o)
    } else {
        0
    }
}

pub fn list_value(vm: &mut ScriptVm, v: ScriptValue, index: usize) -> ScriptValue {
    if let Some(a) = v.as_array() {
        vm.bx.heap.array_index(a, index, NoTrap)
    } else if let Some(o) = v.as_object() {
        vm.bx.heap.vec_value(o, index, NoTrap)
    } else {
        NIL
    }
}

/// Read a `points: [vec3, ...]` option into a Vec.
pub fn opt_points(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> Vec<Vec3f> {
    let v = opts_value(vm, opts, key);
    let len = list_len(vm, v);
    let mut out = Vec::with_capacity(len);
    for index in 0..len {
        let item = list_value(vm, v, index);
        out.push(value_vec3(vm, item));
    }
    out
}
