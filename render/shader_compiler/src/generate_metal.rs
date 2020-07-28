use {
    crate::{
        ast::*,
        generate::{
            BlockGenerator,
            ExprGenerator,
            BackendWriter,
        },
        ident::Ident,
        lit::TyLit,
        ty::Ty,
    },
    std::{
        collections::{
            HashMap,
            HashSet,
        },
        fmt::Write,
    }
};

pub fn generate_shader(shader: &ShaderAst) -> String {
    let mut string = String::new();
    ShaderGenerator {
        shader,
        string: &mut string,
    }
    .generate_shader();
    string
}

struct ShaderGenerator<'a> {
    shader: &'a ShaderAst,
    string: &'a mut String,
}

impl<'a> ShaderGenerator<'a> {
    fn generate_shader(&mut self) {
        self.generate_uniform_structs();
        self.generate_attribute_struct();
        self.generate_instance_struct();
        self.generate_varying_struct();
        let vertex_decl = self.shader.find_fn_decl(Ident::new("vertex")).unwrap();
        let fragment_decl = self.shader.find_fn_decl(Ident::new("pixel")).unwrap();
        for &(ty_lit, ref param_tys) in vertex_decl
            .cons_fn_deps
            .borrow_mut()
            .as_ref()
            .unwrap()
            .union(
                fragment_decl
                    .cons_fn_deps
                    .borrow()
                    .as_ref()
                    .unwrap()
            )
        {
            self.generate_cons_fn(ty_lit, param_tys);
        }
        let mut visited = HashSet::new();
        self.generate_fn_decl(vertex_decl, &mut visited);
        self.generate_fn_decl(fragment_decl, &mut visited);
        self.generate_vertex_main();
        self.generate_fragment_main();
    }

    fn generate_uniform_structs(&mut self) {
        let mut uniform_blocks = HashMap::new();
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
        for (ident, decls) in uniform_blocks {
            writeln!(self.string, "struct mpsc_{}_Uniforms {{", ident).unwrap();
            for decl in decls {
                write!(self.string, "    ").unwrap();
                self.write_var_decl(
                    false,
                    false,
                    decl.ident,
                    decl.ty_expr.ty.borrow().as_ref().unwrap()
                );
                writeln!(self.string, ";").unwrap();
            }
            writeln!(self.string, "}};").unwrap();
        }
    }

