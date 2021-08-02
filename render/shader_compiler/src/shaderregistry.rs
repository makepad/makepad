#![allow(unused_variables)]

use makepad_live_parser::*;
use crate::shaderast::*;
use crate::analyse::*;

//use crate::generate::*;

use crate::shaderparser::ShaderParser;
use crate::shaderparser::ShaderParserDep;
use std::fmt;
use std::cell::{RefCell,Cell};
use std::collections::HashMap;
use crate::builtin::Builtin;
use crate::builtin::generate_builtins;
use crate::env::Env;
use crate::generate_glsl;

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
    pub consts: HashMap<ConstNodePtr, ConstDecl>,
    pub plain_fns: HashMap<FnNodePtr, FnDecl>,
    pub draw_shaders: HashMap<DrawShaderNodePtr, DrawShaderDecl>,
    pub structs: HashMap<StructNodePtr, StructDecl>,
    
    pub draw_inputs: HashMap<ShaderResourceId, DrawShaderInput>,
    pub builtins: HashMap<Ident, Builtin>,
}

impl ShaderRegistry {
    pub fn new() -> Self {
        Self {
            live_registry: LiveRegistry::default(),
            structs: HashMap::new(),
            consts: HashMap::new(),
            draw_shaders: HashMap::new(),
            plain_fns: HashMap::new(),
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
        if let TyExprKind::Lit {ty_lit, ..} = ty_expr.kind {
            if ty_lit == TyLit::Texture2D {
                self.textures.push(DrawShaderInputItem {ident: Ident(Id::from_str(name)), ty_expr});
                return
            }
        }
        self.uniforms.push(DrawShaderInputItem {ident: Ident(Id::from_str(name)), ty_expr});
    }
    
    pub fn add_instance(&mut self, modpath: &str, cls: &str, name: &str, ty_expr: TyExpr) {
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

impl ShaderRegistry {
    
    /*
    pub fn get_plain_fn_decl(&self, fn_ptr: FnNodePtr) -> Option<&FnDecl> {
        self.plain_fns.get(&fn_ptr)
    }
    
    pub fn struct_decl_from_ptr(&self, struct_ptr: StructNodePtr) -> Option<&StructDecl> {
        self.structs.get(&struct_ptr)
    }
    
    pub fn draw_shader_decl_from_ptr(&self, shader_ptr: DrawShaderNodePtr) -> Option<&DrawShaderDecl> {
        self.draw_shaders.get(&shader_ptr)
    }
    
    pub fn const_decl_from_ptr(&self, const_ptr: ConstNodePtr) -> Option<&ConstDecl> {
        self.consts.get(&const_ptr)
    }*/

    pub fn struct_method_from_ptr(&self, struct_node_ptr: StructNodePtr, ident: Ident) -> Option<&FnDecl> {
        if let Some(s) = self.structs.get(&struct_node_ptr) {
            if let Some(node) = s.methods.iter().find( | fn_decl | fn_decl.ident == ident) {
                return Some(node)
            }
        }
        None
    }
    
    pub fn draw_shader_method_from_ptr(&self, shader_ptr: DrawShaderNodePtr, ident: Ident) -> Option<&FnDecl> {
        if let Some(s) = self.draw_shaders.get(&shader_ptr) {
            if let Some(node) = s.methods.iter().find( | fn_decl | fn_decl.ident == ident) {
                return Some(node)
            }
        }
        None
    }
    
    pub fn plain_fn_from_ptr(&self, fn_ptr: FnNodePtr) -> Option<&FnDecl> {
        if let Some(s) = self.plain_fns.get(&fn_ptr) {
            return Some(s)
        }
        None
    }
    
    pub fn fn_decl_from_callee(&self, callee:&Callee) -> Option<&FnDecl> {
        match callee{
            Callee::PlainFn {fn_node_ptr}=>self.plain_fn_from_ptr(*fn_node_ptr),
            Callee::DrawShaderMethod {shader_node_ptr, ident}=>self.draw_shader_method_from_ptr(*shader_node_ptr, *ident),
            Callee::StructMethod {struct_node_ptr, ident}=>self.struct_method_from_ptr(*struct_node_ptr, *ident),
        }
    }
    
    pub fn fn_ident_from_ptr(&self, fn_node_ptr: FnNodePtr) -> Ident {
        let (_, node) = self.live_registry.resolve_ptr(fn_node_ptr.0);
        Ident(node.id_pack.unwrap_single())
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
                
                let const_decl = parser.expect_const_decl(Ident(id)) ?;
                self.consts.insert(const_ptr, const_decl);
                
                self.analyse_deps(&parser_deps) ?;
                
                let mut ca = ConstAnalyser {
                    decl: self.consts.get(&const_ptr).unwrap(),
                    env: &mut Env::new(),
                    shader_registry: self,
                    options: ShaderAnalyseOptions{
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
        
        if self.plain_fns.get(&fn_ptr).is_some() {
            return Ok(());
        }
        // alright lets parse and analyse a plain fn
        let (doc, fn_node) = self.live_registry.resolve_ptr(fn_ptr.0);
        match fn_node.value {
            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                let id = fn_node.id_pack.unwrap_single();
                let mut parser_deps = Vec::new();
                // lets parse this thing
                let mut parser = ShaderParser::new(
                    self,
                    doc.get_tokens(token_start, token_count + 1),
                    doc.get_scopes(scope_start, scope_count),
                    &mut parser_deps,
                    if let Some(struct_ptr) = struct_ptr {Some(FnSelfKind::Struct(struct_ptr))}else {None},
                    //Some(struct_full_ptr)
                );
                
                let fn_decl = parser.expect_plain_fn_decl(
                    fn_ptr,
                    Ident(id),
                ) ?;
                self.plain_fns.insert(fn_ptr, fn_decl);
                
                self.analyse_deps(&parser_deps) ?;
                
                // ok analyse the struct methods now.
                let mut fa = FnDefAnalyser {
                    decl: self.plain_fns.get(&fn_ptr).unwrap(),
                    env: &mut Env::new(),
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
                let mut struct_decl = StructDecl {
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
                            let decl = parser.expect_field(Ident(id), VarDefNodePtr(prop_ptr)) ?;
                            struct_decl.fields.push(decl);
                        },
                        LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id_pack.unwrap_single();
                            // lets parse this thing
                            let mut parser = ShaderParser::new(
                                self,
                                doc.get_tokens(token_start, token_count + 1),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr))
                                //Some(struct_full_ptr)
                            );
                            
                            let fn_decl = parser.expect_method_decl(
                                FnNodePtr(prop_ptr),
                                Ident(id),
                            ) ?;
                            // if we get false, this was not a method but could be static.
                            // statics need a pointer to their struct to resolve Self
                            // so we can't treat them purely as loose methods
                            if let Some(fn_decl) = fn_decl {
                                struct_decl.methods.push(fn_decl)
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
                self.structs.insert(struct_ptr, struct_decl);
                
                self.analyse_deps(&parser_deps) ?;
                
                // ok analyse the struct methods now.
                let mut sa = StructAnalyser {
                    struct_decl: self.structs.get(&struct_ptr).unwrap(),
                    env: &mut Env::new(),
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
            let mut draw_shader_decl = DrawShaderDecl::default();
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
                                        draw_shader_decl.default_geometry = Some(srid);
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
                                        draw_shader_decl.fields.push(decl);
                                    }
                                    //else{ // it was a const
                                    //}
                                }
                            },
                            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                                if let IdUnpack::Single(id) = prop.id_pack.unpack() {
                                    // lets parse this thing
                                    let mut parser = ShaderParser::new(
                                        self,
                                        doc.get_tokens(token_start, token_count + 1),
                                        doc.get_scopes(scope_start, scope_count),
                                        &mut parser_deps,
                                        Some(FnSelfKind::DrawShader(shader_ptr))
                                        //None
                                    );
                                    
                                    let fn_decl = parser.expect_method_decl(
                                        FnNodePtr(prop_ptr),
                                        Ident(id),
                                    ) ?;
                                    if let Some(fn_decl) = fn_decl {
                                        draw_shader_decl.methods.push(fn_decl)
                                    }
                                }
                            }
                            _ => ()
                        }
                    }
                    // if we have a draw_input process it.
                    if let Some((draw_input_srid, span)) = draw_input_srid {
                        if let Some(draw_input) = self.draw_inputs.get(&draw_input_srid) {
                            for decl in &draw_shader_decl.fields {
                                if let DrawShaderFieldKind::Instance {..} = decl.kind {
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span,
                                        message: format!("Cannot use both instance defs and draw_input {}", draw_input_srid)
                                    })
                                }
                            }
                            for instance in &draw_input.instances {
                                draw_shader_decl.fields.push(
                                    DrawShaderFieldDecl {
                                        kind: DrawShaderFieldKind::Instance {
                                            is_used_in_pixel_shader: Cell::new(false),
                                            input_node_ptr: InputNodePtr::ShaderResourceId(draw_input_srid),
                                        },
                                        span,
                                        ident: instance.ident,
                                        ty_expr: instance.ty_expr.clone(),
                                    }
                                )
                            }
                            
                            for uniform in &draw_input.uniforms {
                                draw_shader_decl.fields.push(
                                    DrawShaderFieldDecl {
                                        kind: DrawShaderFieldKind::Uniform {
                                            block_ident: Ident(id!(default)),
                                            input_node_ptr: InputNodePtr::ShaderResourceId(draw_input_srid),
                                        },
                                        span,
                                        ident: uniform.ident,
                                        ty_expr: uniform.ty_expr.clone(),
                                    }
                                )
                            }
                            
                            for texture in &draw_input.textures {
                                draw_shader_decl.fields.push(
                                    DrawShaderFieldDecl {
                                        kind: DrawShaderFieldKind::Texture {
                                            input_node_ptr: InputNodePtr::ShaderResourceId(draw_input_srid),
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
                    self.draw_shaders.insert(shader_ptr, draw_shader_decl);
                    
                    self.analyse_deps(&parser_deps) ?;
                    
                    let mut sa = DrawShaderAnalyser {
                        draw_shader_decl: self.draw_shaders.get(&shader_ptr).unwrap(),
                        env: &mut Env::new(),
                        shader_registry: self,
                        options: ShaderAnalyseOptions {
                            no_const_collapse: true
                        }
                    };
                    sa.analyse_shader(shader_ptr) ?;
                    
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
    
    pub fn generate_glsl_shader(&mut self, crate_id: Id, module_id: Id, ids: &[Id], const_file_id:Option<FileId>) -> Result<(String, String), LiveError> {
        // lets find the FullPointer
        if let Some(shader_ptr) = self.live_registry.find_full_node_ptr_from_ids(crate_id, module_id, ids) {
            // lets generate a vertex shader
            
            if let Some(draw_shader_decl) = self.draw_shaders.get(&DrawShaderNodePtr(shader_ptr)) {
                // TODO this env needs its const table transferred
                let result = generate_glsl::generate_pixel_shader(draw_shader_decl, self, ShaderGenerateOptions{
                    const_file_id
                });
                println!("GOT RESULT {}", result);
            }
            
        }
        return Err(LiveError {
            origin: live_error_origin!(),
            span: Span::default(),
            message: format!("generate_glsl_shader could not find {} {} {} ", crate_id, module_id, ids[0])
        })
    }
    
    
}
