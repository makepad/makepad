use crate::makepad_live_id::live_id::*;
use crate::heap::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::mod_pod::*;
use crate::trap::*;
use crate::*;

pub fn define_shader_builtins(heap:&mut ScriptHeap, math:ScriptObject, native:&mut ScriptNative){
    // constants
    let consts = [
        (id_lut!(PI),3.141592653589793),
        (id_lut!(E), 2.718281828459045),
        (id_lut!(LN2), 0.6931471805599453),
        (id_lut!(LN10), 2.302585092994046),
        (id_lut!(LOG2E), 1.4426950408889634),
        (id_lut!(LOG10E), 0.4342944819032518),
        (id_lut!(SQRT1_2), 0.70710678118654757),
        (id_lut!(TORAD), 0.017453292519943295),
        (id_lut!(GOLDEN), 1.618033988749895),
    ];
    for (id, val) in consts{
        heap.set_value_def(math, id.into(),(val).into());
    }
    // 1 argument functions
    native.add_method(heap, math, id!(abs), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).abs().into() });
    native.add_method(heap, math, id!(acos), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).acos().into() });
    native.add_method(heap, math, id!(acosh), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).acosh().into() });
    native.add_method(heap, math, id!(asin), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).asin().into() });
    native.add_method(heap, math, id!(asinh), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).asinh().into() });
    native.add_method(heap, math, id!(atan), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).atan().into() });
    native.add_method(heap, math, id!(atanh), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).atanh().into() });
    native.add_method(heap, math, id!(ceil), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).ceil().into() });
    native.add_method(heap, math, id!(cos), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).cos().into() });
    native.add_method(heap, math, id!(cosh), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).cosh().into() });
    native.add_method(heap, math, id!(degrees), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).to_degrees().into() });
    native.add_method(heap, math, id!(exp), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).exp().into() });
    native.add_method(heap, math, id!(exp2), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).exp2().into() });
    native.add_method(heap, math, id!(floor), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).floor().into() });
    native.add_method(heap, math, id!(fract), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).fract().into() });
    native.add_method(heap, math, id!(inverseSqrt), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).sqrt().recip().into() });
    native.add_method(heap, math, id!(length), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).abs().into() });
    native.add_method(heap, math, id!(log), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).ln().into() });
    native.add_method(heap, math, id!(log2), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).log2().into() });
    native.add_method(heap, math, id!(radians), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).to_radians().into() });
    native.add_method(heap, math, id!(round), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).round().into() });
    native.add_method(heap, math, id!(sign), script_args!(x=0.0), |vm, args|{ 
        let x = script_value_f64!(vm, args.x);
        (if x > 0.0 {1.0} else if x < 0.0 {-1.0} else {0.0}).into()
    });
    // sin is already in mod_math but we can overwrite or duplicate here, the user asked to add to math_module
    native.add_method(heap, math, id!(sin), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).sin().into() });
    native.add_method(heap, math, id!(sinh), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).sinh().into() });
    native.add_method(heap, math, id!(sqrt), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).sqrt().into() });
    native.add_method(heap, math, id!(tan), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).tan().into() });
    native.add_method(heap, math, id!(tanh), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).tanh().into() });
    native.add_method(heap, math, id!(trunc), script_args!(x=0.0), |vm, args|{ script_value_f64!(vm, args.x).trunc().into() });
    
    // Derivative functions (shader-only, return 0.0 in script runtime)
    native.add_method(heap, math, id!(dFdx), script_args!(x=0.0), |_vm, _args|{ 0.0.into() });
    native.add_method(heap, math, id!(dFdy), script_args!(x=0.0), |_vm, _args|{ 0.0.into() });
    
    // 2 argument functions
    native.add_method(heap, math, id!(atan2), script_args!(y=0.0, x=0.0), |vm, args|{ 
        script_value_f64!(vm, args.y).atan2(script_value_f64!(vm, args.x)).into() 
    });
    native.add_method(heap, math, id!(distance), script_args!(x=0.0, y=0.0), |vm, args|{ 
        (script_value_f64!(vm, args.x) - script_value_f64!(vm, args.y)).abs().into() 
    });
    native.add_method(heap, math, id!(max), script_args!(x=0.0, y=0.0), |vm, args|{ 
        script_value_f64!(vm, args.x).max(script_value_f64!(vm, args.y)).into() 
    });
    native.add_method(heap, math, id!(min), script_args!(x=0.0, y=0.0), |vm, args|{ 
        script_value_f64!(vm, args.x).min(script_value_f64!(vm, args.y)).into() 
    });
    native.add_method(heap, math, id!(pow), script_args!(x=0.0, y=0.0), |vm, args|{ 
        script_value_f64!(vm, args.x).powf(script_value_f64!(vm, args.y)).into() 
    });
    native.add_method(heap, math, id!(step), script_args!(edge=0.0, x=0.0), |vm, args|{ 
        if script_value_f64!(vm, args.x) < script_value_f64!(vm, args.edge) {0.0.into()} else {1.0.into()}
    });

    // 3 argument functions
    native.add_method(heap, math, id!(clamp), script_args!(x=0.0, min=0.0, max=0.0), |vm, args|{ 
        script_value_f64!(vm, args.x).max(script_value_f64!(vm, args.min)).min(script_value_f64!(vm, args.max)).into()
    });
    native.add_method(heap, math, id!(mix), script_args!(x=0.0, y=0.0, a=0.0), |vm, args|{
        let x = script_value_f64!(vm, args.x);
        let y = script_value_f64!(vm, args.y);
        let a = script_value_f64!(vm, args.a);
        (x * (1.0 - a) + y * a).into()
    });
    native.add_method(heap, math, id!(smoothstep), script_args!(e0=0.0, e1=0.0, x=0.0), |vm, args|{
        let e0 = script_value_f64!(vm, args.e0);
        let e1 = script_value_f64!(vm, args.e1);
        let x = script_value_f64!(vm, args.x);
        let t = ((x - e0) / (e1 - e0)).max(0.0).min(1.0);
        (t * t * (3.0 - 2.0 * t)).into()
    });
    native.add_method(heap, math, id!(fma), script_args!(a=0.0, b=0.0, c=0.0), |vm, args|{
        (script_value_f64!(vm, args.a).mul_add(script_value_f64!(vm, args.b), script_value_f64!(vm, args.c))).into()
    });
}

