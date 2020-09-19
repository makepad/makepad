use {
    crate::{
        shaderast::*,
        env::{VarKind, Env},
        span::Span,
        analyse::ShaderCompileOptions,
        generate::{BackendWriter, BlockGenerator, ExprGenerator},
        ident::{Ident, IdentPath},
        livestyles::LiveStyles,
        lit::TyLit,
        ty::Ty,
    },
    std::{
        cell::{Cell},
        collections::{BTreeMap, HashSet},
        fmt::Write,
    },
};

pub fn generate_shader(shader: &ShaderAst, live_styles: &LiveStyles, options: ShaderCompileOptions) -> String {
    let mut string = String::new();
    let env = Env::new(live_styles);
    ShaderGenerator {
        shader,
        env: &env,
        create_const_table: options.create_const_table,
        string: &mut string,
        backend_writer: &MetalBackendWriter {env: &env}
    }
    .generate_shader();
    string
}

struct ShaderGenerator<'a, 'b> {
    shader: &'a ShaderAst,
    create_const_table: bool,
    string: &'a mut String,
    env: &'a Env<'b>,
    backend_writer: &'a dyn BackendWriter
}

impl<'a, 'b> ShaderGenerator<'a, 'b> {
    fn generate_shader(&mut self) {
        writeln!(self.string, "#include <metal_stdlib>").unwrap();
        writeln!(self.string, "using namespace metal;").unwrap();
        writeln!(self.string, "float4 sample2d(texture2d<float> tex, float2 pos){{return tex.sample(sampler(mag_filter::linear,min_filter::linear),pos);}}").unwrap();
        self.generate_struct_decls();
        self.generate_uniform_structs();
        self.generate_texture_struct();
        self.generate_geometry_struct();
        self.generate_instance_struct();
        self.generate_varying_struct();
        self.generate_const_decls();
        let vertex_decl = self.shader.find_fn_decl(IdentPath::from_str("vertex")).unwrap();
        let fragment_decl = self.shader.find_fn_decl(IdentPath::from_str("pixel")).unwrap();
        for &(ty_lit, ref param_tys) in vertex_decl
            .cons_fn_deps
            .borrow_mut()
            .as_ref()
            .unwrap()
            .union(fragment_decl.cons_fn_deps.borrow().as_ref().unwrap())
        {
            self.generate_cons_fn(ty_lit, param_tys);
        }
        let mut visited = HashSet::new();
        self.generate_fn_decl(vertex_decl, &mut visited);
        self.generate_fn_decl(fragment_decl, &mut visited);
        self.generate_vertex_main();
        self.generate_fragment_main();
    }
    
    fn generate_struct_decls(&mut self) {
        for decl in &self.shader.decls {
            match decl {
                Decl::Struct(decl) => {
                    write!(self.string, "struct {} {{", decl.ident).unwrap();
                    if !decl.fields.is_empty() {
                        writeln!(self.string).unwrap();
                        for field in &decl.fields {
                            write!(self.string, "    ").unwrap();
                            self.write_var_decl(
                                false,
                                false,
                                field.ident,
                                field.ty_expr.ty.borrow().as_ref().unwrap(),
                            );
                            writeln!(self.string, ";").unwrap();
                        }
                    }
                    writeln!(self.string, "}};").unwrap();
                }
                _ => {}
            }
        }
    }
    
