use {
    crate::{
        cx::Cx,
        draw_vars::{DrawVars, DRAW_CALL_UNIFORM_BUFFER_SLOTS},
        geometry::GeometryId,
        makepad_live_id::*,
        makepad_script::heap::ScriptHeap,
        makepad_script::pod::{ScriptPodTy, ScriptPodVec},
        makepad_script::shader::*,
        makepad_script::value::{ScriptIp, ScriptObject, ScriptPodType},
        makepad_script::NoTrap,
        makepad_script::ScriptObjectRef,
        os::CxOsDrawShader,
    },
    std::{
        collections::BTreeSet,
        collections::HashMap,
        ops::{Index, IndexMut},
    },
};

// Re-export UniformBufferBindings for use in other modules
pub use makepad_script::shader::UniformBufferBindings;

#[derive(Debug, Clone, PartialEq)]
pub struct CxDrawShaderOptions {
    pub draw_call_group: LiveId,
    pub debug_id: Option<LiveId>,
    pub depth_write: bool,
    pub alpha_blend: bool,
    pub backface_culling: bool,
}

impl Default for CxDrawShaderOptions {
    fn default() -> Self {
        Self {
            draw_call_group: LiveId(0),
            debug_id: None,
            depth_write: true,
            alpha_blend: true,
            backface_culling: false,
        }
    }
}

impl CxDrawShaderOptions {
    /*
    pub fn from_ptr(cx: &Cx, draw_shader_ptr: DrawShaderPtr) -> Self {
        let live_registry_cp = cx.live_registry.clone();
        let live_registry = live_registry_cp.borrow();
        let doc = live_registry.ptr_to_doc(draw_shader_ptr.0);
        let mut ret = Self::default();
        // copy in per-instance settings from the DSL
        let mut node_iter = doc.nodes.first_child(draw_shader_ptr.node_index());
        while let Some(node_index) = node_iter {
            let node = &doc.nodes[node_index];
            match node.id {
                live_id!(draw_call_group) => if let LiveValue::Id(id) = node.value {
                    ret.draw_call_group = id;
                }
                live_id!(debug_id) => if let LiveValue::Id(id) = node.value {
                    ret.debug_id = Some(id);
                }
                _ => ()
            }
            node_iter = doc.nodes.next_child(node_index);
        }
        ret
    }*/

    pub fn _appendable_drawcall(&self, other: &Self) -> bool {
        self == other
    }
}

/*
#[derive(Default)]
pub struct CxDrawShaderItem {
    pub draw_shader_id: usize,
    pub options: CxDrawShaderOptions
}*/

#[derive(Default)]
pub struct CxDrawShaders {
    pub shaders: Vec<CxDrawShader>,
    pub os_shaders: Vec<CxOsDrawShader>,
    pub compile_set: BTreeSet<usize>,
    /// Explicit const-table mode override; None = decide from the
    /// environment / remote bridge (see `Cx::shader_const_table_mode`).
    pub const_table_mode: Option<bool>,

    pub cache_object_reuse_epoch_seen: u64,
    /// Keyed by (heap key, object): an isolate's objects share index space
    /// with the app heap's and must never hit its entries.
    pub cache_object_id_to_shader: HashMap<(usize, ScriptObject), DrawShaderId>,
    pub cache_functions_to_shader: LiveIdMap<LiveId, DrawShaderId>,
    pub cache_code_to_shader: HashMap<CxDrawShaderCode, DrawShaderId>,
    //pub ptr_to_item: HashMap<DrawShaderPtr, CxDrawShaderItem>,
    //pub fingerprints: Vec<DrawShaderFingerprint>,
    //pub error_set: HashSet<DrawShaderPtr>,
    // pub error_fingerprints: Vec<Vec<LiveNode >>,
}

impl CxDrawShaders {
    pub fn reset_for_live_reload(&mut self) {
        self.cache_object_id_to_shader.clear();
        self.cache_functions_to_shader.clear();
    }
}

impl Cx {
    /// Whether draw shaders compile with the constant table: annotated float
    /// literals in fn bodies become hot-patchable scope-uniform slots
    /// instead of folded code. Explicit `set_shader_const_table_mode` wins;
    /// else `MAKEPAD_SHADER_CONST_TABLE=1|0`; else on exactly when the
    /// remote bridge (the tweaker's channel) is active. Off, codegen is
    /// byte-identical to a build without the feature.
    pub fn shader_const_table_mode(&self) -> bool {
        if let Some(on) = self.draw_shaders.const_table_mode {
            return on;
        }
        match std::env::var("MAKEPAD_SHADER_CONST_TABLE").as_deref() {
            Ok("1") | Ok("true") | Ok("on") => true,
            Ok("0") | Ok("false") | Ok("off") => false,
            _ => crate::remote::is_active(),
        }
    }

    /// Force the const-table mode. Takes effect for shaders compiled from
    /// now on: the shader caches are dropped so a re-applied draw object
    /// recompiles, but a DrawVars holding a compiled shader keeps it until
    /// it is re-applied (live edit / script reapply).
    pub fn set_shader_const_table_mode(&mut self, on: bool) {
        if self.draw_shaders.const_table_mode == Some(on) {
            return;
        }
        self.draw_shaders.const_table_mode = Some(on);
        self.draw_shaders.reset_for_live_reload();
        self.draw_shaders.cache_code_to_shader.clear();
    }

    /// The hot-patchable constants of a compiled shader (empty when it was
    /// compiled with the table off or has no annotated literals).
    pub fn shader_const_table(&self, shader_id: DrawShaderId) -> &[DrawShaderTableConst] {
        match self.draw_shaders.shaders.get(shader_id.index) {
            Some(sh) => &sh.mapping.table_consts,
            None => &[],
        }
    }

    /// Hot-patch one table constant: the new value reaches the GPU on the
    /// next frame through the scope-uniform buffer — no recompile, and the
    /// shader source is untouched. Every draw sharing this compiled shader
    /// changes together. Returns false for an unknown shader or index.
    pub fn shader_const_patch(&mut self, shader_id: DrawShaderId, index: usize, value: f32) -> bool {
        let uniforms_gen = self.next_uniform_gen();
        let Some(sh) = self.draw_shaders.shaders.get_mut(shader_id.index) else {
            return false;
        };
        if !sh.mapping.patch_table_const(index, value, uniforms_gen) {
            return false;
        }
        self.redraw_all();
        true
    }

