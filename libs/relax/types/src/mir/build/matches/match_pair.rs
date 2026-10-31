//! rustc_mir_build/src/builder/matches/match_pair.rs: the match pair tree of a pattern.
use super::{FlatPat, MatchPairTree, PatConstKind, PatternExtraData, SliceLenOp, SubpatternBindings, TestableCase};
use crate::mir::build::thir::{DerefPatBorrowMode, FieldPat, Pat, PatKind, Pinnedness};
use crate::mir::build::{BuildResult, Builder, PlaceBase, PlaceBuilder, UnsupportedReason};
use crate::mir::{FieldIdx, PlaceElem};
use crate::types::TyKind;
use std::sync::Arc;

/// For an array or slice pattern's subpatterns (prefix/slice/suffix), returns a list
/// of those subpatterns, each paired with a suitably-projected [`PlaceBuilder`].
fn prefix_slice_suffix<'a>(
    place: &PlaceBuilder,
    array_len: Option<u64>, // Some for array patterns; None for slice patterns
    prefix: &'a [Pat],
    opt_slice: &'a Option<Box<Pat>>,
    suffix: &'a [Pat],
) -> Vec<(PlaceBuilder, &'a Pat)> {
    let prefix_len = prefix.len() as u64;
    let suffix_len = suffix.len() as u64;

    let mut output_pairs = Vec::with_capacity(prefix.len() + usize::from(opt_slice.is_some()) + suffix.len());

    // For slice patterns with a `..` followed by 0 or more suffix subpatterns, the actual slice
    // index of those subpatterns isn't statically known, so we have to index them relative to the
    // end of the slice.
    //
    // For array patterns, all subpatterns are indexed relative to the start.
    let (min_length, is_array) = match array_len {
        Some(len) => (len, true),
        None => (prefix_len + suffix_len, false),
    };

    for (offset, prefix_subpat) in (0u64..).zip(prefix) {
        let elem = PlaceElem::ConstantIndex { offset, min_length, from_end: false };
        output_pairs.push((place.clone().project(elem), prefix_subpat));
    }

    if let Some(slice_subpat) = opt_slice {
        let elem = PlaceElem::Subslice {
            from: prefix_len,
            to: if is_array { min_length - suffix_len } else { suffix_len },
            from_end: !is_array,
        };
        output_pairs.push((place.clone().project(elem), slice_subpat));
    }

    for (offset_from_end, suffix_subpat) in (1u64..).zip(suffix.iter().rev()) {
        let elem = PlaceElem::ConstantIndex {
            offset: if is_array { min_length - offset_from_end } else { offset_from_end },
            min_length,
            from_end: !is_array,
        };
        output_pairs.push((place.clone().project(elem), suffix_subpat));
    }

    output_pairs
}

