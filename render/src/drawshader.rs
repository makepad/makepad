use crate::cx::*;
use makepad_live_parser::LiveRegistry;
use makepad_live_parser::Span;
use makepad_shader_compiler::Ty;
use makepad_shader_compiler::shaderast::DrawShaderDef;
use makepad_shader_compiler::shaderast::DrawShaderFieldKind;
use makepad_shader_compiler::shaderast::DrawShaderFlags;
use makepad_shader_compiler::shaderast::DrawShaderConstTable;
use makepad_shader_compiler::shaderast::ValuePtr;
use makepad_shader_compiler::shaderast::DrawShaderPtr;
use std::collections::HashMap;

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DrawShader {
    pub draw_shader_id: usize,
    pub draw_shader_ptr: DrawShaderPtr
}

pub enum ShaderCompileResult {
    Nop,
    Ok
}

#[derive(Clone)]
pub struct DrawShaderInput {
    pub id: Id,
    pub ty: Ty,
    pub offset: usize,
    pub slots: usize,
    pub value_ptr: Option<ValuePtr>
}

#[derive(Clone, Copy)]
pub enum DrawShaderInputPacking {
    Attribute,
    UniformsGLSL,
    UniformsHLSL,
    UniformsMetal
}

#[derive(Clone)]
pub struct DrawShaderInputs {
    pub inputs: Vec<DrawShaderInput>,
    pub packing_method: DrawShaderInputPacking,
    pub total_slots: usize,
}

pub const DRAW_CALL_USER_UNIFORMS: usize = 32;
pub const DRAW_CALL_TEXTURE_SLOTS: usize = 16;
pub const DRAW_CALL_VAR_INSTANCES: usize = 32;

#[cfg(any(target_os = "linux", target_arch = "wasm32", test))]
pub const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformGLSL;
#[cfg(any(target_os = "macos", test))]
pub const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsMetal;
#[cfg(any(target_os = "windows", test))]
pub const DRAW_SHADER_INPUT_PACKING: DrawShaderInputPacking = DrawShaderInputPacking::UniformsHLSL;


#[derive(Default)]
pub struct DrawCallVars {
    pub var_instance_start: usize,
    pub var_instance_slots: usize,
    pub draw_shader: Option<DrawShader>,
    pub geometry: Option<Geometry>,
    pub user_uniforms: [f32; DRAW_CALL_USER_UNIFORMS],
    pub texture_slots: [Option<Texture>; DRAW_CALL_TEXTURE_SLOTS],
    pub var_instances: [f32; DRAW_CALL_VAR_INSTANCES]
}

impl DrawCallVars {
    
