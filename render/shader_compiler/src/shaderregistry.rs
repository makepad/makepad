#![allow(unused_variables)]
use makepad_live_parser::LiveRegistry;
use makepad_live_parser::Id;
use makepad_live_parser::IdUnpack;
use makepad_live_parser::LiveError;
use makepad_live_parser::LiveValue;
use makepad_live_parser::Span;
use makepad_live_parser::CrateModule;
use makepad_live_parser::IdPack;
use makepad_live_parser::id_pack;
use makepad_live_parser::id;
use makepad_live_parser::IdFmt;
use makepad_live_parser::FullNodePtr;
use crate::shaderast::ShaderAst;
use crate::shaderast::StructDecl;
use crate::shaderast::Decl;
use crate::shaderast::TextureDecl;
use crate::shaderast::InstanceDecl;
use crate::shaderast::UniformDecl;
use crate::shaderast::TyExpr;
use crate::shaderast::TyExprKind;
use crate::shaderast::TyLit;
use crate::shaderast::Ident;
use crate::shaderparser::ShaderParser;
use std::fmt;
use std::cell::Cell;
use std::collections::HashMap;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct ShaderResourceId(CrateModule, Id);

impl fmt::Display for ShaderResourceId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}::{}", self.0, self.1)
    }
}

#[derive(Default, Debug)]
pub struct ShaderRegistry {
    pub live_registry: LiveRegistry,
    pub structs: HashMap<FullNodePtr, StructDecl>,
    pub draw_inputs: HashMap<ShaderResourceId, ShaderDrawInput>,
}

#[derive(Clone, Debug, Default)]
pub struct ShaderDrawInput {
    pub uniforms: Vec<ShaderDrawInputDef>,
    pub instances: Vec<ShaderDrawInputDef>,
    pub textures: Vec<ShaderDrawInputDef>,
}

#[derive(Clone, Debug)]
pub struct ShaderDrawInputDef {
    pub ident: Ident,
    pub ty_expr: TyExpr
}

impl ShaderDrawInput {
    pub fn add_uniform(&mut self, name: &str, ty_expr: TyExpr) {
        if let TyExprKind::Lit {ty_lit, ..} = ty_expr.kind {
            if ty_lit == TyLit::Texture2D {
                self.textures.push(ShaderDrawInputDef {ident: Ident(Id::from_str(name)), ty_expr});
                return
            }
        }
        self.uniforms.push(ShaderDrawInputDef {ident: Ident(Id::from_str(name)), ty_expr});
    }
    
    pub fn add_instance(&mut self, modpath: &str, cls: &str, name: &str, ty_expr: TyExpr) {
        self.instances.push(ShaderDrawInputDef {ident: Ident(Id::from_str(name)), ty_expr});
    }
}