    fn generate_attribute_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Attributes {{").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        true,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
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
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
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
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        true,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        true,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Varying(decl) => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        true,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }

    fn generate_cons_fn(&mut self, ty_lit: TyLit, param_tys: &[Ty]) {
        self.write_ty_lit(ty_lit);
        write!(self.string, " mpsc_{}", ty_lit).unwrap();
        for param_ty in param_tys {
            write!(self.string, "_{}", param_ty).unwrap();
        }
        write!(self.string, "(").unwrap();
        let mut sep = "";
        if param_tys.len() == 1 {
            self.write_var_decl(false, false, Ident::new("x"), &param_tys[0])
        } else {
            for (index, param_ty) in param_tys.iter().enumerate() {
                write!(self.string, "{}", sep).unwrap();
                self.write_var_decl(
                    false,
                    false,
                    Ident::new(format!("x{}", index)),
                    param_ty,
                );
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
                                write!(
                                    self.string,
                                    "{}x[{}][{}]",
                                    sep,
                                    col_index,
                                    row_index
                                )
                                .unwrap();
                            } else {
                                write!(
                                    self.string,
                                    "{}{}",
                                    sep,
                                    if col_index == row_index { 1.0 } else { 0.0 }
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

    fn generate_fn_decl(&mut self, decl: &FnDecl, visited: &mut HashSet<Ident>) {
        FnDeclGenerator {
            shader: self.shader,
            decl,
            visited,
            string: self.string,
        }.generate_fn_decl()
    }

    fn generate_vertex_main(&mut self) {
        let decl = self.shader.find_fn_decl(Ident::new("vertex")).unwrap();
        write!(self.string, "void vertex_main(").unwrap();
        let mut sep = "";
        for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}mpsc_{1}_Uniforms mpsc_{1}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_Textures mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        if !decl.attribute_deps.borrow().as_ref().unwrap().is_empty() {
            write!(self.string, "{}mpsc_Attributes mpsc_attributes", sep).unwrap();
            sep = ", ";
        }
        if !decl.instance_deps.borrow().as_ref().unwrap().is_empty() {
            write!(self.string, "{}mpsc_Instances mpsc_instances", sep).unwrap();
            sep = ", ";
        }
        if decl.has_varying_deps.get().unwrap() {
            write!(self.string, "{}mpsc_Varyings mpsc_varyings", sep).unwrap();
        }
        writeln!(self.string, ") {{").unwrap();
        write!(self.string, "    return ").unwrap();
        self.write_ident(decl.ident);
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}mpsc_{}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        if !decl.attribute_deps.borrow().as_ref().unwrap().is_empty() {
            write!(self.string, "{}mpsc_attributes", sep).unwrap();
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
        writeln!(self.string, "}}").unwrap();
    }

    fn generate_fragment_main(&mut self) {
        let decl = self.shader.find_fn_decl(Ident::new("pixel")).unwrap();
        write!(self.string, "void fragment_main(").unwrap();
        let mut sep = "";
        for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}mpsc_{1}_Uniforms mpsc_{1}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_Textures mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        if !decl.attribute_deps.borrow().as_ref().unwrap().is_empty()
            || decl.instance_deps.borrow().as_ref().unwrap().is_empty()
            || decl.has_varying_deps.get().unwrap()
        {
            write!(self.string, "{}mpsc_Varyings mpsc_varyings", sep).unwrap();
        }
        writeln!(self.string, ") {{").unwrap();
        write!(self.string, "    return ").unwrap();
        self.write_ident(decl.ident);
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}mpsc_{1}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        if !decl.attribute_deps.borrow().as_ref().unwrap().is_empty()
            || decl.instance_deps.borrow().as_ref().unwrap().is_empty()
            || decl.has_varying_deps.get().unwrap()
        {
            write!(self.string, "{}mpsc_varyings", sep).unwrap();
        }
        writeln!(self.string, ");").unwrap();
        writeln!(self.string, "}}").unwrap();
    }

    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader: self.shader,
            decl: None,
            backend_writer: &MetalBackendWriter,
            use_hidden_params: true,
            use_generated_cons_fns: true,
            string: self.string,
        }
        .generate_expr(expr)
    }

    fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
        MetalBackendWriter.write_var_decl(
            &mut self.string,
            is_inout,
            is_packed,
            ident,
            ty
        );
    }

    fn write_ident(&mut self, ident: Ident) {
        MetalBackendWriter.write_ident(&mut self.string, ident);
    }

    fn write_ty_lit(&mut self, ty_lit: TyLit) {
        MetalBackendWriter.write_ty_lit(
            &mut self.string,
            ty_lit,
        );  
    }
}

struct FnDeclGenerator<'a> {
    shader: &'a ShaderAst,
    decl: &'a FnDecl,
    visited: &'a mut HashSet<Ident>,
    string: &'a mut String,
}

