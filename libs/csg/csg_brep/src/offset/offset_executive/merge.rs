//! The merge path of `SmOffsetExecutive::PiecewiseMerge`: the merge B-rep and its
//! `SmMerge::PiecewiseMerge` over the offset temporary.
//!
//! Upstream: `source/SMLib/src/SmOffsetExecutive.cpp` (2429–2474).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `merge_offset` (crate) | `new SmBrep()`, `SetTolerance`, `SmMerge sMerge(..)`, `sMerge.PiecewiseMerge(&sNoSSI, TRUE, TRUE, pResult)` |
//! | `OffsetMergeIdentities` (private) | the addresses of the offset temporary, the merge B-rep and the merge's stand-alone face B-reps |
//!
//! Deviations: the merge runs in a [`SweepStore`] over the caller's records and
//! attributes, which owns the merge session (`SmMerge`'s engine and its stand-alone
//! face B-reps). The offset temporary keeps the caller's result identities (its
//! B-rep key, topology, curve and source-list keys); the merge B-rep and the face
//! B-reps take the caller's [`SweepIdentities`] for the store handles they receive
//! (the temporary is handle 0). The no-SSI relation names surfaces by the merge's
//! surface identity ([`KernelManifoldIntersector::surface_identity`]) instead of
//! surface pointers. The temporary returns to the executive on every exit, as
//! `m_pOffsetBrep` stays alive upstream. The merged B-rep's attribute lists move to the
//! result identities, under which the callers go on (the temporary's are cleared). No
//! recursion.

use std::collections::{BTreeMap, BTreeSet};

use crate::boolean::merge::p4::KernelManifoldIntersector;
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::object::ObjectRef;
use crate::geom::tol::Relation;
use crate::kernel::Kernel;
use crate::math::PI;
use crate::sweep::topology_sweep::access::SweepAccess;
use crate::sweep::topology_sweep::store::{SweepIdentities, SweepStore};
use crate::topology::brep::p2 as bp2;
use crate::topology::{Brep, BrepId, EdgeId, FaceId, RegionId};

use super::p2::OffsetNotifications;

const OP: &str = "offset::offset_executive::merge";

/// The store handle of the offset temporary (the first body the store receives).
const OFFSET: BrepId = BrepId::from_index(0);

/// Identities of the merge's bodies: the offset temporary's are the caller's result
/// identities, every later body's those of the caller's merge resolver.
struct OffsetMergeIdentities<'a> {
    result_brep: ObjectRef,
    result_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    result_curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
    boolean_source_lists: &'a dyn Fn(bool, RegionId) -> [ObjectRef; 2],
    bodies: &'a dyn SweepIdentities,
}

impl SweepIdentities for OffsetMergeIdentities<'_> {
    fn topology(&self, brep: BrepId, entity: EntityRef) -> ObjectRef {
        if brep != OFFSET {
            return self.bodies.topology(brep, entity);
        }
        match entity {
            EntityRef::Brep(_) => self.result_brep,
            _ => (self.result_key_of)(entity),
        }
    }
    fn owned_auxiliary_objects(&self, brep: BrepId, model: &Brep) -> Vec<ObjectKey> {
        if brep == OFFSET {
            // the caller's registry keeps the temporary's objects
            Vec::new()
        } else {
            self.bodies.owned_auxiliary_objects(brep, model)
        }
    }
    fn curve(&self, brep: BrepId, edge: EdgeId, curve: &Curve) -> ObjectRef {
        if brep == OFFSET {
            (self.result_curve_ref)(edge, curve.type_code())
        } else {
            self.bodies.curve(brep, edge, curve)
        }
    }
    fn new_curve(&self, brep: BrepId, edge: EdgeId, type_code: i64) -> Result<ObjectRef> {
        if brep == OFFSET {
            Ok((self.result_curve_ref)(edge, type_code))
        } else {
            self.bodies.new_curve(brep, edge, type_code)
        }
    }
    fn source_lists(&self, brep: BrepId, region: RegionId) -> Result<[ObjectRef; 2]> {
        if brep == OFFSET {
            Ok((self.boolean_source_lists)(true, region))
        } else {
            self.bodies.source_lists(brep, region)
        }
    }
}

