use crate::cx::*;
use makepad_live_parser::LiveRegistry;
use makepad_shader_compiler::Ty;
use makepad_shader_compiler::shaderast::DrawShaderDef;
use makepad_shader_compiler::shaderast::DrawShaderFieldKind;
use makepad_shader_compiler::shaderast::DrawShaderPtr;
use makepad_shader_compiler::shaderast::DrawShaderFlags;
use makepad_shader_compiler::shaderast::DrawShaderConstTable;
use std::collections::HashMap;


#[derive(Clone, Default)]
pub struct DrawShaderVarInputs{
    pub var_uniform_slots: usize,
    pub var_instance_slots: usize,
    pub total_uniform_slots: usize,
    pub total_instance_slots: usize,
    pub inputs: Vec<DrawShaderVarInput>
}

#[derive(Clone)]
pub struct DrawShaderVarInput {
    pub ident: Id,
    pub offset: usize,
    pub size: usize,
    pub kind: DrawShaderVarInputKind
}

#[derive(Clone)]
pub enum DrawShaderVarInputKind{
    Instance,
    Uniform,
}


// TODO CLEAN THIS UP, MAYBE MOVE TO DRAW_SHADER_DEF:

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DrawShader {
    pub draw_shader_id: usize,
    pub draw_shader_ptr: DrawShaderPtr
}