    fn generate_uniform_structs(&mut self) {
        let mut uniform_blocks = BTreeMap::new();
        for decl in &self.shader.decls {
            match decl {
                Decl::Uniform(decl) => {
                    let uniform_block = uniform_blocks
                        .entry(decl.block_ident.unwrap_or(Ident::new("default")))
                        .or_insert(Vec::new());
                    uniform_block.push(decl);
                }
                _ => {}
            }
        }
        let mut has_default = false;
        for (ident, decls) in uniform_blocks {
            if ident == Ident::new("default") {
                has_default = true;
            }
            writeln!(self.string, "struct mpsc_{}_Uniforms {{", ident).unwrap();
            for decl in decls {
                write!(self.string, "    ").unwrap();
                self.write_var_decl(
                    false,
                    false,
                    decl.ident,
                    decl.ty_expr.ty.borrow().as_ref().unwrap(),
                );
                writeln!(self.string, ";").unwrap();
            }
            writeln!(self.string, "}};").unwrap();
        }
        if !has_default {
            writeln!(self.string, "struct mpsc_default_Uniforms{{}};").unwrap();
        }
        
        writeln!(self.string, "struct mpsc_live_Uniforms {{").unwrap();
        for (ty, qualified_ident_path) in self.shader.livestyle_uniform_deps.borrow().as_ref().unwrap() {
            // we have a span and an ident_path.
            // lets fully qualify it
            write!(self.string, "    ").unwrap();
            self.backend_writer.write_ty_lit(self.string, ty.maybe_ty_lit().unwrap());
            write!(self.string, " ").unwrap();
            qualified_ident_path.write_underscored_ident(self.string);
            writeln!(self.string, ";").unwrap();
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_texture_struct(&mut self) {
        let mut index = 0;
        writeln!(self.string, "struct mpsc_Textures {{").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Texture(decl) => {
                    assert_eq!(*decl.ty_expr.ty.borrow().as_ref().unwrap(), Ty::Texture2D);
                    writeln!(
                        self.string,
                        "    texture2d<float> {} [[texture({})]];",
                        decl.ident,
                        index
                    )
                        .unwrap();
                    index += 1;
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_geometry_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Geometries {{").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        true,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_instance_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Instances {{").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Instance(decl) => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        true,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_varying_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Varyings {{").unwrap();
        writeln!(self.string, "    float4 mpsc_position [[position]];").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Varying(decl) => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_const_decls(&mut self) {
        for decl in &self.shader.decls {
            match decl {
                Decl::Const(decl) => {
                    write!(self.string, "constant ").unwrap();
                    self.write_var_decl(
                        false,
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, " = ").unwrap();
                    self.generate_expr(&decl.expr);
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
    }
    
    fn generate_cons_fn(&mut self, ty_lit: TyLit, param_tys: &[Ty]) {
        let mut cons_name = format!("mpsc_{}", ty_lit);
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
            self.write_var_decl(false, false, Ident::new("x"), &param_tys[0])
        } else {
            for (index, param_ty) in param_tys.iter().enumerate() {
                write!(self.string, "{}", sep).unwrap();
                self.write_var_decl(false, false, Ident::new(format!("x{}", index)), param_ty);
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
    
    fn generate_fn_decl(&mut self, decl: &FnDecl, visited: &mut HashSet<IdentPath>) {
        FnDeclGenerator {
            shader: self.shader,
            decl,
            create_const_table: self.create_const_table,
            visited,
            backend_writer: self.backend_writer,
            string: self.string,
        }
        .generate_fn_decl()
    }
    
    fn generate_vertex_main(&mut self) {
        let decl = self.shader.find_fn_decl(IdentPath::from_str("vertex")).unwrap();
        write!(self.string, "vertex mpsc_Varyings mpsc_vertex_main(").unwrap();
        write!(self.string, "mpsc_Textures mpsc_textures").unwrap();
        write!(
            self.string,
            ", const device mpsc_Geometries *in_geometries [[buffer(0)]]"
        ).unwrap();
        write!(
            self.string,
            ", const device mpsc_Instances *in_instances [[buffer(1)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_pass_Uniforms &mpsc_pass_uniforms [[buffer(2)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_view_Uniforms &mpsc_view_uniforms [[buffer(3)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_draw_Uniforms &mpsc_draw_uniforms [[buffer(4)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_default_Uniforms &mpsc_default_uniforms [[buffer(5)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_live_Uniforms &mpsc_live_uniforms [[buffer(6)]]"
        ).unwrap();
        if self.create_const_table {
            write!(
                self.string,
                ", constant const float *mpsc_const_table [[buffer(7)]]"
            ).unwrap();
        }
        write!(self.string, ", uint vtx_id [[vertex_id]]").unwrap();
        write!(self.string, ", uint inst_id [[instance_id]]").unwrap();
        writeln!(self.string, ") {{").unwrap();
        writeln!(
            self.string,
            "    mpsc_Geometries mpsc_geometries = in_geometries[vtx_id];"
        ).unwrap();
        writeln!(
            self.string,
            "    mpsc_Instances mpsc_instances = in_instances[inst_id];"
        ).unwrap();
        writeln!(self.string, "    mpsc_Varyings mpsc_varyings;").unwrap();
        write!(self.string, "    mpsc_varyings.mpsc_position = ").unwrap();
        self.write_ident(decl.ident_path.get_single().expect("unexpected"));
        write!(self.string, "(").unwrap();
        let mut sep = "";
        if self.create_const_table {
            write!(self.string, "mpsc_const_table").unwrap();
            sep = ", ";
        }
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
        }
        writeln!(self.string, ");").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    writeln!(
                        self.string,
                        "    mpsc_varyings.{0} = mpsc_geometries.{0};",
                        decl.ident
                    )
                        .unwrap();
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
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
        writeln!(self.string, "    return mpsc_varyings;").unwrap();
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_fragment_main(&mut self) {
        let decl = self.shader.find_fn_decl(IdentPath::from_str("pixel")).unwrap();
        write!(self.string, "fragment float4 mpsc_fragment_main(").unwrap();
        write!(self.string, "mpsc_Varyings mpsc_varyings[[stage_in]]").unwrap();
        write!(
            self.string,
            ", constant mpsc_pass_Uniforms &mpsc_pass_uniforms [[buffer(0)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_view_Uniforms &mpsc_view_uniforms [[buffer(1)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_draw_Uniforms &mpsc_draw_uniforms [[buffer(2)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_default_Uniforms &mpsc_default_uniforms [[buffer(3)]]"
        ).unwrap();
        write!(
            self.string,
            ", constant mpsc_live_Uniforms &mpsc_live_uniforms [[buffer(4)]]"
        ).unwrap();
        if self.create_const_table {
            write!(
                self.string,
                ", constant const float *mpsc_const_table [[buffer(5)]]"
            ).unwrap();
        }
        write!(self.string, ", mpsc_Textures mpsc_textures").unwrap();
        
        writeln!(self.string, ") {{").unwrap();
        
        write!(self.string, "    return ").unwrap();
        self.write_ident(decl.ident_path.get_single().expect("unexpected"));
        write!(self.string, "(").unwrap();
        let mut sep = "";
        if self.create_const_table {
            write!(self.string, "mpsc_const_table").unwrap();
            sep = ", ";
        }
        for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}mpsc_{}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        let has_geometry_deps = !decl.geometry_deps.borrow().as_ref().unwrap().is_empty();
        let has_instance_deps = !decl.instance_deps.borrow().as_ref().unwrap().is_empty();
        let has_varying_deps = decl.has_varying_deps.get().unwrap();
        if has_geometry_deps || has_instance_deps || has_varying_deps {
            write!(self.string, "{}mpsc_varyings", sep).unwrap();
        }
        writeln!(self.string, ");").unwrap();
        
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader: self.shader,
            decl: None,
            backend_writer: self.backend_writer,
            create_const_table: self.create_const_table,
            //use_generated_cons_fns: false,
            string: self.string,
        }
        .generate_expr(expr)
    }
    
    fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, is_inout, is_packed, ident, ty);
    }
    
    fn write_ident(&mut self, ident: Ident) {
        self.backend_writer.write_ident(&mut self.string, ident);
    }
    
    fn write_ty_lit(&mut self, ty_lit: TyLit) {
        self.backend_writer.write_ty_lit(&mut self.string, ty_lit);
    }
}

struct FnDeclGenerator<'a> {
    shader: &'a ShaderAst,
    decl: &'a FnDecl,
    create_const_table: bool,
    visited: &'a mut HashSet<IdentPath>,
    string: &'a mut String,
    backend_writer: &'a dyn BackendWriter
}

impl<'a> FnDeclGenerator<'a> {
    fn generate_fn_decl(&mut self) {
        if self.visited.contains(&self.decl.ident_path) {
            return;
        }
        for &callee in self.decl.callees.borrow().as_ref().unwrap().iter() {
            FnDeclGenerator {
                shader: self.shader,
                backend_writer: self.backend_writer,
                decl: self.shader.find_fn_decl(callee).unwrap(),
                create_const_table: self.create_const_table,
                visited: self.visited,
                string: self.string,
            }
            .generate_fn_decl()
        }
        self.write_var_decl(
            false,
            false,
            self.decl.ident_path.to_struct_fn_ident(),
            self.decl.return_ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &self.decl.params {
            write!(self.string, "{}", sep).unwrap();
            self.write_var_decl(
                param.is_inout,
                false,
                param.ident,
                param.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            sep = ", ";
        }
        if self.create_const_table {
            write!(self.string, "{}constant float *mpsc_const_table", sep).unwrap();
            sep = ", ";
        }
        for &ident in self.decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(
                self.string,
                "{}constant mpsc_{1}_Uniforms &mpsc_{1}_uniforms",
                sep,
                ident
            )
                .unwrap();
            sep = ", ";
        }
        if self.decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_Textures mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        let is_used_in_vertex_shader = self.decl.is_used_in_vertex_shader.get().unwrap();
        let is_used_in_fragment_shader = self.decl.is_used_in_fragment_shader.get().unwrap();
        let has_geometry_deps = !self
        .decl
            .geometry_deps
            .borrow()
            .as_ref()
            .unwrap()
            .is_empty();
        let has_instance_deps = !self
        .decl
            .instance_deps
            .borrow()
            .as_ref()
            .unwrap()
            .is_empty();
        let has_varying_deps = self.decl.has_varying_deps.get().unwrap();
        if is_used_in_vertex_shader {
            if has_geometry_deps {
                write!(
                    self.string,
                    "{}thread mpsc_Geometries &mpsc_geometries",
                    sep
                )
                    .unwrap();
                sep = ", ";
            }
            if has_instance_deps {
                write!(self.string, "{}thread mpsc_Instances &mpsc_instances", sep).unwrap();
                sep = ", ";
            }
            if has_varying_deps {
                write!(self.string, "{}thread mpsc_Varyings &mpsc_varyings", sep).unwrap();
            }
        }
        if is_used_in_fragment_shader {
            if has_geometry_deps || has_instance_deps || has_varying_deps {
                write!(self.string, "{}thread mpsc_Varyings &mpsc_varyings", sep).unwrap();
            }
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&self.decl.block);
        writeln!(self.string).unwrap();
        self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader: self.shader,
            decl: self.decl,
            backend_writer: self.backend_writer,
            create_const_table: self.create_const_table,
            // use_generated_cons_fns: false,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
    
    fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, is_inout, is_packed, ident, ty);
    }
}

struct MetalBackendWriter<'a, 'b> {
    pub env: &'a Env<'b>
}

impl<'a, 'b> BackendWriter for MetalBackendWriter<'a, 'b> {
    fn write_call_expr_hidden_args(&self, string: &mut String, use_const_table: bool, ident_path: IdentPath, shader: &ShaderAst, sep: &str) {
        let mut sep = sep;
        if let Some(decl) = shader.find_fn_decl(ident_path) {
            if use_const_table {
                write!(string, "{}mpsc_const_table", sep).unwrap();
                sep = ", ";
            }
            for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
                write!(string, "{}mpsc_{}_uniforms", sep, ident).unwrap();
                sep = ", ";
            }
            if decl.has_texture_deps.get().unwrap() {
                write!(string, "{}mpsc_textures", sep).unwrap();
                sep = ", ";
            }
            if decl.is_used_in_vertex_shader.get().unwrap() {
                if !decl.geometry_deps.borrow().as_ref().unwrap().is_empty() {
                    write!(string, "{}mpsc_geometries", sep).unwrap();
                    sep = ", ";
                }
                if !decl.instance_deps.borrow().as_ref().unwrap().is_empty() {
                    write!(string, "{}mpsc_instances", sep).unwrap();
                    sep = ", ";
                }
                if decl.has_varying_deps.get().unwrap() {
                    write!(string, "{}mpsc_varyings", sep).unwrap();
                }
            } else {
                assert!(decl.is_used_in_fragment_shader.get().unwrap());
                if !decl.geometry_deps.borrow().as_ref().unwrap().is_empty()
                    || !decl.instance_deps.borrow().as_ref().unwrap().is_empty()
                    || decl.has_varying_deps.get().unwrap()
                {
                    write!(string, "{}mpsc_varyings", sep).unwrap();
                }
            }
        }
    }
    
    fn generate_var_expr(&self, string: &mut String, span: Span, ident_path: IdentPath, kind: &Cell<Option<VarKind >>, shader: &ShaderAst, decl: &FnDecl, _ty: &Option<Ty>) {
        
        let is_used_in_vertex_shader = decl.is_used_in_vertex_shader.get().unwrap();
        let is_used_in_fragment_shader = decl.is_used_in_fragment_shader.get().unwrap();
        if is_used_in_vertex_shader {
            match kind.get().unwrap() {
                VarKind::Geometry => write!(string, "mpsc_geometries.").unwrap(),
                VarKind::Instance => write!(string, "mpsc_instances.").unwrap(),
                VarKind::Varying => write!(string, "mpsc_varyings.").unwrap(),
                _ => {}
            }
        }
        if is_used_in_fragment_shader {
            match kind.get().unwrap() {
                VarKind::Geometry | VarKind::Instance | VarKind::Varying => {
                    write!(string, "mpsc_varyings.").unwrap()
                }
                _ => {}
            }
        }
        match kind.get().unwrap() {
            VarKind::Uniform => {
                write!(
                    string,
                    "mpsc_{}_uniforms.",
                    shader
                        .find_uniform_decl(ident_path.get_single().expect("unexpected"))
                        .unwrap()
                        .block_ident
                        .unwrap_or(Ident::new("default")),
                )
                    .unwrap();
            }
            VarKind::Texture => write!(string, "mpsc_textures.").unwrap(),
            VarKind::LiveStyle => {
                let qualified = self.env.qualify_ident_path(span.live_body_id, ident_path);
                write!(string, "mpsc_live_uniforms.").unwrap();
                qualified.write_underscored_ident(string);
                return
            },
            _ => ()
        }
        write!(string, "{}", ident_path.get_single().expect("unexpected")).unwrap()
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
        is_inout: bool,
        is_packed: bool,
        ident: Ident,
        ty: &Ty,
    ) {
        let ref_prefix = if is_inout {
            write!(string, "thread ").unwrap();
            "&"
        } else {
            ""
        };
        let packed_prefix = if is_packed {"packed_"} else {""};
        match *ty {
            Ty::Void => {
                write!(string, "void ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bool => {
                self.write_ty_lit(string, TyLit::Bool);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Int => {
                self.write_ty_lit(string, TyLit::Int);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Float => {
                self.write_ty_lit(string, TyLit::Float);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bvec2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bvec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bvec4 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Ivec2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Ivec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Ivec4 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Vec2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Vec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Vec4 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Mat2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Mat2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Mat3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Mat3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Mat4 => {
                self.write_ty_lit(string, TyLit::Mat4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
            Ty::Texture2D => panic!(), // TODO
            Ty::Array {ref elem_ty, len} => {
                self.write_var_decl(string, is_inout, is_packed, ident, elem_ty);
                write!(string, "[{}]", len).unwrap();
            }
            Ty::Struct {
                ident: struct_ident,
            } => {
                write!(string, "{} {}", struct_ident, ref_prefix).unwrap();
                self.write_ident(string, ident);
            }
        }
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
    
    fn write_call_ident(&self, string: &mut String, ident: Ident, arg_exprs: &[Expr]) {
        if ident == Ident::new("atan") {
            if arg_exprs.len() == 2 {
                write!(string, "atan2").unwrap();
            }
            else {
                write!(string, "atan").unwrap();
            }
        }
        else if ident == Ident::new("mod") {
            write!(string, "fmod").unwrap();
        }
        else if ident == Ident::new("dFdx") {
            write!(string, "dfdx").unwrap();
        }
        else if ident == Ident::new("dFdy") {
            write!(string, "dfdy").unwrap();
        }
        else {
            self.write_ident(string, ident);
        }
    }
    
    fn write_ident(&self, string: &mut String, ident: Ident) {
        ident.with( | ident_string | {
            if ident_string.contains("::") {
                write!(string, "mpsc_{}", ident_string.replace("::", "_")).unwrap()
            } else {
                // do a remapping
                write!(
                    string,
                    "{}",
                    match ident_string.as_ref() {
                        "thread" => "mpsc_thread",
                        "device" => "mpsc_device",
                        "dfdx" => "mpsc_dfdx",
                        "dfdy" => "mpsc_dfdy",
                        "using" => "mpsc_using",
                        "union" => "mpsc_union",
                        "namespace" => "mpsc_namespace",
                        "sampler" => "mpsc_sampler",
                        "coord" => "mpsc_coord",
                        "address" => "mpsc_address",
                        "filter" => "mpsc_filter",
                        "mag_filter" => "mpsc_mag_filter",
                        "min_filter" => "mspc_min_filter",
                        "mip_filter" => "mpsc_mip_filter",
                        "compare_func" => "mpsc_compare_func",
                        "access" => "mpsc_access",
                        "write" => "mpsc_write",
                        "read" => "mpsc_read",
                        "read_write" => "mpsc_read_write",
                        "texture2d" => "mpsc_texture2d",
                        "pixel" => "mpsc_pixel",
                        "vertex" => "mpsc_vertex",
                        "constant" => "mpsc_constant",
                        "float2" => "mpsc_float2",
                        "float3" => "mpsc_float3",
                        "float4" => "mpsc_float4",
                        "float2x2" => "mpsc_float2x2",
                        "float3x3" => "mpsc_float3x3",
                        "float4x4" => "mpsc_float4x4",
                        _ => ident_string,
                    }
                )
                    .unwrap()
            }
        })
    }
}
