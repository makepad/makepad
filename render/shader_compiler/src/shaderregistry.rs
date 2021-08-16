#![allow(unused_variables)]

use makepad_live_parser::*;
use crate::shaderast::*;
use crate::analyse::*;

use crate::shaderparser::ShaderParser;
use crate::shaderparser::ShaderParserDep;
use std::collections::BTreeMap;
use std::fmt;
use std::cell::{RefCell, Cell};
use std::collections::HashMap;
use crate::builtin::Builtin;
use crate::builtin::generate_builtins;
use crate::shaderast::Scopes;
use crate::generate_glsl;
use crate::generate_metal;

#[derive(Clone, Debug, Copy, Hash, Eq, PartialEq)]
pub struct ShaderResourceId(CrateModule, Id);

impl fmt::Display for ShaderResourceId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}::{}", self.0, self.1)
    }
}

#[derive(Debug)]
pub struct ShaderRegistry {
    pub live_registry: LiveRegistry,
    pub consts: HashMap<ConstNodePtr, ConstDef>,
    
    pub all_fns: HashMap<FnNodePtr, FnDef>,
    
    //pub plain_fns: HashMap<FnNodePtr, FnDecl>,
    pub draw_shaders: HashMap<DrawShaderNodePtr, DrawShaderDef>,
    pub structs: HashMap<StructNodePtr, StructDef>,
    
    pub draw_inputs: HashMap<ShaderResourceId, DrawShaderInput>,
    pub builtins: HashMap<Ident, Builtin>,
}

