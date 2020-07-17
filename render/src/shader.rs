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

impl CxShader{
    pub fn def_df(sg: ShaderGen) -> ShaderGen {
        sg.compose(shader!{"
            const PI: float = 3.141592653589793;
            const E: float = 2.718281828459045;
            const LN2: float = 0.6931471805599453;
            const LN10: float = 2.302585092994046;
            const LOG2E: float = 1.4426950408889634;
            const LOG10E: float = 0.4342944819032518;
            const SQRT1_2: float = 0.70710678118654757;
            const TORAD: float = 0.017453292519943295;
            const GOLDEN: float = 1.618033988749895;
            
            struct Df{
                pos:vec2,
                result:vec4,
                last_pos:vec2,
                start_pos:vec2,
                shape:float,
                clip:float,
                has_clip:float,
                old_shape:float,
                blur:float,
                aa:float,
                scale:float,
                field: float
            }
            /*
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
            }*/
        "})
    }    
}