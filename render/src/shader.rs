use crate::cx::*;
use makepad_live_compiler::ty::Ty;
use makepad_live_compiler::ident::Ident;
use makepad_live_compiler::shaderast::{ShaderAst, Decl};
use makepad_live_compiler::analyse::ShaderCompileOptions;
use std::collections::HashMap;

pub enum ShaderCompileResult{
    Nop,
    Ok
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct PropDef {
    pub name: String,
    pub ty: Ty,
    pub live_item_id: LiveItemId,
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
            match inst.name.as_ref() {
                "rect_pos" => rect_pos = Some(slot),
                "rect_size" => rect_size = Some(slot),
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
    pub name: String,
    pub live_item_id: LiveItemId,
    pub ty: Ty,
    pub offset: usize,
    pub slots: usize
}

#[derive(Debug, Default, Clone)]
pub struct InstanceProps {
    pub prop_map: HashMap<LiveItemId, usize>,
    pub props: Vec<InstanceProp>,
    pub total_slots: usize,
}

#[derive(Debug, Clone)]
pub struct UniformProp {
    pub name: String,
    pub live_item_id: LiveItemId,
    pub ty: Ty,
    pub offset: usize,
    pub slots: usize
}

#[derive(Debug, Default, Clone)]
pub struct UniformProps {
    pub prop_map: HashMap<LiveItemId, usize>,
    pub props: Vec<UniformProp>,
    pub total_slots: usize,
}

#[derive(Debug, Clone)]
pub struct NamedProp {
    pub name: String,
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
                ty: prop.ty.clone(),
                name: prop.name.clone(),
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
            prop_map.insert(prop.live_item_id, out_props.len());
            out_props.push(InstanceProp {
                live_item_id: prop.live_item_id,
                ty: prop.ty.clone(),
                name: prop.name.clone(),
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
            
            prop_map.insert(prop.live_item_id, out_props.len());
            out_props.push(UniformProp {
                live_item_id: prop.live_item_id,
                ty: prop.ty.clone(),
                name: prop.name.clone(),
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
            if prop.name == "zbias" {
                return Some(prop.offset)
            }
        }
        return None
    }
}

#[derive(Debug, Default, Clone)]
pub struct CxShaderMapping {
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

impl CxShaderMapping {
    pub fn from_shader_ast(shader_ast: ShaderAst, options: ShaderCompileOptions, metal_uniform_packing:bool) -> Self {
        
        let mut instances = Vec::new();
        let mut geometries = Vec::new();
        let mut user_uniforms = Vec::new();
        let mut live_uniforms = Vec::new();
        let mut draw_uniforms = Vec::new();
        let mut view_uniforms = Vec::new();
        let mut pass_uniforms = Vec::new();
        let mut textures = Vec::new();
        for decl in shader_ast.decls {
            match decl {
                Decl::Geometry(decl) => {
                    let prop_def = PropDef {
                        name: decl.ident.to_string(),
                        ty: decl.ty_expr.ty.borrow().clone().unwrap(),
                        live_item_id: decl.qualified_ident_path.to_live_item_id()
                    };
                    geometries.push(prop_def);
                }
                Decl::Instance(decl) => {
                    let prop_def = PropDef {
                        name: decl.ident.to_string(),
                        ty: decl.ty_expr.ty.borrow().clone().unwrap(),
                        live_item_id: decl.qualified_ident_path.to_live_item_id()
                    };
                    instances.push(prop_def);
                }
                Decl::Uniform(decl) => {
                    let prop_def = PropDef {
                        name: decl.ident.to_string(),
                        ty: decl.ty_expr.ty.borrow().clone().unwrap(),
                        live_item_id: decl.qualified_ident_path.to_live_item_id()
                    };
                    match decl.block_ident {
                        Some(bi) if bi == Ident::new("draw") => {
                            draw_uniforms.push(prop_def);
                        }
                        Some(bi) if bi == Ident::new("view") => {
                            view_uniforms.push(prop_def);
                        }
                        Some(bi) if bi == Ident::new("pass") => {
                            pass_uniforms.push(prop_def);
                        }
                        None => {
                            user_uniforms.push(prop_def);
                        }
                        _ => ()
                    }
                }
                Decl::Texture(decl) => {
                    let prop_def = PropDef {
                        name: decl.ident.to_string(),
                        ty: decl.ty_expr.ty.borrow().clone().unwrap(),
                        live_item_id: decl.qualified_ident_path.to_live_item_id()
                    };
                    textures.push(prop_def);
                }
                _ => ()
            }
        }
        
         for (ty, qualified_ident_path) in shader_ast.livestyle_uniform_deps.borrow().as_ref().unwrap() {
            let prop_def = PropDef {
                name: {
                    let mut out = format!("mpsc_live_");
                    qualified_ident_path.write_underscored_ident(&mut out);
                    out
                },
                ty: ty.clone(),
                live_item_id: qualified_ident_path.to_live_item_id()
            };
            live_uniforms.push(prop_def)
        }
        
        
        let live_uniform_props = UniformProps::construct(&live_uniforms, metal_uniform_packing);
        let mut live_uniforms_buf = Vec::new();
        live_uniforms_buf.resize(live_uniform_props.total_slots, 0.0);
        CxShaderMapping {
            live_uniforms_buf,
            rect_instance_props: RectInstanceProps::construct(&instances),
            user_uniform_props: UniformProps::construct(&user_uniforms, metal_uniform_packing),
            live_uniform_props: live_uniform_props,
            instance_props: InstanceProps::construct(&instances),
            geometry_props: InstanceProps::construct(&geometries),
            textures: textures,
            const_table: if options.create_const_table {
                shader_ast.const_table.borrow_mut().take()
            }
            else {
                None
            },
            instances,
            geometries,
            pass_uniforms,
            view_uniforms,
            draw_uniforms,
            live_uniforms,
            user_uniforms
        }
    }
    
    pub fn update_live_uniforms(&mut self, live_styles: &LiveStyles) {
        // and write em into the live_uniforms buffer
        for prop in &self.live_uniform_props.props {
            match prop.ty {
                Ty::Vec4 => { // color or anim
                    let color = live_styles.get_vec4(prop.live_item_id, &prop.name);
                    let o = prop.offset;
                    self.live_uniforms_buf[o + 0] = color.x;
                    self.live_uniforms_buf[o + 1] = color.y;
                    self.live_uniforms_buf[o + 2] = color.z;
                    self.live_uniforms_buf[o + 3] = color.w;
                },
                Ty::Float => { // float or anim
                    let float = live_styles.get_float(prop.live_item_id, &prop.name);
                    let o = prop.offset;
                    self.live_uniforms_buf[o] = float;
                },
                _=>()
            }
        }
    }
}

#[derive(Default, Clone)]
pub struct CxShader {
    pub name: String,
    pub default_geometry: Option<Geometry>,
    pub platform: Option<CxPlatformShader>,
    pub mapping: CxShaderMapping
}