impl MatchPairTree {
    /// Recursively builds a match pair tree for the given pattern and its subpatterns.
    pub(super) fn for_pattern(
        mut place_builder: PlaceBuilder,
        pattern: &Pat,
        cx: &mut Builder<'_>,
        match_pairs: &mut Vec<Self>,     // Newly-created nodes are added to this vector
        extra_data: &mut PatternExtraData, // Bindings/ascriptions are added here
    ) -> BuildResult<()> {
        // Force the place type to the pattern's type.
        if let Some(resolved) = place_builder.resolve_upvar(cx) {
            place_builder = resolved;
        }
        // (No `OpaqueCast` projection: Relax's borrowck reveals opaque types through the solver,
        // like rustc's new-solver mode, where `next_trait_solver_globally` skips the cast.)

        let place = place_builder.try_to_place(cx);

        // Apply any type ascriptions to the value at `match_pair.place`.
        if let (Some(place), Some(extra)) = (&place, &pattern.extra) {
            for ascription in &extra.ascriptions {
                extra_data.ascriptions.push(super::Ascription {
                    source: place.clone(),
                    annotation: ascription.annotation.clone(),
                    variance: ascription.variance,
                });
            }
        }

        let tcx = cx.root.cx.tcx;
        let mut subpairs = Vec::new();
        let testable_case = match &pattern.kind {
            PatKind::Missing | PatKind::Wild | PatKind::Error(_) => None,

            PatKind::Or { pats } => {
                let pats = pats
                    .iter()
                    .map(|pat| FlatPat::new(place_builder.clone(), pat, cx))
                    .collect::<BuildResult<Vec<_>>>()?
                    .into_boxed_slice();
                if !pats[0].extra_data.bindings.is_empty() {
                    // Hold a place for any bindings established in (possibly-nested) or-patterns.
                    // By only holding a place when bindings are present, we skip over any
                    // or-patterns that will be simplified by `merge_trivial_subcandidates`. In
                    // other words, we can assume this expands into subcandidates.
                    extra_data.bindings.push(SubpatternBindings::FromOrPattern);
                }
                Some(TestableCase::Or { pats })
            }

            PatKind::Range(range) => {
                assert_eq!(pattern.ty, range.ty);
                if range.is_full_range(&tcx.tys) == Some(true) {
                    None
                } else {
                    Some(TestableCase::Range(Arc::clone(range)))
                }
            }

            PatKind::Constant { value } => {
                assert_eq!(pattern.ty, value.ty);

                // Classify the constant-pattern into further kinds, to reduce the number of ad-hoc
                // type tests needed later on.
                let const_kind = match *tcx.tys.kind(pattern.ty) {
                    TyKind::Bool => PatConstKind::Bool,
                    TyKind::Int(_) | TyKind::Char => PatConstKind::IntOrChar,
                    TyKind::Float(_) => PatConstKind::Float,
                    TyKind::Str => PatConstKind::String,
                    // raw pointers, pattern types
                    _ => PatConstKind::Other,
                };
                Some(TestableCase::Constant { value: value.clone(), kind: const_kind })
            }

            PatKind::Binding { mode, var, is_shorthand, subpattern, .. } => {
                // In order to please the borrow checker, when lowering a pattern like `x @ subpat`
                // we must establish any bindings in `subpat` before establishing the binding for
                // `x`.
                //
                // For example (from #69971):
                //
                // ```ignore (illustrative)
                // struct NonCopyStruct {
                //     copy_field: u32,
                // }
                //
                // fn foo1(x: NonCopyStruct) {
                //     let y @ NonCopyStruct { copy_field: z } = x;
                //     // the above should turn into
                //     let z = x.copy_field;
                //     let y = x;
                // }
                // ```

                // First, recurse into the subpattern, if any.
                if let Some(subpattern) = subpattern.as_ref() {
                    // this is the `x @ P` case; have to keep matching against `P` now
                    MatchPairTree::for_pattern(place_builder, subpattern, cx, &mut subpairs, extra_data)?;
                }

                // Then push this binding, after any bindings in the subpattern.
                if let Some(source) = place.clone() {
                    extra_data.bindings.push(SubpatternBindings::One(super::Binding {
                        span: pattern.span,
                        source,
                        var_id: *var,
                        binding_mode: *mode,
                        is_shorthand: *is_shorthand,
                    }));
                }

                None
            }

            PatKind::Array { prefix, slice, suffix } => {
                // Determine the statically-known length of the array type being matched. This
                // should always succeed for legal programs, but could fail for erroneous programs
                // (e.g. the type is `[u8; const { panic!() }]`), so take care not to fail if this
                // fails.
                let array_len = match *tcx.tys.kind(pattern.ty) {
                    TyKind::Array(_, len) => len.try_to_target_usize(&tcx.tys),
                    _ => None,
                };
                let Some(array_len) = array_len else {
                    return Err(cx.error(pattern.span, UnsupportedReason::UnresolvedFact("array length in a pattern")));
                };
                for (subplace, subpat) in prefix_slice_suffix(&place_builder, Some(array_len), prefix, slice, suffix) {
                    MatchPairTree::for_pattern(subplace, subpat, cx, &mut subpairs, extra_data)?;
                }
                None
            }
            PatKind::Slice { prefix, slice, suffix } => {
                for (subplace, subpat) in prefix_slice_suffix(&place_builder, None, prefix, slice, suffix) {
                    MatchPairTree::for_pattern(subplace, subpat, cx, &mut subpairs, extra_data)?;
                }

                if prefix.is_empty() && slice.is_some() && suffix.is_empty() {
                    // This pattern is shaped like `[..]`. It can match a slice of any length, so no
                    // length test is needed.
                    None
                } else {
                    // Any other shape of slice pattern requires a length test. Slice patterns with
                    // a `..` subpattern require a minimum length; those without `..` require an
                    // exact length.
                    Some(TestableCase::Slice {
                        len: (prefix.len() + suffix.len()) as u64,
                        op: if slice.is_some() { SliceLenOp::GreaterOrEqual } else { SliceLenOp::Equal },
                    })
                }
            }

            PatKind::Variant { adt_def, variant_index, args: _, subpatterns } => {
                let Some(adt) = tcx.adts.get(adt_def) else {
                    return Err(cx.error(pattern.span, UnsupportedReason::UnresolvedFact("variant pattern of an unknown ADT")));
                };
                let name = adt.variants.get(variant_index.0 as usize).map(|v| v.name);
                let downcast_place = place_builder.downcast(name, *variant_index); // `(x as Variant)`
                for FieldPat { field, pattern: subpat } in subpatterns {
                    let subplace = downcast_place.clone().project(PlaceElem::Field(*field, subpat.ty));
                    MatchPairTree::for_pattern(subplace, subpat, cx, &mut subpairs, extra_data)?;
                }

                // We treat non-exhaustive enums the same independent of the crate they are defined
                // in, to avoid differences in the operational semantics between crates.
                let refutable = adt.variants.len() > 1 || adt.is_variant_list_non_exhaustive();
                if refutable { Some(TestableCase::Variant { adt_def: *adt_def, variant_index: *variant_index }) } else { None }
            }

            PatKind::Leaf { subpatterns } => {
                for FieldPat { field, pattern: subpat } in subpatterns {
                    let subplace = place_builder.clone().project(PlaceElem::Field(*field, subpat.ty));
                    MatchPairTree::for_pattern(subplace, subpat, cx, &mut subpairs, extra_data)?;
                }
                None
            }

            PatKind::Deref { pin: Pinnedness::Pinned, subpattern } => {
                // Project into the `Pin(_)` struct, then deref the inner `&` or `&mut`.
                let pinned_ref_ty = match cx.pinned_ty(pattern.ty) {
                    Some(p_ty) if matches!(tcx.tys.kind(p_ty), TyKind::Ref(..)) => p_ty,
                    _ => return Err(cx.error(pattern.span, UnsupportedReason::UnresolvedFact("bad type for a pinned deref pattern"))),
                };
                MatchPairTree::for_pattern(place_builder.field(FieldIdx(0), pinned_ref_ty).deref(), subpattern, cx, &mut subpairs, extra_data)?;
                None
            }

            PatKind::Deref { pin: Pinnedness::Not, subpattern } | PatKind::DerefPattern { subpattern, borrow: DerefPatBorrowMode::Box } => {
                MatchPairTree::for_pattern(place_builder.deref(), subpattern, cx, &mut subpairs, extra_data)?;
                None
            }

            PatKind::DerefPattern { subpattern, borrow: DerefPatBorrowMode::Borrow(mutability) } => {
                // Create a new temporary for each deref pattern.
                let temp_ty = cx.ref_ty(*mutability == crate::mir::Mutability::Mut, subpattern.ty);
                let temp = cx.temp(temp_ty, pattern.span);
                MatchPairTree::for_pattern(PlaceBuilder::from(temp.clone()).deref(), subpattern, cx, &mut subpairs, extra_data)?;
                Some(TestableCase::Deref { temp, mutability: *mutability })
            }

            PatKind::Guard { .. } => {
                // guard patterns are not supported (unstable)
                None
            }

            PatKind::Never => Some(TestableCase::Never),
        };

        if let Some(testable_case) = testable_case {
            // This pattern is refutable, so push a new match-pair node.
            //
            // Note: unless test_case is TestCase::Or, place must not be None. This means that the
            // closure capture analysis (upvar.rs, and in particular the pattern handling of the
            // expression use visitor) must capture all of the places we'll use. Make sure to keep
            // these two parts in sync!
            match_pairs.push(MatchPairTree { place, testable_case, subpairs, pattern_span: pattern.span })
        } else {
            // This pattern is irrefutable, so it doesn't need its own match-pair node. Just push
            // its refutable subpatterns instead, if any.
            match_pairs.extend(subpairs);
        }
        Ok(())
    }
}

#[allow(dead_code)]
fn _unused(_: PlaceBase) {}
