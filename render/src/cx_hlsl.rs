use crate::cx::*;
use crate::cx_dx11::*;

#[derive(Default,Clone)]
pub struct AssembledHlslShader{
    pub instance_slots:usize,
    pub uniforms_dr: Vec<ShVar>,
    pub uniforms_dl: Vec<ShVar>,
    pub uniforms_cx: Vec<ShVar>,
    pub texture_slots:Vec<ShVar>,
    pub rect_instance_props: RectInstanceProps,
    pub named_uniform_props: NamedProps,
    pub named_instance_props: NamedProps,
    pub mtlsl:String,
}

#[derive(Default,Clone)]
pub struct CompiledShader{
    pub shader_id: usize,
    pub geom_vbuf:Dx11Buffer,
    pub geom_ibuf:Dx11Buffer,
    pub instance_slots:usize,
    pub rect_instance_props: RectInstanceProps,
    pub named_uniform_props: NamedProps,
    pub named_instance_props: NamedProps,
}

impl Cx{
     pub fn hlsl_compile_all_shaders(&mut self){
        for sh in &self.shaders{
            let mtlsh = Self::hlsl_compile_shader(&sh);
            if let Ok(mtlsh) = mtlsh{
                self.compiled_shaders.push(CompiledShader{
                    shader_id:self.compiled_shaders.len(),
                    ..mtlsh
                });
            }
            else if let Err(err) = mtlsh{
                println!("Got shader compile error: {}", err.msg);
                self.compiled_shaders.push(
                    CompiledShader{..Default::default()}
                )
            }
        };
    }

    pub fn hlsl_type(ty:&str)->String{
        match ty.as_ref(){
            "float"=>"float".to_string(),
            "vec2"=>"float2".to_string(),
            "vec3"=>"float3".to_string(),
            "vec4"=>"float4".to_string(),
            "mat2"=>"float2x2".to_string(),
            "mat3"=>"float3x3".to_string(),
            "mat4"=>"float4x4".to_string(),
            "texture2d"=>"Texture2D".to_string(),
            ty=>ty.to_string()
        }
    }

    pub fn hlsl_assemble_struct(lead:&str, name:&str, vars:&Vec<ShVar>, semantic:&str, field:&str)->String{
        let mut out = String::new();
        out.push_str(lead);
        out.push_str(" ");
        out.push_str(name);
        out.push_str("{\n");
        out.push_str(field);
        for (index,var) in vars.iter().enumerate(){
            out.push_str("  ");
            out.push_str(&Self::hlsl_type(&var.ty));
            out.push_str(" ");
            out.push_str(&var.name);
            if semantic.len()>0{
                out.push_str(": ");
                out.push_str(semantic);
                out.push_str(&format!("{}",index));
            }
            out.push_str(";\n")
        };
        out.push_str("};\n\n");
        out
    }

    pub fn mtl_assemble_texture_slots(textures:&Vec<ShVar>)->String{
        let mut out = String::new();
        for (i, tex) in textures.iter().enumerate(){
            out.push_str("Texture2D ");
            out.push_str(&tex.name);
            out.push_str(";\n");
        };
        out
    }

