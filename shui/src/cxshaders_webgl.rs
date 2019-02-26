use crate::shader::*;
use crate::cx::*;
use crate::cxshaders_shared::*;
use crate::cxshaders_gl::*;
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
    pub named_instance_props: NamedInstanceProps
}

#[derive(Default,Clone)]
pub struct WebGLTexture2D{
    pub texture_id: usize
}

// storage buffers for graphics API related resources
#[derive(Clone)]
pub struct CxResources{
    pub from_wasm:FromWasm,
    pub vertex_buffers:usize,
    pub vertex_buffers_free:Vec<usize>,
    pub index_buffers:usize,
    pub index_buffers_free:Vec<usize>,
    pub vaos:usize,
    pub vaos_free:Vec<usize>
}

impl Default for CxResources{
    fn default()->CxResources{
        CxResources{
            from_wasm:FromWasm::zero(),
            vertex_buffers:1,
            vertex_buffers_free:Vec::new(),
            index_buffers:1,
            index_buffers_free:Vec::new(),
            vaos:1,
            vaos_free:Vec::new()
        }
    }
}

impl CxResources{
    fn get_free_vertex_buffer(&mut self)->usize{
        if self.vertex_buffers_free.len() > 0{
            self.vertex_buffers_free.pop().unwrap()
        }
        else{
            self.vertex_buffers += 1;
            self.vertex_buffers
        }
    }
    fn get_free_index_buffer(&mut self)->usize{
        if self.index_buffers_free.len() > 0{
            self.index_buffers_free.pop().unwrap()
        }
        else{
            self.index_buffers += 1;
            self.index_buffers
        }
    }
     fn get_free_vao(&mut self)->usize{
        if self.vaos_free.len() > 0{
            self.vaos_free.pop().unwrap()
        }
        else{
            self.vaos += 1;
            self.vaos
        }
    }
}

#[derive(Clone, Default)]
pub struct DrawListResources{
}

#[derive(Default,Clone)]
pub struct DrawCallResources{
    pub resource_shader_id:usize,
    pub vao_id:usize,
    pub inst_vb_id:usize
}

#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
}

impl DrawCallResources{

    pub fn check_attached_vao(&mut self, csh:&CompiledShader, resources:&mut CxResources){
        if self.resource_shader_id != csh.shader_id{
            self.free(resources); // dont reuse vaos accross shader ids
        }
        // create the VAO
        self.resource_shader_id = csh.shader_id;

        // get a free vao ID
        self.vao_id = resources.get_free_vao();
        self.inst_vb_id = resources.get_free_index_buffer();

        resources.from_wasm.alloc_array_buffer(
            self.inst_vb_id,0,0 as *const f32
        );

        resources.from_wasm.alloc_vao(
            csh.shader_id,
            self.vao_id,
            csh.geom_ib_id,
            csh.geom_vb_id,
            self.inst_vb_id,
        );
    }

    fn free(&mut self, resources:&mut CxResources){
        resources.vaos_free.push(self.vao_id);
        resources.vertex_buffers_free.push(self.inst_vb_id);
        self.vao_id = 0;
        self.inst_vb_id = 0;
    }
}


impl CxShaders{

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
            named_instance_props:ash.named_instance_props.clone(),
            //assembled_shader:ash,
            ..Default::default()
        };

        Ok(csh)
      }
}