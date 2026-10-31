//! Compute kernels: the entry point and the push-constant layout a kernel's
//! host binds it by.
//!
//! A kernel reaches every buffer by its device address, so it has no
//! descriptor sets: one push-constant block holds a 64-bit address per
//! buffer the kernel uses, then its scalar/vector params. The host fills
//! the block by name from `ComputeLayout` and dispatches.

use crate::shader_ir::*;
use makepad_live_id::LiveId;
use makepad_relax_api::relax_api;

#[relax_api(plain)]
#[repr(C, u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComputeSlotKind {
    /// A device buffer address (u64). Writable for `buffer_rw`.
    Buffer { writable: bool },
    /// A param of `lanes` lanes of `scalar`.
    Param { scalar: IrScalar, lanes: u32 },
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ComputeSlot {
    pub name: LiveId,
    pub global: GlobalId,
    pub kind: ComputeSlotKind,
    /// Byte offset in the push-constant block.
    pub offset: u32,
    pub size: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ComputeLayout {
    pub slots: Vec<ComputeSlot>,
    /// Bytes of the push-constant block.
    pub push_size: u32,
    pub workgroup: [u32; 3],
    /// Static workgroup allocations, including alignment between arrays.
    pub shared_bytes: u32,
    /// Opaque subgroup matrix types declared by this kernel. The host can
    /// compare these with its device's cooperative matrix shape reports.
    pub cooperative_matrices: Vec<CooperativeMatrix>,
}

impl ComputeLayout {
    pub fn slot(&self, name: LiveId) -> Option<&ComputeSlot> {
        self.slots.iter().find(|s| s.name == name)
    }
}

/// The push-constant block of a lowered kernel: its buffers (in the order
/// the kernel first uses them) then its params, at their natural alignment.
pub fn compute_layout(m: &IrModule) -> Result<ComputeLayout, String> {
    let mut slots = Vec::new();
    let mut offset = 0u32;
    for (i, g) in m.globals.iter().enumerate() {
        if let IrGlobalKind::Io(IrIo::Storage(writable)) = g.kind {
            slots.push(ComputeSlot { name: g.name, global: i as GlobalId, kind: ComputeSlotKind::Buffer { writable }, offset, size: 8 });
            offset += 8;
        }
    }
    for (i, g) in m.globals.iter().enumerate() {
        if g.kind != IrGlobalKind::Io(IrIo::Uniform) {
            continue;
        }
        let (scalar, lanes) = match m.get(g.ty) {
            IrTy::Scalar(s) => (*s, 1),
            IrTy::Vec(n, s) => (*s, *n as u32),
            other => return Err(format!("kernel param {} must be a scalar or a vector, not {:?}", g.name, other)),
        };
        if scalar == IrScalar::Bool {
            return Err(format!("kernel param {} cannot be a bool (use u32)", g.name));
        }
        let (size, align) = ir_layout(m, g.ty);
        offset = round_up(offset, align);
        slots.push(ComputeSlot { name: g.name, global: i as GlobalId, kind: ComputeSlotKind::Param { scalar, lanes }, offset, size });
        offset += size;
    }
    let push_size = round_up(offset, 8);
    if push_size > 128 {
        // 128 bytes is the Vulkan minimum every device guarantees.
        return Err(format!("kernel push constants take {} bytes, over the 128 every device has", push_size));
    }
    let mut shared_bytes = 0u32;
    for g in &m.globals {
        if g.kind == IrGlobalKind::Io(IrIo::Workgroup) {
            let (size, align) = ir_layout(m, g.ty);
            shared_bytes = round_up(shared_bytes, align).checked_add(size).ok_or("workgroup allocation overflows u32")?;
        }
    }
    let cooperative_matrices = m.types.iter().filter_map(|t| {
        if let IrTy::CooperativeMatrix(matrix) = t { Some(*matrix) } else { None }
    }).collect();
    Ok(ComputeLayout { slots, push_size, workgroup: m.workgroup, shared_bytes, cooperative_matrices })
}

/// Makes `main` (a lowered kernel body, no params, void) the module's
/// compute entry point `kernel_main`; returns the entry's id.
pub fn build_compute_entry(m: &mut IrModule, main: FuncId) -> FuncId {
    let mut f = IrFunction::new("kernel_main".to_string(), TY_VOID);
    let call = f.expr(ExprKind::Call(main, Vec::new()), TY_VOID);
    f.body.push(Stmt::Expr(call));
    f.entry = Some(IrEntry { stage: IrStage::Compute, inputs: Vec::new(), outputs: Vec::new() });
    m.functions.push(f);
    (m.functions.len() - 1) as FuncId
}
