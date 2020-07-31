#![allow(unused_imports)]

use crate::cx::*;
use crate::cx_apple::*;

//use makepad_shader_compiler::ast::{ShaderAst, Decl, TyExprKind};
//use makepad_shader_compiler::colors::Color;
//use makepad_shader_compiler::{generate};
use makepad_shader_compiler::generate_metal;


#[derive(Clone)]
pub struct CxPlatformShader {
    pub library: id,
    pub metal_shader: String,
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
        for (index, sh) in &mut self.shaders.iter_mut().enumerate() {
            let result = Self::mtl_compile_shader(index, false, sh, metal_cx);
            if let ShaderCompileResult::Fail{err, ..} = result {
                panic!("{}", err);
            } 
        };
    } 
    
    pub fn mtl_compile_shader(shader_id:usize, use_const_table:bool, sh: &mut CxShader, metal_cx: &MetalCx) -> ShaderCompileResult {
        let shader_ast = sh.shader_gen.lex_parse_analyse();
        if let Err(err) = shader_ast{
            return ShaderCompileResult::Fail{id:shader_id, err:err}
        } 
        let shader_ast = shader_ast.unwrap();
        
        let mtlsl =  generate_metal::generate_shader(&shader_ast, use_const_table);
        let mapping = CxShaderMapping::from_shader_gen(&sh.shader_gen, shader_ast.const_table.borrow_mut().take());
        
        if let Some(sh_platform) = &sh.platform{
            println!("{}, {}", sh_platform.metal_shader, mtlsl);
            if sh_platform.metal_shader == mtlsl{
                return ShaderCompileResult::Nop{id:shader_id}
            }
        } 
        
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
            panic!("{}", nsstring_to_string(err_str));
            //return Err(SlErr {msg: nsstring_to_string(err_str)})
        }
        
        sh.mapping = mapping;
        sh.platform = Some(CxPlatformShader { 
            metal_shader: mtlsl, 
            pipeline_state: unsafe {
                let vert: id = msg_send![library, newFunctionWithName: str_to_nsstring("mpsc_vertex_main")];
                let frag: id = msg_send![library, newFunctionWithName: str_to_nsstring("mpsc_fragment_main")];
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
        return ShaderCompileResult::Ok{id:shader_id};
    }
}