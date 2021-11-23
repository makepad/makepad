use makepad_live_compiler::*;
use crate::shaderast::*;
use crate::analyse::*;

use crate::shaderparser::ShaderParser;
use crate::shaderparser::ShaderParserDep;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::cell::{RefCell};
use std::collections::HashMap;
use crate::builtin::Builtin;
use crate::builtin::generate_builtins;
use crate::shaderast::Scopes;

pub struct ShaderRegistry {
    pub consts: HashMap<ConstPtr, ConstDef>,
    pub all_fns: HashMap<FnPtr, FnDef>,
    pub draw_shader_defs: HashMap<DrawShaderPtr, DrawShaderDef>,
    pub structs: HashMap<StructPtr, StructDef>,
    pub builtins: HashMap<Ident, Builtin>,
    pub enums: HashMap<LiveType, ShaderEnum>,
}

pub struct ShaderEnum{
    pub enum_name: Id,
    pub variants:Vec<Id>
}

impl ShaderRegistry {
    pub fn new() -> Self {
        Id::from_str("user").unwrap();
        Self {
            structs: HashMap::new(),
            consts: HashMap::new(),
            enums: HashMap::new(),
            draw_shader_defs: HashMap::new(),
            all_fns: HashMap::new(),
            builtins: generate_builtins()
        }
    }
}

#[derive(Debug)] 
pub enum LiveNodeFindResult {
    NotFound,
    Component(LivePtr),
    Struct(StructPtr),
    Function(FnPtr),
    PossibleStatic(StructPtr, FnPtr),
    Const(ConstPtr),
    LiveValue(ValuePtr, TyLit)
}

pub enum DrawShaderQuery{
    DrawShader,
    Geometry,
}

impl ShaderRegistry {
    
    pub fn register_enum(&mut self, live_type:LiveType, shader_enum: ShaderEnum){
        self.enums.insert(live_type, shader_enum);
    }
    
    pub fn compute_const_table(&self, draw_shader_def: &mut DrawShaderDef, filter_file_id: FileId) -> DrawShaderConstTable {
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
        DrawShaderConstTable {
            table,
            offsets
        }
    }
    
