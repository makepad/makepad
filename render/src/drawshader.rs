use crate::cx::*;
use makepad_live_parser::LiveRegistry;
use makepad_shader_compiler::Ty;
use makepad_shader_compiler::shaderast::DrawShaderDef;
use makepad_shader_compiler::shaderast::DrawShaderFieldKind;
use makepad_shader_compiler::shaderast::DrawShaderFlags;
use makepad_shader_compiler::shaderast::DrawShaderConstTable;
use makepad_shader_compiler::shaderast::ValuePtr;

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

impl DrawShaderInputs {
    pub fn new(packing_method: DrawShaderInputPacking) -> Self {
        Self {
            inputs: Vec::new(),
            packing_method,
            total_slots: 0
        }
    }
    
    pub fn push(&mut self, id: Id, slots: usize, value_ptr:Option<ValuePtr>) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    value_ptr
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsGLSL => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
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
            match &field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    geometries.push(field.ident.0, field.ty_expr.ty.borrow().as_ref().unwrap().slots(), None);
                }
                DrawShaderFieldKind::Instance {var_def_ptr, ..} => {
                    if field.ident.0 == id!(rect_pos){
                        rect_pos = Some(instances.total_slots);
                    }
                    if field.ident.0 == id!(rect_size){
                        rect_size = Some(instances.total_slots);
                    }
                    if var_def_ptr.is_some() {
                        var_instances.push(field.ident.0, field.ty_expr.ty.borrow().as_ref().unwrap().slots(), None);
                    }
                    instances.push(field.ident.0, field.ty_expr.ty.borrow().as_ref().unwrap().slots(), None);
                }
                DrawShaderFieldKind::Uniform {block_ident, ..} => {
                    let slots = field.ty_expr.ty.borrow().as_ref().unwrap().slots();
                    match block_ident.0 {
                        id!(draw) => {
                            draw_uniforms.push(field.ident.0, slots, None);
                        }
                        id!(view) => {
                            view_uniforms.push(field.ident.0, slots, None);
                        }
                        id!(pass) => {
                            pass_uniforms.push(field.ident.0, slots, None);
                        }
                        id!(user) => {
                            user_uniforms.push(field.ident.0, slots, None);
                        }
                        _ => ()
                    }
                }
                DrawShaderFieldKind::Texture {..} => {
                    textures.push(DrawShaderTextureInput {
                        ty: field.ty_expr.ty.borrow().clone().unwrap(),
                        id: field.ident.0,
                    });
                }
                _ => ()
            }
        }
        
        // ok now the live uniforms
        for (value_node_ptr, ty) in draw_shader_def.all_live_refs.borrow().iter() {
            live_uniforms.push(Id(0), ty.slots(), Some(*value_node_ptr));
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