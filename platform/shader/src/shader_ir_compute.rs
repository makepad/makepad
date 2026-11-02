//! Compute entry points and their host ABI. Every target shares the original
//! address-slot/parameter byte layout. Only SPIR-V consumes the addresses;
//! text targets bind buffers separately and leave those slots as padding.
//! Storage images have explicit format and binding reflection.

use crate::shader_ir::*;
use makepad_live_id::LiveId;
use makepad_relax_api::relax_api;

#[relax_api(plain)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComputeTarget {
    #[default]
    Spirv,
    Msl,
    Hlsl,
}

#[relax_api(plain)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StorageTextureFormat { R16f, R8unorm, R32f, R32uint, Rgba8unorm, Rgba16f }

impl StorageTextureFormat {
    pub fn channels(self) -> u32 {
        match self { Self::Rgba8unorm | Self::Rgba16f => 4, _ => 1 }
    }
    pub fn scalar(self) -> IrScalar {
        if self == Self::R32uint { IrScalar::U32 } else { IrScalar::F32 }
    }
    pub fn spirv_format(self) -> u32 {
        match self { Self::Rgba16f => 2, Self::R32f => 3, Self::Rgba8unorm => 4,
            Self::R16f => 9, Self::R8unorm => 15, Self::R32uint => 33 }
    }
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StorageTextureBinding {
    pub name: LiveId,
    pub global: GlobalId,
    pub format: StorageTextureFormat,
    /// SPIR-V descriptor set (zero); unused on text targets.
    pub set: u32,
    /// SPIR-V binding, Metal texture index, or HLSL u register.
    pub index: u32,
}

#[relax_api(plain)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferBindingClass {
    DeviceAddress,
    MetalBuffer,
    ShaderResource,
    UnorderedAccess,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ComputeBufferBinding {
    pub name: LiveId,
    pub global: GlobalId,
    pub class: BufferBindingClass,
    /// Metal buffer / HLSL t or u register. For DeviceAddress this is only
    /// the buffer ordinal; the host writes its address at `offset`.
    pub index: u32,
    /// Eight-byte address slot in the common parameter block (padding on
    /// text targets); for draw shaders, the Vulkan push-constant offset.
    pub offset: u32,
    /// Bytes between adjacent elements, including vec3's std430 padding.
    pub stride: u32,
    /// Required base alignment and allocation-size multiple. HLSL raw
    /// buffers require at least four bytes, including packed half buffers.
    pub alignment: u32,
    pub scalar: IrScalar,
    pub lanes: u32,
    pub writable: bool,
}

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
    pub target: ComputeTarget,
    pub buffers: Vec<ComputeBufferBinding>,
    pub storage_textures: Vec<StorageTextureBinding>,
    /// MSL buffer index or HLSL b register; None for push constants.
    pub param_binding: Option<u32>,
    /// Allocation/binding byte size; HLSL rounds the common block to 16 bytes.
    /// Hosts copy push_size bytes and zero any trailing binding padding.
    pub param_binding_size: u32,
    /// HLSL uint3 dispatch group counts, written by the host at this byte offset.
    pub group_count_offset: Option<u32>,
    /// Disable fast math when compiling the target library.
    pub precise_math: bool,
    pub slots: Vec<ComputeSlot>,
    /// Bytes of the push-constant block.
    pub push_size: u32,
    pub workgroup: [u32; 3],
    /// Upper bound for static workgroup allocation, including alignment.
    /// HLSL widens half workgroup elements to 32 bits in shader model 5.
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
    Ok(ComputeLayout { slots, push_size, workgroup: m.workgroup, shared_bytes, cooperative_matrices, precise_math: m.precise, ..Default::default() })
}