impl ShaderRegistry {
    pub fn new() -> Self {
        id_check!(default);
        id_check!(x);
        Self {
            live_registry: LiveRegistry::default(),
            structs: HashMap::new(),
            consts: HashMap::new(),
            draw_shaders: HashMap::new(),
            all_fns: HashMap::new(),
            //plain_fns: HashMap::new(),
            draw_inputs: HashMap::new(),
            builtins: generate_builtins()
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DrawShaderInput {
    pub uniforms: Vec<DrawShaderInputItem>,
    pub instances: Vec<DrawShaderInputItem>,
    pub textures: Vec<DrawShaderInputItem>,
}

#[derive(Clone, Debug)]
pub struct DrawShaderInputItem {
    pub ident: Ident,
    pub ty_expr: TyExpr
}

impl DrawShaderInput {
    pub fn add_uniform(&mut self, name: &str, ty_expr: TyExpr) {
        Id::from_str(name).panic_collision(name);
        if let TyExprKind::Lit {ty_lit, ..} = ty_expr.kind {
            if ty_lit == TyLit::Texture2D {
                self.textures.push(DrawShaderInputItem {ident: Ident(Id::from_str(name)), ty_expr});
                return
            }
        }
        self.uniforms.push(DrawShaderInputItem {ident: Ident(Id::from_str(name)), ty_expr});
    }
    
    pub fn add_instance(&mut self, name: &str, ty_expr: TyExpr) {
        Id::from_str(name).panic_collision(name);
        self.instances.push(DrawShaderInputItem {ident: Ident(Id::from_str(name)), ty_expr});
    }
}

pub enum LiveNodeFindResult {
    NotFound,
    Component(FullNodePtr),
    Struct(StructNodePtr),
    Function(FnNodePtr),
    PossibleStatic(StructNodePtr, FnNodePtr),
    Const(ConstNodePtr),
    LiveValue(ValueNodePtr, TyLit)
}

#[derive(Clone, Default)]
pub struct FinalConstTable {
    pub table: Vec<f32>,
    pub offsets: BTreeMap<FnNodePtr, usize>
}

impl ShaderRegistry {
    
    pub fn compute_final_const_table(&self, draw_shader_def: &DrawShaderDef, filter_file_id: Option<FileId>) -> FinalConstTable {
        if let Some(filter_file_id) = filter_file_id {
            let mut offsets = BTreeMap::new();
            let mut table = Vec::new();
            let mut offset = 0;
            for callee in draw_shader_def.all_fns.borrow().iter() {
                let fn_decl = self.all_fns.get(callee).unwrap();
                if fn_decl.span.file_id() == filter_file_id {
                    let sub_table = fn_decl.const_table.borrow();
                    table.extend(sub_table.as_ref().unwrap().iter());
                    offsets.insert(*callee, offset);
                    offset += sub_table.as_ref().unwrap().len();
                }
            }
            FinalConstTable {table, offsets}
        }
        else {
            FinalConstTable::default()
        }
    }
    
    pub fn fn_ident_from_ptr(&self, fn_node_ptr: FnNodePtr) -> Ident {
        let (_, node) = self.live_registry.resolve_ptr(fn_node_ptr.0);
        Ident(node.id_pack.unwrap_single())
    }
    
    pub fn draw_shader_method_ptr_from_ident(&self, draw_shader_def:&DrawShaderDef, ident: Ident) -> Option<FnNodePtr> {
        for fn_node_ptr in &draw_shader_def.methods{
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident{
                return Some(*fn_node_ptr);
            }
        }
        None
    }
    
     pub fn struct_method_ptr_from_ident(&self, struct_def:&StructDef, ident: Ident) -> Option<FnNodePtr> {
        for fn_node_ptr in &struct_def.methods{
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident{
                return Some(*fn_node_ptr);
            }
        }
        None
    }
    
    pub fn draw_shader_method_decl_from_ident(&self, draw_shader_def:&DrawShaderDef, ident: Ident) -> Option<&FnDef> {
        for fn_node_ptr in &draw_shader_def.methods{
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident{
                return Some(fn_decl)
            }
        }
        None
    }
    
     pub fn struct_method_decl_from_ident(&self, struct_def:&StructDef, ident: Ident) -> Option<&FnDef> {
        for fn_node_ptr in &struct_def.methods{
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident{
                return Some(fn_decl)
            }
        }
        None
    }
    
    
    pub fn find_live_node_by_path(&self, base_ptr: FullNodePtr, ids: &[Id]) -> LiveNodeFindResult {
        // what are the types of things we can find.
        
        fn no_ids(ids: &[Id], result: LiveNodeFindResult) -> LiveNodeFindResult {
            if ids.len() == 0 {result} else {LiveNodeFindResult::NotFound}
        }
        
        fn var_def_is_const(doc: &LiveDocument, token_start: u32, base_ptr: FullNodePtr) -> LiveNodeFindResult {
            if Token::Ident(id!(const)) == doc.tokens[token_start as usize].token {
                LiveNodeFindResult::Const(ConstNodePtr(base_ptr))
            }
            else {
                LiveNodeFindResult::NotFound
            }
        }
        
        let (doc, node) = self.live_registry.resolve_ptr(base_ptr);
        match node.value {
            LiveValue::Bool(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValueNodePtr(base_ptr), TyLit::Bool)),
            LiveValue::Int(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValueNodePtr(base_ptr), TyLit::Int)),
            LiveValue::Float(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValueNodePtr(base_ptr), TyLit::Float)),
            LiveValue::Color(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValueNodePtr(base_ptr), TyLit::Vec4)),
            LiveValue::Vec2(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValueNodePtr(base_ptr), TyLit::Vec2)),
            LiveValue::Vec3(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValueNodePtr(base_ptr), TyLit::Vec3)),
            LiveValue::Fn {..} => return no_ids(ids, LiveNodeFindResult::Function(FnNodePtr(base_ptr))),
            LiveValue::VarDef {token_start, ..} => return no_ids(ids, var_def_is_const(doc, token_start, base_ptr)),
            LiveValue::Class {class, node_start: ns, node_count: nc, ..} => {
                if ids.len() == 0 { // check if we are struct or component
                    match self.live_registry.find_base_class_id(class) {
                        Some(id_pack!(Struct)) => return LiveNodeFindResult::Struct(StructNodePtr(base_ptr)),
                        _ => return LiveNodeFindResult::Component(base_ptr),
                    }
                }
                // now we bounce around the DOM tree.
                let mut parent_ptr = base_ptr.local_ptr;
                let mut node_start = ns as usize;
                let mut node_count = nc as usize;
                let mut level = base_ptr.local_ptr.level + 1;
                for i in 0..ids.len() {
                    let id = ids[i];
                    let mut found = false;
                    for j in 0..node_count {
                        let node = &doc.nodes[level][j + node_start];
                        if node.id_pack == IdPack::single(id) {
                            // we found the node.
                            let node_ptr = LocalNodePtr {level: level, index: j + node_start};
                            if i == ids.len() - 1 { // last item
                                let full_node_ptr = FullNodePtr {file_id: base_ptr.file_id, local_ptr: node_ptr};
                                match node.value {
                                    LiveValue::Class {class, ..} => {
                                        match self.live_registry.find_base_class_id(class) {
                                            Some(id_pack!(Struct)) => return LiveNodeFindResult::Struct(StructNodePtr(full_node_ptr)),
                                            Some(id_pack!(Component)) => return LiveNodeFindResult::Component(full_node_ptr),
                                            _ => return LiveNodeFindResult::NotFound
                                        }
                                    },
                                    LiveValue::Fn {..} => { // check if its a method or a free roaming function
                                        let full_base_ptr = FullNodePtr {file_id: base_ptr.file_id, local_ptr: parent_ptr};
                                        let base_node = doc.resolve_ptr(parent_ptr);
                                        if let LiveValue::Class {class, ..} = base_node.value {
                                            // lets check if our base is a component or a struct
                                            match self.live_registry.find_base_class_id(class) {
                                                Some(id_pack!(Struct)) => return LiveNodeFindResult::PossibleStatic(
                                                    StructNodePtr(full_base_ptr),
                                                    FnNodePtr(full_node_ptr)
                                                ),
                                                _ => return LiveNodeFindResult::Function(FnNodePtr(full_node_ptr)),
                                            }
                                        }
                                        else {
                                            panic!()
                                        }
                                    },
                                    LiveValue::Bool(_) => return LiveNodeFindResult::LiveValue(ValueNodePtr(full_node_ptr), TyLit::Bool),
                                    LiveValue::Int(_) => return LiveNodeFindResult::LiveValue(ValueNodePtr(full_node_ptr), TyLit::Int),
                                    LiveValue::Float(_) => return LiveNodeFindResult::LiveValue(ValueNodePtr(full_node_ptr), TyLit::Float),
                                    LiveValue::Color(_) => return LiveNodeFindResult::LiveValue(ValueNodePtr(full_node_ptr), TyLit::Vec4),
                                    LiveValue::Vec2(_) => return LiveNodeFindResult::LiveValue(ValueNodePtr(full_node_ptr), TyLit::Vec2),
                                    LiveValue::Vec3(_) => return LiveNodeFindResult::LiveValue(ValueNodePtr(full_node_ptr), TyLit::Vec3),
                                    LiveValue::VarDef {token_start, ..} => return var_def_is_const(doc, token_start, full_node_ptr),
                                    _ => return LiveNodeFindResult::NotFound
                                }
                            }
                            else { // we need to be either an object or a class
                                parent_ptr = node_ptr;
                                level += 1;
                                match node.value {
                                    LiveValue::Class {node_start: ns, node_count: nc, ..} => {
                                        node_start = ns as usize;
                                        node_count = nc as usize;
                                    },
                                    _ => return LiveNodeFindResult::NotFound
                                }
                                found = true;
                                break
                            }
                        }
                    }
                    if !found {
                        break
                    }
                }
            }
            _ => ()
        }
        LiveNodeFindResult::NotFound
    }
    
    pub fn register_draw_input(&mut self, mod_path: &str, name: &str, draw_input: DrawShaderInput) {
        let parts = mod_path.split("::").collect::<Vec<_ >> ();
        if parts.len()>2 {
            panic!("submodules not supported");
        }
        let crate_module = CrateModule(
            Id::from_str(parts[0]),
            Id::from_str(if parts.len()>1 {parts[1]}else {"main"})
        );
        let name_id = Id::from_str(name);
        self.draw_inputs.insert(ShaderResourceId(crate_module, name_id), draw_input);
    }
    
    pub fn parse_shader_resource_id_from_multi_id(crate_id: Id, module_id: Id, span: Span, target: IdPack, multi_ids: &[Id]) -> Result<ShaderResourceId, LiveError> {
        match target.unpack() {
            IdUnpack::Multi {index, count} => {
                if count == 2 {
                    let part1 = multi_ids[index + 0];
                    let part2 = multi_ids[index + 1];
                    if part1 != id!(self) {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: span,
                            message: format!("Unsupported target naming {}", IdFmt::col(multi_ids, target))
                        });
                    }
                    // ok so we have to find crate_id, module_id, part2
                    return Ok(ShaderResourceId(CrateModule(crate_id, module_id), part2))
                }
                if count == 3 {
                    let part1 = multi_ids[index + 0];
                    let part2 = multi_ids[index + 1];
                    let part3 = multi_ids[index + 1];
                    return Ok(ShaderResourceId(CrateModule(if part1 == id!(crate) {crate_id}else {part1}, part2), part3));
                }
            }
            _ => ()
        }
        return Err(LiveError {
            origin: live_error_origin!(),
            span: span,
            message: format!("Unsupported target naming {}", IdFmt::col(multi_ids, target))
        });
    }
    
    pub fn analyse_deps(&mut self, deps: &[ShaderParserDep]) -> Result<(), LiveError> {
        // recur on used types
        for dep in deps {
            match dep {
                ShaderParserDep::Const(dep) => {
                    self.analyse_const(*dep) ?;
                },
                ShaderParserDep::Struct(dep) => {
                    self.analyse_struct(*dep) ?;
                },
                ShaderParserDep::Function(struct_ptr, fn_ptr) => {
                    self.analyse_plain_fn(*struct_ptr, *fn_ptr) ?
                }
            }
        }
        Ok(())
    }
    
    // lets compile the thing
    pub fn analyse_const(&mut self, const_ptr: ConstNodePtr) -> Result<(), LiveError> {
        if self.consts.get(&const_ptr).is_some() {
            return Ok(());
        }
        let (doc, const_node) = self.live_registry.resolve_ptr(const_ptr.0);
        match const_node.value {
            LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                let mut parser_deps = Vec::new();
                let id = const_node.id_pack.unwrap_single();
                let mut parser = ShaderParser::new(
                    self,
                    doc.get_tokens(token_start, token_count + 1),
                    doc.get_scopes(scope_start, scope_count),
                    &mut parser_deps,
                    None,
                    //Some(struct_full_ptr)
                );
                
                let const_decl = parser.expect_const_def(Ident(id)) ?;
                self.consts.insert(const_ptr, const_decl);
                
                self.analyse_deps(&parser_deps) ?;
                
                let mut ca = ConstAnalyser {
                    const_def: self.consts.get(&const_ptr).unwrap(),
                    scopes: &mut Scopes::new(),
                    shader_registry: self,
                    options: ShaderAnalyseOptions {
                        no_const_collapse: true
                    },
                };
                ca.analyse_const_decl() ?;
            }
            _ => panic!()
        }
        return Ok(())
    }
    
    // lets compile the thing
    pub fn analyse_plain_fn(&mut self, struct_ptr: Option<StructNodePtr>, fn_ptr: FnNodePtr) -> Result<(), LiveError> {
        
        if self.all_fns.get(&fn_ptr).is_some() {
            return Ok(());
        }
        // alright lets parse and analyse a plain fn
        let (doc, fn_node) = self.live_registry.resolve_ptr(fn_ptr.0);
        match fn_node.value {
            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                let id = fn_node.id_pack.unwrap_single();
                let mut parser_deps = Vec::new();
                // lets parse this thing
                let parser = ShaderParser::new(
                    self,
                    doc.get_tokens(token_start, token_count + 1),
                    doc.get_scopes(scope_start, scope_count),
                    &mut parser_deps,
                    if let Some(struct_ptr) = struct_ptr {Some(FnSelfKind::Struct(struct_ptr))}else {None},
                    //Some(struct_full_ptr)
                );
                
                let fn_def = parser.expect_plain_fn_def(
                    fn_ptr,
                    Ident(id),
                ) ?;
                self.all_fns.insert(fn_ptr, fn_def);
                
                self.analyse_deps(&parser_deps) ?;
                
                // ok analyse the struct methods now.
                let mut fa = FnDefAnalyser {
                    closure_return_ty: None,
                    fn_def: self.all_fns.get(&fn_ptr).unwrap(),
                    scopes: &mut Scopes::new(),
                    shader_registry: self,
                    is_inside_loop: false,
                    options: ShaderAnalyseOptions {
                        no_const_collapse: true
                    }
                };
                fa.analyse_fn_decl() ?;
                fa.analyse_fn_def() ?;
                
                Ok(())
            }
            _ => panic!()
        }
    }
    
    
    // lets compile the thing
    pub fn analyse_struct(&mut self, struct_ptr: StructNodePtr) -> Result<(), LiveError> {
        
        if self.structs.get(&struct_ptr).is_some() {
            return Ok(());
        }
        
        let (doc, class_node) = self.live_registry.resolve_ptr(struct_ptr.0);
        //let doc = &self.live_registry.expanded[full_ptr.file_id.to_index()];
        //let class_node = &doc.nodes[full_ptr.local_ptr.level][full_ptr.local_ptr.index];
        
        match class_node.value {
            LiveValue::Class {node_start, node_count, class} => {
                let mut struct_def = StructDef {
                    span: self.live_registry.token_id_to_span(class_node.token_id),
                    // ident: Ident(class_node.id_pack.unwrap_single()),
                    struct_refs: RefCell::new(None),
                    fields: Vec::new(),
                    methods: Vec::new()
                    //    struct_body: ShaderBody::default()
                };
                
                let mut parser_deps = Vec::new();
                for i in 0..node_count as usize {
                    let prop_ptr = FullNodePtr {file_id: struct_ptr.0.file_id, local_ptr: LocalNodePtr {
                        level: struct_ptr.0.local_ptr.level + 1,
                        index: i + node_start as usize
                    }};
                    let prop = doc.resolve_ptr(prop_ptr.local_ptr);
                    match prop.value {
                        LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id_pack.unwrap_single();
                            let mut parser = ShaderParser::new(
                                self,
                                doc.get_tokens(token_start, token_count + 1),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr))
                                //Some(struct_full_ptr)
                            );
                            // we only allow a field def
                            let def = parser.expect_field(Ident(id), VarDefNodePtr(prop_ptr)) ?;
                            if let Some(def) = def{
                                struct_def.fields.push(def);
                            }
                        },
                        LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id_pack.unwrap_single();
                            // lets parse this thing
                            let parser = ShaderParser::new(
                                self,
                                doc.get_tokens(token_start, token_count + 1),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr))
                                //Some(struct_full_ptr)
                            );
                            
                            let fn_def = parser.expect_method_def(
                                FnNodePtr(prop_ptr),
                                Ident(id),
                            ) ?;
                            // if we get false, this was not a method but could be static.
                            // statics need a pointer to their struct to resolve Self
                            // so we can't treat them purely as loose methods
                            if let Some(fn_def) = fn_def {
                                struct_def.methods.push(fn_def.fn_node_ptr);
                                self.all_fns.insert(fn_def.fn_node_ptr, fn_def);
                            }
                        }
                        _ => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: self.live_registry.token_id_to_span(prop.token_id),
                                message: format!("Cannot use {:?} in struct", prop.value)
                            })
                        }
                    }
                }
                // we should store the structs
                self.structs.insert(struct_ptr, struct_def);
                
                self.analyse_deps(&parser_deps) ?;
                
                // ok analyse the struct methods now.
                let mut sa = StructAnalyser {
                    struct_def: self.structs.get(&struct_ptr).unwrap(),
                    scopes: &mut Scopes::new(),
                    shader_registry: self,
                    options: ShaderAnalyseOptions {
                        no_const_collapse: true
                    }
                };
                sa.analyse_struct() ?;
                //println!("STRUCT");
                
            }
            _ => ()
        }
        
        Ok(())
    }
    
    // lets compile the thing
    pub fn analyse_draw_shader(&mut self, crate_id: Id, module_id: Id, ids: &[Id]) -> Result<(), LiveError> {
        // lets find the FullPointer
        
        if let Some(shader_ptr) = self.live_registry.find_full_node_ptr_from_ids(crate_id, module_id, ids) {
            let shader_ptr = DrawShaderNodePtr(shader_ptr);
            let mut draw_shader_def = DrawShaderDef::default();
            // we have a pointer to the thing to instance.
            let (doc, class_node) = self.live_registry.resolve_ptr(shader_ptr.0);
            
            match class_node.value {
                LiveValue::Class {node_start, node_count, ..} => {
                    let mut parser_deps = Vec::new();
                    let mut draw_input_srid = None;
                    for i in 0..node_count as usize {
                        let prop_ptr = FullNodePtr {file_id: shader_ptr.0.file_id, local_ptr: LocalNodePtr {
                            level: shader_ptr.0.local_ptr.level + 1,
                            index: i + node_start as usize
                        }};
                        let prop = doc.resolve_ptr(prop_ptr.local_ptr);
                        match prop.value {
                            LiveValue::ResourceRef {target} => {
                                // draw input or default_geometry
                                match prop.id_pack {
                                    id_pack!(draw_input) => {
                                        let srid = Self::parse_shader_resource_id_from_multi_id(
                                            crate_id,
                                            module_id,
                                            self.live_registry.token_id_to_span(prop.token_id),
                                            target,
                                            &doc.multi_ids
                                        ) ?;
                                        draw_input_srid = Some((srid, self.live_registry.token_id_to_span(prop.token_id)))
                                    },
                                    id_pack!(default_geometry) => {
                                        // this thing needs to have 3 seg
                                        let srid = Self::parse_shader_resource_id_from_multi_id(
                                            crate_id,
                                            module_id,
                                            self.live_registry.token_id_to_span(prop.token_id),
                                            target,
                                            &doc.multi_ids
                                        ) ?;
                                        draw_shader_def.default_geometry = Some(srid);
                                    },
                                    _ => { // unknown
                                        return Err(LiveError {
                                            origin: live_error_origin!(),
                                            span: self.live_registry.token_id_to_span(prop.token_id),
                                            message: format!("Unknown var ref type {}", prop.id_pack)
                                        })
                                    }
                                }
                            }
                            LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                                if let IdUnpack::Single(id) = prop.id_pack.unpack() {
                                    let mut parser = ShaderParser::new(
                                        self,
                                        doc.get_tokens(token_start, token_count + 1),
                                        doc.get_scopes(scope_start, scope_count),
                                        &mut parser_deps,
                                        Some(FnSelfKind::DrawShader(shader_ptr))
                                        //None
                                    );
                                    let decl = parser.expect_self_decl(Ident(id), prop_ptr) ?;
                                    if let Some(decl) = decl {
                                        draw_shader_def.fields.push(decl);
                                    }
                                    //else{ // it was a const
                                    //}
                                }
                            },
                            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                                if let IdUnpack::Single(id) = prop.id_pack.unpack() {
                                    // lets parse this thing
                                    let parser = ShaderParser::new(
                                        self,
                                        doc.get_tokens(token_start, token_count + 1),
                                        doc.get_scopes(scope_start, scope_count),
                                        &mut parser_deps,
                                        Some(FnSelfKind::DrawShader(shader_ptr))
                                        //None
                                    );
                                    
                                    let fn_def = parser.expect_method_def(
                                        FnNodePtr(prop_ptr),
                                        Ident(id),
                                    ) ?;
                                    if let Some(fn_def) = fn_def {
                                        draw_shader_def.methods.push(fn_def.fn_node_ptr);
                                        self.all_fns.insert(fn_def.fn_node_ptr, fn_def);
                                    }
                                }
                            }
                            _ => ()
                        }
                    }
                    // if we have a draw_input process it.
                    if let Some((draw_input_srid, span)) = draw_input_srid {
                        if let Some(draw_input) = self.draw_inputs.get(&draw_input_srid) {
                            for decl in &draw_shader_def.fields {
                                if let DrawShaderFieldKind::Instance {..} = decl.kind {
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span,
                                        message: format!("Cannot use both instance defs and draw_input {}", draw_input_srid)
                                    })
                                }
                            }
                            for instance in &draw_input.instances {
                                draw_shader_def.fields.push(
                                    DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Instance {
                                            is_used_in_pixel_shader: Cell::new(false),
                                            input_type: DrawShaderInputType::ShaderResourceId(draw_input_srid),
                                        },
                                        span,
                                        ident: instance.ident,
                                        ty_expr: instance.ty_expr.clone(),
                                    }
                                )
                            }
                            
                            for uniform in &draw_input.uniforms {
                                draw_shader_def.fields.push(
                                    DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Uniform {
                                            block_ident: Ident(id!(default)),
                                            input_type: DrawShaderInputType::ShaderResourceId(draw_input_srid),
                                        },
                                        span,
                                        ident: uniform.ident,
                                        ty_expr: uniform.ty_expr.clone(),
                                    }
                                )
                            }
                            
                            for texture in &draw_input.textures {
                                draw_shader_def.fields.push(
                                    DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Texture {
                                            input_type: DrawShaderInputType::ShaderResourceId(draw_input_srid),
                                        },
                                        span,
                                        ident: texture.ident,
                                        ty_expr: texture.ty_expr.clone(),
                                    }
                                )
                            }
                        }
                        else {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span,
                                message: format!("Cannot find draw_input {}", draw_input_srid)
                            })
                        }
                    }
                    // lets check for duplicate fields
                    for i in 0..draw_shader_def.fields.len(){
                        for j in (i+1)..draw_shader_def.fields.len(){
                            let field_a = &draw_shader_def.fields[i]; 
                            let field_b = &draw_shader_def.fields[j]; 
                            if field_a.ident == field_b.ident{
                               return Err(LiveError {
                                    origin: live_error_origin!(),
                                    span:field_a.span,
                                    message: format!("Field double declaration {}", field_b.ident)
                                })
                            }
                        }
                    }
                    
                    self.draw_shaders.insert(shader_ptr, draw_shader_def);
                    
                    self.analyse_deps(&parser_deps) ?;
                    
                    let mut sa = DrawShaderAnalyser {
                        draw_shader_def: self.draw_shaders.get(&shader_ptr).unwrap(),
                        scopes: &mut Scopes::new(),
                        shader_registry: self,
                        options: ShaderAnalyseOptions {
                            no_const_collapse: true
                        }
                    };
                    sa.analyse_shader() ?;
                    
                    // ok we have all structs
                    return Ok(())
                }
                _ => ()
            }
        }
        return Err(LiveError {
            origin: live_error_origin!(),
            span: Span::default(),
            message: format!("analyse_draw_shader could not find {} {} {} ", crate_id, module_id, ids[0])
        })
    }
    
    pub fn generate_glsl_shader(&mut self, crate_id: Id, module_id: Id, ids: &[Id], const_file_id: Option<FileId>) -> Result<(String, String), LiveError> {
        // lets find the FullPointer
        if let Some(shader_ptr) = self.live_registry.find_full_node_ptr_from_ids(crate_id, module_id, ids) {
            if let Some(draw_shader_decl) = self.draw_shaders.get(&DrawShaderNodePtr(shader_ptr)) {
                // TODO this env needs its const table transferred
                let final_const_table = self.compute_final_const_table(draw_shader_decl, const_file_id);
                let vertex = generate_glsl::generate_vertex_shader(draw_shader_decl, &final_const_table, self);
                let pixel = generate_glsl::generate_pixel_shader(draw_shader_decl, &final_const_table, self);
                return Ok((vertex, pixel))
            }
            
        }
        return Err(LiveError {
            origin: live_error_origin!(),
            span: Span::default(),
            message: format!("generate_glsl_shader could not find {} {} {} ", crate_id, module_id, ids[0])
        })
    }
    
    pub fn generate_metal_shader(&mut self, crate_id: Id, module_id: Id, ids: &[Id], const_file_id: Option<FileId>) -> Result<String, LiveError> {
        // lets find the FullPointer
        if let Some(shader_ptr) = self.live_registry.find_full_node_ptr_from_ids(crate_id, module_id, ids) {
            if let Some(draw_shader_decl) = self.draw_shaders.get(&DrawShaderNodePtr(shader_ptr)) {
                // TODO this env needs its const table transferred
                let final_const_table = self.compute_final_const_table(draw_shader_decl, const_file_id);
                let shader = generate_metal::generate_shader(draw_shader_decl, &final_const_table, self);
                return Ok(shader)
            }
        }
        return Err(LiveError {
            origin: live_error_origin!(),
            span: Span::default(),
            message: format!("generate_glsl_shader could not find {} {} {} ", crate_id, module_id, ids[0])
        })
    }
    
}
