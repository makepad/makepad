//! An [`Sdf3`] field backed by a math-AOT-compiled splash expression.
//!
//! The splash math AOT (platform/script/src/math_aot) compiles a
//! pure-math splash function — `|p| length(p) - 1.0`, or `|x, y, z| ...`
//! — into a batch-evaluable [`CompiledMath`]. This adapter wraps that as
//! an [`Sdf3`] so the dual-contouring mesher (`sdf_to_mesh`, `SdfGrid3`)
//! can sample it like any built-in field.
//!
//! Precision contract: the field is evaluated at f32 input precision
//! (each coordinate is rounded to f32 before evaluation, exactly like the
//! batch entry), and the result is the compiled function's f64 output —
//! bit-identical to running the splash interpreter on the same
//! f32-rounded point.
//!
//! Wiring a `csg.implicit` surface verb is three lines at the verb site
//! (which owns the VM and the fn value):
//!
//! ```ignore
//! let aot = MathAot::new(vm);
//! let compiled = aot.compile(vm, fn_value, &[MathAotParam::Vec3], &uniform_types)?;
//! let mesh = sdf_to_mesh(SdfSplashExpr::new(compiled.into_inner()), min, max, depth);
//! ```
//!
//! For a parametric model (`|p, radius, k| ...`), pass the knob types as
//! `uniform_types`, call `set_uniforms(...)` with the current values, and
//! re-mesh on every knob change — the compile happens once.
//!
//! When `MathAot::compile` returns `None` the expression is outside the
//! pure-math subset; `csg.implicit` samples it through the owning splash VM
//! with `sdf_to_mesh_ref`. The AOT is an accelerator, never a semantic fork.

use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use makepad_script::math_aot::{CompiledMath, MathAotValue};

/// A compiled splash math expression as a signed distance field.
///
/// The compiled function's per-point parameters must be either one `vec3`
/// or three scalars; any UNIFORM parameters (a parametric model's knobs —
/// radius, twist, blend k) are set with [`SdfSplashExpr::set_uniforms`]
/// and can change between samplings without recompiling.
pub struct SdfSplashExpr {
    compiled: Box<dyn CompiledMath>,
    /// True: single vec3 point parameter; false: three scalar parameters.
    vec_param: bool,
    /// Current uniform values (call-shaped and lane-flattened).
    uniforms: Vec<MathAotValue>,
    uniform_lanes: Vec<f32>,
}

impl SdfSplashExpr {
    /// Wraps a compiled expression with a single `vec3` point parameter
    /// (`|p| ...` or `|p, knobs...| ...`).
    pub fn new(compiled: Box<dyn CompiledMath>) -> Self {
        Self {
            compiled,
            vec_param: true,
            uniforms: Vec::new(),
            uniform_lanes: Vec::new(),
        }
    }

    /// Wraps a compiled expression with three scalar point parameters
    /// (`|x, y, z| ...`).
    pub fn new_xyz(compiled: Box<dyn CompiledMath>) -> Self {
        Self {
            compiled,
            vec_param: false,
            uniforms: Vec::new(),
            uniform_lanes: Vec::new(),
        }
    }

    /// Sets the uniform parameter values (in the compiled function's
    /// declared order). Re-sample after this for the parametric-model
    /// loop — no recompile.
    pub fn set_uniforms(&mut self, uniforms: Vec<MathAotValue>) -> &mut Self {
        self.uniform_lanes.clear();
        for u in &uniforms {
            match u {
                MathAotValue::Scalar(v) => self.uniform_lanes.push(*v as f32),
                MathAotValue::Vec2(v) => self.uniform_lanes.extend_from_slice(v),
                MathAotValue::Vec3(v) => self.uniform_lanes.extend_from_slice(v),
                MathAotValue::Vec4(v) => self.uniform_lanes.extend_from_slice(v),
            }
        }
        self.uniforms = uniforms;
        self
    }

    /// Batch sampling straight through the compiled function: `xyz` holds
    /// 3 f32 lanes per point, `out` one f32 distance per point. This is
    /// the fast path for samplers that have their points in an array —
    /// the point loop runs inside the compiled function.
    pub fn distance_batch(&self, xyz: &[f32], out: &mut [f32]) {
        self.compiled.eval_batch(xyz, &self.uniform_lanes, out);
    }
}

impl Sdf3 for SdfSplashExpr {
    fn distance(&self, p: Vec3d) -> f64 {
        let x = p.x as f32;
        let y = p.y as f32;
        let z = p.z as f32;
        let mut args = if self.vec_param {
            vec![MathAotValue::Vec3([x, y, z])]
        } else {
            vec![
                MathAotValue::Scalar(x as f64),
                MathAotValue::Scalar(y as f64),
                MathAotValue::Scalar(z as f64),
            ]
        };
        args.extend_from_slice(&self.uniforms);
        self.compiled
            .call(&args)
            .expect("compiled splash field: wrong parameter shape")
    }
}
