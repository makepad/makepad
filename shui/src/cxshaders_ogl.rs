use std::mem;
use std::ptr;

use crate::shader::*;
use crate::cxtextures::*;
use crate::cxdrawing::*;
use crate::cxshaders_shared::*;
use crate::cxshaders_gl::*;

#[derive(Default,Clone)]
pub struct GLAttribute{
    pub loc:gl::types::GLuint,
    pub size:gl::types::GLsizei,
    pub offset:gl::types::GLsizei,
    pub stride:gl::types::GLsizei
}

#[derive(Default,Clone)]
pub struct GLUniform{
    pub loc:gl::types::GLint,
    pub name:String,
    pub size:usize
}

#[derive(Default,Clone)]
pub struct GLTextureSlot{
    pub loc:gl::types::GLint,
    pub name:String
}

#[derive(Default,Clone)]
pub struct AssembledGLShader{
    pub geometry_slots:usize,
    pub instance_slots:usize,
    pub geometry_attribs:usize,
    pub instance_attribs:usize,

    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_dl: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots:Vec<ShVar>,

    pub fragment:String,
    pub vertex:String,
    pub named_instance_props: NamedInstanceProps
}

#[derive(Default,Clone)]
pub struct CompiledShader{
    pub shader_id: usize,
    pub program: gl::types::GLuint,
    pub geom_attribs: Vec<GLAttribute>,
    pub inst_attribs: Vec<GLAttribute>,
    pub geom_vb: gl::types::GLuint,
    pub geom_ib: gl::types::GLuint,
    //pub assembled_shader: AssembledGLShader,
    pub instance_slots:usize,
    pub uniforms_dr: Vec<GLUniform>,
    pub uniforms_dl: Vec<GLUniform>,
    pub uniforms_cx: Vec<GLUniform>,
    pub texture_slots: Vec<GLUniform>,
    pub named_instance_props: NamedInstanceProps
}

#[derive(Default,Clone)]
pub struct GLTexture2D{
    pub texture_id: usize
}

#[derive(Clone, Default)]
pub struct CxResources{

}

#[derive(Clone, Default)]
pub struct DrawListResources{
}


#[derive(Default,Clone)]
pub struct DrawCallResources{
    pub resource_shader_id:usize,
    pub vao:gl::types::GLuint,
    pub vb:gl::types::GLuint
}

impl DrawCallResources{

    pub fn check_attached_vao(&mut self, csh:&CompiledShader){
        if self.resource_shader_id != csh.shader_id{
            self.free();
        }
        // create the VAO
        unsafe{
            self.resource_shader_id = csh.shader_id;
            self.vao = mem::uninitialized();
            gl::GenVertexArrays(1, &mut self.vao);
            gl::BindVertexArray(self.vao);
            
            // bind the vertex and indexbuffers
            gl::BindBuffer(gl::ARRAY_BUFFER, csh.geom_vb);
            for attr in &csh.geom_attribs{
                gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                gl::EnableVertexAttribArray(attr.loc);
            }

            // create and bind the instance buffer
            self.vb = mem::uninitialized();
            gl::GenBuffers(1, &mut self.vb);
            gl::BindBuffer(gl::ARRAY_BUFFER, self.vb);
            
            for attr in &csh.inst_attribs{
                gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                gl::EnableVertexAttribArray(attr.loc);
                gl::VertexAttribDivisor(attr.loc, 1 as gl::types::GLuint);
            }

            // bind the indexbuffer
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, csh.geom_ib);
            gl::BindVertexArray(0);
        }
    }

    fn free(&mut self){
        unsafe{
            if self.vao != 0{
                gl::DeleteVertexArrays(1, &mut self.vao);
            }
            if self.vb != 0{
                gl::DeleteBuffers(1, &mut self.vb);
            }
        }
        self.vao = 0;
        self.vb = 0;
    }
}

#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
}

impl CxShaders{
    pub fn compile_all_shaders(&mut self){
        for sh in &self.shaders{
            let glsh = Self::compile_shader(&sh);
            if let Ok(glsh) = glsh{
                self.compiled_shaders.push(CompiledShader{
                    shader_id:self.compiled_shaders.len(),
                    ..glsh
                });
            }
            else if let Err(err) = glsh{
                println!("GOT ERROR: {}", err.msg);
                self.compiled_shaders.push(
                    CompiledShader{..Default::default()}
                )
            }
        };
    }

