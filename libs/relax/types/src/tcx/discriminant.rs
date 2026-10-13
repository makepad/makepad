// ported from compiler/rustc_middle/src/ty/adt.rs @8d1a764
// ported from compiler/rustc_middle/src/ty/util.rs @8d1a764
// ported from compiler/rustc_hir_analysis/src/collect.rs @8d1a764
// ported from compiler/rustc_hir_analysis/src/check/check.rs @8d1a764
use super::{AdtT, Tcx, TypingEnv};
use crate::const_eval::{ErrorHandled, evaluation_error};
use crate::program::{DefId, Prim, Program};
use crate::types::Types;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantDiscr { Explicit(DefId), Relative(u32) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Discr { pub val: u128, pub ty: Prim }
impl Discr {
    pub fn signed_value(self) -> i128 {
        let (bytes, signed) = Types::int_info(self.ty);
        let shift = 128 - bytes * 8;
        if signed { ((self.val << shift) as i128) >> shift } else { self.val as i128 }
    }
    pub fn wrap_incr(self) -> Self { self.checked_add(1).0 }
    /// ty/util.rs:83-105: preserve the representation's signed overflow boundary.
    pub fn checked_add(self, n: u128) -> (Self, bool) {
        let (bytes, signed) = Types::int_info(self.ty);
        let bits = bytes * 8;
        let mask = u128::MAX >> (128 - bits);
        let (val, overflow) = if signed {
            let min = i128::MIN >> (128 - bits);
            let max = i128::MAX >> (128 - bits);
            let val = self.signed_value();
            assert!(n < i128::MAX as u128);
            let n = n as i128;
            let overflow = val > max - n;
            let val = if overflow { min + (n - (max - val) - 1) } else { val + n };
            (val as u128 & mask, overflow)
        } else {
            let overflow = self.val > mask - n;
            let val = if overflow { n - (mask - self.val) - 1 } else { self.val + n };
            (val, overflow)
        };
        (Self { val, ty: self.ty }, overflow)
    }
}
impl std::fmt::Display for Discr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if Types::int_info(self.ty).1 { write!(f, "{}", self.signed_value()) } else { write!(f, "{}", self.val) }
    }
}
impl AdtT {
    fn initial_discriminant(&self) -> Discr { Discr { val: 0, ty: self.repr_int.unwrap_or(Prim::Isize) } }
    /// adt.rs:640-675: evaluate the declaration's anonymous constant by its identity.
    pub fn eval_explicit_discr(&self, tcx: &Tcx, prog: &Program, def: DefId) -> Result<Discr, ErrorHandled> {
        assert!(self.is_enum());
        let initial = self.initial_discriminant();
        let value = tcx.const_eval_resolve(prog, TypingEnv::fully_monomorphized(prog), def, &[])?;
        let leaf = value.try_to_leaf().ok_or_else(evaluation_error)?;
        Ok(Discr { val: leaf.to_bits(Types::int_info(initial.ty).0 as u8), ty: initial.ty })
    }
    pub fn discriminants(&self, tcx: &Tcx, prog: &Program) -> Vec<(u32, Discr)> {
        assert!(self.is_enum());
        let mut prev = None::<Discr>;
        let mut result = Vec::with_capacity(self.variants.len());
        for (i, v) in self.variants.iter().enumerate() {
            let mut discr = prev.map_or(self.initial_discriminant(), Discr::wrap_incr);
            if let VariantDiscr::Explicit(def) = v.discr {
                if let Ok(value) = self.eval_explicit_discr(tcx, prog, def) { discr = value; }
            }
            prev = Some(discr);
            result.push((i as u32, discr));
        }
        result
    }
    /// RELOAD' (lead design, round 33): `discriminants` with every explicit value from
    /// const folding alone (const_eval::fold_without_execution); None when one needs the
    /// executable provider. A failed evaluation keeps the recovery value, as `discriminants`.
    pub fn fold_discriminants(&self, tcx: &Tcx, prog: &Program) -> Option<Vec<u128>> {
        assert!(self.is_enum());
        let initial = self.initial_discriminant();
        let mut prev = None::<Discr>;
        let mut result = Vec::with_capacity(self.variants.len());
        for v in &self.variants {
            let mut discr = prev.map_or(initial, Discr::wrap_incr);
            if let VariantDiscr::Explicit(def) = v.discr {
                let value = crate::const_eval::fold_without_execution(tcx, prog, TypingEnv::fully_monomorphized(prog), def)?;
                if let Some(leaf) = value.ok().and_then(|v| v.try_to_leaf()) {
                    discr = Discr { val: leaf.to_bits(Types::int_info(initial.ty).0 as u8), ty: initial.ty };
                }
            }
            prev = Some(discr);
            result.push(discr.val);
        }
        Some(result)
    }
    /// Empty enums have no discriminant, including for layout's placeholder variant 0.
    pub fn discriminant_for_variant(&self, tcx: &Tcx, prog: &Program, index: u32) -> Option<Discr> {
        if !self.is_enum() { return None; }
        let (def, offset) = self.discriminant_def_for_variant(index)?;
        let value = def.and_then(|def| self.eval_explicit_discr(tcx, prog, def).ok()).unwrap_or(self.initial_discriminant());
        Some(value.checked_add(offset as u128).0)
    }
    pub fn discriminant_def_for_variant(&self, index: u32) -> Option<(Option<DefId>, u32)> {
        let mut explicit_index = index;
        let def = loop {
            match self.variants.get(explicit_index as usize)?.discr {
                VariantDiscr::Relative(0) => break None,
                VariantDiscr::Relative(distance) => explicit_index -= distance,
                VariantDiscr::Explicit(def) => break Some(def),
            }
        };
        Some((def, index - explicit_index))
    }
    /// collect.rs:825-850 and check/check.rs:2005-2101: the recovery values
    /// used by layout do not suppress evaluation, increment or duplicate errors.
    pub fn check_discriminants(&self, tcx: &Tcx, prog: &Program) -> Vec<String> {
        let mut errors = Vec::new();
        let mut prev = None::<Discr>;
        for v in &self.variants {
            let wrapped = prev.map_or(self.initial_discriminant(), Discr::wrap_incr);
            let current = match v.discr {
                VariantDiscr::Explicit(def) => match self.eval_explicit_discr(tcx, prog, def) {
                    Ok(discr) => discr,
                    Err(_) => { errors.push(diagnostic(prog, def, "E0080: evaluation of enum discriminant failed")); wrapped }
                },
                VariantDiscr::Relative(_) => {
                    if prev.is_some_and(|d| d.checked_add(1).1) {
                        errors.push(diagnostic(prog, v.def, &format!("E0370: enum discriminant overflowed after `{}`", prev.unwrap())));
                    }
                    wrapped
                }
            };
            prev = Some(current);
        }
        let mut discrs = self.discriminants(tcx, prog);
        let mut i = 0;
        while i < discrs.len() {
            let mut j = i + 1;
            let mut reported = false;
            while j < discrs.len() {
                if discrs[i].1.val == discrs[j].1.val {
                    if !reported {
                        errors.push(diagnostic(prog, self.variants[discrs[i].0 as usize].def,
                            &format!("E0081: discriminant value `{}` assigned more than once", discrs[i].1)));
                        reported = true;
                    }
                    discrs.swap_remove(j);
                } else { j += 1; }
            }
            i += 1;
        }
        errors
    }
}
fn diagnostic(prog: &Program, def: DefId, message: &str) -> String {
    let d = prog.def(def);
    let file = &prog.files[d.file as usize];
    let (line, col) = file.line_col(file.ast.item(d.item).lo);
    format!("{}:{}:{}: {}", prog.file_paths[d.file as usize], line, col, message)
}

impl Tcx {
    pub fn check_enum_discriminants(&self, prog: &Program, krate: u32) -> Vec<String> {
        let mut errors = Vec::new();
        for (i, def) in prog.defs.iter().enumerate() {
            if def.krate == krate && def.kind == crate::program::DefKind::Enum {
                errors.extend(self.adts[&DefId(i as u32)].check_discriminants(self, prog));
            }
        }
        errors
    }
}
