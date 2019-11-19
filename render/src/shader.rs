
// Shader AST typedefs

pub use shader_ast::*;
pub use crate::cx::*;
pub use crate::math::*;
pub use crate::colors::*;
use std::any::TypeId;

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct InstanceColor(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct InstanceVec4(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct InstanceVec3(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct InstanceVec2(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct InstanceFloat(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct UniformColor(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct UniformVec4(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct UniformVec3(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct UniformVec2(pub TypeId);

#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct UniformFloat(pub TypeId);

pub struct UniqueId(pub TypeId);

#[macro_export]
macro_rules!uid { 
    () => {{
        struct Unique{};
        UniqueId(std::any::TypeId::of::<Unique>()).into()
    }}
}

impl InstanceColor{
    pub fn shader_type(&self) -> String{"vec4".to_string()}
    pub fn instance_type(&self) -> InstanceType{InstanceType::Color(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Instance(self.instance_type())}
}

impl Into<InstanceColor> for UniqueId{
    fn into(self) -> InstanceColor{InstanceColor(self.0)}
}

impl InstanceVec4{
    pub fn shader_type(&self) -> String{"vec4".to_string()}
    pub fn instance_type(&self) -> InstanceType{InstanceType::Vec4(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Instance(self.instance_type())}
}

impl Into<InstanceVec4> for UniqueId{
    fn into(self) -> InstanceVec4{InstanceVec4(self.0)}
}

impl InstanceVec3{
    pub fn shader_type(&self) -> String{"vec3".to_string()}
    pub fn instance_type(&self) -> InstanceType{InstanceType::Vec3(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Instance(self.instance_type())}
}

impl Into<InstanceVec3> for UniqueId{
    fn into(self) -> InstanceVec3{InstanceVec3(self.0)}
}

impl InstanceVec2{
    pub fn shader_type(&self) -> String{"vec2".to_string()}
    pub fn instance_type(&self) -> InstanceType{InstanceType::Vec2(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Instance(self.instance_type())}
}

impl Into<InstanceVec2> for UniqueId{
    fn into(self) -> InstanceVec2{InstanceVec2(self.0)}
}

impl InstanceFloat{
    pub fn shader_type(&self) -> String{"float".to_string()}
    pub fn instance_type(&self) -> InstanceType{InstanceType::Float(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Instance(self.instance_type())}
}

impl Into<InstanceFloat> for UniqueId{
    fn into(self) -> InstanceFloat{InstanceFloat(self.0)}
}

impl UniformColor{
    pub fn shader_type(&self) -> String{"vec4".to_string()}
    pub fn uniform_type(&self) -> UniformType{UniformType::Color(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Uniform(self.uniform_type())}
}

impl Into<UniformColor> for UniqueId{
    fn into(self) -> UniformColor{UniformColor(self.0)}
}

impl UniformVec4{
    pub fn shader_type(&self) -> String{"vec4".to_string()}
    pub fn uniform_type(&self) -> UniformType{UniformType::Vec4(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Uniform(self.uniform_type())}
}

impl Into<UniformVec4> for UniqueId{
    fn into(self) -> UniformVec4{UniformVec4(self.0)}
}

impl UniformVec3{
    pub fn shader_type(&self) -> String{"vec3".to_string()}
    pub fn uniform_type(&self) -> UniformType{UniformType::Vec3(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Uniform(self.uniform_type())}
}

impl Into<UniformVec3> for UniqueId{
    fn into(self) -> UniformVec3{UniformVec3(self.0)}
}

impl UniformVec2{
    pub fn shader_type(&self) -> String{"vec2".to_string()}
    pub fn uniform_type(&self) -> UniformType{UniformType::Vec2(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Uniform(self.uniform_type())}
}

impl Into<UniformVec2> for UniqueId{
    fn into(self) -> UniformVec2{UniformVec2(self.0)}
}

impl UniformFloat{
    pub fn shader_type(&self) -> String{"float".to_string()}
    pub fn uniform_type(&self) -> UniformType{UniformType::Float(*self)}
    pub fn var_store(&self) -> ShVarStore{ShVarStore::Uniform(self.uniform_type())}
}

impl Into<UniformFloat> for UniqueId{
    fn into(self) -> UniformFloat{UniformFloat(self.0)}
}

#[derive(Default, Clone, PartialEq)]
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
    pub fn construct(sg: &ShaderGen, instances: &Vec<ShVar>) -> RectInstanceProps {
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
            slot += sg.get_type_slots(&inst.ty);
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
    pub ident: InstanceType,
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
    pub ident: UniformType,
    pub offset: usize,
    pub slots: usize
}

#[derive(Default, Clone)]
pub struct UniformProps {
    pub props: Vec<UniformProp>,
    pub total_slots: usize,
}

impl InstanceProps {
    pub fn construct(sg: &ShaderGen, in_props: &Vec<ShVar>)->InstanceProps{
        let mut offset = 0;
        let mut out_props = Vec::new();
        for prop in in_props {
            let slots = sg.get_type_slots(&prop.ty);
            match &prop.store{
                ShVarStore::Instance(t)=>{
                    out_props.push(InstanceProp {
                        ident: t.clone(),
                        name: prop.name.clone(),
                        offset: offset,
                        slots: slots
                    })
                },
                _=>panic!("Non instance in props")
            }
            offset += slots
        };
        InstanceProps {
            props: out_props,
            total_slots: offset
        }
    }
}

impl UniformProps{
    pub fn construct(sg: &ShaderGen, in_props: &Vec<ShVar>)->UniformProps{
        let mut out_props = Vec::new();
        let mut offset = 0;
    
        for prop in in_props {
            let slots = sg.get_type_slots(&prop.ty);
            
            if (offset & 3) + slots > 4 { // goes over the boundary
                offset += 4 - (offset & 3); // make jump to new slot
            }
            if slots == 2 && (offset & 1) != 0 {
                panic!("Please re-order uniform {} to be size-2 aligned", prop.name);
            }
            match &prop.store{
                ShVarStore::Uniform(t)=>{
                    out_props.push(UniformProp {
                        ident:t.clone(),  
                        name: prop.name.clone(),
                        offset: offset,
                        slots: slots
                    })
                },
                _=>panic!("Non uniform in props")
            }
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
    pub geometries: Vec<ShVar>,
    pub instances: Vec<ShVar>,
    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_vw: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots: Vec<ShVar>,
    pub rect_instance_props: RectInstanceProps,
    pub uniform_props: UniformProps,
    pub instance_props: InstanceProps,
    pub zbias_uniform_prop: Option<usize>
}

#[derive(Default, Clone)]
pub struct CxShader {
    pub name: String,
    pub shader_gen: ShaderGen,
    pub platform: Option<CxPlatformShader>,
    pub mapping: CxShaderMapping
}

impl CxShader {
    
    pub fn def_builtins(sg: ShaderGen) -> ShaderGen {
        sg.compose(
            ShAst {
                types: vec![
                    ShType {name: "float".to_string(), slots: 1, prim: true, fields: Vec::new()},
                    ShType {name: "int".to_string(), slots: 1, prim: true, fields: Vec::new()},
                    ShType {name: "bool".to_string(), slots: 1, prim: true, fields: Vec::new()},
                    ShType {
                        name: "vec2".to_string(),
                        slots: 2,
                        prim: true,
                        fields: vec![ShTypeField::new("x", "float"), ShTypeField::new("y", "float")]
                    },
                    ShType {
                        name: "vec3".to_string(),
                        slots: 3,
                        prim: true,
                        fields: vec![ShTypeField::new("x", "float"), ShTypeField::new("y", "float"), ShTypeField::new("z", "float")]
                    },
                    ShType {
                        name: "vec4".to_string(),
                        slots: 4,
                        prim: true,
                        fields: vec![ShTypeField::new("x", "float"), ShTypeField::new("y", "float"), ShTypeField::new("z", "float"), ShTypeField::new("w", "float")]
                    },
                    ShType {
                        name: "mat2".to_string(),
                        slots: 4,
                        prim: true,
                        fields: vec![
                            ShTypeField::new("a", "float"),
                            ShTypeField::new("b", "float"),
                            ShTypeField::new("c", "float"),
                            ShTypeField::new("d", "float")
                        ]
                    },
                    ShType {
                        name: "mat3".to_string(),
                        slots: 9,
                        prim: true,
                        fields: vec![
                            ShTypeField::new("a", "float"),
                            ShTypeField::new("b", "float"),
                            ShTypeField::new("c", "float"),
                            ShTypeField::new("d", "float"),
                            ShTypeField::new("e", "float"),
                            ShTypeField::new("f", "float"),
                            ShTypeField::new("g", "float"),
                            ShTypeField::new("h", "float"),
                            ShTypeField::new("i", "float")
                        ]
                    },
                    ShType {
                        name: "mat4".to_string(),
                        slots: 16,
                        prim: true,
                        fields: vec![
                            ShTypeField::new("a", "float"),
                            ShTypeField::new("b", "float"),
                            ShTypeField::new("c", "float"),
                            ShTypeField::new("d", "float"),
                            ShTypeField::new("e", "float"),
                            ShTypeField::new("f", "float"),
                            ShTypeField::new("g", "float"),
                            ShTypeField::new("h", "float"),
                            ShTypeField::new("i", "float"),
                            ShTypeField::new("j", "float"),
                            ShTypeField::new("k", "float"),
                            ShTypeField::new("l", "float"),
                            ShTypeField::new("m", "float"),
                            ShTypeField::new("n", "float"),
                            ShTypeField::new("o", "float"),
                            ShTypeField::new("p", "float")
                        ]
                    },
                ],
                vars: Vec::new(),
                fns: vec![
                    // shorthand typed:
                    // T - generic
                    // O - optional
                    // F - float-like (float, vec2, vec3, etc)
                    // B - bool-vector (bvecn)
                    
                    ShFn {name: "sizeof".to_string(), args: vec![ShFnArg::new("type", "T")], ret: "int".to_string(), block: None},
                    ShFn {name: "color".to_string(), args: vec![ShFnArg::new("color", "string")], ret: "vec4".to_string(), block: None},
                    
                    ShFn {name: "radians".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "degrees".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "sin".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "cos".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "tan".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "asin".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "acos".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "atan".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "pow".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "exp".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "log".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "exp2".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "log2".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "sqrt".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "inversesqrt".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "abs".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "sign".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "floor".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "ceil".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "fract".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "fmod".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "min".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "max".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "clamp".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("mi", "T"), ShFnArg::new("ma", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "mix".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T"), ShFnArg::new("t", "F")], ret: "T".to_string(), block: None},
                    ShFn {name: "step".to_string(), args: vec![ShFnArg::new("e", "T"), ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "smoothstep".to_string(), args: vec![ShFnArg::new("e0", "F"), ShFnArg::new("e1", "F"), ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "length".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "float".to_string(), block: None},
                    ShFn {name: "distance".to_string(), args: vec![ShFnArg::new("p0", "T"), ShFnArg::new("p1", "T")], ret: "float".to_string(), block: None},
                    ShFn {name: "dot".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "float".to_string(), block: None},
                    ShFn {name: "cross".to_string(), args: vec![ShFnArg::new("x", "vec3"), ShFnArg::new("y", "vec3")], ret: "vec3".to_string(), block: None},
                    ShFn {name: "normalize".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "faceforward".to_string(), args: vec![ShFnArg::new("n", "T"), ShFnArg::new("i", "T"), ShFnArg::new("nref", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "reflect".to_string(), args: vec![ShFnArg::new("i", "T"), ShFnArg::new("n", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "refract".to_string(), args: vec![ShFnArg::new("i", "T"), ShFnArg::new("n", "T"), ShFnArg::new("eta", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "matrix_comp_mult".to_string(), args: vec![ShFnArg::new("a", "mat4"), ShFnArg::new("b", "mat4")], ret: "mat4".to_string(), block: None},
                    
                    ShFn {name: "less_than".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "B".to_string(), block: None},
                    ShFn {name: "less_than_equal".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "B".to_string(), block: None},
                    ShFn {name: "greater_than".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "B".to_string(), block: None},
                    ShFn {name: "greater_than_equal".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "B".to_string(), block: None},
                    ShFn {name: "equal".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "B".to_string(), block: None},
                    ShFn {name: "not_equal".to_string(), args: vec![ShFnArg::new("x", "T"), ShFnArg::new("y", "T")], ret: "B".to_string(), block: None},
                    
                    ShFn {name: "any".to_string(), args: vec![ShFnArg::new("x", "B")], ret: "bool".to_string(), block: None},
                    ShFn {name: "all".to_string(), args: vec![ShFnArg::new("x", "B")], ret: "bool".to_string(), block: None},
                    ShFn {name: "not".to_string(), args: vec![ShFnArg::new("x", "B")], ret: "B".to_string(), block: None},
                    
                    ShFn {name: "dfdx".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "dfdy".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    ShFn {name: "fwidth".to_string(), args: vec![ShFnArg::new("x", "T")], ret: "T".to_string(), block: None},
                    
                    ShFn {name: "sample2d".to_string(), args: vec![ShFnArg::new("texture", "texture2d"), ShFnArg::new("coord", "vec2")], ret: "vec4".to_string(), block: None},
                    /*
                    ShFn{name:"texture2DLod".to_string(), args:vec![ShFnArg::new("sampler","sampler2D"), ShFnArg::new("coord","vec2"), ShFnArg::new("lod","float")], ret:"vec4".to_string(), block:None},
                    ShFn{name:"texture2DProjLod".to_string(), args:vec![ShFnArg::new("sampler","sampler2D"), ShFnArg::new("coord","vec2"), ShFnArg::new("lod","float")], ret:"vec4".to_string(), block:None},
                    ShFn{name:"textureCubeLod".to_string(), args:vec![ShFnArg::new("sampler","samplerCube"), ShFnArg::new("coord","vec3"), ShFnArg::new("lod","float")], ret:"vec4".to_string(), block:None},
                    ShFn{name:"texture2DProj".to_string(), args:vec![ShFnArg::new("sampler","sampler2D"), ShFnArg::new("coord","vec2"), ShFnArg::new("bias","O")], ret:"vec4".to_string(), block:None},
                    ShFn{name:"textureCube".to_string(), args:vec![ShFnArg::new("sampler","samplerCube"), ShFnArg::new("coord","vec3"), ShFnArg::new("bias","O")], ret:"vec4".to_string(), block:None},
                    */
                ],
                consts: Vec::new()
            }
        )
    }
    
    pub fn def_df(sg: ShaderGen) -> ShaderGen {
        sg.compose(shader_ast!({
            const PI: float = 3.141592653589793;
            const E: float = 2.718281828459045;
            const LN2: float = 0.6931471805599453;
            const LN10: float = 2.302585092994046;
            const LOG2E: float = 1.4426950408889634;
            const LOG10E: float = 0.4342944819032518;
            const SQRT1_2: float = 0.70710678118654757;
            const TORAD: float = 0.017453292519943295;
            const GOLDEN: float = 1.618033988749895;
            
            let df_pos: vec2<Local>;
            let df_result: vec4<Local>;
            let df_last_pos: vec2<Local>;
            let df_start_pos: vec2<Local>;
            let df_shape: float<Local>;
            let df_clip: float<Local>;
            let df_has_clip: float<Local>;
            let df_old_shape: float<Local>;
            let df_blur: float<Local>;
            let df_aa: float<Local>;
            let df_scale: float<Local>;
            let df_field: float<Local>;
            
            fn df_iq_pal(t: float, a: vec3, b: vec3, c: vec3, d: vec3) -> vec3 {
                return a + b * cos(6.28318 * (c * t + d));
            }
            
            fn df_iq_pal0(t: float) -> vec3 {
                return mix(vec3(0., 0., 0.), vec3(1., 1., 1.), cos(t * PI) * 0.5 + 0.5)
            }
            
            fn df_iq_pal1(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0., 0.33, 0.67));
            }
            
            fn df_iq_pal2(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0., 0.1, 0.2));
            }
            
            fn df_iq_pal3(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0.3, 0.2, 0.2));
            }
            
            fn df_iq_pal4(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 0.5), vec3(0.8, 0.9, 0.3));
            }
            
            fn df_iq_pal5(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 0.7, 0.4), vec3(0, 0.15, 0.20));
            }
            
            fn df_iq_pal6(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(2., 1.0, 0.), vec3(0.5, 0.2, 0.25));
            }
            
            fn df_iq_pal7(t: float) -> vec3 {
                return df_iq_pal(t, vec3(0.8, 0.5, 0.4), vec3(0.2, 0.4, 0.2), vec3(2., 1.0, 1.0), vec3(0., 0.25, 0.25));
            }
            
            fn df_viewport(pos: vec2) -> vec2 {
                df_pos = pos;
                df_result = vec4(0., 0., 0., 0.);
                df_old_shape =
                df_shape = 1e+20;
                df_clip = -1e+20;
                df_blur = 0.00001;
                df_aa = df_antialias(pos);
                df_scale = 1.0;
                df_field = 0.0;
                df_clip = 0.0;
                df_has_clip = 0.0;
                return df_pos;
            }
            
            fn df_antialias(p: vec2) -> float {
                return 1.0 / length(vec2(length(dfdx(p)), length(dfdy(p))));
            }
            
            fn df_translate(x: float, y: float) -> vec2 {
                df_pos -= vec2(x, y);
                return df_pos;
            }
            
            fn df_rotate(a: float, x: float, y: float) {
                let ca: float = cos(-a);
                let sa: float = sin(-a);
                let p: vec2 = df_pos - vec2(x, y);
                df_pos = vec2(p.x * ca - p.y * sa, p.x * sa + p.y * ca) + vec2(x, y);
            }
            
            fn df_scale(f: float, x: float, y: float) {
                df_scale *= f;
                df_pos = (df_pos - vec2(x, y)) * f + vec2(x, y);
            }
            
            fn df_clear(color: vec4) {
                df_result = vec4(color.rgb * color.a + df_result.rgb * (1.0 - color.a), color.a);
            }
            
            fn df_calc_blur(w: float) -> float {
                let wa: float = clamp(-w * df_aa, 0.0, 1.0);
                let wb: float = 1.0;
                if df_blur > 0.001 {
                    wb = clamp(-w / df_blur, 0.0, 1.0)
                }
                return wa * wb;
            }
            
            fn df_fill_keep(color: vec4) -> vec4 {
                let f: float = df_calc_blur(df_shape);
                let source: vec4 = vec4(color.rgb * color.a, color.a);
                df_result = source * f + df_result * (1. - source.a * f);
                if df_has_clip > 0.5 {
                    let f2: float = 1. - df_calc_blur(-df_clip);
                    df_result = source * f2 + df_result * (1. - source.a * f2);
                }
                return df_result;
            }
            
            fn df_fill(color: vec4) -> vec4 {
                df_fill_keep(color);
                df_old_shape = df_shape = 1e+20;
                df_clip = -1e+20;
                df_has_clip = 0.;
                return df_result;
            }
            
            fn df_stroke_keep(color: vec4, width: float) -> vec4 {
                let f: float = df_calc_blur(abs(df_shape) - width / df_scale);
                let source: vec4 = vec4(color.rgb * color.a, color.a);
                let dest: vec4 = df_result;
                df_result = source * f + dest * (1.0 - source.a * f);
                return df_result;
            }
            
            fn df_stroke(color: vec4, width: float) -> vec4 {
                df_stroke_keep(color, width);
                df_old_shape = df_shape = 1e+20;
                df_clip = -1e+20;
                df_has_clip = 0.;
                return df_result;
            }
            
            fn df_glow_keep(color: vec4, width: float) -> vec4 {
                let f: float = df_calc_blur(abs(df_shape) - width / df_scale);
                let source: vec4 = vec4(color.rgb * color.a, color.a);
                let dest: vec4 = df_result;
                df_result = vec4(source.rgb * f, 0.) + dest;
                return df_result;
            }
            
            fn df_glow(color: vec4, width: float) -> vec4 {
                df_glow_keep(color, width);
                df_old_shape = df_shape = 1e+20;
                df_clip = -1e+20;
                df_has_clip = 0.;
                return df_result;
            }
            
            fn df_union() {
                df_old_shape = df_shape = min(df_field, df_old_shape);
            }
            
            fn df_intersect() {
                df_old_shape = df_shape = max(df_field, df_old_shape);
            }
            
            fn df_subtract() {
                df_old_shape = df_shape = max(-df_field, df_old_shape);
            }
            
            fn df_gloop(k: float) {
                let h: float = clamp(0.5 + 0.5 * (df_old_shape - df_field) / k, 0.0, 1.0);
                df_old_shape = df_shape = mix(df_old_shape, df_field, h) - k * h * (1.0 - h);
            }
            
            fn df_blend(k: float) {
                df_old_shape = df_shape = mix(df_old_shape, df_field, k);
            }
            
            fn df_circle(x: float, y: float, r: float) {
                let c: vec2 = df_pos - vec2(x, y);
                df_field = (length(c.xy) - r) / df_scale;
                df_old_shape = df_shape;
                df_shape = min(df_shape, df_field);
            }
            
            fn df_box(x: float, y: float, w: float, h: float, r: float) {
                let p: vec2 = df_pos - vec2(x, y);
                let size: vec2 = vec2(0.5 * w, 0.5 * h);
                let bp: vec2 = max(abs(p - size.xy) - (size.xy - vec2(2. * r, 2. * r).xy), vec2(0., 0.));
                df_field = (length(bp) - 2. * r) / df_scale;
                df_old_shape = df_shape;
                df_shape = min(df_shape, df_field);
            }
            
            fn df_rect(x: float, y: float, w: float, h: float) {
                let s: vec2 = vec2(w, h) * 0.5;
                let d: vec2 = abs(vec2(x, y) - df_pos + s) - s;
                let dm: vec2 = min(d, vec2(0., 0.));
                df_field = max(dm.x, dm.y) + length(max(d, vec2(0., 0.)));
                df_old_shape = df_shape;
                df_shape = min(df_shape, df_field);
            }
            
            fn df_move_to(x: float, y: float) {
                df_last_pos =
                df_start_pos = vec2(x, y);
            }
            
            fn df_line_to(x: float, y: float) {
                let p: vec2 = vec2(x, y);
                
                let pa: vec2 = df_pos - df_last_pos;
                let ba: vec2 = p - df_last_pos;
                let h: float = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
                let s: float = sign(pa.x * ba.y - pa.y * ba.x);
                df_field = length(pa - ba * h) / df_scale;
                df_old_shape = df_shape;
                df_shape = min(df_shape, df_field);
                df_clip = max(df_clip, df_field * s);
                df_has_clip = 1.0;
                df_last_pos = p;
            }
            
            fn df_close_path() {
                df_line_to(df_start_pos.x, df_start_pos.y);
            }
            
            fn df_hsv2rgb(c: vec4) -> vec4 { //http://gamedev.stackexchange.com/questions/59797/glsl-shader-change-hue-saturation-brightness
                let K: vec4 = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
                let p: vec4 = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
                return vec4(c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y), c.w);
            }
            
            fn df_rgb2hsv(c: vec4) -> vec4 {
                let K: vec4 = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
                let p: vec4 = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
                let q: vec4 = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
                
                let d: float = q.x - min(q.w, q.y);
                let e: float = 1.0e-10;
                return vec4(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x, c.w);
            }
        }))
    }
}