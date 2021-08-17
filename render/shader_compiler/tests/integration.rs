const SOURCE: &'static str = r#"
    DrawQuad: DrawShader {
        draw_input self::DrawQuad;
        uniform uni1: float
        const cnst1: float = 1.0
        live: 1.0
        
        Struct2: Struct {
            field c: float
            fn struct_2_set(inout self, x: float) {self.c = x;}
        }
        
        Struct1: Struct {
            field a: float
            field b: Struct2
            fn struct_1_closure(inout self, c: fn(x: float) -> float) -> float {
                return c(self.a);
            }
            fn struct_1_set(inout self) {self.a = 2.0; self.b.struct_2_set(self.struct_1_closure( | x | x + 1.0));}
        }
        
        fn pixel(self) -> vec4 {
            let x = Struct1 {a: 1.0, b: Struct2 {c: 1.0 + self.dinst}};
            x.struct_1_set();
            return #f;
        }
        
        fn override(self) {}
        
        fn vertex(self) -> vec4 {
            let x = Struct2 {c: self.uni1 + cnst1 + live};
            x.struct_2_set(3.0);
            self.override();
            return #f;
        }
    }
    DrawQuad2: DrawQuad {
        fn override(self) {1 + 1;}
    }
"#;

const GLSL_OUTPUT: &'static str = r#"
VERTEXSHADER
// Uniform block default
uniform float ds_uni1;
uniform float ds_duni;
attribute float packed_instance_0;
varying float packed_varying_0;
float ds_dinst=0.0;
uniform float live_0_1_3;
struct struct_0_1_4 {
    float f_c;
};
const float const_0_1_2 = 1.0;
void fn_0_2_1_struct_2_set(inout struct_0_1_4 var_self_0, float var_x_0) {
    (var_self_0.f_c = var_x_0);
}
void fn_0_1_16_override() {
    (1 + 1);
}
vec4 fn_0_1_17_vertex() {
    struct_0_1_4 var_x_0 = struct_0_1_4(((ds_uni1 + const_0_1_2) + live_0_1_3));
    fn_0_2_1_struct_2_set (var_x_0, 3.0);
    fn_0_1_16_override ();
    return vec4(1.0, 1.0, 1.0, 1.0);
}
void main() {
    ds_dinst = packed_instance_0;
    gl_Position = fn_0_1_17_vertex();
    packed_varying_0 = dinst;
}
PIXELSHADER
// Uniform block default
uniform float ds_uni1;
uniform float ds_duni;
varying float packed_varying_0;
float ds_dinst=0.0;
uniform float live_0_1_3;
struct struct_0_1_4 {
    float f_c;
};
struct struct_0_1_5 {
    float f_a;
    struct_0_1_4 f_b;
};
float closure_0_in_fn_0_2_5(float var_x_0) {
    return (var_x_0 + 1.0);
}
float site_0_of_fn_0_2_5_struct_1_closure(inout struct_0_1_5 var_self_0) {
    return closure_0_in_fn_0_2_5(var_self_0.f_a);
}
void fn_0_2_1_struct_2_set(inout struct_0_1_4 var_self_0, float var_x_0) {
    (var_self_0.f_c = var_x_0);
}
void fn_0_2_5_struct_1_set(inout struct_0_1_5 var_self_0) {
    (var_self_0.f_a = 2.0);
    fn_0_2_1_struct_2_set (var_self_0.f_b, site_0_of_fn_0_2_5_struct_1_closure (var_self_0));
}
vec4 fn_0_1_15_pixel() {
    struct_0_1_5 var_x_0 = struct_0_1_5(1.0,struct_0_1_4((1.0 + ds_dinst)));
    fn_0_2_5_struct_1_set (var_x_0);
    return vec4(1.0, 1.0, 1.0, 1.0);
}
void main() {
    ds_dinst = packed_varying_0;
    gl_FragColor = fn_0_1_15_pixel();
}
"#;

