// ported from N:rustc_borrowck/src/type_check/input_output.rs @8d1a764
//! Equates the MIR's input/output locals with the normalized signature, and checks
//! explicit closure signature annotations.
use super::*;

impl<'a> TypeChecker<'a> {
    /// Check an explicit closure signature annotation, e.g. `|x: &'static u32| ...`.
    /// The async coroutine-closure output rewrite (input_output.rs:58-102) needs
    /// DefiningTy::CoroutineClosure and is part of N01/M2b's closure handoff.
    pub(super) fn check_signature_annotation(&mut self) {
        if !matches!(self.body.source.kind, BodyKind::Closure(_)) { return; }
        let Some(user_provided_sig) = self.infcx.closure_user_provided_sig(self.body) else { return };
        // MIR closure arguments begin with the implicit `self`; coroutines have a resume
        // argument which may be implicitly `()`.
        let is_coroutine_with_implicit_resume_ty = self.body.coroutine.is_some() && user_provided_sig.inputs.is_empty();
        let skip = 1 + is_coroutine_with_implicit_resume_ty as usize;
        let args: Vec<_> = (1..=self.body.arg_count as usize).skip(skip).collect();
        if args.len() != user_provided_sig.inputs.len() {
            return self.mirbug(format!("closure signature {user_provided_sig:?} does not match {} arguments", args.len()));
        }
        for (&user_ty, local) in user_provided_sig.inputs.iter().zip(args) {
            let decl = &self.body.local_decls[local];
            self.infcx.ascribe_user_type_skip_wf(&mut self.constraints, decl.ty, user_ty, decl.source_info.span);
        }
        // An explicitly annotated output type is enforced.
        let output_decl = &self.body.local_decls[0];
        self.infcx.ascribe_user_type_skip_wf(&mut self.constraints, output_decl.ty, user_provided_sig.output, output_decl.source_info.span);
    }

    /// Equate the types of the input/output locals with the *normalized* signature
    /// produced while computing implied bounds: locals start with fresh region variables
    /// that must equal the universal regions, and normalization may introduce new
    /// variables that the implied bounds mention (#136547).
    pub(super) fn equate_inputs_and_outputs(&mut self, normalized_inputs_and_output: &[TyId], universal_yield_resume: Option<(TyId, TyId)>) {
        let (&normalized_output_ty, normalized_input_tys) = normalized_inputs_and_output.split_last().expect("signature without output");
        for (argument_index, &normalized_input_ty) in normalized_input_tys.iter().enumerate() {
            if argument_index + 1 >= self.body.local_decls.len() {
                return self.mirbug("found more normalized_input_ty than local_decls".into());
            }
            // Argument N is stored in local N+1.
            let decl = &self.body.local_decls[argument_index + 1];
            self.equate_normalized_input_or_output(normalized_input_ty, decl.ty, decl.source_info.span);
        }
        if let Some(coroutine) = &self.body.coroutine {
            let (universal_yield, universal_resume) = universal_yield_resume.expect("coroutine without universal yield/resume types");
            let yield_span = self.body.local_decls[0].source_info.span;
            self.equate_normalized_input_or_output(universal_yield, coroutine.yield_ty, yield_span);
            self.equate_normalized_input_or_output(universal_resume, coroutine.resume_ty, yield_span);
        }
        // The return place.
        let output = &self.body.local_decls[0];
        self.equate_normalized_input_or_output(normalized_output_ty, output.ty, output.source_info.span);
    }

    /// The next solver deeply normalizes writeback results: no re-normalization retry.
    fn equate_normalized_input_or_output(&mut self, a: TyId, b: TyId, span: Span) {
        if let Err(terr) = self.eq_types(a, b, Locations::All(span), ConstraintCategory::BoringNoLocation) {
            self.mirbug(format!("equate_normalized_input_or_output: `{a:?}=={b:?}` failed with `{terr:?}`"));
        }
    }
}
