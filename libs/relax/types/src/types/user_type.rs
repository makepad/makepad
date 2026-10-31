//! User-written types before body inference, following
//! rustc_middle/src/ty/typeck_results.rs:731-912 (CanonicalUserType, UserType, UserTypeKind,
//! UserArgs, UserSelfTy, IsIdentity).

use crate::consts::GenericArg;
use crate::next_solver::canonical::Canonical;
use crate::next_solver::Predicate;
use crate::program::DefId;
use crate::types::fold::TypeFolder;
use crate::types::{TyId, Types};

pub type CanonicalUserType = Canonical<UserType>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UserType {
    pub kind: UserTypeKind,
    /// Bounds introduced by `impl Trait` in a binding belong to the same
    /// canonical binder as its type, and must be proved during MIR type checking.
    pub bounds: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UserTypeKind {
    Ty(TyId),
    TypeOf(DefId, UserArgs),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UserArgs {
    pub args: Vec<GenericArg>,
    pub user_self_ty: Option<UserSelfTy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UserSelfTy {
    pub impl_def_id: DefId,
    pub self_ty: TyId,
}

impl UserType {
    /// rustc_middle/src/ty/typeck_results.rs:840.
    pub fn new(kind: UserTypeKind) -> Self {
        Self { kind, bounds: Vec::new() }
    }

    /// rustc_middle/src/ty/typeck_results.rs:833-837 (derived TypeFoldable).
    /// The canonical binder is outside this value; predicates introduce their
    /// own binders, while types and generic arguments start at depth zero.
    pub fn fold_with(&self, tys: &Types, folder: &mut dyn TypeFolder) -> Self {
        use crate::next_solver::fold::{fold_arg, fold_predicate};
        let kind = match &self.kind {
            UserTypeKind::Ty(ty) => UserTypeKind::Ty(folder.fold_ty(tys, *ty, 0)),
            UserTypeKind::TypeOf(def, user) => UserTypeKind::TypeOf(*def, UserArgs {
                args: user.args.iter().map(|&arg| fold_arg(tys, arg, folder, 0)).collect(),
                user_self_ty: user.user_self_ty.map(|s| UserSelfTy {
                    impl_def_id: s.impl_def_id,
                    self_ty: folder.fold_ty(tys, s.self_ty, 0),
                }),
            }),
        };
        let bounds = self.bounds.iter().map(|p| fold_predicate(tys, p, folder)).collect();
        Self { kind, bounds }
    }
}

impl CanonicalUserType {
    /// rustc_middle/src/ty/typeck_results.rs:868-912: omit only an identity
    /// substitution with distinct canonical variables in declaration order.
    pub fn is_identity(&self, tys: &Types) -> bool {
        use crate::consts::ConstKind;
        use crate::types::{RegionKind, TyKind};
        if !self.value.bounds.is_empty() { return false; }
        let UserTypeKind::TypeOf(_, user) = &self.value.kind else { return false; };
        user.user_self_ty.is_none() && user.args.iter().enumerate().all(|(i, arg)| {
            let var = match *arg {
                GenericArg::Type(ty) => match tys.kind(ty) {
                    TyKind::Bound(0, b) => b.var,
                    _ => return false,
                },
                GenericArg::Lifetime(region) => match tys.region_kind(region) {
                    RegionKind::ReBound(0, bound) => bound,
                    _ => return false,
                },
                GenericArg::Const(ct) => match tys.const_kind(ct) {
                    ConstKind::Bound(0, b) => b.var,
                    _ => return false,
                },
            };
            var.0 as usize == i
        })
    }
}