impl ShaderRegistry {
    pub fn register_draw_input(&mut self, mod_path: &str, name: &str, draw_input: ShaderDrawInput) {
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
            span: span,
            message: format!("Unsupported target naming {}", IdFmt::col(multi_ids, target))
        });
    }

    // lets compile the thing
    pub fn analyse_struct(&mut self, full_ptr: FullNodePtr) -> Result<(), LiveError> {
        
        if self.structs.get(&full_ptr).is_some(){
            return Ok(());
        }
        
        let doc = &self.live_registry.expanded[full_ptr.file_id.to_index()];
        let class_node = &doc.nodes[full_ptr.local_ptr.level][full_ptr.local_ptr.index];
        
        match class_node.value {
            LiveValue::Class {node_start, node_count, class} => {
                let mut struct_decl = StructDecl{
                    span:self.live_registry.token_id_to_span(class_node.token_id),
                    ident:Ident(class_node.id_pack.unwrap_single()),
                    fields: Vec::new(),
                    methods: Vec::new()
                };
                
                let mut type_deps = Vec::new();
                for i in 0..node_count as usize {
                    let prop = &doc.nodes[full_ptr.local_ptr.level + 1][i + node_start as usize];
                    match &prop.value {
                        LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id_pack.unwrap_single();
                            let mut parser = ShaderParser::new(
                                &doc.tokens[*token_start as usize..(token_start + token_count + 1)as usize],
                                &doc.scopes[*scope_start as usize..(*scope_start + *scope_count as u32) as usize],
                                &mut type_deps,
                                Some((struct_decl.ident, full_ptr))
                            );
                            // we only allow a field def
                            let decl = parser.expect_field(Ident(id)) ?;
                            struct_decl.fields.push(decl);
                        },
                        LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                            let id = prop.id_pack.unwrap_single();
                            // lets parse this thing
                            let mut parser = ShaderParser::new(
                                &doc.tokens[*token_start as usize..(token_start + token_count + 1)as usize],
                                &doc.scopes[*scope_start as usize..(*scope_start + *scope_count as u32) as usize],
                                &mut type_deps,
                                Some((struct_decl.ident, full_ptr))
                            );
                            
                            let decl = parser.expect_fn_decl(Ident(id)) ?;
                            struct_decl.methods.push(decl);
                        }
                        _ => {
                            return Err(LiveError {
                                span: self.live_registry.token_id_to_span(prop.token_id),
                                message: format!("Cannot use {:?} in struct", prop.value)
                            })                            
                        }
                    }
                }
                // recur on used types
                for dep in type_deps{
                    if dep != full_ptr{
                        self.analyse_struct(dep)?;
                    }
                }
                // ok we have all structs
                // now we can run the analyser on our struct.
                
            }
            _ => ()
        }
        
        Ok(())
    }
    
    // lets compile the thing
    pub fn analyse_draw_shader(&mut self, crate_id: Id, module_id: Id, ids: &[Id]) -> Result<ShaderAst, LiveError> {
        // lets find the FullPointer
        
        if let Some(full_ptr) = self.live_registry.find_full_node_ptr(crate_id, module_id, ids) {
            let mut shast = ShaderAst::default();
            // we have a pointer to the thing to instance.
            let doc = &self.live_registry.expanded[full_ptr.file_id.to_index()];
            let class_node = &doc.nodes[full_ptr.local_ptr.level][full_ptr.local_ptr.index];
            
            match class_node.value {
                LiveValue::Class {node_start, node_count, ..} => {
                    let mut type_deps = Vec::new();
                    let mut draw_input_srid = None;
                    for i in 0..node_count as usize {
                        let prop = &doc.nodes[full_ptr.local_ptr.level + 1][i + node_start as usize];
                        match &prop.value {
                            LiveValue::ResourceRef {target} => {
                                // draw input or default_geometry
                                match prop.id_pack {
                                    id_pack!(draw_input) => {
                                        let srid = Self::parse_shader_resource_id_from_multi_id(
                                            crate_id,
                                            module_id,
                                            self.live_registry.token_id_to_span(prop.token_id),
                                            *target,
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
                                            *target,
                                            &doc.multi_ids
                                        ) ?;
                                        shast.default_geometry = Some(srid);
                                    },
                                    _ => { // unknown
                                        return Err(LiveError {
                                            span: self.live_registry.token_id_to_span(prop.token_id),
                                            message: format!("Unknown var ref type {}", prop.id_pack)
                                        })
                                    }
                                }
                            }
                            LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                                if let IdUnpack::Single(id) = prop.id_pack.unpack() {
                                    let mut parser = ShaderParser::new(
                                        &doc.tokens[*token_start as usize..(token_start + token_count + 1)as usize],
                                        &doc.scopes[*scope_start as usize..(*scope_start + *scope_count as u32) as usize],
                                        &mut type_deps,
                                        None
                                    );
                                    let decl = parser.expect_other_decl(Ident(id)) ?;
                                    shast.decls.push(decl);
                                }
                            },
                            LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                                if let IdUnpack::Single(id) = prop.id_pack.unpack() {
                                    // lets parse this thing
                                    let mut parser = ShaderParser::new(
                                        &doc.tokens[*token_start as usize..(token_start + token_count + 1)as usize],
                                        &doc.scopes[*scope_start as usize..(*scope_start + *scope_count as u32) as usize],
                                        &mut type_deps,
                                        None
                                    );
                                    
                                    let decl = parser.expect_fn_decl(Ident(id)) ?;
                                    shast.decls.push(Decl::Fn(decl));
                                }
                            }
                            _ => ()
                        }
                    }
                    // if we have a draw_input process it.
                    if let Some((draw_input_srid, span)) = draw_input_srid {
                        if let Some(draw_input) = self.draw_inputs.get(&draw_input_srid) {
                            for decl in &shast.decls {
                                if let Decl::Instance(_) = decl {
                                    return Err(LiveError {
                                        span,
                                        message: format!("Cannot use both instance defs and draw_input {}", draw_input_srid)
                                    })
                                }
                            }
                            for instance in &draw_input.instances {
                                shast.decls.push(
                                    Decl::Instance(InstanceDecl {
                                        is_used_in_fragment_shader: Cell::new(None),
                                        span,
                                        ident: instance.ident,
                                        ty_expr: instance.ty_expr.clone(),
                                    })
                                )
                            }
                            
                            for uniform in &draw_input.uniforms {
                                shast.decls.push(
                                    Decl::Uniform(UniformDecl {
                                        block_ident: None,
                                        span,
                                        ident: uniform.ident,
                                        ty_expr: uniform.ty_expr.clone(),
                                    })
                                )
                            }
                            
                            for texture in &draw_input.textures {
                                shast.decls.push(
                                    Decl::Texture(TextureDecl {
                                        span,
                                        ident: texture.ident,
                                        ty_expr: texture.ty_expr.clone(),
                                    })
                                )
                            }
                        }
                        else {
                            return Err(LiveError {
                                span,
                                message: format!("Cannot find draw_input {}", draw_input_srid)
                            })
                        }
                        
                        
                    }
                    // do shit
                    for dep in type_deps{
                        self.analyse_struct(dep)?;
                    }
                    // ok we have all structs
                    return Ok(shast)
                }
                _ => ()
            }
        }
        return Err(LiveError {
            span: Span::default(),
            message: format!("analyse_draw_shader could not find {} {} {} ", crate_id, module_id, ids[0])
        })
    }
    
}
