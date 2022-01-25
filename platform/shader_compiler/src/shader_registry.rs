use {
    std::{
        collections::{BTreeMap, HashSet, HashMap},
        cell::{Cell, RefCell},
    },
    crate::{
        makepad_live_compiler::*,
        shader_ast::*,
        analyse::*,
        shader_parser::{ShaderParser, ShaderParserDep},
        builtin::{Builtin, generate_builtins},
    }
};


pub struct ShaderRegistry {
    pub all_fns: HashMap<FnPtr, FnDef>,
    pub draw_shader_defs: HashMap<DrawShaderPtr, DrawShaderDef>,
    pub structs: HashMap<StructPtr, StructDef>,
    pub builtins: HashMap<Ident, Builtin>,
    pub enums: HashMap<LiveType, ShaderEnum>,
}

pub struct ShaderEnum {
    pub enum_name: LiveId,
    pub variants: Vec<LiveId>
}

impl ShaderRegistry {
    pub fn new() -> Self {
        Self {
            structs: HashMap::new(),
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
    LiveValue(ValuePtr, TyLit),
    Error(LiveError)
}

pub enum DrawShaderQuery {
    DrawShader,
    Geometry,
}

impl ShaderRegistry {
    
    pub fn flush_registry(&mut self){
        self.all_fns.clear();
        self.draw_shader_defs.clear();
        self.structs.clear();
    }
    
    pub fn register_enum(&mut self, live_type: LiveType, shader_enum: ShaderEnum) {
        self.enums.insert(live_type, shader_enum);
    }
    
    pub fn compute_const_table(&self, draw_shader_ptr: DrawShaderPtr/*, filter_file_id: LiveFileId*/) -> DrawShaderConstTable {
        
        let draw_shader_def = self.draw_shader_defs.get(&draw_shader_ptr).unwrap();
        
        let mut offsets = BTreeMap::new();
        let mut table = Vec::new();
        let mut offset = 0;
        let mut table_index = BTreeMap::new();
        
        for callee in draw_shader_def.all_fns.borrow().iter() {
            let fn_decl = self.all_fns.get(callee).unwrap();
            //if fn_decl.span.file_id == filter_file_id {
            let sub_table = fn_decl.const_table.borrow();
            table.extend(sub_table.as_ref().unwrap().iter());
            
            for ct_span in fn_decl.const_table_spans.borrow().as_ref().unwrap().iter() {
                table_index.insert(
                    ct_span.token_id,
                    ConstTableItem {
                        offset: offset + ct_span.offset,
                        slots: ct_span.slots
                    }
                );
            }
            offsets.insert(*callee, offset);
            offset += sub_table.as_ref().unwrap().len();
        }
        
        let size = table.len();
        let align_gap = 4 - (size - ((size >> 2) << 2));
        for _ in 0..align_gap {
            table.push(0.0);
        }
        DrawShaderConstTable {
            table,
            offsets,
            table_index
        }
    }
    
    pub fn fn_ident_from_ptr(&self, live_registry: &LiveRegistry, fn_node_ptr: FnPtr) -> Ident {
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
    
    pub fn find_live_node_by_path(&self, live_registry: &LiveRegistry, base_ptr: LivePtr, ids: &[LiveId]) -> LiveNodeFindResult {
        
        let doc = &live_registry.ptr_to_doc(base_ptr);
        
        let ret = walk_recur(live_registry, None, base_ptr.file_id, base_ptr.generation, base_ptr.index as usize, &doc.nodes, ids);
        return ret;
        // ok so we got a node. great. now what
        fn walk_recur(live_registry: &LiveRegistry, struct_ptr: Option<LivePtr>, file_id: LiveFileId, generation:LiveFileGeneration, index: usize, nodes: &[LiveNode], ids: &[LiveId]) -> LiveNodeFindResult {
            let node = &nodes[index];
            
            if ids.len() != 0 && !node.value.is_class() && !node.value.is_clone() && !node.value.is_object() {
                return LiveNodeFindResult::NotFound;
            }
            
            let now_ptr = LivePtr {file_id, index: index as u32, generation};
            //let first_def = node.origin.first_def().unwrap();
            match node.value {
                LiveValue::Bool(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Bool)
                },
                LiveValue::Int(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Int)
                }
                LiveValue::Float(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Float)
                }
                LiveValue::Color(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec4)
                }
                LiveValue::Vec2(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec2)
                }
                LiveValue::Vec3(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec3)
                }
                LiveValue::Vec4(_) if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), TyLit::Vec4)
                }
                LiveValue::Expr{..} if live_registry.get_node_prefix(node.origin) == Some(id!(const)) => {
                    // ok lets eval the expr to get a type
                    match live_eval(live_registry, index, &mut (index + 1), nodes){
                        Ok(value) => {
                            if let Some(ty) = Ty::from_live_eval(value){
                                if let Some(ty_lit) = ty.maybe_ty_lit(){
                                    return LiveNodeFindResult::LiveValue(ValuePtr(now_ptr), ty_lit)
                                }
                            }
                            return LiveNodeFindResult::Error(
                                LiveError {
                                    origin: live_error_origin!(),
                                    message: format!("Type of eval result not valid for shader"),
                                    span: nodes[index].origin.token_id().unwrap().into()
                                }
                            );
                        }
                        Err(err)=>{
                            println!("HERE ERROR");
                            return LiveNodeFindResult::Error(err)
                        }
                    }
                }
                LiveValue::DSL {token_start, ..} => {
                    // lets get the first token
                    let origin_doc = live_registry.token_id_to_origin_doc(node.origin.token_id().unwrap());
                    match origin_doc.tokens[token_start as usize].token {
                        LiveToken::Ident(id!(fn)) => {
                            if let Some(struct_ptr) = struct_ptr {
                                return LiveNodeFindResult::PossibleStatic(StructPtr(struct_ptr), FnPtr(now_ptr));
                            }
                            return LiveNodeFindResult::Function(FnPtr(now_ptr));
                        }
                        _ => LiveNodeFindResult::NotFound
                    }
                }
                LiveValue::Class {..} => {
                    if ids.len() == 0 {
                        return LiveNodeFindResult::Component(now_ptr);
                    }
                    match nodes.child_by_name(index, ids[0]) {
                        Some(child_index) => {
                            return walk_recur(live_registry, None, file_id, generation, child_index, nodes, &ids[1..])
                        }
                        None => {
                            return LiveNodeFindResult::NotFound;
                        }
                    }
                }
                LiveValue::Clone(clone) => {
                    if ids.len() == 0 {
                        if clone == id!(Struct) {
                            return LiveNodeFindResult::Struct(StructPtr(now_ptr));
                        }
                        return LiveNodeFindResult::Component(now_ptr);
                    }
                    match nodes.child_by_name(index, ids[0]) {
                        Some(child_index) => {
                            let struct_ptr = if clone == id!(Struct) {
                                Some(now_ptr)
                            }
                            else {
                                None
                            };
                            return walk_recur(live_registry, struct_ptr, file_id,  generation, child_index, nodes, &ids[1..])
                        }
                        None => {
                            return LiveNodeFindResult::NotFound;
                        }
                    }
                }
                LiveValue::Object => {
                    if ids.len() == 0 {
                        return LiveNodeFindResult::NotFound;
                    }
                    match nodes.child_by_name(index, ids[0]) {
                        Some(child_index) => {
                            return walk_recur(live_registry, None, file_id,  generation,  child_index, nodes, &ids[1..])
                        }
                        None => {
                            return LiveNodeFindResult::NotFound;
                        }
                    }
                }
                _ => {
                    return LiveNodeFindResult::NotFound;
                }
            }
        }
    }
    
    pub fn analyse_deps(&mut self, live_registry: &LiveRegistry, deps: &[ShaderParserDep]) -> Result<(), LiveError> {
        // recur on used types
        for dep in deps {
            match dep {
                /*ShaderParserDep::Const(dep) => {
                    self.analyse_const(live_registry, *dep) ?;
                },*/
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
    pub fn analyse_plain_fn(&mut self, live_registry: &LiveRegistry, struct_ptr: Option<StructPtr>, fn_ptr: FnPtr) -> Result<(), LiveError> {
        
        if self.all_fns.get(&fn_ptr).is_some() {
            return Ok(());
        }
        // alright lets parse and analyse a plain fn
        let fn_node = live_registry.ptr_to_node(fn_ptr.0);
        match fn_node.value {
            LiveValue::DSL {token_start, token_count, expand_index} => {
                let id = fn_node.id;
                let mut parser_deps = Vec::new();
                // lets parse this thing
                let origin_doc = &live_registry.token_id_to_origin_doc(fn_node.origin.token_id().unwrap());
                
                let parser = ShaderParser::new(
                    live_registry,
                    self,
                    origin_doc.get_tokens(token_start as usize, token_count as usize),
                    &mut parser_deps,
                    if let Some(struct_ptr) = struct_ptr {Some(FnSelfKind::Struct(struct_ptr))}else {None},
                    expand_index.unwrap() as usize,
                    fn_node.origin.token_id().unwrap().file_id(),
                    token_start as usize
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
    pub fn analyse_struct(&mut self, live_registry: &LiveRegistry, struct_ptr: StructPtr) -> Result<(), LiveError> {
        
        if self.structs.get(&struct_ptr).is_some() {
            return Ok(());
        }
        
        let (doc, struct_node) = live_registry.ptr_to_doc_node(struct_ptr.0);
        
        match struct_node.value {
            LiveValue::Clone(clone) => {
                if clone != id!(Struct) {
                    panic!()
                }
                let mut struct_def = StructDef {
                    span: struct_node.origin.token_id().unwrap().into(),
                    struct_refs: RefCell::new(None),
                    fields: Vec::new(),
                    methods: Vec::new()
                };
                
                let mut parser_deps = Vec::new();
                
                // ok how do we iterate children of this node
                let mut node_iter = doc.nodes.first_child(struct_ptr.node_index());
                while let Some(node_index) = node_iter {
                    let prop_ptr = struct_ptr.with_index(node_index);
                    let prop = &doc.nodes[node_index];
                    
                    match prop.value {
                        LiveValue::DSL {token_start, token_count, expand_index} => {
                            let id = prop.id;
                            let origin_doc = &live_registry.token_id_to_origin_doc(prop.origin.token_id().unwrap());
                            
                            let parser = ShaderParser::new(
                                live_registry,
                                self,
                                origin_doc.get_tokens(token_start as usize, token_count as usize),
                                &mut parser_deps,
                                Some(FnSelfKind::Struct(struct_ptr)),
                                expand_index.unwrap() as usize,
                                prop.origin.token_id().unwrap().file_id(),
                                token_start as usize
                                //Some(struct_full_ptr)
                            );
                            match origin_doc.tokens[token_start as usize].token {
                                LiveToken::Ident(id!(fn)) => {
                                    
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
                                        span: prop.origin.token_id().unwrap().into(),
                                        message: format!("Unexpected DSL node")
                                    })
                                    /*
                                    let def = parser.expect_field(Ident(id), VarDefPtr(prop_ptr)) ?;
                                    if let Some(def) = def {
                                        struct_def.fields.push(def);
                                    }*/
                                }
                            }
                        },
                        LiveValue::Id(type_name) if live_registry.get_node_prefix(prop.origin) == Some(id!(field)) => {
                            // lets fetch the span
                            
                            if let Some(ty_lit) = TyLit::from_id(type_name) {
                                struct_def.fields.push(StructFieldDef {
                                    var_def_ptr: VarDefPtr(prop_ptr),
                                    span:prop.origin.token_id().unwrap().into(),
                                    ident: Ident(prop.id),
                                    ty_expr: ty_lit.to_ty().to_ty_expr()
                                })
                            }
                            else {
                                // TODO support structs as fields here
                                return Err(LiveError {
                                    origin: live_error_origin!(),
                                    span: prop.origin.token_id().unwrap().into(),
                                    message: format!("Type not found for struct field {}", type_name)
                                })
                            }
                        },
                        _ => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: prop.origin.token_id().unwrap().into(),
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
    pub fn analyse_draw_shader<F>(&mut self, live_registry: &LiveRegistry, draw_shader_ptr: DrawShaderPtr, mut ext_self: F) -> Result<(),
        LiveError>
    where F: FnMut(&LiveRegistry, &ShaderRegistry, TokenSpan, DrawShaderQuery, LiveType, &mut DrawShaderDef)
    {
        let mut draw_shader_def = DrawShaderDef::default();
        
        // lets insert the 2D drawshader uniforms
        draw_shader_def.add_uniform(id_from_str!(camera_projection).unwrap(), id_from_str!(pass).unwrap(), Ty::Mat4, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(camera_view).unwrap(), id_from_str!(pass).unwrap(), Ty::Mat4, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(camera_inv).unwrap(), id_from_str!(pass).unwrap(), Ty::Mat4, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(dpi_factor).unwrap(), id_from_str!(pass).unwrap(), Ty::Float, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(dpi_dilate).unwrap(), id_from_str!(pass).unwrap(), Ty::Float, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(view_transform).unwrap(), id_from_str!(view).unwrap(), Ty::Mat4, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(draw_clip).unwrap(), id_from_str!(draw).unwrap(), Ty::Vec4, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(draw_scroll).unwrap(), id_from_str!(draw).unwrap(), Ty::Vec4, TokenSpan::default());
        draw_shader_def.add_uniform(id_from_str!(draw_zbias).unwrap(), id_from_str!(draw).unwrap(), Ty::Float, TokenSpan::default());
        
        let (doc, class_node) = live_registry.ptr_to_doc_node(draw_shader_ptr.0);
        
        match class_node.value {
            LiveValue::Class {live_type, ..} => {
                
                ext_self(
                    live_registry,
                    self,
                    class_node.origin.token_id().unwrap().into(),
                    DrawShaderQuery::DrawShader,
                    live_type,
                    &mut draw_shader_def
                );
                
                let mut parser_deps = Vec::new();
                
                let mut node_iter = doc.nodes.first_child(draw_shader_ptr.node_index());
                let mut method_set = HashSet::new();
                while let Some(node_index) = node_iter {
                    let prop = &doc.nodes[node_index];
                    let prop_ptr = draw_shader_ptr.with_index(node_index);
                    if prop.id == id!(debug_id){
                        node_iter = doc.nodes.next_child(node_index);
                        continue;
                    }
                    match prop.value {
                        LiveValue::Bool(_) |
                        LiveValue::Id(_) |
                        LiveValue::Int(_) |
                        LiveValue::Float(_) |
                        LiveValue::Color(_) |
                        LiveValue::Vec2(_) |
                        LiveValue::Vec3(_) |
                        LiveValue::Vec4(_) | 
                        LiveValue::Expr{..} => {
                            
                            
                            let first_def = prop.origin.first_def().unwrap();
                            let before = live_registry.get_node_prefix(prop.origin);
                           
                            let ty = match ShaderTy::from_live_node(live_registry, node_index, &doc.nodes){
                                Ok(ty)=>ty,
                                Err(err)=>{
                                    return Err(err)
                                }
                            };
                            let ty_expr = ty.to_ty_expr();
                            match before {
                                Some(id!(geometry)) => {
                                    draw_shader_def.fields.push(DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Geometry {
                                            is_used_in_pixel_shader: Cell::new(false),
                                            var_def_ptr: Some(VarDefPtr(prop_ptr)),
                                        },
                                        span: first_def.into(),
                                        ident: Ident(prop.id),
                                        ty_expr
                                    });
                                },
                                Some(id!(instance)) => {
                                    let decl = DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Instance {
                                            is_used_in_pixel_shader: Cell::new(false),
                                            live_field_kind: LiveFieldKind::Live,
                                            var_def_ptr: Some(VarDefPtr(prop_ptr)),
                                        },
                                        span: first_def.into(),
                                        ident: Ident(prop.id),
                                        ty_expr
                                    };
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
                                    
                                },
                                Some(id!(uniform)) => {
                                    draw_shader_def.fields.push(DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Uniform {
                                            var_def_ptr: Some(VarDefPtr(prop_ptr)),
                                            block_ident: Ident(id!(user)),
                                        },
                                        span: first_def.into(),
                                        ident: Ident(prop.id),
                                        ty_expr
                                    });
                                },
                                Some(id!(varying)) => {
                                    draw_shader_def.fields.push(DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Varying {
                                            var_def_ptr: VarDefPtr(prop_ptr),
                                        },
                                        span: first_def.into(),
                                        ident: Ident(prop.id),
                                        ty_expr
                                    });
                                },
                                Some(id!(texture)) => {
                                    draw_shader_def.fields.push(DrawShaderFieldDef {
                                        kind: DrawShaderFieldKind::Texture {
                                            var_def_ptr: Some(VarDefPtr(prop_ptr)),
                                        },
                                        span: first_def.into(),
                                        ident: Ident(prop.id),
                                        ty_expr
                                    });
                                }
                                Some(id!(const)) => {
                                },
                                None => {
                                    if let LiveValue::Bool(val) = prop.value {
                                        match prop.id {
                                            id!(debug) => {
                                                draw_shader_def.flags.debug = val;
                                            }
                                            id!(draw_call_compare) => {
                                                draw_shader_def.flags.draw_call_nocompare = val;
                                            }
                                            id!(draw_call_always) => {
                                                draw_shader_def.flags.draw_call_always = val;
                                            }
                                            _ => {} // could be input value
                                        }
                                    }
                                },
                                _ => {
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span: first_def.into(),
                                        message: format!("Unexpected variable prefix {:?}", before)
                                    })
                                }
                            };
                        }
                        LiveValue::Class {live_type, ..} => {
                            if prop.id == id!(geometry) {
                                ext_self(
                                    live_registry,
                                    self,
                                    prop.origin.token_id().unwrap().into(),
                                    DrawShaderQuery::Geometry,
                                    live_type,
                                    &mut draw_shader_def
                                );
                            }
                        }
                        LiveValue::DSL {token_start, token_count, expand_index} => {
                            let origin_doc = live_registry.token_id_to_origin_doc(prop.origin.token_id().unwrap());
                            
                            let parser = ShaderParser::new(
                                live_registry,
                                self,
                                origin_doc.get_tokens(token_start as usize, token_count as usize),
                                &mut parser_deps,
                                Some(FnSelfKind::DrawShader(draw_shader_ptr)),
                                expand_index.unwrap() as usize,
                                prop.origin.token_id().unwrap().file_id(),
                                token_start as usize
                                //None
                            );
                            
                            let token = &origin_doc.tokens[token_start as usize];
                            match token.token {
                                LiveToken::Ident(id!(fn)) => {
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
                                _ => {
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span: token.span.into(),
                                        message: format!("Unexpected in shader body {}", token)
                                    })
                                    /*
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
                                    }*/
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
                        if field_a.ident == field_b.ident && !field_a.ident.0.is_empty() {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: field_a.span.into(),
                                message: format!("Field double declaration  {}", field_b.ident)
                            })
                        }
                    }
                }
                
                self.draw_shader_defs.insert(draw_shader_ptr, draw_shader_def);
                
                if !method_set.contains(&id!(vertex)) {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: class_node.origin.token_id().unwrap().into(),
                        message: format!("analyse_draw_shader missing vertex method")
                    })
                }
                
                if !method_set.contains(&id!(pixel)) {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: class_node.origin.token_id().unwrap().into(),
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
                return Ok(())
            }
            _ => return Err(LiveError {
                origin: live_error_origin!(),
                span: class_node.origin.token_id().unwrap().into(),
                message: format!("analyse_draw_shader could not find shader class")
            })
        }
    }
    
}
