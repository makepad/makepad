use crate::shader::*;
use crate::cx::*;
use crate::cxdrawing_gl::*;
use crate::shadergen::*;
use crate::cxdrawing_shared::*;
/*
#[derive(Default,Clone)]
pub struct WebGLAttribute{
    pub loc:u32,
    pub size:u32,
    pub offset:u32,
    pub stride:u32
}

#[derive(Default,Clone)]
pub struct WebGLUniform{
    pub loc:u32,
    pub name:String,
    pub size:usize
}
#[derive(Default,Clone)]
pub struct WebGLTextureSlot{
    pub loc:u32,
    pub name:String
}*/

#[derive(Default,Clone)]
pub struct CompiledShader{
    pub shader_id: usize,
    pub geom_vb_id: usize,
    pub geom_ib_id: usize,
    pub instance_slots:usize,
    pub geometry_slots:usize,
    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_dl: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots:Vec<ShVar>,
    pub rect_instance_props: RectInstanceProps,
    pub named_instance_props: NamedInstanceProps
}

#[derive(Default,Clone)]
pub struct WebGLTexture2D{
    pub texture_id: usize
}

#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
}

impl CxDrawing{

    pub fn compile_all_webgl_shaders(&mut self, resources: &mut CxResources){
        for sh in &self.shaders{
            let csh = self.compile_webgl_shader(&sh, resources);
            if let Ok(csh) = csh{
                self.compiled_shaders.push(CompiledShader{
                    shader_id:self.compiled_shaders.len(),
                    ..csh
                });
            }
            else if let Err(err) = csh{
                resources.from_wasm.log(&format!("GOT ERROR: {}", err.msg));
                self.compiled_shaders.push(
                    CompiledShader{..Default::default()}
                )
            }
        };
    }

    pub fn compile_webgl_shader(&self, sh:&Shader, resources: &mut CxResources)->Result<CompiledShader, SlErr>{
        let ash = gl_assemble_shader(sh, GLShaderType::WebGL1)?;
        let shader_id = self.compiled_shaders.len();
        resources.from_wasm.compile_webgl_shader(shader_id, &ash);

        let geom_ib_id = resources.get_free_index_buffer();
        let geom_vb_id = resources.get_free_index_buffer();

        resources.from_wasm.alloc_array_buffer(
            geom_vb_id,
            sh.geometry_vertices.len(),
            sh.geometry_vertices.as_ptr() as *const f32
        );

        resources.from_wasm.alloc_index_buffer(
            geom_ib_id,
            sh.geometry_indices.len(),
            sh.geometry_indices.as_ptr() as *const u32
        );

        let csh = CompiledShader{
            shader_id:shader_id,
            geometry_slots:ash.geometry_slots,
            instance_slots:ash.instance_slots,
            geom_vb_id:geom_vb_id,
            geom_ib_id:geom_ib_id,
            uniforms_cx:ash.uniforms_cx.clone(),
            uniforms_dl:ash.uniforms_dl.clone(),
            uniforms_dr:ash.uniforms_dr.clone(),
            texture_slots:ash.texture_slots.clone(),
            rect_instance_props:ash.rect_instance_props.clone(),
            named_instance_props:ash.named_instance_props.clone(),
            //assembled_shader:ash,
            ..Default::default()
        };

        Ok(csh)
      }
}