    /// Put one table constant back to the literal in the source.
    pub fn shader_const_reset(&mut self, shader_id: DrawShaderId, index: usize) -> bool {
        let initial = match self
            .draw_shaders
            .shaders
            .get(shader_id.index)
            .and_then(|sh| sh.mapping.table_consts.get(index))
        {
            Some(tc) => tc.initial,
            None => return false,
        };
        self.shader_const_patch(shader_id, index, initial)
    }
}

impl Cx {
    pub fn flush_draw_shaders(&mut self) {
        /*
        self.shader_registry.flush_registry();
        self.draw_shaders.shaders.clear();
        self.draw_shaders.ptr_to_item.clear();
        self.draw_shaders.fingerprints.clear();
        self.draw_shaders.error_set.clear();
        self.draw_shaders.error_fingerprints.clear();*/
    }
}

impl Index<usize> for CxDrawShaders {
    type Output = CxDrawShader;
    fn index(&self, index: usize) -> &Self::Output {
        &self.shaders[index]
    }
}

impl IndexMut<usize> for CxDrawShaders {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.shaders[index]
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DrawShaderId {
    pub index: usize,
    //pub draw_shader_ptr: DrawShaderPtr
}

impl DrawShaderId {
    pub fn false_compare_check(&self) -> u64 {
        (self.index as u64) << 32 //| self.draw_shader_ptr.0.index as u64
    }
}

pub struct CxDrawShader {
    pub debug_id: LiveId,
    pub os_shader_id: Option<usize>,
    pub mapping: CxDrawShaderMapping,
}

#[derive(Clone, Debug)]
pub struct DrawShaderInputs {
    pub inputs: Vec<DrawShaderInput>,
    pub packing_method: DrawShaderInputPacking,
    /// f32-lane count. For the all-F32xN case this is also `stride_bytes / 4`.
    pub total_slots: usize,
    /// Packed byte stride. Equals `total_slots * 4` whenever every input is F32xN.
    pub stride_bytes: usize,
    max_byte_align: usize,
}

#[derive(Clone, Copy, Debug)]
pub enum DrawShaderInputPacking {
    Attribute,
    UniformsGLSLTight,
    UniformsGLSL140,
    #[allow(dead_code)]
    UniformsHLSL,
    #[allow(dead_code)]
    UniformsMetal,
}

/// Physical vertex/instance fetch format. The existing f32 path is the `F32xN`
/// case of this enum; compact formats convert to `vec2f`/`vec4f` at fetch.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawShaderAttrFormat {
    F32x1,
    F32x2,
    F32x3,
    F32x4,
    F16x2,
    F16x4,
    U16x2,
    I16x2,
    U16x2Norm,
    I16x2Norm,
    U8x4Norm,
    I8x4Norm,
    U32x1,
    I32x1,
}

#[allow(non_upper_case_globals)]
impl DrawShaderAttrFormat {
    /// Old three-variant names kept as aliases so remaining matches compile.
    pub const Float: Self = Self::F32x1;
    pub const UInt: Self = Self::U32x1;
    pub const SInt: Self = Self::I32x1;
}

impl Default for DrawShaderAttrFormat {
    fn default() -> Self {
        Self::F32x1
    }
}

impl DrawShaderAttrFormat {
    pub fn from_slots_f32(slots: usize) -> Self {
        match slots {
            1 => Self::F32x1,
            2 => Self::F32x2,
            3 => Self::F32x3,
            _ => Self::F32x4,
        }
    }

    pub fn is_f32_lane(self) -> bool {
        matches!(self, Self::F32x1 | Self::F32x2 | Self::F32x3 | Self::F32x4)
    }

    pub fn is_compact(self) -> bool {
        !self.is_f32_lane() && !matches!(self, Self::U32x1 | Self::I32x1)
    }

    /// Integer vertexAttribIPointer / DXGI *_UINT/SINT (not normalized).
    pub fn is_integer_fetch(self) -> bool {
        matches!(self, Self::U32x1 | Self::I32x1)
    }

    pub fn byte_align(self) -> usize {
        match self {
            Self::U8x4Norm | Self::I8x4Norm => 1,
            Self::F16x2
            | Self::F16x4
            | Self::U16x2
            | Self::I16x2
            | Self::U16x2Norm
            | Self::I16x2Norm => 2,
            _ => 4,
        }
    }

    pub fn byte_size(self) -> usize {
        match self {
            Self::F32x1 | Self::U32x1 | Self::I32x1 => 4,
            Self::F32x2 => 8,
            Self::F32x3 => 12,
            Self::F32x4 => 16,
            Self::F16x2
            | Self::U16x2
            | Self::I16x2
            | Self::U16x2Norm
            | Self::I16x2Norm
            | Self::U8x4Norm
            | Self::I8x4Norm => 4,
            Self::F16x4 => 8,
        }
    }

    pub fn component_count(self) -> usize {
        match self {
            Self::F32x1 | Self::U32x1 | Self::I32x1 => 1,
            Self::F32x2
            | Self::F16x2
            | Self::U16x2
            | Self::I16x2
            | Self::U16x2Norm
            | Self::I16x2Norm => 2,
            Self::F32x3 => 3,
            Self::F32x4 | Self::F16x4 | Self::U8x4Norm | Self::I8x4Norm => 4,
        }
    }

    pub fn logical_slots(self) -> usize {
        self.component_count()
    }

    pub fn is_normalized(self) -> bool {
        matches!(
            self,
            Self::U16x2Norm | Self::I16x2Norm | Self::U8x4Norm | Self::I8x4Norm
        )
    }

