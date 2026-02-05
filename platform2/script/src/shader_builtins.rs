use crate::makepad_live_id::live_id::*;
use crate::heap::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::trap::*;
use crate::mod_pod::ScriptPodBuiltins;
use crate::vm::*;
use crate::value::*;
use crate::suggest::format_pod_type_from_builtins;
use crate::numeric::NumericValue;
use crate::*;
use makepad_math::{Vec2f, Vec3f, Vec4f};

// Helper trait to add vm-based conversion methods for NumericValue
trait NumericValueVmExt {
    fn from_script_value_vm(vm: &mut ScriptVm, value: ScriptValue) -> NumericValue;
    fn to_script_value_vm(self, vm: &mut ScriptVm) -> ScriptValue;
}

impl NumericValueVmExt for NumericValue {
    fn from_script_value_vm(vm: &mut ScriptVm, value: ScriptValue) -> NumericValue {
        NumericValue::from_script_value_heap(&vm.bx.heap, value, vm.bx.threads.cur_ref().trap.ip)
    }
    
    fn to_script_value_vm(self, vm: &mut ScriptVm) -> ScriptValue {
        self.to_script_value_heap(&mut vm.bx.heap, &vm.bx.code)
    }
}

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
    // 1 argument functions - support f64, Vec2f, Vec3f, Vec4f, Color
    native.add_method(heap, math, id_lut!(abs), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.abs()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(acos), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.acos()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(acosh), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.acosh()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(asin), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.asin()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(asinh), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.asinh()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(atan), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.atan()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(atan2), script_args!(y=0.0, x=0.0), |vm, args|{ 
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        // atan2 computes atan(y/x) with correct quadrant
        y_nv.zip_f32(x_nv, |y, x| y.atan2(x)).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(atanh), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.atanh()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(ceil), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.ceil()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(cos), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.cos()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(cosh), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.cosh()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(degrees), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.to_degrees()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(exp), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.exp()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(exp2), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.exp2()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(floor), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.floor()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(fract), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.fract()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(inverseSqrt), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.sqrt().recip()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(length), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let nv = NumericValue::from_script_value_vm(vm, x_val);
        // length returns a scalar for vectors
        ScriptValue::from_f64(nv.length())
    });
    native.add_method(heap, math, id_lut!(log), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.ln()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(log2), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.log2()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(radians), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.to_radians()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(round), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.round()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(sign), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| {
            if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 }
        }).to_script_value_vm(vm)
    });
    // sin is already in mod_math but we can overwrite or duplicate here, the user asked to add to math_module
    native.add_method(heap, math, id_lut!(sin), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.sin()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(sinh), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.sinh()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(sqrt), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.sqrt()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(tan), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.tan()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(tanh), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.tanh()).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(trunc), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        NumericValue::from_script_value_vm(vm, x_val).map_f32(|v| v.trunc()).to_script_value_vm(vm)
    });
    
    // Derivative functions (shader-only, return 0.0 in script runtime)
    native.add_method(heap, math, id_lut!(dFdx), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        // Return zero with same type as input
        let nv = NumericValue::from_script_value_vm(vm, x_val);
        nv.zero_like().to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(dFdy), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        // Return zero with same type as input
        let nv = NumericValue::from_script_value_vm(vm, x_val);
        nv.zero_like().to_script_value_vm(vm)
    });
    
    // 2 argument functions - support f64, Vec2f, Vec3f, Vec4f, Color
    native.add_method(heap, math, id_lut!(atan2), script_args!(y=0.0, x=0.0), |vm, args|{ 
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        y_nv.zip_f32(x_nv, |y, x| y.atan2(x)).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(distance), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        // distance returns a scalar (length of difference)
        let diff = x_nv.zip_f32(y_nv, |a, b| a - b);
        ScriptValue::from_f64(diff.length())
    });
    native.add_method(heap, math, id_lut!(dot), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        // dot returns a scalar (sum of component-wise products)
        ScriptValue::from_f64(x_nv.dot(y_nv))
    });
    native.add_method(heap, math, id_lut!(normalize), script_args!(x=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let nv = NumericValue::from_script_value_vm(vm, x_val);
        // normalize returns a unit vector in the same direction
        nv.normalize().to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(cross), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        // cross product (only defined for vec3)
        x_nv.cross(y_nv).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(max), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        x_nv.zip_f32(y_nv, |a, b| a.max(b)).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(min), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        x_nv.zip_f32(y_nv, |a, b| a.min(b)).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(pow), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        x_nv.zip_f32(y_nv, |a, b| a.powf(b)).to_script_value_vm(vm)
    });
    // modf (fmod) - float modulo
    native.add_method(heap, math, id_lut!(modf), script_args!(x=0.0, y=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        x_nv.zip_f32(y_nv, |a, b| a % b).to_script_value_vm(vm)
    });
    native.add_method(heap, math, id_lut!(step), script_args!(edge=0.0, x=0.0), |vm, args|{ 
        let edge_val = vm.bx.heap.value(args, id!(edge).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        // step can have scalar edge or matching type edge
        if let Some(edge_f) = edge_val.as_f64() {
            NumericValue::step_scalar(edge_f, x_nv).to_script_value_vm(vm)
        } else {
            let edge_nv = NumericValue::from_script_value_vm(vm, edge_val);
            edge_nv.zip_f32(x_nv, |e, x| if x < e { 0.0 } else { 1.0 }).to_script_value_vm(vm)
        }
    });

    // 3 argument functions - support f64, Vec2f, Vec3f, Vec4f, Color
    native.add_method(heap, math, id_lut!(clamp), script_args!(x=0.0, min=0.0, max=0.0), |vm, args|{ 
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let min_val = vm.bx.heap.value(args, id!(min).into(), vm.bx.threads.cur_ref().trap.pass());
        let max_val = vm.bx.heap.value(args, id!(max).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        // clamp can have scalar min/max or matching type
        if let (Some(min_f), Some(max_f)) = (min_val.as_f64(), max_val.as_f64()) {
            x_nv.clamp_scalar(min_f, max_f).to_script_value_vm(vm)
        } else {
            let min_nv = NumericValue::from_script_value_vm(vm, min_val);
            let max_nv = NumericValue::from_script_value_vm(vm, max_val);
            x_nv.zip_f32(min_nv, |x, m| x.max(m))
                .zip_f32(max_nv, |x, m| x.min(m))
                .to_script_value_vm(vm)
        }
    });
    native.add_method(heap, math, id_lut!(mix), script_args!(x=0.0, y=0.0, a=0.0), |vm, args|{
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let y_val = vm.bx.heap.value(args, id!(y).into(), vm.bx.threads.cur_ref().trap.pass());
        let a_val = vm.bx.heap.value(args, id!(a).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        let y_nv = NumericValue::from_script_value_vm(vm, y_val);
        // mix typically has scalar alpha, but can also have matching type alpha
        if let Some(a_f) = a_val.as_f64() {
            x_nv.mix_scalar(y_nv, a_f).to_script_value_vm(vm)
        } else {
            let a_nv = NumericValue::from_script_value_vm(vm, a_val);
            // Component-wise mix
            match (x_nv, y_nv, a_nv) {
                (NumericValue::Vec2(x), NumericValue::Vec2(y), NumericValue::Vec2(a)) => {
                    NumericValue::Vec2(Vec2f {
                        x: x.x * (1.0 - a.x) + y.x * a.x,
                        y: x.y * (1.0 - a.y) + y.y * a.y,
                    }).to_script_value_vm(vm)
                }
                (NumericValue::Vec3(x), NumericValue::Vec3(y), NumericValue::Vec3(a)) => {
                    NumericValue::Vec3(Vec3f {
                        x: x.x * (1.0 - a.x) + y.x * a.x,
                        y: x.y * (1.0 - a.y) + y.y * a.y,
                        z: x.z * (1.0 - a.z) + y.z * a.z,
                    }).to_script_value_vm(vm)
                }
                (NumericValue::Vec4(x), NumericValue::Vec4(y), NumericValue::Vec4(a)) => {
                    NumericValue::Vec4(Vec4f {
                        x: x.x * (1.0 - a.x) + y.x * a.x,
                        y: x.y * (1.0 - a.y) + y.y * a.y,
                        z: x.z * (1.0 - a.z) + y.z * a.z,
                        w: x.w * (1.0 - a.w) + y.w * a.w,
                    }).to_script_value_vm(vm)
                }
                (NumericValue::Color(x), NumericValue::Color(y), NumericValue::Color(a)) => {
                    NumericValue::Color(Vec4f {
                        x: x.x * (1.0 - a.x) + y.x * a.x,
                        y: x.y * (1.0 - a.y) + y.y * a.y,
                        z: x.z * (1.0 - a.z) + y.z * a.z,
                        w: x.w * (1.0 - a.w) + y.w * a.w,
                    }).to_script_value_vm(vm)
                }
                _ => {
                    // Fallback: treat alpha as scalar
                    let a_f = match a_nv {
                        NumericValue::F64(v) => v,
                        _ => 0.5, // Default alpha
                    };
                    x_nv.mix_scalar(y_nv, a_f).to_script_value_vm(vm)
                }
            }
        }
    });
    native.add_method(heap, math, id_lut!(smoothstep), script_args!(e0=0.0, e1=0.0, x=0.0), |vm, args|{
        let e0_val = vm.bx.heap.value(args, id!(e0).into(), vm.bx.threads.cur_ref().trap.pass());
        let e1_val = vm.bx.heap.value(args, id!(e1).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_val = vm.bx.heap.value(args, id!(x).into(), vm.bx.threads.cur_ref().trap.pass());
        let x_nv = NumericValue::from_script_value_vm(vm, x_val);
        // smoothstep can have scalar edges or matching type edges
        if let (Some(e0_f), Some(e1_f)) = (e0_val.as_f64(), e1_val.as_f64()) {
            NumericValue::smoothstep_scalar(e0_f, e1_f, x_nv).to_script_value_vm(vm)
        } else {
            let e0_nv = NumericValue::from_script_value_vm(vm, e0_val);
            let e1_nv = NumericValue::from_script_value_vm(vm, e1_val);
            // Component-wise smoothstep
            match (e0_nv, e1_nv, x_nv) {
                (NumericValue::Vec2(e0), NumericValue::Vec2(e1), NumericValue::Vec2(x)) => {
                    let smoothstep_f = |e0: f32, e1: f32, x: f32| {
                        let t = ((x - e0) / (e1 - e0)).max(0.0).min(1.0);
                        t * t * (3.0 - 2.0 * t)
                    };
                    NumericValue::Vec2(Vec2f {
                        x: smoothstep_f(e0.x, e1.x, x.x),
                        y: smoothstep_f(e0.y, e1.y, x.y),
                    }).to_script_value_vm(vm)
                }
                (NumericValue::Vec3(e0), NumericValue::Vec3(e1), NumericValue::Vec3(x)) => {
                    let smoothstep_f = |e0: f32, e1: f32, x: f32| {
                        let t = ((x - e0) / (e1 - e0)).max(0.0).min(1.0);
                        t * t * (3.0 - 2.0 * t)
                    };
                    NumericValue::Vec3(Vec3f {
                        x: smoothstep_f(e0.x, e1.x, x.x),
                        y: smoothstep_f(e0.y, e1.y, x.y),
                        z: smoothstep_f(e0.z, e1.z, x.z),
                    }).to_script_value_vm(vm)
                }
                (NumericValue::Vec4(e0), NumericValue::Vec4(e1), NumericValue::Vec4(x)) => {
                    let smoothstep_f = |e0: f32, e1: f32, x: f32| {
                        let t = ((x - e0) / (e1 - e0)).max(0.0).min(1.0);
                        t * t * (3.0 - 2.0 * t)
                    };
                    NumericValue::Vec4(Vec4f {
                        x: smoothstep_f(e0.x, e1.x, x.x),
                        y: smoothstep_f(e0.y, e1.y, x.y),
                        z: smoothstep_f(e0.z, e1.z, x.z),
                        w: smoothstep_f(e0.w, e1.w, x.w),
                    }).to_script_value_vm(vm)
                }
                (NumericValue::Color(e0), NumericValue::Color(e1), NumericValue::Color(x)) => {
                    let smoothstep_f = |e0: f32, e1: f32, x: f32| {
                        let t = ((x - e0) / (e1 - e0)).max(0.0).min(1.0);
                        t * t * (3.0 - 2.0 * t)
                    };
                    NumericValue::Color(Vec4f {
                        x: smoothstep_f(e0.x, e1.x, x.x),
                        y: smoothstep_f(e0.y, e1.y, x.y),
                        z: smoothstep_f(e0.z, e1.z, x.z),
                        w: smoothstep_f(e0.w, e1.w, x.w),
                    }).to_script_value_vm(vm)
                }
                _ => {
                    // Fallback: use scalar edges
                    let e0_f = match e0_nv { NumericValue::F64(v) => v, _ => 0.0 };
                    let e1_f = match e1_nv { NumericValue::F64(v) => v, _ => 1.0 };
                    NumericValue::smoothstep_scalar(e0_f, e1_f, x_nv).to_script_value_vm(vm)
                }
            }
        }
    });
    native.add_method(heap, math, id_lut!(fma), script_args!(a=0.0, b=0.0, c=0.0), |vm, args|{
        let a_val = vm.bx.heap.value(args, id!(a).into(), vm.bx.threads.cur_ref().trap.pass());
        let b_val = vm.bx.heap.value(args, id!(b).into(), vm.bx.threads.cur_ref().trap.pass());
        let c_val = vm.bx.heap.value(args, id!(c).into(), vm.bx.threads.cur_ref().trap.pass());
        let a_nv = NumericValue::from_script_value_vm(vm, a_val);
        let b_nv = NumericValue::from_script_value_vm(vm, b_val);
        let c_nv = NumericValue::from_script_value_vm(vm, c_val);
        // fma: a * b + c, component-wise
        a_nv.zip_f32(b_nv, |a, b| a * b)
            .zip_f32(c_nv, |ab, c| ab + c)
            .to_script_value_vm(vm)
    });
}

pub fn type_table_builtin(
    name: LiveId, 
    args: &[ScriptPodType], 
    builtins: &ScriptPodBuiltins,
    trap: ScriptTrap
) -> ScriptPodType {
    // Helper to format type names for error messages
    let fmt_ty = |t: ScriptPodType| format_pod_type_from_builtins(t, builtins);
    
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
                 script_err_invalid_args!(trap, "shader builtin {:?} requires 1 arg, got {}", name, args.len());
                 return builtins.pod_void;
             }
             let t = args[0];
             if is_any_float(t) {
                 return t;
             }
             script_err_type_mismatch!(trap, "shader builtin {:?} requires float/vec-float arg, got {}", name, fmt_ty(t));
             return builtins.pod_void;
        }
        id!(length) => {
            if args.len() != 1 {
                script_err_invalid_args!(trap, "shader builtin 'length' requires 1 arg, got {}", args.len());
                return builtins.pod_void;
            }
            let t = args[0];
            if is_any_float(t) {
                if t == vec2f_t || t == vec3f_t || t == vec4f_t { return f32_t; }
                if t == vec2h_t || t == vec3h_t || t == vec4h_t { return f16_t; }
                return t; 
            }
            script_err_type_mismatch!(trap, "shader builtin 'length' requires float/vec-float arg, got {}", fmt_ty(t));
            return builtins.pod_void;
       }
        // normalize: returns unit vector in same direction
        id!(normalize) => {
            if args.len() != 1 {
                script_err_invalid_args!(trap, "shader builtin 'normalize' requires 1 arg, got {}", args.len());
                return builtins.pod_void;
            }
            let t = args[0];
            if is_vec_float(t) {
                return t;
            }
            script_err_type_mismatch!(trap, "shader builtin 'normalize' requires vec-float arg, got {}", fmt_ty(t));
            return builtins.pod_void;
       }
        // Float or Int 1 argument
        id!(abs) | id!(sign) => {
            if args.len() != 1 {
                 script_err_invalid_args!(trap, "shader builtin {:?} requires 1 arg, got {}", name, args.len());
                 return builtins.pod_void;
             }
             let t = args[0];
             if is_any_float(t) || is_any_int(t) {
                 return t;
             }
             script_err_type_mismatch!(trap, "shader builtin {:?} requires float/int arg, got {}", name, fmt_ty(t));
             return builtins.pod_void;
        }
        // Float 2 arguments
        id!(atan2) | id!(pow) | id!(modf) => {
             if args.len() != 2 {
                 script_err_invalid_args!(trap, "shader builtin {:?} requires 2 args, got {}", name, args.len());
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && is_any_float(t1) {
                 return t1;
             }
             script_err_type_mismatch!(trap, "shader builtin {:?} requires matching float types, got {} and {}", name, fmt_ty(t1), fmt_ty(t2));
             return builtins.pod_void;
        }
        id!(step) => {
             if args.len() != 2 {
                 script_err_invalid_args!(trap, "shader builtin 'step' requires 2 args, got {}", args.len());
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
             script_err_type_mismatch!(trap, "shader builtin 'step' requires (float,float) or (scalar,vec-float), got {} and {}", fmt_ty(t1), fmt_ty(t2));
             return builtins.pod_void;
        }
        id!(distance) | id!(dot) => {
             if args.len() != 2 {
                 script_err_invalid_args!(trap, "shader builtin {:?} requires 2 args, got {}", name, args.len());
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && is_any_float(t1) {
                 if t1 == vec2f_t || t1 == vec3f_t || t1 == vec4f_t { return f32_t; }
                 if t1 == vec2h_t || t1 == vec3h_t || t1 == vec4h_t { return f16_t; }
                 return t1;
             }
             script_err_type_mismatch!(trap, "shader builtin {:?} requires matching float types, got {} and {}", name, fmt_ty(t1), fmt_ty(t2));
             return builtins.pod_void;
        }
        // cross product: only works on vec3
        id!(cross) => {
             if args.len() != 2 {
                 script_err_invalid_args!(trap, "shader builtin 'cross' requires 2 args, got {}", args.len());
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 {
                 if t1 == vec3f_t { return vec3f_t; }
                 if t1 == vec3h_t { return vec3h_t; }
             }
             script_err_type_mismatch!(trap, "shader builtin 'cross' requires two vec3 args, got {} and {}", fmt_ty(t1), fmt_ty(t2));
             return builtins.pod_void;
        }
        // Float or Int 2 arguments
        id!(max) | id!(min) => {
             if args.len() != 2 {
                 script_err_invalid_args!(trap, "shader builtin {:?} requires 2 args, got {}", name, args.len());
                 return builtins.pod_void;
             }
             let (t1, t2) = (args[0], args[1]);
             if t1 == t2 && (is_any_float(t1) || is_any_int(t1)) {
                 return t1;
             }
             script_err_type_mismatch!(trap, "shader builtin {:?} requires matching float/int types, got {} and {}", name, fmt_ty(t1), fmt_ty(t2));
             return builtins.pod_void;
        }
        // Float 3 arguments
        id!(mix) => {
             if args.len() != 3 {
                 script_err_invalid_args!(trap, "shader builtin 'mix' requires 3 args (x, y, alpha), got {}", args.len());
                 return builtins.pod_void;
             }
             let (t1, t2, t3) = (args[0], args[1], args[2]);
             // mix(x, y, a)
             if t1 == t2 && is_any_float(t1) {
                 if t3 == t1 { return t1; }
                 // vector with scalar alpha
                 if (t1 == vec2f_t || t1 == vec3f_t || t1 == vec4f_t) && (is_float(t3) || is_int(t3)){ return t1; }
             }
             script_err_type_mismatch!(trap, "shader builtin 'mix' requires matching float types for x,y and compatible alpha, got {}, {}, {}", fmt_ty(t1), fmt_ty(t2), fmt_ty(t3));
             return builtins.pod_void;
        }
        id!(smoothstep) | id!(fma) => {
             if args.len() != 3 {
                 script_err_invalid_args!(trap, "shader builtin {:?} requires 3 args, got {}", name, args.len());
                 return builtins.pod_void;
             }
             let (t1, t2, t3) = (args[0], args[1], args[2]);
             if t1 == t2 && t2 == t3 && is_any_float(t1) {
                 return t1;
             }
             script_err_type_mismatch!(trap, "shader builtin {:?} requires 3 matching float args, got {}, {}, {}", name, fmt_ty(t1), fmt_ty(t2), fmt_ty(t3));
             return builtins.pod_void;
        }
        // Clamp: Float or Int 3 arguments
        id!(clamp) => {
             if args.len() != 3 {
                 script_err_invalid_args!(trap, "shader builtin 'clamp' requires 3 args (value, min, max), got {}", args.len());
                 return builtins.pod_void;
             }
             let (t1, t2, t3) = (args[0], args[1], args[2]);
             if t1 == t2 && t2 == t3 && (is_any_float(t1) || is_any_int(t1)) {
                 return t1;
             }
             script_err_type_mismatch!(trap, "shader builtin 'clamp' requires 3 matching float/int args, got {}, {}, {}", fmt_ty(t1), fmt_ty(t2), fmt_ty(t3));
             return builtins.pod_void;
        }
        _ => {
             script_err_wrong_value!(trap, "unknown shader builtin function {:?}", name);
             builtins.pod_void
        }
    }
}

