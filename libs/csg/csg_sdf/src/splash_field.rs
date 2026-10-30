//! An [`Sdf3`] field backed by a Splash function compiled by the kernel
//! compiler (platform/script/compute).
//!
//! The document's function — `fn(p) { return length(p) - 1.0 }`, or
//! `fn(x, y, z) { .. }` — is compiled as a per-element kernel that reads one
//! point and writes one distance, so the mesher (`sdf_to_mesh`,
//! `mesh_field`) can sample it like any built-in field. The
//! kernel runs as native code where the host has a backend (ARM64) and on
//! the compiler's reference interpreter elsewhere; both give the same bits.
//!
//! Precision contract: points are rounded to f32, the function runs with
//! the kernel compiler's f32 semantics in `exact32` math (f32 polynomials,
//! unfused arithmetic: only IEEE f32 operations, so the SDF engine's GPU
//! bake computes the same bits), and the distance is that f32 result.
//!
//! Wiring a surface verb (which owns the VM and the fn value):
//!
//! ```ignore
//! let field = SdfSplashExpr::compile(vm, fn_object, SplashPoint::Vec3, &[])?;
//! let mesh = sdf_to_mesh(field, min, max, depth);
//! ```
//!
//! For a parametric model (`fn(p, radius, k)`), pass the knobs' lane counts
//! as `uniforms` (1 for a scalar, 2-4 for a vector), call `set_uniforms`
//! with the current values and re-mesh on every knob change — the compile
//! happens once.
//!
//! When compiling fails the function is outside the kernel language and
//! cannot be meshed as a field.

use makepad_script_compute_sched::kernel::KernelExt;
use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use makepad_script::*;
use makepad_script_compute::kernel::{KernelRef, MathMode};
use makepad_script_compute::vm_kernel::{self, Decl, Entry, VmKernel};
use makepad_script_compute::{Backend, ShaderError};
use std::fmt::Write;


/// How the field's function takes its point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplashPoint {
    /// One `vec3` parameter (`fn(p, knobs..)`).
    Vec3,
    /// Three scalar parameters (`fn(x, y, z, knobs..)`).
    Xyz,
}

/// A compiled Splash function as a signed distance field.
///
/// Uniform parameters (a parametric model's knobs: radius, twist, blend k)
/// follow the point parameters; they are set with
/// [`SdfSplashExpr::set_uniforms`] and can change between samplings
/// without recompiling.
///
/// The document's numbers the function reads from its scope (`height` in
/// `let height = 70mm`, then `fn(p) { .. height * 0.3 .. }`) are lifted:
/// kernel parameters set from the values the document gave them, not
/// constants in the program, so a document evaluated again with another
/// height is the same compiled kernel ([`SdfSplashExpr::structure_key`])
/// with other parameter values ([`SdfSplashExpr::params`]).
#[derive(Clone)]
pub struct SdfSplashExpr {
    kernel: KernelRef,
    /// Kernel param names, one per uniform lane, in declaration order.
    params: Vec<String>,
    values: Vec<f32>,
    /// The lifted numbers' values, by kernel parameter index (the uniforms'
    /// slots hold their current values).
    lifted: Vec<f32>,
    /// Per uniform lane, its kernel parameter index.
    slots: Vec<usize>,
    /// The program's key (hashing it walks the whole program: once).
    structure: u64,
}

/// The most document numbers a field lifts to parameters (the rest stay
/// constants): what the GPU route passes per call.
pub const MAX_LIFTED: usize = 16;

