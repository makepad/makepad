use{
    std::{
        fmt,
        fmt::Write,
        collections::BTreeSet,
    },
    crate::{
        makepad_live_compiler::{
            id,
            LiveId,
        },
        generate::*,
        swizzle::Swizzle,
        shader_ast::*,
        shader_registry::ShaderRegistry
    }
};

pub fn generate_vertex_shader(draw_shader_def: &DrawShaderDef, const_table:&DrawShaderConstTable, shader_registry: &ShaderRegistry) -> String {
    let mut string = String::new();
    DrawShaderGenerator {
        draw_shader_def,
        const_table,
        shader_registry,
        string: &mut string,
        backend_writer: &GlslBackendWriter {shader_registry, const_table}
    }
    .generate_vertex_shader();
    string
}

pub fn generate_pixel_shader(draw_shader_def: &DrawShaderDef, const_table:&DrawShaderConstTable, shader_registry: &ShaderRegistry) -> String {
    let mut string = String::new();
    DrawShaderGenerator {
        draw_shader_def,
        const_table,
        shader_registry,
        string: &mut string,
        backend_writer: &GlslBackendWriter {shader_registry, const_table}
    }
    .generate_pixel_shader();
    string
}

struct DrawShaderGenerator<'a> {
    draw_shader_def: &'a DrawShaderDef,
    shader_registry: &'a ShaderRegistry,
    string: &'a mut String,
    const_table: &'a DrawShaderConstTable,
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
        let packed_geometries_slots = self.compute_packed_geometries_slots();
        let packed_instances_slots = self.compute_packed_instances_slots();
        let packed_varyings_slots = self.compute_packed_varyings_slots();
        self.generate_decls(
            Some(packed_geometries_slots),
            Some(packed_instances_slots),
            packed_varyings_slots,
        );
        for field in &self.draw_shader_def.fields {
            match field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Instance {..} => {
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Varying {..} => {
                    self.write_var_decl(
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
            packed_geometries_slots,
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
            packed_instances_slots,
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
        
        let vertex_def = self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap();
        
        writeln!(self.string, "    gl_Position = {}();", DisplayFnName(vertex_def.fn_ptr, vertex_def.ident)).unwrap();
        let mut varying_packer = VarPacker::new(
            "packed_varying",
            packed_varyings_slots,
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
    
    pub fn generate_shader_body(&mut self, fn_deps: &Vec<FnPtr>, struct_deps: &Vec<StructPtr>) {
        
        // alright so. we have our fn deps which have struct deps
        // and we have struct deps in our struct deps.
        let mut all_constructor_fns = BTreeSet::new();
        
        for callee in fn_deps.iter().rev() {
            let decl = self.shader_registry.all_fns.get(callee).unwrap();
            all_constructor_fns.extend(decl.constructor_fn_deps.borrow().as_ref().unwrap().iter().cloned());
        }
        
        // all our live ref uniforms
        for (live_ref, ty) in self.draw_shader_def.all_live_refs.borrow().iter() {
            self.generate_live_decl(*live_ref, ty);
        }
        
        // we have all the structs already from analyse
        for struct_ptr in struct_deps.iter().rev() {
            let struct_def = self.shader_registry.structs.get(struct_ptr).unwrap();
            self.generate_struct_def(*struct_ptr, struct_def);
        }
        
        for (ty_lit, param_tys) in all_constructor_fns
        {
            generate_cons_fn(self.backend_writer, self.string, ty_lit, &param_tys);
        }
        
        for fn_iter in fn_deps.iter().rev() {
            let const_table_offset = self.const_table.offsets.get(fn_iter).cloned();
            let fn_def = self.shader_registry.all_fns.get(fn_iter).unwrap();
            if fn_def.has_closure_args() {
                for call_iter in fn_deps.iter().rev() {
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
    }
    
    pub fn generate_pixel_shader(&mut self) {
        let packed_varyings_slots = self.compute_packed_varyings_slots();
        self.generate_decls(None, None, packed_varyings_slots);
        for field in &self.draw_shader_def.fields {
            match &field.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    self.write_var_decl(
                        &DisplayDsIdent(field.ident),
                        field.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                    write!(self.string, "=").unwrap();
                    self.write_ty_init(field.ty_expr.ty.borrow().as_ref().unwrap());
                    writeln!(self.string, ";").unwrap();
                }
                DrawShaderFieldKind::Varying {..} => {
                    self.write_var_decl(
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
            packed_varyings_slots,
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
        writeln!(self.string, "    gl_FragColor = {}();", DisplayFnName(pixel_decl.fn_ptr, pixel_decl.ident)).unwrap();
        writeln!(self.string, "}}").unwrap();
    }
    
    fn generate_decls(
        &mut self,
        packed_attributes_size: Option<usize>,
        packed_instances_size: Option<usize>,
        packed_varyings_size: usize,
    ) {
        
        if self.const_table.table.len()>0 {
            writeln!(
                self.string,
                "uniform float const_table[{}];",
                self.const_table.table.len()
            ).unwrap();
        }
        
        for (ident, vec) in self.draw_shader_def.fields_as_uniform_blocks() {
            writeln!(self.string, "// Uniform block {}", ident).unwrap();
            for (index, _item) in vec {
                let field = &self.draw_shader_def.fields[index];
                if let DrawShaderFieldKind::Uniform {..} = &field.kind {
                    self.generate_uniform_decl(field);
                }
                else {
                    panic!()
                }
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
    
    fn generate_struct_def(&mut self, struct_ptr: StructPtr, struct_def: &StructDef) {
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

    fn generate_uniform_decl(&mut self, decl: &DrawShaderFieldDef) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            &DisplayDsIdent(decl.ident),
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_live_decl(&mut self, ptr: ValuePtr, ty: &Ty) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            &ptr,
            ty,
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_texture_decl(&mut self, decl: &DrawShaderFieldDef) {
        write!(self.string, "uniform ").unwrap();
        self.write_var_decl(
            &DisplayDsIdent(decl.ident),
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }
    
    fn compute_packed_geometries_slots(&self) -> usize {
        let mut packed_attributes_size = 0;
        for field in &self.draw_shader_def.fields {
            packed_attributes_size += match field.kind {
                DrawShaderFieldKind::Geometry {..} => field.ty_expr.ty.borrow().as_ref().unwrap().slots(),
                _ => 0,
            }
        }
        packed_attributes_size
    }
    
    fn compute_packed_instances_slots(&self) -> usize {
        let mut packed_instances_size = 0;
        for field in &self.draw_shader_def.fields {
            packed_instances_size += match field.kind {
                DrawShaderFieldKind::Instance {..} => field.ty_expr.ty.borrow().as_ref().unwrap().slots(),
                _ => 0,
            }
        }
        packed_instances_size
    }
    
    fn compute_packed_varyings_slots(&self) -> usize {
        let mut packed_varyings_size = 0;
        for field in &self.draw_shader_def.fields {
            packed_varyings_size += match &field.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    field.ty_expr.ty.borrow().as_ref().unwrap().slots()
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} if is_used_in_pixel_shader.get() => {
                    field.ty_expr.ty.borrow().as_ref().unwrap().slots()
                }
                DrawShaderFieldKind::Varying {..} => field.ty_expr.ty.borrow().as_ref().unwrap().slots(),
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
    
    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            fn_def: None,
            shader_registry: self.shader_registry,
            closure_site_info: None,
            const_table_offset: None,
            backend_writer: self.backend_writer,
            string: self.string,
        }
        .generate_expr(expr)
    }
    
    fn write_var_decl(&mut self, ident: &dyn fmt::Display, ty: &Ty) {
        self.backend_writer.write_var_decl(&mut self.string, "", false, false, ident, ty);
    }

    fn write_ty_lit(&mut self, ty_lit: TyLit){
        self.backend_writer.write_ty_lit(&mut self.string, ty_lit);
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
        let var_slots = ty.slots();
        let mut var_offset = 0;
        let mut in_matrix = None;
        while var_offset < var_slots {
            let count = var_slots - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = if var_slots > 4 {1} else {count.min(packed_count)};
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
            if var_slots > 1 {
                if var_slots <= 4 {
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
        let var_slots = ty.slots();
        let mut var_offset = 0;
        let mut in_matrix = None;
        while var_offset < var_slots {
            let count = var_slots - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = if var_slots > 4 {1} else {count.min(packed_count)};
            write!(self.string, "    {}", &DisplayDsIdent(ident)).unwrap();
            if var_slots > 1 {
                if var_slots <= 4 { // its a matrix
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
    pub shader_registry: &'a ShaderRegistry,
    const_table: &'a DrawShaderConstTable
}

impl<'a> BackendWriter for GlslBackendWriter<'a> {

    fn needs_cstyle_struct_cons(&self) -> bool {
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
        fn prefix(string: &mut String, sep: &'static str, is_inout: bool) {
            write!(string, "{}", sep).unwrap();
            if is_inout {
                write!(string, "inout ").unwrap();
            }
        }
        match *ty {
            Ty::Void => {
                write!(string, "{}void {}", sep, ident).unwrap();
            }
            Ty::Bool => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Bool);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Int => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Int);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Float => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Float);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Bvec2 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Bvec2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Bvec3 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Bvec3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Bvec4 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Bvec4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Ivec2 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Ivec2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Ivec3 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Ivec3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Ivec4 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Ivec4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Vec2 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Vec2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Vec3 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Vec3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Vec4 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Vec4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Mat2 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Mat2);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Mat3 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Mat3);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Mat4 => {
                prefix(string, sep, is_inout);
                self.write_ty_lit(string, TyLit::Mat4);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Texture2D => {
                write!(string, "{}", sep).unwrap();
                self.write_ty_lit(string, TyLit::Texture2D);
                write!(string, " {}", ident).unwrap();
            }
            Ty::Array {ref elem_ty, len} => {
                self.write_var_decl(string, sep, is_inout, is_packed, ident, elem_ty);
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
                prefix(string, sep, is_inout);
                write!(string, "{} {}", ptr, ident).unwrap();
            }
            Ty::Enum(_) => {
                prefix(string, sep, is_inout);
                write!(string, "int {}", ident).unwrap();
            }
        }
        true
    }
    
    fn write_call_expr_hidden_args(&self, _string: &mut String, _hidden_args:&BTreeSet<HiddenArgKind >, _sep: &str){
    }
    
    fn write_fn_def_hidden_params(&self, _string: &mut String, _hidden_args:&BTreeSet<HiddenArgKind >, _sep: &str){
    }

    fn generate_live_value_prefix(&self, _string: &mut String) {
    }

    fn generate_draw_shader_field_expr(&self, string: &mut String, field_ident: Ident, _ty:&Ty) {
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
    
}
