// ported from N:rustc_borrowck/src/type_check/canonical.rs @8d1a764 (fully_perform_op_raw, :21-71)
use super::constraint_conversion::ConstraintConversion;
use super::{Locations, MirTypeckRegionConstraints};
use crate::mir::Span;
use crate::borrowck::constraints::{ConstraintCategory};
use crate::borrowck::infer_ctxt::BorrowckInferCtxt;
use crate::borrowck::universal_regions::UniversalRegions;
use crate::next_solver::type_op::{TypeOp, TypeOpOutput};
use crate::next_solver::{NoSolution, Predicate};
use crate::types::{RegionId, TyId};

/// Given some operation `op` that manipulates types, proves predicates, or otherwise uses
/// the inference context, executes `op` and then executes all the further obligations
/// that `op` returns. The outlives constraints among regions that result are recorded as
/// having occurred at `locations`.
///
/// **Any inference operations that might generate region constraints should occur within
/// this method so that those constraints can be properly localized!**
///
/// `span` is `locations.span(body)`, supplied by the caller (the body is the driver's).
#[allow(clippy::too_many_arguments)]
pub fn fully_perform_op_raw<Op: TypeOp>(
    infcx: &BorrowckInferCtxt<'_>,
    universal_regions: &UniversalRegions,
    region_bound_pairs: &[(TyId, RegionId)],
    known_type_outlives_obligations: &[Predicate],
    constraints: &mut MirTypeckRegionConstraints,
    locations: Locations,
    span: Span,
    category: ConstraintCategory,
    op: Op,
) -> Result<Op::Output, NoSolution> {
    let TypeOpOutput { output, constraints: query_constraints } = op.fully_perform(&infcx.infcx, infcx.prog)?;
    if cfg!(debug_assertions) {
        let data = infcx.take_and_reset_region_constraints();
        if !data.constraints.is_empty() { panic!("leftover region constraints: {:?}", data.constraints); }
    }
    if let Some(data) = query_constraints {
        ConstraintConversion::new(infcx, universal_regions, region_bound_pairs, known_type_outlives_obligations, locations, span, category, constraints)
            .convert_all(&data);
    }
    Ok(output)
}
