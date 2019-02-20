use std::mem;
use std::ptr;

use crate::shader::*;
use crate::cxtextures::*;
use crate::cxdrawing::*;
use crate::cxshaders_shared::*;

impl<'a> SlCx<'a>{
    pub fn map_call(&self, name:&str, _args:&Vec<Sl>)->MapCallResult{
        match name{
            "matrix_comp_mult"=>return MapCallResult::Rename("matrixCompMult".to_string()),
            "less_than"=>return MapCallResult::Rename("less_than".to_string()),
            "less_than_equal"=>return MapCallResult::Rename("lessThanEqual".to_string()),
            "greater_than"=>return MapCallResult::Rename("greaterThan".to_string()),
            "greater_than_equal"=>return MapCallResult::Rename("greaterThanEqual".to_string()),
            "not_equal"=>return MapCallResult::Rename("notEqual".to_string()),
            "sample2d"=>{
                return MapCallResult::Rename("texture2D".to_string())
            },
            _=>return MapCallResult::None
        }
    }

    pub fn map_type(&self, ty:&str)->String{
        match ty{
            "texture2d"=>return "sampler2D".to_string(),
            _=>return ty.to_string()
        }
    }

    pub fn map_var(&mut self, var:&ShVar)->String{
        match var.store{
            ShVarStore::Instance=>{
                if let SlTarget::Pixel = self.target{
                    self.auto_vary.push(var.clone());
                }
            },
            ShVarStore::Geometry=>{
                if let SlTarget::Pixel = self.target{
                    self.auto_vary.push(var.clone());
                }
            },
            _=>()
        }
        var.name.clone()
    }
    
}

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
    pub vertex:String
}

#[derive(Default,Clone)]
pub struct CompiledShader{
    pub shader_id: usize,
    pub program: gl::types::GLuint,
    pub geom_attribs: Vec<GLAttribute>,
    pub inst_attribs: Vec<GLAttribute>,
    pub geom_vb: gl::types::GLuint,
    pub geom_ib: gl::types::GLuint,
    pub assembled_shader: AssembledGLShader,
    pub uniforms_dr: Vec<GLUniform>,
    pub uniforms_dl: Vec<GLUniform>,
    pub uniforms_cx: Vec<GLUniform>,
    pub texture_slots: Vec<GLUniform>
}

#[derive(Default,Clone)]
pub struct GLTexture2D{
    pub texture_id: usize
}

#[derive(Clone, Default)]
pub struct CxBuffers{
}

#[derive(Clone, Default)]
pub struct DrawListBuffers{
}


#[derive(Default,Clone)]
pub struct DrawBuffers{
}

#[derive(Clone, Default)]
pub struct CxShaders{
    pub compiled_shaders: Vec<CompiledShader>,
    pub shaders: Vec<Shader>,
}

impl CxShaders{

    pub fn get(&self, id:usize)->&CompiledShader{
        &self.compiled_shaders[id]
    }

    pub fn add(&mut self, sh:Shader)->usize{
        let id = self.shaders.len();
        // lets compile this sh
        self.shaders.push(sh);
        id
    }

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

    pub fn assemble_uniforms(unis:&Vec<ShVar>)->String{
        let mut out = String::new();
        for uni in unis{
            out.push_str("uniform ");
            out.push_str(&uni.ty);
            out.push_str(" ");
            out.push_str(&uni.name);
            out.push_str(";\n")
        };
        out
    }
    
    fn ceil_div4(base:usize)->usize{
        let r = base >> 2;
        if base&3 != 0{
            return r + 1
        }
        r
    }

    pub fn assemble_texture_slots(unis:&Vec<ShVar>)->String{
        let mut out = String::new();
        for uni in unis{
            out.push_str("uniform ");
            match uni.ty.as_ref(){
                "texture2d"=>out.push_str("sampler2D"),
                _=>out.push_str("unknown_texture_type")
            };
            out.push_str(" ");
            out.push_str(&uni.name);
            out.push_str(";\n")
        };
        out
    }

    fn assemble_vartype(i:usize, total:usize, left:usize)->String{
        if i == total - 1{
            match left{
                1=>"float",
                2=>"vec2",
                3=>"vec3",
                _=>"vec4"
            }.to_string()
        }
        else{
            "vec4".to_string()
        }
    }
    

    fn assemble_varblock(thing: &str, base: &str, slots:usize)->String{
        // ok lets do a ceil
        let mut out = String::new();
        let total = Self::ceil_div4(slots);
        for i in 0..total{
            out.push_str(thing);
            out.push_str(" ");
            out.push_str(&Self::assemble_vartype(i, total, slots&3));
            out.push_str(" ");
            out.push_str(base);
            out.push_str(&i.to_string());
            out.push_str(";\n");
        }
        out
    }

