use crate::shader::*;
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
            "dfdx"=>{
                return MapCallResult::Rename("dFdx".to_string())
            },
            "dfdy"=>{
                return MapCallResult::Rename("dFdy".to_string())
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
                    if self.auto_vary.iter().find(|v|v.name == var.name).is_none(){
                        self.auto_vary.push(var.clone());
                    }
                }
            },
            ShVarStore::Geometry=>{
                if let SlTarget::Pixel = self.target{
                    if self.auto_vary.iter().find(|v|v.name == var.name).is_none(){
                        self.auto_vary.push(var.clone());
                    }
                }
            },
            _=>()
        }
        var.name.clone()
    }
    
}

#[derive(Default,Clone)]
pub struct AssembledGLShader{
    pub geometry_slots:usize,
    pub instance_slots:usize,

    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_dl: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots:Vec<ShVar>,

    pub fragment:String,
    pub vertex:String,
    pub named_instance_props: NamedInstanceProps
}

pub fn gl_assemble_uniforms(unis:&Vec<ShVar>)->String{
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

pub fn ceil_div4(base:usize)->usize{
    let r = base >> 2;
    if base&3 != 0{
        return r + 1
    }
    r
}

pub fn gl_assemble_texture_slots(unis:&Vec<ShVar>)->String{
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

pub fn gl_assemble_vartype(i:usize, total:usize, left:usize)->String{
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


pub fn gl_assemble_varblock(thing: &str, base: &str, slots:usize)->String{
    // ok lets do a ceil
    let mut out = String::new();
    let total = ceil_div4(slots);
    for i in 0..total{
        out.push_str(thing);
        out.push_str(" ");
        out.push_str(&gl_assemble_vartype(i, total, slots&3));
        out.push_str(" ");
        out.push_str(base);
        out.push_str(&i.to_string());
        out.push_str(";\n");
    }
    out
}

pub fn gl_assemble_vardef(var:&ShVar)->String{
    // ok lets do a ceil
    let mut out = String::new();
    out.push_str(&var.ty);
    out.push_str(" ");
    out.push_str(&var.name);
    out.push_str(";\n");
    out
}

pub fn gl_assemble_unpack(base: &str, slot:usize, total_slots:usize,sv:&ShVar)->String{
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

pub fn gl_assemble_pack_chunk(base: &str, id:usize, chunk:&str, sv:&ShVar)->String{
    let mut out = String::new();
    out.push_str("    ");
    out.push_str(base);
    out.push_str(&id.to_string());
    out.push_str(chunk);
    out.push_str(&sv.name);
    out.push_str(";\n");
    out
}

pub fn gl_assemble_pack(base: &str, slot:usize, total_slots:usize,sv:&ShVar)->String{
    // now we go the other way. we take slot and assign ShaderVar into it
    let mut out = String::new();
    let id = (slot)>>2;

    // just splat directly
    if sv.ty == "vec2"{
        match slot&3{
            0=>return gl_assemble_pack_chunk(base, id, ".xy =", &sv),
            1=>return gl_assemble_pack_chunk(base, id, ".yz =", &sv),
            2=>return gl_assemble_pack_chunk(base, id, ".zw =", &sv),
            _=>()            
        }
    }
    if sv.ty == "vec3"{
        match slot&3{
            0=>return gl_assemble_pack_chunk(base, id, ".xyz =", &sv),
            1=>return gl_assemble_pack_chunk(base, id, ".yzw =", &sv),
            _=>()            
        }
    }        
    if sv.ty == "vec4"{
        if slot&3 == 0{
            return gl_assemble_pack_chunk(base, id, ".xyzw =", &sv);
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

pub enum GLShaderType{
    OpenGLNoPartialDeriv,
    OpenGL,
    WebGL1
}

pub fn gl_assemble_shader(sh:&Shader, shtype:GLShaderType)->Result<AssembledGLShader, SlErr>{
    
    let mut vtx_out = String::new();
    let mut pix_out = String::new();
    let mut pix_compat = String::new();
    match shtype{
        GLShaderType::OpenGLNoPartialDeriv=>{
            vtx_out.push_str("#version 100\n");
            pix_out.push_str("#version 100\n");
            vtx_out.push_str("precision highp float;\n");
            pix_out.push_str("precision highp float;\n");
            vtx_out.push_str("precision highp int;\n");
            pix_out.push_str("precision highp int;\n");
            pix_compat.push_str("vec2 dFdx(vec2 dummy){\nreturn vec2(0.05);\n}\n");
            pix_compat.push_str("vec2 dFdy(vec2 dummy){\nreturn vec2(0.05);\n}\n")
        }
        GLShaderType::OpenGL=>{
            vtx_out.push_str("#version 100\n");
            pix_out.push_str("#version 100\n");
            pix_out.push_str("#extension GL_OES_standard_derivatives : enable\n");
            vtx_out.push_str("precision highp float;\n");
            pix_out.push_str("precision highp float;\n");
            vtx_out.push_str("precision highp int;\n");
            pix_out.push_str("precision highp int;\n");
        },
        GLShaderType::WebGL1=>{
            pix_out.push_str("#extension GL_OES_standard_derivatives : enable\n");
            vtx_out.push_str("precision highp float;\n");
            pix_out.push_str("precision highp float;\n");
            vtx_out.push_str("precision highp int;\n");
            pix_out.push_str("precision highp int;\n");
        }
    }
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
    shared.push_str(&gl_assemble_uniforms(&uniforms_cx));
    shared.push_str("//DrawList uniforms\n");
    shared.push_str(&gl_assemble_uniforms(&uniforms_dl));
    shared.push_str("//Draw uniforms\n");
    shared.push_str(&gl_assemble_uniforms(&uniforms_dr));
    shared.push_str("//Texture slots\n");
    shared.push_str(&gl_assemble_texture_slots(&texture_slots));
    shared.push_str("// Varyings\n");
    shared.push_str(&gl_assemble_varblock("varying", "varying", varying_slots));

    for local in &locals{shared.push_str(&gl_assemble_vardef(&local));}

    pix_out.push_str(&shared);

    pix_out.push_str(&pix_compat);

    vtx_out.push_str(&shared);

    let mut vtx_main = "void main(){\n".to_string();
    let mut pix_main = "void main(){\n".to_string();

    vtx_out.push_str("// Geometry attributes\n");
    vtx_out.push_str(&gl_assemble_varblock("attribute", "geomattr", geometry_slots));
    let mut slot_id = 0;
    for geometry in &geometries{
        vtx_out.push_str(&gl_assemble_vardef(&geometry));
        vtx_main.push_str(&gl_assemble_unpack("geomattr", slot_id, geometry_slots, &geometry));
        slot_id += sh.get_type_slots(&geometry.ty);
    }

    vtx_out.push_str("// Instance attributes\n");
    vtx_out.push_str(&gl_assemble_varblock("attribute", "instattr", instance_slots));
    let mut slot_id = 0;
    for instance in &instances{
        vtx_out.push_str(&gl_assemble_vardef(&instance));
        vtx_main.push_str(&gl_assemble_unpack("instattr", slot_id, instance_slots, &instance));
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
            vtx_out.push_str(&gl_assemble_vardef(&vary));
        } 
        pix_out.push_str(&gl_assemble_vardef(&vary));
        // pack it in the vertexshader
        vtx_main.push_str( &gl_assemble_pack("varying", slot_id, varying_slots, &vary));
        // unpack it in the pixelshader
        pix_main.push_str( &gl_assemble_unpack("varying", slot_id, varying_slots, &vary));
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

    if sh.log != 0{
        println!("---------- Pixelshader:  ---------\n{}", pix_out);
        println!("---------- Vertexshader:  ---------\n{}", vtx_out);
    }
    // we can also flatten our uniform variable set
    
    // lets composite our ShAst structure into a set of methods
    Ok(AssembledGLShader{
        geometry_slots:geometry_slots,
        instance_slots:instance_slots,
        uniforms_dr:uniforms_dr,
        uniforms_dl:uniforms_dl,
        uniforms_cx:uniforms_cx,
        texture_slots:texture_slots,
        fragment:pix_out,
        vertex:vtx_out,
        named_instance_props:NamedInstanceProps::construct(sh, &instances)
    })
}
