//! Splash SDF kernels on the GPU.
//!
//! A part's custom distance function (`m.sdf({field: fn(p) {..}})`) is a
//! kernel compiled by the Splash kernel compiler (`makepad_script_compute`)
//! into AIR, the structured IR whose ops are exactly specified. Here that
//! AIR becomes a function of the bake shader, built as shader IR by the
//! shader compiler's host hook (`makepad_script_compute::shader_route`, inside
//! the compiler: the AIR and the shader IR are its values): the program
//! moves as data, never as source text, and every AIR op maps to the shader
//! op with the same IEEE meaning (min/max as selects, integer ops wrapping,
//! saturating conversions written out). The bake shader is `precise_math`
//! (no contraction, correctly rounded division and square root), so a baked
//! brick equals the CPU kernel's distances bit for bit.
//!
//! - The document's numbers the function reads (`vase_height`, ...) are
//!   kernel parameters ([`makepad_csg_sdf::SdfSplashExpr`] lifts them), so a
//!   parameter drag is the same compiled kernel: its values travel in the
//!   program texels (op `KERNEL`) and only the touched bricks bake again.
//! - A volume's kernels have slots ([`KernelSlot`], by program identity); a
//!   bake variant ([`KernelBake`]) holds the dispatch over them. A new
//!   program (a shape-structure change) is a new variant, compiled while the
//!   old bricks keep drawing.
//! - Limits: a kernel's AIR is bounded by construction (every loop has a
//!   cap); one whose worst case exceeds [`MAX_KERNEL_COST`] ops per sample,
//!   reads more than [`MAX_KERNEL_PARAMS`] parameters, uses f64, host calls,
//!   tables or more than [`MAX_FRAME_WORDS`] words of local arrays has no GPU
//!   route: its bricks bake on the CPU twin.


use crate::bake::DrawSdfBake;
use crate::march::DrawSdfMarch;
use makepad_csg_sdf::{Sdf3, SdfKernel, SdfNode, SdfSplashExpr};
use makepad_draw::*;
use makepad_script_compute::shader_route::{self, ShaderRoute};
use std::sync::Arc;

/// Worst-case AIR ops of one sample: past it a kernel bakes on the CPU (a
/// bake quad samples 512 points, a march step one; GPU watchdogs kill
/// draws of seconds).
pub const MAX_KERNEL_COST: u64 = 20_000;
/// Parameters a kernel call passes (four texels).
pub const MAX_KERNEL_PARAMS: usize = 16;
/// Words of local arrays (the AIR frame) a GPU kernel may use.
pub const MAX_FRAME_WORDS: u32 = 64;

/// A kernel's GPU route ([`ShaderRoute`]: its AIR checked and translated
/// into shader IR inside the compiler), with what the volume reads of it.
pub struct KernelAir {
    /// The program's identity ([`SdfSplashExpr::structure_key`]).
    pub key: u64,
    pub params: usize,
    /// Worst-case AIR ops per sample.
    pub cost: u64,
    route: ShaderRoute,
}

impl KernelAir {
    /// The GPU route of a Splash field, or why it has none.
    pub fn new(splash: &SdfSplashExpr) -> Result<KernelAir, String> {
        let key = splash.structure_key();
        let route = ShaderRoute::new(splash.kernel(), key, MAX_KERNEL_COST, MAX_KERNEL_PARAMS as u32, MAX_FRAME_WORDS);
        let info = route.info();
        if !info.error.is_empty() {
            return Err(info.error.to_string());
        }
        let (params, cost) = (info.params as usize, info.cost);
        Ok(KernelAir { key, params, cost, route })
    }
}

/// One kernel of a volume (by program identity).
#[derive(Clone)]
pub struct KernelSlot {
    /// The program's identity (or the field's key when it is no Splash program).
    pub key: u64,
    field: Arc<dyn Sdf3 + Send + Sync>,
    splash: Option<Arc<SdfSplashExpr>>,
    /// The GPU route; `None`: its bricks bake on the CPU twin.
    pub gpu: Option<Arc<KernelAir>>,
}

fn structure_key(k: &SdfKernel) -> u64 {
    k.splash.as_ref().map_or(k.key, |s| s.structure_key())
}

impl KernelSlot {
    pub fn new(k: &SdfKernel) -> KernelSlot {
        let gpu = k.splash.as_ref().and_then(|s| match KernelAir::new(s) {
            Ok(a) => Some(Arc::new(a)),
            Err(e) => {
                log!("sdf engine: kernel {:016x} bakes on the CPU: {e}", structure_key(k));
                None
            }
        });
        KernelSlot { key: structure_key(k), field: k.field.clone(), splash: k.splash.clone(), gpu }
    }

    /// Whether `k` runs this slot's program.
    pub fn matches(&self, k: &SdfKernel) -> bool {
        self.key == structure_key(k)
    }

    /// [`KernelSlot::distance`] at many points (one kernel call).
    pub fn distance_many(&self, params: &[f32], ps: &[[f32; 3]]) -> Vec<f32> {
        match &self.splash {
            Some(s) => {
                let xyz: Vec<f32> = ps.iter().flatten().copied().collect();
                let mut out = vec![0.0f32; ps.len()];
                s.distance_batch_with(params, &xyz, &mut out);
                out
            }
            None => ps.iter().map(|p| self.distance(params, *p)).collect(),
        }
    }

    /// The distance at `p` with parameters `params` (by parameter index).
    pub fn distance(&self, params: &[f32], p: [f32; 3]) -> f32 {
        match &self.splash {
            Some(s) => {
                let mut out = [0.0f32];
                s.distance_batch_with(params, &p, &mut out);
                out[0]
            }
            None => self.field.distance(makepad_csg_math::Vec3d::new(p[0] as f64, p[1] as f64, p[2] as f64)) as f32,
        }
    }
}

