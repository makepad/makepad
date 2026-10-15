//! Splash compilation state: VM provenance around the shared backend output.
use makepad_relax_api::relax_api;
use crate::object::ScriptMapEntry;
use crate::{heap::*, value::*, vm::*, shader_backend::*};
use makepad_live_id::*;
pub use makepad_shader::shader_output::{SamplerFilter, SamplerAddress, SamplerCoord, ShaderSampler, ShaderSamplerOptions, ShaderStorageFlags, TextureType, ShaderIoKind, ShaderIo, ShaderMode, ShaderIoKindTag, UniformBufferBindings};
#[relax_api(plain)]
#[derive(Debug, Clone)]
pub struct ScopeUniformSource {
    /// The source object to read the value from
    pub source_obj: ScriptObject,
    /// The key to read from the source object
    pub key: LiveId,
    /// The name used in the shader (may be prefixed for collision avoidance)
    pub shader_name: LiveId,
    /// The pod type of this uniform
    pub ty: ScriptPodType,
    /// `Some(i)`: this slot is `ShaderOutput::table_consts[i]` — an
    /// annotated literal lifted out of the code — and is filled from the
    /// table's current value, never from the heap.
    pub table_const: Option<usize>,
}

/// A numeric literal inside a shader fn body that carried a `/** … */`
/// annotation and was compiled under [`ShaderOutput::const_table`]: it is
/// read from the scope-uniform buffer at draw time instead of being folded
/// into the code, so its value can be hot-patched with zero recompiles. The
/// source is never rewritten by a patch — the span is context for whoever
/// eventually edits it (the ledger hands it to the AI).
#[relax_api(handle)]
#[derive(Debug, Clone)]
pub struct ShaderTableConst {
    /// The scope-uniform io name this constant occupies (`ct0`, `ct1`, …).
    pub shader_name: LiveId,
    /// The annotation text as written (hint grammar not yet parsed).
    pub doc: String,
    /// The literal's value in the source.
    pub value: f64,
    /// Where the literal sits: the immediate's ip (resolves to file:line:col
    /// through `ScriptCode::ip_to_loc`).
    pub ip: ScriptIp,
    /// A live literal ([`ShaderOutput::live_literals`]): where it is written
    /// in its source file. `None` for an annotated constant outside them.
    pub site: Option<crate::literal::LiteralSite>,
    /// A colour literal (`#ff8800`): the slot is a vec4 holding these, and
    /// `value` is unused.
    pub color: Option<[f32; 4]>,
    /// The table holds the literal negated (`/**x*/ -0.5` holds -0.5): a
    /// patch of the site's literal writes its negation.
    pub negated: bool,
}

/// Tracks a uniform buffer defined in the script scope (e.g., `let buf = shader.uniform_buffer(...)`)
/// This is distinct from uniform buffers on io_self - these come from the surrounding scope.
#[relax_api(plain)]
#[derive(Debug, Clone)]
pub struct ScopeUniformBufferSource {
    /// The uniform buffer ScriptObject itself (needed for runtime buffer binding)
    pub obj: ScriptObject,
    /// The pod type of the uniform buffer's prototype
    pub pod_ty: ScriptPodType,
    /// The name used in the shader code
    pub shader_name: LiveId,
}

/// Tracks a texture defined in the script scope (e.g., `let tex = shader.texture_2d(float)`)
/// This is distinct from textures on io_self - these come from the surrounding scope.
#[relax_api(plain)]
#[derive(Debug, Clone)]
pub struct ScopeTextureSource {
    /// The texture ScriptObject itself (needed for runtime texture binding)
    pub obj: ScriptObject,
    /// The texture type (2d, cube, etc.)
    pub tex_type: TextureType,
    /// The name used in the shader code
    pub shader_name: LiveId,
}