    pub fn hlsl_assemble_shader(_sh:&Shader)->Result<AssembledHlslShader, SlErr>{
        
        let mut hlsl_out = String::new();

        // ok now define samplers from our sh. 
        let texture_slots = sh.flat_vars(ShVarStore::Texture);
        let geometries = sh.flat_vars(ShVarStore::Geometry);
        let instances = sh.flat_vars(ShVarStore::Instance);
        let mut varyings = sh.flat_vars(ShVarStore::Varying);
        let locals = sh.flat_vars(ShVarStore::Local);
        let uniforms_cx = sh.flat_vars(ShVarStore::UniformCx); 
        let uniforms_dl = sh.flat_vars(ShVarStore::UniformDl);
        let uniforms_dr = sh.flat_vars(ShVarStore::Uniform);

        // lets count the slots
        //let geometry_slots = sh.compute_slot_total(&geometries);
        let instance_slots = sh.compute_slot_total(&instances);
        //let varying_slots = sh.compute_slot_total(&varyings);

        hlsl_out.push_str(&Self::hlsl_assemble_struct("struct", "_Geom", &geometries, "POSITION", ""));
        hlsl_out.push_str(&Self::hlsl_assemble_struct("struct", "_Inst", &instances, "TEXCOORD", ""));
        hlsl_out.push_str(&Self::mtl_assemble_struct("cbuffer", "_UniCx", &uniforms_cx, "", ""));
        hlsl_out.push_str(&Self::mtl_assemble_struct("cbuffer", "_UniDl", &uniforms_dl, "", ""));
        hlsl_out.push_str(&Self::mtl_assemble_struct("cbuffer", "_UniDr", &uniforms_dr, "", ""));
        hlsl_out.push_str(&Self::mtl_assemble_struct("struct", "_Loc", &locals, "", ""));

        // we need to figure out which texture slots exist 
        hlsl_out.push_str(&Self::hlsl_assemble_texture_slots(&texture_slots));
        // we need to figure out which texture slots exist 
        // mtl_out.push_str(&Self::assemble_constants(&texture_slots));
        
        let mut const_cx = SlCx{
            depth:0,
            target:SlTarget::Constant,
            defargs_fn:"".to_string(),
            defargs_call:"".to_string(),
            call_prefix:"_".to_string(),
            shader:sh,
            scope:Vec::new(),
            fn_deps:Vec::new(),
            fn_done:Vec::new(),
            auto_vary:Vec::new()
        };
        let consts = sh.flat_consts();
        for cnst in &consts{
            let const_init = assemble_const_init(cnst, &mut const_cx)?;
            hlsl_out.push_str("#define ");
            hlsl_out.push_str(" ");
            hlsl_out.push_str(&cnst.name);
            hlsl_out.push_str(" (");
            hlsl_out.push_str(&const_init.sl);
            hlsl_out.push_str(")\n");
        }

        let mut vtx_cx = SlCx{
            depth:0,
            target:SlTarget::Vertex,
            defargs_fn:"inout _Loc _loc, inout _Vary _vary, in _Geom _geom, in _Inst _inst, in _UniCx _uni_cx, in _UniDl _uni_dl, in _UniDr _uni_dr".to_string(),
            defargs_call:"_loc, _vary, _geom, _inst, _uni_cx, _uni_dl, _uni_dr".to_string(),
            call_prefix:"_".to_string(),
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
            defargs_fn:"inout _Loc _loc, inout _Vary _vary, in _UniCx _uni_cx, in _UniDl _uni_dl, in _UniDr _uni_dr".to_string(),
            defargs_call:"_loc, _vary, _uni_cx, _uni_dl, _uni_dr".to_string(),
            call_prefix:"_".to_string(),
            shader:sh,
            scope:Vec::new(),
            fn_deps:vec!["pixel".to_string()],
            fn_done:vtx_cx.fn_done,
            auto_vary:Vec::new()
        };

        let pix_fns = assemble_fn_and_deps(sh, &mut pix_cx)?;

        // lets add the auto_vary ones to the varyings struct
        for auto in &pix_cx.auto_vary{
            varyings.push(auto.clone());
        }
        hlsl_out.push_str(&Self::mtl_assemble_struct("_Vary", &varyings, false, "  float4 hlsl_position : SV_POSITION;\n"));

        hlsl_out.push_str("//Vertex shader\n");
        hlsl_out.push_str(&vtx_fns);
        hlsl_out.push_str("//Pixel shader\n");
        hlsl_out.push_str(&pix_fns);

        // lets define the vertex shader
        hlsl_out.push_str("_Vary _vertex_shader(_Geom _geom, _Inst _inst, uint inst_id: SV_InstanceID){\n");
        hlsl_out.push_str("  _Loc _loc;\n");
        hlsl_out.push_str("  _Vary _vary;\n");
        hlsl_out.push_str("  _vary.mtl_position = _vertex(");
        hlsl_out.push_str(&vtx_cx.defargs_call);
        hlsl_out.push_str(");\n\n");

        for auto in pix_cx.auto_vary{
            if let ShVarStore::Geometry = auto.store{
              hlsl_out.push_str("       _vary.");
              hlsl_out.push_str(&auto.name);
              hlsl_out.push_str(" = _geom.");
              hlsl_out.push_str(&auto.name);
              hlsl_out.push_str(";\n");
            }
            else if let ShVarStore::Instance = auto.store{
              hlsl_out.push_str("       _vary.");
              hlsl_out.push_str(&auto.name);
              hlsl_out.push_str(" = _inst.");
              hlsl_out.push_str(&auto.name);
              hlsl_out.push_str(";\n");
            }
        }

        hlsl_out.push_str("       return _vary;\n");
        hlsl_out.push_str("};\n");
        // then the fragment shader
        hlsl_out.push_str("float4 _fragment_shader(_Vary _vary) :  SV_TARGET{\n");
        hlsl_out.push_str("  _Loc _loc;\n");
        hlsl_out.push_str("  return _pixel(");
        hlsl_out.push_str(&pix_cx.defargs_call);
        hlsl_out.push_str(");\n};\n");

        //if sh.log != 0{
            println!("---- HLSL shader -----\n{}",hlsl_out);
        //}

         Ok(AssembledMtlShader{
            rect_instance_props:RectInstanceProps::construct(sh, &instances),
            named_instance_props:NamedProps::construct(sh, &instances),
            named_uniform_props:NamedProps::construct(sh, &uniforms_dr),
            instance_slots:instance_slots,
            uniforms_dr:uniforms_dr,
            uniforms_dl:uniforms_dl,
            uniforms_cx:uniforms_cx,
            texture_slots:texture_slots,
            hlsl:hlsl_out
        })
        Err(SlErr{msg:"".to_string()})
    }