impl SdfSplashExpr {
    /// Compiles `f` (a script fn) as a field. `uniforms` holds the lane
    /// count (1-4) of each parameter after the point.
    pub fn compile(vm: &mut ScriptVm, f: ScriptObject, point: SplashPoint, uniforms: &[usize]) -> Result<SdfSplashExpr, Vec<ShaderError>> {
        let mut decls = vec![
            Decl::Input { name: "sdf_points".into(), ty: "vec3".into(), stride: None, offset: None, buffer: None },
            Decl::Output { name: "sdf_dist".into(), ty: "f32".into(), stride: None, offset: None, buffer: None },
        ];
        let mut args = match point {
            SplashPoint::Vec3 => "sdf_points[i]".to_string(),
            SplashPoint::Xyz => "sdf_points[i].x, sdf_points[i].y, sdf_points[i].z".to_string(),
        };
        let mut params = Vec::new();
        for (u, &lanes) in uniforms.iter().enumerate() {
            if !(1..=4).contains(&lanes) {
                return Err(vec![ShaderError::new(0, 1, format!("uniform {u}: {lanes} lanes (1-4 are supported)"))]);
            }
            let names: Vec<String> = (0..lanes).map(|l| format!("sdf_u{u}_{l}")).collect();
            for n in &names {
                decls.push(Decl::Param { name: n.clone(), default: 0.0, range: None });
            }
            if lanes == 1 {
                let _ = write!(args, ", {}", names[0]);
            } else {
                let _ = write!(args, ", vec{lanes}({})", names.join(", "));
            }
            params.extend(names);
        }
        // The host's entry: one point in, one distance out, the document's
        // fn bound by name.
        let entry = vm.eval(ScriptMod::new("", "", "sdf_splash_entry", 0, 0, &format!("let sdf_entry = fn(i) {{ sdf_dist[i] = sdf_field({args}) }}\n(sdf_entry)"), &[]));
        let Some(entry_fn) = entry.as_object() else {
            return Err(vec![ShaderError::new(0, 1, "the field entry did not evaluate to a fn".into())]);
        };
        let _entry_ref = vm.new_object_ref(entry_fn);
        let k = VmKernel {
            decls,
            entry: Entry::Element,
            entry_fn,
            math: MathMode::Exact32,
            uses: Vec::new(),
            bind: vec![("sdf_field".into(), f)],
        };
        let (kernel, lifted_named) = vm_kernel::compile_lifted(vm, &k, Backend::Native, MAX_LIFTED)?;
        let mut lifted = vec![0.0; kernel.param_count()];
        for (name, v) in &lifted_named {
            if let Some(i) = kernel.param_index(name) {
                lifted[i] = *v;
            }
        }
        let slots = params.iter().map(|n| kernel.param_index(n).expect("a declared uniform is a kernel parameter")).collect();
        let values = vec![0.0; params.len()];
        let structure = kernel.program_key();
        Ok(SdfSplashExpr { kernel, params, values, lifted, slots, structure })
    }

    /// The compiled program's identity (its whole AIR; lifted numbers are
    /// parameters, not part of it): equal keys are the same compiled code.
    pub fn structure_key(&self) -> u64 {
        self.structure
    }

    /// The field's identity: the program and its parameter values. Equal
    /// keys sample equal fields.
    pub fn key(&self) -> u64 {
        let mut h = self.structure_key() ^ 0x9e37_79b9_7f4a_7c15;
        for v in self.params() {
            h = (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    /// Every kernel parameter's value now, by parameter index (what one
    /// call reads).
    pub fn params(&self) -> Vec<f32> {
        let mut p = self.lifted.clone();
        for (slot, v) in self.slots.iter().zip(&self.values) {
            p[*slot] = *v;
        }
        p
    }

    /// The compiled kernel (its program and parameter table).
    pub fn kernel(&self) -> &KernelRef {
        &self.kernel
    }

    /// Sets the uniform values, lane-flattened in declaration order.
    /// Re-sample after this for the parametric-model loop — no recompile.
    pub fn set_uniforms(&mut self, lanes: &[f32]) -> &mut Self {
        assert!(lanes.len() == self.params.len(), "{} uniform lanes, the field takes {}", lanes.len(), self.params.len());
        self.values.copy_from_slice(lanes);
        self
    }

    /// Batch sampling straight through the compiled kernel: `xyz` holds 3
    /// f32 lanes per point, `out` one f32 distance per point. This is the
    /// fast path for samplers that have their points in an array — the
    /// point loop runs inside the kernel.
    pub fn distance_batch(&self, xyz: &[f32], out: &mut [f32]) {
        self.distance_batch_with(&self.params(), xyz, out)
    }

    /// [`SdfSplashExpr::distance_batch`] with every parameter given (by
    /// parameter index, as [`SdfSplashExpr::params`] lists them).
    pub fn distance_batch_with(&self, params: &[f32], xyz: &[f32], out: &mut [f32]) {
        assert!(xyz.len() == out.len() * 3);
        let count = out.len();
        let mut call = self.kernel.call();
        for (k, v) in params.iter().enumerate().take(self.kernel.param_count()) {
            call.set_param(self.kernel.param_view(k).name, *v);
        }
        call.input("sdf_points", xyz).expect("the field's point input");
        call.output("sdf_dist", out).expect("the field's distance output");
        call.run(count).expect("the field kernel runs");
    }
}

impl Sdf3 for SdfSplashExpr {
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        let xyz: Vec<f32> = ps.iter().flat_map(|p| [p.x as f32, p.y as f32, p.z as f32]).collect();
        let mut d = vec![0f32; ps.len()];
        self.distance_batch(&xyz, &mut d);
        for (o, x) in out.iter_mut().zip(d) {
            *o = x as f64;
        }
    }

    fn distance(&self, p: Vec3d) -> f64 {
        let mut out = [0f32; 1];
        self.distance_batch(&[p.x as f32, p.y as f32, p.z as f32], &mut out);
        out[0] as f64
    }
}
