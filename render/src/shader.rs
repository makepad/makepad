use crate::cx::*;
 
#[derive(Default, Copy, Clone, PartialEq)]
pub struct Shader {
    pub shader_id: Option<(usize, usize)>,
} 

#[derive(Default, Clone)]
pub struct RectInstanceProps {
    pub x: Option<usize>,
    pub y: Option<usize>,
    pub w: Option<usize>,
    pub h: Option<usize>,
}

impl RectInstanceProps {
    pub fn construct(instances: &Vec<PropDef>) -> RectInstanceProps {
        let mut x = None;
        let mut y = None;
        let mut w = None;
        let mut h = None;
        let mut slot = 0;
        for inst in instances {
            match inst.name.as_ref() {
                "x" => x = Some(slot),
                "y" => y = Some(slot),
                "w" => w = Some(slot),
                "h" => h = Some(slot),
                _ => ()
            }
            slot += inst.prop_id.shader_ty().size();//sg.get_type_slots(&inst.ty);
        };
        RectInstanceProps {
            x: x,
            y: y,
            w: w,
            h: h
        }
    }
}

#[derive(Clone)]
pub struct InstanceProp {
    pub name: String,
    pub prop_id: PropId,
    pub offset: usize,
    pub slots: usize
}

#[derive(Default, Clone)]
pub struct InstanceProps {
    pub props: Vec<InstanceProp>,
    pub total_slots: usize,
}

#[derive(Clone)]
pub struct UniformProp {
    pub name: String,
    pub prop_id: PropId,
    pub offset: usize,
    pub slots: usize
}

#[derive(Default, Clone)]
pub struct UniformProps {
    pub props: Vec<UniformProp>,
    pub total_slots: usize,
}

#[derive(Clone)]
pub struct NamedProp {
    pub name: String,
    pub offset: usize,
    pub slots: usize
}

#[derive(Default, Clone)]
pub struct NamedProps {
    pub props: Vec<NamedProp>,
    pub total_slots: usize,
}

impl NamedProps {
    pub fn construct(in_props: &Vec<PropDef>)->NamedProps{
        let mut offset = 0;
        let mut out_props = Vec::new();
        for prop in in_props {
            let slots = prop.prop_id.shader_ty().size();
            out_props.push(NamedProp {
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
    pub fn construct(in_props: &Vec<PropDef>)->InstanceProps{
        let mut offset = 0;
        let mut out_props = Vec::new();
        for prop in in_props {
            let slots = prop.prop_id.shader_ty().size();
            out_props.push(InstanceProp {
                prop_id: prop.prop_id.clone(),
                name: prop.name.clone(),
                offset: offset,
                slots: slots
            });
            offset += slots
        };
        InstanceProps {
            props: out_props,
            total_slots: offset
        }
    }
}

impl UniformProps{
    pub fn construct(in_props: &Vec<PropDef>)->UniformProps{
        let mut out_props = Vec::new();
        let mut offset = 0;
    
        for prop in in_props {
            let slots = prop.prop_id.shader_ty().size();
            
            if (offset & 3) + slots > 4 { // goes over the boundary
                offset += 4 - (offset & 3); // make jump to new slot
            }
            if slots == 2 && (offset & 1) != 0 {
                panic!("Please re-order uniform {} to be size-2 aligned", prop.name);
            }
            out_props.push(UniformProp {
                prop_id:prop.prop_id.clone(),  
                name: prop.name.clone(),
                offset: offset,
                slots: slots
            });
            offset += slots
        };
        if offset & 3 > 0 {
            offset += 4 - (offset & 3);
        }
        UniformProps {
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
 
#[derive(Default, Clone)]
pub struct CxShaderMapping {
    pub instance_slots: usize,
    pub geometry_slots: usize,
    pub geometries: Vec<PropDef>,
    pub instances: Vec<PropDef>,
    pub draw_uniforms: Vec<PropDef>,
    pub view_uniforms: Vec<PropDef>,
    pub pass_uniforms: Vec<PropDef>,
    pub uniforms: Vec<PropDef>,
    pub texture_slots: Vec<PropDef>,
    pub rect_instance_props: RectInstanceProps,
    pub uniform_props: UniformProps,
    pub instance_props: InstanceProps,
}

#[derive(Default, Clone)]
pub struct CxShader {
    pub name: String,
    pub shader_gen: ShaderGen,
    pub platform: Option<CxPlatformShader>,
    pub mapping: CxShaderMapping
}