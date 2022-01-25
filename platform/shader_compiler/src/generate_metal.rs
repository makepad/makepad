use {
    std::{
        fmt::Write,
        fmt,
        collections::{BTreeMap, BTreeSet}
    },
    crate::{
        makepad_live_compiler::*,
        shader_ast::*,
        generate::*,
        shader_registry::ShaderRegistry,
    }
};

pub struct MetalGeneratedShader{
    pub mtlsl: String,
    pub fields_as_uniform_blocks:BTreeMap<Ident, Vec<(usize, Ident) >>   
}

pub fn generate_shader(draw_shader_def: &DrawShaderDef, const_table:&DrawShaderConstTable, shader_registry: &ShaderRegistry) -> MetalGeneratedShader {
    let mut string = String::new();
    let fields_as_uniform_blocks = draw_shader_def.fields_as_uniform_blocks();
    DrawShaderGenerator {
        draw_shader_def,
        shader_registry,
        const_table,
        string: &mut string,
        fields_as_uniform_blocks: &fields_as_uniform_blocks,
        backend_writer: &MetalBackendWriter {shader_registry, draw_shader_def, const_table}
    }
    .generate_shader();
    MetalGeneratedShader{
        mtlsl:string, 
        fields_as_uniform_blocks
    }
}

struct DrawShaderGenerator<'a> {
    draw_shader_def: &'a DrawShaderDef,
    shader_registry: &'a ShaderRegistry,
    string: &'a mut String,
    fields_as_uniform_blocks: &'a BTreeMap<Ident, Vec<(usize, Ident) >>,
    backend_writer: &'a dyn BackendWriter,
    const_table: &'a DrawShaderConstTable
}