    /// WebGL/OpenGL type token: FLOAT=0, HALF_FLOAT=1, UNSIGNED_SHORT=2,
    /// SHORT=3, UNSIGNED_BYTE=4, BYTE=5, UNSIGNED_INT=6, INT=7.
    pub fn decode_to_f32(self, bytes: &[u8]) -> [f32; 4] {
        fn f16(b: &[u8], i: usize) -> f32 {
            if i + 1 >= b.len() {
                return 0.0;
            }
            crate::f16_bits_to_f32(u16::from_le_bytes([b[i], b[i + 1]]))
        }
        fn u16v(b: &[u8], i: usize) -> u16 {
            if i + 1 >= b.len() {
                return 0;
            }
            u16::from_le_bytes([b[i], b[i + 1]])
        }
        fn i16v(b: &[u8], i: usize) -> i16 {
            if i + 1 >= b.len() {
                return 0;
            }
            i16::from_le_bytes([b[i], b[i + 1]])
        }
        match self {
            Self::F32x1 | Self::F32x2 | Self::F32x3 | Self::F32x4 => {
                let n = self.component_count();
                let mut out = [0.0f32; 4];
                for i in 0..n {
                    let o = i * 4;
                    if o + 3 < bytes.len() {
                        out[i] = f32::from_le_bytes([
                            bytes[o],
                            bytes[o + 1],
                            bytes[o + 2],
                            bytes[o + 3],
                        ]);
                    }
                }
                out
            }
            Self::F16x2 => [f16(bytes, 0), f16(bytes, 2), 0.0, 0.0],
            Self::F16x4 => [f16(bytes, 0), f16(bytes, 2), f16(bytes, 4), f16(bytes, 6)],
            Self::U16x2 => [u16v(bytes, 0) as f32, u16v(bytes, 2) as f32, 0.0, 0.0],
            Self::I16x2 => [i16v(bytes, 0) as f32, i16v(bytes, 2) as f32, 0.0, 0.0],
            Self::U16x2Norm => [
                u16v(bytes, 0) as f32 / 65535.0,
                u16v(bytes, 2) as f32 / 65535.0,
                0.0,
                0.0,
            ],
            Self::I16x2Norm => [
                (i16v(bytes, 0) as f32 / 32767.0).max(-1.0),
                (i16v(bytes, 2) as f32 / 32767.0).max(-1.0),
                0.0,
                0.0,
            ],
            Self::U8x4Norm => {
                let b = |i| bytes.get(i).copied().unwrap_or(0) as f32 / 255.0;
                [b(0), b(1), b(2), b(3)]
            }
            Self::I8x4Norm => {
                let b = |i| {
                    (bytes.get(i).copied().unwrap_or(0) as i8 as f32 / 127.0).max(-1.0)
                };
                [b(0), b(1), b(2), b(3)]
            }
            Self::U32x1 => {
                if bytes.len() >= 4 {
                    [f32::from_bits(u32::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                    ])), 0.0, 0.0, 0.0]
                } else {
                    [0.0; 4]
                }
            }
            Self::I32x1 => {
                if bytes.len() >= 4 {
                    [f32::from_bits(i32::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                    ]) as u32), 0.0, 0.0, 0.0]
                } else {
                    [0.0; 4]
                }
            }
        }
    }

    pub fn gl_type_code(self) -> u32 {
        match self {
            Self::F32x1 | Self::F32x2 | Self::F32x3 | Self::F32x4 => 0,
            Self::F16x2 | Self::F16x4 => 1,
            Self::U16x2 | Self::U16x2Norm => 2,
            Self::I16x2 | Self::I16x2Norm => 3,
            Self::U8x4Norm => 4,
            Self::I8x4Norm => 5,
            Self::U32x1 => 6,
            Self::I32x1 => 7,
        }
    }

    /// Kind used by the f32 instance/uniform write path (bit-cast ints).
    pub fn f32_write_kind(self) -> DrawShaderF32WriteKind {
        match self {
            Self::U32x1 => DrawShaderF32WriteKind::UInt,
            Self::I32x1 => DrawShaderF32WriteKind::SInt,
            _ => DrawShaderF32WriteKind::Float,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawShaderF32WriteKind {
    Float,
    UInt,
    SInt,
}

#[derive(Clone, Debug)]
pub struct DrawShaderInput {
    pub id: LiveId,
    /// f32-lane offset. For all-F32 inputs this is `byte_offset / 4`.
    pub offset: usize,
    pub slots: usize,
    pub attr_format: DrawShaderAttrFormat,
    pub byte_offset: usize,
    pub byte_size: usize,
}

fn uniform_packing() -> DrawShaderInputPacking {
    #[cfg(any(target_arch = "wasm32"))]
    {
        return DrawShaderInputPacking::UniformsGLSL140;
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        return DrawShaderInputPacking::UniformsGLSL140;
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    {
        return DrawShaderInputPacking::UniformsMetal;
    }

    #[cfg(target_os = "windows")]
    {
        return DrawShaderInputPacking::UniformsHLSL;
    }
}

impl DrawShaderInputs {
    pub fn new(packing_method: DrawShaderInputPacking) -> Self {
        Self {
            inputs: Vec::new(),
            packing_method,
            total_slots: 0,
            stride_bytes: 0,
            max_byte_align: 1,
        }
    }

    pub fn has_compact(&self) -> bool {
        self.inputs.iter().any(|i| i.attr_format.is_compact())
    }

    pub fn all_f32_lanes(&self) -> bool {
        !self.inputs.is_empty() && self.inputs.iter().all(|i| i.attr_format.is_f32_lane())
    }

    /// Stable signature of the physical fetch record. Field names are not
    /// included: only byte interpretation and ordering affect buffer safety.
    pub fn layout_signature(&self) -> u64 {
        fn mix(hash: &mut u64, value: usize) {
            for byte in value.to_le_bytes() {
                *hash ^= byte as u64;
                *hash = hash.wrapping_mul(0x100000001b3);
            }
        }

        let mut hash = 0xcbf29ce484222325;
        mix(&mut hash, self.stride_bytes);
        mix(&mut hash, self.total_slots);
        mix(&mut hash, self.inputs.len());
        for input in &self.inputs {
            mix(&mut hash, input.attr_format as usize);
            mix(&mut hash, input.byte_offset);
            mix(&mut hash, input.byte_size);
            mix(&mut hash, input.slots);
        }
        hash
    }

    /// Decode one packed vertex into f32 logical slots (headless fetch).
    pub fn decode_vertex_f32(&self, vertex_bytes: &[u8], dst: &mut [f32]) {
        for input in &self.inputs {
            let end = (input.byte_offset + input.byte_size).min(vertex_bytes.len());
            if input.byte_offset >= vertex_bytes.len() {
                continue;
            }
            let decoded = input
                .attr_format
                .decode_to_f32(&vertex_bytes[input.byte_offset..end]);
            let n = input.slots.min(4).min(dst.len().saturating_sub(input.offset));
            for i in 0..n {
                dst[input.offset + i] = decoded[i];
            }
        }
    }

    fn push_input(
        &mut self,
        id: LiveId,
        offset: usize,
        slots: usize,
        attr_format: DrawShaderAttrFormat,
        byte_offset: usize,
        byte_size: usize,
    ) {
        self.inputs.push(DrawShaderInput {
            id,
            offset,
            slots,
            attr_format,
            byte_offset,
            byte_size,
        });
    }

    pub fn push(&mut self, id: LiveId, slots: usize, attr_format: DrawShaderAttrFormat) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                let byte_size = if attr_format.is_f32_lane() || attr_format.is_integer_fetch() {
                    // F32xN / U32x1 / I32x1: size follows the f32-lane count so a
                    // 19-slot f32 blob stays 76 bytes (the packed-geometry path).
                    slots * 4
                } else {
                    attr_format.byte_size()
                };
                let align = attr_format.byte_align();
                self.max_byte_align = self.max_byte_align.max(align);
                if align > 1 && self.stride_bytes % align != 0 {
                    self.stride_bytes += align - (self.stride_bytes % align);
                }
                let byte_offset = self.stride_bytes;
                // Keep the old vec4-slot pad for integer *lane* vectors so
                // existing UInt/SInt f32-slot layouts stay byte-identical.
                let needs_int_align = attr_format.is_integer_fetch() && slots > 1;
                if needs_int_align && (self.total_slots & 3) != 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
                let offset = self.total_slots;
                self.push_input(id, offset, slots, attr_format, byte_offset, byte_size);
                self.stride_bytes += byte_size;
                self.total_slots += slots;
                if needs_int_align && (self.total_slots & 3) != 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
            }
            DrawShaderInputPacking::UniformsGLSLTight => {
                self.push_input(
                    id,
                    self.total_slots,
                    slots,
                    attr_format,
                    self.total_slots * 4,
                    slots * 4,
                );
                self.total_slots += slots;
                self.stride_bytes = self.total_slots * 4;
            }
            DrawShaderInputPacking::UniformsGLSL140 => {
                // std140 alignment rules:
                // scalar (1 slot): no alignment requirement
                // vec2 (2 slots): 2-slot aligned
                // vec3/vec4 (3-4 slots): 4-slot aligned
                // larger (matrices, arrays): 4-slot aligned
                let alignment = match slots {
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                if self.total_slots % alignment != 0 {
                    self.total_slots += alignment - (self.total_slots % alignment);
                }
                self.push_input(
                    id,
                    self.total_slots,
                    slots,
                    attr_format,
                    self.total_slots * 4,
                    slots * 4,
                );
                self.total_slots += slots;
                self.stride_bytes = self.total_slots * 4;
            }
            DrawShaderInputPacking::UniformsHLSL => {
                if slots > 4 {
                    if (self.total_slots & 3) != 0 {
                        self.total_slots += 4 - (self.total_slots & 3);
                    }
                } else if (self.total_slots & 3) + slots > 4 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
                self.push_input(
                    id,
                    self.total_slots,
                    slots,
                    attr_format,
                    self.total_slots * 4,
                    slots * 4,
                );
                self.total_slots += slots;
                self.stride_bytes = self.total_slots * 4;
            }
            DrawShaderInputPacking::UniformsMetal => {
                // Metal struct alignment rules:
                // float (1 slot): 4-byte aligned (1-float)
                // float2 (2 slots): 8-byte aligned (2-float)
                // float3/float4 (3-4 slots): 16-byte aligned (4-float)
                // larger (matrices, arrays): 16-byte aligned (4-float)
                let aligned_slots = if slots == 3 { 4 } else { slots };
                let alignment = match aligned_slots {
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                if self.total_slots % alignment != 0 {
                    self.total_slots += alignment - (self.total_slots % alignment);
                }
                self.push_input(
                    id,
                    self.total_slots,
                    slots,
                    attr_format,
                    self.total_slots * 4,
                    aligned_slots * 4,
                );
                self.total_slots += aligned_slots;
                self.stride_bytes = self.total_slots * 4;
            }
        }
    }

    pub fn finalize(&mut self) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                if self.inputs.iter().all(|i| !i.attr_format.is_compact()) {
                    self.stride_bytes = self.total_slots * 4;
                } else if self.stride_bytes % self.max_byte_align != 0 {
                    self.stride_bytes +=
                        self.max_byte_align - (self.stride_bytes % self.max_byte_align);
                }
            }
            DrawShaderInputPacking::UniformsGLSLTight => {
                self.stride_bytes = self.total_slots * 4;
            }
            DrawShaderInputPacking::UniformsHLSL
            | DrawShaderInputPacking::UniformsMetal
            | DrawShaderInputPacking::UniformsGLSL140 => {
                if self.total_slots & 3 > 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
                self.stride_bytes = self.total_slots * 4;
            }
        }
    }
}

