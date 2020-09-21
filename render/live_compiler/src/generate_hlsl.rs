use {
    crate::{
        shaderast::*,
        env::{VarKind, Env},
        span::Span,
        analyse::ShaderCompileOptions,
        generate::{BackendWriter, BlockGenerator, ExprGenerator},
        ident::{Ident,IdentPath},
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

pub fn index_to_char(index: usize) -> char {
    std::char::from_u32(index as u32 + 65).unwrap()
}

pub fn generate_shader(shader: &ShaderAst, live_styles: &LiveStyles, options:ShaderCompileOptions) -> String {
    let mut string = String::new();
    let env = Env::new(live_styles);
    ShaderGenerator {
        shader,
        create_const_table: options.create_const_table,
        string: &mut string,
        env: &env,
        backend_writer: &HlslBackendWriter {env: &env}
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
        writeln!(self.string, "SamplerState mpsc_default_texture_sampler{{Filter=MIN_MAX_MIP_LINEAR;AddressU = Wrap;AddressV=Wrap;}};").unwrap();
        writeln!(self.string, "float4 sample2d(Texture2D tex, float2 pos){{return tex.Sample(mpsc_default_texture_sampler,pos);}}").unwrap();
        self.generate_struct_decls();
        self.generate_uniform_structs();
        self.generate_texture_defs();
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
            let register;
            if ident == Ident::new("pass") {
                register = 0;
            }
            else if ident == Ident::new("view") {
                register = 1;
            }
            else if ident == Ident::new("draw") {
                register = 2;
            }
            else if ident == Ident::new("default") {
                register = 3;
                has_default = true;
            }
            else {
                panic!("extra uniform blocks not supported");
            }
            
            writeln!(self.string, "cbuffer mpsc_{}_Uniforms : register(b{}) {{", ident, register).unwrap();
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
            writeln!(self.string, "cbuffer mpsc_default_Uniforms : register(b3){{}};").unwrap();
        }
        writeln!(self.string, "cbuffer mpsc_live_Uniforms : register(b4) {{").unwrap();
        for (ty, ident_path) in self.shader.livestyle_uniform_deps.borrow().as_ref().unwrap() {
            // we have a span and an ident_path.
            // lets fully qualify it
            write!(self.string, "    {} mpsc_live_", ty).unwrap();
            ident_path.write_underscored_ident(self.string);
            writeln!(self.string, ";").unwrap();
        }
        writeln!(self.string, "}}").unwrap();
        
        if self.create_const_table{
            if let Some(const_table) = self.shader.const_table.borrow_mut().as_mut(){
                // lets use a float4 table.
                let size = const_table.len();
                let align_gap = 4 - (size - ((size>>2) << 2));
                for _ in 0..align_gap{
                    const_table.push(0.0);
                }
                writeln!(self.string, "cbuffer mspc_const_Table : register(b5){{float4 mpsc_const_table[{}];}};",const_table.len()>>2).unwrap();
            };
        }
    }
    
    fn generate_texture_defs(&mut self) {
        let mut index = 0;
        //writeln!(self.string, "struct mpsc_Textures {{").unwrap();
        for decl in &self.shader.decls {
            match decl {
                Decl::Texture(decl) => {
                    assert_eq!(*decl.ty_expr.ty.borrow().as_ref().unwrap(), Ty::Texture2D);
                    writeln!(
                        self.string,
                        "Texture2D {}: register(t{});",
                        decl.ident,
                        index
                    )
                        .unwrap();
                    index += 1;
                }
                _ => {}
            }
        }
        // writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_geometry_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Geometries {{").unwrap();
        let mut index = 0;
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
                    writeln!(self.string, ": GEOM{};", index_to_char(index)).unwrap();
                    index += 1;
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_instance_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Instances {{").unwrap();
        let mut index = 0;
        for decl in &self.shader.decls {
            match decl {
                Decl::Instance(decl) => {
                    match decl.ty_expr.ty.borrow().as_ref().unwrap(){
                        Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 =>{
                            write!(self.string, "    ").unwrap();
                            self.write_var_decl(
                                false,
                                true,
                                decl.ident,
                                decl.ty_expr.ty.borrow().as_ref().unwrap(),
                            );
                            writeln!(self.string, ": INST{};", index_to_char(index)).unwrap();
                            index += 1;
                        },
                        Ty::Mat4 =>{
                            for i in 0..4{
                                write!(self.string, "    ").unwrap();
                                self.write_ty_lit(TyLit::Vec4);
                                write!(self.string, " {}{}", decl.ident, i).unwrap();
                                writeln!(self.string, ": INST{};", index_to_char(index)).unwrap();
                                index += 1;                                
                            }
                        },
                        Ty::Mat3=>{
                            for i in 0..3{
                                write!(self.string, "    ").unwrap();
                                self.write_ty_lit(TyLit::Vec3);
                                write!(self.string, " {}{}", decl.ident, i).unwrap();
                                writeln!(self.string, ": INST{};", index_to_char(index)).unwrap();
                                index += 1;                                
                            }
                        },
                        Ty::Mat2 =>{
                            write!(self.string, "    ").unwrap();
                            self.write_ty_lit(TyLit::Vec4);
                            write!(self.string, " {}", decl.ident).unwrap();
                            writeln!(self.string, ": INST{};", index_to_char(index)).unwrap();
                            index += 1;                                
                        },
                        _=>panic!("unsupported type in generate_instance_struct")
                    }
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_varying_struct(&mut self) {
        writeln!(self.string, "struct mpsc_Varyings {{").unwrap();
        writeln!(self.string, "    float4 mpsc_position: SV_POSITION;").unwrap();
        let mut index = 0;
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
                    writeln!(self.string, ": VARY{};", index_to_char(index)).unwrap();
                    index += 1;
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ": VARY{};", index_to_char(index)).unwrap();
                    index += 1;
                }
                Decl::Varying(decl) => {
                    write!(self.string, "    ").unwrap();
                    self.write_var_decl(
                        false,
                        false,
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    writeln!(self.string, ": VARY{};", index_to_char(index)).unwrap();
                    index += 1;
                }
                _ => {}
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_varying_init(&mut self) {
        write!(self.string, "{{").unwrap();
        write!(self.string, "float4(0.0,0.0,0.0,0.0)").unwrap();
        let sep = ", ";
        for decl in &self.shader.decls {
            match decl {
                Decl::Geometry(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "{}", sep).unwrap();
                    self.write_var_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    write!(self.string, "{}", sep).unwrap();
                    self.write_var_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                Decl::Varying(decl) => {
                    write!(self.string, "{}", sep).unwrap();
                    self.write_var_init(decl.ty_expr.ty.borrow().as_ref().unwrap());
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
                    write!(self.string, "static const ").unwrap();
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
        if !self.backend_writer.use_cons_fn(&cons_name){
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
            backend_writer: self.backend_writer,
            create_const_table: self.create_const_table,
            visited,
            string: self.string,
        }
        .generate_fn_decl()
    }
    
    fn generate_vertex_main(&mut self) {
        let decl = self.shader.find_fn_decl(IdentPath::from_str("vertex")).unwrap();
        write!(self.string, "mpsc_Varyings mpsc_vertex_main(").unwrap();
        write!(
            self.string,
            "mpsc_Geometries mpsc_geometries"
        )
            .unwrap();
        write!(
            self.string,
            ", mpsc_Instances mpsc_instances"
        )
            .unwrap();
        write!(
            self.string,
            ", uint inst_id: SV_InstanceID"
        )
            .unwrap();
        writeln!(self.string, ") {{").unwrap();
        writeln!(self.string, "    mpsc_Varyings mpsc_varyings = ").unwrap();
        self.generate_varying_init();
        write!(self.string, "    mpsc_varyings.mpsc_position = ").unwrap();
        self.write_ident(decl.ident_path.get_single().expect("unexpected"));
        write!(self.string, "(").unwrap();
        let mut sep = "";
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
        write!(self.string, "float4 mpsc_fragment_main(").unwrap();
        write!(self.string, "mpsc_Varyings mpsc_varyings").unwrap();
        writeln!(self.string, ") : SV_TARGET{{").unwrap();
        
        write!(self.string, "    return ").unwrap();
        self.write_ident(decl.ident_path.get_single().expect("unexpected"));
        write!(self.string, "(").unwrap();
        
        let has_geometry_deps = !decl.geometry_deps.borrow().as_ref().unwrap().is_empty();
        let has_instance_deps = !decl.instance_deps.borrow().as_ref().unwrap().is_empty();
        let has_varying_deps = decl.has_varying_deps.get().unwrap();
        if has_geometry_deps || has_instance_deps || has_varying_deps {
            write!(self.string, "mpsc_varyings").unwrap();
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
            //use_generated_cons_fns: true,
            string: self.string,
        }
        .generate_expr(expr)
    }
    
    fn write_var_init(&mut self, ty: &Ty) {
        match ty {
            Ty::Bool => write!(self.string, "false").unwrap(),
            Ty::Int => write!(self.string, "0").unwrap(),
            Ty::Float => write!(self.string, "0.0").unwrap(),
            Ty::Bvec2 => write!(self.string, "bool2(0,0)").unwrap(),
            Ty::Bvec3 => write!(self.string, "bool3(0,0,0)").unwrap(),
            Ty::Bvec4 => write!(self.string, "bool4(0,0,0,0)").unwrap(),
            Ty::Ivec2 => write!(self.string, "int2(0,0)").unwrap(),
            Ty::Ivec3 => write!(self.string, "int3(0,0,0)").unwrap(),
            Ty::Ivec4 => write!(self.string, "int4(0,0,0,0)").unwrap(),
            Ty::Vec2 => write!(self.string, "float2(0.0,0.0)").unwrap(),
            Ty::Vec3 => write!(self.string, "float3(0.0,0.0,0.0)").unwrap(),
            Ty::Vec4 => write!(self.string, "float4(0.0,0.0,0.0,0.0)").unwrap(),
            Ty::Mat2 => write!(self.string, "float2x2(0.0,0.0,0.0,0.0)").unwrap(),
            Ty::Mat3 => write!(self.string, "float3x3(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0").unwrap(),
            Ty::Mat4 => write!(self.string, "float4x4(0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0").unwrap(),
            _ => panic!("Implement init for type")
        }
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
                backend_writer: self.backend_writer,
                shader: self.shader,
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
                    "{}in mpsc_Geometries mpsc_geometries",
                    sep
                )
                    .unwrap();
                sep = ", ";
            }
            if has_instance_deps {
                write!(self.string, "{}in mpsc_Instances mpsc_instances", sep).unwrap();
                sep = ", ";
            }
            if has_varying_deps {
                write!(self.string, "{}inout mpsc_Varyings mpsc_varyings", sep).unwrap();
            }
        }
        if is_used_in_fragment_shader {
            if has_geometry_deps || has_instance_deps || has_varying_deps {
                write!(self.string, "{}inout mpsc_Varyings mpsc_varyings", sep).unwrap();
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
            //use_generated_cons_fns: true,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
    
    fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, is_inout, is_packed, ident, ty);
    }
}

struct HlslBackendWriter<'a, 'b>{
    pub env: &'a Env<'b>
}

impl<'a, 'b> BackendWriter for HlslBackendWriter<'a, 'b> {
    
    fn write_call_expr_hidden_args(&self, string: &mut String, _use_const_table: bool, ident_path: IdentPath, shader: &ShaderAst, sep: &str) {
        if let Some(decl) = shader.find_fn_decl(ident_path) {
            let mut sep = sep;
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
    
    
    fn generate_var_expr(&self, string: &mut String, span:Span, ident_path: IdentPath, kind: &Cell<Option<VarKind>>, _shader: &ShaderAst, decl: &FnDecl, ty:&Option<Ty>) {
        
        let is_used_in_vertex_shader = decl.is_used_in_vertex_shader.get().unwrap();
        let is_used_in_fragment_shader = decl.is_used_in_fragment_shader.get().unwrap();
        if is_used_in_vertex_shader {
            match kind.get().unwrap() {
                VarKind::Geometry => write!(string, "mpsc_geometries.").unwrap(),
                VarKind::Instance =>{
                    match ty{
                        Some(Ty::Mat4)=>{
                            write!(string, "float4x4(").unwrap();
                            for i in 0..4{
                                if i != 0{
                                    write!(string, ",").unwrap();
                                }
                                write!(string, "mpsc_instances.").unwrap();
                                write!(string, "{}{}", ident_path.get_single().expect("unexpected"), i).unwrap();
                            }
                            write!(string, ")").unwrap();
                            return
                        },
                        Some(Ty::Mat3)=>{
                            write!(string, "float3x3(").unwrap();
                            for i in 0..3{
                                if i != 0{
                                    write!(string, ",").unwrap();
                                }
                                write!(string, "mpsc_instances.").unwrap();
                                write!(string, "{}{}", ident_path.get_single().expect("unexpected"), i).unwrap();
                            }
                            write!(string, ")").unwrap();
                            return
                        },
                        Some(Ty::Mat2)=>{
                            write!(string, "float2x2(").unwrap();
                            write!(string, "mpsc_instances.").unwrap();
                            write!(string, ")").unwrap();
                            return
                        },
                        _=>{
                            write!(string, "mpsc_instances.").unwrap();
                        }
                    }
                },
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
        match kind.get().unwrap(){
            VarKind::LiveStyle => {
                let qualified = self.env.qualify_ident_path(span.live_body_id, ident_path);
                write!(string, "mpsc_live_").unwrap();
                qualified.write_underscored_ident(string);
                return
            },
            _ => {}
        }
        
        write!(string, "{}", ident_path.get_single().expect("unexpected")).unwrap();
    }
    
    fn needs_mul_fn_for_matrix_multiplication(&self) -> bool {
        true
    }

    fn needs_unpack_for_matrix_multiplication(&self)->bool{
        false
    }

    fn  const_table_is_vec4(&self) -> bool{
        true
    }

    fn use_cons_fn(&self, what:&str)->bool{
        match what{
            "mpsc_vec4_float_float_float_float"=>false,
            "mpsc_vec3_float_float_float"=>false,
            "mpsc_vec2_float_float"=>false,
            _=>true
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
        if is_inout {
            write!(string, "inout ").unwrap();
        }
        //let packed_prefix = if is_packed { "packed_" } else { "" };
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
            Ty::Texture2D => panic!(), // TODO
            Ty::Array {ref elem_ty, len} => {
                self.write_var_decl(string, is_inout, is_packed, ident, elem_ty);
                write!(string, " ").unwrap();
                write!(string, "[{}]", len).unwrap();
            }
            Ty::Struct {
                ident: struct_ident,
            } => {
                write!(string, "{}", struct_ident).unwrap();
                write!(string, " ").unwrap();
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
        else if ident == Ident::new("mix") {
            write!(string, "lerp").unwrap();
        }
        else if ident == Ident::new("dFdx") {
            write!(string, "ddx").unwrap();
        }
        else if ident == Ident::new("dFdy") {
            write!(string, "ddy").unwrap();
        }
        else if ident == Ident::new("fract") {
            write!(string, "frac").unwrap();
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
                        "frac"=>"mpsc_frac",
                        "thread" => "mpsc_thread",
                        "device" => "mpsc_device",
                        "ddx" => "mpsc_ddx",
                        "ddy" => "mpsc_ddy",
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
