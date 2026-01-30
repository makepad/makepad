use {
    std::{
        ops::{Index, IndexMut},
        collections::BTreeSet,
        collections::HashMap,
    },
    crate::{
        
        makepad_live_id::*,
        makepad_script::ScriptObjectRef,
        makepad_script::shader::*,
        makepad_script::heap::ScriptHeap,
        makepad_script::value::ScriptObject,
        draw_vars::DrawVars,
        geometry::GeometryId,
        os::CxOsDrawShader,
        cx::Cx
    }
};

// Re-export UniformBufferBindings for use in other modules
pub use makepad_script::shader::UniformBufferBindings;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct CxDrawShaderOptions {
    pub draw_call_group: LiveId,
    pub debug_id: Option<LiveId>,
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
    
    pub cache_object_id_to_shader: HashMap<ScriptObject, DrawShaderId>,
    pub cache_functions_to_shader: LiveIdMap<LiveId, DrawShaderId>,
    pub cache_code_to_shader: HashMap<CxDrawShaderCode, DrawShaderId>,
    //pub ptr_to_item: HashMap<DrawShaderPtr, CxDrawShaderItem>,
    //pub fingerprints: Vec<DrawShaderFingerprint>,
    //pub error_set: HashSet<DrawShaderPtr>,
    // pub error_fingerprints: Vec<Vec<LiveNode >>,
}