    pub fn hlsl_compile_shader(sh:&Shader)->Result<CompiledShader, SlErr>{
        let _ash = Self::hlsl_assemble_shader(sh)?;
        Err(SlErr{msg:"".to_string()})
/*
        let options = CompileOptions::new();
        let library = device.new_library_with_source(&ash.mtlsl, &options);

        match library{
            Err(library)=>Err(SlErr{msg:library}),
            Ok(library)=>Ok(CompiledShader{
                shader_id:0,
                pipeline_state:{
                    let vert = library.get_function("_vertex_shader", None).unwrap();
                    let frag = library.get_function("_fragment_shader", None).unwrap();
                    let rpd = RenderPipelineDescriptor::new();
                    rpd.set_vertex_function(Some(&vert));
                    rpd.set_fragment_function(Some(&frag));
                    let color = rpd.color_attachments().object_at(0).unwrap();
                    color.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    color.set_blending_enabled(true);
                    color.set_source_rgb_blend_factor(MTLBlendFactor::One);
                    color.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
                    color.set_source_alpha_blend_factor(MTLBlendFactor::One);
                    color.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
                    color.set_rgb_blend_operation(MTLBlendOperation::Add);
                    color.set_alpha_blend_operation(MTLBlendOperation::Add);
                    Some(device.new_render_pipeline_state(&rpd).unwrap())
                },
                library:Some(library),
                instance_slots:ash.instance_slots,
                named_instance_props:ash.named_instance_props.clone(),
                named_uniform_props:ash.named_uniform_props.clone(),
                rect_instance_props:ash.rect_instance_props.clone(),
                //assembled_shader:ash,
                geom_ibuf:{
                    let mut geom_ibuf = MetalBuffer{..Default::default()};
                    geom_ibuf.update_with_u32_data(device, &sh.geometry_indices);
                    geom_ibuf
                },
                geom_vbuf:{
                    let mut geom_vbuf = MetalBuffer{..Default::default()};
                    geom_vbuf.update_with_f32_data(device, &sh.geometry_vertices);
                    geom_vbuf
                }
            })
        }*/
    }
}


impl<'a> SlCx<'a>{
    pub fn map_call(&self, name:&str, args:&Vec<Sl>)->MapCallResult{
        match name{
            "sample2d"=>{ // transform call to
                let base = &args[0];
                let coord = &args[1];
                return MapCallResult::Rewrite(
                    format!("{}.sample(sampler(mag_filter::linear,min_filter::linear),{})", base.sl, coord.sl),
                    "vec4".to_string()
                )
            },
            "color"=>{
                let col = color(&args[0].sl);
                return MapCallResult::Rewrite(
                    format!("float4({},{},{},{})", col.r, col.g, col.b, col.a),
                    "vec4".to_string()
                );
            },
            _=>return MapCallResult::None
        }
    }

    pub fn map_type(&self, ty:&str)->String{
        Cx::hlsl_type(ty)
    }

    pub fn map_var(&mut self, var:&ShVar)->String{
        //let mty = Cx::hlsl_type(&var.ty);
        match var.store{
            ShVarStore::Uniform=>return format!("_uni_dr.{}", var.name),
            ShVarStore::UniformDl=>return format!("_uni_dl.{}", var.name),
            ShVarStore::UniformCx=>return format!("_uni_cx.{}", var.name),
            ShVarStore::Instance=>{
                if let SlTarget::Pixel = self.target{
                    if self.auto_vary.iter().find(|v|v.name == var.name).is_none(){
                        self.auto_vary.push(var.clone());
                    }
                    return format!("_vary.{}",var.name);
                }
                else{
                    return format!("_inst.{}", var.name);
                }
            },
            ShVarStore::Geometry=>{
                if let SlTarget::Pixel = self.target{
                    if self.auto_vary.iter().find(|v|v.name == var.name).is_none(){
                        self.auto_vary.push(var.clone());
                    }
                    return format!("_vary.{}",var.name);
                }
                else{
                    
                    return format!("_geom.{}", var.name);
                }
            },
            ShVarStore::Texture=>return format!("_tex.{}",var.name),
            ShVarStore::Local=>return format!("_loc.{}",var.name),
            ShVarStore::Varying=>return format!("_vary.{}",var.name),
        }
    }
}