/// The merge path's tail (cpp:2453–2474): a new merge B-rep with `tolerance` (the
/// original's) and editing enabled, then `SmMerge(ctx, merge, offset, tolerance,
/// 20°)` and `PiecewiseMerge(&no_ssi, TRUE, TRUE)`. `no_ssi` relates faces of
/// `offset` whose surfaces need no intersection. Returns the merged B-rep and hands
/// `offset` back (upstream keeps `m_pOffsetBrep` for its caller to delete), also
/// when the merge fails.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] without the caller's merge identities
/// ([`OffsetNotifications::merge_identities`]); those of the store and of
/// [`crate::boolean::merge::p3::piecewise_merge`].
pub(crate) fn merge_offset(
    kernel: &mut Kernel,
    offset: Brep,
    tolerance: f64,
    no_ssi: &Relation<FaceId, FaceId>,
    notifications: &mut OffsetNotifications<'_>,
) -> (Result<Brep>, Option<Brep>) {
    let Some(bodies) = notifications.merge_identities else {
        return (
            Err(SolidError::invalid_input(
                OP,
                "a merging PiecewiseMerge needs the caller's merge identities",
            )),
            Some(offset),
        );
    };
    // sNoSSI over the merge's surface identities, in the relation's own order
    let mut skip: Relation<ObjectRef, ObjectRef> = Relation::default();
    let related = (|| {
        for a in no_ssi.all_firsts() {
            let sa = KernelManifoldIntersector::surface_identity(&offset, a)?;
            for b in no_ssi.seconds(a) {
                let sb = KernelManifoldIntersector::surface_identity(&offset, b)?;
                skip.relate_pair(sa, sb)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = related {
        return (Err(error), Some(offset));
    }

    let identities = OffsetMergeIdentities {
        result_brep: notifications.result_brep,
        result_key_of: notifications.result_key_of,
        result_curve_ref: notifications.result_curve_ref,
        boolean_source_lists: notifications.boolean_source_lists,
        bodies,
    };
    // the temporary carries the reserved result key (allocate_offset_brep); check it
    // before the store takes the body
    if offset.key() != notifications.result_brep.key {
        return (
            Err(SolidError::invalid_input(
                OP,
                "the offset temporary does not carry the reserved result key",
            )),
            Some(offset),
        );
    }
    let mut next_handle = 0;
    let mut store = SweepStore::new(
        &identities,
        &mut *notifications.records,
        &mut *notifications.attributes,
        &mut next_handle,
    );
    let context = match store.register_kernel(kernel) {
        Ok(context) => context,
        Err(error) => return (Err(error), Some(offset)),
    };
    let offset_id = match store.adopt_brep(context, offset) {
        Ok(id) => id,
        // only exhausted store handles remain here (the key was checked above); the
        // refused body is dropped with the store
        Err(error) => return (Err(error), None),
    };
    let result = (|| {
        // SmBrep *pMergedOffset = new SmBrep(); SetTolerance(original tolerance);
        // m_bEditingEnabled = TRUE
        let merged = store.create_brep(context)?;
        store.with_brep_mut(merged, |k, b, _, _| {
            bp2::set_tolerance(k, b, tolerance, true, true)?;
            b.set_editing_enabled(true);
            Ok(())
        })?;
        let merge_tol = store.brep(merged)?.tolerance();
        let angle_tol = 20.0 * PI / 180.0;
        let result =
            store.with_merge(merged, merged, offset_id, merge_tol, angle_tol, |s, id| {
                s.piecewise_merge_skipping(id, Some(&skip), true, true)
            })?;
        Ok((result, store.take_brep(result)?))
    })();
    // m_pOffsetBrep still exists here; the caller cleans it up
    let offset = store.take_brep(offset_id).ok();
    drop(store);
    let result = result.and_then(|(id, merged)| {
        rename_merged_attributes(
            notifications.attributes,
            &merged,
            &|e| identities.topology(id, e),
            offset.as_ref(),
            notifications.result_key_of,
        )?;
        Ok(merged)
    });
    (result, offset)
}

/// (not upstream: upstream's attributes stay on the objects.) The merged B-rep takes
/// the offset temporary's place as the offset's result, which its callers notify under
/// the result identities: the attribute lists of its objects (under the merge's
/// `merged_key_of`) move to `result_key_of`'s names of them, and those of the
/// temporary's objects (the same names) are cleared first, as when upstream's
/// executive deletes `m_pOffsetBrep` (dependent attributes die with their last user).
fn rename_merged_attributes(
    attributes: &mut AttributeStore,
    merged: &Brep,
    merged_key_of: &dyn Fn(EntityRef) -> ObjectRef,
    offset: Option<&Brep>,
    result_key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    let renamed: BTreeMap<ObjectKey, ObjectKey> = super::attributes::live_entities(merged)
        .into_iter()
        .map(|e| (merged_key_of(e).key, result_key_of(e).key))
        .collect();
    let gone: BTreeSet<ObjectKey> = offset
        .map(super::attributes::live_entities)
        .unwrap_or_default()
        .into_iter()
        .map(|e| result_key_of(e).key)
        .collect();
    attributes.retain_rekeyed(&|key| match renamed.get(&key) {
        Some(&new) => Some(new),
        None if gone.contains(&key) => None,
        None => Some(key),
    })
}
