use {
    crate::{
        cx::Cx,
        draw_vars::{DrawVars, DRAW_CALL_UNIFORM_BUFFER_SLOTS},
        geometry::GeometryId,
        makepad_live_id::*,
        makepad_script::heap::ScriptHeap,
        makepad_script::pod::{ScriptPodTy, ScriptPodVec},
        makepad_script::shader::*,
        makepad_script::literal::{LiteralOrigin, LiteralReach, LiteralSite, LiteralValue},
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
    /// Premultiplied source-over blending for this draw call. `false` makes a
    /// fragment replace the destination, alpha included. Honoured per draw
    /// call by every backend: Metal binds a second pipeline state with
    /// blending disabled (BGRA8 window targets only; data formats keep their
    /// never-blending pipeline), D3D11 binds a blend state with BlendEnable
    /// off, Vulkan keys its pipeline on the flag, GL and WebGL toggle
    /// GL_BLEND around the call, and the gpusim rasteriser blends only
    /// when the target is BGRA8 AND the flag is set. With `true`, GL, WebGL,
    /// D3D11 and Vulkan blend into any target format the pass gives them;
    /// only Metal's data formats stay unblended. Shaders declared with
    /// `alpha_blend: false` must output alpha 1.0 or intend a raw write.
    pub alpha_blend: bool,
    pub backface_culling: bool,
    /// Scissor rectangle (x, y, width, height) in target pixels, row 0 at
    /// the top; `None` = the whole target. Fragments outside are dropped
    /// before shading, which a `discard` cannot do: a pass that renders
    /// several tiles of one target (the sun's shadow cascades) confines
    /// each draw to its tile with it. Honoured by Metal and Vulkan; the
    /// other backends ignore it, so a draw must stay correct without it.
    pub scissor: Option<[u32; 4]>,
}

impl Default for CxDrawShaderOptions {
    fn default() -> Self {
        Self {
            draw_call_group: LiveId(0),
            debug_id: None,
            depth_write: true,
            alpha_blend: true,
            backface_culling: false,
            scissor: None,
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
    /// Keyed by the generated code and the pipeline state
    /// (`DrawVars::pipeline_state_hash`): one code with another colour
    /// format or blend op is another shader.
    pub cache_code_to_shader: HashMap<(CxDrawShaderCode, LiveId), DrawShaderId>,
    /// Installed shader packs and, on native, a recording
    /// ([`crate::shader_pack`]).
    pub packs: crate::shader_pack::CxShaderPacks,
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

    /// Edit mode's literal-site map for the files `edited` (the names their
    /// compiled copies' origins give, see [`Self::set_literal_source`];
    /// none = off). On, every float and colour literal a compiled text
    /// copies from an edited file lands somewhere [`Self::patch_literal`]
    /// reaches: shaders read it from a uniform slot, kernels from a hidden
    /// parameter, a running program's VM from its code and what load
    /// stored it in (see [`makepad_script::literal`]). A literal-only edit of
    /// the source then compiles to the same program text (the GPU program
    /// is reused). Off (the default, and for shipped or web playback)
    /// literals fold and compiled code is byte-identical to a build without
    /// the map. Takes effect for what compiles from now on.
    pub fn set_live_literals(&mut self, edited: Vec<String>) {
        if !crate::makepad_script::literal::set_edited(edited) {
            return;
        }
        self.draw_shaders.reset_for_live_reload();
        self.draw_shaders.cache_code_to_shader.clear();
    }

    pub fn live_literals_on(&self) -> bool {
        crate::makepad_script::literal::edit_mode()
    }

    /// Name the rows of compiled text `module` (a `ScriptMod::file`, a
    /// kernel's name) that are copies of an edited file: its literals there
    /// compile live. Replaces what was registered under that name; an empty
    /// list removes it. A host that compiles text copied out of a document
    /// (a layer's shader fn, a pass) registers its origins just before.
    pub fn set_literal_source(&mut self, module: &str, origins: Vec<LiteralOrigin>) {
        // The same text compiled where it now sits names other sites: the
        // function-keyed caches would hand back the old ones.
        if crate::makepad_script::literal::set_source(module, origins) && self.live_literals_on() {
            self.draw_shaders.reset_for_live_reload();
        }
    }

    /// Set the literal at `site` to `value` everywhere it landed, for the
    /// next frame: every compiled shader slot made from it, and every
    /// registered sink (kernel parameters, a running program's VM). What it
    /// reached, and whether a reload is still needed (the literal also fed
    /// a value computed while the program loaded).
    pub fn patch_literal(&mut self, site: &LiteralSite, value: LiteralValue) -> LiteralReach {
        let uniforms_gen = self.next_uniform_gen();
        let mut wrote = 0;
        for sh in self.draw_shaders.shaders.iter_mut() {
            if !sh.mapping.table_consts.is_empty() {
                wrote += sh.mapping.patch_literal(site, value, uniforms_gen);
            }
        }
        let mut reach = crate::makepad_script::literal::patch_sinks(site, value);
        reach.places += wrote;
        if self.draw_shaders.shaders.iter().any(|sh| sh.mapping.folded_sites.contains(site)) {
            reach.reload.push("compiled into a shader as a constant (an integer)".to_string());
        }
        if reach.places > 0 {
            self.redraw_all();
        }
        reach
    }

    /// Every live literal site the compiled shaders hold, with its value
    /// now (an editor's or a test's view of what patches will reach).
    pub fn shader_literal_sites(&self) -> Vec<(DrawShaderId, LiteralSite, LiteralValue)> {
        let mut out = Vec::new();
        for (index, sh) in self.draw_shaders.shaders.iter().enumerate() {
            for tc in &sh.mapping.table_consts {
                let Some(site) = &tc.site else { continue };
                let value = match tc.color {
                    Some(c) => LiteralValue::Color(c),
                    None => LiteralValue::Number(if tc.negated { -tc.value } else { tc.value } as f64),
                };
                out.push((DrawShaderId { index }, site.clone(), value));
            }
        }
        out
    }
}

impl CxDrawShaders {
    /// The shader compiled before from the same code, when it can stand
    /// for this compile. A table constant's value is the shader's own (a
    /// live literal compiles to the same text whatever it says), so a
    /// compile whose table holds other values or sites is a shader of its
    /// own: it gets its own mapping, and the backend reuses the GPU
    /// program by its source.
    pub fn code_hit(&self, code: &CxDrawShaderCode, pipe: LiveId, table: &[ShaderTableConst]) -> Option<DrawShaderId> {
        let id = *self.cache_code_to_shader.get(&(code.clone(), pipe))?;
        let have = &self.shaders.get(id.index)?.mapping.table_consts;
        let same = have.len() == table.len()
            && have.iter().zip(table).all(|(h, t)| {
                h.initial == t.value as f32 && h.initial_color == t.color && h.site == t.site && h.doc == t.doc
            });
        same.then_some(id)
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

    /// Decode one packed vertex into f32 logical slots (gpusim fetch).
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

/// How a blending pipeline combines a fragment with the target. Declared in
/// the shader DSL as `blend_op: @Max`; the default is premultiplied over.
/// Baked into the shader's pipelines on every backend (a per-shader
/// property, like `color_format`); `alpha_blend: false` draw calls still
/// write raw.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum DrawShaderBlendOp {
    /// `src + dst * (1 - src.a)` (premultiplied source-over; additive when
    /// the shader outputs alpha 0).
    #[default]
    Over,
    /// Per-channel `max(src, dst)`: overlapping glows and lines keep the
    /// brightest value instead of summing.
    Max,
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
    /// A value written when the mapping was built (a prebuilt shader's
    /// recorded value, where its source could not be found again).
    Fixed,
}

/// One io of a draw shader as the mapping reads it: the compiler's
/// `ShaderIo` with its pod type resolved out of the heap, so a mapping is
/// built the same way from a compile and from a prebuilt shader.
#[derive(Clone, Debug)]
pub struct DrawShaderDescIo {
    pub kind: ShaderIoKind,
    pub name: LiveId,
    pub ty: ScriptPodTy,
    /// The pod type's name (the draw system's own uniform buffers are told
    /// apart by it).
    pub pod_name: Option<LiveId>,
    /// The heap's pod type (custom uniform buffers keep it).
    pub pod_type: ScriptPodType,
    pub buffer_index: Option<usize>,
    /// Uniform buffers: the block name the code declares.
    pub block_name: String,
    /// Textures: the sampler the code samples it with.
    pub sampler: usize,
}

/// Where a scope uniform's value comes from.
#[derive(Clone, Debug)]
pub enum DrawShaderDescScope {
    /// Read from the heap: `key` looked up from `obj`'s scope.
    Scope(ScriptObject, LiveId),
    /// The table constant at this index.
    Const(usize),
    /// These slot values.
    Fixed(Vec<f32>),
}

#[derive(Clone, Debug)]
pub struct DrawShaderDescScopeUniform {
    pub shader_name: LiveId,
    pub slots: usize,
    pub source: DrawShaderDescScope,
}

/// Everything a draw shader's mapping needs from its compile besides the
/// code: the io in the compiler's order, samplers, scope uniforms (in io
/// order), table constants and uniform buffer bindings. A compile makes one
/// ([`DrawShaderDesc::from_output`]); a prebuilt shader carries one.
#[derive(Clone, Debug, Default)]
pub struct DrawShaderDesc {
    pub io: Vec<DrawShaderDescIo>,
    pub samplers: Vec<ShaderSampler>,
    pub scope_uniforms: Vec<DrawShaderDescScopeUniform>,
    pub table_consts: Vec<ShaderTableConst>,
    /// Live literals that stayed folded (see `ShaderOutput::folded_sites`).
    pub folded_sites: Vec<LiteralSite>,
    pub uniform_buffer_bindings: UniformBufferBindings,
}

impl DrawShaderDesc {
    /// The description of a finished compile (after
    /// `assign_uniform_buffer_indices`).
    pub fn from_output(heap: &ScriptHeap, output: &ShaderOutput) -> Self {
        let io = output
            .io
            .iter()
            .map(|io| {
                let pod = heap.pod_type_ref(io.ty);
                DrawShaderDescIo {
                    kind: io.kind.clone(),
                    name: io.name,
                    ty: pod.ty.clone(),
                    pod_name: pod.name,
                    pod_type: io.ty,
                    buffer_index: io.buffer_index,
                    block_name: match io.kind {
                        ShaderIoKind::UniformBuffer => {
                            format!("{}_Uniforms", output.backend.map_io_name(io.name))
                        }
                        _ => String::new(),
                    },
                    sampler: match io.kind {
                        ShaderIoKind::Texture(_) => {
                            let texture_name = format!("tex_{}", io.name);
                            output
                                .texture_sampler_bindings
                                .iter()
                                .find(|(bound_texture, _)| bound_texture == &texture_name)
                                .map(|(_, idx)| *idx)
                                .unwrap_or(0)
                        }
                        _ => 0,
                    },
                }
            })
            .collect();
        let mut scope_uniforms = Vec::new();
        for io in &output.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                if let Some(source) = output
                    .scope_uniforms
                    .iter()
                    .find(|su| su.shader_name == io.name)
                {
                    scope_uniforms.push(DrawShaderDescScopeUniform {
                        shader_name: io.name,
                        slots: heap.pod_type_ref(source.ty).ty.slots(),
                        source: match source.table_const {
                            Some(ci) => DrawShaderDescScope::Const(ci),
                            None => DrawShaderDescScope::Scope(source.source_obj, source.key),
                        },
                    });
                }
            }
        }
        DrawShaderDesc {
            io,
            samplers: output.samplers.clone(),
            scope_uniforms,
            table_consts: output.table_consts.clone(),
            folded_sites: output.folded_sites.clone(),
            uniform_buffer_bindings: output.get_uniform_buffer_bindings(heap),
        }
    }
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
    /// A live literal's place in its edited file (edit mode, see
    /// `Cx::set_shader_live_literals`); `None` for a tweaker constant.
    pub site: Option<LiteralSite>,
    /// A colour literal's live value (its slot is a vec4); `value` and
    /// `initial` are unused then.
    pub color: Option<[f32; 4]>,
    pub initial_color: Option<[f32; 4]>,
    /// The slot holds the literal negated (`/**x*/ -0.5`).
    pub negated: bool,
}


#[derive(Clone)]
pub struct CxDrawShaderMapping {
    pub source: ScriptObjectRef,
    pub code: CxDrawShaderCode,
    /// The pick variant's source (`Cx::enable_pick_variants`).
    pub pick_code: Option<String>,
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
    /// Where an instance keeps the depth it is drawn at, when it keeps one:
    /// the slot `Cx2d::lift_align_range` adds to so already-drawn ink wins
    /// the depth test against ink painted after it.
    pub draw_depth: Option<usize>,
    pub uniform_buffer_bindings: UniformBufferBindings,
    pub scope_uniforms: DrawShaderInputs,
    pub scope_uniform_sources: Vec<ScopeUniformSlot>,
    pub scope_uniforms_buf: Vec<f32>,
    /// The shader's hot-patchable constants (annotated literals compiled
    /// under const-table mode), each backed by one scope-uniform slot.
    pub table_consts: Vec<DrawShaderTableConst>,
    /// Live literals compiled in as constants: a patch of one needs a new
    /// program.
    pub folded_sites: Vec<LiteralSite>,
    /// Bumped by every patch of `scope_uniforms_buf`; backends that keep a
    /// GPU-side copy of the buffer re-upload when it moves.
    pub scope_uniforms_gen: u64,
    pub geometry_id: Option<GeometryId>,
    /// Total f32 slots in the varying buffer (instances + explicit varyings).
    /// Set by the gpusim backend during shader compilation.
    pub varying_total_slots: usize,
    /// The color-attachment format this shader's pipeline targets.
    pub color_format: DrawShaderColorFormat,
    /// The blend operation of this shader's blending pipelines.
    pub blend_op: DrawShaderBlendOp,
    /// Bit `i` set when the shader declares `fragment_output(i, …)`. In a
    /// pass with several color attachments (MRT) an attachment the shader
    /// does not write keeps its contents: its write mask is off.
    pub fragment_outputs: u8,
    /// The typed instance and vertex fields, for `layout_of`.
    pub reflection: crate::draw_shader_layout::DrawShaderReflection,
}

impl CxDrawShaderMapping {
    pub(crate) fn attr_format_from_pod_type(ty: &ScriptPodTy) -> DrawShaderAttrFormat {
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

    pub(crate) fn push_pod_fields(
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
        let desc = DrawShaderDesc::from_output(heap, output);
        Self::from_desc(source, code, heap, &desc, geometry_id)
    }

    /// The mapping of a draw shader from its io description, a compile's
    /// ([`Self::from_shader_output`]) or a prebuilt shader's: the draw-call
    /// flags and pipeline state come from the shader object, the instance,
    /// uniform and geometry layouts from the io with this target's packing.
    pub fn from_desc(
        source: ScriptObjectRef,
        code: CxDrawShaderCode,
        heap: &ScriptHeap,
        desc: &DrawShaderDesc,
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
        let blend_op = match heap.value(source.as_object(), id!(blend_op).into(), NoTrap).as_id() {
            Some(id) if id == id!(Max) => DrawShaderBlendOp::Max,
            _ => DrawShaderBlendOp::Over,
        };
        let fragment_outputs = desc.io.iter().fold(0u8, |mask, io| match io.kind {
            ShaderIoKind::FragmentOutput(index) if index < 8 => mask | (1 << index),
            _ => mask,
        });
        // The instance record's word layout (attribute packing), which the
        // backends read by instance index (`ShaderOutput::instance_record`);
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
        let mut draw_depth = None;

        // Memory layout: DynInstance fields first, then RustInstance fields
        // This matches metal_create_instance_struct

        // 1. Process DynInstance fields first (added to both instances and dyn_instances)
        for io in &desc.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let slots = io.ty.slots();
                let attr_format = Self::attr_format_from_pod_type(&io.ty);
                Self::push_pod_fields(&mut instances, &io.ty, io.name);
                dyn_instances.push(io.name, slots, attr_format);
            }
        }

        // 2. Process RustInstance fields after (already in correct order from pre_collect_rust_instance_io)
        for io in desc
            .io
            .iter()
            .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
        {
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
            // The depth slot, under either of the two names the shaders in
            // this repo give it: quads, vectors and SVGs call it
            // `draw_depth`, the glyph shaders `glyph_depth`. No shader has
            // both, so one slot names the depth of any instance.
            if io.name == live_id!(draw_depth) || io.name == live_id!(glyph_depth) {
                draw_depth = Some(instances.total_slots);
            }

            Self::push_pod_fields(&mut instances, &io.ty, io.name);
        }

        // Process Uniform fields
        for io in &desc.io {
            if let ShaderIoKind::Uniform = io.kind {
                dyn_uniforms.push(io.name, io.ty.slots(), DrawShaderAttrFormat::Float);
            }
        }

        // Process VertexBuffer (geometry) fields. Compact POD structs are
        // flattened to per-field physical formats; all-F32 structs stay one
        // blob so packed_geometry_N codegen remains byte-identical.
        for io in &desc.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                Self::push_pod_fields(&mut geometries, &io.ty, io.name);
            }
        }

        // Process texture fields.
        for io in &desc.io {
            if let ShaderIoKind::Texture(tex_type) = &io.kind {
                textures.push(DrawShaderTextureInput {
                    id: io.name,
                    tex_type: *tex_type,
                });
                texture_sampler_indices.push(io.sampler);
            }
        }

        for io in &desc.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                if matches!(
                    io.pod_name,
                    Some(id!(DrawPassUniforms))
                        | Some(id!(DrawListUniforms))
                        | Some(id!(DrawCallUniforms))
                ) {
                    continue;
                }
                uniform_buffers.push(DrawShaderUniformBufferInput {
                    id: io.name,
                    block_name: io.block_name.clone(),
                    ty: io.pod_type,
                    size: io.ty.size_of(),
                    align: io.ty.align_of(),
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

        let uniform_buffer_bindings = desc.uniform_buffer_bindings.clone();

        // Build scope uniforms layout using DrawShaderInputs (4-byte slot alignment)
        let mut scope_uniforms = DrawShaderInputs::new(uniform_packing());
        let mut scope_uniform_sources = Vec::new();

        // Scope uniforms in the order they appear in the io list.
        let mut table_consts: Vec<DrawShaderTableConst> = Vec::new();
        let mut fixed = Vec::new();
        for su in &desc.scope_uniforms {
            let input = scope_uniforms.inputs.len();
            scope_uniforms.push(su.shader_name, su.slots, DrawShaderAttrFormat::Float);
            match &su.source {
                DrawShaderDescScope::Const(ci) => {
                    let tc = &desc.table_consts[*ci];
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
                        site: tc.site.clone(),
                        color: tc.color,
                        initial_color: tc.color,
                        negated: tc.negated,
                    });
                }
                DrawShaderDescScope::Scope(obj, key) => {
                    scope_uniform_sources.push(ScopeUniformSlot::Scope(*obj, *key))
                }
                DrawShaderDescScope::Fixed(values) => {
                    scope_uniform_sources.push(ScopeUniformSlot::Fixed);
                    fixed.push((input, values));
                }
            }
        }
        scope_uniforms.finalize();

        // Allocate the buffer for scope uniforms (as f32 slots)
        let mut scope_uniforms_buf = vec![0.0f32; scope_uniforms.total_slots];
        for (input, values) in fixed {
            let input = &scope_uniforms.inputs[input];
            for (slot, value) in values.iter().take(input.slots).enumerate() {
                scope_uniforms_buf[input.offset + slot] = *value;
            }
        }

        if debug_layout {
            crate::log!(
                "debug_layout shader {:?}: flags draw={} layout={} code={} uniform_packing={:?}",
                source.as_object(),
                debug_draw,
                debug_layout,
                debug_code,
                dyn_uniforms.packing_method
            );

            for io in desc
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::DynInstance))
            {
                if let Some(input) = dyn_instances
                    .inputs
                    .iter()
                    .find(|input| input.id == io.name)
                {
                    crate::log!(
                        "debug_layout shader {:?}: dyn_instance {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        io.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in desc
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
            {
                if let Some(input) = instances.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: rust_instance {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        io.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in desc
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::Uniform))
            {
                if let Some(input) = dyn_uniforms.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: dyn_uniform {:?} ty={:?} size={} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        io.ty,
                        io.ty.size_of(),
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in desc
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::VertexBuffer))
            {
                if let Some(input) = geometries.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: vertex_buffer {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        io.ty,
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
            pick_code: None,
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
            samplers: desc.samplers.clone(),
            texture_sampler_indices,
            uses_time,
            rect_pos,
            rect_size,
            draw_clip,
            draw_depth,
            uniform_buffer_bindings,
            scope_uniforms,
            scope_uniform_sources,
            scope_uniforms_buf,
            table_consts,
            folded_sites: desc.folded_sites.clone(),
            scope_uniforms_gen: 0,
            geometry_id,
            varying_total_slots: 0,
            color_format,
            blend_op,
            fragment_outputs,
            reflection: crate::draw_shader_layout::DrawShaderReflection::from_desc(desc),
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

    /// Write `value` into every live literal slot compiled from `site`;
    /// how many it wrote (a number never lands in a colour's slot, nor the
    /// reverse).
    pub fn patch_literal(&mut self, site: &LiteralSite, value: LiteralValue, uniforms_gen: u64) -> usize {
        let mut wrote = 0;
        for tc in self.table_consts.iter_mut() {
            if tc.site.as_ref() != Some(site) {
                continue;
            }
            let offset = self.scope_uniforms.inputs[tc.input].offset;
            match (value, tc.color.is_some()) {
                (LiteralValue::Number(v), false) => {
                    let v = if tc.negated { -v } else { v } as f32;
                    tc.value = v;
                    if let Some(slot) = self.scope_uniforms_buf.get_mut(offset) {
                        *slot = v;
                    }
                }
                (LiteralValue::Color(c), true) => {
                    tc.color = Some(c);
                    if let Some(slots) = self.scope_uniforms_buf.get_mut(offset..offset + 4) {
                        slots.copy_from_slice(&c);
                    }
                }
                _ => continue,
            }
            wrote += 1;
        }
        if wrote > 0 {
            self.scope_uniforms_gen = uniforms_gen;
        }
        wrote
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
                    let tc = &self.table_consts[ci];
                    match tc.color {
                        Some(c) => {
                            if let Some(slots) = self.scope_uniforms_buf.get_mut(input.offset..input.offset + 4) {
                                slots.copy_from_slice(&c);
                            }
                        }
                        None => {
                            if let Some(slot) = self.scope_uniforms_buf.get_mut(input.offset) {
                                *slot = tc.value;
                            }
                        }
                    }
                    continue;
                }
                ScopeUniformSlot::Fixed => continue,
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
