// ported from N:rustc_hir_typeck/src/fn_ctxt/_impl.rs:262-309,543-554,588-594 @8d1a764 (1.97.1 identical),
// gather_locals.rs:132-160, method/confirm.rs:496-536, closure.rs:944.
//! User-written types, saved before inference unifies them: `user_provided_types` and
//! `user_provided_sigs` of rustc's TypeckResults. MIR borrowck relates the inferred type to the
//! canonical annotation (`AscribeUserType`, `Constant::user_ty`), which is how a written lifetime
//! (`let x: &'a u8`, `Vec::<&'a u8>::new()`) reaches region inference.
use super::*;
use crate::consts::GenericArg;
use crate::next_solver::canonical::user_type::{canonicalize_user_type, canonicalize_user_type_response};
use crate::types::user_type::{UserArgs, UserSelfTy, UserType, UserTypeKind};
use crate::types::TypeFlags as F;

/// rustc keys annotations by HirId; Relax has separate arenas for type syntax, expressions and
/// patterns, so the node space is part of the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UserTypeKey {
    Type(u32),
    Expr(u32),
    Pattern(u32),
}

impl UserTypeKey {
    /// The key of a value-path node: a pattern id carries `PAT_KEY`.
    pub fn of_node(node: u32) -> Self {
        if node & PAT_KEY != 0 { UserTypeKey::Pattern(node & !PAT_KEY) } else { UserTypeKey::Expr(node) }
    }
}