#[relax_api(handle)]
#[derive(Default, Debug)]
pub struct ShaderOutput {
    function_ids: std::collections::HashMap<ScriptObject, makepad_shader::shader_ids::ShaderFnId>,
    function_objects: Vec<ScriptObject>,
    pub compiled: makepad_shader::shader_output::ShaderOutput,
    /// Functions lowered directly from source AST (inventory/diagnostics).
    pub ast_functions: usize,
    pub mode: ShaderMode,
    pub recur_block: Vec<ScriptObject>,
    pub scope_uniforms: Vec<ScopeUniformSource>,
    pub scope_uniform_buffers: Vec<ScopeUniformBufferSource>,
    pub scope_textures: Vec<ScopeTextureSource>,
    pub const_table: bool,
    pub table_consts: Vec<ShaderTableConst>,
    pub live_literals: Option<std::sync::Arc<crate::literal::LiveLiterals>>,
    pub folded_sites: Vec<crate::literal::LiteralSite>,
    pub scope_prefixes: Vec<usize>,
}
impl std::ops::Deref for ShaderOutput {
    type Target = makepad_shader::shader_output::ShaderOutput;
    fn deref(&self) -> &Self::Target { &self.compiled }
}
impl std::ops::DerefMut for ShaderOutput {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.compiled }
}
pub const MAX_EMITTED_BYTES: usize = 1 << 20;
impl ShaderOutput {
    pub(crate) fn register_function(&mut self, object: ScriptObject) -> makepad_shader::shader_ids::ShaderFnId {
        *self.function_ids.entry(object).or_insert_with(|| {
            self.function_objects.push(object);
            makepad_shader::shader_ids::ShaderFnId(self.function_objects.len() as u32)
        })
    }
    /// Frontend provenance for a function reached by shader-pack recording.
    /// Backend-synthesized entry functions have no VM object.
    pub fn function_object(&self, id: makepad_shader::shader_ids::ShaderFnId) -> Option<ScriptObject> {
        id.0.checked_sub(1).and_then(|index| self.function_objects.get(index as usize)).copied()
    }