impl<'a> FnDeclGenerator<'a> {
    fn generate_fn_decl(&mut self) {
        if self.visited.contains(&self.decl.ident) {
            return;
        }
        for &callee in self.decl.callees.borrow().as_ref().unwrap().iter() {
            FnDeclGenerator {
                shader: self.shader,
                decl: self.shader.find_fn_decl(callee).unwrap(),
                visited: self.visited,
                string: self.string,
            }.generate_fn_decl()
        }
        self.write_var_decl(
            false,
            false,
            self.decl.ident,
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
        for &ident in self.decl.uniform_block_deps.borrow().as_ref().unwrap() {
            write!(self.string, "{}_mpsc_{1}_Uniforms mpsc_{1}_uniforms", sep, ident).unwrap();
            sep = ", ";
        }
        if self.decl.has_texture_deps.get().unwrap() {
            write!(self.string, "{}_mpsc_Textures mpsc_textures", sep).unwrap();
            sep = ", ";
        }
        let is_used_in_vertex_shader = self.decl.is_used_in_vertex_shader.get().unwrap();
        let is_used_in_fragment_shader = self.decl.is_used_in_fragment_shader.get().unwrap();
        let has_attribute_deps = !self.decl.attribute_deps.borrow().as_ref().unwrap().is_empty();
        let has_instance_deps = !self.decl.instance_deps.borrow().as_ref().unwrap().is_empty();
        let has_varying_deps = self.decl.has_varying_deps.get().unwrap();
        if is_used_in_vertex_shader {
            if has_attribute_deps {
                write!(self.string, "{}_mpsc_Attributes mpsc_attributes", sep).unwrap();
                sep = ", ";
            }
            if has_instance_deps {
                write!(self.string, "{}_mpsc_Instances mpsc_instances", sep).unwrap();
                sep = ", ";
            }
            if has_varying_deps {
                write!(self.string, "{}_mpsc_Varyings mpsc_varyings", sep).unwrap();
            }
        }
        if is_used_in_fragment_shader {
            if has_attribute_deps || has_instance_deps || has_varying_deps {       
                write!(self.string, "{}_mpsc_Varyings mpsc_varyings", sep).unwrap();
            }
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&self.decl.block);
        writeln!(self.string).unwrap();
        self.visited.insert(self.decl.ident);
    }

    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader: self.shader,
            decl: self.decl,
            backend_writer: &MetalBackendWriter,
            use_hidden_params: true,
            use_generated_cons_fns: true,
            indent_level: 0,
            string: self.string
        }
        .generate_block(block)
    }

    fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
        MetalBackendWriter.write_var_decl(
            &mut self.string,
            is_inout,
            is_packed,
            ident,
            ty
        );
    }
}

struct MetalBackendWriter;

impl BackendWriter for MetalBackendWriter {
    fn write_var_decl(
        &self,
        string: &mut String,
        is_inout: bool,
        is_packed: bool,
        ident: Ident,
        ty: &Ty
    ) {
        let ref_prefix = if is_inout {
            "&"
        } else {
            ""
        };
        let packed_prefix = if is_packed {
            "packed_"
        } else {
            ""
        };
        match *ty {
            Ty::Void => {
                write!(string, "void ").unwrap();
                self.write_ident(string, ident);
            },
            Ty::Bool => {
                self.write_ty_lit(string, TyLit::Bool);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Int => {
                self.write_ty_lit(string, TyLit::Int);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Float => {
                self.write_ty_lit(string, TyLit::Float);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Bvec2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Bvec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Bvec4 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Bvec4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Ivec2 => {
                write!(string, "{}", packed_prefix).unwrap();
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Ivec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Ivec4 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Ivec4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Vec2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Vec3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Vec4 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Vec4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Mat2 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Mat2);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Mat3 => {
                write!(string, "{}", packed_prefix).unwrap();
                self.write_ty_lit(string, TyLit::Mat3);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Mat4 => {
                self.write_ty_lit(string, TyLit::Mat4);
                write!(string, " {}", ref_prefix).unwrap();
                self.write_ident(string, ident);
            },
            Ty::Texture2D => panic!(), // TODO
            Ty::Array { ref elem_ty, len } => {
                write!(string, "{}", packed_prefix).unwrap();
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
        write!(string, "{}", match ty_lit {
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
            TyLit::Mat2 => "mat2",
            TyLit::Mat3 => "mat3",
            TyLit::Mat4 => "mat4",
            TyLit::Texture2D => panic!(), // TODO
        }).unwrap();
    }
}