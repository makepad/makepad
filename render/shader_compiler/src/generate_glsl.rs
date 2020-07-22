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
        swizzle::Swizzle,
        ty::Ty,
    },
    std::fmt::Write,
};

pub fn generate_vertex_shader(shader: &ShaderAst) -> String {
    let mut string = String::new();
    ShaderGenerator {
        shader,
        string: &mut string,
    }
    .generate_vertex_shader();
    string
}

pub fn generate_fragment_shader(shader: &ShaderAst) -> String {
    let mut string = String::new();
    ShaderGenerator {
        shader,
        string: &mut string,
    }
    .generate_fragment_shader();
    string
}

struct ShaderGenerator<'a> {
    shader: &'a ShaderAst,
    string: &'a mut String,
}

impl<'a> ShaderGenerator<'a> {
    fn generate_vertex_shader(&mut self) {
        let packed_attributes_size = self.compute_packed_attributes_size();
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_decls(Some(packed_attributes_size), packed_varyings_size);
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                },
                Decl::Instance(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                },
                Decl::Varying(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                },
                _ => {}
            }
        }
        self.generate_fn_decl(self.shader.find_fn_decl(Ident::new("vertex")).unwrap());
        writeln!(self.string, "void main() {{").unwrap();
        let mut attribute_unpacker = VarUnpacker::new(
            "mpsc_packed_attribute",
            packed_attributes_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) => {
                    attribute_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                Decl::Instance(decl) => {
                    attribute_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                }
                _ => {}
            }
        }
        writeln!(self.string, "    gl_Position = vertex();").unwrap();
        let mut varying_packer = VarPacker::new(
            "mpsc_packed_varying",
            packed_varyings_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_packer.pack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_packer.pack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                }
                Decl::Varying(decl) => {
                    varying_packer.pack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                _ => {}
            }
        }
        writeln!(self.string, "}}").unwrap();
    }

    fn generate_fragment_shader(&mut self) {
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_decls(None, packed_varyings_size);
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Varying(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    writeln!(self.string, ";").unwrap();
                },
                _ => {}
            }
        }
        self.generate_fn_decl(self.shader.find_fn_decl(Ident::new("pixel")).unwrap());
        writeln!(self.string, "void main() {{").unwrap();
        let mut varying_unpacker = VarUnpacker::new(
            "mpsc_packed_varying",
            packed_varyings_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                }
                Decl::Varying(decl) => {
                    varying_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                _ => {}
            }
        }
        writeln!(self.string, "    gl_FragColor = pixel();").unwrap();
        writeln!(self.string, "}}").unwrap();
    }

    fn generate_decls(
        &mut self,
        packed_attributes_size: Option<usize>,
        packed_varyings_size: usize
    ) {
        for decl in &self.shader.decls {
            match decl {
                Decl::Struct(decl) => self.generate_struct_decl(decl),
                _ => {}
            }
        }

        for decl in &self.shader.decls {
            match decl {
                Decl::Const(decl) => self.generate_const_decl(decl),
                _ => {}
            }
        }

        for decl in &self.shader.decls {
            match decl {
                Decl::Uniform(decl) => self.generate_uniform_decl(decl),
                _ => {}
            }
        }

        if let Some(packed_attributes_size) = packed_attributes_size {
            self.generate_packed_attribute_decls(packed_attributes_size);
        }

        self.generate_packed_varying_decls(packed_varyings_size);
    }

    fn generate_struct_decl(&mut self, decl: &StructDecl) {
        write!(self.string, "struct {} {{", decl.ident).unwrap();
        if !decl.fields.is_empty() {
            writeln!(self.string).unwrap();
            for field in &decl.fields {
                write!(self.string, "    ").unwrap();
                self.write_ident_and_ty(
                    field.ident,
                    field.ty_expr.ty.borrow().as_ref().unwrap(),
                );
                writeln!(self.string, ";").unwrap();
            }
        }
        writeln!(self.string, "}};").unwrap();
    }

    fn generate_const_decl(&mut self, decl: &ConstDecl) {
        write!(self.string, "const ").unwrap();
        self.write_ident_and_ty(
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, " = ").unwrap();
        self.generate_expr(&decl.expr);
        writeln!(self.string, ";").unwrap();
    }

    fn generate_uniform_decl(&mut self, decl: &UniformDecl) {
        write!(self.string, "uniform ").unwrap();
        self.write_ident_and_ty(
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }

    fn compute_packed_attributes_size(&self) -> usize {
        let mut packed_attributes_size = 0;
        for decl in &self.shader.decls {
            packed_attributes_size += match decl {
                Decl::Attribute(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                Decl::Instance(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_attributes_size
    }

    fn generate_packed_attribute_decls(
        &mut self,
        mut packed_attributes_size: usize
    ) {
        let mut packed_attribute_index = 0;
        loop {
            let packed_attribute_size = packed_attributes_size.min(4);
            writeln!(
                self.string,
                "attribute {} mpsc_packed_attribute_{};",
                match packed_attribute_size {
                    0 => break,
                    1 => "float",
                    2 => "vec2",
                    3 => "vec3",
                    4 => "vec4",
                    _ => panic!(),
                },
                packed_attribute_index,
            )
            .unwrap();
            packed_attributes_size -= packed_attribute_size;
            packed_attribute_index += 1;
        }
    }

    fn compute_packed_varyings_size(&self) -> usize {
        let mut packed_varyings_size = 0;
        for decl in &self.shader.decls {
            packed_varyings_size += match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    decl.ty_expr.ty.borrow().as_ref().unwrap().size()
                },
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    decl.ty_expr.ty.borrow().as_ref().unwrap().size()
                }
                Decl::Varying(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_varyings_size
    }

    fn generate_packed_varying_decls(
        &mut self,
        mut packed_varyings_size: usize
    ) {
        let mut packed_varying_index = 0;
        loop {
            let packed_varying_size = packed_varyings_size.min(4);
            writeln!(
                self.string,
                "varying {} mpsc_packed_varying_{};",
                match packed_varying_size {
                    0 => break,
                    1 => "float",
                    2 => "vec2",
                    3 => "vec3",
                    4 => "vec4",
                    _ => panic!(),
                },
                packed_varying_index,
            )
            .unwrap();
            packed_varyings_size -= packed_varying_size;
            packed_varying_index += 1;
        }
    }

    fn generate_fn_decl(&mut self, decl: &FnDecl) {
        for &callee in decl.callees.borrow().as_ref().unwrap().iter() {
            self.generate_fn_decl(self.shader.find_fn_decl(callee).unwrap());
        }
        self.write_ident_and_ty(
            decl.ident,
            decl.return_ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &decl.params {
            write!(self.string, "{}", sep).unwrap();
            self.write_ident_and_ty(
                param.ident,
                param.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            sep = ", ";
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&decl.block);
        writeln!(self.string).unwrap();
    }

    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader: self.shader,
            backend_writer: &GlslBackendWriter,
            use_hidden_params: false,
            use_generated_cons_fns: false,
            indent_level: 0,
            string: self.string
        }
        .generate_block(block)
    }

    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader: self.shader,
            backend_writer: &GlslBackendWriter,
            use_hidden_params: false,
            use_generated_cons_fns: false,
            string: self.string,
        }
        .generate_expr(expr)
    }

    fn write_ident_and_ty(&mut self, ident: Ident, ty: &Ty) {
        GlslBackendWriter.write_ident_and_ty(
            &mut self.string,
            ident,
            ty
        );
    }
}

struct VarPacker<'a> {
    packed_var_prefix: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String
}

impl<'a> VarPacker<'a> {
    fn new(
        packed_var_prefix: &'a str,
        packed_vars_size: usize,
        string: &'a mut String
    ) -> VarPacker<'a> {
        VarPacker {
            packed_var_prefix,
            packed_vars_size,
            packed_var_index: 0,
            packed_var_size: packed_vars_size.min(4),
            packed_var_offset: 0,
            string,
        }
    }

    fn pack_var(&mut self, ident: Ident, ty: &Ty) {
        let var_size = ty.size();
        let mut var_offset = 0;
        while var_offset < var_size {
            let count = var_size - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = count.min(packed_count);
            write!(self.string, "    {}_{}", self.packed_var_prefix, self.packed_var_index).unwrap();
            if self.packed_var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        self.packed_var_offset,
                        self.packed_var_offset + min_count
                    )
                )
                .unwrap();
            }
            write!(self.string, " = {}", ident).unwrap();
            if var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        var_offset,
                        var_offset + min_count
                    )
                )
                .unwrap();
            }
            writeln!(self.string, ";").unwrap();
            self.packed_var_offset += min_count;
            if self.packed_var_offset == self.packed_var_size {
                self.packed_vars_size -= self.packed_var_size;
                self.packed_var_index += 1;
                self.packed_var_size = self.packed_vars_size.min(4);
                self.packed_var_offset = 0;
            }
            var_offset += min_count; 
        }
    }
}

