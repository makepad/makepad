
use metal::*;
use crate::cx::*;

#[derive(Clone)]
pub struct CxPlatformShader {
    pub library: metal::Library,
    pub pipeline_state: metal::RenderPipelineState,
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

impl Cx {
    pub fn mtl_compile_all_shaders(&mut self, metal_cx: &MetalCx) {
        for sh in &mut self.shaders {
            let mtlsh = Self::mtl_compile_shader(sh, metal_cx);
            if let Err(err) = mtlsh {
                panic!("Got metal shader compile error: {}", err.msg);
            }
        };
    }
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
        let texture_slots = sg.flat_vars(|v| if let ShVarStore::Texture = *v{true} else {false});
        let geometries = sg.flat_vars(|v| if let ShVarStore::Geometry = *v{true} else {false});
        let instances = sg.flat_vars(|v| if let ShVarStore::Instance(_) = *v{true} else {false});
        let mut varyings = sg.flat_vars(|v| if let ShVarStore::Varying = *v{true} else {false});
        let locals = sg.flat_vars(|v| if let ShVarStore::Local = *v{true} else {false});
        let uniforms_cx = sg.flat_vars(|v| if let ShVarStore::UniformCx = *v{true} else {false});
        let uniforms_vw = sg.flat_vars(|v| if let ShVarStore::UniformVw = *v{true} else {false});
        let uniforms_dr = sg.flat_vars(|v| if let ShVarStore::Uniform(_) = *v{true} else {false});
        
        // lets count the slots
        let geometry_slots = sg.compute_slot_total(&geometries);
        let instance_slots = sg.compute_slot_total(&instances);
        //let varying_slots = sh.compute_slot_total(&varyings);
        
        mtl_out.push_str(&Self::mtl_assemble_struct("_Geom", &geometries, PackType::Packed, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_Inst", &instances, PackType::Packed, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniCx", &uniforms_cx, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniVw", &uniforms_vw, PackType::Unpacked, ""));
        mtl_out.push_str(&Self::mtl_assemble_struct("_UniDr", &uniforms_dr, PackType::Unpacked, ""));
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
            defargs_fn: "_Tex _tex, thread _Loc &_loc, thread _Vary &_vary, thread _Geom &_geom, thread _Inst &_inst, device _UniCx &_uni_cx, device _UniVw &_uni_vw, device _UniDr &_uni_dr".to_string(),
            defargs_call: "_tex, _loc, _vary, _geom, _inst, _uni_cx, _uni_vw, _uni_dr".to_string(),
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
            defargs_fn: "_Tex _tex, thread _Loc &_loc, thread _Vary &_vary, device _UniCx &_uni_cx, device _UniVw &_uni_vw, device _UniDr &_uni_dr".to_string(),
            defargs_call: "_tex, _loc, _vary, _uni_cx, _uni_vw, _uni_dr".to_string(),
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
        mtl_out.push_str("  device _UniCx &_uni_cx [[buffer(2)]], device _UniVw &_uni_vw [[buffer(3)]], device _UniDr &_uni_dr [[buffer(4)]],\n");
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
        mtl_out.push_str("  device _UniCx &_uni_cx [[buffer(0)]], device _UniVw &_uni_vw [[buffer(1)]], device _UniDr &_uni_dr [[buffer(2)]]){\n");
        mtl_out.push_str("  _Loc _loc;\n");
        mtl_out.push_str("  return _pixel(");
        mtl_out.push_str(&pix_cx.defargs_call);
        mtl_out.push_str(");\n};\n");
        
        if sg.log != 0 {
            println!("---- Metal shader -----\n{}", mtl_out);
        }

        let uniform_props =  UniformProps::construct(sg, &uniforms_dr);
        Ok((mtl_out, CxShaderMapping {
            zbias_uniform_prop: uniform_props.find_zbias_uniform_prop(),
            rect_instance_props: RectInstanceProps::construct(sg, &instances),
            instance_props: InstanceProps::construct(sg, &instances),
            uniform_props: uniform_props,
            instances: instances,
            geometries: geometries,
            instance_slots: instance_slots,
            geometry_slots: geometry_slots,
            uniforms_dr: uniforms_dr,
            uniforms_vw: uniforms_vw,
            uniforms_cx: uniforms_cx,
            texture_slots: texture_slots,
        }))
    }
    
    pub fn mtl_compile_shader(sh: &mut CxShader, metal_cx: &MetalCx) -> Result<(), SlErr> {
        let (mtlsl, mapping) = Self::mtl_assemble_shader(&sh.shader_gen) ?;
        
        let options = CompileOptions::new();
        let library = metal_cx.device.new_library_with_source(&mtlsl, &options);
        
        match library {
            Err(library) => return Err(SlErr {msg: library}),
            Ok(library) => {
                sh.mapping = mapping;
                sh.platform = Some(CxPlatformShader {
                    pipeline_state: {
                        let vert = library.get_function("_vertex_shader", None).unwrap();
                        let frag = library.get_function("_fragment_shader", None).unwrap();
                        let rpd = RenderPipelineDescriptor::new();
                        rpd.set_vertex_function(Some(&vert));
                        rpd.set_fragment_function(Some(&frag));
                        //rpd.set_sample_count(2);
                        //rpd.set_alpha_to_coverage_enabled(false);
                        let color = rpd.color_attachments().object_at(0).unwrap();
                        color.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                        color.set_blending_enabled(true);
                        color.set_source_rgb_blend_factor(MTLBlendFactor::One);
                        color.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
                        color.set_source_alpha_blend_factor(MTLBlendFactor::One);
                        color.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
                        color.set_rgb_blend_operation(MTLBlendOperation::Add);
                        color.set_alpha_blend_operation(MTLBlendOperation::Add);
                        
                        rpd.set_depth_attachment_pixel_format(MTLPixelFormat::Depth32Float_Stencil8);
                        
                        metal_cx.device.new_render_pipeline_state(&rpd).unwrap()
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
        };
    }
}

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
    
    pub fn map_constructor(&self, name: &str, args: &Vec<Sl>)->String{
        let mut out = String::new();
        out.push_str(&self.map_type(name));
        out.push_str("(");
        for (i,arg) in args.iter().enumerate(){
            if i != 0{
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
            ShVarStore::Uniform(_) => return format!("_uni_dr.{}", var.name),
            ShVarStore::UniformColor(_) => return format!("_uni_col.{}", var.name),
            ShVarStore::UniformVw => return format!("_uni_vw.{}", var.name),
            ShVarStore::UniformCx => return format!("_uni_cx.{}", var.name),
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
}