impl<'a> DrawShaderGenerator<'a> {
    fn generate_shader(&mut self) {
        writeln!(self.string, "#include <metal_stdlib>").unwrap();
        writeln!(self.string, "using namespace metal;").unwrap();
        
        for fn_iter in self.draw_shader_def.all_fns.borrow().iter() {
            let fn_def = self.shader_registry.all_fns.get(fn_iter).unwrap();
            if fn_def.builtin_deps.borrow().as_ref().unwrap().contains(&Ident(id!(sample2d))) {
                writeln!(self.string, "float4 sample2d(texture2d<float> tex, float2 pos){{return tex.sample(sampler(mag_filter::linear,min_filter::linear),pos);}}").unwrap();
                break;
            }
        };
        
        self.generate_struct_defs();
        //let fields_as_uniform_blocks = self.draw_shader_def.fields_as_uniform_blocks();
        self.generate_uniform_structs();
        self.generate_texture_struct();
        self.generate_geometry_struct();
        self.generate_instance_struct();
        self.generate_varying_struct();
        
        let vertex_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap();
        let pixel_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(pixel))).unwrap();
        
        for &(ty_lit, ref param_tys) in pixel_def
            .constructor_fn_deps
            .borrow_mut()
            .as_ref()
            .unwrap()
            .union(vertex_def.constructor_fn_deps.borrow().as_ref().unwrap())
        {
            generate_cons_fn(self.backend_writer, self.string, ty_lit, &param_tys);
        }
        
        let all_fns = self.draw_shader_def.all_fns.borrow();
        for fn_iter in all_fns.iter().rev() {
            let const_table_offset = self.const_table.offsets.get(fn_iter).cloned();
            let fn_def = self.shader_registry.all_fns.get(fn_iter).unwrap();
            if fn_def.has_closure_args() {
                for call_iter in all_fns.iter().rev() {
                    // any function that depends on us, will have the closures we need
                    let call_def = self.shader_registry.all_fns.get(call_iter).unwrap();
                    if call_def.callees.borrow().as_ref().unwrap().contains(&fn_iter) {
                        FnDefWithClosureArgsGenerator::generate_fn_def_with_all_closures(
                            &mut self.string,
                            self.shader_registry,
                            fn_def,
                            call_def,
                            self.backend_writer,
                            const_table_offset
                        );
                    }
                }
                continue
            }
            FnDefGenerator {
                fn_def,
                const_table_offset,
                shader_registry: self.shader_registry,
                backend_writer: self.backend_writer,
                string: self.string,
            }
            .generate_fn_def()
        }
        self.generate_vertex_main();
        self.generate_pixel_main();
    }
    
    fn generate_struct_defs(&mut self) {
        // we have all the structs already from analyse
        for struct_ptr in self.draw_shader_def.all_structs.borrow().iter().rev() {
            let struct_def = self.shader_registry.structs.get(struct_ptr).unwrap();
            write!(self.string, "struct {} {{", struct_ptr).unwrap();
            if !struct_def.fields.is_empty() {
                writeln!(self.string).unwrap();
                for field in &struct_def.fields {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(&DisplayStructField(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                    writeln!(self.string, ";").unwrap();
                }
            }
            writeln!(self.string, "}};").unwrap();
        }
    }
    
    fn generate_uniform_structs(&mut self,) {
        writeln!(self.string, "struct LiveUniforms {{").unwrap();
        for (value_node_ptr, ty) in self.draw_shader_def.all_live_refs.borrow().iter() {
            // we have a span and an ident_path.
            // lets fully qualify it
            write!(self.string, "    ").unwrap();
            self.write_ty_lit(ty.maybe_ty_lit().unwrap());
            write!(self.string, " ").unwrap();
            write!(self.string, "{}", value_node_ptr).unwrap();
            writeln!(self.string, ";").unwrap();
        }
        writeln!(self.string, "}};").unwrap();
        
        for (ident, vec) in self.fields_as_uniform_blocks {
            writeln!(self.string, "struct Uniforms_{} {{", ident).unwrap();
            for (index, _item) in vec {
                let field = &self.draw_shader_def.fields[*index];
                write!(self.string, "    ").unwrap();
                self.write_var_decl(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                writeln!(self.string, ";").unwrap();
            }
            writeln!(self.string, "}};").unwrap();
        }
    }
    
    fn generate_texture_struct(&mut self) {
        let mut index = 0;
        writeln!(self.string, "struct Textures {{").unwrap();
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Texture {..} => {
                    assert_eq!(*field.ty_expr.ty.borrow().as_ref().unwrap(), Ty::Texture2D);
                    write!(self.string, "    texture2d<float> ").unwrap();
                    write!(self.string, "{}", &DisplayDsIdent(field.ident)).unwrap();
                    write!(self.string, " [[texture({})]];", index).unwrap();
                    index += 1;
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_geometry_struct(&mut self) {
        writeln!(self.string, "struct Geometries {{").unwrap();
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl_packed(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                    writeln!(self.string, ";").unwrap();
                }
                _ => ()
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_instance_struct(&mut self) {
        let mut padding = 0;
        writeln!(self.string, "struct Instances {{").unwrap();
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Instance {..} => {
                    match field.ty_expr.ty.borrow().as_ref().unwrap() {
                        Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => {
                            write!(self.string, "    ").unwrap();
                            if field.ident == Ident(LiveId(0)){
                                self.write_var_decl_packed(&DisplayPadding(padding), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                                padding += 1;
                            }
                            else{
                                self.write_var_decl_packed(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                            }
                            writeln!(self.string, ";").unwrap();
                            //write!(self.string, "    ").unwrap();
                            //self.write_var_decl(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                            //writeln!(self.string, ";").unwrap();
                        },
                        Ty::Mat4 => {
                            for i in 0..4 {
                                write!(self.string, "    ").unwrap();
                                self.write_var_decl_packed(&DisplayDsIdent(field.ident), &Ty::Vec4);
                                writeln!(self.string, " {};", i).unwrap();
                            }
                        },
                        Ty::Mat3 => {
                            for i in 0..3 {
                                write!(self.string, "    ").unwrap();
                                self.write_var_decl_packed(&DisplayDsIdent(field.ident), &Ty::Vec3);
                                writeln!(self.string, " {};", i).unwrap();
                            }
                        },
                        Ty::Mat2 => {
                            write!(self.string, "    ").unwrap();
                            self.write_var_decl_packed(&DisplayDsIdent(field.ident), &Ty::Vec4);
                            writeln!(self.string, ";").unwrap();
                        },
                        Ty::Enum(v) =>{
                            write!(self.string, "    ").unwrap();
                            self.write_var_decl_packed(&DisplayDsIdent(field.ident), &Ty::Enum(*v));
                            writeln!(self.string, ";").unwrap();
                        }
                        _ => panic!("unsupported type in generate_instance_struct")
                    }
                }
                /*                
                DrawShaderFieldKind::Instance {..} => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl_packed(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }*/
                _ => ()
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_varying_struct(&mut self) {
        writeln!(self.string, "struct Varyings {{").unwrap();
        writeln!(self.string, "    float4 position [[position]];").unwrap();
        for field in &self.draw_shader_def.fields {
            match &field.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    match field.ty_expr.ty.borrow().as_ref().unwrap() {
                        Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => {
                            write!(self.string, "    ").unwrap();
                            self.write_var_decl(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                            writeln!(self.string, ";").unwrap();
                        },
                        Ty::Mat4 => {
                            for i in 0..4 {
                                write!(self.string, "    ").unwrap();
                                self.write_ty_lit(TyLit::Vec4);
                                writeln!(self.string, " {}{};", &DisplayDsIdent(field.ident), i).unwrap();
                            }
                        },
                        Ty::Mat3 => {
                            for i in 0..3 {
                                write!(self.string, "    ").unwrap();
                                self.write_ty_lit(TyLit::Vec3);
                                writeln!(self.string, " {}{};", &DisplayDsIdent(field.ident), i).unwrap();
                            }
                        },
                        Ty::Mat2 => {
                            write!(self.string, "    ").unwrap();
                            self.write_ty_lit(TyLit::Vec4);
                            writeln!(self.string, " {};", &DisplayDsIdent(field.ident)).unwrap();
                        },
                        Ty::Enum(v) =>{
                            write!(self.string, "    ").unwrap();
                            self.write_var_decl_packed(&DisplayDsIdent(field.ident), &Ty::Enum(*v));
                            writeln!(self.string, ";").unwrap();
                        }
                        _ => panic!("unsupported type in generate_varying_struct")
                    }
                }
                DrawShaderFieldKind::Varying {..} => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(&DisplayDsIdent(field.ident), field.ty_expr.ty.borrow().as_ref().unwrap(),);
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_fn_def(&mut self, fn_def: &FnDef, const_table_offset: Option<usize>) {
        FnDefGenerator {
            fn_def,
            shader_registry: self.shader_registry,
            const_table_offset,
            backend_writer: self.backend_writer,
            string: self.string,
        }
        .generate_fn_def()
    }
    
    fn generate_vertex_main(&mut self) {
        
        write!(self.string, "vertex Varyings vertex_main(").unwrap();
        writeln!(self.string, "Textures textures").unwrap();
        writeln!(self.string, ", const device Geometries *in_geometries [[buffer(0)]]").unwrap();
        writeln!(self.string, ", const device Instances *in_instances [[buffer(1)]]").unwrap();
        writeln!(self.string, ", constant LiveUniforms &live_uniforms [[buffer(2)]]").unwrap();
        writeln!(self.string, ", constant const float *const_table [[buffer(3)]]").unwrap();
        let mut buffer_id = 4;
        for (field, _set) in self.fields_as_uniform_blocks {
            writeln!(self.string, ", constant Uniforms_{0} &uniforms_{0} [[buffer({1})]]", field, buffer_id).unwrap();
            buffer_id += 1;
        }
        writeln!(self.string, ", uint vtx_id [[vertex_id]]").unwrap();
        writeln!(self.string, ", uint inst_id [[instance_id]]").unwrap();
        writeln!(self.string, ") {{").unwrap();
        writeln!(
            self.string,
            "    Geometries geometries = in_geometries[vtx_id];"
        ).unwrap();
        writeln!(
            self.string,
            "    Instances instances = in_instances[inst_id];"
        ).unwrap();
        writeln!(self.string, "    Varyings varyings;").unwrap();
        
        for decl in &self.draw_shader_def.fields {
            match &decl.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    writeln!(self.string, "    varyings.{0} = geometries.{0};", DisplayDsIdent(decl.ident)).unwrap();
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    match decl.ty_expr.ty.borrow().as_ref().unwrap() {
                        Ty::Mat4 => {
                            for i in 0..4 {
                                writeln!(self.string, "    varyings.{0}{1} = instances.{0}{1};", DisplayDsIdent(decl.ident), i).unwrap();
                            }
                        }
                        Ty::Mat3 => {
                            for i in 0..3 {
                                writeln!(self.string, "    varyings.{0}{1} = instances.{0}{1};", DisplayDsIdent(decl.ident), i).unwrap();
                            }
                        }
                        _ => {
                            writeln!(self.string, "    varyings.{0} = instances.{0};", DisplayDsIdent(decl.ident)).unwrap();
                        }
                    }
                }
                _ => {}
            }
        }
        
        let vertex_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap();
        write!(self.string, "    varyings.position = {}", DisplayFnName(vertex_def.fn_ptr, vertex_def.ident)).unwrap();
        
        write!(self.string, "(").unwrap();
        self.backend_writer.write_call_expr_hidden_args(self.string, vertex_def.hidden_args.borrow().as_ref().unwrap(), "");
        
        writeln!(self.string, ");").unwrap();
        
        writeln!(self.string, "    return varyings;").unwrap();
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_pixel_main(&mut self) {
        
        write!(self.string, "fragment float4 fragment_main(").unwrap();
        writeln!(self.string, "Varyings varyings[[stage_in]]").unwrap();
        writeln!(self.string, ", Textures textures").unwrap();
        writeln!(self.string, ", constant LiveUniforms &live_uniforms [[buffer(2)]]").unwrap();
        writeln!(self.string, ", constant const float *const_table [[buffer(3)]]").unwrap();
        let mut buffer_id = 4;
        for (field, _set) in self.fields_as_uniform_blocks {
            writeln!(self.string, ", constant Uniforms_{0} &uniforms_{0} [[buffer({1})]]", field, buffer_id).unwrap();
            buffer_id += 1;
        }
        
        writeln!(self.string, ") {{").unwrap();
        
        write!(self.string, "    return ").unwrap();
        
        let pixel_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(pixel))).unwrap();
        write!(self.string, "    {}", DisplayFnName(pixel_def.fn_ptr, pixel_def.ident)).unwrap();
        
        write!(self.string, "(").unwrap();
        self.backend_writer.write_call_expr_hidden_args(self.string, pixel_def.hidden_args.borrow().as_ref().unwrap(), "");
        
        writeln!(self.string, ");").unwrap();
        
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            fn_def: None,
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            const_table_offset: None,
            backend_writer: self.backend_writer,
            string: self.string,
        }
        .generate_expr(expr)
    }
    
    fn write_var_decl_packed(&mut self, ident: &dyn fmt::Display, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, "", false, true, ident, ty);
    }
    
    fn write_var_decl(&mut self, ident: &dyn fmt::Display, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, "", false, false, ident, ty);
    }
    
    fn write_ty_lit(&mut self, ty_lit: TyLit) {
        self.backend_writer.write_ty_lit(&mut self.string, ty_lit);
    }
    
}

struct MetalBackendWriter<'a> {
    pub shader_registry: &'a ShaderRegistry,
    pub draw_shader_def: &'a DrawShaderDef,
    pub const_table: &'a DrawShaderConstTable,
}

impl<'a> BackendWriter for MetalBackendWriter<'a> {
    
    fn needs_cstyle_struct_cons(&self) -> bool {
        false
    }
    
    fn needs_mul_fn_for_matrix_multiplication(&self) -> bool {
        false
    }
    
    fn needs_unpack_for_matrix_multiplication(&self) -> bool {
        true
    }
    
    fn const_table_is_vec4(&self) -> bool {
        false
    }
    
    
    fn use_cons_fn(&self, what: &str) -> bool {
        match what {
            "mpsc_mat3_mat4" => true,
            "mpsc_mat2_mat4" => true,
            "mpsc_mat2_mat3" => true,
            _ => false
        }
    }
    
    fn write_var_decl(
        &self,
        string: &mut String,
        sep: &'static str,
        is_inout: bool,
        is_packed: bool,
        ident: &dyn fmt::Display,
        ty: &Ty,
    ) -> bool {
        let ref_prefix = if is_inout {
            "&"
        } else {
            ""
        };
        
        fn prefix(string: &mut String, sep: &'static str, is_inout: bool) {
            write!(string, "{}", sep).unwrap();
            if is_inout {
                write!(string, "thread ").unwrap();
            }
        }
        
        let packed_prefix = if is_packed {"packed_"} else {""};
        match *ty {
            Ty::Void => {
                write!(string, "{}void {}", sep, ident).unwrap();
            }
            Ty::Bool => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Bool);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Int => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Int);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Float => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Float);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Bvec2 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Bvec2);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Bvec3 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec3);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Bvec4 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec4);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Ivec2 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec2);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Ivec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec3);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Ivec4 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec4);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Vec2 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec2);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Vec3 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec3);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Vec4 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec4);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Mat2 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Mat2);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Mat3 => {
                prefix(string, sep, is_inout);
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Mat3);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Mat4 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Mat4);
                write!(string, " {}{}", ref_prefix, ident).unwrap();
            }
            Ty::Texture2D => panic!(), // TODO
            Ty::Array {ref elem_ty, len} => {
                self.write_var_decl(string, sep, is_inout, is_packed, ident, elem_ty);
                write!(string, "[{}]", len).unwrap();
            }
            Ty::Struct(struct_node_ptr) => {
                prefix(string, sep, is_inout);
                write!(string, "{} {} {}", struct_node_ptr, ref_prefix, ident).unwrap();
            }
            Ty::Enum(_) => {
                prefix(string, sep, is_inout);
                write!(string, "uint32_t {} {}", ref_prefix, ident).unwrap();
            }
            Ty::DrawShader(_) => {
                return false
            }
            Ty::ClosureDef {..} => {
                return false
            }
            Ty::ClosureDecl => {
                return false
            }
        }
        true
    }
    
    fn write_call_expr_hidden_args(&self, string: &mut String, hidden_args: &BTreeSet<HiddenArgKind >, sep: &str) {
        let mut sep = sep;
        if self.const_table.table.len()>0 {
            write!(string, "{}", sep).unwrap();
            sep = ", ";
            write!(string, "const_table").unwrap();
        }
        
        for hidden_arg in hidden_args {
            write!(string, "{}", sep).unwrap();
            match hidden_arg {
                HiddenArgKind::Geometries => {
                    write!(string, "geometries").unwrap();
                }
                HiddenArgKind::Instances => {
                    write!(string, "instances").unwrap();
                }
                HiddenArgKind::Varyings => {
                    write!(string, "varyings").unwrap();
                }
                HiddenArgKind::Textures => {
                    write!(string, "textures").unwrap();
                }
                HiddenArgKind::Uniform(ident) => {
                    write!(string, "uniforms_{}", ident).unwrap();
                }
                HiddenArgKind::LiveUniforms => {
                    write!(string, "live_uniforms").unwrap();
                }
            }
            sep = ", ";
        }
    }
    
    fn write_fn_def_hidden_params(&self, string: &mut String, hidden_args: &BTreeSet<HiddenArgKind >, sep: &str) {
        let mut sep = sep;
        if self.const_table.table.len()>0 {
            write!(string, "{}", sep).unwrap();
            sep = ", ";
            write!(string, "constant const float *const_table").unwrap();
        }
        
        for hidden_arg in hidden_args {
            write!(string, "{}", sep).unwrap();
            match hidden_arg {
                HiddenArgKind::Geometries => {
                    write!(string, "thread Geometries &geometries").unwrap();
                }
                HiddenArgKind::Instances => {
                    write!(string, "thread Instances &instances").unwrap();
                }
                HiddenArgKind::Varyings => {
                    write!(string, "thread Varyings &varyings").unwrap();
                }
                HiddenArgKind::Textures => {
                    write!(string, "Textures textures").unwrap();
                }
                HiddenArgKind::Uniform(ident) => {
                    write!(string, "constant Uniforms_{0} &uniforms_{0}", ident).unwrap();
                }
                HiddenArgKind::LiveUniforms => {
                    write!(string, "constant LiveUniforms &live_uniforms").unwrap();
                }
            }
            sep = ", ";
        }
    }
    
    fn generate_live_value_prefix(&self, string: &mut String) {
        write!(string, "live_uniforms.").unwrap();
    }
    
    fn generate_draw_shader_field_expr(&self, string: &mut String, field_ident: Ident, ty: &Ty) {
        let field_def = self.draw_shader_def.find_field(field_ident).unwrap();
        
        match &field_def.kind {
            DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} => {
                if is_used_in_pixel_shader.get() {
                    write!(string, "varyings.").unwrap()
                }
                else {
                    write!(string, "geometries.").unwrap()
                }
            }
            DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} => {
                let prefix = if is_used_in_pixel_shader.get() {
                    "varyings"
                }
                else {
                    "instances"
                };
                
                match ty {
                    Ty::Mat4 => {
                        write!(string, "float4x4(").unwrap();
                        for i in 0..4 {
                            for j in 0..4 {
                                if i != 0 || j != 0 {
                                    write!(string, ",").unwrap();
                                }
                                write!(string, "{}.", prefix).unwrap();
                                write!(string, "{}{}", DisplayDsIdent(field_ident), j).unwrap();
                                match i {
                                    0 => write!(string, ".x").unwrap(),
                                    1 => write!(string, ".y").unwrap(),
                                    2 => write!(string, ".z").unwrap(),
                                    _ => write!(string, ".w").unwrap()
                                }
                            }
                        }
                        write!(string, ")").unwrap();
                        return
                    },
                    Ty::Mat3 => {
                        write!(string, "float3x3(").unwrap();
                        for i in 0..3 {
                            for j in 0..3 {
                                if i != 0 || j != 0 {
                                    write!(string, ",").unwrap();
                                }
                                write!(string, "{}.", prefix).unwrap();
                                write!(string, "{}{}", DisplayDsIdent(field_ident), j).unwrap();
                                match i {
                                    0 => write!(string, ".x").unwrap(),
                                    1 => write!(string, ".y").unwrap(),
                                    _ => write!(string, ".z").unwrap(),
                                }
                            }
                        }
                        write!(string, ")").unwrap();
                        return
                    },
                    Ty::Mat2 => {
                        write!(string, "float2x2({0}.{1}.x, {0}.{1}.y, {0}.{1}.z, {0}.{1}.w)", prefix, DisplayDsIdent(field_ident)).unwrap();
                        return
                    },
                    _ => {
                        write!(string, "{}.",prefix).unwrap();
                    }
                }
            }
            DrawShaderFieldKind::Varying {..} => {
                write!(string, "varyings.").unwrap()
            }
            DrawShaderFieldKind::Texture {..} => {
                write!(string, "textures.").unwrap()
            }
            DrawShaderFieldKind::Uniform {block_ident, ..} => {
                write!(string, "uniforms_{}.", block_ident).unwrap()
            }
        }
        write!(string, "{}", &DisplayDsIdent(field_ident)).unwrap();
    }
    
    fn write_ty_lit(&self, string: &mut String, ty_lit: TyLit) {
        write!(
            string,
            "{}",
            match ty_lit {
                TyLit::Bool => "bool",
                TyLit::Int => "int",
                TyLit::Float => "float",
                TyLit::Bvec2 => "bool2",
                TyLit::Bvec3 => "bool3",
                TyLit::Bvec4 => "bool4",
                TyLit::Ivec2 => "int2",
                TyLit::Ivec3 => "int3",
                TyLit::Ivec4 => "int4",
                TyLit::Vec2 => "float2",
                TyLit::Vec3 => "float3",
                TyLit::Vec4 => "float4",
                TyLit::Mat2 => "float2x2",
                TyLit::Mat3 => "float3x3",
                TyLit::Mat4 => "float4x4",
                TyLit::Texture2D => panic!(), // TODO
            }
        )
            .unwrap();
    }
    
    fn write_builtin_call_ident(&self, string: &mut String, ident: Ident, arg_exprs: &[Expr]) {
        match ident {
            Ident(id!(atan)) => {
                if arg_exprs.len() == 2 {
                    write!(string, "atan2").unwrap();
                }
                else {
                    write!(string, "atan").unwrap();
                }
            }
            Ident(id!(mod)) => {
                write!(string, "fmod").unwrap();
            }
            Ident(id!(dFdx)) => {
                write!(string, "dfdx").unwrap();
            }
            Ident(id!(dFdy)) => {
                write!(string, "dfdy").unwrap();
            }
            _ => {
                write!(string, "{}", ident).unwrap()
            }
        }
    }
}
