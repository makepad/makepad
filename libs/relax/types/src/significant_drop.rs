//! `list_significant_drop_tys` (rustc_ty_utils needs_drop.rs, exhaustive) and
//! `extract_component_with_significant_dtor` (rustc_middle ty/significant_drop_order.rs): the types
//! with a significant destructor that dropping a value of a type drops.
use crate::program::Program;
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{TyId, TyKind};

pub struct SigDrop<'a> {
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
    pub env: TypingEnv,
}

use crate::lint_late2::has_def_attr;

impl<'a> SigDrop<'a> {
    fn kind(&self, t: TyId) -> TyKind { *self.tcx.tys.kind(t) }

    fn norm(&self, t: TyId) -> TyId { self.tcx.try_normalize_erasing_regions(self.prog, self.env, t).unwrap_or(t) }

    /// `needs_drop_components`: Err = always requires drop.
    fn needs_drop_components(&self, t: TyId) -> Result<Vec<TyId>, ()> {
        match self.kind(t) {
            TyKind::Bool | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never | TyKind::FnDef(..) | TyKind::FnPtr(..) | TyKind::Char
            | TyKind::Ptr(..) | TyKind::Ref(..) | TyKind::Str | TyKind::Foreign(_) => Ok(Vec::new()),
            TyKind::Dyn(..) | TyKind::Error => Err(()),
            TyKind::Slice(e) => self.needs_drop_components(e),
            TyKind::Array(e, len) => match self.needs_drop_components(e) {
                Ok(v) if v.is_empty() => Ok(v),
                res => match len.try_to_target_usize(&self.tcx.tys) {
                    Some(0) => Ok(Vec::new()),
                    Some(_) => res,
                    None => Ok(vec![t]),
                },
            },
            TyKind::Tuple(l) => {
                let mut acc = Vec::new();
                for &x in self.tcx.tys.list(l) { acc.extend(self.needs_drop_components(x)?); }
                Ok(acc)
            }
            _ => Ok(vec![t]),
        }
    }

    /// `list_significant_drop_tys` followed by `extract_component_raw`'s dedup: the types with a
    /// significant destructor `ty` drops.
    pub fn significant_drop_tys(&self, ty: TyId) -> Vec<TyId> {
        let ty = self.norm(ty);
        let mut out: Vec<TyId> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        seen.insert(ty);
        let mut stack = vec![(ty, 0u32)];
        'outer: while let Some((t, level)) = stack.pop() {
            if level > 128 { out.push(t); continue; }
            let comps = match self.needs_drop_components(t) { Err(()) => { out.push(t); continue } Ok(c) => c };
            for c in comps {
                match self.kind(c) {
                    TyKind::Closure(..) => {}
                    _ if self.tcx.is_copy_modulo_regions(self.prog, self.env, c) => {}
                    TyKind::Adt(d, args, _) => {
                        let Some(a) = self.tcx.adts.get(&d) else { continue };
                        let tys: Vec<TyId> = if a.is_manually_drop() {
                            Vec::new()
                        } else if has_def_attr(self.prog, d, "rustc_insignificant_dtor") {
                            self.tcx.tys.list(args).to_vec()
                        } else if self.tcx.adt_destructor(d).is_some() {
                            out.push(t);
                            continue 'outer;
                        } else if a.is_union {
                            Vec::new()
                        } else {
                            a.variants.iter().flat_map(|v| v.fields.iter()).map(|f| self.tcx.tys.subst(f.ty, self.tcx.tys.list(args))).collect()
                        };
                        for r in tys {
                            let r = self.norm(r);
                            if seen.insert(r) { stack.push((r, level + 1)); }
                        }
                    }
                    TyKind::Alias(..) | TyKind::Array(..) | TyKind::Placeholder(_) | TyKind::Param(_) => {
                        if t == c { out.push(c); continue 'outer; }
                        if seen.insert(c) { stack.push((c, level + 1)); }
                    }
                    TyKind::Foreign(_) | TyKind::Dyn(..) => { out.push(t); continue 'outer; }
                    _ => {}
                }
            }
        }
        let mut dedup = std::collections::HashSet::new();
        out.retain(|x| dedup.insert(*x));
        out
    }


    /// `true_significant_drop_ty`: well-known ecosystem types whose drop is trivial or delegated.
    fn true_significant_drop_ty(&self, ty: TyId) -> Option<Vec<TyId>> {
        let TyKind::Adt(def, args, _) = self.kind(ty) else { return None };
        let path = self.prog.def_path(def);
        let parts: Vec<&str> = path.split("::").collect();
        match parts.as_slice() {
            ["syn" | "proc_macro2", ..] | ["core" | "std", "task", "LocalWaker" | "Waker"] | ["core" | "std", "task", "wake", "LocalWaker" | "Waker"] => Some(Vec::new()),
            ["tracing", "instrument", "Instrumented"] | ["bytes", "Bytes"] => Some(Vec::new()),
            ["hashbrown", "raw", "RawTable" | "RawIntoIter"] => self.tcx.tys.list(args).first().map(|t| vec![*t]),
            ["hashbrown", "raw", "RawDrain"] => self.tcx.tys.list(args).get(1).map(|t| vec![*t]),
            _ => None,
        }
    }

    fn extract_component_raw(&self, ty: TyId, seen: &mut std::collections::HashSet<TyId>) -> Vec<TyId> {
        let ty = self.norm(ty);
        let mut out = Vec::new();
        for t in self.significant_drop_tys(ty) {
            if let Some(tys) = self.true_significant_drop_ty(t) {
                for t in tys {
                    if seen.insert(t) { out.extend(self.extract_component_raw(t, seen)); }
                }
            } else if seen.insert(t) {
                out.push(t);
            }
        }
        out
    }

    /// `extract_component_with_significant_dtor`.
    pub fn extract_component_with_significant_dtor(&self, ty: TyId) -> Vec<TyId> {
        let mut tys = self.extract_component_raw(ty, &mut std::collections::HashSet::new());
        let mut dedup = std::collections::HashSet::new();
        tys.retain(|t| dedup.insert(*t));
        tys
    }
}
