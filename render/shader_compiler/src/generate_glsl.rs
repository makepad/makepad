use crate::shaderast::*;
use makepad_live_parser::*;
use crate::generate::*;
use crate::swizzle::Swizzle;
use std::fmt::Write;
use std::fmt;
use std::collections::BTreeSet;
use crate::shaderregistry::ShaderRegistry;
use crate::shaderregistry::FinalConstTable;

pub fn generate_vertex_shader(draw_shader_def: &DrawShaderDef, final_const_table: &FinalConstTable, shader_registry: &ShaderRegistry) -> String {
    let mut string = String::new();
    DrawShaderGenerator {
        draw_shader_def,
        shader_registry,
        final_const_table,
        string: &mut string,
        backend_writer: &GlslBackendWriter {shader_registry}
    }
    .generate_vertex_shader();
    string
}

pub fn generate_pixel_shader(draw_shader_def: &DrawShaderDef, final_const_table: &FinalConstTable, shader_registry: &ShaderRegistry) -> String {
    let mut string = String::new();
    DrawShaderGenerator {
        draw_shader_def,
        shader_registry,
        final_const_table,
        string: &mut string,
        backend_writer: &GlslBackendWriter {shader_registry}
    }
    .generate_pixel_shader();
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
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    self.write_var_decl(
                        false,
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Instance {..} => {
                    self.write_var_decl(
                        false,
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Varying {..} => {
                    self.write_var_decl(
                        false,
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        
        // we need to use the all_fns to compute our const table offsets.
        
        self.generate_shader_body(&self.draw_shader_def.vertex_fns.borrow(), &self.draw_shader_def.vertex_structs.borrow());
        
        writeln!(self.string, "void main() {{").unwrap();
        let mut geometry_unpacker = VarUnpacker::new(
            "packed_geometry",
            packed_geometries_size,
            &mut self.string,
        );
        for decl in &self.draw_shader_def.fields {
            match decl.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    geometry_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                _ => {}
            }
        }
        let mut instance_unpacker = VarUnpacker::new(
            "packed_instance",
            packed_instances_size,
            &mut self.string,
        );
        for decl in &self.draw_shader_def.fields {
            match decl.kind {
                DrawShaderFieldKind::Instance {..} => {
                    instance_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                _ => {}
            }
        }
        
        let vertex_decl = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap();
        
        writeln!(self.string, "    gl_Position = {}();", DisplayFnName(vertex_decl.fn_node_ptr, vertex_decl.ident)).unwrap();
        let mut varying_packer = VarPacker::new(
            "packed_varying",
            packed_varyings_size,
            &mut self.string,
        );
        for decl in &self.draw_shader_def.fields {
            match &decl.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    varying_packer.pack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    varying_packer.pack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                DrawShaderFieldKind::Varying {..} => {
                    varying_packer.pack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                _ => {}
            }
        }
        writeln!(self.string, "}}").unwrap();
    }
    
    pub fn generate_shader_body(&mut self, fn_deps: &Vec<FnNodePtr>, struct_deps: &Vec<StructNodePtr>) {
        
        // alright so. we have our fn deps which have struct deps
        // and we have struct deps in our struct deps.
        let mut all_consts = BTreeSet::new();
        let mut all_constructor_fns = BTreeSet::new();
        let mut all_live_refs = BTreeSet::new();
        
        for callee in fn_deps.iter().rev() {
            let decl = self.shader_registry.all_fns.get(callee).unwrap();
            all_constructor_fns.extend(decl.constructor_fn_deps.borrow().as_ref().unwrap().iter().cloned());
            all_consts.extend(decl.const_refs.borrow().as_ref().unwrap().iter().cloned());
            all_live_refs.extend(decl.live_refs.borrow().as_ref().unwrap().iter().cloned());
        }
        // we have all the structs already from analyse
        for struct_ptr in struct_deps.iter().rev() {
            let struct_decl = self.shader_registry.structs.get(struct_ptr).unwrap();
            self.generate_struct_decl(*struct_ptr, struct_decl);
        }
        
        for (ty_lit, param_tys) in all_constructor_fns
        {
            self.generate_cons_fn(ty_lit, &param_tys);
        }
        
        for const_node_ptr in all_consts
        {
            let const_decl = self.shader_registry.consts.get(&const_node_ptr).unwrap();
            self.generate_const_def(const_node_ptr, const_decl);
        }
        
        for fn_iter in fn_deps.iter().rev() {
            let offset = self.final_const_table.offsets.get(fn_iter).cloned();
            let fn_def = self.shader_registry.all_fns.get(fn_iter).unwrap();
            if fn_def.has_closure_args() {
                for call_iter in fn_deps.iter().rev() {
                    // any function that depends on us, will have the closures we need
                    let call_def = self.shader_registry.all_fns.get(call_iter).unwrap();
                    if call_def.callees.borrow().as_ref().unwrap().contains(&fn_iter) {
                        self.generate_fn_def_with_closure_args(fn_def, call_def, self.backend_writer, offset);
                    }
                }
                continue
            }
            
            self.generate_fn_def(fn_def, self.backend_writer, offset);
        }
    }
    
    
    fn generate_fn_def_with_closure_args(
        &mut self,
        fn_def: &FnDef,
        call_def: &FnDef,
        backend_writer: &dyn BackendWriter,
        const_table_offset: Option<usize>
    ) {
        // so first we are collecting the closures in defs that are actually used
        for (closure_def_index, closure_def) in call_def.closure_defs.iter().enumerate() {
            let closure_def_index = ClosureDefIndex(closure_def_index);
            for site in call_def.closure_sites.borrow().as_ref().unwrap() {
                if site.call_to == fn_def.fn_node_ptr { // alright this site calls the fn_def
                    for closure_site_arg in &site.closure_args {
                        if closure_site_arg.closure_def_index == closure_def_index {
                            // alright lets generate this closure
                            ClosureDefGenerator {
                                closure_site_arg: *closure_site_arg,
                                closure_def,
                                fn_def,
                                call_def,
                                shader_registry: self.shader_registry,
                                //env:self.env,
                                const_table_offset,
                                backend_writer,
                                string: self.string,
                            }
                            .generate_fn_def()
                        }
                    }
                }
            }
        }
        // great. we have output all the closures this fn_def needs.
        // however we could have multiple uses in one function
        // we need to create one function per call-site
        for (site_index, closure_site) in call_def.closure_sites.borrow().as_ref().unwrap().iter().enumerate() {
            // for each site
            if closure_site.call_to == fn_def.fn_node_ptr { // alright this site calls the fn_def
                // alright we need a fn def for this site_index
                FnDefWithClosureArgsGenerator {
                    closure_site_info:ClosureSiteInfo{
                        site_index,
                        closure_site,
                        call_ptr:call_def.fn_node_ptr,
                    },
                    fn_def,
                    call_def,
                    shader_registry: self.shader_registry,
                    //env:self.env,
                    const_table_offset,
                    backend_writer,
                    string: self.string,
                }
                .generate_fn_def_with_closure_args()
                
            }
        }
        
    }
    
    pub fn generate_pixel_shader(&mut self) {
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_decls(None, None, packed_varyings_size);
        for field in &self.draw_shader_def.fields {
            match &field.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    self.write_var_decl(
                        false,
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    self.write_var_decl(
                        false,
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Varying {..} => {
                    self.write_var_decl(
                        false,
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                _ => {}
            }
        }
        
        self.generate_shader_body(&self.draw_shader_def.pixel_fns.borrow(), &self.draw_shader_def.pixel_structs.borrow());
        
        writeln!(self.string, "void main() {{").unwrap();
        let mut varying_unpacker = VarUnpacker::new(
            "packed_varying",
            packed_varyings_size,
            &mut self.string,
        );
        for decl in &self.draw_shader_def.fields {
            match &decl.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    varying_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    varying_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                DrawShaderFieldKind::Varying {..} => {
                    varying_unpacker
                        .unpack_var(decl.ident, decl.ty_expr.ty.borrow().as_ref().unwrap());
                }
                _ => {}
            }
        }
        // we need to collect all consts
        let pixel_decl = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(pixel))).unwrap();
        writeln!(self.string, "    gl_FragColor = {}();", DisplayFnName(pixel_decl.fn_node_ptr, pixel_decl.ident)).unwrap();
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_decls(
        &mut self,
        packed_attributes_size: Option<usize>,
        packed_instances_size: Option<usize>,
        packed_varyings_size: usize,
    ) {
        
        if self.final_const_table.table.len()>0 {
            writeln!(
                self.string,
                "uniform float const_table[{}];",
                self.final_const_table.table.len()
            ).unwrap();
        }
        
        for decl in &self.draw_shader_def.fields {
            match decl.kind {
                DrawShaderFieldKind::Uniform {..} => self.generate_uniform_decl(decl),
                _ => {}
            }
        }
        
        for decl in &self.draw_shader_def.fields {
            match decl.kind {
                DrawShaderFieldKind::Texture {..} => self.generate_texture_decl(decl),
                _ => {}
            }
        }
        
        if let Some(packed_attributes_size) = packed_attributes_size {
            self.generate_packed_var_decls(
                "attribute",
                "packed_geometry",
                packed_attributes_size,
            );
        }
        
        if let Some(packed_instances_size) = packed_instances_size {
            self.generate_packed_var_decls(
                "attribute",
                "packed_instance",
                packed_instances_size,
            );
        }
        
        self.generate_packed_var_decls("varying", "packed_varying", packed_varyings_size);
    }
    
    fn generate_struct_decl(&mut self, struct_ptr: StructNodePtr, struct_def: &StructDef) {
        write!(self.string, "struct {} {{", struct_ptr).unwrap();
        if !struct_def.fields.is_empty() {
            writeln!(self.string).unwrap();
            for field in &struct_def.fields {
                write!(self.string, "    ").unwrap();
                self.write_var_decl(
                    false,
                    &field.ident,
                    field.ty_expr.ty.borrow().as_ref().unwrap(),
                );
                writeln!(self.string, ";").unwrap();
            }
        }
        writeln!(self.string, "}};").unwrap();
    }
    
    fn generate_const_def(&mut self, ptr: ConstNodePtr, def: &ConstDef) {
        write!(self.string, "const ").unwrap();
        self.write_var_decl(
            false,
            &ptr,
            def.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, " = ").unwrap();
        self.generate_expr(&def.expr);
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_uniform_decl(&mut self, decl: &DrawShaderFieldDef) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            false,
            &DisplayDsIdent(decl.ident),
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_texture_decl(&mut self, decl: &DrawShaderFieldDef) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            false,
            &DisplayDsIdent(decl.ident),
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn compute_packed_geometries_size(&self) -> usize {
        let mut packed_attributes_size = 0;
        for field in &self.draw_shader_def.fields {
            packed_attributes_size += match field.kind {
                DrawShaderFieldKind::Geometry {..} => field.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_attributes_size
    }
    
    fn compute_packed_instances_size(&self) -> usize {
        let mut packed_instances_size = 0;
        for field in &self.draw_shader_def.fields {
            packed_instances_size += match field.kind {
                DrawShaderFieldKind::Instance {..} => field.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_instances_size
    }
    
    fn compute_packed_varyings_size(&self) -> usize {
        let mut packed_varyings_size = 0;
        for field in &self.draw_shader_def.fields {
            packed_varyings_size += match &field.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    field.ty_expr.ty.borrow().as_ref().unwrap().size()
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    field.ty_expr.ty.borrow().as_ref().unwrap().size()
                }
                DrawShaderFieldKind::Varying {..} => field.ty_expr.ty.borrow().as_ref().unwrap().size(),
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
        let mut cons_name = format!("cons_{}", ty_lit);
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
            self.write_var_decl(false, &id!(x), &param_tys[0])
        } else {
            for (index, param_ty) in param_tys.iter().enumerate() {
                write!(self.string, "{}", sep).unwrap();
                self.write_var_decl(false, &format!("x{}", index), param_ty);
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
    
    fn generate_fn_def(&mut self, fn_def: &FnDef, backend_writer: &dyn BackendWriter, const_table_offset: Option<usize>) {
        FnDefGenerator {
            fn_def,
            shader_registry: self.shader_registry,
            //env:self.env,
            const_table_offset,
            backend_writer,
            string: self.string,
        }
        .generate_fn_def()
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
    
    fn write_var_decl(&mut self, is_inout: bool, ident: &dyn fmt::Display, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, "", is_inout, false, ident, ty);
    }
    
    //fn write_ident(&mut self, ident: Ident) {
    //   self.backend_writer.write_ident(&mut self.string, ident);
    //}
    
    fn write_ty_lit(&mut self, ty_lit: TyLit) {
        self.backend_writer.write_ty_lit(&mut self.string, ty_lit);
    }
    
}

struct FnDefGenerator<'a> {
    fn_def: &'a FnDef,
    shader_registry: &'a ShaderRegistry,
    const_table_offset: Option<usize>,
    string: &'a mut String,
    backend_writer: &'a dyn BackendWriter
}

impl<'a> FnDefGenerator<'a> {
    fn generate_fn_def(&mut self) {
        
        self.backend_writer.write_var_decl(
            &mut self.string,
            "",
            false,
            false,
            &DisplayFnName(self.fn_def.fn_node_ptr, self.fn_def.ident), // here we must expand IdentPath to something
            self.fn_def.return_ty.borrow().as_ref().unwrap()
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &self.fn_def.params {
            if !param.shadow.get().is_none() {
                if self.backend_writer.write_var_decl(
                    &mut self.string,
                    sep,
                    param.is_inout,
                    false,
                    &DisplayVarName(param.ident, param.shadow.get().unwrap()),
                    param.ty_expr.ty.borrow().as_ref().unwrap(),
                ) {
                    sep = ", ";
                }
            }
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&self.fn_def.block);
        writeln!(self.string).unwrap();
        //self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            fn_def: self.fn_def,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
}


struct FnDefWithClosureArgsGenerator<'a> {
    closure_site_info: ClosureSiteInfo<'a>,
    fn_def: &'a FnDef,
    call_def: &'a FnDef,
    shader_registry: &'a ShaderRegistry,
    const_table_offset: Option<usize>,
    string: &'a mut String,
    backend_writer: &'a dyn BackendWriter
}

impl<'a> FnDefWithClosureArgsGenerator<'a> {
    fn generate_fn_def_with_closure_args(&mut self) {
        
        self.backend_writer.write_var_decl(
            &mut self.string,
            "",
            false,
            false,
            &DisplayFnNameWithClosureArgs(
                self.closure_site_info.site_index,
                self.call_def.fn_node_ptr,
                self.fn_def.ident
            ), // here we must expand IdentPath to something
            self.fn_def.return_ty.borrow().as_ref().unwrap()
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &self.fn_def.params {
            if !param.shadow.get().is_none() {
                if self.backend_writer.write_var_decl(
                    &mut self.string,
                    sep,
                    param.is_inout,
                    false,
                    &DisplayVarName(param.ident, param.shadow.get().unwrap()),
                    param.ty_expr.ty.borrow().as_ref().unwrap(),
                ) {
                    sep = ", ";
                }
            }
        }
        // now we iterate over the closures in our site,
        // and we need to merge the set of closed over args.
        for sym in &self.closure_site_info.closure_site.all_closed_over {
            if self.backend_writer.write_var_decl(
                &mut self.string,
                sep,
                false,
                false,
                &DisplayClosedOverArg(sym.ident, sym.shadow),
                &sym.ty,
            ) {
                sep = ", ";
            }
        }
        
        write!(self.string, ") ").unwrap();
        // alright so here the block is generated.. however
        // we need to know the names and the closed-over-args passthrough
        self.generate_block(&self.fn_def.block);
        
        
        writeln!(self.string).unwrap();
        //self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: Some(self.closure_site_info.clone()),
            //env: self.env,
            fn_def: self.fn_def,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
}

struct ClosureDefGenerator<'a> {
    closure_def: &'a ClosureDef,
    closure_site_arg: ClosureSiteArg,
    fn_def: &'a FnDef,
    call_def: &'a FnDef,
    shader_registry: &'a ShaderRegistry,
    const_table_offset: Option<usize>,
    string: &'a mut String,
    backend_writer: &'a dyn BackendWriter
}

impl<'a> ClosureDefGenerator<'a> {
    fn generate_fn_def(&mut self) {
        
        let fn_param = &self.fn_def.params[self.closure_site_arg.param_index];
        
        let mut sep = "";
        
        if let TyExprKind::ClosureDecl {params, return_ty, ..} = &fn_param.ty_expr.kind {
            
            self.backend_writer.write_var_decl(
                &mut self.string,
                "",
                false,
                false,
                &DisplayClosureName(self.call_def.fn_node_ptr, self.closure_site_arg.closure_def_index), // here we must expand IdentPath to something
                return_ty.borrow().as_ref().unwrap(),
            );
            write!(self.string, "(").unwrap();   
            
            // ok we have now params and names
            for (param_index, param) in params.iter().enumerate() {
                // lets fetch the name of this thing
                let closure_param = &self.closure_def.params[param_index];
                let shadow = closure_param.shadow.get().unwrap();
                if self.backend_writer.write_var_decl(
                    &mut self.string,
                    sep,
                    param.is_inout,
                    false,
                    &DisplayVarName(closure_param.ident, shadow),
                    param.ty_expr.ty.borrow().as_ref().unwrap(),
                ) {
                    sep = ", ";
                }
            }
        }
        else {
            panic!()
        }
        
        for sym in self.closure_def.closed_over_syms.borrow().as_ref().unwrap() {
            if self.backend_writer.write_var_decl(
                &mut self.string,
                sep,
                false,
                false,
                &DisplayVarName(sym.ident, sym.shadow),
                &sym.ty,
            ) {
                sep = ", ";
            }
        }
        writeln!(self.string, ") {{").unwrap();
        
        match &self.closure_def.kind {
            ClosureDefKind::Expr(expr) => {
                write!(self.string, "    return ").unwrap();
                self.generate_expr(expr);
                writeln!(self.string, ";").unwrap();
                writeln!(self.string, "}}").unwrap();
            }
            ClosureDefKind::Block(block) => {
                self.generate_block(block);
                writeln!(self.string).unwrap();
            }
        }
        //self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            fn_def: self.fn_def,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
    
    
    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            fn_def: Some(self.fn_def),
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            string: self.string,
        }
        .generate_expr(expr)
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
            write!(self.string, "    {}", &DisplayDsIdent(ident)).unwrap();
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

struct GlslBackendWriter<'a> {
    pub shader_registry: &'a ShaderRegistry
}

impl<'a> BackendWriter for GlslBackendWriter<'a> {
    
    fn needs_bare_struct_cons(&self) -> bool {
        true
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
        sep: &'static str,
        is_inout: bool,
        is_packed: bool,
        ident: &dyn fmt::Display,
        ty: &Ty,
    ) -> bool {
        match *ty {
            Ty::Void => {
                write!(string, "{}void {}", sep, ident).unwrap();
            }
            Ty::Bool => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Bool);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Int => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Int);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Float => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Float);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Bvec2 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Bvec2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Bvec3 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Bvec3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Bvec4 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Bvec4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Ivec2 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Ivec2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Ivec3 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Ivec3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Ivec4 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Ivec4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Vec2 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Vec2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Vec3 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Vec3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Vec4 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Vec4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Mat2 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Mat2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Mat3 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Mat3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Mat4 => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Mat4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Texture2D => {
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Texture2D);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Array {ref elem_ty, len} => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{}", sep).unwrap();
                self.write_var_decl(string, "", is_inout, is_packed, ident, elem_ty);
                write!(string, "[{}]", len).unwrap();
            }
            Ty::DrawShader(_) => { 
                // we should output nothing
                return false
            }
            Ty::ClosureDef {..} => {
                return false
            }
            Ty::ClosureDecl => {
                return false
            }
            Ty::Struct(ptr) => {
                if is_inout {
                    write!(string, "inout ").unwrap();
                }
                write!(string, "{} {} {}", sep, ptr, ident).unwrap();
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
    
    fn write_builtin_call_ident(&self, string: &mut String, ident: Ident, _arg_exprs: &[Expr]) {
        write!(string, "{}", ident).unwrap();
    }
    /*
    fn write_call_ident(&self, string: &mut String, ident: Ident, _arg_exprs: &[Expr]) {
        self.write_ident(string, ident);
    }
    */
    /*
    fn write_ident(&self, string: &mut String, ident: Ident) {
        ident.0.as_string( | ident_string | {
            let ident_string = ident_string.unwrap();
            write!(
                string,
                "{}",
                match ident_string.as_ref() {
                    "union" => "mpsc_union",
                    "texture" => "mpsc_texture",
                    _ => ident_string,
                }
            )
                .unwrap()
        })
    }*/
}
