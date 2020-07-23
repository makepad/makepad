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
    std::collections::HashSet,
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
    
        
    fn write_ty_init(&mut self, ty: &Ty) {
        write!(self.string, "{}", match ty {
            Ty::Bool => "false",
            Ty::Int => "0",
            Ty::Float => "0.0",
            Ty::Bvec2 => "bvec2(0)",
            Ty::Bvec3 => "bvec3(0)",
            Ty::Bvec4 => "bvec4(0)",
            Ty::Ivec2 => "ivec2(0)",
            Ty::Ivec3 => "ivec3(0)",
            Ty::Ivec4 => "ivec4(0)",
            Ty::Vec2 => "vec2(0.0)",
            Ty::Vec3 => "vec3(0.0)",
            Ty::Vec4 => "vec4(0.0)",
            Ty::Mat2 => "mat2(0.0)",
            Ty::Mat3 => "mat3(0.0)",
            Ty::Mat4 => "mat4(0.0)",
            _ => panic!("unexpected as initializeable type"),
        }).unwrap()
    }
    
    fn generate_vertex_shader(&mut self) {
        let packed_attributes_size = self.compute_packed_attributes_size();
        let packed_instances_size = self.compute_packed_instances_size();
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_decls(
            Some(packed_attributes_size),
            Some(packed_instances_size),
            packed_varyings_size
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                },
                Decl::Instance(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                },
                Decl::Varying(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                },
                _ => {}
            }
        }
        let mut visited = HashSet::new();
        self.generate_fn_decl(
            self.shader.find_fn_decl(Ident::new("vertex")).unwrap(),
            &mut visited
        );
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
                _ => {}
            }
        }
        let mut instance_unpacker = VarUnpacker::new(
            "mpsc_packed_instance",
            packed_instances_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Instance(decl) => {
                    instance_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
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
        self.generate_decls(None, None, packed_varyings_size);
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());                    
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());                    
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Varying(decl) => {
                    self.write_ident_and_ty(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap()
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());                    
                    writeln!(self.string, ";").unwrap();
                },
                _ => {}
            }
        }
        let mut visited = HashSet::new();
        self.generate_fn_decl(
            self.shader.find_fn_decl(Ident::new("pixel")).unwrap(),
            &mut visited
        );
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
        packed_instances_size: Option<usize>,
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

        for decl in &self.shader.decls {
            match decl {
                Decl::Texture(decl) => self.generate_texture_decl(decl),
                _ => {}
            }
        }

        if let Some(packed_attributes_size) = packed_attributes_size {
            self.generate_packed_var_decls(
                "attribute",
                "mpsc_packed_attribute",
                packed_attributes_size
            );
        }

        if let Some(packed_instances_size) = packed_instances_size {
            self.generate_packed_var_decls(
                "attribute",
                "mpsc_packed_instance",
                packed_instances_size
            );
        }

        self.generate_packed_var_decls(
            "varying",
            "mpsc_packed_varying",
            packed_varyings_size,
        );
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

    fn generate_texture_decl(&mut self, decl: &TextureDecl) {
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
                _ => 0,
            }
        }
        packed_attributes_size
    }

    fn compute_packed_instances_size(&self) -> usize {
        let mut packed_instances_size = 0;
        for decl in &self.shader.decls {
            packed_instances_size += match decl {
                Decl::Instance(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_instances_size
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

    fn generate_packed_var_decls(
        &mut self,
        packed_var_qualifier: &'a str,
        packed_var_name: &'a str,
        mut packed_vars_size: usize
    ) {
        let mut packed_var_index = 0;
        loop {
            let packed_var_size = packed_vars_size.min(4);
            writeln!(
                self.string,
                "{} {} {}_{};",
                packed_var_qualifier,
                match packed_var_size {
                    0 => break,
                    1 => "float",
                    2 => "vec2",
                    3 => "vec3",
                    4 => "vec4",
                    _ => panic!(),
                },
                packed_var_name,
                packed_var_index,
            )
            .unwrap();
            packed_vars_size -= packed_var_size;
            packed_var_index += 1;
        }
    }

    fn generate_fn_decl(&mut self, decl: &FnDecl, visited: &mut HashSet<Ident>) {
        if visited.contains(&decl.ident) {
            return;
        }
        for &callee in decl.callees.borrow().as_ref().unwrap().iter() {
            self.generate_fn_decl(self.shader.find_fn_decl(callee).unwrap(), visited);
        }
        self.write_ident_and_ty(
            decl.ident,
            decl.return_ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &decl.params {
            write!(self.string, "{}", sep).unwrap();
            if param.is_inout {
                write!(self.string, "inout ").unwrap();
            }
            self.write_ident_and_ty(
                param.ident,
                param.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            sep = ", ";
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&decl.block);
        writeln!(self.string).unwrap();
        visited.insert(decl.ident);
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
    packed_var_name: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String
}

impl<'a> VarPacker<'a> {
    fn new(
        packed_var_name: &'a str,
        packed_vars_size: usize,
        string: &'a mut String
    ) -> VarPacker<'a> {
        VarPacker {
            packed_var_name,
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
            write!(self.string, "    {}_{}", self.packed_var_name, self.packed_var_index).unwrap();
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
    packed_var_name: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String
}

impl<'a> VarUnpacker<'a> {
    fn new(
        packed_var_name: &'a str,
        packed_vars_size: usize,
        string: &'a mut String
    ) -> VarUnpacker<'a> {
        VarUnpacker {
            packed_var_name,
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
            write!(self.string, " = {}_{}", self.packed_var_name, self.packed_var_index).unwrap();
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
        write!(string, "{}", match ty_lit {
            TyLit::Bool => "bool",
            TyLit::Int => "int",
            TyLit::Float => "float",
            TyLit::Bvec2 => "bvec2",
            TyLit::Bvec3 => "bvec3",
            TyLit::Bvec4 => "bvec4",
            TyLit::Ivec2 => "ivec2",
            TyLit::Ivec3 => "ivec3",
            TyLit::Ivec4 => "ivec4",
            TyLit::Vec2 => "vec2",
            TyLit::Vec3 => "vec3",
            TyLit::Vec4 => "vec4",
            TyLit::Mat2 => "mat2",
            TyLit::Mat3 => "mat3",
            TyLit::Mat4 => "mat4",
            TyLit::Texture2D => "sampler2D",
        }).unwrap();
    }

}