const METAL_OUTPUT: &'static str = r#"
#include <metal_stdlib>
using namespace metal;
float4 sample2d(texture2d<float> tex, float2 pos){return tex.sample(sampler(mag_filter::linear,min_filter::linear),pos);}
struct struct_0_1_4 {
    float f_c;
};
struct struct_0_1_5 {
    float f_a;
     struct_0_1_4;
};
struct Uniforms_default {
    float ds_uni1;
    float ds_duni;
};
struct LiveUniforms {
    float live_0_1_3;
};
struct Textures {
};
struct Geometries {
};
struct Instances {
    float ds_dinst;
};
struct Varyings {
    float4 position [[position]];
    float ds_dinst;
};
constant float const_0_1_2 = 1.0;
float closure_0_in_fn_0_2_5(float var_x_0) {
    return (var_x_0 + 1.0);
}
float site_0_of_fn_0_2_5_struct_1_closure(thread & struct_0_1_5) {
    return closure_0_in_fn_0_2_5(var_self_0.f_a);
}
void fn_0_2_1_struct_2_set(thread & struct_0_1_4, float var_x_0) {
    (var_self_0.f_c = var_x_0);
}
void fn_0_2_5_struct_1_set(thread & struct_0_1_5) {
    (var_self_0.f_a = 2.0);
    fn_0_2_1_struct_2_set (var_self_0.f_b, site_0_of_fn_0_2_5_struct_1_closure (var_self_0));
}
float4 fn_0_1_15_pixel(thread Varyings &varyings) {
     struct_0_1_5 = struct_0_1_5(1.0,struct_0_1_4((1.0 + varyings.ds_dinst)));
    fn_0_2_5_struct_1_set (var_x_0);
    return float4(1.0, 1.0, 1.0, 1.0);
}
void fn_0_1_16_override() {
    (1 + 1);
}
float4 fn_0_1_17_vertex(constant Uniforms_default uniforms_default, constan LiveUniforms live_uniforms) {
     struct_0_1_4 = struct_0_1_4(((uniforms_default.ds_uni1 + const_0_1_2) + live_uniforms.live_0_1_3));
    fn_0_2_1_struct_2_set (var_x_0, 3.0);
    fn_0_1_16_override ();
    return float4(1.0, 1.0, 1.0, 1.0);
}
vertex Varyings vertex_main(Textures textures
, const device Geometries *in_geometries [[buffer(0)]]
, const device Instances *in_instances [[buffer(1)]]
, constant LiveUniforms &live_uniforms [[buffer(2)]]
, constant const float *const_table [[buffer(3)]]
, constant Uniforms_default &uniforms_default [[buffer(4)]]
, uint vtx_id [[vertex_id]]
, uint inst_id [[instance_id]]
) {
    Geometries geometries = in_geometries[vtx_id];
    Instances instances = in_instances[inst_id];
    Varyings varyings;
    varyings.dinst = instances.dinst;
    varyings.position = fn_0_1_17_vertex(uniforms_default, live_uniforms);
    return varyings;
}
fragment float4 fragment_main(Varyings varyings[[stage_in]]
, Textures textures
, constant LiveUniforms &live_uniforms [[buffer(2)]]
, constant const float *const_table [[buffer(3)]]
, constant Uniforms_default &uniforms_default [[buffer(4)]]
) {
    return     fn_0_1_15_pixel(varyings);
}
"#;

use makepad_live_parser::*;
use makepad_shader_compiler::shaderregistry::ShaderRegistry;
use makepad_shader_compiler::shaderregistry::DrawShaderInput;
use makepad_shader_compiler::shaderast::TyLit;
// lets just test most features in one go.

fn compare_no_ws(a: &str, b: &str) -> Option<String> {
    let mut b = b.as_bytes();
    let mut a = a.as_bytes();
    
    let mut start = 0;
    let mut changed = false;
    let len = b.len().min(a.len());
    for i in 0..len {
        if a[i] != b[i] {
            changed = true;
            break
        }
        start = i;
    }
    // now go from the back to i
    let mut end = 0;
    for i in 0..len {
        end = i;
        if a[a.len() - i - 1] != b[b.len() - i - 1] {
            changed = true;
            break
        }
    }
    // okaay so we have to show the changed thing
    if changed {
        let range_a = if start < (a.len() - end - 1) {std::str::from_utf8(&a[start..(a.len() - end - 1)]).unwrap()} else {""};
        let range_b = if start < (b.len() - end - 1) {std::str::from_utf8(&b[start..(b.len() - end - 1)]).unwrap()} else {""};
        Some(format!(
            "########## OLD ########## {} to {}\n{}\n########## NEW ########## {} to {}\n{}\n########## END ##########",
            start, (a.len() - end - 1),
            range_a,
            start, (b.len() - end - 1),
            range_b
        ))
    }
    else {
        None
    }
}

#[test]
fn main() {
    let mut sr = ShaderRegistry::new();
    
    match sr.live_registry.parse_live_file("test.live", id_check!(main), id_check!(test), SOURCE.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    sr.live_registry.expand_all_documents(&mut errors);
    
    //println!("{}", lr.registry.expanded[0]);
    
    for msg in errors {
        println!("{}\n", msg.to_live_file_error("", SOURCE));
    }
    
    id_check!(duni);
    id_check!(dinst);
    let mut di = DrawShaderInput::default();
    di.add_uniform("duni", TyLit::Float.to_ty_expr());
    di.add_instance("dinst", TyLit::Float.to_ty_expr());
    sr.register_draw_input("main::test", "DrawQuad", di);
    
    // lets just call the shader compiler on this thing
    let result = sr.analyse_draw_shader(id!(main), id!(test), &[id!(DrawQuad2)]);
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
        }
        Ok(_) => {
            println!("OK!");
        }
    }
    // ok the shader is analysed.
    // now we will generate the glsl shader.
    let result = sr.generate_glsl_shader(id!(main), id!(test), &[id!(DrawQuad2)], None); //Some(FileId(0)));
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
            assert_eq!(true, false);
        }
        Ok((vertex, pixel)) => {
            let compare = format!("\nVERTEXSHADER\n{}PIXELSHADER\n{}", vertex, pixel);
            if let Some(change) = compare_no_ws(GLSL_OUTPUT, &compare) {
                println!("GLSL OUTPUT CHANGED\n{}", change);
                println!("########## ALL ##########\n{}\n########## END ##########", compare);
                //assert_eq!(true, false);
            }
        }
    }
    
    let result = sr.generate_metal_shader(id!(main), id!(test), &[id!(DrawQuad2)], None); //Some(FileId(0)));
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
            assert_eq!(true, false);
        }
        Ok(shader) => {
            let compare = format!("\n{}", shader);
            if let Some(change) = compare_no_ws(METAL_OUTPUT, &compare) {
                println!("METAL OUTPUT CHANGED\n{}", change);
                println!("########## ALL ##########\n{}\n########## END ##########", compare);
                //assert_eq!(true, false);
            }
        }
    }
    
}