pub fn type_table_builtin(
    name: LiveId, 
    args: &[ScriptPodType], 
    builtins: &ScriptPodBuiltins,
    trap: &ScriptTrap
) -> ScriptPodType {
    let f32_t = builtins.pod_f32;
    let f16_t = builtins.pod_f16;
    let u32_t = builtins.pod_u32;
    let i32_t = builtins.pod_i32;
    
    let vec2f_t = builtins.pod_vec2f;
    let vec3f_t = builtins.pod_vec3f;
    let vec4f_t = builtins.pod_vec4f;

    let vec2h_t = builtins.pod_vec2h;
    let vec3h_t = builtins.pod_vec3h;
    let vec4h_t = builtins.pod_vec4h;

    let vec2u_t = builtins.pod_vec2u;
    let vec3u_t = builtins.pod_vec3u;
    let vec4u_t = builtins.pod_vec4u;

    let vec2i_t = builtins.pod_vec2i;
    let vec3i_t = builtins.pod_vec3i;
    let vec4i_t = builtins.pod_vec4i;

    // Helpers to check types
    let is_float = |t| t == f32_t || t == f16_t;
    let is_int = |t| t == u32_t || t == i32_t;
    let is_vec_float = |t| t == vec2f_t || t == vec3f_t || t == vec4f_t || t == vec2h_t || t == vec3h_t || t == vec4h_t;
    let is_vec_int = |t| t == vec2u_t || t == vec3u_t || t == vec4u_t || t == vec2i_t || t == vec3i_t || t == vec4i_t;
    
    let is_any_float = |t| is_float(t) || is_vec_float(t);
    let is_any_int = |t| is_int(t) || is_vec_int(t);

    match name {
        // Float only 1 argument
        id!(acos) | id!(acosh) | id!(asin) | id!(asinh) | id!(atan) | id!(atanh) |
        id!(ceil) | id!(cos) | id!(cosh) | id!(degrees) | id!(exp) | id!(exp2) | id!(floor) |
        id!(fract) | id!(inverseSqrt) | id!(log) | id!(log2) | id!(radians) |
        id!(round) | id!(sin) | id!(sinh) | id!(sqrt) | id!(tan) | id!(tanh) | id!(trunc) |
        id!(dFdx) | id!(dFdy) => {
             if args.len() != 1 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let t = args[0];
             if is_any_float(t) {
                 return t;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        id!(length) => {
            if args.len() != 1 {
                trap.err_invalid_arg_count();
                return builtins.pod_void;
            }
            let t = args[0];
            if is_any_float(t) {
                if t == vec2f_t || t == vec3f_t || t == vec4f_t { return f32_t; }
                if t == vec2h_t || t == vec3h_t || t == vec4h_t { return f16_t; }
                return t; 
            }
            trap.err_invalid_arg_type();
            return builtins.pod_void;
       }
        // Float or Int 1 argument
        id!(abs) | id!(sign) => {
            if args.len() != 1 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let t = args[0];
             if is_any_float(t) || is_any_int(t) {
                 return t;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        // Float 2 arguments
        id!(atan2) | id!(pow) => {
             if args.len() != 2 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && is_any_float(t1) {
                 return t1;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        id!(step) => {
             if args.len() != 2 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && is_any_float(t1) {
                 return t1;
             }
             if is_vec_float(t2) && (
                t1 == f32_t && (t2 == vec2f_t || t2 == vec3f_t || t2 == vec4f_t) || 
                t1 == f16_t && (t2 == vec2h_t || t2 == vec3h_t || t2 == vec4h_t)) {
                 return t2;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        id!(distance) => {
             if args.len() != 2 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && is_any_float(t1) {
                 if t1 == vec2f_t || t1 == vec3f_t || t1 == vec4f_t { return f32_t; }
                 if t1 == vec2h_t || t1 == vec3h_t || t1 == vec4h_t { return f16_t; }
                 return t1;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        // Float or Int 2 arguments
        id!(max) | id!(min) => {
             if args.len() != 2 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && (is_any_float(t1) || is_any_int(t1)) {
                 return t1;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        // Float 3 arguments
        id!(mix) => {
             if args.len() != 3 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2, t3) = (args[0], args[1], args[2]);
             // mix(x, y, a)
             if t1 == t2 && is_any_float(t1) {
                 if t3 == t1 { return t1; }
                 // vector with scalar alpha
                 if (t1 == vec2f_t || t1 == vec3f_t || t1 == vec4f_t) && (is_float(t3) || is_int(t3)){ return t1; }
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        id!(smoothstep) | id!(fma) => {
             if args.len() != 3 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2, t3) = (args[0], args[1], args[2]);
             if t1 == t2 && t2 == t3 && is_any_float(t1) {
                 return t1;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        // Clamp: Float or Int 3 arguments
        id!(clamp) => {
             if args.len() != 3 {
                 trap.err_invalid_arg_count();
                 return builtins.pod_void;
             }
             let (t1, t2, t3) = (args[0], args[1], args[2]);
             if t1 == t2 && t2 == t3 && (is_any_float(t1) || is_any_int(t1)) {
                 return t1;
             }
             trap.err_invalid_arg_type();
             return builtins.pod_void;
        }
        _ => {
             trap.err_not_fn();
             builtins.pod_void
        }
    }
}