pub enum ShaderCompileResult{
    Nop,
    Ok
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct PropDef {
   // pub name: String,
    pub ty: Ty,
    pub id: Id,
    pub live_ptr: Option<LivePtr>
}

#[derive(Debug, Default, Clone)]
pub struct RectInstanceProps {
    pub rect_pos: Option<usize>,
    pub rect_size: Option<usize>,
}
impl RectInstanceProps {
    pub fn construct(instances: &Vec<PropDef>) -> RectInstanceProps {
        let mut rect_pos = None;
        let mut rect_size = None;
        let mut slot = 0;
        for inst in instances {
            match inst.id {
                id!(rect_pos) => rect_pos = Some(slot),
                id!(rect_size) => rect_size = Some(slot),
                _ => ()
            }
            slot += inst.ty.size(); //sg.get_type_slots(&inst.ty);
        };
        RectInstanceProps {
            rect_pos,
            rect_size
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstanceProp {
    pub id: Id,
    pub ty: Ty,
    pub offset: usize,
    pub slots: usize
}

#[derive(Debug, Default, Clone)]
pub struct InstanceProps {
    pub prop_map: HashMap<Id, usize>,
    pub props: Vec<InstanceProp>,
    pub total_slots: usize,
}

#[derive(Debug, Clone)]
pub struct UniformProp {
    pub id: Id,
    pub ty: Ty,
    pub offset: usize,
    pub slots: usize
}

#[derive(Debug, Default, Clone)]
pub struct UniformProps {
    pub prop_map: HashMap<Id, usize>,
    pub props: Vec<UniformProp>,
    pub total_slots: usize,
}

#[derive(Debug, Clone)]
pub struct NamedProp {
    pub id: Id,
    pub ty: Ty,
    pub offset: usize,
    pub slots: usize
}

#[derive(Debug, Default, Clone)]
pub struct NamedProps {
    pub props: Vec<NamedProp>,
    pub total_slots: usize,
}

impl NamedProps {
    pub fn construct(in_props: &Vec<PropDef>) -> NamedProps {
        let mut offset = 0;
        let mut out_props = Vec::new();
        for prop in in_props {
            let slots = prop.ty.size();
            out_props.push(NamedProp {
                id: prop.id,
                ty: prop.ty.clone(),
                offset: offset,
                slots: slots
            });
            offset += slots
        };
        NamedProps {
            props: out_props,
            total_slots: offset
        }
    }
}


impl InstanceProps {
    pub fn construct(in_props: &Vec<PropDef>) -> InstanceProps {
        let mut offset = 0;
        let mut out_props = Vec::new();
        let mut prop_map = HashMap::new();
        for prop in in_props {
            let slots = prop.ty.size();
            prop_map.insert(prop.id, out_props.len());
            out_props.push(InstanceProp {
                id: prop.id,
                ty: prop.ty.clone(),
                offset: offset,
                slots: slots
            });
            offset += slots
        };
        InstanceProps {
            prop_map,
            props: out_props,
            total_slots: offset
        }
    }
}

impl UniformProps {
    pub fn construct(in_props: &Vec<PropDef>, metal_uniform_packing:bool) -> UniformProps {
        let mut out_props = Vec::new();
        let mut prop_map = HashMap::new();
        let mut offset = 0;
        
        for prop in in_props {
            let slots = prop.ty.size();
            
            // metal+webgl
            let aligned_slots = if metal_uniform_packing && slots==3{4}else{slots};
            if (offset & 3) + aligned_slots > 4 { // goes over the boundary
                offset += 4 - (offset & 3); // make jump to new slot
            }
            
            prop_map.insert(prop.id, out_props.len());
            out_props.push(UniformProp {
                id: prop.id,
                ty: prop.ty.clone(),
                offset: offset,
                slots: slots
            });
            offset += aligned_slots
        };
        if offset & 3 > 0 {
            offset += 4 - (offset & 3);
        }
        UniformProps { 
            prop_map,
            props: out_props,
            total_slots: offset
        }
    }
    
    pub fn find_zbias_uniform_prop(&self) -> Option<usize> {
        for prop in &self.props {
            if prop.id == id!(zbias) {
                return Some(prop.offset)
            }
        }
        return None
    }
}

#[derive(Default, Clone)]
pub struct CxDrawShaderMapping {
    pub flags: DrawShaderFlags,
    pub var_inputs: DrawShaderVarInputs,
    pub rect_instance_props: RectInstanceProps,
    pub user_uniform_props: UniformProps,
    pub live_uniform_props: UniformProps,
    pub instance_props: InstanceProps,
    pub geometry_props: InstanceProps,
    pub textures: Vec<PropDef>,
    pub const_table: DrawShaderConstTable,
    pub geometries: Vec<PropDef>,
    pub instances: Vec<PropDef>,
    pub live_uniforms_buf: Vec<f32>, 
    pub live_uniforms: Vec<PropDef>,
    pub draw_uniforms: Vec<PropDef>,
    pub view_uniforms: Vec<PropDef>,
    pub pass_uniforms: Vec<PropDef>,
    pub user_uniforms: Vec<PropDef>
}

impl CxDrawShaderMapping {
    
    pub fn from_draw_shader_def(draw_shader_def: &DrawShaderDef, metal_uniform_packing:bool)->CxDrawShaderMapping{//}, options: ShaderCompileOptions, metal_uniform_packing:bool) -> Self {
        
        let mut instances = Vec::new();
        let mut geometries = Vec::new();
        let mut user_uniforms = Vec::new();
        let mut live_uniforms = Vec::new();
        let mut draw_uniforms = Vec::new();
        let mut view_uniforms = Vec::new();
        let mut pass_uniforms = Vec::new();
        let mut textures = Vec::new();
        
        let mut var_inputs = DrawShaderVarInputs::default();

        for field in &draw_shader_def.fields {
            match &field.kind {
                DrawShaderFieldKind::Geometry{var_def_ptr,..} => {
                    geometries.push(PropDef {
                        //name: field.ident.to_string(),
                        ty: field.ty_expr.ty.borrow().clone().unwrap(),
                        id: field.ident.0,
                        live_ptr: if let Some(l) = var_def_ptr{Some(l.0)}else{None}
                    });
                }
                DrawShaderFieldKind::Instance{var_def_ptr, ..} => {
                    if var_def_ptr.is_some(){
                        var_inputs.inputs.push(DrawShaderVarInput{
                            ident: field.ident.0,
                            offset:var_inputs.var_instance_slots,
                            size: field.ty_expr.ty.borrow().as_ref().unwrap().size(),
                            kind:DrawShaderVarInputKind::Instance
                        });
                        var_inputs.var_instance_slots += field.ty_expr.ty.borrow().as_ref().unwrap().size();
                    }
                    var_inputs.total_instance_slots += field.ty_expr.ty.borrow().as_ref().unwrap().size();

                    
                    instances.push(PropDef {
                        //name: field.ident.to_string(),
                        ty: field.ty_expr.ty.borrow().clone().unwrap(),
                        id: field.ident.0,
                        live_ptr: if let Some(l) = var_def_ptr{Some(l.0)}else{None}
                    });
                }
                DrawShaderFieldKind::Uniform{var_def_ptr,block_ident,..} => {
                    let prop_def = PropDef {
                        //name: field.ident.to_string(),
                        ty: field.ty_expr.ty.borrow().clone().unwrap(),
                        id: field.ident.0,
                        live_ptr: if let Some(l) = var_def_ptr{Some(l.0)}else{None}
                    };
                    match block_ident.0 {
                        id!(draw) => {
                            draw_uniforms.push(prop_def);
                        }
                        id!(view) => {
                            view_uniforms.push(prop_def);
                        }
                        id!(pass) => {
                            pass_uniforms.push(prop_def);
                        }
                        id!(user) => {
                            user_uniforms.push(prop_def);
                        }
                        _ => ()
                    }
                    if block_ident.0 == id!(user){ 
                        if var_def_ptr.is_some(){
                            var_inputs.inputs.push(DrawShaderVarInput{
                                ident:field.ident.0,
                                offset:var_inputs.var_uniform_slots,
                                size: field.ty_expr.ty.borrow().as_ref().unwrap().size(),
                                kind:DrawShaderVarInputKind::Uniform
                            });
                            var_inputs.var_uniform_slots += field.ty_expr.ty.borrow().as_ref().unwrap().size();
                        }
                        var_inputs.total_uniform_slots += field.ty_expr.ty.borrow().as_ref().unwrap().size();
                    }
                }
                DrawShaderFieldKind::Texture{var_def_ptr, ..} => {
                    textures.push(PropDef {
                        //name: field.ident.to_string(),
                        ty: field.ty_expr.ty.borrow().clone().unwrap(),
                        id: field.ident.0,
                        live_ptr: if let Some(l) = var_def_ptr{Some(l.0)}else{None}
                    });
                }
                _ => ()
            }
        }
        
        // ok now the live uniforms
        for (value_node_ptr, ty) in draw_shader_def.all_live_refs.borrow().iter(){
            
            live_uniforms.push(PropDef {
                ty: ty.clone(),
                id: Id(0),
                live_ptr: Some(value_node_ptr.0)
            });
            /*
            let prop_def = PropDef {
                name: {
                    let mut out = format!("mpsc_live_");
                    qualified_ident_path.write_underscored_ident(&mut out);
                    out
                },
                ty: ty.clone(),
                live_item_id: qualified_ident_path.to_live_item_id()
            };
            live_uniforms.push(prop_def)*/
        }
        
        let live_uniform_props = UniformProps::construct(&live_uniforms, metal_uniform_packing);
        let mut live_uniforms_buf = Vec::new();
        live_uniforms_buf.resize(live_uniform_props.total_slots, 0.0);
        
        CxDrawShaderMapping {
            flags: draw_shader_def.flags,
            live_uniforms_buf,
            rect_instance_props: RectInstanceProps::construct(&instances),
            user_uniform_props: UniformProps::construct(&user_uniforms, metal_uniform_packing),
            live_uniform_props: live_uniform_props,
            instance_props: InstanceProps::construct(&instances),
            geometry_props: InstanceProps::construct(&geometries),
            textures: textures,
            const_table: DrawShaderConstTable::default(),
            instances,
            var_inputs,
            geometries,
            pass_uniforms, 
            view_uniforms,
            draw_uniforms,
            live_uniforms,
            user_uniforms
        }
    }
    
    pub fn update_live_uniforms(&mut self, live_registry:&LiveRegistry) {
        // and write em into the live_uniforms buffer
        for i in 0..self.live_uniforms.len(){
            let prop = &self.live_uniform_props.props[i];
            let uni = &self.live_uniforms[i];
            match prop.ty {
                Ty::Float => { // float
                    let node = live_registry.resolve_ptr(uni.live_ptr.unwrap());
                    if let LiveValue::Float(float) = node.value{
                        let o = prop.offset;
                        self.live_uniforms_buf[o] = float as f32;
                        
                    }
                },
                Ty::Vec2 => { // float
                    let node = live_registry.resolve_ptr(uni.live_ptr.unwrap());
                    if let LiveValue::Vec2(value) = node.value{
                        let o = prop.offset;
                        self.live_uniforms_buf[o + 0] = value.x;
                        self.live_uniforms_buf[o + 1] = value.y;
                    }
                },
                Ty::Vec3 => { // float
                    let node = live_registry.resolve_ptr(uni.live_ptr.unwrap());
                    if let LiveValue::Vec3(value) = node.value{
                        let o = prop.offset;
                        self.live_uniforms_buf[o + 0] = value.x;
                        self.live_uniforms_buf[o + 1] = value.y;
                        self.live_uniforms_buf[o + 2] = value.z;
                    }
                },
                Ty::Vec4 => { // color
                    let node = live_registry.resolve_ptr(uni.live_ptr.unwrap());
                    if let LiveValue::Color(color_u32) = node.value{
                        let o = prop.offset;
                        let color = Vec4::from_u32(color_u32);
                        self.live_uniforms_buf[o + 0] = color.x;
                        self.live_uniforms_buf[o + 1] = color.y;
                        self.live_uniforms_buf[o + 2] = color.z;
                        self.live_uniforms_buf[o + 3] = color.w;
                    }
                },
                _=>panic!()
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct CxDrawShader {
    pub name: String,
    pub platform: Option<CxPlatformShader>,
    pub mapping: CxDrawShaderMapping
}