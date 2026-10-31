use super::*;
use crate::types::user_type::{CanonicalUserType, UserType};

/// rustc_infer/src/infer/canonical/canonicalizer.rs:118-131,219-249.
/// Canonicalize before unifying a user annotation with its initializer. Unlike
/// a solver input, declaration parameters and written regions stay rigid.
pub fn canonicalize_user_type(infcx: &InferCtxt<'_>, user: &UserType) -> CanonicalUserType {
    let mut folder = Canonicalizer::new(infcx, CanonicalizeMode::UserType);
    folder.infer_query = true;
    let value = user.fold_with(&infcx.tcx.tys, &mut folder);
    let (max_universe, _, var_kinds) = folder.finalize();
    Canonical { value, max_universe, var_kinds }
}

/// rustc_hir_typeck/src/fn_ctxt/_impl.rs:577-590. Type syntax uses response
/// canonicalization, preserving universes and resolved region equalities.
pub fn canonicalize_user_type_response(infcx: &InferCtxt<'_>, user: &UserType) -> CanonicalUserType {
    let mut folder = Canonicalizer::new(infcx, CanonicalizeMode::Response { max_input_universe: UniverseIndex::ROOT });
    folder.infer_query = true;
    let value = user.fold_with(&infcx.tcx.tys, &mut folder);
    let (max_universe, _, var_kinds) = folder.finalize();
    Canonical { value, max_universe, var_kinds }
}

/// rustc_hir_typeck/src/closure.rs:907-913. A supplied closure signature uses
/// response canonicalization, before normalization or merging the expectation.
/// The FnPtr binder protects the signature's own late-bound lifetimes.
pub fn canonicalize_user_signature(infcx: &InferCtxt<'_>, signature: TyId) -> Canonical<TyId> {
    let mut folder = Canonicalizer::new(infcx, CanonicalizeMode::Response { max_input_universe: UniverseIndex::ROOT });
    folder.infer_query = true;
    let value = folder.fold_ty(&infcx.tcx.tys, signature, 0);
    let (max_universe, _, var_kinds) = folder.finalize();
    Canonical { value, max_universe, var_kinds }
}

/// rustc_borrowck/src/type_check/canonical.rs:64-81. The canonical variables
/// are fresh for each ascription, and shared by its type, arguments and bounds.
pub fn instantiate_user_type(infcx: &InferCtxt<'_>, user: &CanonicalUserType) -> UserType {
    let values = instantiate_canonical_vars(infcx, user.max_universe, &user.var_kinds);
    user.value.fold_with(&infcx.tcx.tys, &mut CanonicalInstantiator { values: &values })
}

/// rustc_borrowck/src/type_check/input_output.rs:48-56. This instantiates only
/// the outer canonical binder; the caller then instantiates the FnPtr binder.
pub fn instantiate_user_signature(infcx: &InferCtxt<'_>, signature: &Canonical<TyId>) -> TyId {
    let values = instantiate_canonical_vars(infcx, signature.max_universe, &signature.var_kinds);
    CanonicalInstantiator { values: &values }.fold_ty(&infcx.tcx.tys, signature.value, 0)
}