    fn assemble_vardef(var:&ShVar)->String{
        // ok lets do a ceil
        let mut out = String::new();
        out.push_str(&var.ty);
        out.push_str(" ");
        out.push_str(&var.name);
        out.push_str(";\n");
        out
    }

    fn assemble_unpack(base: &str, slot:usize, total_slots:usize,sv:&ShVar)->String{
        let mut out = String::new();
        // ok we have the slot we start at
        out.push_str("    ");
        out.push_str(&sv.name);
        out.push_str("=");
        let id = (slot)>>2;

        // just splat directly
        if sv.ty == "vec2"{
            match slot&3{
                0=>{
                    out.push_str(base);
                    out.push_str(&id.to_string());
                    out.push_str(".xy;\r\n");
                    return out
                }
                1=>{
                    out.push_str(base);
                    out.push_str(&id.to_string());
                    out.push_str(".yz;\r\n");
                    return out
                }
                2=>{
                    out.push_str(base);
                    out.push_str(&id.to_string());
                    out.push_str(".zw;\r\n");
                    return out
                }
                _=>()            
            }
        }
        if sv.ty == "vec3"{
            match slot&3{
                0=>{
                    out.push_str(base);
                    out.push_str(&id.to_string());
                    out.push_str(".xyz;\r\n");
                    return out
                }
                1=>{
                    out.push_str(base);
                    out.push_str(&id.to_string());
                    out.push_str(".yzw;\r\n");
                    return out
                }            
                _=>()            
            }
        }        
        if sv.ty == "vec4"{
            if slot&3 == 0{
                out.push_str(base);
                out.push_str(&id.to_string());
                out.push_str(".xyzw;\r\n");
                return out
            }
        }          
        if sv.ty != "float"{
            out.push_str(&sv.ty);
            out.push_str("(");       
        }

        // splat via loose props
        let svslots = match sv.ty.as_ref(){
            "float"=>1,
            "vec2"=>2,
            "vec3"=>3,
            _=>4
        };

        for i in 0..svslots{
            if i != 0{
                out.push_str(", ");
            }
            out.push_str(base);

            let id = (slot+i)>>2;
            let ext = (slot+i)&3;

            out.push_str(&id.to_string());
            out.push_str(
                match ext{
                    0=>{
                        if (id == total_slots>>2) && total_slots&3 == 1{
                            ""
                        }
                        else{
                            ".x"
                        }
                    }
                    1=>".y",
                    2=>".z",
                    _=>".w"
                }
            );
        }
        if sv.ty != "float"{
            out.push_str(")");
        }
        out.push_str(";\n");
        out
    }

    fn assemble_pack_chunk(base: &str, id:usize, chunk:&str, sv:&ShVar)->String{
        let mut out = String::new();
        out.push_str("    ");
        out.push_str(base);
        out.push_str(&id.to_string());
        out.push_str(chunk);
        out.push_str(&sv.name);
        out.push_str(";\n");
        out
    }

    fn assemble_pack(base: &str, slot:usize, total_slots:usize,sv:&ShVar)->String{
        // now we go the other way. we take slot and assign ShaderVar into it
        let mut out = String::new();
        let id = (slot)>>2;

        // just splat directly
        if sv.ty == "vec2"{
            match slot&3{
                0=>return Self::assemble_pack_chunk(base, id, ".xy =", &sv),
                1=>return Self::assemble_pack_chunk(base, id, ".yz =", &sv),
                2=>return Self::assemble_pack_chunk(base, id, ".zw =", &sv),
                _=>()            
            }
        }
        if sv.ty == "vec3"{
            match slot&3{
                0=>return Self::assemble_pack_chunk(base, id, ".xyz =", &sv),
                1=>return Self::assemble_pack_chunk(base, id, ".yzw =", &sv),
                _=>()            
            }
        }        
        if sv.ty == "vec4"{
            if slot&3 == 0{
                return Self::assemble_pack_chunk(base, id, ".xyzw =", &sv);
            }
        }          

       let svslots = match sv.ty.as_ref(){
            "float"=>1,
            "vec2"=>2,
            "vec3"=>3,
            _=>4
        };
        
        for i in 0..svslots{
            out.push_str("    ");
            out.push_str(base);
            let id = (slot+i)>>2;
            let ext = (slot+i)&3;
            out.push_str(&id.to_string());
            out.push_str(
                match ext{
                    0=>{
                        if (id == total_slots>>2) && total_slots&3 == 1{ // we are at last slot
                            ""
                        }
                        else{
                            ".x"
                        }
                    }
                    1=>".y",
                    2=>".z",
                    _=>".w"
                }
            );
            out.push_str(" = ");
            out.push_str(&sv.name);
            out.push_str(
                match i{
                    0=>{
                        if sv.ty == "float"{
                            ""
                        }
                        else{
                            ".x"
                        }
                    }
                    1=>".y",
                    2=>".z",
                    _=>".w"
                }
            );
            out.push_str(";\r\n");
        }
        out
    }

