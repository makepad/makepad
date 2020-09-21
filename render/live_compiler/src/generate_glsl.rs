use {
    crate::{
        shaderast::*,
        span::Span,
        env::{Env, VarKind},
        analyse::ShaderCompileOptions,
        generate::{BackendWriter, BlockGenerator, ExprGenerator},
        ident::{Ident, IdentPath},
        livestyles::LiveStyles,
        lit::TyLit,
        swizzle::Swizzle,
        ty::Ty,
    },
    std::collections::HashSet,
    std::fmt::Write,
    std::cell::Cell,
};

pub fn generate_vertex_shader(shader: &ShaderAst, live_styles: &LiveStyles, options:ShaderCompileOptions) -> String {
    let mut string = String::new();
    let env = Env::new(live_styles);
    ShaderGenerator {
        shader,
        env: &env,
        create_const_table: options.create_const_table,
        string: &mut string,
        backend_writer: &GlslBackendWriter {env: &env}
    }
    .generate_vertex_shader();
    string
}

pub fn generate_fragment_shader(shader: &ShaderAst, live_styles: &LiveStyles, options:ShaderCompileOptions) -> String {
    let mut string = String::new();
    let env = Env::new(live_styles);
    ShaderGenerator {
        shader,
        env: &env,
        create_const_table: options.create_const_table,
        string: &mut string,
        backend_writer: &GlslBackendWriter {env: &env}
    }
    .generate_fragment_shader();
    string
}

struct ShaderGenerator<'a, 'b> {
    shader: &'a ShaderAst,
    env: &'a Env<'b>,
    create_const_table: bool,
    string: &'a mut String,
    backend_writer: &'a dyn BackendWriter
}

impl<'a, 'b> ShaderGenerator<'a, 'b> {
    fn write_ty_init(&mut self, ty: &Ty) {
        write!(
            self.string,
            "{}",
            match ty {
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
            }
        )
            .unwrap()
    }
    