/// Target bindings supplement, never reorder, the common byte layout.
pub fn compute_target_layout(m: &IrModule, target: ComputeTarget) -> Result<ComputeLayout, String> {
    for g in &m.globals {
        if !matches!(g.kind, IrGlobalKind::Io(IrIo::Storage(_) | IrIo::Uniform | IrIo::Workgroup) | IrGlobalKind::StorageTexture(_) | IrGlobalKind::IterCounter) {
            return Err(format!("kernel resource {} ({:?}) is unsupported on {target:?}", g.name, g.kind));
        }
        if target != ComputeTarget::Spirv && g.kind == IrGlobalKind::Io(IrIo::Workgroup) {
            let IrTy::Array(elem, _) = m.get(g.ty) else { unreachable!() };
            if !matches!(m.get(*elem), IrTy::Scalar(IrScalar::F32 | IrScalar::F16 | IrScalar::U32 | IrScalar::I32) | IrTy::Vec(2..=4, IrScalar::F32 | IrScalar::F16 | IrScalar::U32 | IrScalar::I32)) {
                return Err(format!("workgroup element {:?} is unsupported on {target:?}", m.get(*elem)));
            }
        }
    }
    let mut layout = compute_layout(m)?;
    layout.target = target;
    if target != ComputeTarget::Spirv && !layout.cooperative_matrices.is_empty() {
        return Err(format!("cooperative matrices are unsupported on {target:?}"));
    }
    let mut read = 0;
    let mut write = 0;
    for slot in &layout.slots {
        let ComputeSlotKind::Buffer { writable } = slot.kind else { continue };
        let (class, index) = match target {
            ComputeTarget::Spirv => (BufferBindingClass::DeviceAddress, layout.buffers.len() as u32),
            ComputeTarget::Msl => (BufferBindingClass::MetalBuffer, layout.buffers.len() as u32),
            ComputeTarget::Hlsl if writable => { let n = write; write += 1; (BufferBindingClass::UnorderedAccess, n) }
            ComputeTarget::Hlsl => { let n = read; read += 1; (BufferBindingClass::ShaderResource, n) }
        };
        layout.buffers.push(buffer_binding(m, slot.global, target, class, index, slot.offset, writable)?);
    }
    layout.param_binding = match target {
        ComputeTarget::Spirv => None,
        ComputeTarget::Msl => Some(layout.buffers.len() as u32),
        ComputeTarget::Hlsl => Some(0),
    };
    for (g, global) in m.globals.iter().enumerate() {
        if let IrGlobalKind::StorageTexture(format) = global.kind {
            let index = layout.storage_textures.len() as u32 + if target == ComputeTarget::Hlsl { write } else { 0 };
            layout.storage_textures.push(StorageTextureBinding { name: global.name, global: g as GlobalId, format, set: 0, index });
        }
    }
    write += layout.storage_textures.len() as u32;
    if target == ComputeTarget::Hlsl {
        layout.shared_bytes = 0;
        for g in &m.globals {
            if g.kind == IrGlobalKind::Io(IrIo::Workgroup) {
                let (mut size, mut align) = ir_layout(m, g.ty);
                if let IrTy::Array(elem, _) = m.get(g.ty) {
                    if m.scalar_of(*elem) == Some(IrScalar::F16) { size *= 2; align *= 2; }
                }
                layout.shared_bytes = round_up(layout.shared_bytes, align).checked_add(size).ok_or("HLSL workgroup allocation overflows u32")?;
            }
        }
        for f in &m.functions {
            for e in &f.exprs {
                if let ExprKind::Builtin(op, _) = e.kind {
                    match op {
                        Builtin::SubgroupSize | Builtin::SubgroupLane | Builtin::SubgroupId |
                        Builtin::SubgroupAdd | Builtin::SubgroupMin | Builtin::SubgroupMax |
                        Builtin::SubgroupShuffle | Builtin::SubgroupShuffleXor | Builtin::SubgroupFirst =>
                            return Err("subgroup ops need shader model 6; D3D11 runs cs_5_0".into()),
                        Builtin::GroupCount => layout.group_count_offset = Some(round_up(layout.push_size, 16)),
                        _ => {}
                    }
                }
            }
        }
        if let Some(offset) = layout.group_count_offset {
            layout.push_size = round_up(offset + 12, 8);
            if layout.push_size > 128 { return Err("HLSL kernel params including group_count exceed 128 bytes".into()); }
        }
        if write > 8 { return Err(format!("D3D11.0 supports 8 compute UAVs; kernel needs {write}")); }
    }
    layout.param_binding_size = if target == ComputeTarget::Hlsl { round_up(layout.push_size, 16) } else { layout.push_size };
    if layout.push_size == 0 { layout.param_binding = None; }
    Ok(layout)
}

fn buffer_binding(m: &IrModule, global: GlobalId, target: ComputeTarget, class: BufferBindingClass, index: u32, offset: u32, writable: bool) -> Result<ComputeBufferBinding, String> {
    let g = &m.globals[global as usize];
    let IrTy::Array(elem, _) = m.get(g.ty) else { return Err(format!("buffer type {:?} is unsupported on {target:?}", m.get(g.ty))) };
    let (scalar, lanes) = match m.get(*elem) {
        IrTy::Scalar(s) => (*s, 1),
        IrTy::Vec(n, s) => (*s, *n as u32),
        ty => return Err(format!("buffer element {ty:?} is unsupported on {target:?}")),
    };
    if target != ComputeTarget::Spirv && (scalar == IrScalar::Bool || !(1..=4).contains(&lanes)) {
        return Err(format!("buffer element {:?} is unsupported on {target:?}", m.get(*elem)));
    }
    let (size, align) = ir_layout(m, *elem);
    Ok(ComputeBufferBinding { name: g.name, global, class, index, offset,
        stride: round_up(size, align), alignment: if target == ComputeTarget::Hlsl { align.max(4) } else { align },
        scalar, lanes, writable })
}

/// Draw buffers use the same physical-array ABI, with no compute uniforms.
/// The frontend assigns declaration ordinals to their binding fields. Only
/// Vulkan uses a parameter block here; text targets bind resources directly.
pub fn draw_storage_layout(m: &IrModule, target: ComputeTarget) -> Result<ComputeLayout, String> {
    let mut globals: Vec<_> = m.globals.iter().enumerate().filter_map(|(i, g)|
        matches!(g.kind, IrGlobalKind::Io(IrIo::Storage(_))).then_some(i as GlobalId)).collect();
    globals.sort_by_key(|g| m.globals[*g as usize].binding.unwrap_or(*g));
    let mut layout = ComputeLayout { target, ..Default::default() };
    let class = match target {
        ComputeTarget::Spirv => BufferBindingClass::DeviceAddress,
        ComputeTarget::Msl => BufferBindingClass::MetalBuffer,
        ComputeTarget::Hlsl => BufferBindingClass::ShaderResource,
    };
    for (i, global) in globals.into_iter().enumerate() {
        let g = &m.globals[global as usize];
        if g.kind != IrGlobalKind::Io(IrIo::Storage(false)) { return Err("draw storage buffers must be read-only".into()); }
        let offset = i as u32 * 8;
        layout.buffers.push(buffer_binding(m, global, target, class, i as u32, offset, false)?);
        layout.slots.push(ComputeSlot { name: g.name, global, kind: ComputeSlotKind::Buffer { writable: false }, offset, size: 8 });
    }
    if target == ComputeTarget::Spirv {
        layout.push_size = layout.buffers.len() as u32 * 8;
        if layout.push_size > 128 { return Err(format!("draw buffer push constants take {} bytes, over the 128 every Vulkan device has", layout.push_size)); }
        layout.param_binding_size = layout.push_size;
    }
    Ok(layout)
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