    pub fn compile_has_shader_error(compile:bool, shader:gl::types::GLuint, source:&str)->Option<String>{
        unsafe{
            let mut success = i32::from(gl::FALSE);
           
            if compile{
                gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut success);
            }
            else{
                gl::GetProgramiv(shader, gl::LINK_STATUS, &mut success);
            };
           
            if success != i32::from(gl::TRUE) {
                 let mut info_log = Vec::<u8>::with_capacity(2048);
                info_log.set_len(2047);
                for i in 0..2047{
                    info_log[i] = 0;
                };
                if compile{
                    gl::GetShaderInfoLog(shader, 2048, ptr::null_mut(),
                        info_log.as_mut_ptr() as *mut gl::types::GLchar)
                }
                else{
                    gl::GetProgramInfoLog(shader, 2048, ptr::null_mut(),
                        info_log.as_mut_ptr() as *mut gl::types::GLchar)
                }
                let mut r = "".to_string();
                r.push_str(&String::from_utf8(info_log).unwrap());
                r.push_str("\n");
                let split = source.split("\n");
                for (line,chunk) in split.enumerate(){
                    r.push_str(&(line+1).to_string());
                    r.push_str(":");
                    r.push_str(chunk);
                    r.push_str("\n");
                }
                Some(r)
            }
            else{
                None
            }
        }
    }

    pub fn compile_get_attributes(program:gl::types::GLuint, prefix:&str, slots:usize, num_attr:usize)->Vec<GLAttribute>{
        let mut attribs = Vec::new();
        let stride = (slots * mem::size_of::<f32>()) as gl::types::GLsizei;
        for i in 0..num_attr{
            let mut name = prefix.to_string();
            name.push_str(&i.to_string());
            name.push_str("\0");
            
            let mut size = ((slots - i*4)) as gl::types::GLsizei;
            if size > 4{
                size = 4;
            }
            unsafe{
                attribs.push(
                    GLAttribute{
                        loc: gl::GetAttribLocation(program, name.as_ptr() as *const _) as gl::types::GLuint,
                        offset: (i * 4 * mem::size_of::<f32>()) as i32,
                        size:  size,
                        stride: stride
                    }
                )
            }
        }
        attribs
    }

    pub fn compile_get_uniforms(program:gl::types::GLuint, sh:&Shader, unis:&Vec<ShVar>)->Vec<GLUniform>{
        let mut gl_uni = Vec::new();
        for uni in unis{
            let mut name0 = "".to_string();
            name0.push_str(&uni.name);
            name0.push_str("\0");
            unsafe{
                gl_uni.push(GLUniform{
                    loc:gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name:uni.name.clone(),
                    size:sh.get_type_slots(&uni.ty)
                })
            }
        }
        gl_uni
    }

    pub fn compile_get_texture_slots(program:gl::types::GLuint, texture_slots:&Vec<ShVar>)->Vec<GLUniform>{
        let mut gl_texture_slots = Vec::new();
        for slot in texture_slots{
            let mut name0 = "".to_string();
            name0.push_str(&slot.name);
            name0.push_str("\0");
            unsafe{
                gl_texture_slots.push(GLUniform{
                    loc:gl::GetUniformLocation(program, name0.as_ptr() as *const _),
                    name:slot.name.clone(),
                    size:0
                    //,sampler:sam.sampler.clone()
                })
            }
        }
        gl_texture_slots
    }

    pub fn compile_shader(sh:&Shader)->Result<CompiledShader, SlErr>{
        let ash = gl_assemble_shader(sh)?;
        // now we have a pixel and a vertex shader
        // so lets now pass it to GL
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
        }
    }


    pub fn set_uniform_buffer_fallback(locs:&Vec<GLUniform>, uni:&Vec<f32>){
        let mut o = 0;
        for loc in locs{
            if loc.loc >=0 {
                unsafe{
                    match loc.size{
                        1=>gl::Uniform1f(loc.loc, uni[o]),
                        2=>gl::Uniform2f(loc.loc, uni[o], uni[o+1]),
                        3=>gl::Uniform3f(loc.loc, uni[o], uni[o+1], uni[o+2]),
                        4=>gl::Uniform4f(loc.loc, uni[o], uni[o+1], uni[o+2], uni[o+3]),
                        16=>gl::UniformMatrix4fv(loc.loc, 1, 0, uni.as_ptr().offset((o) as isize)),
                        _=>()
                    }
                }
            };
            o = o + loc.size;
        }
    }

    pub fn set_texture_slots(locs:&Vec<GLUniform>, texture_ids:&Vec<usize>, cxtex:&mut CxTextures){
        let mut o = 0;
        for loc in locs{
            let id = texture_ids[o];
            unsafe{
                gl::ActiveTexture(gl::TEXTURE0 + o as u32);
            }        
            
            if loc.loc >=0{
                let mut tex = &mut cxtex.textures[id];
                if tex.dirty{
                    tex.upload_to_device();
                }
                if let Some(gl_texture) = tex.gl_texture{
                    unsafe{
                        gl::BindTexture(gl::TEXTURE_2D, gl_texture);
                    }
                }
            }
            else{
                unsafe{
                    gl::BindTexture(gl::TEXTURE_2D, 0);
                }
            }
            o = o +1;
        }
    }
}