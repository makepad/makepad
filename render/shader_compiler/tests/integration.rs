const SOURCE: &'static str = r#"
    DrawQuad: DrawShader {
        uniform uni1: float
        
        rust_type: {{0}},
        
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
            let x = vec3(1.0) * self.dmat;
            x.struct_1_set();
            return #f;
        }
        
        fn override(self) {}
        
        fn vertex(self) -> vec4 {
            let x = Struct2 {c: self.uni1 + cnst1 + live};
            x.struct_2_set(3.0);
            let x = vec3(1.0) * self.dmat;
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
    // Uniform block user
    uniform float ds_uni1;
    uniform float ds_duni;
    attribute vec4 packed_instance_0;
    attribute vec4 packed_instance_1;
    attribute vec2 packed_instance_2;
    varying vec4 packed_varying_0;
    varying vec4 packed_varying_1;
    varying vec2 packed_varying_2;
    float ds_dinst = 0.0;
    mat3 ds_dmat = mat3(0.0);
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
        vec3 var_x_1 = (vec3(1.0) * ds_dmat);
        fn_0_1_16_override ();
        return vec4(1.0, 1.0, 1.0, 1.0);
    }
    void main() {
        ds_dinst = packed_instance_0.x;
        ds_dmat[0][0] = packed_instance_0.y;
        ds_dmat[0][1] = packed_instance_0.z;
        ds_dmat[0][2] = packed_instance_0.w;
        ds_dmat[0][3] = packed_instance_1.x;
        ds_dmat[1][0] = packed_instance_1.y;
        ds_dmat[1][1] = packed_instance_1.z;
        ds_dmat[1][2] = packed_instance_1.w;
        ds_dmat[1][3] = packed_instance_2.x;
        ds_dmat[2][0] = packed_instance_2.y;
        gl_Position = fn_0_1_17_vertex();
        packed_varying_0.x = dinst;
        packed_varying_0.y = dmat[0][0];
        packed_varying_0.z = dmat[0][1];
        packed_varying_0.w = dmat[0][2];
        packed_varying_1.x = dmat[0][3];
        packed_varying_1.y = dmat[1][0];
        packed_varying_1.z = dmat[1][1];
        packed_varying_1.w = dmat[1][2];
        packed_varying_2.x = dmat[1][3];
        packed_varying_2.y = dmat[2][0];
    }
    PIXELSHADER
    // Uniform block user
    uniform float ds_uni1;
    uniform float ds_duni;
    varying vec4 packed_varying_0;
    varying vec4 packed_varying_1;
    varying vec2 packed_varying_2;
    float ds_dinst = 0.0;
    mat3 ds_dmat = mat3(0.0);
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
        struct_0_1_5 var_x_0 = struct_0_1_5(1.0, struct_0_1_4((1.0 + ds_dinst)));
        vec3 var_x_1 = (vec3(1.0) * ds_dmat);
        fn_0_2_5_struct_1_set (var_x_1);
        return vec4(1.0, 1.0, 1.0, 1.0);
    }
    void main() {
        ds_dinst = packed_varying_0.x;
        ds_dmat[0][0] = packed_varying_0.y;
        ds_dmat[0][1] = packed_varying_0.z;
        ds_dmat[0][2] = packed_varying_0.w;
        ds_dmat[0][3] = packed_varying_1.x;
        ds_dmat[1][0] = packed_varying_1.y;
        ds_dmat[1][1] = packed_varying_1.z;
        ds_dmat[1][2] = packed_varying_1.w;
        ds_dmat[1][3] = packed_varying_2.x;
        ds_dmat[2][0] = packed_varying_2.y;
        gl_FragColor = fn_0_1_15_pixel();
    }
"#;

