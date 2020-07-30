use crate::error::Error;
use crate::ident::Ident;
use crate::span::Span;
use crate::ty::Ty;
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Env {
    scopes: Vec<Scope>,
}

impl Env {
    pub fn new() -> Env {
        Env { scopes: Vec::new() }
    }

    pub fn find_sym(&self, ident: Ident) -> Option<&Sym> {
        self.scopes.iter().rev().find_map(|scope| scope.get(&ident))
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(Scope::new())
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop().unwrap();
    }

    pub fn insert_sym(&mut self, span: Span, ident: Ident, sym: Sym) -> Result<(), Error> {
        match self.scopes.last_mut().unwrap().entry(ident) {
            Entry::Vacant(entry) => {
                entry.insert(sym);
                Ok(())
            }
            Entry::Occupied(_) => Err(Error {
                span,
                message: format!("`{}` is already defined in this scope", ident),
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
}

type Scope = HashMap<Ident, Sym>;