impl CxDrawShaders{
    pub fn reset_for_live_reload(&mut self){
        /*
        self.ptr_to_item.clear();
        self.fingerprints.clear();
        self.error_set.clear();
        self.error_fingerprints.clear();*/
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

impl DrawShaderId{
    pub fn false_compare_check(&self)->u64{
        (self.index as u64)<<32 //| self.draw_shader_ptr.0.index as u64
    }
}

pub struct CxDrawShader {
    pub debug_id: LiveId,
    pub os_shader_id: Option<usize>,
    pub mapping: CxDrawShaderMapping
}

#[derive(Clone, Debug)]
pub struct DrawShaderInputs {
    pub inputs: Vec<DrawShaderInput>,
    pub packing_method: DrawShaderInputPacking,
    pub total_slots: usize,
}

#[derive(Clone, Copy, Debug)]
pub enum DrawShaderInputPacking {
    Attribute,
    UniformsGLSLTight,
    UniformsGLSL140,
    #[allow(dead_code)]
    UniformsHLSL,
    #[allow(dead_code)]
    UniformsMetal
}


#[derive(Clone, Debug)]
pub struct DrawShaderInput {
    pub id: LiveId,
    //pub ty: ShaderTy,
    pub offset: usize,
    pub slots: usize,
   // pub live_ptr: Option<LivePtr>
}

fn uniform_packing() -> DrawShaderInputPacking {
    #[cfg(any(target_arch = "wasm32"))]
    { return DrawShaderInputPacking::UniformsGLSLTight; }
    
    #[cfg(all(any(target_os = "android", target_os = "linux"), use_gles_3))]
    { return DrawShaderInputPacking::UniformsGLSL140; }
    
    #[cfg(all(any(target_os = "android", target_os = "linux"), not(use_gles_3)))]
    { return DrawShaderInputPacking::UniformsGLSLTight; }
    
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    { return DrawShaderInputPacking::UniformsMetal; }
    
    #[cfg(target_os = "windows")]
    { return DrawShaderInputPacking::UniformsHLSL; }
}

impl DrawShaderInputs {
    pub fn new(packing_method: DrawShaderInputPacking) -> Self {
        Self {
            inputs: Vec::new(),
            packing_method,
            total_slots: 0
        }
    }
    
    pub fn push(&mut self, id: LiveId, slots: usize) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsGLSLTight => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsGLSL140 => {
                if (self.total_slots & 3) + slots > 4 { // goes over the boundary
                    self.total_slots += 4 - (self.total_slots & 3); // make jump to new slot
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsHLSL => {
                if (self.total_slots & 3) + slots > 4 { // goes over the boundary
                    self.total_slots += 4 - (self.total_slots & 3); // make jump to new slot
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsMetal => {
                let aligned_slots = if slots == 3 {4} else {slots};
                if (self.total_slots & 3) + aligned_slots > 4 { // goes over the boundary
                    self.total_slots += 4 - (self.total_slots & 3); // make jump to new slot
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                });
                self.total_slots += aligned_slots;
            }
        }
    }
    
    pub fn finalize(&mut self) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => (),
            DrawShaderInputPacking::UniformsGLSLTight =>(),
            DrawShaderInputPacking::UniformsHLSL |
            DrawShaderInputPacking::UniformsMetal|
            DrawShaderInputPacking::UniformsGLSL140
            => {
                if self.total_slots & 3 > 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct DrawShaderTextureInput {
    pub id: LiveId,
    //pub ty: ShaderTy
}

#[derive(Clone, Copy, Default, Debug)]
pub struct DrawShaderFlags {
    pub debug: bool,
    pub draw_call_nocompare: bool,
    pub draw_call_always: bool,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub enum CxDrawShaderCode {
    Separate{vertex:String, fragment:String},
    Combined{code:String}
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
    pub uses_time: bool,
    pub rect_pos: Option<usize>,
    pub rect_size: Option<usize>,
    pub draw_clip: Option<usize>,
    pub uniform_buffer_bindings: UniformBufferBindings,
    pub scope_uniforms: DrawShaderInputs,
    pub scope_uniform_sources: Vec<(ScriptObject, LiveId)>,
    pub scope_uniforms_buf: Vec<f32>,
    pub geometry_id: Option<GeometryId>,
}

impl CxDrawShaderMapping {
    pub fn from_shader_output(source:ScriptObjectRef, code:CxDrawShaderCode, heap: &ScriptHeap, output: &ShaderOutput, geometry_id: Option<GeometryId>) -> CxDrawShaderMapping {
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
                instances.push(io.name, slots);
                dyn_instances.push(io.name, slots);
            }
        }
        
        // 2. Process RustInstance fields after (already in correct order from pre_collect_rust_instance_io)
        for io in output.io.iter().filter(|io| matches!(io.kind, ShaderIoKind::RustInstance)) {
            let pod_ty = heap.pod_type_ref(io.ty);
            let slots = pod_ty.ty.slots();
            
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
            
            instances.push(io.name, slots);
        }
        
        // Process Uniform fields
        for io in &output.io {
            if let ShaderIoKind::Uniform = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                dyn_uniforms.push(io.name, slots);
            }
        }
        
        // Process VertexBuffer (geometry) fields
        for io in &output.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                geometries.push(io.name, slots);
            }
        }
        
        // Process Texture and Sampler fields
        for io in &output.io {
            match &io.kind {
                ShaderIoKind::Texture(_) | ShaderIoKind::Sampler(_) => {
                    textures.push(DrawShaderTextureInput {
                        id: io.name,
                    });
                }
                _ => ()
            }
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
        for io in &output.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                // Find the corresponding ScopeUniformSource
                if let Some(source) = output.scope_uniforms.iter().find(|su| su.shader_name == io.name) {
                    let pod_ty = heap.pod_type_ref(source.ty);
                    let slots = pod_ty.ty.slots();
                    scope_uniforms.push(io.name, slots);
                    scope_uniform_sources.push((source.source_obj, source.key));
                }
            }
        }
        scope_uniforms.finalize();
        
        // Allocate the buffer for scope uniforms (as f32 slots)
        let scope_uniforms_buf = vec![0.0f32; scope_uniforms.total_slots];
        
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
            flags: DrawShaderFlags::default(),
            instances,
            dyn_instances,
            dyn_uniforms,
            geometries,
            textures,
            uses_time,
            rect_pos,
            rect_size,
            draw_clip,
            uniform_buffer_bindings,
            scope_uniforms,
            scope_uniform_sources,
            scope_uniforms_buf,
            geometry_id,
        }
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
            let (source_obj, key) = self.scope_uniform_sources[i];
            
            // Read the value from the heap
            let value = heap.scope_value(source_obj, key, *trap);
            
            // Write value to buffer at the input's offset
            DrawVars::write_value_to_f32_slots(heap, value, &mut self.scope_uniforms_buf, input.offset, input.slots);
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