const METAL_OUTPUT: &'static str = r#"
    #include <metal_stdlib>
    using namespace metal;
    struct struct_0_1_4 {
        float f_c;
    };
    struct struct_0_1_5 {
        float f_a;
        struct_0_1_4;
    };
    struct LiveUniforms {
        float live_0_1_3;
    };
    struct Uniforms_user {
        float ds_uni1;
        float ds_duni;
    };
    struct Textures {
    };
    struct Geometries {
    };
    struct Instances {
        float ds_dinst;
        packed_float3 ds_dmat 0;
        packed_float3 ds_dmat 1;
        packed_float3 ds_dmat 2;
    };
    struct Varyings {
        float4 position [[position]];
        float ds_dinst;
        float3 ds_dmat0;
        float3 ds_dmat1;
        float3 ds_dmat2;
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
    float4 fn_0_1_15_pixel(thread Varyings & varyings) {
        struct_0_1_5 = struct_0_1_5(1.0, struct_0_1_4((1.0 + varyings.ds_dinst)));
        float3 var_x_1 = (float3(float3(1.0)) * float3x3(varyings.ds_dmat0.x, varyings.ds_dmat1.x, varyings.ds_dmat2.x, varyings.ds_dmat0.y, varyings.ds_dmat1.y, varyings.ds_dmat2.y, varyings.ds_dmat0.z, varyings.ds_dmat1.z, varyings.ds_dmat2.z));
        fn_0_2_5_struct_1_set (var_x_1);
        return float4(1.0, 1.0, 1.0, 1.0);
    }
    void fn_0_1_16_override() {
        (1 + 1);
    }
    float4 fn_0_1_17_vertex(thread Varyings & varyings, constant Uniforms_user & uniforms_user, constant LiveUniforms & live_uniforms) {
        struct_0_1_4 = struct_0_1_4(((uniforms_user.ds_uni1 + const_0_1_2) + live_uniforms.live_0_1_3));
        fn_0_2_1_struct_2_set (var_x_0, 3.0);
        float3 var_x_1 = (float3(float3(1.0)) * float3x3(varyings.ds_dmat0.x, varyings.ds_dmat1.x, varyings.ds_dmat2.x, varyings.ds_dmat0.y, varyings.ds_dmat1.y, varyings.ds_dmat2.y, varyings.ds_dmat0.z, varyings.ds_dmat1.z, varyings.ds_dmat2.z));
        fn_0_1_16_override ();
        return float4(1.0, 1.0, 1.0, 1.0);
    }
    vertex Varyings vertex_main(Textures textures, const device Geometries * in_geometries [[buffer(0)]], const device Instances * in_instances [[buffer(1)]], constant LiveUniforms & live_uniforms [[buffer(2)]], constant const float * const_table [[buffer(3)]], constant Uniforms_user & uniforms_user [[buffer(4)]], uint vtx_id [[vertex_id]], uint inst_id [[instance_id]]) {
        Geometries geometries = in_geometries[vtx_id];
        Instances instances = in_instances[inst_id];
        Varyings varyings;
        varyings.ds_dinst = instances.ds_dinst;
        varyings.ds_dmat0 = instances.ds_dmat0;
        varyings.ds_dmat1 = instances.ds_dmat1;
        varyings.ds_dmat2 = instances.ds_dmat2;
        varyings.position = fn_0_1_17_vertex(varyings, uniforms_user, live_uniforms);
        return varyings;
    }
    fragment float4 fragment_main(Varyings varyings[[stage_in]], Textures textures, constant LiveUniforms & live_uniforms [[buffer(2)]], constant const float * const_table [[buffer(3)]], constant Uniforms_user & uniforms_user [[buffer(4)]]) {
        return fn_0_1_15_pixel(varyings);
    }
    
"#;

const HLSL_OUTPUT: &'static str = r#"
    
    struct struct_0_1_4 {
        float f_c;
    };
    struct struct_0_1_5 {
        float f_a;
        struct_0_1_4 f_b;
    };
    cbuffer LiveUniforms: register(b0) {
        float live_0_1_3;
    };
    cbuffer ConstTable: register(b1) {float4 const_table[0];};
    cbuffer Uniforms_user: register(b2) {
        float ds_uni1;
        float ds_duni;
    };
    struct Geometries {
    };
    struct Instances {
        float ds_dinst: INSTA;
        float3 ds_dmat0: INSTB;
        float3 ds_dmat1: INSTC;
        float3 ds_dmat2: INSTD;
    };
    struct Varyings {
        float4 position: SV_POSITION;
        float ds_dinst: VARYA;
        float3 ds_dmat0: VARYB;
        float3 ds_dmat1: VARYC;
        float3 ds_dmat2: VARYD;
    };
    static const float const_0_1_2 = 1.0;
    float3 consfn_vec3_float(float x) {
        return float3(x, x, x);
    }
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
    float4 fn_0_1_15_pixel(inout Varyings varyings) {
        struct_0_1_5 var_x_0 = struct_0_1_5 {1.0, struct_0_1_4 {(1.0 + varyings.ds_dinst)}};
        float3 var_x_1 = mul(consfn_vec3_float(1.0), float3x3(varyings.ds_dmat0.x, varyings.ds_dmat1.x, varyings.ds_dmat2.x, varyings.ds_dmat0.y, varyings.ds_dmat1.y, varyings.ds_dmat2.y, varyings.ds_dmat0.z, varyings.ds_dmat1.z, varyings.ds_dmat2.z));
        fn_0_2_5_struct_1_set (var_x_1);
        return float4(1.0, 1.0, 1.0, 1.0);
    }
    void fn_0_1_16_override() {
        (1 + 1);
    }
    float4 fn_0_1_17_vertex(inout Varyings varyings,,) {
        struct_0_1_4 var_x_0 = struct_0_1_4 {((ds_uni1 + const_0_1_2) + live_0_1_3)};
        fn_0_2_1_struct_2_set (var_x_0, 3.0);
        float3 var_x_1 = mul(consfn_vec3_float(1.0), float3x3(varyings.ds_dmat0.x, varyings.ds_dmat1.x, varyings.ds_dmat2.x, varyings.ds_dmat0.y, varyings.ds_dmat1.y, varyings.ds_dmat2.y, varyings.ds_dmat0.z, varyings.ds_dmat1.z, varyings.ds_dmat2.z));
        fn_0_1_16_override ();
        return float4(1.0, 1.0, 1.0, 1.0);
    }
    Varyings vertex_main(Geometries geometries, Instances instances, uint inst_id: SV_InstanceID) {
        Varyings varyings = {float4(0.0, 0.0, 0.0, 0.0), 0.0, float3(0.0, 0.0, 0.0), float3(0.0, 0.0, 0.0), float3(0.0, 0.0, 0.0)};
        varyings.ds_dinst = instances.ds_dinst;
        varyings.ds_dmat0 = instances.ds_dmat0;
        varyings.ds_dmat1 = instances.ds_dmat1;
        varyings.ds_dmat2 = instances.ds_dmat2;
        varyings.position = fn_0_1_17_vertex(varyings,,);
        return varyings;
    }
    float4 pixel_main(Varyings varyings): SV_TARGET {
        return fn_0_1_15_pixel(varyings);
    }
    
    
"#;

use makepad_live_parser::*;
use makepad_shader_compiler::shaderregistry::ShaderRegistry;
use makepad_shader_compiler::shaderast::DrawShaderPtr;
use makepad_shader_compiler::shaderast::DrawShaderConstTable;
use makepad_shader_compiler::shaderast::Ty;
use makepad_shader_compiler::generate_glsl;
use makepad_shader_compiler::generate_hlsl;
use makepad_shader_compiler::generate_metal;
// lets just test most features in one go.

fn compare_no_ws(a: &str, b: &str) -> Option<String> {
    let mut b = b.to_string();
    b.retain( | c | !c.is_whitespace());
    let mut a = a.to_string();
    a.retain( | c | !c.is_whitespace());
    
    let b = b.as_bytes();
    let a = a.as_bytes();
    
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
    for i in 2..len {
        end = i - 2;
        if a[a.len() - i] != b[b.len() - i] {
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
            start,
            (a.len() - end - 1),
            range_a,
            start,
            (b.len() - end - 1),
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
    
    struct FakeType();
    
    let fake_typeof = LiveType(std::any::TypeId::of::<FakeType>());
    let module_path = ModulePath::from_str("test").unwrap();
    match sr.live_registry.parse_live_file("test.live", module_path, SOURCE.to_string(), vec![fake_typeof]) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    sr.live_registry.expand_all_documents(&mut errors);
    
    for msg in errors {
        println!("{}\n", msg.to_live_file_error("", SOURCE));
    }
    
    let shader_ptr = DrawShaderPtr(sr.live_registry.live_ptr_from_path(
        module_path,
        &[id!(DrawQuad2)]
    ).unwrap());
    // lets just call the shader compiler on this thing
    
    let result = sr.analyse_draw_shader(shader_ptr, | span, id, _live_type, draw_shader_def | {
        if id == id!(rust_type) {
            draw_shader_def.add_uniform(Id::from_str("duni").unwrap(), Ty::Float, span);
            draw_shader_def.add_instance(Id::from_str("dinst").unwrap(), Ty::Float, span);
            draw_shader_def.add_instance(Id::from_str("dmat").unwrap(), Ty::Mat3, span);
        }
        if id == id!(geometry) {
            
        }
    });
    
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
        }
        Ok(_) => {
            println!("OK!");
        }
    }
    
    /*
    pub fn generate_glsl_shader(&mut self, shader_ptr: DrawShaderNodePtr) -> (String, String) {
        // lets find the FullPointer
        let draw_shader_decl = self.draw_shaders.get(&shader_ptr).unwrap();
        // TODO this env needs its const table transferred
        let vertex = generate_glsl::generate_vertex_shader(draw_shader_decl, self);
        let pixel = generate_glsl::generate_pixel_shader(draw_shader_decl, self);
        return (vertex, pixel)
    }
    
    pub fn generate_metal_shader(&mut self, shader_ptr: DrawShaderNodePtr) -> String{
        // lets find the FullPointer
        let draw_shader_decl = self.draw_shaders.get(&shader_ptr).unwrap();
        let shader = generate_metal::generate_shader(draw_shader_decl, self);
        return shader
    }
    
    pub fn generate_hlsl_shader(&mut self, shader_ptr: DrawShaderNodePtr) -> String{
        // lets find the FullPointer
        let draw_shader_decl = self.draw_shaders.get(&shader_ptr).unwrap();
        let shader = generate_hlsl::generate_shader(draw_shader_decl, self);
        return shader
    }*/
    
    // ok the shader is analysed.
    // now we will generate the glsl shader.
    let draw_shader_def = sr.draw_shader_defs.get(&shader_ptr).unwrap();
    // TODO this env needs its const table transferred
    let const_table = DrawShaderConstTable::default();
    let vertex = generate_glsl::generate_vertex_shader(draw_shader_def, &const_table, &sr);
    let pixel = generate_glsl::generate_pixel_shader(draw_shader_def, &const_table, &sr);
    let compare = format!("\nVERTEXSHADER\n{}PIXELSHADER\n{}", vertex, pixel);
    if let Some(change) = compare_no_ws(GLSL_OUTPUT, &compare) {
        println!("GLSL OUTPUT CHANGED\n{}", change);
        println!("########## ALL ##########\n{}\n########## END ##########", compare);
        assert_eq!(true, false);
    }
    
    let shader = generate_metal::generate_shader(draw_shader_def, &const_table, &sr);
    let compare = format!("\n{}", shader.mtlsl);
    if let Some(change) = compare_no_ws(METAL_OUTPUT, &compare) {
        println!("METAL OUTPUT CHANGED\n{}", change);
        println!("########## ALL ##########\n{}\n########## END ##########", compare);
        assert_eq!(true, false);
    }
    
    let shader = generate_hlsl::generate_shader(draw_shader_def, &const_table, &sr);
    let compare = format!("\n{}", shader);
    if let Some(change) = compare_no_ws(HLSL_OUTPUT, &compare) {
        println!("HLSL OUTPUT CHANGED\n{}", change);
        println!("########## ALL ##########\n{}\n########## END ##########", compare);
        assert_eq!(true, false);
    }
    
}
