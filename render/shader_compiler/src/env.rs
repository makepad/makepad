use makepad_live_parser::LiveError;
use crate::shaderregistry::ShaderRegistry;
use makepad_live_parser::Span;
use crate::ident::IdentPath;
use crate::ty::Ty;
use std::collections::hash_map::Entry;
use std::collections::HashMap;

type Scope = HashMap<IdentPath, Sym>;

#[derive(Clone, Debug)]
pub struct Env<'a> {
    scopes: Vec<Scope>,
    shader_registry: &'a ShaderRegistry
}

impl<'a> Env<'a> {
    pub fn new(shader_registry : &'a ShaderRegistry) -> Env {
        Env { scopes: Vec::new(), shader_registry }
    }

    pub fn find_sym(&self, _ident_path: IdentPath, _span:Span) -> Option<Sym> {
        /*
        let ret = self.scopes.iter().rev().find_map(|scope| scope.get(&ident_path));
        if ret.is_some(){
            return Some(ret.unwrap().clone())
        }
        // lets look up ident_path in our live_styles
        // we support color and float lookups, and soon animation lookups too.
        let live_item_id = ident_path.qualify(&self.live_styles.live_bodies[span.live_body_id.0].module_path).to_live_item_id();
        if let Some(_) = self.live_styles.vec4s.get(&live_item_id){
            return Some(Sym::Var{
                is_mut: false,
                ty: Ty::Vec4,
                kind: VarKind::LiveStyle
            });
        }
        if let Some(_) = self.live_styles.floats.get(&live_item_id){
            return Some(Sym::Var{
                is_mut: false,
                ty: Ty::Float,
                kind: VarKind::LiveStyle
            });
        }*/
        return None
    }

   /* pub fn qualify_ident_path(&self, live_body_id:LiveBodyId, ident_path:IdentPath)->QualifiedIdentPath{
        ident_path.qualify(&self.live_styles.live_bodies[live_body_id.0].module_path)
    }*/

    pub fn push_scope(&mut self) {
        self.scopes.push(Scope::new())
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop().unwrap();
    }

    pub fn insert_sym(&mut self, span: Span, ident_path: IdentPath, sym: Sym) -> Result<(), LiveError> {
        match self.scopes.last_mut().unwrap().entry(ident_path) {
            Entry::Vacant(entry) => {
                entry.insert(sym);
                Ok(())
            }
            Entry::Occupied(_) => Err(LiveError {
                span,
                message: format!("`{}` is already defined in this scope", ident_path),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Sym {
    Builtin,
    Fn,
    TyVar { ty: Ty },
    Var { is_mut: bool, ty: Ty, kind: VarKind },
}

#[derive(Clone, Copy, Debug)]
pub enum VarKind {
    Geometry,
    Const,
    Instance,
    Local,
    Texture,
    Uniform,
    Varying,
    LiveStyle
}