    pub fn fn_ident_from_ptr(&self, live_registry:&LiveRegistry, fn_node_ptr: FnPtr) -> Ident {
        let node = live_registry.ptr_to_node(fn_node_ptr.0);
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
    
    pub fn find_live_node_by_path(&self, live_registry:&LiveRegistry, base_ptr: LivePtr, ids: &[Id]) -> LiveNodeFindResult {
        
        
        let doc = &live_registry.ptr_to_doc(base_ptr);

        return walk_recur(live_registry, None, base_ptr.file_id, base_ptr.local_ptr.0, &doc.nodes, ids);
        // ok so we got a node. great. now what
        fn walk_recur(live_registry:&LiveRegistry, struct_ptr:Option<LivePtr>,file_id: FileId, index: usize, nodes: &[LiveNode], ids: &[Id]) -> LiveNodeFindResult {
            let node = &nodes[index];
            //println!("RESOLVING {:?}", ids);
    
            if ids.len() != 0 && !node.value.is_class() && !node.value.is_clone() && !node.value.is_object() {
                return LiveNodeFindResult::NotFound;
            }
            
            let now_ptr = LivePtr {file_id, local_ptr: LocalPtr(index)};
    
             
            match node.value {
                LiveValue::Bool(_) => return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Bool),
                LiveValue::Int(_) => return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Int),
                LiveValue::Float(_) => return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Float),
                LiveValue::Color(_) => return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec4),
                LiveValue::Vec2(_) => return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec2),
                LiveValue::Vec3(_) => return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec3),
                LiveValue::DSL {token_start, ..} => {
                    // lets get the first token
                    let origin_doc = live_registry.token_id_to_origin_doc(node.token_id.unwrap());
                    match origin_doc.tokens[token_start as usize].token{
                        Token::Ident(id!(fn))=>{
                            if let Some(struct_ptr) = struct_ptr{
                                return LiveNodeFindResult::PossibleStatic(StructPtr(struct_ptr),FnPtr(now_ptr));
                            }
                            return LiveNodeFindResult::Function(FnPtr(now_ptr));
                        }
                        Token::Ident(id!(const))=>{
                            return LiveNodeFindResult::Const(ConstPtr(now_ptr));
                        }
                        _=>LiveNodeFindResult::NotFound
                    }
                }
                LiveValue::Class(_)=>{
                    if ids.len() == 0{
                        return LiveNodeFindResult::Component(now_ptr);
                    }
                    match nodes.child_by_name(index, ids[0]){
                        Ok(child_index)=>{
                            return walk_recur(live_registry, None, file_id, child_index, nodes, &ids[1..])
                        }
                        Err(_)=>{
                            return LiveNodeFindResult::NotFound;
                        }
                    }
                }
                LiveValue::Clone(clone)=>{
                    if ids.len() == 0{
                        if clone == id!(Struct){
                            return LiveNodeFindResult::Struct(StructPtr(now_ptr));
                        }
                        return LiveNodeFindResult::Component(now_ptr);
                    }
                    match nodes.child_by_name(index, ids[0]){
                        Ok(child_index)=>{
                            let struct_ptr = if clone == id!(Struct){
                                Some(now_ptr)
                            }
                            else{
                                None
                            };
                            return walk_recur(live_registry, struct_ptr, file_id, child_index, nodes, &ids[1..])
                        }
                        Err(_)=>{
                            return LiveNodeFindResult::NotFound;
                        }
                    }
                }
                LiveValue::Object => { 
                    if ids.len() == 0{
                        return LiveNodeFindResult::NotFound;
                    }
                    match nodes.child_by_name(index, ids[0]){
                        Ok(child_index)=>{
                            return walk_recur(live_registry, None, file_id, child_index, nodes, &ids[1..])
                        }
                        Err(_)=>{
                            return LiveNodeFindResult::NotFound;
                        }
                    }
                }
                _=>{
                    return LiveNodeFindResult::NotFound;
                }
            }
        }
    }
    
    pub fn analyse_deps(&mut self, live_registry:&LiveRegistry, deps: &[ShaderParserDep]) -> Result<(), LiveError> {
        // recur on used types
        for dep in deps {
            match dep {
                ShaderParserDep::Const(dep) => {
                    self.analyse_const(live_registry, *dep) ?;
                },
                ShaderParserDep::Struct(dep) => {
                    self.analyse_struct(live_registry, *dep) ?;
                },
                ShaderParserDep::Function(struct_ptr, fn_ptr) => {
                    self.analyse_plain_fn(live_registry, *struct_ptr, *fn_ptr) ?
                }
            }
        }
        Ok(())
    }
    
    // lets compile the thing
    pub fn analyse_const(&mut self, live_registry:&LiveRegistry, const_ptr: ConstPtr) -> Result<(), LiveError> {
        if self.consts.get(&const_ptr).is_some() {
            return Ok(());
        }
        let (doc, const_node) = live_registry.ptr_to_doc_node(const_ptr.0);
        match const_node.value {
            LiveValue::DSL {token_start, token_count, scope_start, scope_count} => {
                let mut parser_deps = Vec::new();
                let id = const_node.id;
                let origin_doc = &live_registry.token_id_to_origin_doc(const_node.token_id.unwrap());
                let mut parser = ShaderParser::new(
                    live_registry,
                    self,
                    origin_doc.get_tokens(token_start as usize, token_count as usize),
                    &doc.scopes[scope_start as usize..(scope_start as usize+scope_count as usize)],
                    &mut parser_deps,
                    None,
                    const_ptr.0.file_id
                    //Some(struct_full_ptr)
                );
                
                let const_decl = parser.expect_const_def(Ident(id)) ?;
                self.consts.insert(const_ptr, const_decl);
                
                self.analyse_deps(live_registry, &parser_deps) ?;
                
                let mut ca = ConstAnalyser {
                    live_registry,
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
    pub fn analyse_plain_fn(&mut self, live_registry:&LiveRegistry, struct_ptr: Option<StructPtr>, fn_ptr: FnPtr) -> Result<(), LiveError> {
        
        if self.all_fns.get(&fn_ptr).is_some() {
            return Ok(());
        }
        // alright lets parse and analyse a plain fn
        let (doc, fn_node) = live_registry.ptr_to_doc_node(fn_ptr.0);
        match fn_node.value {
            LiveValue::DSL {token_start, token_count, scope_start, scope_count} => {
                let id = fn_node.id;
                let mut parser_deps = Vec::new();
                // lets parse this thing
                let origin_doc = &live_registry.token_id_to_origin_doc(fn_node.token_id.unwrap());

                let parser = ShaderParser::new(
                    live_registry, 
                    self,
                    origin_doc.get_tokens(token_start as usize, token_count as usize),
                    &doc.scopes[scope_start as usize..(scope_start as usize+scope_count as usize)],
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
                
                self.analyse_deps(live_registry, &parser_deps) ?;
                
                // ok analyse the struct methods now.
                let mut fa = FnDefAnalyser {
                    live_registry,
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
    pub fn analyse_struct(&mut self, live_registry:&LiveRegistry, struct_ptr: StructPtr) -> Result<(), LiveError> {
        
        if self.structs.get(&struct_ptr).is_some() {
            return Ok(());
        }
        
        let (doc, struct_node) = live_registry.ptr_to_doc_node(struct_ptr.0);

        match struct_node.value {
            LiveValue::Clone(clone) => {
                if clone != id!(Struct){
                    panic!()
                }
                let mut struct_def = StructDef {
                    span: live_registry.token_id_to_span(struct_node.token_id.unwrap()),
                    struct_refs: RefCell::new(None),
                    fields: Vec::new(),
                    methods: Vec::new()
                };
                
                let mut parser_deps = Vec::new();
                
                // ok how do we iterate children of this node
                let mut node_iter = doc.nodes.first_child(struct_ptr.node_index());
                while let Some(node_index) = node_iter{
                    let prop_ptr = struct_ptr.with_index(node_index);
                    let prop = &doc.nodes[node_index];

                    match prop.value {
                        LiveValue::DSL {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id;
                            let origin_doc = &live_registry.token_id_to_origin_doc(prop.token_id.unwrap());
                            let scopes_doc = live_registry.token_id_to_expanded_doc(prop.token_id.unwrap());

                            let mut parser = ShaderParser::new(
                                live_registry,
                                self,
                                origin_doc.get_tokens(token_start as usize, token_count as usize),
                                &scopes_doc.scopes[scope_start as usize..scope_start as usize+scope_count as usize],
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr)),
                                struct_ptr.0.file_id
                                //Some(struct_full_ptr)
                            );
                            match origin_doc.tokens[token_start as usize].token{
                                Token::Ident(id!(fn))=>{
                                    
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
                                _=>{
                                    let def = parser.expect_field(Ident(id), VarDefPtr(prop_ptr)) ?;
                                    if let Some(def) = def {
                                        struct_def.fields.push(def);
                                    }
                                }
                            }
                        },
                        _ => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: live_registry.token_id_to_span(prop.token_id.unwrap()),
                                message: format!("Cannot use {:?} in struct", prop.value)
                            })
                        }
                    }
                    node_iter = doc.nodes.next_child(node_index);
                }
                // we should store the structs
                self.structs.insert(struct_ptr, struct_def);
                
                self.analyse_deps(live_registry, &parser_deps) ?;
                
                // ok analyse the struct methods now.
                let mut sa = StructAnalyser {
                    live_registry,
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
    pub fn analyse_draw_shader<F>(&mut self, live_registry:&LiveRegistry, draw_shader_ptr: DrawShaderPtr, mut ext_self: F) -> Result<&DrawShaderDef,
    LiveError>
    where F: FnMut(&LiveRegistry, &ShaderRegistry, Span, DrawShaderQuery, LiveType, &mut DrawShaderDef)
    {
        let mut draw_shader_def = DrawShaderDef::default();
        
        // lets insert the 2D drawshader uniforms
        draw_shader_def.add_uniform(id_from_str!(camera_projection).unwrap(), id_from_str!(pass).unwrap(), Ty::Mat4, Span::default());
        draw_shader_def.add_uniform(id_from_str!(camera_view).unwrap(), id_from_str!(pass).unwrap(), Ty::Mat4, Span::default());
        draw_shader_def.add_uniform(id_from_str!(camera_inv).unwrap(), id_from_str!(pass).unwrap(), Ty::Mat4, Span::default());
        draw_shader_def.add_uniform(id_from_str!(dpi_factor).unwrap(), id_from_str!(pass).unwrap(), Ty::Float, Span::default());
        draw_shader_def.add_uniform(id_from_str!(dpi_dilate).unwrap(), id_from_str!(pass).unwrap(), Ty::Float, Span::default());
        draw_shader_def.add_uniform(id_from_str!(view_transform).unwrap(), id_from_str!(view).unwrap(), Ty::Mat4, Span::default());
        draw_shader_def.add_uniform(id_from_str!(draw_clip).unwrap(), id_from_str!(draw).unwrap(), Ty::Vec4, Span::default());
        draw_shader_def.add_uniform(id_from_str!(draw_scroll).unwrap(), id_from_str!(draw).unwrap(), Ty::Vec4, Span::default());
        draw_shader_def.add_uniform(id_from_str!(draw_zbias).unwrap(), id_from_str!(draw).unwrap(), Ty::Float, Span::default());

        let (doc, class_node) = live_registry.ptr_to_doc_node(draw_shader_ptr.0);

        match class_node.value {
            LiveValue::Class(draw_shader_type) => {
                
                 ext_self(
                     live_registry,
                     self,
                     live_registry.token_id_to_span(class_node.token_id.unwrap()),
                     DrawShaderQuery::DrawShader,
                     draw_shader_type,
                     &mut draw_shader_def
                );
                
                let mut parser_deps = Vec::new();
                
                let mut node_iter = doc.nodes.first_child(draw_shader_ptr.node_index());
                let mut method_set = HashSet::new();
                while let Some(node_index) = node_iter{
                    let prop = &doc.nodes[node_index];
                    let prop_ptr = draw_shader_ptr.with_index(node_index);
                    match prop.value {
                        // if we have a float or a vec2/3/4
                        // we should look to set a default value
                        LiveValue::Bool(val) => {
                            if prop.id == id!(debug) {
                                draw_shader_def.flags.debug = val;
                            }
                            if prop.id == id!(draw_call_compare) {
                                draw_shader_def.flags.draw_call_compare = val;
                            }
                            if prop.id == id!(draw_call_always) {
                                draw_shader_def.flags.draw_call_always = val;
                            }
                        }
                        LiveValue::Class(live_type) => {
                            if prop.id == id!(geometry){
                                ext_self(
                                    live_registry,
                                    self,
                                    live_registry.token_id_to_span(prop.token_id.unwrap()),
                                    DrawShaderQuery::Geometry,
                                    live_type,
                                    &mut draw_shader_def
                                );
                            }
                        }
                        LiveValue::DSL {token_start, token_count, scope_start, scope_count} => {
                            let origin_doc = live_registry.token_id_to_origin_doc(prop.token_id.unwrap());
                            let scopes_doc = live_registry.token_id_to_expanded_doc(prop.token_id.unwrap());
                            
                            let mut parser = ShaderParser::new(
                                live_registry,
                                self,
                                origin_doc.get_tokens(token_start as usize, token_count as usize),
                                &scopes_doc.scopes[scope_start as usize..(scope_start as usize+scope_count as usize)],
                                &mut parser_deps,
                                Some(FnSelfKind::DrawShader(draw_shader_ptr)),
                                prop.token_id.unwrap().file_id()
                                //None
                            );
                            
                            match origin_doc.tokens[token_start as usize].token{
                                Token::Ident(id!(fn))=>{
                                    let fn_def = parser.expect_method_def(
                                        FnPtr(prop_ptr),
                                        Ident(prop.id),
                                    ) ?;
                                    if let Some(fn_def) = fn_def {
                                        method_set.insert(prop.id);
                                        draw_shader_def.methods.push(fn_def.fn_ptr);
                                        self.all_fns.insert(fn_def.fn_ptr, fn_def);
                                    }
                                }
                                _=>{
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
                                }
                            }
                            
                        },
                        _ => ()
                    }
                    node_iter = doc.nodes.next_child(node_index);
                }
                // lets check for duplicate fields
                for i in 0..draw_shader_def.fields.len() {
                    for j in (i + 1)..draw_shader_def.fields.len() {
                        let field_a = &draw_shader_def.fields[i];
                        let field_b = &draw_shader_def.fields[j];
                        if field_a.ident == field_b.ident && !field_a.ident.0.is_empty(){
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: field_a.span,
                                message: format!("Field double declaration  {}", field_b.ident)
                            })
                        }
                    }
                }
                
                self.draw_shader_defs.insert(draw_shader_ptr, draw_shader_def);
                
                if !method_set.contains(&id!(vertex)){
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: live_registry.token_id_to_span(class_node.token_id.unwrap()),
                        message: format!("analyse_draw_shader missing vertex method")
                    })
                }
                
                if !method_set.contains(&id!(pixel)){
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: live_registry.token_id_to_span(class_node.token_id.unwrap()),
                        message: format!("analyse_draw_shader missing pixel method")
                    })
                }
                
                    
                self.analyse_deps(live_registry, &parser_deps) ?;
                
                let draw_shader_def = self.draw_shader_defs.get(&draw_shader_ptr).unwrap();
                let mut sa = DrawShaderAnalyser {
                    live_registry,
                    shader_registry: self,
                    draw_shader_def: draw_shader_def,
                    scopes: &mut Scopes::new(),
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
                span: live_registry.token_id_to_span(class_node.token_id.unwrap()),
                message: format!("analyse_draw_shader could not find shader class")
            })
        }
    }
    
}