/// Whether `n` calls any kernel.
pub fn has_kernel(n: &SdfNode) -> bool {
    match n {
        SdfNode::Kernel(_) => true,
        SdfNode::Combine { of, .. } => of.iter().any(has_kernel),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } => has_kernel(of),
        SdfNode::Material { within, of, .. } => within.as_deref().is_some_and(has_kernel) || has_kernel(of),
        _ => false,
    }
}

/// Adds the kernels of `n` missing from `slots` (their order is stable:
/// a slot keeps its index while its volume lives).
pub fn collect(n: &SdfNode, slots: &mut Vec<KernelSlot>) {
    match n {
        SdfNode::Kernel(k) => {
            if !slots.iter().any(|s| s.matches(k)) {
                slots.push(KernelSlot::new(k));
            }
        }
        SdfNode::Combine { of, .. } => of.iter().for_each(|c| collect(c, slots)),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } => collect(of, slots),
        SdfNode::Material { within, of, .. } => {
            if let Some(w) = within {
                collect(w, slots);
            }
            collect(of, slots);
        }
        _ => {}
    }
}

/// A volume's shaders with its kernels compiled in: the bake's, and the
/// march's (made from the march its host draws with, once asked for).
pub struct KernelBake {
    /// The kernels' program identities (slot order) it was made for.
    pub keys: Vec<u64>,
    pub draw: DrawSdfBake,
    /// The march with the kernels (exact evaluation near the surface), and
    /// the shader it was made from.
    march: Option<(DrawShaderId, DrawSdfMarch)>,
    /// Every GPU kernel is cheap enough to evaluate in the march.
    pub march_ok: bool,
    stub: ScriptObject,
    refs: Vec<ScriptObjectRef>,
}

/// Worst-case ops per sample of a kernel the march evaluates exactly (a ray
/// may evaluate it at every step in the band and four times for its
/// normal); costlier kernels march their baked copy.
pub const MAX_MARCH_KERNEL_COST: u64 = 2_000;

/// `base` with the kernels' dispatch `own` and the fetching kernel_at.
fn variant(vm: &mut ScriptVm, base: ScriptObject, own: ScriptObject) -> Option<ScriptObject> {
    let fetching = vm.value(base, id!(kernel_at_fetch)).as_object()?;
    let obj = vm.new_with_proto(base);
    vm.set_value(obj, id!(kernel_call), own);
    vm.set_value(obj, id!(kernel_at), fetching);
    Some(obj)
}

impl KernelBake {
    /// The bake variant for `slots` (`None` when none has a GPU route).
    pub fn new(cx: &mut Cx, slots: &[KernelSlot]) -> Option<KernelBake> {
        if slots.iter().all(|s| s.gpu.is_none()) {
            return None;
        }
        let keys: Vec<u64> = slots.iter().map(|s| s.key).collect();
        let mut key = 0xcbf2_9ce4_8422_2325u64;
        for (k, s) in keys.iter().zip(slots) {
            key = (key ^ k ^ s.gpu.is_some() as u64).wrapping_mul(0x100_0000_01b3);
        }
        let march_ok = slots.iter().all(|s| s.gpu.as_ref().is_some_and(|g| g.cost <= MAX_MARCH_KERNEL_COST));
        let routes: Vec<Option<&ShaderRoute>> = slots.iter().map(|s| s.gpu.as_ref().map(|g| &g.route)).collect();
        cx.with_vm(|vm| {
            let draw_mod = vm.heap().module(id!(draw));
            let base = vm.value(draw_mod, id!(SdfBake)).as_object()?;
            let stub = vm.value(base, id!(kernel_call)).as_object()?;
            // a fn object of its own (the shader's identity and the hook's key) with the dispatch
            let own = shader_route::dispatch_fn(vm, stub, key, &routes)?;
            let obj = variant(vm, base, own)?;
            let refs = vec![vm.new_object_ref(own), vm.new_object_ref(obj)];
            let draw = DrawSdfBake::script_from_value(vm, obj.into());
            Some(KernelBake { keys, draw, march: None, march_ok, stub: own, refs })
        })
    }

    /// The march `base` with the kernels compiled in (made once per base
    /// shader); `None` when a kernel is too costly to evaluate per step.
    pub fn march(&mut self, cx: &mut Cx, base: &DrawSdfMarch) -> Option<&mut DrawSdfMarch> {
        if !self.march_ok {
            return None;
        }
        let id = base.draw_super.draw_vars.draw_shader_id?;
        if self.march.as_ref().is_none_or(|(b, _)| *b != id) {
            let source = cx.draw_shaders.shaders.get(id.index)?.mapping.source.as_object();
            let own = self.stub;
            let made = cx.with_vm(|vm| {
                let obj = variant(vm, source, own)?;
                let r = vm.new_object_ref(obj);
                let mut draw = DrawSdfMarch::script_from_value(vm, obj.into());
                draw.kernels = true;
                Some((r, draw))
            });
            let (r, draw) = made?;
            self.refs.push(r);
            self.march = Some((id, draw));
        }
        self.march.as_mut().map(|(_, d)| d)
    }

    /// Whether this variant runs `slots`' kernels.
    pub fn serves(&self, slots: &[KernelSlot]) -> bool {
        self.keys.len() == slots.len() && self.keys.iter().zip(slots).all(|(k, s)| *k == s.key)
    }

    pub fn release(self, cx: &mut Cx) {
        let stub = self.stub;
        cx.with_vm(|vm| shader_route::release_dispatch(vm, stub));
    }
}

