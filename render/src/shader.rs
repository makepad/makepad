use crate::cx::*;
use makepad_live_parser::LiveRegistry;
use makepad_live_parser::Span;
use makepad_shader_compiler::Ty;
use makepad_shader_compiler::shaderast::DrawShaderDef;
use makepad_shader_compiler::shaderast::DrawShaderFieldKind;
use makepad_shader_compiler::shaderast::DrawShaderPtr;
use makepad_shader_compiler::shaderast::DrawShaderVarInputKind;
use makepad_shader_compiler::ShaderRegistry;
use std::collections::HashMap;

impl Cx{
    pub fn update_draw_shader_var_inputs(&self, draw_shader_ptr: DrawShaderPtr, value_ptr: LivePtr, id: Id, uniforms: &mut [f32], instances: &mut [f32]) {
        fn store_values(shader_registry: &ShaderRegistry, draw_shader_ptr: DrawShaderPtr, id: Id, values: &[f32], uniforms: &mut[f32], instances: &mut [f32]) {
            if let Some(draw_shader_def) = shader_registry.draw_shaders.get(&draw_shader_ptr) {
                let var_inputs = draw_shader_def.var_inputs.borrow();
                for input in &var_inputs.inputs {
                    if input.ident.0 == id {
                        match input.kind {
                            DrawShaderVarInputKind::Instance => {
                                if values.len() == input.size {
                                    for i in 0..input.size {
                                        let index = instances.len() - var_inputs.var_instance_slots + input.offset + i;
                                        instances[index] = values[i];
                                    }
                                }
                                else {
                                    println!("variable shader input size not correct {} {}", values.len(), input.size)
                                }
                            }
                            DrawShaderVarInputKind::Uniform => {
                                if values.len() == input.size {
                                    for i in 0..input.size {
                                        uniforms[input.offset + i] = values[i];
                                    }
                                }
                                else {
                                    println!("variable shader input size not correct {} {}", values.len(), input.size)
                                }
                            }
                        }
                    }
                }
            }
        }
        
        let node = self.shader_registry.live_registry.resolve_ptr(value_ptr);
        match node.value {
            LiveValue::Int(val) => {
                store_values(&self.shader_registry, draw_shader_ptr, id, &[val as f32], uniforms, instances);
            }
            LiveValue::Float(val) => {
                store_values(&self.shader_registry, draw_shader_ptr, id, &[val as f32], uniforms, instances);
            }
            LiveValue::Color(val) => {
                let val = Vec4::from_u32(val);
                store_values(&self.shader_registry, draw_shader_ptr, id, &[val.x, val.y, val.z, val.w], uniforms, instances);
            }
            LiveValue::Vec2(val) => {
                store_values(&self.shader_registry, draw_shader_ptr, id, &[val.x, val.y], uniforms, instances);
            }
            LiveValue::Vec3(val) => {
                store_values(&self.shader_registry, draw_shader_ptr, id, &[val.x, val.y, val.z], uniforms, instances);
            }
            _ => ()
        }
    }
    
    pub fn get_draw_shader_var_input_layout(
        &self,
        draw_shader: Option<DrawShader>,
        var_instance_start: &mut usize,
        var_instance_slots: &mut usize,
        var_instance_buffer_size: usize
    ) {
        // ALRIGHT so
        // we need to fetch a draw_shader_def
        // then we need to update the instance layout values
        if let Some(draw_shader) = draw_shader {
            if let Some(draw_shader_def) = self.shader_registry.draw_shaders.get(&draw_shader.draw_shader_ptr) {
                let var_inputs = draw_shader_def.var_inputs.borrow();
                *var_instance_start = var_instance_buffer_size - var_inputs.var_instance_slots;
                *var_instance_slots = var_inputs.total_instance_slots;
            }
        }
    }
    
