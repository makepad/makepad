use crate::error::LiveError;
use crate::ident::{IdentPath, QualifiedIdentPath};
use crate::span::{Span,LiveBodyId};
use crate::ty::Ty;
use crate::livestyles::LiveStyles;
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Env<'a> {
    scopes: Vec<Scope>,
    live_styles: &'a LiveStyles
}

impl<'a> Env<'a> {
    pub fn new(live_styles : &'a LiveStyles) -> Env {
        Env { scopes: Vec::new(), live_styles }
    }

    pub fn find_sym(&self, ident_path: IdentPath, span:Span) -> Option<Sym> {
        let ret = self.scopes.iter().rev().find_map(|scope| scope.get(&ident_path));
        if ret.is_some(){
            return Some(ret.unwrap().clone())
        }
        // lets look up ident_path in our live_styles
        // we support color and float lookups, and soon animation lookups too.
        let live_id = ident_path.qualify(&self.live_styles.live_bodies[span.live_body_id.0].module_path).to_live_id();
        if let Some(_) = self.live_styles.base.colors.get(&live_id){
            return Some(Sym::Var{
                is_mut: false,
                ty: Ty::Vec4,
                kind: VarKind::LiveStyle
            });
        }
        if let Some(_) = self.live_styles.base.floats.get(&live_id){
            return Some(Sym::Var{
                is_mut: false,
                ty: Ty::Float,
                kind: VarKind::LiveStyle
            });
        }
        return None
    }

    pub fn qualify_ident_path(&self, live_body_id:LiveBodyId, ident_path:IdentPath)->QualifiedIdentPath{
        ident_path.qualify(&self.live_styles.live_bodies[live_body_id.0].module_path)
    }

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

type Scope = HashMap<IdentPath, Sym>;
