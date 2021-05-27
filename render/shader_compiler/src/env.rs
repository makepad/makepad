use makepad_live_parser::LiveError;
use crate::shaderregistry::ShaderRegistry;
use makepad_live_parser::Span;
use makepad_live_parser::FullNodePtr;
//use makepad_live_parser::LiveValue;
use makepad_live_parser::LiveErrorOrigin;
use makepad_live_parser::live_error_origin;
//use crate::shaderast::IdentPath;
use crate::shaderast::Ty;
use crate::shaderast::Ident;
use crate::shaderast::StructDecl;
use crate::shaderast::FnNodePtr;
use crate::shaderast::FnDecl;
use crate::shaderast::StructNodePtr;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::cell::RefCell;
use std::collections::BTreeSet;

type Scope = HashMap<Ident, LocalSym>;

#[derive(Clone, Debug)]
pub struct Env<'a> {
    pub const_table: RefCell<Option<Vec<f32 >> >,
    pub const_table_spans: RefCell<Option<Vec<(usize, Span) >> >,
    pub live_uniform_deps: RefCell<Option<BTreeSet<(Ty, FullNodePtr) >> >,
    pub scopes: Vec<Scope>,
    pub shader_registry: &'a ShaderRegistry
}

impl<'a> Env<'a> {
    pub fn new(shader_registry: &'a ShaderRegistry) -> Env {
        Env {
            const_table: RefCell::new(Some(Vec::new())),
            const_table_spans: RefCell::new(Some(Vec::new())),
            live_uniform_deps: RefCell::new(Some(BTreeSet::new())),
            scopes: Vec::new(),
            shader_registry
        }
    }
    /*
    pub fn find_fn_decl(&self, ident_path: IdentPath) -> Option<&FnDecl> {
        return None
        /*
        self.decls.iter().rev().find_map( | decl | {
            match decl {
                Decl::Fn(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident_path == ident_path)
        })*/
    }*/
    /*
    pub fn find_const_decl(&self, _ident: Ident, _scope_node_ptr: ScopeNodePtr) -> Option<&ConstDecl> {
        return None
        /*
        self.decls.iter().find_map( | decl | {
            match decl {
                Decl::Const(decl) => Some(decl),
                _ => None,
            }
            .filter( | decl | decl.ident == ident)
        })
        */
    }*/
    /*
    pub fn find_struct_ptr(&self, ident: Ident, scope_node_ptr: ScopeNodePtr) -> Option<StructNodePtr> {
        // ok we have to find MyStruct on the scope of _fn_node_ptr.
        let (doc,fn_node) = self.shader_registry.live_registry.resolve_ptr(scope_node_ptr.0);
        // ok so what if the struct we had to find is our Self.
        
        // ok lets look in our scopes
        match fn_node.value {
            LiveValue::Fn {scope_start, scope_count, ..} | LiveValue::VarDef {scope_start, scope_count, ..} => {
                for i in (0..scope_count).rev() {
                    let item = &doc.scopes[scope_start as usize + i as usize];
                    if item.id == ident.0{
                        let struct_ptr = StructNodePtr(item.target.to_full_node_ptr(scope_node_ptr.0.file_id));
                        return Some(struct_ptr);
                    }
                }
                
            }
            
            _ => ()
        }
        return None
        //return None
    }
    */
    
    pub fn plain_fn_decl_from_ptr(&self, fn_node_ptr:FnNodePtr) -> Option<&FnDecl> {
        self.shader_registry.plain_fns.get(&fn_node_ptr)
    }
    
    pub fn struct_decl_from_ptr(&self, struct_node_ptr:StructNodePtr) -> Option<&StructDecl> {
        self.shader_registry.structs.get(&struct_node_ptr)
    }
    
    pub fn struct_method_from_ptr(&self, struct_node_ptr:StructNodePtr, ident:Ident) -> Option<&FnDecl>{
        if let Some(s) = self.shader_registry.structs.get(&struct_node_ptr){
            if let Some(node) = s.methods.iter().find(|fn_decl| fn_decl.ident == ident){
                return Some(node)
            }
        }
        None
    }
    
    pub fn fn_ident_from_ptr(&self, fn_node_ptr:FnNodePtr) -> Ident {
        let (_,node) = self.shader_registry.live_registry.resolve_ptr(fn_node_ptr.0);
        Ident(node.id_pack.unwrap_single())
    }
    
    pub fn find_sym_on_scopes(&self, ident: Ident, _span: Span,) -> Option<LocalSym> {
        
        let ret = self.scopes.iter().rev().find_map( | scope | scope.get(&ident));
        if ret.is_some() {
            return Some(ret.unwrap().clone())
        }
        return None
    }
    
    pub fn push_scope(&mut self) {
        self.scopes.push(Scope::new())
    }
    
    pub fn pop_scope(&mut self) {
        self.scopes.pop().unwrap();
    }
    
    pub fn insert_sym(&mut self, span: Span, ident: Ident, local_sym: LocalSym) -> Result<(), LiveError> {
        match self.scopes.last_mut().unwrap().entry(ident) {
            Entry::Vacant(entry) => {
                entry.insert(local_sym);
                Ok(())
            }
            Entry::Occupied(_) => Err(LiveError {
                origin:live_error_origin!(),
                span,
                message: format!("`{}` is already defined in this scope", ident),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LocalSym {
    pub is_mut: bool, 
    pub ty: Ty, 
}