    pub fn new_for(backend: ShaderBackend, use_vulkan: bool, const_table: bool, pick: bool) -> Self {
        Self { compiled: makepad_shader::shader_output::ShaderOutput { backend, use_vulkan, pick, ..Default::default() }, const_table, live_literals: crate::literal::live(), ..Default::default() }
    }
    pub fn type_name_into(&self, heap: &ScriptHeap, ty: ScriptPodType, out: &mut crate::vm_api::ScriptTextOut) {
        let mut text = String::new();
        self.compiled.backend.pod_type_name_from_ty(heap, ty.into(), &mut text);
        out.push(&text);
    }
    pub fn assign_uniform_buffers(&mut self, heap: &ScriptHeap, start_index: usize) -> usize { self.assign_uniform_buffer_indices(heap, start_index).1 }
    pub fn set_mode(&mut self, mode: ShaderMode) { self.mode = mode; }
    pub fn scope_uniforms(&self) -> &[ScopeUniformSource] { &self.scope_uniforms }
    pub fn table_consts(&self) -> &[ShaderTableConst] { &self.table_consts }
    pub fn folded_sites(&self) -> &[crate::literal::LiteralSite] { &self.folded_sites }
    fn validate_collected_vertex_fetch_io(&mut self, heap: &ScriptHeap) {
        let io: Vec<_> = self
            .io
            .iter()
            .map(|io| (io.kind.clone(), io.name, io.ty))
            .collect();
        for (kind, name, ty) in io {
            self.validate_vertex_fetch_io(heap, &kind, name, ty);
        }
    }
    pub fn pre_collect_rust_instance_io(&mut self, vm: &mut ScriptVm, io_self: ScriptObject) {
        self.pre_collect_rust_instance_io_recursive(vm, io_self);
    }
    fn pre_collect_rust_instance_io_recursive(&mut self, vm: &mut ScriptVm, obj: ScriptObject) {
        // First, recurse to prototype (to process deepest ancestor first)
        // This ensures parent's RustInstance fields come before child's fields
        if let Some(proto_obj) = vm.bx.heap.proto(obj).as_object() {
            self.pre_collect_rust_instance_io_recursive(vm, proto_obj);
        }

        // Then process this object's type properties
        let ty_index = vm.bx.heap.obj_info(obj).tag.as_type_index();

        if let Some(ty_index) = ty_index {
            // Collect the ordered props - iter_rust_instance_ordered returns all instance fields
            // (parent fields + this type's fields after deref) because rust_instance_start is now
            // correctly left at 0 (config fields before deref are skipped during script_proto_props)
            let type_check = vm.bx.heap.type_check(ty_index);
            // Collect into a Vec first to avoid borrow issues with heap mutation below
            let ordered_props: Vec<_> = type_check.props.iter_rust_instance_ordered().collect();

            for (field_id, type_id) in ordered_props {
                // Get the pod type from the type_id
                if let Some(pod_ty) = vm
                    .bx
                    .heap
                    .type_id_to_pod_type(type_id, &vm.bx.code.builtins.pod)
                {
                    // Skip if already added (handles duplicate prototypes in chain)
                    if !self.compiled.io.iter().any(|io| io.name == field_id) {
                        vm.bx.heap.pod_type_name_if_not_set(pod_ty, field_id);
                        self.compiled.io.push(ShaderIo {
                            kind: ShaderIoKind::RustInstance,
                            name: field_id,
                            ty: pod_ty.into(),
                            buffer_index: None, instance_lane: None,
                        });
                    }
                }
            }
        }
    }
    pub fn pre_collect_shader_io(&mut self, vm: &mut ScriptVm, io_self: ScriptObject) {
        // Use recursion to process from deepest prototype first (no temporary Vec needed)
        self.pre_collect_shader_io_recursive(vm, io_self);

        // Set pod type names for uniforms (requires mutable heap, done after iteration)
        for io in &self.compiled.io {
            if matches!(io.kind, ShaderIoKind::Uniform) {
                vm.bx.heap.pod_type_name_if_not_set(io.ty.into(), io.name);
            }
        }
        self.validate_collected_vertex_fetch_io(&vm.bx.heap);
    }
    fn pre_collect_shader_io_recursive(&mut self, vm: &ScriptVm, obj: ScriptObject) {
        use crate::mod_shader::*;

        // First recurse to prototype (to process deepest first)
        if let Some(proto_obj) = vm.bx.heap.proto(obj).as_object() {
            self.pre_collect_shader_io_recursive(vm, proto_obj);
        }

        // Then process this object's map entries in insertion order
        for ScriptMapEntry { key, value } in vm.bx.heap.map_entries_ordered(obj) {
            if let Some(value_obj) = value.as_object() {
                if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                    if let Some(field_id) = key.as_id() {
                        // Skip if already exists (derived class overrides)
                        if self.compiled.io.iter().any(|io| io.name == field_id) {
                            continue;
                        }
                        
                        // Get the pod type from the prototype
                        let proto_value = vm.bx.heap.proto(value_obj);
                        let pod_ty = Self::get_pod_type_from_value(vm, proto_value);
                        
                        // Handle different shader IO types
                        match io_type {
                            // Uniforms
                            SHADER_IO_DYN_UNIFORM => {
                                if let Some(pod_ty) = pod_ty {
                                    self.compiled.io.push(ShaderIo {
                                        kind: ShaderIoKind::Uniform,
                                        name: field_id,
                                        ty: pod_ty.into(),
                                        buffer_index: None, instance_lane: None,
                                    });
                                }
                            }

                            SHADER_IO_UNIFORM_BUFFER => {
                                if let Some(pod_ty) = pod_ty {
                                    self.compiled.io.push(ShaderIo {
                                        kind: ShaderIoKind::UniformBuffer,
                                        name: field_id,
                                        ty: pod_ty.into(),
                                        buffer_index: None, instance_lane: None,
                                    });
                                }
                            }
                            
                            // Fragment outputs
                            io_type if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 
                                    && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 => {
                                let index = (io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0) as u8;
                                // Check by index, not name
                                let already_exists = self.compiled.io.iter().any(|io| {
                                    matches!(io.kind, ShaderIoKind::FragmentOutput(idx) if idx == index)
                                });
                                if !already_exists {
                                    if let Some(pod_ty) = pod_ty {
                                        self.compiled.io.push(ShaderIo {
                                            kind: ShaderIoKind::FragmentOutput(index),
                                            name: field_id,
                                            ty: pod_ty.into(),
                                            buffer_index: None, instance_lane: None,
                                        });
                                    }
                                }
                            }
                            
                            // Textures
                            SHADER_IO_TEXTURE_1D => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture1d), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_1D_ARRAY => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture1dArray), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_2D => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture2d), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_2D_ARRAY => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture2dArray), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_3D => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture3d), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_3D_ARRAY => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture3dArray), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_CUBE => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureCube), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_CUBE_ARRAY => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureCubeArray), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_DEPTH => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureDepth), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_DEPTH_ARRAY => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureDepthArray), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),
                            SHADER_IO_TEXTURE_VIDEO => self.compiled.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureVideo), name: field_id, ty: ScriptPodType::VOID.into(), buffer_index: None, instance_lane: None }),

                            // Other IO types are handled during compilation or elsewhere
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    pub(crate) fn get_pod_type_from_value(
        vm: &ScriptVm,
        value: ScriptValue,
    ) -> Option<ScriptPodType> {
        // Check if it's a primitive type (f32, f64, bool, etc.)
        if let Some(pod_ty) = vm.bx.code.builtins.pod.value_to_exact_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a color - colors map to vec4f
        if value.is_color() {
            return Some(vm.bx.code.builtins.pod.pod_vec4f);
        }
        // Check if it's a pod type object
        if let Some(pod_ty) = vm.bx.heap.pod_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a pod instance
        if let Some(pod) = value.as_pod() {
            let pod = &vm.bx.heap.pods[pod];
            return Some(pod.ty);
        }
        // Check if it's a pod type reference
        if let Some(pod_ty) = value.as_pod_type() {
            return Some(pod_ty);
        }
        None
    }
    pub fn find_vertex_buffer_object(
        &self,
        vm: &ScriptVm,
        io_self: ScriptObject,
    ) -> Option<ScriptObject> {
        use crate::mod_shader::SHADER_IO_VERTEX_BUFFER;

        // Walk the prototype chain looking for vertex buffer properties
        let mut current = Some(io_self);
        while let Some(obj) = current {
            // Check map properties
            for ScriptMapEntry { value, .. } in vm.bx.heap.map_entries(obj) {
                if let Some(value_obj) = value.as_object() {
                    if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                        if io_type == SHADER_IO_VERTEX_BUFFER {
                            return Some(value_obj);
                        }
                    }
                }
            }

            // Move to next prototype
            current = vm.bx.heap.proto(obj).as_object();
        }
        None
    }
}
impl ScopeUniformSource {
    pub fn source_obj(&self) -> ScriptObject { self.source_obj }
    pub fn key(&self) -> LiveId { self.key }
    pub fn shader_name(&self) -> LiveId { self.shader_name }
    pub fn ty(&self) -> ScriptPodType { self.ty }
    pub fn table_const(&self) -> Option<usize> { self.table_const }
}
/// What a table constant reads as (ShaderTableConst::view); the colour is meaningful when `has_color`.
#[relax_api(plain)]
#[repr(C)]
pub struct ShaderTableConstView<'a> {
    pub shader_name: LiveId,
    pub value: f64,
    pub ip: ScriptIp,
    pub color: [f32; 4],
    pub has_color: bool,
    pub negated: bool,
    pub doc: &'a str,
    pub site: *const crate::literal::LiteralSite,
    pub _view: std::marker::PhantomData<&'a ()>,
}
#[relax_api(marked)]
impl ShaderTableConst {
    #[relax_api]
    pub fn view(&self) -> ShaderTableConstView<'_> {
        ShaderTableConstView {
            shader_name: self.shader_name,
            value: self.value,
            ip: self.ip,
            color: self.color.unwrap_or([0.0; 4]),
            has_color: self.color.is_some(),
            negated: self.negated,
            doc: &self.doc,
            site: self.site.as_ref().map_or(std::ptr::null(), |s| s as *const _),
            _view: std::marker::PhantomData,
        }
    }
    #[inline]
    pub fn shader_name(&self) -> LiveId { self.view().shader_name }
    #[inline]
    pub fn value(&self) -> f64 { self.view().value }
    #[inline]
    pub fn ip(&self) -> ScriptIp { self.view().ip }
    #[inline]
    pub fn color(&self) -> Option<[f32; 4]> { let v = self.view(); v.has_color.then_some(v.color) }
    #[inline]
    pub fn negated(&self) -> bool { self.view().negated }
    #[inline]
    pub fn doc(&self) -> &str { self.view().doc }
    #[inline]
    pub fn site(&self) -> Option<&crate::literal::LiteralSite> {
        // SAFETY: the pointer is the table constant's own site, borrowed from `self`
        unsafe { self.view().site.as_ref() }
    }
}
impl ShaderTableConst {
    pub fn new(shader_name: LiveId, doc: &str, value: f64) -> Self {
        Self { shader_name, doc: doc.to_owned(), value, ip: ScriptIp::default(), site: None, color: None, negated: false }
    }
}
