//! Interned types and their machine layout.

use crate::program::{DefId, Prim};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TyId(pub u32);

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Bool,
    Char,
    Str,
    Int(Prim),
    Float(Prim),
    Never,
    Tuple(Vec<TyId>),
    Array(TyId, u64),
    Slice(TyId),
    Ref(bool, TyId),
    Ptr(bool, TyId),
    Adt(DefId, Vec<TyId>),
    FnPtr(Vec<TyId>, TyId),
    FnDef(DefId, Vec<TyId>),
    /// generic parameter by index in the owner's generics list
    Param(u32),
    /// inference variable (only inside one body's type check)
    Infer(u32),
    /// a closure expression: (file, expr id, signature as FnPtr, captured state as a Tuple
    /// (by-reference captures are pointers), owner fn instance as FnDef). The owner names the
    /// type-checked body the closure's code is lowered from.
    Closure(u32, u32, TyId, TyId, TyId),
    /// associated type projection: (trait's assoc type def, [Self, trait args...])
    Assoc(DefId, Vec<TyId>),
    /// trait object `dyn Trait<Args, Assoc = T>`: (principal trait, its generic args without
    /// Self, associated type bindings). `dyn Fn(A) -> R` is (Fn, [fn(A) -> R], []).
    Dyn(DefId, Vec<TyId>, Vec<(DefId, TyId)>),
    Error,
}

pub struct Types {
    pub kinds: Vec<TyKind>,
    map: HashMap<TyKind, TyId>,
    pub unit: TyId,
    pub bool_: TyId,
    pub char_: TyId,
    pub str_: TyId,
    pub never: TyId,
    pub error: TyId,
    pub i32_: TyId,
    pub i64_: TyId,
    pub u8_: TyId,
    pub u32_: TyId,
    pub u64_: TyId,
    pub usize_: TyId,
    pub isize_: TyId,
    pub f32_: TyId,
    pub f64_: TyId,
    pub str_ref: TyId,
}

impl Types {
    pub fn new() -> Types {
        let mut t = Types {
            kinds: Vec::new(),
            map: HashMap::new(),
            unit: TyId(0),
            bool_: TyId(0),
            char_: TyId(0),
            str_: TyId(0),
            never: TyId(0),
            error: TyId(0),
            i32_: TyId(0),
            i64_: TyId(0),
            u8_: TyId(0),
            u32_: TyId(0),
            u64_: TyId(0),
            usize_: TyId(0),
            isize_: TyId(0),
            f32_: TyId(0),
            f64_: TyId(0),
            str_ref: TyId(0),
        };
        t.unit = t.intern(TyKind::Tuple(Vec::new()));
        t.bool_ = t.intern(TyKind::Bool);
        t.char_ = t.intern(TyKind::Char);
        t.str_ = t.intern(TyKind::Str);
        t.never = t.intern(TyKind::Never);
        t.error = t.intern(TyKind::Error);
        t.i32_ = t.intern(TyKind::Int(Prim::I32));
        t.i64_ = t.intern(TyKind::Int(Prim::I64));
        t.u8_ = t.intern(TyKind::Int(Prim::U8));
        t.u32_ = t.intern(TyKind::Int(Prim::U32));
        t.u64_ = t.intern(TyKind::Int(Prim::U64));
        t.usize_ = t.intern(TyKind::Int(Prim::Usize));
        t.isize_ = t.intern(TyKind::Int(Prim::Isize));
        t.f32_ = t.intern(TyKind::Float(Prim::F32));
        t.f64_ = t.intern(TyKind::Float(Prim::F64));
        t.str_ref = t.intern(TyKind::Ref(false, t.str_));
        t
    }

    pub fn intern(&mut self, k: TyKind) -> TyId {
        if let Some(&id) = self.map.get(&k) {
            return id;
        }
        let id = TyId(self.kinds.len() as u32);
        self.kinds.push(k.clone());
        self.map.insert(k, id);
        id
    }

    #[inline]
    pub fn kind(&self, t: TyId) -> &TyKind {
        &self.kinds[t.0 as usize]
    }

    pub fn prim(&mut self, p: Prim) -> TyId {
        match p {
            Prim::Bool => self.bool_,
            Prim::Char => self.char_,
            Prim::Str => self.str_,
            Prim::F32 | Prim::F64 => self.intern(TyKind::Float(p)),
            _ => self.intern(TyKind::Int(p)),
        }
    }