    pub fn live_type()->LiveType{
        LiveType(std::any::TypeId::of::<DrawCallVars>())    
    }
    
    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts((&self.var_instances[self.var_instance_start - 1] as *const _ as *const f32).offset(1), self.var_instance_slots)
        }
    }
    
    pub fn init_shader(&mut self, cx: &mut Cx, draw_shader_ptr: DrawShaderPtr, geometry_fields: &dyn GeometryFields) {
        if self.draw_shader.is_some(){
            return
        }

        if let Some(draw_shader_id) = cx.draw_shader_ptr_to_id.get(&draw_shader_ptr) {
            self.draw_shader = Some(DrawShader {
                draw_shader_ptr,
                draw_shader_id: *draw_shader_id
            });
        }
        else {
            fn live_type_to_shader_ty(live_type: LiveType) -> Option<Ty> {
                if live_type == f32::live_type() {Some(Ty::Float)}
                else if live_type == Vec2::live_type() {Some(Ty::Vec2)}
                else if live_type == Vec3::live_type() {Some(Ty::Vec3)}
                else if live_type == Vec4::live_type() {Some(Ty::Vec4)}
                else {None}
            }
            // ok ! we have to compile it
            let live_factories = &cx.live_factories;
            let result = cx.shader_registry.analyse_draw_shader(draw_shader_ptr, | span, id, live_type, draw_shader_def | {
                if id == id!(rust_type) {
                    fn recur_expand(after_draw_call_vars:&mut bool, live_type: LiveType, live_factories: &HashMap<LiveType, Box<dyn LiveFactory >>, draw_shader_def: &mut DrawShaderDef, span: Span) {
                        if let Some(lf) = live_factories.get(&live_type) {
                            
                            let mut fields = Vec::new();
                            
                            lf.live_fields(&mut fields);
                            let mut slots = 0;
                            for field in fields {
                                if field.id == id!(deref_target) {
                                    recur_expand(after_draw_call_vars, field.live_type.unwrap(), live_factories, draw_shader_def, span);
                                    continue
                                }
                                if field.id == id!(draw_call_vars){
                                    // assert the thing to be marked correctly
                                    if let LiveFieldKind::Local = field.kind{}
                                    else{panic!()}
                                    if field.live_type.unwrap() != DrawCallVars::live_type(){panic!();}
                                    
                                    *after_draw_call_vars = true;
                                    continue;
                                }
                                if *after_draw_call_vars{
                                    // lets count sizes
                                    let ty = live_type_to_shader_ty(field.live_type.unwrap()).expect("Please only put shader instance fields after draw_call_vars");
                                    slots += ty.slots();
                                    draw_shader_def.add_instance(field.id, ty, span);
                                }
                            }
                            // insert padding
                            if slots%2 == 1{
                                draw_shader_def.add_instance(Id(0), Ty::Float, span);
                            }
                        }
                    }
                    recur_expand(&mut false, live_type, live_factories, draw_shader_def, span);
                }
                if id == id!(geometry) {
                    if live_type == geometry_fields.live_type_check() {
                        let mut fields = Vec::new();
                        geometry_fields.geometry_fields(&mut fields);
                        for field in fields {
                            draw_shader_def.add_geometry(field.id, field.ty, span);
                        }
                    }
                    else {
                        eprintln!("lf.get_type() != geometry_fields.live_type_check()");
                    }
                }
            });
            // ok lets print an error
            match result {
                Err(e) => {
                    // ok so. lets get the source for this file id
                    let file = &cx.shader_registry.live_registry.live_files[e.span.file_id().to_index()];
                    //println!("{}", file.source);
                    println!("Error {}", e.to_live_file_error(&file.file, &file.source, file.line_offset));
                }
                Ok(draw_shader_def) => {
                    // OK! SO the shader parsed
                    let draw_shader_id = cx.draw_shaders.len();
                    
                    let const_table = DrawShaderConstTable::default();
                    //let mut const_table = cx.shader_registry.compute_const_table(&draw_shader_def, NONE)
                    
                    let mut mapping = CxDrawShaderMapping::from_draw_shader_def(
                        draw_shader_def,
                        const_table,
                        DRAW_SHADER_INPUT_PACKING
                    );
                    mapping.update_live_uniforms(&cx.shader_registry.live_registry);
                    
                    cx.draw_shaders.push(CxDrawShader {
                        name: "todo".to_string(),
                        platform: None,
                        mapping: mapping
                    });
                    // ok so. maybe we should fill the live_uniforms buffer?
                    
                    cx.draw_shader_ptr_to_id.insert(draw_shader_ptr, draw_shader_id);
                    cx.draw_shader_compile_set.insert(draw_shader_ptr);
                    // now we simply queue it somewhere somehow to compile.
                    self.draw_shader = Some(DrawShader {
                        draw_shader_id,
                        draw_shader_ptr
                    });
                    
                    self.geometry = Some(geometry_fields.get_geometry());
                    // also we should allocate it a Shader object
                }
            }
        }
    }

    pub fn apply_value(&mut self, _cx: &mut Cx, index:&mut usize, nodes:&[GenNode]) {
        GenValue::skip_value(index, nodes);
    } 
    
    pub fn update_value(&mut self, cx: &mut Cx, value_ptr: LivePtr, id: Id) {
        fn store_values(cx: &Cx, draw_shader: DrawShader, id: Id, values: &[f32], draw_call_vars: &mut DrawCallVars) {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            for input in &sh.mapping.user_uniforms.inputs {
                if input.id == id {
                    if values.len() == input.slots {
                        for i in 0..input.slots {
                            draw_call_vars.user_uniforms[input.offset + i] = values[i];
                        }
                    }
                }
            }
            for input in &sh.mapping.var_instances.inputs {
                if input.id == id {
                    if values.len() == input.slots {
                        for i in 0..input.slots {
                            let index = (draw_call_vars.var_instances.len() - sh.mapping.var_instances.total_slots) + input.offset + i;
                            draw_call_vars.var_instances[index] = values[i];
                        }
                    }
                }
            }
        }
        if let Some(draw_shader) = self.draw_shader {
            let node = cx.shader_registry.live_registry.resolve_ptr(value_ptr);
            match node.value {
                LiveValue::Int(val) => {
                    store_values(&cx, draw_shader, id, &[val as f32], self);
                }
                LiveValue::Float(val) => {
                    store_values(&cx, draw_shader, id, &[val as f32], self);
                }
                LiveValue::Color(val) => {
                    let val = Vec4::from_u32(val);
                    store_values(&cx, draw_shader, id, &[val.x, val.y, val.z, val.w], self);
                }
                LiveValue::Vec2(val) => {
                    store_values(&cx, draw_shader, id, &[val.x, val.y], self);
                }
                LiveValue::Vec3(val) => {
                    store_values(&cx, draw_shader, id, &[val.x, val.y, val.z], self);
                }
                _ => ()
            }
        }
    }
    
    pub fn init_slicer(
        &mut self,
        cx: &mut Cx,
    ) {
        if let Some(draw_shader) = self.draw_shader {
            let sh = &cx.draw_shaders[draw_shader.draw_shader_id];
            self.var_instance_start = self.var_instances.len() - sh.mapping.var_instances.total_slots;
            self.var_instance_slots = sh.mapping.instances.total_slots;
        }
    }
}