    pub fn assemble_shader(sh:&Shader)->Result<AssembledGLShader, SlErr>{
        let mut vtx_out = "#version 100\nprecision highp float;\n".to_string();
        // #extension GL_OES_standard_derivatives : enable
        let mut pix_out = "#version 100\nprecision highp float;\n".to_string();
        let compat_dfdxy = "vec2 dfdx(vec2 dummy){\nreturn vec2(0.05);\n}\nvec2 dfdy(vec2 dummy){\nreturn vec2(0.05);\n}\n";
        // ok now define samplers from our sh. 
        let texture_slots = sh.flat_vars(ShVarStore::Texture);
        let geometries = sh.flat_vars(ShVarStore::Geometry);
        let instances = sh.flat_vars(ShVarStore::Instance);
        let mut varyings = sh.flat_vars(ShVarStore::Varying);
        let locals = sh.flat_vars(ShVarStore::Local);
        let uniforms_cx = sh.flat_vars(ShVarStore::UniformCx);
        let uniforms_dl = sh.flat_vars(ShVarStore::UniformDl);
        let uniforms_dr = sh.flat_vars(ShVarStore::Uniform);

        let mut vtx_cx = SlCx{
            depth:0,
            target:SlTarget::Vertex,
            defargs_fn:"".to_string(),
            defargs_call:"".to_string(),
            call_prefix:"".to_string(),
            shader:sh,
            scope:Vec::new(),
            fn_deps:vec!["vertex".to_string()],
            fn_done:Vec::new(),
            auto_vary:Vec::new()
        };
        let vtx_fns = assemble_fn_and_deps(sh, &mut vtx_cx)?;

        let mut pix_cx = SlCx{
            depth:0,
            target:SlTarget::Pixel,
            defargs_fn:"".to_string(),
            defargs_call:"".to_string(),
            call_prefix:"".to_string(),
            shader:sh,
            scope:Vec::new(),
            fn_deps:vec!["pixel".to_string()],
            fn_done:Vec::new(),
            auto_vary:Vec::new()
        };        
        let pix_fns = assemble_fn_and_deps(sh, &mut pix_cx)?;
        
        for auto in &pix_cx.auto_vary{
            varyings.push(auto.clone());
        }

        // lets count the slots
        let geometry_slots = sh.compute_slot_total(&geometries);
        let instance_slots = sh.compute_slot_total(&instances);
        let varying_slots = sh.compute_slot_total(&varyings);
        let mut shared = String::new();
        shared.push_str("//Context uniforms\n");
        shared.push_str(&Self::assemble_uniforms(&uniforms_cx));
        shared.push_str("//DrawList uniforms\n");
        shared.push_str(&Self::assemble_uniforms(&uniforms_dl));
        shared.push_str("//Draw uniforms\n");
        shared.push_str(&Self::assemble_uniforms(&uniforms_dr));
        shared.push_str("//Texture slots\n");
        shared.push_str(&Self::assemble_texture_slots(&texture_slots));
        shared.push_str("// Varyings\n");
        shared.push_str(&Self::assemble_varblock("varying", "varying", varying_slots));

        for local in &locals{shared.push_str(&Self::assemble_vardef(&local));}

        pix_out.push_str(&shared);

        pix_out.push_str(compat_dfdxy);

        vtx_out.push_str(&shared);

        let mut vtx_main = "void main(){\n".to_string();
        let mut pix_main = "void main(){\n".to_string();

        vtx_out.push_str("// Geometry attributes\n");
        vtx_out.push_str(&Self::assemble_varblock("attribute", "geomattr", geometry_slots));
        let mut slot_id = 0;
        for geometry in &geometries{
            vtx_out.push_str(&Self::assemble_vardef(&geometry));
            vtx_main.push_str(&Self::assemble_unpack("geomattr", slot_id, geometry_slots, &geometry));
            slot_id += sh.get_type_slots(&geometry.ty);
        }

        vtx_out.push_str("// Instance attributes\n");
        vtx_out.push_str(&Self::assemble_varblock("attribute", "instattr", instance_slots));
        let mut slot_id = 0;
        for instance in &instances{
            vtx_out.push_str(&Self::assemble_vardef(&instance));
            vtx_main.push_str(&Self::assemble_unpack("instattr", slot_id, instance_slots, &instance));
            slot_id += sh.get_type_slots(&instance.ty);
        }


        vtx_main.push_str("\n    gl_Position = vertex();\n");
        vtx_main.push_str("\n    // Varying packing\n");

        pix_main.push_str("\n    // Varying unpacking\n");

        // alright lets pack/unpack varyings
        let mut slot_id = 0;
        for vary in &varyings{
            // only if we aren't already a geom/instance var
            if geometries.iter().find(|v|v.name == vary.name).is_none() &&
               instances.iter().find(|v|v.name == vary.name).is_none(){
                vtx_out.push_str(&Self::assemble_vardef(&vary));
            } 
            pix_out.push_str(&Self::assemble_vardef(&vary));
            // pack it in the vertexshader
            vtx_main.push_str( &Self::assemble_pack("varying", slot_id, varying_slots, &vary));
            // unpack it in the pixelshader
            pix_main.push_str( &Self::assemble_unpack("varying", slot_id, varying_slots, &vary));
            slot_id += sh.get_type_slots(&vary.ty);
        }

        pix_main.push_str("\n    gl_FragColor = pixel();\n");
        vtx_main.push_str("\n}\n\0");
        pix_main.push_str("\n}\n\0");

        vtx_out.push_str("//Vertex shader\n");
        vtx_out.push_str(&vtx_fns);
        pix_out.push_str("//Pixel shader\n");
        pix_out.push_str(&pix_fns);

        vtx_out.push_str("//Main function\n");
        vtx_out.push_str(&vtx_main);

        pix_out.push_str("//Main function\n");
        pix_out.push_str(&pix_main);

        println!("---------- Pixelshader:  ---------\n{}", pix_out);
        println!("---------- Vertexshader:  ---------\n{}", vtx_out);

        // we can also flatten our uniform variable set
        
        // lets composite our ShAst structure into a set of methods
        Ok(AssembledGLShader{
            geometry_slots:geometry_slots,
            instance_slots:instance_slots,
            geometry_attribs:Self::ceil_div4(geometry_slots),
            instance_attribs:Self::ceil_div4(instance_slots),
            uniforms_dr:uniforms_dr,
            uniforms_dl:uniforms_dl,
            uniforms_cx:uniforms_cx,
            texture_slots:texture_slots,
            fragment:pix_out,
            vertex:vtx_out
        })
    }