impl Fcx<'_> {
    /// fn_ctxt/_impl.rs:588-594 can_contain_user_lifetime_bounds: only a type with a free region,
    /// an alias, an inference variable or a parameter can add a constraint over its inferred type.
    fn can_contain_user_lifetime_bounds(&self, flags: F) -> bool {
        flags.intersects(F::HAS_FREE_REGIONS | F::HAS_ALIAS | F::HAS_TY_INFER | F::HAS_PARAM)
    }

    fn arg_can_contain_user_lifetime_bounds(&self, arg: GenericArg) -> bool {
        match arg {
            GenericArg::Type(ty) => self.can_contain_user_lifetime_bounds(self.tcx.tys.flags(ty)),
            GenericArg::Const(ct) => self.tcx.tys.const_flags(ct).0 & (F::HAS_FREE_REGIONS | F::HAS_ALIAS | F::HAS_TY_INFER | F::HAS_PARAM).bits() != 0,
            GenericArg::Lifetime(r) => !matches!(self.tcx.tys.region_kind(r), crate::types::RegionKind::ReBound(..) | crate::types::RegionKind::ReErased),
        }
    }

    /// fn_ctxt/_impl.rs:293-309 write_user_type_annotation: an identity annotation adds nothing.
    fn write_user_type_annotation(&mut self, key: UserTypeKey, user: UserType) {
        let canonical = canonicalize_user_type(&self.infcx, &user);
        if canonical.is_identity(&self.tcx.tys) { return; }
        self.b.user_provided_types.insert(key, std::sync::Arc::new(canonical));
    }

    /// fn_ctxt/_impl.rs:267-290. Call before the path's arguments are unified with anything, so
    /// that `Vec::<_>::new` keeps its hole. `node` is an expression id, or a pattern id | PAT_KEY;
    /// u32::MAX is a name-resolution probe with no node.
    pub(super) fn write_user_type_annotation_from_args(&mut self, node: u32, def: DefId, args: &[GenericArg], user_self_ty: Option<UserSelfTy>) {
        if node == u32::MAX || !self.keep_user_types { return; }
        let can_contain = args.iter().any(|&arg| self.arg_can_contain_user_lifetime_bounds(arg))
            || user_self_ty.is_some_and(|s| self.can_contain_user_lifetime_bounds(self.tcx.tys.flags(s.self_ty)));
        if !can_contain { return; }
        // The overwhelmingly common path -- every argument a distinct fresh variable -- is the
        // identity annotation by construction (canonical variables number in first-occurrence
        // order); decide that without canonicalizing.
        if user_self_ty.is_none() && self.args_are_distinct_fresh_vars(args) { return; }
        let user = UserType::new(UserTypeKind::TypeOf(def, UserArgs { args: args.to_vec(), user_self_ty }));
        self.write_user_type_annotation(UserTypeKey::of_node(node), user);
    }

    /// Whether `args` are unresolved inference variables, all different (after unification):
    /// then `CanonicalUserType::is_identity` holds for them. Anything else answers false.
    fn args_are_distinct_fresh_vars(&mut self, args: &[GenericArg]) -> bool {
        let tys = &self.tcx.tys;
        let mut seen: Vec<(u8, u32)> = Vec::with_capacity(args.len());
        for &arg in args {
            let key = match arg {
                GenericArg::Type(ty) => {
                    let ty = self.infcx.shallow_resolve(ty);
                    match *tys.kind(ty) {
                        TyKind::Infer(vid) => (0, self.infcx.inner.borrow_mut().type_variables().root_var(vid)),
                        _ => return false,
                    }
                }
                GenericArg::Lifetime(r) => match tys.region_kind(r) {
                    crate::types::RegionKind::ReVar(vid) => (1, vid),
                    _ => return false,
                },
                GenericArg::Const(ct) => {
                    let ct = self.infcx.shallow_resolve_const(ct);
                    match tys.const_kind(ct) {
                        crate::consts::ConstKind::Infer(crate::consts::InferConst::Var(vid)) =>
                            (2, self.infcx.inner.borrow_mut().const_unification_table().find(vid.0)),
                        _ => return false,
                    }
                }
            };
            if seen.contains(&key) { return false; }
            seen.push(key);
        }
        true
    }

    /// fn_ctxt/_impl.rs:556-565 + check_struct_path (checks.rs:790-815): a struct literal or
    /// pattern annotates the ADT's instantiated arguments, before the fields constrain them.
    pub(super) fn write_adt_user_type(&mut self, node: u32, ty: TyId) {
        let TyKind::Adt(def, args, regions) = *self.tcx.tys.kind(ty) else { return; };
        let args = self.tcx.complete_args(self.prog, def, self.tcx.tys.list(args), regions);
        self.write_user_type_annotation_from_args(node, def, &args, None);
    }

    /// method/confirm.rs:496-536. The receiver's parameters are inferred, not written: replace the
    /// parent prefix with fresh variables (under a probe) and keep the method's own arguments.
    pub(super) fn write_method_user_type(&mut self, call: u32, def: DefId, args: &[GenericArg]) {
        let parent = self.tcx.generic_indices_of(self.prog, self.prog.def(def).parent);
        let parent_count = parent.lifetimes.len() + parent.types_consts.len();
        // `!args.is_empty() && !generics.is_own_empty()`
        if args.len() <= parent_count || !self.keep_user_types { return; }
        let snapshot = self.start_snapshot();
        let mut user_args = args.to_vec();
        for arg in &mut user_args[..parent_count.min(args.len())] {
            *arg = match *arg {
                GenericArg::Type(_) => GenericArg::Type(self.infcx.next_ty_var()),
                GenericArg::Lifetime(_) => GenericArg::Lifetime(self.infcx.next_region_var()),
                GenericArg::Const(_) => GenericArg::Const(self.infcx.next_const_var()),
            };
        }
        let user = UserType::new(UserTypeKind::TypeOf(def, UserArgs { args: user_args, user_self_ty: None }));
        let canonical = canonicalize_user_type(&self.infcx, &user);
        self.rollback_to(snapshot);
        if !canonical.is_identity(&self.tcx.tys) {
            self.b.user_provided_types.insert(UserTypeKey::Expr(call), std::sync::Arc::new(canonical));
        }
    }

    /// fn_ctxt/_impl.rs:543-554: the type of a cast target. Relax lowers and normalizes in one
    /// step, so the saved type is the lowered one.
    pub(super) fn lower_ty_saving_user_provided_ty(&mut self, syntax: crate::ast::TyId) -> TyId {
        let ty = self.lower_ty(syntax);
        if self.keep_user_types && self.can_contain_user_lifetime_bounds(self.tcx.tys.flags(ty)) {
            let user = UserType::new(UserTypeKind::Ty(ty));
            let canonical = canonicalize_user_type_response(&self.infcx, &user);
            self.b.user_provided_types.insert(UserTypeKey::Type(syntax.0), std::sync::Arc::new(canonical));
        }
        ty
    }

    /// gather_locals.rs:136-152 `declare`: the annotation of a `let`, saved UNCONDITIONALLY (rustc
    /// does not apply `can_contain_user_lifetime_bounds` here, unlike casts), before it is
    /// normalized or related to the initializer. `impl Trait` bounds in bindings
    /// (`collect_impl_trait_clauses_from_hir_ty`) do not exist in Relax.
    pub(super) fn lower_let_ty_saving_user_provided_ty(&mut self, syntax: crate::ast::TyId) -> TyId {
        let ty = self.lower_ty(syntax);
        if self.keep_user_types { self.write_user_type_annotation(UserTypeKey::Type(syntax.0), UserType::new(UserTypeKind::Ty(ty))); }
        ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typeck::solve::program;

    fn body_of(src: &str, name: &str) -> (Program, Tcx, std::sync::Arc<Body>) {
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(true)));
        let (prog, tcx) = program(src);
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == name).map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
        (prog, tcx, body)
    }

    fn annotations(body: &Body) -> Vec<(UserTypeKey, std::sync::Arc<crate::types::user_type::CanonicalUserType>)> {
        body.user_provided_types.iter().map(|(k, v)| (*k, v.clone())).collect()
    }

    /// The type is a reference whose region is a declared one (`'a`), not a variable.
    fn is_declared_region(tcx: &Tcx, r: crate::types::RegionId) -> bool {
        matches!(tcx.tys.region_kind(r), crate::types::RegionKind::ReEarlyParam(_) | crate::types::RegionKind::ReLateParam(..))
    }

    fn mentions_early_region(tcx: &Tcx, ty: TyId) -> bool {
        matches!(tcx.tys.kind(ty), TyKind::Ref(_, _, r) if is_declared_region(tcx, *r))
    }

    #[test]
    fn let_annotation_keeps_the_written_lifetime() {
        let (_, tcx, body) = body_of("#![no_std]\nfn f<'a>(x: &'a u8) { let _y: &'a u8 = x; }", "f");
        let all = annotations(&body);
        assert_eq!(all.len(), 1, "{all:?}");
        let (UserTypeKey::Type(_), user) = &all[0] else { panic!("{all:?}") };
        let UserTypeKind::Ty(ty) = user.value.kind else { panic!("{user:?}") };
        assert!(mentions_early_region(&tcx, ty), "the written 'a is kept: {user:?}");
        assert!(user.var_kinds.is_empty(), "a written lifetime is no canonical variable: {user:?}");
    }

    #[test]
    fn let_annotation_holes_are_canonical_variables() {
        let (_, _, body) = body_of("#![no_std]\nstruct V<T>(T); fn f() { let _v: V<_> = V(1u8); }", "f");
        let ty_annotation = annotations(&body).into_iter().find(|(k, _)| matches!(k, UserTypeKey::Type(_))).expect("the let annotation");
        assert_eq!(ty_annotation.1.var_kinds.len(), 1, "{:?}", ty_annotation.1);
    }

    #[test]
    fn plain_let_annotation_is_saved_like_rustc() {
        // gather_locals saves every let annotation: `let mut v: Vec<usize>` must reach MIR as an AscribeUserType
        let (_, _, body) = body_of("#![no_std]\nfn f() { let _x: u32 = 1; }", "f");
        let all = annotations(&body);
        assert_eq!(all.len(), 1, "{all:?}");
        assert!(all[0].1.var_kinds.is_empty());
    }

    #[test]
    fn identity_path_arguments_are_skipped() {
        // `id(1u8)`: the arguments are the fresh variables [?0] -- the identity annotation of rustc
        let (_, _, body) = body_of("#![no_std]\nfn id<T>(x: T) -> T { x } fn f() { id(1u8); }", "f");
        assert!(annotations(&body).is_empty(), "{:?}", annotations(&body));
    }

    #[test]
    fn turbofish_with_a_hole_is_saved() {
        let (_, _, body) = body_of("#![no_std]\nstruct V<T>(T); fn mk<T>(t: T) -> V<T> { V(t) } fn f() { let _ = mk::<V<_>>(V(1u8)); }", "f");
        let paths: Vec<_> = annotations(&body).into_iter().filter(|(k, _)| matches!(k, UserTypeKey::Expr(_))).collect();
        assert_eq!(paths.len(), 1, "{:?}", annotations(&body));
        let UserTypeKind::TypeOf(_, user) = &paths[0].1.value.kind else { panic!() };
        assert_eq!(user.args.len(), 1);
        assert_eq!(paths[0].1.var_kinds.len(), 1);
    }

    #[test]
    fn written_lifetime_in_turbofish_is_saved() {
        let (_, tcx, body) = body_of("#![no_std]\nfn g<'a, T: 'a>(_: &'a T) {} fn f<'b>(x: &'b u8) { g::<'b, u8>(x); }", "f");
        let paths: Vec<_> = annotations(&body).into_iter().filter(|(k, _)| matches!(k, UserTypeKey::Expr(_))).collect();
        assert_eq!(paths.len(), 1, "{:?}", annotations(&body));
        let UserTypeKind::TypeOf(_, user) = &paths[0].1.value.kind else { panic!() };
        assert!(user.args.iter().any(|a| matches!(a, GenericArg::Lifetime(r) if is_declared_region(&tcx, *r))), "{user:?}");
    }

    #[test]
    fn qualified_trait_path_is_saved() {
        let (_, _, body) = body_of("#![no_std]\ntrait Tr { fn f() -> u8; } impl Tr for u8 { fn f() -> u8 { 1 } } fn h<T: Tr>() -> u8 { <T as Tr>::f() }", "h");
        let paths: Vec<_> = annotations(&body).into_iter().filter(|(k, _)| matches!(k, UserTypeKey::Expr(_))).collect();
        assert_eq!(paths.len(), 1, "{:?}", annotations(&body));
    }

    #[test]
    fn inherent_path_keeps_the_written_self_type() {
        let (_, _, body) = body_of("#![no_std]\nstruct S<'a>(&'a u8); impl<'a> S<'a> { fn new(x: &'a u8) -> S<'a> { S(x) } } fn f<'b>(x: &'b u8) { let _ = S::<'b>::new(x); }", "f");
        let paths: Vec<_> = annotations(&body).into_iter().filter(|(k, _)| matches!(k, UserTypeKey::Expr(_))).collect();
        let user = paths.iter().find_map(|(_, u)| match &u.value.kind { UserTypeKind::TypeOf(_, a) if a.user_self_ty.is_some() => Some(a.clone()), _ => None });
        assert!(user.is_some(), "{:?}", annotations(&body));
    }

    #[test]
    fn struct_literal_and_pattern_with_a_hole_are_saved() {
        let (_, _, body) = body_of("#![no_std]\nstruct W<T> { t: T } enum O<T> { N, S(T) } fn f() { let w = W::<O<_>> { t: O::S(1u8) }; let W::<O<_>> { t: _ } = w; }", "f");
        assert_eq!(annotations(&body).iter().filter(|(k, _)| matches!(k, UserTypeKey::Expr(_))).count(), 1, "{:?}", annotations(&body));
        assert_eq!(annotations(&body).iter().filter(|(k, _)| matches!(k, UserTypeKey::Pattern(_))).count(), 1, "{:?}", annotations(&body));
    }

    #[test]
    fn method_call_user_args_drop_the_receiver_prefix() {
        let (_, _, body) = body_of("#![no_std]\nstruct S; enum O<T> { N, S(T) } impl S { fn m<U>(&self, u: U) -> U { u } } fn f(s: S) { s.m::<O<_>>(O::S(1u8)); }", "f");
        let calls: Vec<_> = annotations(&body).into_iter().filter(|(k, _)| matches!(k, UserTypeKey::Expr(_))).collect();
        assert!(!calls.is_empty(), "{:?}", annotations(&body));
    }

    /// A call-form assignee `T::<&'a u8>(a, ..) = t` is annotated like the constructor path of a
    /// tuple-struct expression (rustc lowers it to a pattern with the written path).
    #[test]
    fn call_form_assignee_saves_the_constructor_annotation() {
        let (_, _, body) = body_of("#![no_std]\nstruct T<X>(X, u8); fn f<'a>(t: T<&'a u8>, mut a: &'a u8) { T::<&'a u8>(a, ..) = t; }", "f");
        assert!(annotations(&body).iter().any(|(k, _)| matches!(k, UserTypeKey::Expr(_))), "{:?}", annotations(&body));
    }

    #[test]
    fn closure_signature_is_saved_before_merging() {
        let (_, _, body) = body_of("#![no_std]\nfn f() { let _c = |x: &u8| *x; }", "f");
        assert_eq!(body.user_provided_sigs.len(), 1);
    }

    #[test]
    fn cast_target_with_a_free_region_is_saved() {
        let (_, _, body) = body_of("#![no_std]\nfn f<'a>(p: *const &'a u8) { let _q = p as *const &'a u8; }", "f");
        assert!(annotations(&body).iter().any(|(k, _)| matches!(k, UserTypeKey::Type(_))), "{:?}", annotations(&body));
    }

    #[test]
    fn elided_region_in_a_let_annotation_is_a_canonical_variable() {
        let (_, _, body) = body_of("#![no_std]\nfn f(x: &u8) { let _y: &u8 = x; }", "f");
        let all = annotations(&body);
        assert_eq!(all.len(), 1, "{all:?}");
        assert!(all[0].1.var_kinds.iter().any(|k| matches!(k, crate::next_solver::canonical::CanonicalVarKind::Region(_))), "{:?}", all[0].1);
    }

    #[test]
    fn named_lifetime_survives_a_projection_annotation() {
        let (_, tcx, body) = body_of("#![no_std]\ntrait Tr { type A; } impl<'a> Tr for &'a u8 { type A = &'a u8; } fn f<'a>(x: &'a u8) { let _y: <&'a u8 as Tr>::A = x; }", "f");
        let all = annotations(&body);
        assert_eq!(all.len(), 1, "{all:?}");
        let UserTypeKind::Ty(ty) = all[0].1.value.kind else { panic!() };
        assert!(tcx.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS), "the annotation keeps a free region");
    }

    /// rustc rejects this in borrowck (`'a: 'static`): the annotation must keep the alias with
    /// its written lifetime, because normalizing it to `u32` would erase the constraint.
    #[test]
    fn projection_annotation_is_saved_unnormalized() {
        let (_, tcx, body) = body_of("#![no_std]\ntrait Tr { type A; } impl Tr for &'static u8 { type A = u32; } fn f<'a>(x: &'a u8) { let _y: <&'a u8 as Tr>::A; }", "f");
        let all = annotations(&body);
        assert_eq!(all.len(), 1, "the unnormalized annotation is saved: {all:?}");
        let UserTypeKind::Ty(ty) = all[0].1.value.kind else { panic!() };
        assert!(matches!(tcx.tys.kind(ty), TyKind::Alias(..)), "kept as the projection");
        assert!(tcx.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS), "with its 'a");
    }

    #[test]
    fn crates_that_are_not_borrow_checked_keep_no_user_types() {
        // the run-path rule: nothing is canonicalized for std / framework crates
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(false)));
        let (prog, tcx) = program("#![no_std]\nfn id<T>(x: T) -> T { x } fn f<'a>(x: &'a u8) { let _y: &'a u8 = x; let _z = id::<&'a u8>(x); let _c = |a: &u8| *a; }");
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "f").map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        assert!(body.user_provided_types.is_empty() && body.user_provided_sigs.is_empty());
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(None));
    }
}
#[cfg(test)]
mod ctor_tests {
    use crate::typeck::solve::program;
    #[test]
    fn a_turbofish_on_a_tuple_struct_constructor_call_is_saved() {
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(true)));
        let (prog, tcx) = program("#![no_std]\nstruct S<T>(T); fn f() { let c = 1u32; S::<&'static u32>(&c); }");
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "f").map(|(i, _)| crate::program::DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        assert!(!body.user_provided_types.is_empty(), "the constructor path's turbofish is an annotation");
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(None));
    }
}