    pub fn get_draw_shader_from_ptr(&mut self, draw_shader_ptr: DrawShaderPtr, geometry_fields: &dyn GeometryFields) -> Option<DrawShader> {
        // lets first fetch the shader from live_ptr
        // if it doesn't exist, we should allocate and
        if let Some(draw_shader_id) = self.draw_shader_ptr_to_id.get(&draw_shader_ptr) {
            Some(DrawShader {
                draw_shader_ptr,
                draw_shader_id: *draw_shader_id
            })
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
            let live_factories = &self.live_factories;
            let result = self.shader_registry.analyse_draw_shader(draw_shader_ptr, | span, id, live_type, draw_shader_def | {
                if id == id!(rust_type) {
                    fn recur_expand(is_instance:&mut bool, live_type:LiveType, live_factories:&HashMap<LiveType, Box<dyn LiveFactory>>, draw_shader_def:&mut DrawShaderDef, span:Span){
                        if let Some(lf) = live_factories.get(&live_type) {
                            
                            let mut fields = Vec::new();
                            
                            lf.live_fields(&mut fields);
                            
                            for field in fields {
                                if field.id == id!(geometry) {
                                    *is_instance = true;
                                    continue
                                }
                                if field.id == id!(deref_target){
                                    recur_expand(is_instance, field.live_type, live_factories, draw_shader_def, span);
                                    continue
                                }
                                if let Some(ty) = live_type_to_shader_ty(field.live_type) {
                                    if *is_instance {
                                        
                                        draw_shader_def.add_instance(field.id, ty, span);
                                    }
                                    else {
                                        
                                        draw_shader_def.add_uniform(field.id, ty, span);
                                    }
                                };
                            }
                            // when should i insert a filler float?
                        }
                    }
                    recur_expand(&mut false, live_type, live_factories, draw_shader_def, span);
                }
                if id == id!(geometry) {
                    if let Some(lf) = live_factories.get(&live_type) {
                        if lf.live_type() == geometry_fields.live_type_check() {
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
                }
            });
            // ok lets print an error
            match result {
                Err(e) => {
                    println!("Error {}", e.to_live_file_error("", ""));
                }
                Ok(draw_shader_def) => {
                    // OK! SO the shader parsed
                    let draw_shader_id = self.draw_shaders.len();
                    let mut mapping = CxDrawShaderMapping::from_draw_shader_def(draw_shader_def, true);
                    mapping.update_live_uniforms(&self.shader_registry.live_registry);
                    
                    self.draw_shaders.push(CxDrawShader {
                        name: "todo".to_string(),
                        default_geometry: Some(geometry_fields.get_geometry()),
                        platform: None,
                        mapping: mapping
                    });
                    // ok so. maybe we should fill the live_uniforms buffer?
                    
                    self.draw_shader_ptr_to_id.insert(draw_shader_ptr, draw_shader_id);
                    self.draw_shader_compile_set.insert(draw_shader_ptr);
                    // now we simply queue it somewhere somehow to compile.
                    return Some(DrawShader {
                        draw_shader_id,
                        draw_shader_ptr
                    });
                    // also we should allocate it a Shader object
                }
            }
            None
        }
    }    
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

#[derive(Debug, Default, Clone)]
pub struct CxDrawShaderMapping {
    pub draw_call_compare: bool,
    pub rect_instance_props: RectInstanceProps,
    pub user_uniform_props: UniformProps,
    pub live_uniform_props: UniformProps,
    pub instance_props: InstanceProps,
    pub geometry_props: InstanceProps,
    pub textures: Vec<PropDef>,
    pub const_table: Option<Vec<f32 >>,
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
                        id!(default) => {
                            user_uniforms.push(prop_def);
                        }
                        _ => ()
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
            live_uniforms_buf,
            rect_instance_props: RectInstanceProps::construct(&instances),
            user_uniform_props: UniformProps::construct(&user_uniforms, metal_uniform_packing),
            live_uniform_props: live_uniform_props,
            instance_props: InstanceProps::construct(&instances),
            geometry_props: InstanceProps::construct(&geometries),
            textures: textures,
            const_table: None,
            instances,
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
                Ty::Float => { // float
                    let node = live_registry.resolve_ptr(uni.live_ptr.unwrap());
                    if let LiveValue::Float(float) = node.value{
                        let o = prop.offset;
                        self.live_uniforms_buf[o] = float as f32;
                        
                    }
                },
                _=>()
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct CxDrawShader {
    pub name: String,
    pub default_geometry: Option<Geometry>,
    pub platform: Option<CxPlatformShader>,
    pub mapping: CxDrawShaderMapping
}