#![allow(unused_variables)]

use makepad_live_parser::*;
use crate::shaderast::*;
use crate::analyse::*;

use crate::shaderparser::ShaderParser;
use crate::shaderparser::ShaderParserDep;
use std::collections::BTreeMap;
use std::cell::{RefCell};
use std::collections::HashMap;
use crate::builtin::Builtin;
use crate::builtin::generate_builtins;
use crate::shaderast::Scopes;
/*
#[derive(Clone, Debug, Copy, Hash, Eq, PartialEq)]
pub struct ShaderResourceId(CrateModule, Id);

impl fmt::Display for ShaderResourceId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}::{}", self.0, self.1)
    }
}*/

pub struct ShaderRegistry {
    pub live_registry: LiveRegistry,
    pub consts: HashMap<ConstPtr, ConstDef>,
    pub all_fns: HashMap<FnPtr, FnDef>,
    pub draw_shader_defs: HashMap<DrawShaderPtr, DrawShaderDef>,
    pub structs: HashMap<StructPtr, StructDef>,
    pub builtins: HashMap<Ident, Builtin>,
}

impl ShaderRegistry {
    pub fn new() -> Self {
        Id::from_str("user").unwrap();
        Self {
            live_registry: LiveRegistry::default(),
            structs: HashMap::new(),
            consts: HashMap::new(),
            draw_shader_defs: HashMap::new(),
            all_fns: HashMap::new(),
            builtins: generate_builtins()
        }
    }
}

pub enum LiveNodeFindResult {
    NotFound,
    Component(LivePtr),
    Struct(StructPtr),
    Function(FnPtr),
    PossibleStatic(StructPtr, FnPtr),
    Const(ConstPtr),
    LiveValue(ValuePtr, TyLit)
}


impl ShaderRegistry {
    
    pub fn compute_const_table(&self, draw_shader_def: &mut DrawShaderDef, filter_file_id: FileId)->DrawShaderConstTable {
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
        let size = table.len();
        let align_gap = 4 - (size - ((size >> 2) << 2));
        for _ in 0..align_gap {
            table.push(0.0);
        }
        DrawShaderConstTable{
            table,
            offsets
        }
    }
    
    pub fn fn_ident_from_ptr(&self, fn_node_ptr: FnPtr) -> Ident {
        let node = self.live_registry.resolve_ptr(fn_node_ptr.0);
        Ident(node.id)
    }
    
    pub fn draw_shader_method_ptr_from_ident(&self, draw_shader_def: &DrawShaderDef, ident: Ident) -> Option<FnPtr> {
        for fn_node_ptr in &draw_shader_def.methods {
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident {
                return Some(*fn_node_ptr);
            }
        }
        None
    }
    
    pub fn struct_method_ptr_from_ident(&self, struct_def: &StructDef, ident: Ident) -> Option<FnPtr> {
        for fn_node_ptr in &struct_def.methods {
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident {
                return Some(*fn_node_ptr);
            }
        }
        None
    }
    
    pub fn draw_shader_method_decl_from_ident(&self, draw_shader_def: &DrawShaderDef, ident: Ident) -> Option<&FnDef> {
        for fn_node_ptr in &draw_shader_def.methods {
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident {
                return Some(fn_decl)
            }
        }
        None
    }
    