impl DrawShaderInputs {
    pub fn new(packing_method: DrawShaderInputPacking) -> Self {
        Self {
            inputs: Vec::new(),
            packing_method,
            total_slots: 0
        }
    }
    
    pub fn push(&mut self, id: Id,  ty:Ty, value_ptr:Option<ValuePtr>) {
        let slots = ty.slots();
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    ty,
                    value_ptr
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsGLSL => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    ty,
                    value_ptr
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
                    value_ptr
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
                    value_ptr
                });
                self.total_slots += aligned_slots;
            }
        }
    }
    
    pub fn finalize(&mut self){
        match self.packing_method {
             DrawShaderInputPacking::Attribute => (),
            DrawShaderInputPacking::UniformsGLSL |
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
pub struct DrawShaderTextureInput{
    id:Id,
    ty:Ty
}

#[derive(Clone)]
pub struct CxDrawShaderMapping {
    pub flags: DrawShaderFlags,
    pub const_table: DrawShaderConstTable,
    
    pub geometries: DrawShaderInputs,
    pub instances: DrawShaderInputs,
    pub var_instances: DrawShaderInputs,
    pub live_uniforms: DrawShaderInputs,
    pub user_uniforms: DrawShaderInputs,
    pub draw_uniforms: DrawShaderInputs,
    pub view_uniforms: DrawShaderInputs,
    pub pass_uniforms: DrawShaderInputs,
    pub textures: Vec<DrawShaderTextureInput>,
    pub rect_pos: Option<usize>,
    pub rect_size: Option<usize>,
    pub live_uniforms_buf: Vec<f32>
}

impl CxDrawShaderMapping {
    
    pub fn from_draw_shader_def(draw_shader_def: &DrawShaderDef, const_table: DrawShaderConstTable, uniform_packing: DrawShaderInputPacking) -> CxDrawShaderMapping { //}, options: ShaderCompileOptions, metal_uniform_packing:bool) -> Self {
        
        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut var_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut user_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut live_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut draw_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut view_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut pass_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut textures = Vec::new();
        let mut rect_pos = None;
        let mut rect_size = None;
        
        for field in &draw_shader_def.fields {
            let ty = field.ty_expr.ty.borrow().as_ref().unwrap().clone();
            match &field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    geometries.push(field.ident.0, ty, None);
                }
                DrawShaderFieldKind::Instance {var_def_ptr, ..} => {
                    if field.ident.0 == id!(rect_pos){
                        rect_pos = Some(instances.total_slots);
                    }
                    if field.ident.0 == id!(rect_size){
                        rect_size = Some(instances.total_slots);
                    }
                    if var_def_ptr.is_some() {
                        var_instances.push(field.ident.0, ty.clone(), None);
                    }
                    instances.push(field.ident.0, ty, None);
                }
                DrawShaderFieldKind::Uniform {block_ident, ..} => {
                    match block_ident.0 {
                        id!(draw) => {
                            draw_uniforms.push(field.ident.0, ty, None);
                        }
                        id!(view) => {
                            view_uniforms.push(field.ident.0, ty, None);
                        }
                        id!(pass) => {
                            pass_uniforms.push(field.ident.0, ty, None);
                        }
                        id!(user) => {
                            user_uniforms.push(field.ident.0, ty, None);
                        }
                        _ => ()
                    }
                }
                DrawShaderFieldKind::Texture {..} => {
                    textures.push(DrawShaderTextureInput {
                        ty,
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
        
        // ok now the live uniforms
        for (value_node_ptr, ty) in draw_shader_def.all_live_refs.borrow().iter() {
            live_uniforms.push(Id(0), ty.clone(), Some(*value_node_ptr));
        }
        
        CxDrawShaderMapping {
            const_table,
            flags: draw_shader_def.flags,
            geometries,
            instances,
            live_uniforms_buf: { let mut r = Vec::new(); r.resize(live_uniforms.total_slots, 0.0); r},
            var_instances,
            user_uniforms,
            live_uniforms,
            draw_uniforms,
            view_uniforms,
            pass_uniforms,
            textures,
            rect_pos,
            rect_size,
        }
    }
    
    pub fn update_live_uniforms(&mut self, live_registry: &LiveRegistry) {
        // and write em into the live_uniforms buffer
        for input in &self.live_uniforms.inputs{
            match input.slots {
                1 => { // float
                    let node = live_registry.resolve_ptr(input.value_ptr.unwrap().0);
                    if let LiveValue::Float(float) = node.value {
                        let o = input.offset;
                        self.live_uniforms_buf[o] = float as f32;
                        
                    }
                },
                2 => { // float
                    let node = live_registry.resolve_ptr(input.value_ptr.unwrap().0);
                    if let LiveValue::Vec2(value) = node.value {
                        let o = input.offset;
                        self.live_uniforms_buf[o + 0] = value.x;
                        self.live_uniforms_buf[o + 1] = value.y;
                    }
                },
                3 => { // float
                    let node = live_registry.resolve_ptr(input.value_ptr.unwrap().0);
                    if let LiveValue::Vec3(value) = node.value {
                        let o = input.offset;
                        self.live_uniforms_buf[o + 0] = value.x;
                        self.live_uniforms_buf[o + 1] = value.y;
                        self.live_uniforms_buf[o + 2] = value.z;
                    }
                },
                4 => { // color
                    let node = live_registry.resolve_ptr(input.value_ptr.unwrap().0);
                    if let LiveValue::Color(color_u32) = node.value {
                        let o = input.offset;
                        let color = Vec4::from_u32(color_u32);
                        self.live_uniforms_buf[o + 0] = color.x;
                        self.live_uniforms_buf[o + 1] = color.y;
                        self.live_uniforms_buf[o + 2] = color.z;
                        self.live_uniforms_buf[o + 3] = color.w;
                    }
                },
                _ => panic!()
            }
        }
    }
}

#[derive(Clone)]
pub struct CxDrawShader {
    pub name: String,
    pub platform: Option<CxPlatformShader>,
    pub mapping: CxDrawShaderMapping
}