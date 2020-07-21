#![allow(unused_imports)]

use crate::cx::*;
use crate::cx_apple::*;

se makepad_shader_compiler::ast::{ShaderAst, Decl, TyExprKind};
use makepad_shader_compiler::colors::Color;
use makepad_shader_compiler::{generate};


#[derive(Clone)]
pub struct CxPlatformShader {
    pub library: id,
    pub pipeline_state: id,
    pub geom_vbuf: MetalBuffer,
    pub geom_ibuf: MetalBuffer,
}

impl PartialEq for CxPlatformShader {
    fn eq(&self, _other: &Self) -> bool {false}
}

pub enum PackType {
    Packed,
    Unpacked
}

pub struct SlErr {
    msg: String
}

impl Cx {
    
    pub fn mtl_compile_all_shaders(&mut self, metal_cx: &MetalCx) {
        for sh in &mut self.shaders {
            let mtlsh = Self::mtl_compile_shader(sh, metal_cx);
            if let Err(_err) = mtlsh {
                //panic!("Got metal shader compile error: {}", err.msg);
            }
        };
    }
    
    pub fn mtl_compile_shader(sh: &mut CxShader, metal_cx: &MetalCx) -> Result<(), SlErr> {
        let (mtlsl, mapping) = Self::mtl_assemble_shader(&sh.shader_gen) ?;
        
        let options: id = unsafe {msg_send![class!(MTLCompileOptions), new]};
        let ns_mtlsl: id = str_to_nsstring(&mtlsl);
        let mut err: id = nil;
        let library: id = unsafe {msg_send![
            metal_cx.device,
            newLibraryWithSource: ns_mtlsl
            options: options
            error: &mut err
        ]};
        
        if library == nil {
            let err_str: id = unsafe {msg_send![err, localizedDescription]};
            return Err(SlErr {msg: nsstring_to_string(err_str)})
        }
        
        sh.mapping = mapping;
        sh.platform = Some(CxPlatformShader {
            pipeline_state: unsafe {
                let vert: id = msg_send![library, newFunctionWithName: str_to_nsstring("_vertex_shader")];
                let frag: id = msg_send![library, newFunctionWithName: str_to_nsstring("_fragment_shader")];
                let rpd: id = msg_send![class!(MTLRenderPipelineDescriptor), new];
                
                let () = msg_send![rpd, setVertexFunction: vert];
                let () = msg_send![rpd, setFragmentFunction: frag];
                
                let color_attachments: id = msg_send![rpd, colorAttachments];
                
                let ca: id = msg_send![color_attachments, objectAtIndexedSubscript: 0u64];
                let () = msg_send![ca, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                let () = msg_send![ca, setBlendingEnabled: YES];
                let () = msg_send![ca, setSourceRGBBlendFactor: MTLBlendFactor::One];
                let () = msg_send![ca, setDestinationRGBBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
                let () = msg_send![ca, setSourceAlphaBlendFactor: MTLBlendFactor::One];
                let () = msg_send![ca, setDestinationAlphaBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
                let () = msg_send![ca, setRgbBlendOperation: MTLBlendOperation::Add];
                let () = msg_send![ca, setAlphaBlendOperation: MTLBlendOperation::Add];
                let () = msg_send![rpd, setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float_Stencil8];
                
                let mut err: id = nil;
                let rps: id = msg_send![
                    metal_cx.device,
                    newRenderPipelineStateWithDescriptor: rpd
                    error: &mut err
                ];
                if rps == nil {
                    panic!("Could not create render pipeline state")
                }
                rps //.expect("Could not create render pipeline state")
            },
            library: library,
            geom_ibuf: {
                let mut geom_ibuf = MetalBuffer {..Default::default()};
                geom_ibuf.update_with_u32_data(metal_cx, &sh.shader_gen.geometry_indices);
                geom_ibuf
            },
            geom_vbuf: {
                let mut geom_vbuf = MetalBuffer {..Default::default()};
                geom_vbuf.update_with_f32_data(metal_cx, &sh.shader_gen.geometry_vertices);
                geom_vbuf
            }
        });
        return Ok(());
    }
    
    pub fn mtl_assemble_shader(sg: &ShaderGen) -> Result<(String, CxShaderMapping), SlErr> {
        // ok lets parse everything. this should never give errors
        let mut shader = ShaderAst::new();
        let mut input_props = Vec::new();
        for (index,sub) in sg.subs.iter().enumerate() {
            // lets tokenize the sub
            let tokens = lex::lex(sub.code.chars(), index).collect::<Result<Vec<_>, _>>();
            if let Err(err) = &tokens {
                let start = ShaderGen::byte_to_row_col(err.span.start, &sub.code);
                println!("Shader lex error {}:{} col:{} - {}", sub.loc.file, start.0 + sub.loc.line, start.1 + 1, err);
            }
            let tokens = tokens.unwrap();
            if let Err(err) = parse::parse(&tokens, &mut shader) {
                // lets find the span info
                let start = ShaderGen::byte_to_row_col(err.span.start, &sub.code);
                println!("Shader parse error {}:{} col:{} - {}", sub.loc.file, start.0 + sub.loc.line, start.1 + 1, err);
            }
            // lets add our instance_props
            input_props.extend(sub.attribute_props.iter());
            input_props.extend(sub.instance_props.iter());
            input_props.extend(sub.uniform_props.iter());
            input_props.extend(sub.texture_props.iter());
        }
        
        // lets collect all our 
        
        // ok now we have the shader, lets analyse
        if let Err(err) = analyse::analyse(&mut shader, &input_props){
            let sub = &sg.subs[err.span.loc_id];
            let start = ShaderGen::byte_to_row_col(err.span.start, &sub.code);
            println!("Shader analyse error {}:{} col:{} - {}", sub.loc.file, start.0 + sub.loc.line, start.1 + 1, err);
        }
        //println!("{}", generate(&shader));
        
        return Err(SlErr {msg: "".to_string()});
        
        /*
        let mut mtl_out = "#include <metal_stdlib>\nusing namespace metal;\n".to_string();
        
        // ok now define samplers from our sh.
        let texture_slots = sg.flat_vars( | v | if let ShVarStore::Texture = *v {true} else {false});
        let geometries = sg.flat_vars( | v | if let ShVarStore::Geometry = *v {true} else {false});
        let instances = sg.flat_vars( | v | if let ShVarStore::Instance(_) = *v {true} else {false});
        let mut varyings = sg.flat_vars( | v | if let ShVarStore::Varying = *v {true} else {false});
        let locals = sg.flat_vars( | v | if let ShVarStore::Local = *v {true} else {false});
        let pass_uniforms = sg.flat_vars( | v | if let ShVarStore::PassUniform = *v {true} else {false});
        let view_uniforms = sg.flat_vars( | v | if let ShVarStore::ViewUniform = *v {true} else {false});
        let draw_uniforms = sg.flat_vars( | v | if let ShVarStore::DrawUniform = *v {true} else {false});
        let uniforms = sg.flat_vars( | v | if let ShVarStore::Uniform(_) = *v {true} else {false});
        
        // lets count the slots
        let geometry_slots = sg.compute_slot_total(&geometries);
        let instance_slots = sg.compute_slot_total(&instances);
        //let varying_slots = sh.compute_slot_total(&varyings);
        
        mtl_out.push_str(&Self::mtl_assemble_struct("_Geom", &geometries, PackType::Packed, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Inst", &instances, PackType::Packed, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniPs", &pass_uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniVw", &view_uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniDr", &draw_uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Uni", &uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Loc", &locals, PackType::Unpacked, ""));
        
        // we need to figure out which texture slots exist
        mtl_out.push_str(&Self::mtl_assemble_texture_slots(&texture_slots));
        
        // we need to figure out which texture slots exist
        // mtl_out.push_str(&Self::assemble_constants(&texture_slots));
        let mut const_cx = SlCx {
            depth: 0,
            target: SlTarget::Constant,
            defargs_fn: "".to_string(),
            defargs_call: "".to_string(),
            call_prefix: "_".to_string(),
            shader_gen: sg,
            scope: Vec::new(),
            fn_deps: Vec::new(),
            fn_done: Vec::new(),
            auto_vary: Vec::new()
        };
        let consts = sg.flat_consts();
        for cnst in &consts {
            let const_init = assemble_const_init(cnst, &mut const_cx) ?;
            mtl_out.push_str("#define ");
            mtl_out.push_str(" ");
            mtl_out.push_str(&cnst.name);
            mtl_out.push_str(" (");
            mtl_out.push_str(&const_init.sl);
            mtl_out.push_str(")\n");
        }
        
        let mut vtx_cx = SlCx {
            depth: 0,
            target: SlTarget::Vertex,
            defargs_fn: "_Tex _tex, thread _Loc &_loc, thread _Vary &_vary, thread _Geom &_geom, thread _Inst &_inst, device _UniPs &_uni_ps, device _UniVw &_uni_vw, device _UniDr &_uni_dr, device _Uni &_uni".to_string(),
            defargs_call: "_tex, _loc, _vary, _geom, _inst, _uni_ps, _uni_vw, _uni_dr, _uni".to_string(),
            call_prefix: "_".to_string(),
            shader_gen: sg,
            scope: Vec::new(),
            fn_deps: vec!["vertex".to_string()],
            fn_done: Vec::new(),
            auto_vary: Vec::new()
        };
        let vtx_fns = assemble_fn_and_deps(sg, &mut vtx_cx) ?;
        let mut pix_cx = SlCx {
            depth: 0,
            target: SlTarget::Pixel,
            defargs_fn: "_Tex _tex, thread _Loc &_loc, thread _Vary &_vary, device _UniPs &_uni_ps, device _UniVw &_uni_vw, device _UniDr &_uni_dr, device _Uni &_uni".to_string(),
            defargs_call: "_tex, _loc, _vary, _uni_ps, _uni_vw, _uni_dr, _uni".to_string(),
            call_prefix: "_".to_string(),
            shader_gen: sg,
            scope: Vec::new(),
            fn_deps: vec!["pixel".to_string()],
            fn_done: vtx_cx.fn_done,
            auto_vary: Vec::new()
        };
        
        let pix_fns = assemble_fn_and_deps(sg, &mut pix_cx) ?;
        
        // lets add the auto_vary ones to the varyings struct
        for auto in &pix_cx.auto_vary {
            varyings.push(auto.clone());
        }
        mtl_out.push_str(&Self::mtl_assemble_struct("_Vary", &varyings, PackType::Unpacked, "  float4 mtl_position [[position]];\n"));
        
        mtl_out.push_str("//Vertex shader\n");
        mtl_out.push_str(&vtx_fns);
        mtl_out.push_str("//Pixel shader\n");
        mtl_out.push_str(&pix_fns);
        
        // lets define the vertex shader
        mtl_out.push_str("vertex _Vary _vertex_shader(_Tex _tex, device _Geom *in_geometries [[buffer(0)]], device _Inst *in_instances [[buffer(1)]],\n");
        mtl_out.push_str("  device _UniPs &_uni_ps [[buffer(2)]], device _UniVw &_uni_vw [[buffer(3)]], device _UniDr &_uni_dr [[buffer(4)]], device _Uni &_uni [[buffer(5)]],\n");
        mtl_out.push_str("  uint vtx_id [[vertex_id]], uint inst_id [[instance_id]]){\n");
        mtl_out.push_str("  _Loc _loc;\n");
        mtl_out.push_str("  _Vary _vary;\n");
        mtl_out.push_str("  _Geom _geom = in_geometries[vtx_id];\n");
        mtl_out.push_str("  _Inst _inst = in_instances[inst_id];\n");
        mtl_out.push_str("  _vary.mtl_position = _vertex(");
        mtl_out.push_str(&vtx_cx.defargs_call);
        mtl_out.push_str(");\n\n");
        
        for auto in pix_cx.auto_vary {
            if let ShVarStore::Geometry = auto.store {
                mtl_out.push_str("       _vary.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(" = _geom.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(";\n");
            }
            else if let ShVarStore::Instance(_) = auto.store {
                mtl_out.push_str("       _vary.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(" = _inst.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(";\n");
            }
        }
        
        mtl_out.push_str("       return _vary;\n");
        mtl_out.push_str("};\n");
        // then the fragment shader
        mtl_out.push_str("fragment float4 _fragment_shader(_Vary _vary[[stage_in]],_Tex _tex,\n");
        mtl_out.push_str("  device _UniPs &_uni_ps [[buffer(0)]], device _UniVw &_uni_vw [[buffer(1)]], device _UniDr &_uni_dr [[buffer(2)]], device _Uni &_uni [[buffer(3)]]){\n");
        mtl_out.push_str("  _Loc _loc;\n");
        mtl_out.push_str("  return _pixel(");
        mtl_out.push_str(&pix_cx.defargs_call);
        mtl_out.push_str(");\n};\n");
        
        if sg.log != 0 {
            println!("---- Metal shader -----\n{}", mtl_out);
        }
        
        let uniform_props = UniformProps::construct(sg, &uniforms);
        Ok((mtl_out, CxShaderMapping {
            rect_instance_props: RectInstanceProps::construct(sg, &instances),
            instance_props: InstanceProps::construct(sg, &instances),
            uniform_props,
            instances,
            geometries,
            instance_slots,
            geometry_slots,
            draw_uniforms,
            view_uniforms,
            pass_uniforms,
            uniforms,
            texture_slots,
        }))
        */
    }
    
    /*
    pub fn mtl_type_to_packed_metal(ty: &str) -> String {
        match ty.as_ref() {
            "float" => "float".to_string(),
            "vec2" => "packed_float2".to_string(),
            "vec3" => "packed_float3".to_string(),
            "vec4" => "packed_float4".to_string(),
            "mat2" => "packed_float2x2".to_string(),
            "mat3" => "packed_float3x3".to_string(),
            "mat4" => "float4x4".to_string(),
            ty => ty.to_string()
        }
    }
    
    pub fn mtl_type_to_metal(ty: &str) -> String {
        match ty.as_ref() {
            "float" => "float".to_string(),
            "vec2" => "float2".to_string(),
            "vec3" => "float3".to_string(),
            "vec4" => "float4".to_string(),
            "mat2" => "float2x2".to_string(),
            "mat3" => "float3x3".to_string(),
            "mat4" => "float4x4".to_string(),
            "texture2d" => "texture2d<float>".to_string(),
            ty => ty.to_string()
        }
    }
    
    pub fn mtl_assemble_struct(name: &str, vars: &Vec<ShVar>, pack_type: PackType, field: &str) -> String {
        let mut out = String::new();
        out.push_str("struct ");
        out.push_str(name);
        out.push_str("{\n");
        out.push_str(field);
        for var in vars {
            out.push_str("  ");
            match pack_type {
                PackType::Packed => {
                    out.push_str(&Self::mtl_type_to_packed_metal(&var.ty));
                    out.push_str(" ");
                    out.push_str(&var.name);
                    out.push_str(";\n");
                },
                PackType::Unpacked => {
                    out.push_str(&Self::mtl_type_to_metal(&var.ty));
                    out.push_str(" ");
                    out.push_str(&var.name);
                    out.push_str(";\n");
                }
            }
            
        };
        out.push_str("};\n\n");
        out
    }
    
    pub fn mtl_assemble_texture_slots(textures: &Vec<ShVar>) -> String {
        let mut out = String::new();
        out.push_str("struct ");
        out.push_str("_Tex{\n");
        for (i, tex) in textures.iter().enumerate() {
            out.push_str("texture2d<float> ");
            out.push_str(&tex.name);
            out.push_str(&format!(" [[texture({})]];\n", i));
        };
        out.push_str("};\n\n");
        out
    }
    
    pub fn mtl_assemble_shader(sg: &ShaderGen) -> Result<(String, CxShaderMapping), SlErr> {
        
        let mut mtl_out = "#include <metal_stdlib>\nusing namespace metal;\n".to_string();
        
        // ok now define samplers from our sh.
        let texture_slots = sg.flat_vars( | v | if let ShVarStore::Texture = *v {true} else {false});
        let geometries = sg.flat_vars( | v | if let ShVarStore::Geometry = *v {true} else {false});
        let instances = sg.flat_vars( | v | if let ShVarStore::Instance(_) = *v {true} else {false});
        let mut varyings = sg.flat_vars( | v | if let ShVarStore::Varying = *v {true} else {false});
        let locals = sg.flat_vars( | v | if let ShVarStore::Local = *v {true} else {false});
        let pass_uniforms = sg.flat_vars( | v | if let ShVarStore::PassUniform = *v {true} else {false});
        let view_uniforms = sg.flat_vars( | v | if let ShVarStore::ViewUniform = *v {true} else {false});
        let draw_uniforms = sg.flat_vars( | v | if let ShVarStore::DrawUniform = *v {true} else {false});
        let uniforms = sg.flat_vars( | v | if let ShVarStore::Uniform(_) = *v {true} else {false});
        
        // lets count the slots
        let geometry_slots = sg.compute_slot_total(&geometries);
        let instance_slots = sg.compute_slot_total(&instances);
        //let varying_slots = sh.compute_slot_total(&varyings);
        
        mtl_out.push_str(&Self::mtl_assemble_struct("_Geom", &geometries, PackType::Packed, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Inst", &instances, PackType::Packed, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniPs", &pass_uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniVw", &view_uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniDr", &draw_uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Uni", &uniforms, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Loc", &locals, PackType::Unpacked, ""));
        
        // we need to figure out which texture slots exist
        mtl_out.push_str(&Self::mtl_assemble_texture_slots(&texture_slots));
        
        // we need to figure out which texture slots exist
        // mtl_out.push_str(&Self::assemble_constants(&texture_slots));
        let mut const_cx = SlCx {
            depth: 0,
            target: SlTarget::Constant,
            defargs_fn: "".to_string(),
            defargs_call: "".to_string(),
            call_prefix: "_".to_string(),
            shader_gen: sg,
            scope: Vec::new(),
            fn_deps: Vec::new(),
            fn_done: Vec::new(),
            auto_vary: Vec::new()
        };
        let consts = sg.flat_consts();
        for cnst in &consts {
            let const_init = assemble_const_init(cnst, &mut const_cx) ?;
            mtl_out.push_str("#define ");
            mtl_out.push_str(" ");
            mtl_out.push_str(&cnst.name);
            mtl_out.push_str(" (");
            mtl_out.push_str(&const_init.sl);
            mtl_out.push_str(")\n");
        }
        
        let mut vtx_cx = SlCx {
            depth: 0,
            target: SlTarget::Vertex,
            defargs_fn: "_Tex _tex, thread _Loc &_loc, thread _Vary &_vary, thread _Geom &_geom, thread _Inst &_inst, device _UniPs &_uni_ps, device _UniVw &_uni_vw, device _UniDr &_uni_dr, device _Uni &_uni".to_string(),
            defargs_call: "_tex, _loc, _vary, _geom, _inst, _uni_ps, _uni_vw, _uni_dr, _uni".to_string(),
            call_prefix: "_".to_string(),
            shader_gen: sg,
            scope: Vec::new(),
            fn_deps: vec!["vertex".to_string()],
            fn_done: Vec::new(),
            auto_vary: Vec::new()
        };
        let vtx_fns = assemble_fn_and_deps(sg, &mut vtx_cx) ?;
        let mut pix_cx = SlCx {
            depth: 0,
            target: SlTarget::Pixel,
            defargs_fn: "_Tex _tex, thread _Loc &_loc, thread _Vary &_vary, device _UniPs &_uni_ps, device _UniVw &_uni_vw, device _UniDr &_uni_dr, device _Uni &_uni".to_string(),
            defargs_call: "_tex, _loc, _vary, _uni_ps, _uni_vw, _uni_dr, _uni".to_string(),
            call_prefix: "_".to_string(),
            shader_gen: sg,
            scope: Vec::new(),
            fn_deps: vec!["pixel".to_string()],
            fn_done: vtx_cx.fn_done,
            auto_vary: Vec::new()
        };
        
        let pix_fns = assemble_fn_and_deps(sg, &mut pix_cx) ?;
        
        // lets add the auto_vary ones to the varyings struct
        for auto in &pix_cx.auto_vary {
            varyings.push(auto.clone());
        }
        mtl_out.push_str(&Self::mtl_assemble_struct("_Vary", &varyings, PackType::Unpacked, "  float4 mtl_position [[position]];\n"));
        
        mtl_out.push_str("//Vertex shader\n");
        mtl_out.push_str(&vtx_fns);
        mtl_out.push_str("//Pixel shader\n");
        mtl_out.push_str(&pix_fns);
        
        // lets define the vertex shader
        mtl_out.push_str("vertex _Vary _vertex_shader(_Tex _tex, device _Geom *in_geometries [[buffer(0)]], device _Inst *in_instances [[buffer(1)]],\n");
        mtl_out.push_str("  device _UniPs &_uni_ps [[buffer(2)]], device _UniVw &_uni_vw [[buffer(3)]], device _UniDr &_uni_dr [[buffer(4)]], device _Uni &_uni [[buffer(5)]],\n");
        mtl_out.push_str("  uint vtx_id [[vertex_id]], uint inst_id [[instance_id]]){\n");
        mtl_out.push_str("  _Loc _loc;\n");
        mtl_out.push_str("  _Vary _vary;\n");
        mtl_out.push_str("  _Geom _geom = in_geometries[vtx_id];\n");
        mtl_out.push_str("  _Inst _inst = in_instances[inst_id];\n");
        mtl_out.push_str("  _vary.mtl_position = _vertex(");
        mtl_out.push_str(&vtx_cx.defargs_call);
        mtl_out.push_str(");\n\n");
        
        for auto in pix_cx.auto_vary {
            if let ShVarStore::Geometry = auto.store {
                mtl_out.push_str("       _vary.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(" = _geom.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(";\n");
            }
            else if let ShVarStore::Instance(_) = auto.store {
                mtl_out.push_str("       _vary.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(" = _inst.");
                mtl_out.push_str(&auto.name);
                mtl_out.push_str(";\n");
            }
        }
        
        mtl_out.push_str("       return _vary;\n");
        mtl_out.push_str("};\n");
        // then the fragment shader
        mtl_out.push_str("fragment float4 _fragment_shader(_Vary _vary[[stage_in]],_Tex _tex,\n");
        mtl_out.push_str("  device _UniPs &_uni_ps [[buffer(0)]], device _UniVw &_uni_vw [[buffer(1)]], device _UniDr &_uni_dr [[buffer(2)]], device _Uni &_uni [[buffer(3)]]){\n");
        mtl_out.push_str("  _Loc _loc;\n");
        mtl_out.push_str("  return _pixel(");
        mtl_out.push_str(&pix_cx.defargs_call);
        mtl_out.push_str(");\n};\n");
        
        if sg.log != 0 {
            println!("---- Metal shader -----\n{}", mtl_out);
        }
        
        let uniform_props = UniformProps::construct(sg, &uniforms);
        Ok((mtl_out, CxShaderMapping {
            rect_instance_props: RectInstanceProps::construct(sg, &instances),
            instance_props: InstanceProps::construct(sg, &instances),
            uniform_props,
            instances,
            geometries,
            instance_slots,
            geometry_slots,
            draw_uniforms,
            view_uniforms,
            pass_uniforms,
            uniforms,
            texture_slots,
        }))
    }
    
    pub fn mtl_compile_shader(sh: &mut CxShader, metal_cx: &MetalCx) -> Result<(), SlErr> {
        let (mtlsl, mapping) = Self::mtl_assemble_shader(&sh.shader_gen) ?;
        
        let options: id = unsafe {msg_send![class!(MTLCompileOptions), new]};
        let ns_mtlsl: id = str_to_nsstring(&mtlsl);
        let mut err: id = nil;
        let library: id = unsafe {msg_send![
            metal_cx.device,
            newLibraryWithSource: ns_mtlsl
            options: options
            error: &mut err
        ]};

        if library == nil {
            let err_str: id = unsafe {msg_send![err, localizedDescription]};
            return Err(SlErr {msg: nsstring_to_string(err_str)})
        }
        
        sh.mapping = mapping;
        sh.platform = Some(CxPlatformShader {
            pipeline_state: unsafe {
                let vert: id = msg_send![library, newFunctionWithName: str_to_nsstring("_vertex_shader")];
                let frag: id = msg_send![library, newFunctionWithName: str_to_nsstring("_fragment_shader")];
                let rpd: id = msg_send![class!(MTLRenderPipelineDescriptor), new];
                
                let () = msg_send![rpd, setVertexFunction: vert];
                let () = msg_send![rpd, setFragmentFunction: frag];
                
                let color_attachments: id = msg_send![rpd, colorAttachments];
                
                let ca: id = msg_send![color_attachments, objectAtIndexedSubscript: 0u64];
                let () = msg_send![ca, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
                let () = msg_send![ca, setBlendingEnabled: YES];
                let () = msg_send![ca, setSourceRGBBlendFactor: MTLBlendFactor::One];
                let () = msg_send![ca, setDestinationRGBBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
                let () = msg_send![ca, setSourceAlphaBlendFactor: MTLBlendFactor::One];
                let () = msg_send![ca, setDestinationAlphaBlendFactor: MTLBlendFactor::OneMinusSourceAlpha];
                let () = msg_send![ca, setRgbBlendOperation: MTLBlendOperation::Add];
                let () = msg_send![ca, setAlphaBlendOperation: MTLBlendOperation::Add];
                let () = msg_send![rpd, setDepthAttachmentPixelFormat: MTLPixelFormat::Depth32Float_Stencil8];
                
                let mut err: id = nil;
                let rps: id = msg_send![
                    metal_cx.device,
                    newRenderPipelineStateWithDescriptor: rpd 
                    error: &mut err
                ];
                if rps == nil{
                    panic!("Could not create render pipeline state")
                }
                rps//.expect("Could not create render pipeline state")
            },
            library: library,
            geom_ibuf: {
                let mut geom_ibuf = MetalBuffer {..Default::default()};
                geom_ibuf.update_with_u32_data(metal_cx, &sh.shader_gen.geometry_indices);
                geom_ibuf
            },
            geom_vbuf: {
                let mut geom_vbuf = MetalBuffer {..Default::default()};
                geom_vbuf.update_with_f32_data(metal_cx, &sh.shader_gen.geometry_vertices);
                geom_vbuf
            }
        });
        return Ok(());
    }*/
}
/*
impl<'a> SlCx<'a> {
    pub fn map_call(&self, name: &str, args: &Vec<Sl>) -> MapCallResult {
        match name {
            "sample2d" => { // transform call to
                let base = &args[0];
                let coord = &args[1];
                return MapCallResult::Rewrite(
                    format!("{}.sample(sampler(mag_filter::linear,min_filter::linear),{})", base.sl, coord.sl),
                    "vec4".to_string()
                )
            },
            "color" => {
                let col = color(&args[0].sl);
                return MapCallResult::Rewrite(
                    format!("float4({},{},{},{})", col.r, col.g, col.b, col.a),
                    "vec4".to_string()
                );
            },
            _ => return MapCallResult::None
        }
    }
    
    pub fn mat_mul(&self, left: &str, right: &str) -> String {
        format!("{}*{}", left, right)
    }
    
    pub fn map_type(&self, ty: &str) -> String {
        Cx::mtl_type_to_metal(ty)
    }
    
    pub fn map_constructor(&self, name: &str, args: &Vec<Sl>) -> String {
        let mut out = String::new();
        out.push_str(&self.map_type(name));
        out.push_str("(");
        for (i, arg) in args.iter().enumerate() {
            if i != 0 {
                out.push_str(", ");
            }
            out.push_str(&arg.sl);
        }
        out.push_str(")");
        return out;
    }
    
    pub fn map_var(&mut self, var: &ShVar) -> String {
        let mty = Cx::mtl_type_to_metal(&var.ty);
        match var.store {
            ShVarStore::Uniform(_) => return format!("_uni.{}", var.name),
            ShVarStore::UniformColor(_) => return format!("_uni_col.{}", var.name),
            ShVarStore::ViewUniform => return format!("_uni_vw.{}", var.name),
            ShVarStore::PassUniform => return format!("_uni_ps.{}", var.name),
            ShVarStore::DrawUniform => return format!("_uni_dr.{}", var.name),
            ShVarStore::Instance(_) => {
                if let SlTarget::Pixel = self.target {
                    if self.auto_vary.iter().find( | v | v.name == var.name).is_none() {
                        self.auto_vary.push(var.clone());
                    }
                    return format!("_vary.{}", var.name);
                }
                else {
                    return format!("{}(_inst.{})", mty, var.name);
                }
            },
            ShVarStore::Geometry => {
                if let SlTarget::Pixel = self.target {
                    if self.auto_vary.iter().find( | v | v.name == var.name).is_none() {
                        self.auto_vary.push(var.clone());
                    }
                    return format!("_vary.{}", var.name);
                }
                else {
                    
                    return format!("{}(_geom.{})", mty, var.name);
                }
            },
            ShVarStore::Texture => return format!("_tex.{}", var.name),
            ShVarStore::Local => return format!("_loc.{}", var.name),
            ShVarStore::Varying => return format!("_vary.{}", var.name),
        }
    }
}*/