    pub fn struct_method_decl_from_ident(&self, struct_def: &StructDef, ident: Ident) -> Option<&FnDef> {
        for fn_node_ptr in &struct_def.methods {
            let fn_decl = self.all_fns.get(fn_node_ptr).unwrap();
            if fn_decl.ident == ident {
                return Some(fn_decl)
            }
        }
        None
    }
    
    
    pub fn find_live_node_by_path(&self, base_ptr: LivePtr, ids: &[Id]) -> LiveNodeFindResult {
        // what are the types of things we can find.
        
        fn no_ids(ids: &[Id], result: LiveNodeFindResult) -> LiveNodeFindResult {
            if ids.len() == 0 {result} else {LiveNodeFindResult::NotFound}
        }
        
        let (doc, node) = self.live_registry.resolve_doc_ptr(base_ptr);
        match node.value {
            LiveValue::Bool(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValuePtr(base_ptr), TyLit::Bool)),
            LiveValue::Int(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValuePtr(base_ptr), TyLit::Int)),
            LiveValue::Float(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValuePtr(base_ptr), TyLit::Float)),
            LiveValue::Color(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValuePtr(base_ptr), TyLit::Vec4)),
            LiveValue::Vec2(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValuePtr(base_ptr), TyLit::Vec2)),
            LiveValue::Vec3(_) => return no_ids(ids, LiveNodeFindResult::LiveValue(ValuePtr(base_ptr), TyLit::Vec3)),
            LiveValue::Fn {..} => return no_ids(ids, LiveNodeFindResult::Function(FnPtr(base_ptr))),
            LiveValue::Const{token_start,..}=>return no_ids(ids, LiveNodeFindResult::Const(ConstPtr(base_ptr))), 
            LiveValue::Class {class, node_start: ns, node_count: nc, ..} => {
                if ids.len() == 0 { // check if we are struct or component
                    let base_class =  self.live_registry.find_base_class_id(class);
                    
                    match self.live_registry.find_base_class_id(class) {
                        multi_id!(Struct) => return LiveNodeFindResult::Struct(StructPtr(base_ptr)),
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
                        if node.id == id {
                            // we found the node.
                            let node_ptr = LocalPtr {level: level, index: j + node_start};
                            if i == ids.len() - 1 { // last item
                                let full_node_ptr = LivePtr {file_id: base_ptr.file_id, local_ptr: node_ptr};
                                match node.value {
                                    LiveValue::Class {class, ..} => {
                                        match self.live_registry.find_base_class_id(class) {
                                            multi_id!(Struct) => return LiveNodeFindResult::Struct(StructPtr(full_node_ptr)),
                                            multi_id!(Component) => return LiveNodeFindResult::Component(full_node_ptr),
                                            _ => return LiveNodeFindResult::NotFound
                                        }
                                    },
                                    LiveValue::Fn {..} => { // check if its a method or a free roaming function
                                        let full_base_ptr = LivePtr {file_id: base_ptr.file_id, local_ptr: parent_ptr};
                                        let base_node = doc.resolve_ptr(parent_ptr);
                                        if let LiveValue::Class {class, ..} = base_node.value {
                                            // lets check if our base is a component or a struct
                                            match self.live_registry.find_base_class_id(class) {
                                                multi_id!(Struct) => return LiveNodeFindResult::PossibleStatic(
                                                    StructPtr(full_base_ptr),
                                                    FnPtr(full_node_ptr)
                                                ),
                                                _ => return LiveNodeFindResult::Function(FnPtr(full_node_ptr)),
                                            }
                                        }
                                        else {
                                            panic!()
                                        }
                                    },
                                    LiveValue::Bool(_) => return LiveNodeFindResult::LiveValue(ValuePtr(full_node_ptr), TyLit::Bool),
                                    LiveValue::Int(_) => return LiveNodeFindResult::LiveValue(ValuePtr(full_node_ptr), TyLit::Int),
                                    LiveValue::Float(_) => return LiveNodeFindResult::LiveValue(ValuePtr(full_node_ptr), TyLit::Float),
                                    LiveValue::Color(_) => return LiveNodeFindResult::LiveValue(ValuePtr(full_node_ptr), TyLit::Vec4),
                                    LiveValue::Vec2(_) => return LiveNodeFindResult::LiveValue(ValuePtr(full_node_ptr), TyLit::Vec2),
                                    LiveValue::Vec3(_) => return LiveNodeFindResult::LiveValue(ValuePtr(full_node_ptr), TyLit::Vec3),
                                    LiveValue::Const{token_start,..}=>return LiveNodeFindResult::Const(ConstPtr(full_node_ptr)),
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
    pub fn analyse_const(&mut self, const_ptr: ConstPtr) -> Result<(), LiveError> {
        if self.consts.get(&const_ptr).is_some() {
            return Ok(());
        }
        let (doc, const_node) = self.live_registry.resolve_doc_ptr(const_ptr.0);
        match const_node.value {
            LiveValue::Const {token_start, token_count, scope_start, scope_count} => {
                let mut parser_deps = Vec::new();
                let id = const_node.id;
                let origin_doc = &self.live_registry.get_origin_doc_from_token_id(const_node.token_id);
                let mut parser = ShaderParser::new(
                    self,
                    origin_doc.get_tokens(token_start, token_count + 1),
                    doc.get_scopes(scope_start, scope_count),
                    &mut parser_deps,
                    None,
                    const_ptr.0.file_id
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
    pub fn analyse_plain_fn(&mut self, struct_ptr: Option<StructPtr>, fn_ptr: FnPtr) -> Result<(), LiveError> {
        
        if self.all_fns.get(&fn_ptr).is_some() {
            return Ok(());
        }
        // alright lets parse and analyse a plain fn
        let (doc, fn_node) = self.live_registry.resolve_doc_ptr(fn_ptr.0);
        match fn_node.value {
            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                let id = fn_node.id;
                let mut parser_deps = Vec::new();
                // lets parse this thing
                let origin_doc = &self.live_registry.get_origin_doc_from_token_id(fn_node.token_id);
                let parser = ShaderParser::new(
                    self,
                    origin_doc.get_tokens(token_start, token_count + 1),
                    doc.get_scopes(scope_start, scope_count),
                    &mut parser_deps,
                    if let Some(struct_ptr) = struct_ptr {Some(FnSelfKind::Struct(struct_ptr))}else {None},
                    fn_ptr.0.file_id
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
    pub fn analyse_struct(&mut self, struct_ptr: StructPtr) -> Result<(), LiveError> {
        
        if self.structs.get(&struct_ptr).is_some() {
            return Ok(());
        }
        
        let (doc, class_node) = self.live_registry.resolve_doc_ptr(struct_ptr.0);
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
                    let prop_ptr = LivePtr {file_id: struct_ptr.0.file_id, local_ptr: LocalPtr {
                        level: struct_ptr.0.local_ptr.level + 1,
                        index: i + node_start as usize
                    }};
                    let prop = doc.resolve_ptr(prop_ptr.local_ptr);
                    match prop.value {
                        LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id;
                            let origin_doc = &self.live_registry.get_origin_doc_from_token_id(prop.token_id);
                            let mut parser = ShaderParser::new(
                                self,
                                origin_doc.get_tokens(token_start, token_count + 1),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr)),
                                struct_ptr.0.file_id
                                //Some(struct_full_ptr)
                            );
                            // we only allow a field def
                            let def = parser.expect_field(Ident(id), VarDefPtr(prop_ptr)) ?;
                            if let Some(def) = def {
                                struct_def.fields.push(def);
                            }
                        },
                        LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id;
                            // lets parse this thing
                            let origin_doc = &self.live_registry.get_origin_doc_from_token_id(prop.token_id);
                            let parser = ShaderParser::new(
                                self,
                                origin_doc.get_tokens(token_start, token_count + 1),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr)),
                                struct_ptr.0.file_id
                                //Some(struct_full_ptr)
                            );
                            
                            let fn_def = parser.expect_method_def(
                                FnPtr(prop_ptr),
                                Ident(id),
                            ) ?;
                            // if we get false, this was not a method but could be static.
                            // statics need a pointer to their struct to resolve Self
                            // so we can't treat them purely as loose methods
                            if let Some(fn_def) = fn_def {
                                struct_def.methods.push(fn_def.fn_ptr);
                                self.all_fns.insert(fn_def.fn_ptr, fn_def);
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
    pub fn analyse_draw_shader<F>(&mut self, draw_shader_ptr: DrawShaderPtr, mut ext_self: F) -> Result<&DrawShaderDef,
    LiveError>
    where F: FnMut(Span, Id, LiveType, &mut DrawShaderDef)
    {
        let mut draw_shader_def = DrawShaderDef::default();

        let (doc, class_node) = self.live_registry.resolve_doc_ptr(draw_shader_ptr.0);
        
        match class_node.value {
            LiveValue::Class {node_start, node_count, ..} => {
                let mut parser_deps = Vec::new();
                let mut iter = self.live_registry.live_object_iterator(draw_shader_ptr.0, node_start, node_count);
                while let Some((id, prop_ptr)) = iter.next_id(&self.live_registry) {
                    let prop = doc.resolve_ptr(prop_ptr.local_ptr);
                    
                    match prop.value {
                        // if we have a float or a vec2/3/4
                        // we should look to set a default value
                        LiveValue::Bool(val)=>{
                            if prop.id == id!(debug) {
                                draw_shader_def.flags.debug = true;
                            }
                            if prop.id == id!(draw_call_compare){
                                draw_shader_def.flags.draw_call_compare = true;
                            }
                            if prop.id == id!(draw_call_always){
                                draw_shader_def.flags.draw_call_always = true;
                            }
                        }
                        LiveValue::LiveType(lt) => {
                            if prop.id == id!(rust_type) {
                                ext_self(
                                    self.live_registry.token_id_to_span(prop.token_id),
                                    id,
                                    lt,
                                    &mut draw_shader_def
                                );
                            }
                        }
                        LiveValue::Class {class, node_start, node_count} => {
                            // if our id is geometry, process it
                            if prop.id == id!(geometry) {
                                // we need to find the rust_type from here
                                if let Some(local_ptr) = doc.scan_for_object_path_from(
                                    &[id!(rust_type)],
                                    node_start as usize,
                                    node_count as usize,
                                    prop_ptr.local_ptr.level + 1
                                ) {
                                    let node = doc.resolve_ptr(local_ptr);
                                    if let LiveValue::LiveType(lt) = node.value {
                                        ext_self(
                                            self.live_registry.token_id_to_span(prop.token_id),
                                            id,
                                            lt,
                                            &mut draw_shader_def
                                        );
                                    }
                                }
                            }
                        },
                        LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                            let origin_doc = &self.live_registry.get_origin_doc_from_token_id(prop.token_id);
                            let mut parser = ShaderParser::new(
                                self,
                                origin_doc.get_tokens(token_start, token_count),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::DrawShader(draw_shader_ptr)),
                                draw_shader_ptr.0.file_id
                                //None
                            );
                            let decl = parser.expect_self_decl(Ident(prop.id), prop_ptr) ?;
                            if let Some(decl) = decl {
                                // lets see where to inject this.
                                // if its an instance var it needs to
                                // go above the var_def_node_ptr one
                                if let DrawShaderFieldKind::Instance {..} = decl.kind {
                                    // find from the start the first instancefield
                                    // without a var_def_node_prt
                                    if let Some(index) = draw_shader_def.fields.iter().position( | field | {
                                        if let DrawShaderFieldKind::Instance {var_def_ptr, ..} = field.kind {
                                            if var_def_ptr.is_none() {
                                                return true
                                            }
                                        }
                                        false
                                    }) {
                                        draw_shader_def.fields.insert(index, decl);
                                    }
                                    else {
                                        draw_shader_def.fields.push(decl);
                                    }
                                }
                                else {
                                    draw_shader_def.fields.push(decl);
                                }
                            }
                        },
                        LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                            let origin_doc = &self.live_registry.get_origin_doc_from_token_id(prop.token_id);
                            let parser = ShaderParser::new(
                                self,
                                origin_doc.get_tokens(token_start, token_count),
                                doc.get_scopes(scope_start, scope_count),
                                &mut parser_deps,
                                Some(FnSelfKind::DrawShader(draw_shader_ptr)),
                                draw_shader_ptr.0.file_id
                                //None
                            );
                            
                            let fn_def = parser.expect_method_def(
                                FnPtr(prop_ptr),
                                Ident(prop.id),
                            ) ?;
                            if let Some(fn_def) = fn_def {
                                draw_shader_def.methods.push(fn_def.fn_ptr);
                                self.all_fns.insert(fn_def.fn_ptr, fn_def);
                            }
                        }
                        _ => ()
                    }
                }
                // lets check for duplicate fields
                for i in 0..draw_shader_def.fields.len() {
                    for j in (i + 1)..draw_shader_def.fields.len() {
                        let field_a = &draw_shader_def.fields[i];
                        let field_b = &draw_shader_def.fields[j];
                        if field_a.ident == field_b.ident {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: field_a.span,
                                message: format!("Field double declaration {}", field_b.ident)
                            })
                        }
                    }
                }
                
                self.draw_shader_defs.insert(draw_shader_ptr, draw_shader_def);
                
                self.analyse_deps(&parser_deps) ?;
                
                let draw_shader_def = self.draw_shader_defs.get(&draw_shader_ptr).unwrap();
                let mut sa = DrawShaderAnalyser {
                    draw_shader_def: draw_shader_def,
                    scopes: &mut Scopes::new(),
                    shader_registry: self,
                    options: ShaderAnalyseOptions {
                        no_const_collapse: true
                    }
                };
                sa.analyse_shader() ?;
                
                // ok we have all structs
                return Ok(draw_shader_def)
            }
            _ => return Err(LiveError {
                origin: live_error_origin!(),
                span: Span::default(),
                message: format!("analyse_draw_shader could not find shader class")
            })
        }
    }
    /*
    pub fn generate_glsl_shader(&mut self, shader_ptr: DrawShaderNodePtr) -> (String, String) {
        // lets find the FullPointer
        let draw_shader_decl = self.draw_shaders.get(&shader_ptr).unwrap();
        // TODO this env needs its const table transferred
        let vertex = generate_glsl::generate_vertex_shader(draw_shader_decl, self);
        let pixel = generate_glsl::generate_pixel_shader(draw_shader_decl, self);
        return (vertex, pixel)
    }
    
    pub fn generate_metal_shader(&mut self, shader_ptr: DrawShaderNodePtr) -> String{
        // lets find the FullPointer
        let draw_shader_decl = self.draw_shaders.get(&shader_ptr).unwrap();
        let shader = generate_metal::generate_shader(draw_shader_decl, self);
        return shader
    }
    
    pub fn generate_hlsl_shader(&mut self, shader_ptr: DrawShaderNodePtr) -> String{
        // lets find the FullPointer
        let draw_shader_decl = self.draw_shaders.get(&shader_ptr).unwrap();
        let shader = generate_hlsl::generate_shader(draw_shader_decl, self);
        return shader
    }*/
}