struct VarUnpacker<'a> {
    packed_var_prefix: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String
}

impl<'a> VarUnpacker<'a> {
    fn new(
        packed_var_prefix: &'a str,
        packed_vars_size: usize,
        string: &'a mut String
    ) -> VarUnpacker<'a> {
        VarUnpacker {
            packed_var_prefix,
            packed_vars_size,
            packed_var_index: 0,
            packed_var_size: packed_vars_size.min(4),
            packed_var_offset: 0,
            string,
        }
    }

    fn unpack_var(&mut self, ident: Ident, ty: &Ty) {
        let var_size = ty.size();
        let mut var_offset = 0;
        while var_offset < var_size {
            let count = var_size - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = count.min(packed_count);
            write!(self.string, "    {}", ident).unwrap();
            if var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        var_offset,
                        var_offset + min_count
                    )
                )
                .unwrap();
            }
            write!(self.string, " = {}_{}", self.packed_var_prefix, self.packed_var_index).unwrap();
            if self.packed_var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        self.packed_var_offset,
                        self.packed_var_offset + min_count
                    )
                )
                .unwrap();
            }
            writeln!(self.string, ";").unwrap();
            var_offset += min_count;
            self.packed_var_offset += min_count;
            if self.packed_var_offset == self.packed_var_size {
                self.packed_vars_size -= self.packed_var_size;
                self.packed_var_index += 1;
                self.packed_var_size = self.packed_vars_size.min(4);
                self.packed_var_offset = 0;
            } 
        }
    }
}

struct GlslBackendWriter;

impl BackendWriter for GlslBackendWriter {
    fn write_ty_lit(&self, string: &mut String, ty_lit: TyLit) {
        write!(string, "{}", ty_lit).unwrap();
    }
}