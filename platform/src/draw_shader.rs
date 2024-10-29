use {
    std::{
        ops::{Index, IndexMut},
        collections::{
            HashMap,
            HashSet,
            BTreeSet,
        },
    },
    crate::{
        makepad_live_compiler::{LiveValue, LiveFieldKind, LiveNode, LivePtr, LiveNodeSliceApi},
        makepad_shader_compiler::*,
        makepad_live_id::*,
        live_traits::*,
        draw_vars::DrawVars,
        os::CxOsDrawShader,
        cx::Cx
    }
};

#[derive(Default, Debug, Clone, PartialEq)]
pub struct CxDrawShaderOptions {
    pub draw_call_group: LiveId,
    pub debug_id: Option<LiveId>,
}

impl CxDrawShaderOptions {
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
    }
    
    pub fn _appendable_drawcall(&self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Default)]
pub struct CxDrawShaderItem {
    pub draw_shader_id: usize,
    pub options: CxDrawShaderOptions
}

#[derive(Default)]
pub struct CxDrawShaders {
    pub shaders: Vec<CxDrawShader>,
    pub os_shaders: Vec<CxOsDrawShader>,
    pub generation: u64,
    pub ptr_to_item: HashMap<DrawShaderPtr, CxDrawShaderItem>,
    pub compile_set: BTreeSet<DrawShaderPtr>,
    pub fingerprints: Vec<DrawShaderFingerprint>,
    pub error_set: HashSet<DrawShaderPtr>,
    pub error_fingerprints: Vec<Vec<LiveNode >>,
}

impl CxDrawShaders{
    pub fn reset_for_live_reload(&mut self){
        self.ptr_to_item.clear();
        self.fingerprints.clear();
        self.error_set.clear();
        self.error_fingerprints.clear();
    }
}

impl Cx {
    pub fn flush_draw_shaders(&mut self) {
        self.draw_shaders.generation += 1;
        self.shader_registry.flush_registry();
        self.draw_shaders.shaders.clear();
        self.draw_shaders.ptr_to_item.clear();
        self.draw_shaders.fingerprints.clear();
        self.draw_shaders.error_set.clear();
        self.draw_shaders.error_fingerprints.clear();
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
pub struct DrawShader {
    pub draw_shader_generation: u64,
    pub draw_shader_id: usize,
    pub draw_shader_ptr: DrawShaderPtr
}

impl DrawShader{
    pub fn false_compare_id(&self)->u64{
        ((self.draw_shader_id as u64)<<32) | self.draw_shader_ptr.0.index as u64
    }
}

pub struct CxDrawShader {
    pub class_prop: LiveId,
    pub type_name: LiveId,
    pub os_shader_id: Option<usize>,
    pub mapping: CxDrawShaderMapping
}

#[derive(Debug, PartialEq)]
pub struct DrawShaderFingerprint {
    pub fingerprint: Vec<LiveNode>,
    pub draw_shader_id: usize
}

impl DrawShaderFingerprint {
    pub fn from_ptr(cx: &Cx, draw_shader_ptr: DrawShaderPtr) -> Vec<LiveNode> {
        let live_registry_cp = cx.live_registry.clone();
        let live_registry = live_registry_cp.borrow();
        let doc = live_registry.ptr_to_doc(draw_shader_ptr.0);
        let mut node_iter = doc.nodes.first_child(draw_shader_ptr.node_index());
        let mut fingerprint = Vec::new();
        while let Some(node_index) = node_iter {
            let node = &doc.nodes[node_index];
            match node.value {
                LiveValue::DSL {token_start, token_count, ..} => {
                    fingerprint.push(LiveNode {
                        id: node.id,
                        origin: node.origin,
                        value: LiveValue::DSL {token_start, token_count, expand_index: None}
                    });
                }
                _ => ()
            }
            node_iter = doc.nodes.next_child(node_index);
        }
        fingerprint
    }
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
    UniformsGLSL,
    #[allow(dead_code)]
    UniformsHLSL,
    #[allow(dead_code)]
    UniformsMetal
}


#[derive(Clone, Debug)]
pub struct DrawShaderInput {
    pub id: LiveId,
    pub ty: ShaderTy,
    pub offset: usize,
    pub slots: usize,
    pub live_ptr: Option<LivePtr>
}


#[cfg(any(target_os = "android", target_os = "linux", target_arch = "wasm32"))]
pub const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsGLSL;
#[cfg(any(target_os = "macos", target_os = "ios", target_os="tvos"))]
pub const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsMetal;
#[cfg(any(target_os = "windows"))]
pub const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsHLSL;

impl DrawShaderInputs {
    pub fn new(packing_method: DrawShaderInputPacking) -> Self {
        Self {
            inputs: Vec::new(),
            packing_method,
            total_slots: 0
        }
    }
    
    pub fn push(&mut self, id: LiveId, ty: ShaderTy, live_ptr: Option<LivePtr>) {
        let slots = ty.slots();
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    ty,
                    live_ptr
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsGLSL => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    ty,
                    live_ptr
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
                    ty,
                    live_ptr
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
                    ty,
                    live_ptr
                });
                self.total_slots += aligned_slots;
            }
        }
    }
    
    pub fn finalize(&mut self) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => (),
            DrawShaderInputPacking::UniformsGLSL =>(),
            DrawShaderInputPacking::UniformsHLSL |
            DrawShaderInputPacking::UniformsMetal => {
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
    pub ty: ShaderTy
}

#[derive(Clone)]
pub struct CxDrawShaderMapping {
    pub flags: DrawShaderFlags,
    pub const_table: DrawShaderConstTable,
    
    pub geometries: DrawShaderInputs,
    pub instances: DrawShaderInputs,
    pub var_instances: DrawShaderInputs,
    pub live_instances: DrawShaderInputs,
    pub live_uniforms: DrawShaderInputs,
    pub user_uniforms: DrawShaderInputs,
    pub draw_uniforms: DrawShaderInputs,
    pub view_uniforms: DrawShaderInputs,
    pub pass_uniforms: DrawShaderInputs,
    pub textures: Vec<DrawShaderTextureInput>,
    pub uses_time: bool,
    pub instance_enums: Vec<usize>,
    pub rect_pos: Option<usize>,
    pub rect_size: Option<usize>,
    pub draw_clip: Option<usize>,
    pub live_uniforms_buf: Vec<f32>,
}

impl CxDrawShaderMapping {
    
    pub fn from_draw_shader_def(draw_shader_def: &DrawShaderDef, const_table: DrawShaderConstTable, uniform_packing: DrawShaderInputPacking) -> CxDrawShaderMapping { //}, options: ShaderCompileOptions, metal_uniform_packing:bool) -> Self {
        
        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut var_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut live_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut user_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut live_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut draw_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut view_uniforms = DrawShaderInputs::new(uniform_packing);
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
                        live_id!(draw) => {
                            draw_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(view) => {
                            view_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(pass) => {
                            pass_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(user) => {
                            user_uniforms.push(field.ident.0, ty, None);
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
        user_uniforms.finalize();
        live_uniforms.finalize();
        draw_uniforms.finalize();
        view_uniforms.finalize();
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
            user_uniforms,
            live_uniforms,
            draw_uniforms,
            view_uniforms,
            pass_uniforms,
            instance_enums,
            textures,
            rect_pos,
            rect_size,
            draw_clip,
        }
    }
    
    pub fn update_live_and_user_uniforms(&mut self, cx: &mut Cx, apply: &mut Apply) {
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
    }
}
