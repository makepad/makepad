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
    pub geom_vb: usize,
    pub geom_ib: usize,
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
#[derive(Clone, Default)]
pub struct CxResources{
    pub wasm_send:WasmSend,
    pub vertex_buffers:usize,
    pub vertex_buffers_free:Vec<usize>,
    pub index_buffers:usize,
    pub index_buffers_free:Vec<usize>
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
}

#[derive(Clone, Default)]
pub struct DrawListResources{
}

#[derive(Default,Clone)]
pub struct DrawCallResources{
}

#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
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
                resources.wasm_send.log(&format!("GOT ERROR: {}", err.msg));
                self.compiled_shaders.push(
                    CompiledShader{..Default::default()}
                )
            }
        };
    }

    pub fn compile_webgl_shader(&self, sh:&Shader, resources: &mut CxResources)->Result<CompiledShader, SlErr>{
        let ash = gl_assemble_shader(sh, GLShaderType::WebGL1)?;
        let shader_id = self.compiled_shaders.len();
        resources.wasm_send.compile_webgl_shader(shader_id, &ash);

        let geom_ib = resources.get_free_index_buffer();
        let geom_vb = resources.get_free_index_buffer();

        unsafe{
            resources.wasm_send.alloc_array_buffer(
                geom_vb,
                sh.geometry_vertices.len(),
                sh.geometry_vertices.as_ptr() as *const f32
            );

            resources.wasm_send.alloc_index_buffer(
                geom_ib,
                sh.geometry_indices.len(),
                sh.geometry_indices.as_ptr() as *const u32
            );
        }
/*
// lets create static geom and index buffers for this shader
            let mut geom_vb = mem::uninitialized();
            gl::GenBuffers(1, &mut geom_vb);
            gl::BindBuffer(gl::ARRAY_BUFFER, geom_vb);
            gl::BufferData(gl::ARRAY_BUFFER,
                            (sh.geometry_vertices.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr,
                            sh.geometry_vertices.as_ptr() as *const _, gl::STATIC_DRAW);

            let mut geom_ib = mem::uninitialized();
            gl::GenBuffers(1, &mut geom_ib);
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, geom_ib);
            gl::BufferData(gl::ELEMENT_ARRAY_BUFFER,
                            (sh.geometry_indices.len() * mem::size_of::<u32>()) as gl::types::GLsizeiptr,
                            sh.geometry_indices.as_ptr() as *const _, gl::STATIC_DRAW);
*/
        let csh = CompiledShader{
            shader_id:shader_id,
            geometry_slots:ash.geometry_slots,
            instance_slots:ash.instance_slots,
            geom_vb:geom_vb,
            geom_ib:geom_ib,
            uniforms_cx:ash.uniforms_cx.clone(),
            uniforms_dl:ash.uniforms_dl.clone(),
            uniforms_dr:ash.uniforms_dr.clone(),
            texture_slots:ash.texture_slots.clone(),
            named_instance_props:ash.named_instance_props.clone(),
            //assembled_shader:ash,
            ..Default::default()
        };

        Ok(csh)
        // now we have a pixel and a vertex shader
        // so lets now pass it to GL
        /*
        unsafe{
            
            let vs = gl::CreateShader(gl::VERTEX_SHADER);
            gl::ShaderSource(vs, 1, [ash.vertex.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(vs);
            if let Some(error) = Self::compile_has_shader_error(true, vs, &ash.vertex){
                return Err(SlErr{
                    msg:format!("ERROR::SHADER::VERTEX::COMPILATION_FAILED\n{}",error)
                })
            }

            let fs = gl::CreateShader(gl::FRAGMENT_SHADER);
            gl::ShaderSource(fs, 1, [ash.fragment.as_ptr() as *const _].as_ptr(), ptr::null());
            gl::CompileShader(fs);
            if let Some(error) = Self::compile_has_shader_error(true, fs, &ash.fragment){
                return Err(SlErr{
                    msg:format!("ERROR::SHADER::FRAGMENT::COMPILATION_FAILED\n{}",error)
                })
            }

            let program = gl::CreateProgram();
            gl::AttachShader(program, vs);
            gl::AttachShader(program, fs);
            gl::LinkProgram(program);
            if let Some(error) = Self::compile_has_shader_error(false, program, ""){
                return Err(SlErr{
                    msg:format!("ERROR::SHADER::LINK::COMPILATION_FAILED\n{}",error)
                })
            }
            gl::DeleteShader(vs);
            gl::DeleteShader(fs);

            let geom_attribs = Self::compile_get_attributes(program, "geomattr", ash.geometry_slots, ash.geometry_attribs);
            let inst_attribs = Self::compile_get_attributes(program, "instattr", ash.instance_slots, ash.instance_attribs);

            // lets create static geom and index buffers for this shader
            let mut geom_vb = mem::uninitialized();
            gl::GenBuffers(1, &mut geom_vb);
            gl::BindBuffer(gl::ARRAY_BUFFER, geom_vb);
            gl::BufferData(gl::ARRAY_BUFFER,
                            (sh.geometry_vertices.len() * mem::size_of::<f32>()) as gl::types::GLsizeiptr,
                            sh.geometry_vertices.as_ptr() as *const _, gl::STATIC_DRAW);

            let mut geom_ib = mem::uninitialized();
            gl::GenBuffers(1, &mut geom_ib);
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, geom_ib);
            gl::BufferData(gl::ELEMENT_ARRAY_BUFFER,
                            (sh.geometry_indices.len() * mem::size_of::<u32>()) as gl::types::GLsizeiptr,
                            sh.geometry_indices.as_ptr() as *const _, gl::STATIC_DRAW);

            // lets fetch the uniform positions for our uniforms
            return Ok(CompiledShader{
                program:program,
                geom_attribs:geom_attribs,
                inst_attribs:inst_attribs,
                geom_vb:geom_vb,
                geom_ib:geom_ib,
                uniforms_cx:Self::compile_get_uniforms(program, sh, &ash.uniforms_cx),
                uniforms_dl:Self::compile_get_uniforms(program, sh, &ash.uniforms_dl),
                uniforms_dr:Self::compile_get_uniforms(program, sh, &ash.uniforms_dr),
                texture_slots:Self::compile_get_texture_slots(program, &ash.texture_slots),
                named_instance_props:ash.named_instance_props.clone(),
                instance_slots:ash.instance_slots,
                //assembled_shader:ash,
                ..Default::default()
            })
        }*/
    }
}