    fn generate_vertex_shader(&mut self) {
        let packed_geometries_size = self.compute_packed_geometries_size();
        let packed_instances_size = self.compute_packed_instances_size();
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_decls(
            Some(packed_geometries_size),
            Some(packed_instances_size),
            packed_varyings_size,
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) => {
                    self.write_var_decl(
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Instance(decl) => {
                    self.write_var_decl(
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Varying(decl) => {
                    self.write_var_decl(
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        let vertex_decl = self.shader.find_fn_decl(IdentPath::from_str("vertex")).unwrap();
        for &(ty_lit, ref param_tys) in vertex_decl
            .cons_fn_deps
            .borrow_mut()
            .as_ref()
            .unwrap()
        {
            self.generate_cons_fn(ty_lit, param_tys);
        }
        self.generate_fn_decl(vertex_decl, self.backend_writer);
        writeln!(self.string, "void main() {{").unwrap();
        let mut geometry_unpacker = VarUnpacker::new(
            "mpsc_packed_geometry",
            packed_geometries_size,
            &mut self.string,
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) => {
                    geometry_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                _ => {}
            }
        }
        let mut instance_unpacker = VarUnpacker::new(
            "mpsc_packed_instance",
            packed_instances_size,
            &mut self.string,
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Instance(decl) => {
                    instance_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                _ => {}
            }
        }
        writeln!(self.string, "    gl_Position = vertex();").unwrap();
        let mut varying_packer = VarPacker::new(
            "mpsc_packed_varying",
            packed_varyings_size,
            &mut self.string,
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_packer.pack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_packer.pack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                Decl::Varying(decl) => {
                    varying_packer.pack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
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
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    self.write_var_decl(
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    self.write_var_decl(
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                Decl::Varying(decl) => {
                    self.write_var_decl(
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        
        let pixel_decl = self.shader.find_fn_decl(IdentPath::from_str("pixel")).unwrap();
        for &(ty_lit, ref param_tys) in pixel_decl
            .cons_fn_deps
            .borrow_mut()
            .as_ref()
            .unwrap()
        {
            self.generate_cons_fn(ty_lit, param_tys);
        }
        self.generate_fn_decl(pixel_decl, self.backend_writer);
        writeln!(self.string, "void main() {{").unwrap();
        let mut varying_unpacker = VarUnpacker::new(
            "mpsc_packed_varying",
            packed_varyings_size,
            &mut self.string,
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                Decl::Varying(decl) => {
                    varying_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
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
        packed_varyings_size: usize,
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
        
        if self.create_const_table {
            writeln!(
                self.string,
                "uniform float mpsc_const_table[{}];",
                self.shader.const_table.borrow().as_ref().unwrap().len()
            ).unwrap();
        }
        
        for decl in &self.shader.decls {
            match decl {
                Decl::Uniform(decl) => self.generate_uniform_decl(decl),
                _ => {}
            }
        }
        
        // lets output the livestyle uniforms
        for (ty, ident_path) in self.shader.livestyle_uniform_deps.borrow().as_ref().unwrap() {
            // we have a span and an ident_path.
            // lets fully qualify it
            write!(self.string, "uniform {} mpsc_live_", ty).unwrap();
            ident_path.write_underscored_ident(self.string);
            writeln!(self.string, ";").unwrap();
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
                "mpsc_packed_geometry",
                packed_attributes_size,
            );
        }
        
        if let Some(packed_instances_size) = packed_instances_size {
            self.generate_packed_var_decls(
                "attribute",
                "mpsc_packed_instance",
                packed_instances_size,
            );
        }
        
        self.generate_packed_var_decls("varying", "mpsc_packed_varying", packed_varyings_size);
    }
    
    fn generate_struct_decl(&mut self, decl: &StructDecl) {
        write!(self.string, "struct {} {{", decl.ident).unwrap();
        if !decl.fields.is_empty() {
            writeln!(self.string).unwrap();
            for field in &decl.fields {
                write!(self.string, "    ").unwrap();
                self.write_var_decl(
                    false,
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
        self.write_var_decl(
            false,
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, " = ").unwrap();
        self.generate_expr(&decl.expr);
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_uniform_decl(&mut self, decl: &UniformDecl) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            false,
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_texture_decl(&mut self, decl: &TextureDecl) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            false,
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn compute_packed_geometries_size(&self) -> usize {
        let mut packed_attributes_size = 0;
        for decl in &self.shader.decls {
            packed_attributes_size += match decl {
                Decl::Geometry(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
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
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    decl.ty_expr.ty.borrow().as_ref().unwrap().size()
                }
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
        mut packed_vars_size: usize,
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
            self.write_var_decl(false, Ident::new("x"), &param_tys[0])
        } else {
            for (index, param_ty) in param_tys.iter().enumerate() {
                write!(self.string, "{}", sep).unwrap();
                self.write_var_decl(false, Ident::new(format!("x{}", index)), param_ty);
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
    
    
    fn generate_fn_decl(&mut self, decl: &FnDecl, backend_writer: &dyn BackendWriter) {
        FnDeclGenerator {
            shader: self.shader,
            decl,
            create_const_table: self.create_const_table,
            backend_writer,
            visited: &mut HashSet::new(),
            string: self.string,
        }
        .generate_fn_decl()
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
    
    fn write_var_decl(&mut self, is_inout: bool, ident: Ident, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, is_inout, false, ident, ty);
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
                decl: self.shader.find_fn_decl(callee).unwrap(),
                create_const_table: self.create_const_table,
                visited: self.visited,
                backend_writer: self.backend_writer,
                string: self.string,
            }
            .generate_fn_decl()
        }
        self.write_var_decl(
            false,
            self.decl.ident_path.to_struct_fn_ident(), // here we must expand IdentPath to something
            self.decl.return_ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &self.decl.params {
            write!(self.string, "{}", sep).unwrap();
            self.write_var_decl(
                param.is_inout,
                param.ident,
                param.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            sep = ", ";
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
            //use_generated_cons_fns: false,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
    
    fn write_var_decl(&mut self, is_inout: bool, ident: Ident, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, is_inout, false, ident, ty);
    }
}

struct VarPacker<'a> {
    packed_var_name: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String,
}

impl<'a> VarPacker<'a> {
    fn new(
        packed_var_name: &'a str,
        packed_vars_size: usize,
        string: &'a mut String,
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
        let mut in_matrix = None;
        while var_offset < var_size {
            let count = var_size - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = if var_size > 4 {1} else {count.min(packed_count)};
            write!(
                self.string,
                "    {}_{}",
                self.packed_var_name,
                self.packed_var_index
            )
                .unwrap();
            if self.packed_var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(self.packed_var_offset, self.packed_var_offset + min_count)
                )
                    .unwrap();
            }
            write!(self.string, " = {}", ident).unwrap();
            if var_size > 1 {
                if var_size <= 4 {
                    in_matrix = None;
                    write!(
                        self.string,
                        ".{}",
                        Swizzle::from_range(var_offset, var_offset + min_count)
                    )
                        .unwrap();
                }
                else { // make a matrix constructor and unpack into it
                    if let Some(in_matrix) = &mut in_matrix {
                        *in_matrix += 1;
                    }
                    else {
                        in_matrix = Some(0);
                    }
                    if let Some(in_matrix) = in_matrix {
                        write!(
                            self.string,
                            "[{}][{}]",
                            in_matrix >> 2,
                            in_matrix & 3,
                        ).unwrap();
                    }
                }
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
    string: &'a mut String,
}

impl<'a> VarUnpacker<'a> {
    fn new(
        packed_var_name: &'a str,
        packed_vars_size: usize,
        string: &'a mut String,
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
        let mut in_matrix = None;
        while var_offset < var_size {
            let count = var_size - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = if var_size > 4 {1} else {count.min(packed_count)};
            write!(self.string, "    {}", ident).unwrap();
            if var_size > 1 {
                if var_size <= 4 { // its a matrix
                    in_matrix = None;
                    write!(
                        self.string,
                        ".{}",
                        Swizzle::from_range(var_offset, var_offset + min_count)
                    )
                        .unwrap();
                }
                else {
                    if let Some(in_matrix) = &mut in_matrix {
                        *in_matrix += 1;
                    }
                    else {
                        in_matrix = Some(0);
                    }
                    if let Some(in_matrix) = in_matrix {
                        write!(
                            self.string,
                            "[{}][{}]",
                            in_matrix >> 2,
                            in_matrix & 3,
                        ).unwrap();
                    }
                }
            }
            write!(
                self.string,
                " = {}_{}",
                self.packed_var_name,
                self.packed_var_index
            )
                .unwrap();
            
            if self.packed_var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(self.packed_var_offset, self.packed_var_offset + min_count)
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

struct GlslBackendWriter<'a, 'b> {
    pub env: &'a Env<'b>
}

impl<'a, 'b> BackendWriter for GlslBackendWriter<'a, 'b> {
    fn write_call_expr_hidden_args(&self, _string: &mut String, _use_const_table: bool, _ident_path: IdentPath, _shader: &ShaderAst, _sep: &str) {
        
    }
    
    fn generate_var_expr(&self, string: &mut String, span: Span, ident_path: IdentPath, _kind: &Cell<Option<VarKind >>, _shader: &ShaderAst, _decl: &FnDecl, _ty: &Option<Ty>) {
        // we have env. so now we need to map ident_paths to
        // fully qualified uniform names.
        if ident_path.len()>1 {
            let qualified = self.env.qualify_ident_path(span.live_body_id, ident_path);
            write!(string, "mpsc_live_").unwrap();
            qualified.write_underscored_ident(string);
        }
        else {
            write!(string, "{}", ident_path.get_single().expect("unexpected")).unwrap()
        }
    }
    
    fn needs_mul_fn_for_matrix_multiplication(&self) -> bool {
        false
    }
    
    fn needs_unpack_for_matrix_multiplication(&self) -> bool {
        false
    }
    
    fn const_table_is_vec4(&self) -> bool {
        false
    }
    
    fn use_cons_fn(&self, _what: &str) -> bool {
        false
    }
    
    
    fn write_var_decl(
        &self,
        string: &mut String,
        is_inout: bool,
        is_packed: bool,
        ident: Ident,
        ty: &Ty,
    ) {
        if is_inout {
            write!(string, "inout ").unwrap();
        }
        match *ty {
            Ty::Void => {
                write!(string, "void ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bool => {
                self.write_ty_lit(string, TyLit::Bool);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Int => {
                self.write_ty_lit(string, TyLit::Int);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Float => {
                self.write_ty_lit(string, TyLit::Float);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bvec2 => {
                self.write_ty_lit(string, TyLit::Bvec2);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bvec3 => {
                self.write_ty_lit(string, TyLit::Bvec3);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Bvec4 => {
                self.write_ty_lit(string, TyLit::Bvec4);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Ivec2 => {
                self.write_ty_lit(string, TyLit::Ivec2);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Ivec3 => {
                self.write_ty_lit(string, TyLit::Ivec3);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Ivec4 => {
                self.write_ty_lit(string, TyLit::Ivec4);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Vec2 => {
                self.write_ty_lit(string, TyLit::Vec2);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Vec3 => {
                self.write_ty_lit(string, TyLit::Vec3);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Vec4 => {
                self.write_ty_lit(string, TyLit::Vec4);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Mat2 => {
                self.write_ty_lit(string, TyLit::Mat2);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Mat3 => {
                self.write_ty_lit(string, TyLit::Mat3);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Mat4 => {
                self.write_ty_lit(string, TyLit::Mat4);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Texture2D => {
                self.write_ty_lit(string, TyLit::Texture2D);
                write!(string, " ").unwrap();
                self.write_ident(string, ident);
            }
            Ty::Array {ref elem_ty, len} => {
                self.write_var_decl(string, is_inout, is_packed, ident, elem_ty);
                write!(string, "[{}]", len).unwrap();
            }
            Ty::Struct {
                ident: struct_ident,
            } => {
                write!(string, "{} ", struct_ident).unwrap();
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
            }
        )
            .unwrap();
    }
    
    fn write_call_ident(&self, string: &mut String, ident: Ident, _arg_exprs: &[Expr]) {
        self.write_ident(string, ident);
    }
    
    fn write_ident(&self, string: &mut String, ident: Ident) {
        ident.with( | ident_string | {
            if ident_string.contains("::") {
                write!(string, "mpsc_{}", ident_string.replace("::", "_")).unwrap()
            } else {
                write!(
                    string,
                    "{}",
                    match ident_string.as_ref() {
                        "union" => "mpsc_union",
                        _ => ident_string,
                    }
                )
                    .unwrap()
            }
        })
    }
}