    pub fn is_int(&self, t: TyId) -> bool {
        matches!(self.kind(t), TyKind::Int(_))
    }
    pub fn is_float(&self, t: TyId) -> bool {
        matches!(self.kind(t), TyKind::Float(_))
    }

    /// Replaces `Param(i)` by `args[i]`.
    pub fn subst(&mut self, t: TyId, args: &[TyId]) -> TyId {
        if args.is_empty() {
            return t;
        }
        let k = self.kind(t).clone();
        match k {
            TyKind::Param(i) => {
                if (i as usize) < args.len() {
                    args[i as usize]
                } else {
                    t
                }
            }
            TyKind::Tuple(v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.subst(x, args));
                }
                self.intern(TyKind::Tuple(n))
            }
            TyKind::Array(e, len) => {
                let e2 = self.subst(e, args);
                self.intern(TyKind::Array(e2, len))
            }
            TyKind::Slice(e) => {
                let e2 = self.subst(e, args);
                self.intern(TyKind::Slice(e2))
            }
            TyKind::Ref(m, e) => {
                let e2 = self.subst(e, args);
                self.intern(TyKind::Ref(m, e2))
            }
            TyKind::Ptr(m, e) => {
                let e2 = self.subst(e, args);
                self.intern(TyKind::Ptr(m, e2))
            }
            TyKind::Adt(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.subst(x, args));
                }
                self.intern(TyKind::Adt(d, n))
            }
            TyKind::FnPtr(ps, r) => {
                let mut n = Vec::new();
                for x in ps {
                    n.push(self.subst(x, args));
                }
                let r2 = self.subst(r, args);
                self.intern(TyKind::FnPtr(n, r2))
            }
            TyKind::FnDef(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.subst(x, args));
                }
                self.intern(TyKind::FnDef(d, n))
            }
            TyKind::Assoc(d, v) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.subst(x, args));
                }
                self.intern(TyKind::Assoc(d, n))
            }
            TyKind::Dyn(d, v, bs) => {
                let mut n = Vec::new();
                for x in v {
                    n.push(self.subst(x, args));
                }
                let mut nb = Vec::new();
                for (a, x) in bs {
                    nb.push((a, self.subst(x, args)));
                }
                self.intern(TyKind::Dyn(d, n, nb))
            }
            TyKind::Closure(f, e, sig, up, owner) => {
                let sig = self.subst(sig, args);
                let up = self.subst(up, args);
                let owner = self.subst(owner, args);
                self.intern(TyKind::Closure(f, e, sig, up, owner))
            }
            _ => t,
        }
    }

    pub fn prim_name(p: Prim) -> &'static str {
        for (n, q) in crate::program::PRIMS {
            if q == p {
                return n;
            }
        }
        "?"
    }

    /// Integer width in bytes and signedness.
    pub fn int_info(p: Prim) -> (u32, bool) {
        match p {
            Prim::I8 => (1, true),
            Prim::I16 => (2, true),
            Prim::I32 => (4, true),
            Prim::I64 | Prim::Isize => (8, true),
            Prim::I128 => (16, true),
            Prim::U8 => (1, false),
            Prim::U16 => (2, false),
            Prim::U32 => (4, false),
            Prim::U64 | Prim::Usize => (8, false),
            Prim::U128 => (16, false),
            _ => (0, false),
        }
    }
}

impl Types {
    /// Does the type contain generic params or inference variables?
    pub fn is_concrete(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Param(_) | TyKind::Infer(_) | TyKind::Error => false,
            TyKind::Tuple(v) | TyKind::Adt(_, v) | TyKind::FnDef(_, v) | TyKind::Assoc(_, v) => {
                for x in v {
                    if !self.is_concrete(*x) {
                        return false;
                    }
                }
                true
            }
            TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e) | TyKind::Ptr(_, e) => self.is_concrete(*e),
            TyKind::Closure(_, _, sig, up, _) => self.is_concrete(*sig) && self.is_concrete(*up),
            TyKind::Dyn(_, v, bs) => v.iter().all(|x| self.is_concrete(*x)) && bs.iter().all(|x| self.is_concrete(x.1)),
            TyKind::FnPtr(ps, r) => {
                for x in ps {
                    if !self.is_concrete(*x) {
                        return false;
                    }
                }
                self.is_concrete(*r)
            }
            _ => true,
        }
    }
}
