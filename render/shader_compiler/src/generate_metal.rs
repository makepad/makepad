use crate::shaderast::*;
use makepad_live_parser::*;
use crate::generate::*;
use std::fmt::Write;
use std::fmt;
use std::collections::BTreeMap;
use crate::shaderregistry::ShaderRegistry;
use crate::shaderregistry::FinalConstTable;

pub fn generate_shader(draw_shader_def: &DrawShaderDef, final_const_table: &FinalConstTable, shader_registry: &ShaderRegistry) -> String {
    let mut string = String::new();
    DrawShaderGenerator {
        draw_shader_def,
        shader_registry,
        final_const_table,
        string: &mut string,
        backend_writer: &MetalBackendWriter {shader_registry, draw_shader_def}
    }
    .generate_shader();
    string
}

struct DrawShaderGenerator<'a> {
    draw_shader_def: &'a DrawShaderDef,
    shader_registry: &'a ShaderRegistry,
    final_const_table: &'a FinalConstTable,
    string: &'a mut String,
    backend_writer: &'a dyn BackendWriter
}

impl<'a> DrawShaderGenerator<'a> {
    fn generate_shader(&mut self) {
        writeln!(self.string, "#include <metal_stdlib>").unwrap();
        writeln!(self.string, "using namespace metal;").unwrap();
        writeln!(self.string, "float4 sample2d(texture2d<float> tex, float2 pos){{return tex.sample(sampler(mag_filter::linear,min_filter::linear),pos);}}").unwrap();
        self.generate_struct_defs();
        let fields_as_uniform_blocks = self.draw_shader_def.fields_as_uniform_blocks();
        self.generate_uniform_structs(&fields_as_uniform_blocks);
        self.generate_texture_struct();
        self.generate_geometry_struct();
        self.generate_instance_struct();
        self.generate_varying_struct();
        self.generate_const_decls();
        
        let vertex_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap();
        let pixel_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(pixel))).unwrap();
        
        for &(ty_lit, ref param_tys) in pixel_def
            .constructor_fn_deps
            .borrow_mut()
            .as_ref()
            .unwrap()
            .union(vertex_def.constructor_fn_deps.borrow().as_ref().unwrap())
        {
            self.generate_cons_fn(ty_lit, param_tys);
        }
        
        let all_fns = self.draw_shader_def.all_fns.borrow();
        for fn_iter in all_fns.iter().rev() {
            let const_table_offset = self.final_const_table.offsets.get(fn_iter).cloned();
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
        self.generate_vertex_main(&fields_as_uniform_blocks);
        self.generate_fragment_main(&fields_as_uniform_blocks);
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
                    self.write_var_decl(
                        &DisplayStructField(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
            }
            writeln!(self.string, "}};").unwrap();
        }
    }
    
    fn generate_uniform_structs(&mut self, fields_as_uniform_blocks: &BTreeMap<Ident, Vec<(usize, Ident) >>) {
        
        for (ident, vec) in fields_as_uniform_blocks {
            writeln!(self.string, "struct Uniforms_{} {{", ident).unwrap();
            for (index, _item) in vec {
                let field = &self.draw_shader_def.fields[*index];
                write!(self.string, "    ").unwrap();
                self.write_var_decl(
                    &DisplayDsIdent(field.ident),
                    field.ty_expr.ty.borrow().as_ref().unwrap(),
                );
                writeln!(self.string, ";").unwrap();
            }
            writeln!(self.string, "}};").unwrap();
        }
        
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
    }
    
    fn generate_texture_struct(&mut self) {
        let mut index = 0;
        writeln!(self.string, "struct Textures {{").unwrap();
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Texture {..} => {
                    assert_eq!(*field.ty_expr.ty.borrow().as_ref().unwrap(), Ty::Texture2D);
                    write!(self.string, "    texture2d<float> ").unwrap();
                    write!(self.string, "{}", field.ident).unwrap();
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
                    self.write_var_decl_packed(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _=>()
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_instance_struct(&mut self) {
        writeln!(self.string, "struct Instances {{").unwrap();
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Instance {..} => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl_packed(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _=>()
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
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Varying {..} => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_const_decls(&mut self) {
        for const_ref_ptr in self.draw_shader_def.all_const_refs.borrow().iter() {
            let const_def = self.shader_registry.consts.get(const_ref_ptr).unwrap();
            
            write!(self.string, "constant ").unwrap();
            self.write_var_decl(
                &const_ref_ptr,
                const_def.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            write!(self.string, " = ").unwrap();
            self.generate_expr(&const_def.expr);
            writeln!(self.string, ";").unwrap();
        }
    }
    
    fn generate_cons_fn(&mut self, ty_lit: TyLit, param_tys: &[Ty]) {
        let mut cons_name = format!("consfn_{}", ty_lit);
        for param_ty in param_tys {
            write!(cons_name, "_{}", param_ty).unwrap();
        }
        if !self.backend_writer.use_cons_fn(&cons_name) {
            return
        }
        
        self.write_ty_lit(ty_lit);
        write!(self.string, " {}(", cons_name).unwrap();
        let mut sep = "";
        if param_tys.len() == 1 {
            self.write_var_decl(&Ident(id!(x)), &param_tys[0])
        } else {
            for (index, param_ty) in param_tys.iter().enumerate() {
                write!(self.string, "{}", sep).unwrap();
                self.write_var_decl(&DisplaConstructorArg(index), param_ty);
                sep = ", ";
            }
        }
        writeln!(self.string, ") {{").unwrap();
        write!(self.string, "    return ").unwrap();
        self.write_ty_lit(ty_lit);
        write!(self.string, "(").unwrap();
        let ty = ty_lit.to_ty();
        if param_tys.len() == 1 {
            let param_ty = &param_tys[0];
            match param_ty {
                Ty::Bool | Ty::Int | Ty::Float => {
                    let mut sep = "";
                    for _ in 0..ty.size() {
                        write!(self.string, "{}x", sep).unwrap();
                        sep = ", ";
                    }
                }
                Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => {
                    let dst_size = match ty {
                        Ty::Mat2 => 2,
                        Ty::Mat3 => 3,
                        Ty::Mat4 => 4,
                        _ => panic!(),
                    };
                    let src_size = match param_ty {
                        Ty::Mat2 => 2,
                        Ty::Mat3 => 3,
                        Ty::Mat4 => 4,
                        _ => panic!(),
                    };
                    let mut sep = "";
                    for col_index in 0..dst_size {
                        for row_index in 0..dst_size {
                            if row_index < src_size && col_index < src_size {
                                write!(self.string, "{}x[{}][{}]", sep, col_index, row_index)
                                    .unwrap();
                            } else {
                                write!(
                                    self.string,
                                    "{}{}",
                                    sep,
                                    if col_index == row_index {1.0} else {0.0}
                                )
                                    .unwrap();
                            }
                            sep = ", ";
                        }
                    }
                }
                _ => panic!(),
            }
        } else {
            let mut sep = "";
            for (index_0, param_ty) in param_tys.iter().enumerate() {
                if param_ty.size() == 1 {
                    write!(self.string, "{}x{}", sep, index_0).unwrap();
                    sep = ", ";
                } else {
                    for index_1 in 0..param_ty.size() {
                        write!(self.string, "{}x{}[{}]", sep, index_0, index_1).unwrap();
                        sep = ", ";
                    }
                }
            }
        }
        writeln!(self.string, ");").unwrap();
        writeln!(self.string, "}}").unwrap();
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
    
    fn generate_vertex_main(&mut self, fields_as_uniform_blocks: &BTreeMap<Ident, Vec<(usize, Ident) >>) {
        
        write!(self.string, "vertex Varyings vertex_main(").unwrap();
        writeln!(self.string, "Textures textures").unwrap();
        writeln!(self.string, ", const device Geometries *in_geometries [[buffer(0)]]").unwrap();
        writeln!(self.string, ", const device Instances *in_instances [[buffer(1)]]").unwrap();
        writeln!(self.string, ", constant LiveUniforms &live_uniforms [[buffer(2)]]").unwrap();
        writeln!(self.string, ", constant const float *const_table [[buffer(3)]]").unwrap();
        let mut buffer_id = 4;
        for (field, _set) in fields_as_uniform_blocks {
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
                    writeln!(
                        self.string,
                        "    mpsc_varyings.{0} = mpsc_geometries.{0};",
                        decl.ident
                    )
                        .unwrap();
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    writeln!(
                        self.string,
                        "    mpsc_varyings.{0} = mpsc_instances.{0};",
                        decl.ident
                    )
                        .unwrap();
                }
                _ => {}
            }
        }
        
        let vertex_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap();
        write!(self.string, "    varyings.position = {}", DisplayFnName(vertex_def.fn_node_ptr, vertex_def.ident)).unwrap();
        
        
        write!(self.string, "(").unwrap();
        //let mut sep = "";
        write!(self.string, "const_table").unwrap();
        //sep = ", ";
        /*
        for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}mpsc_{}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        if !decl.geometry_deps.borrow().as_ref().unwrap().is_empty() {
            write!(self.string, "{}mpsc_geometries", sep).unwrap();
            sep = ", ";
        }
        if !decl.instance_deps.borrow().as_ref().unwrap().is_empty() {
            write!(self.string, "{}mpsc_instances", sep).unwrap();
            sep = ", ";
        }
        if decl.has_varying_deps.get().unwrap() {
            write!(self.string, "{}mpsc_varyings", sep).unwrap();
        }*/
        writeln!(self.string, ");").unwrap();
        
        writeln!(self.string, "    return mpsc_varyings;").unwrap();
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_fragment_main(&mut self, fields_as_uniform_blocks: &BTreeMap<Ident, Vec<(usize, Ident) >>) {
        
        write!(self.string, "fragment float4 fragment_main(").unwrap();
        writeln!(self.string, "Varyings varyings[[stage_in]]").unwrap();
        writeln!(self.string, ", Textures textures").unwrap();
        writeln!(self.string, ", constant LiveUniforms &live_uniforms [[buffer(2)]]").unwrap();
        writeln!(self.string, ", constant const float *const_table [[buffer(3)]]").unwrap();
        let mut buffer_id = 4;
        for (field, _set) in fields_as_uniform_blocks {
            writeln!(self.string, ", constant Uniforms_{0} &uniforms_{0} [[buffer({1})]]", field, buffer_id).unwrap();
            buffer_id += 1;
        }
        
        writeln!(self.string, ") {{").unwrap();
        
        write!(self.string, "    return ").unwrap();
        
        let pixel_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(pixel))).unwrap();
        write!(self.string, "    {}", DisplayFnName(pixel_def.fn_node_ptr, pixel_def.ident)).unwrap();
        
        write!(self.string, "(").unwrap();
        //let mut sep = "";
        write!(self.string, "const_table").unwrap();
        //sep = ", ";

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
    pub draw_shader_def: &'a DrawShaderDef
}

impl<'a> BackendWriter for MetalBackendWriter<'a> {
    
    fn generate_draw_shader_prefix(&self, string: &mut String, _expr: &Expr, field_ident: Ident) {
        let field_def = self.draw_shader_def.find_field(field_ident).unwrap();
        
        match &field_def.kind {
            DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} => {
                if is_used_in_pixel_shader.get() {
                    write!(string, "varyings.").unwrap()
                }
                else {
                    write!(string, "geometry.").unwrap()
                }
            }
            DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} => {
                if is_used_in_pixel_shader.get() {
                    write!(string, "varyings.").unwrap()
                }
                else {
                    write!(string, "instances.").unwrap()
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
                write!(string, "{} {}", ref_prefix, struct_node_ptr).unwrap();
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