#[derive(Clone)]
pub struct DrawShaderTextureInput {
    pub id: LiveId,
    pub tex_type: TextureType,
}

#[derive(Clone, Debug)]
pub struct DrawShaderUniformBufferInput {
    pub id: LiveId,
    pub block_name: String,
    pub ty: ScriptPodType,
    pub size: usize,
    pub align: usize,
    pub buffer_index: usize,
}

/// The pixel format of the color attachment a shader renders into. Backends
/// that bake the target format into their pipeline state (Metal, D3D,
/// Vulkan) read this when building the pipeline; GL-family backends ignore
/// it (the FBO's texture format decides). Declared in the shader DSL as
/// `color_format: @Rf32`; the default is the swapchain's BGRA8.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum DrawShaderColorFormat {
    #[default]
    Bgra8Unorm,
    /// BGRA8 with blending DISABLED: raw component writes for data passes
    /// whose alpha channel is payload, not opacity (an SDF byte, a coverage
    /// flag). Under the default premultiplied-over pipeline dst alpha can
    /// only ever grow (`a_out = a_src + a_dst·(1-a_src)`), so alpha-as-data
    /// is unwritable without this. Declared as `color_format: @Bgra8NoBlend`.
    Bgra8NoBlend,
    /// Single-channel 32-bit float (pairs with `TextureFormat::RenderRf32`).
    /// Blending is disabled on such pipelines — float blending is not
    /// universally supported and the consumers are data passes.
    Rf32,
    /// Four-channel 16-bit float (pairs with `TextureFormat::RenderRGBAf16`).
    /// Blending disabled — simulation/data passes write whole texels.
    /// Declared as `color_format: @Rgba16F`.
    Rgba16F,
    /// Four-channel 32-bit float (pairs with `TextureFormat::RenderRGBAf32`):
    /// the GPU-simulation state format (particle pos/vel, fluid fields).
    /// Blending disabled — simulation/data passes write whole texels.
    /// Declared as `color_format: @Rgba32F`.
    Rgba32F,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct DrawShaderFlags {
    pub debug_draw: bool,
    pub debug_layout: bool,
    pub debug_code: bool,
    pub draw_call_nocompare: bool,
    pub draw_call_always: bool,
    pub async_compile: bool,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub enum CxDrawShaderCode {
    Separate { vertex: String, fragment: String },
    Combined { code: String },
}

/// What fills one scope-uniform slot: a script scope value read from the
/// heap, or one of the shader's table constants.
#[derive(Clone, Copy, Debug)]
pub enum ScopeUniformSlot {
    Scope(ScriptObject, LiveId),
    Const(usize),
}

/// A hot-patchable shader constant: a float literal in a shader fn body
/// that carried a `/** name [min..max] [step s] */` annotation and was
/// compiled under const-table mode. Patching writes the scope-uniform slot
/// (`input`) and never touches the source; `ip` names the literal for the
/// change ledger.
#[derive(Clone, Debug)]
pub struct DrawShaderTableConst {
    /// Scope-uniform io name (`ct<n>`).
    pub shader_name: LiveId,
    /// Annotation text as written.
    pub doc: String,
    /// Parsed hint: friendly name and optional range/step.
    pub name: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// The literal in the source.
    pub initial: f32,
    /// The live value (what the GPU reads).
    pub value: f32,
    /// Index into `scope_uniforms.inputs`.
    pub input: usize,
    /// The literal's immediate ip (file:line:col via the script code).
    pub ip: ScriptIp,
}

#[derive(Clone)]
pub struct CxDrawShaderMapping {
    pub source: ScriptObjectRef,
    pub code: CxDrawShaderCode,
    pub flags: DrawShaderFlags,
    pub instances: DrawShaderInputs,
    pub dyn_instances: DrawShaderInputs,
    pub dyn_uniforms: DrawShaderInputs,
    pub geometries: DrawShaderInputs,
    pub textures: Vec<DrawShaderTextureInput>,
    pub uniform_buffers: Vec<DrawShaderUniformBufferInput>,
    pub samplers: Vec<ShaderSampler>,
    pub texture_sampler_indices: Vec<usize>,
    pub uses_time: bool,
    pub rect_pos: Option<usize>,
    pub rect_size: Option<usize>,
    pub draw_clip: Option<usize>,
    pub uniform_buffer_bindings: UniformBufferBindings,
    pub scope_uniforms: DrawShaderInputs,
    pub scope_uniform_sources: Vec<ScopeUniformSlot>,
    pub scope_uniforms_buf: Vec<f32>,
    /// The shader's hot-patchable constants (annotated literals compiled
    /// under const-table mode), each backed by one scope-uniform slot.
    pub table_consts: Vec<DrawShaderTableConst>,
    /// Bumped by every patch of `scope_uniforms_buf`; backends that keep a
    /// GPU-side copy of the buffer re-upload when it moves.
    pub scope_uniforms_gen: u64,
    pub geometry_id: Option<GeometryId>,
    /// Total f32 slots in the varying buffer (instances + explicit varyings).
    /// Set by the headless backend during shader compilation.
    pub varying_total_slots: usize,
    /// The color-attachment format this shader's pipeline targets.
    pub color_format: DrawShaderColorFormat,
}

impl CxDrawShaderMapping {
    fn attr_format_from_pod_type(ty: &ScriptPodTy) -> DrawShaderAttrFormat {
        match ty {
            ScriptPodTy::Packed(p) => match p {
                crate::makepad_script::pod::ScriptPodPacked::F16x2 => DrawShaderAttrFormat::F16x2,
                crate::makepad_script::pod::ScriptPodPacked::F16x4 => DrawShaderAttrFormat::F16x4,
                crate::makepad_script::pod::ScriptPodPacked::U16x2 => DrawShaderAttrFormat::U16x2,
                crate::makepad_script::pod::ScriptPodPacked::I16x2 => DrawShaderAttrFormat::I16x2,
                crate::makepad_script::pod::ScriptPodPacked::U16x2Norm => {
                    DrawShaderAttrFormat::U16x2Norm
                }
                crate::makepad_script::pod::ScriptPodPacked::I16x2Norm => {
                    DrawShaderAttrFormat::I16x2Norm
                }
                crate::makepad_script::pod::ScriptPodPacked::U8x4Norm => {
                    DrawShaderAttrFormat::U8x4Norm
                }
                crate::makepad_script::pod::ScriptPodPacked::I8x4Norm => {
                    DrawShaderAttrFormat::I8x4Norm
                }
            },
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => DrawShaderAttrFormat::U32x1,
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => DrawShaderAttrFormat::I32x1,
            ScriptPodTy::Bool => DrawShaderAttrFormat::U32x1,
            ScriptPodTy::F32 | ScriptPodTy::F16 => DrawShaderAttrFormat::F32x1,
            ScriptPodTy::Vec(vec_ty) => match vec_ty {
                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => {
                    DrawShaderAttrFormat::U32x1
                }
                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                    DrawShaderAttrFormat::I32x1
                }
                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => {
                    DrawShaderAttrFormat::U32x1
                }
                ScriptPodVec::Vec2f | ScriptPodVec::Vec2h => DrawShaderAttrFormat::F32x2,
                ScriptPodVec::Vec3f | ScriptPodVec::Vec3h => DrawShaderAttrFormat::F32x3,
                ScriptPodVec::Vec4f | ScriptPodVec::Vec4h => DrawShaderAttrFormat::F32x4,
            },
            _ => DrawShaderAttrFormat::from_slots_f32(ty.slots().max(1).min(4)),
        }
    }

    fn push_pod_fields(
        inputs: &mut DrawShaderInputs,
        ty: &ScriptPodTy,
        fallback_id: LiveId,
    ) {
        if let ScriptPodTy::Struct { fields, .. } = ty {
            if ty.has_compact_format() {
                for field in fields {
                    Self::push_pod_fields(inputs, &field.ty.data.ty, field.name);
                }
                return;
            }
        }
        let slots = ty.slots();
        let attr_format = Self::attr_format_from_pod_type(ty);
        inputs.push(fallback_id, slots, attr_format);
    }

    pub fn debug_dump_shader_draw_call(
        backend: &str,
        draw_item_id: usize,
        draw_shader: &CxDrawShader,
        draw_call: &crate::draw_list::CxDrawCall,
        instance_data: &[f32],
        instances: usize,
    ) {
        let instance_slots = draw_shader.mapping.instances.total_slots;
        if instance_slots == 0 {
            crate::log!(
                "debug_draw [{}] item={} shader={} debug_id={:?}: no instance layout",
                backend,
                draw_item_id,
                draw_call.draw_shader_id.index,
                draw_call.options.debug_id.unwrap_or(draw_shader.debug_id)
            );
            return;
        }

        let dyn_slots = draw_shader
            .mapping
            .dyn_uniforms
            .total_slots
            .min(draw_call.dyn_uniforms.len());
        crate::log!(
        "debug_draw [{}] item={} shader={} debug_id={:?} instances={} instance_slots={} dyn_uniform_slots={}",
        backend,
        draw_item_id,
        draw_call.draw_shader_id.index,
        draw_call.options.debug_id.unwrap_or(draw_shader.debug_id),
        instances,
        instance_slots,
        dyn_slots
    );

        for input in &draw_shader.mapping.dyn_uniforms.inputs {
            if input.offset >= dyn_slots {
                continue;
            }
            let end = (input.offset + input.slots).min(dyn_slots);
            crate::log!(
                "debug_draw [{}]   u {:?}: {:?}",
                backend,
                input.id,
                &draw_call.dyn_uniforms[input.offset..end]
            );
        }

        for inst_idx in 0..instances {
            let base = inst_idx * instance_slots;
            if base + instance_slots > instance_data.len() {
                break;
            }
            let mut parts = Vec::new();
            for input in &draw_shader.mapping.instances.inputs {
                let start = base + input.offset;
                let end = start + input.slots;
                if end > instance_data.len() {
                    continue;
                }
                let vals = &instance_data[start..end];
                if input.slots == 1 {
                    parts.push(format!("{:?}={}", input.id, vals[0]));
                } else {
                    parts.push(format!("{:?}={:?}", input.id, vals));
                }
            }
            crate::log!(
                "debug_draw [{}]   i[{}] {}",
                backend,
                inst_idx,
                parts.join(" ")
            );
        }
    }

    pub fn from_shader_output(
        source: ScriptObjectRef,
        code: CxDrawShaderCode,
        heap: &ScriptHeap,
        output: &ShaderOutput,
        geometry_id: Option<GeometryId>,
    ) -> CxDrawShaderMapping {
        let debug_draw = heap
            .value(source.as_object(), id!(debug_draw).into(), NoTrap)
            .as_bool()
            == Some(true);
        let debug_layout = heap
            .value(source.as_object(), id!(debug_layout).into(), NoTrap)
            .as_bool()
            == Some(true);
        let debug_code = heap
            .value(source.as_object(), id!(debug_code).into(), NoTrap)
            .as_bool()
            == Some(true);
        let async_compile = heap
            .value(source.as_object(), id!(async_compile).into(), NoTrap)
            .as_bool()
            == Some(true);
        // Color-attachment format: `color_format: @Rf32` selects the float
        // render-target pipeline; anything else keeps the BGRA8 default.
        let color_format = match heap
            .value(source.as_object(), id!(color_format).into(), NoTrap)
            .as_id()
        {
            Some(id) if id == id!(Rf32) => DrawShaderColorFormat::Rf32,
            Some(id) if id == id!(Rgba16F) => DrawShaderColorFormat::Rgba16F,
            Some(id) if id == id!(Rgba32F) => DrawShaderColorFormat::Rgba32F,
            Some(id) if id == id!(Bgra8NoBlend) => DrawShaderColorFormat::Bgra8NoBlend,
            _ => DrawShaderColorFormat::Bgra8Unorm,
        };
        // Use attribute packing for instances (they're vertex attributes)
        // instances contains ALL instance fields (dyn first, then rust)
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        // dyn_instances tracks just the dynamic portion for offset calculations
        let mut dyn_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        // Use platform-specific packing for uniforms
        let mut dyn_uniforms = DrawShaderInputs::new(uniform_packing());
        // Geometries for vertex buffer fields
        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut textures = Vec::new();
        let mut uniform_buffers = Vec::new();
        let mut texture_sampler_indices = Vec::new();

        let mut rect_pos = None;
        let mut rect_size = None;
        let mut draw_clip = None;

        // Memory layout: DynInstance fields first, then RustInstance fields
        // This matches metal_create_instance_struct

        // 1. Process DynInstance fields first (added to both instances and dyn_instances)
        for io in &output.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let attr_format = Self::attr_format_from_pod_type(&pod_ty.ty);
                Self::push_pod_fields(&mut instances, &pod_ty.ty, io.name);
                dyn_instances.push(io.name, slots, attr_format);
            }
        }

        // 2. Process RustInstance fields after (already in correct order from pre_collect_rust_instance_io)
        for io in output
            .io
            .iter()
            .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
        {
            let pod_ty = heap.pod_type_ref(io.ty);
            let attr_format = Self::attr_format_from_pod_type(&pod_ty.ty);

            // Track special field offsets
            if io.name == live_id!(rect_pos) {
                rect_pos = Some(instances.total_slots);
            }
            if io.name == live_id!(rect_size) {
                rect_size = Some(instances.total_slots);
            }
            if io.name == live_id!(draw_clip) {
                draw_clip = Some(instances.total_slots);
            }

            let _ = attr_format;
            Self::push_pod_fields(&mut instances, &pod_ty.ty, io.name);
        }

        // Process Uniform fields
        for io in &output.io {
            if let ShaderIoKind::Uniform = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                dyn_uniforms.push(io.name, slots, DrawShaderAttrFormat::Float);
            }
        }

        // Process VertexBuffer (geometry) fields. Compact POD structs are
        // flattened to per-field physical formats; all-F32 structs stay one
        // blob so packed_geometry_N codegen remains byte-identical.
        for io in &output.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                Self::push_pod_fields(&mut geometries, &pod_ty.ty, io.name);
            }
        }

        // Process texture fields.
        for io in &output.io {
            match &io.kind {
                ShaderIoKind::Texture(tex_type) => {
                    textures.push(DrawShaderTextureInput {
                        id: io.name,
                        tex_type: *tex_type,
                    });
                    let texture_name = format!("tex_{}", io.name);
                    let sampler_idx = output
                        .texture_sampler_bindings
                        .iter()
                        .find(|(bound_texture, _)| bound_texture == &texture_name)
                        .map(|(_, idx)| *idx)
                        .unwrap_or(0);
                    texture_sampler_indices.push(sampler_idx);
                }
                _ => (),
            }
        }

        for io in &output.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                if matches!(
                    pod_ty.name,
                    Some(id!(DrawPassUniforms))
                        | Some(id!(DrawListUniforms))
                        | Some(id!(DrawCallUniforms))
                ) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                uniform_buffers.push(DrawShaderUniformBufferInput {
                    id: io.name,
                    block_name: format!("{}_Uniforms", io_name),
                    ty: io.ty,
                    size: pod_ty.ty.size_of(),
                    align: pod_ty.ty.align_of(),
                    buffer_index: io
                        .buffer_index
                        .expect("UniformBuffer must have buffer_index assigned"),
                });
            }
        }

        if uniform_buffers.len() > DRAW_CALL_UNIFORM_BUFFER_SLOTS {
            panic!(
                "shader {:?} declares {} custom uniform buffers but only {} draw-call slots are available",
                source.as_object(),
                uniform_buffers.len(),
                DRAW_CALL_UNIFORM_BUFFER_SLOTS
            );
        }

        instances.finalize();
        dyn_instances.finalize();
        dyn_uniforms.finalize();
        geometries.finalize();

        // Get uniform buffer bindings from the shader output
        // (must call assign_uniform_buffer_indices before from_shader_output)
        let uniform_buffer_bindings = output.get_uniform_buffer_bindings(heap);

        // Build scope uniforms layout using DrawShaderInputs (4-byte slot alignment)
        let mut scope_uniforms = DrawShaderInputs::new(uniform_packing());
        let mut scope_uniform_sources = Vec::new();

        // Process scope uniforms in order - same order as they appear in the io list
        let mut table_consts: Vec<DrawShaderTableConst> = Vec::new();
        for io in &output.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                // Find the corresponding ScopeUniformSource
                if let Some(source) = output
                    .scope_uniforms
                    .iter()
                    .find(|su| su.shader_name == io.name)
                {
                    let pod_ty = heap.pod_type_ref(source.ty);
                    let slots = pod_ty.ty.slots();
                    let input = scope_uniforms.inputs.len();
                    scope_uniforms.push(io.name, slots, DrawShaderAttrFormat::Float);
                    match source.table_const {
                        Some(ci) => {
                            let tc = &output.table_consts[ci];
                            let hint = crate::makepad_script::docs::parse_doc_hint(&tc.doc);
                            scope_uniform_sources.push(ScopeUniformSlot::Const(table_consts.len()));
                            table_consts.push(DrawShaderTableConst {
                                shader_name: tc.shader_name,
                                doc: tc.doc.clone(),
                                name: hint.name,
                                min: hint.min,
                                max: hint.max,
                                step: hint.step,
                                initial: tc.value as f32,
                                value: tc.value as f32,
                                input,
                                ip: tc.ip,
                            });
                        }
                        None => scope_uniform_sources
                            .push(ScopeUniformSlot::Scope(source.source_obj, source.key)),
                    }
                }
            }
        }
        scope_uniforms.finalize();

        // Allocate the buffer for scope uniforms (as f32 slots)
        let scope_uniforms_buf = vec![0.0f32; scope_uniforms.total_slots];

        if debug_layout {
            crate::log!(
                "debug_layout shader {:?}: flags draw={} layout={} code={} uniform_packing={:?}",
                source.as_object(),
                debug_draw,
                debug_layout,
                debug_code,
                dyn_uniforms.packing_method
            );

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::DynInstance))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = dyn_instances
                    .inputs
                    .iter()
                    .find(|input| input.id == io.name)
                {
                    crate::log!(
                        "debug_layout shader {:?}: dyn_instance {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = instances.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: rust_instance {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::Uniform))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = dyn_uniforms.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: dyn_uniform {:?} ty={:?} size={} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        pod_ty.ty.size_of(),
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::VertexBuffer))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = geometries.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: vertex_buffer {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for input in &scope_uniforms.inputs {
                crate::log!(
                    "debug_layout shader {:?}: scope_uniform {:?} slots={} offset={} attr={:?}",
                    source.as_object(),
                    input.id,
                    input.slots,
                    input.offset,
                    input.attr_format
                );
            }

            for (type_name, idx) in &uniform_buffer_bindings.bindings {
                crate::log!(
                    "debug_layout shader {:?}: uniform_buffer {} -> b{}",
                    source.as_object(),
                    type_name,
                    idx
                );
            }

            crate::log!(
                "debug_layout shader {:?}: totals dyn_instances={} instances={} dyn_uniforms={} geometries={} scope_uniforms={} textures={}",
                source.as_object(),
                dyn_instances.total_slots,
                instances.total_slots,
                dyn_uniforms.total_slots,
                geometries.total_slots,
                scope_uniforms.total_slots,
                textures.len()
            );
        }

        // Check if shader uses draw_pass->time (requires repaint every frame)
        let uses_time = match &code {
            CxDrawShaderCode::Combined { code } => code.contains("draw_pass->time"),
            CxDrawShaderCode::Separate { vertex, fragment } => {
                vertex.contains("draw_pass->time") || fragment.contains("draw_pass->time")
            }
        };

        CxDrawShaderMapping {
            source,
            code,
            flags: DrawShaderFlags {
                debug_draw,
                debug_layout,
                debug_code,
                async_compile,
                ..DrawShaderFlags::default()
            },
            instances,
            dyn_instances,
            dyn_uniforms,
            geometries,
            textures,
            uniform_buffers,
            samplers: output.samplers.clone(),
            texture_sampler_indices,
            uses_time,
            rect_pos,
            rect_size,
            draw_clip,
            uniform_buffer_bindings,
            scope_uniforms,
            scope_uniform_sources,
            scope_uniforms_buf,
            table_consts,
            scope_uniforms_gen: 0,
            geometry_id,
            varying_total_slots: 0,
            color_format,
        }
    }

    /// Shader-side geom stride in bytes. All-F32 shaders report `total_slots * 4`.
    pub fn geometry_stride_bytes(&self) -> usize {
        if self.geometries.stride_bytes != 0 {
            self.geometries.stride_bytes
        } else {
            self.geometries.total_slots.saturating_mul(4)
        }
    }

    pub fn geometry_is_compact(&self) -> bool {
        self.geometries.has_compact()
    }

    /// Write one table constant's live value into its scope-uniform slot
    /// and bump the generation so GPU-side copies refresh. Returns false
    /// for an index the shader does not have.
    pub fn patch_table_const(&mut self, index: usize, value: f32, uniforms_gen: u64) -> bool {
        let Some(tc) = self.table_consts.get_mut(index) else {
            return false;
        };
        tc.value = value;
        let input = &self.scope_uniforms.inputs[tc.input];
        if let Some(slot) = self.scope_uniforms_buf.get_mut(input.offset) {
            *slot = value;
        }
        debug_assert_ne!(uniforms_gen, 0);
        self.scope_uniforms_gen = uniforms_gen;
        true
    }

    /// Fill the scope uniform buffer from script values.
    ///
    /// This reads values from the script heap using the source_obj and key for each entry,
    /// converts them to f32 slots, and writes to the buffer.
    pub fn fill_scope_uniforms_buffer(
        &mut self,
        heap: &ScriptHeap,
        trap: &crate::makepad_script::trap::ScriptTrap,
    ) {
        for (i, input) in self.scope_uniforms.inputs.iter().enumerate() {
            if i >= self.scope_uniform_sources.len() {
                break;
            }
            let (source_obj, key) = match self.scope_uniform_sources[i] {
                ScopeUniformSlot::Scope(obj, key) => (obj, key),
                ScopeUniformSlot::Const(ci) => {
                    // A table constant: its live value, never the heap.
                    if let Some(slot) = self.scope_uniforms_buf.get_mut(input.offset) {
                        *slot = self.table_consts[ci].value;
                    }
                    continue;
                }
            };

            // Read the value from the heap
            let value = heap.scope_value(source_obj, key, *trap);

            // Write value to buffer at the input's offset
            DrawVars::write_value_to_f32_slots(
                heap,
                value,
                &mut self.scope_uniforms_buf,
                input.offset,
                input.slots,
                DrawShaderAttrFormat::Float,
            );
        }
    }

    /*
    pub fn from_draw_shader_def(draw_shader_def: &DrawShaderDef, const_table: DrawShaderConstTable, uniform_packing: DrawShaderInputPacking) -> CxDrawShaderMapping { //}, options: ShaderCompileOptions, metal_uniform_packing:bool) -> Self {

        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut var_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut live_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut draw_call_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut live_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut draw_list_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut draw_call_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut pass_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut textures = Vec::new();
        let mut instance_enums = Vec::new();
        let mut rect_pos = None;
        let mut rect_size = None;
        let mut draw_clip = None;
        for field in &draw_shader_def.fields {
            let ty = field.ty_expr.ty.borrow().as_ref().unwrap().clone();
            match &field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    geometries.push(field.ident.0, ty, None);
                }
                DrawShaderFieldKind::Instance {var_def_ptr, live_field_kind, ..} => {
                    if field.ident.0 == live_id!(rect_pos) {
                        rect_pos = Some(instances.total_slots);
                    }
                    if field.ident.0 == live_id!(rect_size) {
                        rect_size = Some(instances.total_slots);
                    }
                    if field.ident.0 == live_id!(draw_clip) {
                        draw_clip = Some(instances.total_slots);
                    }
                    if var_def_ptr.is_some() {
                        var_instances.push(field.ident.0, ty.clone(), None,);
                    }
                    if let ShaderTy::Enum{..} = ty{
                        instance_enums.push(instances.total_slots);
                    }
                    instances.push(field.ident.0, ty, None);
                    if let LiveFieldKind::Live = live_field_kind {
                        live_instances.inputs.push(instances.inputs.last().unwrap().clone());
                    }
                }
                DrawShaderFieldKind::Uniform {block_ident, ..} => {
                    match block_ident.0 {
                        live_id!(draw_call) => {
                            draw_call_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(draw_list) => {
                            draw_list_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(pass) => {
                            pass_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(user) => {
                            draw_call_uniforms.push(field.ident.0, ty, None);
                        }
                        _ => ()
                    }
                }
                DrawShaderFieldKind::Texture {..} => {
                    textures.push(DrawShaderTextureInput {
                        ty:ty,
                        id: field.ident.0,
                    });
                }
                _ => ()
            }
        }

        geometries.finalize();
        instances.finalize();
        var_instances.finalize();
        draw_call_uniforms.finalize();
        live_uniforms.finalize();
        draw_list_uniforms.finalize();
        draw_call_uniforms.finalize();
        pass_uniforms.finalize();

        // fill up the default values for the user uniforms


        // ok now the live uniforms
        for (value_node_ptr, ty) in draw_shader_def.all_live_refs.borrow().iter() {
            live_uniforms.push(LiveId(0), ty.clone(), Some(value_node_ptr.0));
        }

        CxDrawShaderMapping {
            const_table,
            uses_time: draw_shader_def.uses_time.get(),
            flags: draw_shader_def.flags,
            geometries,
            instances,
            live_uniforms_buf: {let mut r = Vec::new(); r.resize(live_uniforms.total_slots, 0.0); r},
            var_instances,
            live_instances,
            draw_call_uniforms,
            live_uniforms,
            draw_list_uniforms,
            draw_call_uniforms,
            pass_uniforms,
            instance_enums,
            textures,
            rect_pos,
            rect_size,
            draw_clip,
        }
    }*/
    /*
    pub fn update_live_and_user_uniforms(&mut self, cx: &mut Cx, apply: &Apply) {
        // and write em into the live_uniforms buffer
        let live_registry = cx.live_registry.clone();
        let live_registry = live_registry.borrow();

        for input in &self.live_uniforms.inputs {
            let (nodes,index) = live_registry.ptr_to_nodes_index(input.live_ptr.unwrap());
            DrawVars::apply_slots(
                cx,
                input.slots,
                &mut self.live_uniforms_buf,
                input.offset,
                apply,
                index,
                nodes
            );
        }
    }*/
}