    pub fn compile_shader(sh:&Shader)->Result<CompiledShader, SlErr>{
        let ash = Self::assemble_shader(sh)?;
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
                assembled_shader:ash,
                ..Default::default()
            })
        }
    }

    pub fn create_vao(shgl:&CompiledShader)->GLInstanceVAO{
        // create the VAO
        let mut vao;
        let mut vb;
        unsafe{
            vao = mem::uninitialized();
            gl::GenVertexArrays(1, &mut vao);
            gl::BindVertexArray(vao);
            
            // bind the vertex and indexbuffers
            gl::BindBuffer(gl::ARRAY_BUFFER, shgl.geom_vb);
            for attr in &shgl.geom_attribs{
                gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                gl::EnableVertexAttribArray(attr.loc);
            }

            // create and bind the instance buffer
            vb = mem::uninitialized();
            gl::GenBuffers(1, &mut vb);
            gl::BindBuffer(gl::ARRAY_BUFFER, vb);
            
            for attr in &shgl.inst_attribs{
                gl::VertexAttribPointer(attr.loc, attr.size, gl::FLOAT, 0, attr.stride, attr.offset as *const () as *const _);
                gl::EnableVertexAttribArray(attr.loc);
                gl::VertexAttribDivisor(attr.loc, 1 as gl::types::GLuint);
            }

            // bind the indexbuffer
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, shgl.geom_ib);
            gl::BindVertexArray(0);
        }
        GLInstanceVAO{
            vao:vao,
            vb:vb
        }
    }

    pub fn destroy_vao(glivao:&mut GLInstanceVAO){
        unsafe{
            gl::DeleteVertexArrays(1, &mut glivao.vao);
            gl::DeleteBuffers(1, &mut glivao.vb);
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
                        16=>gl::UniformMatrix4fv(loc.loc, 1, 0, uni.as_ptr().offset((o*4) as isize)),
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