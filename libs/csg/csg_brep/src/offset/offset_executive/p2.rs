//! Solid offset, extended offset, shell ownership and the full public API prefixes.
//!
//! Upstream: `source/SMLib/src/SmOffsetExecutive.cpp` (100–129, 146–359,
//! 379–1053, 1062–1924), `source/SMLib/inc/SmOffsetExecutive.h`,
//! `source/SMLib/src/SmBrep.cpp` (493–537, empty-constructor region metadata),
//! `source/SM_API/src/SmApiBrep.cpp` (2350–2431, 2438–2463).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`offset_brep`] | `SmOffsetExecutive::OffsetBrep` |
//! | [`OffsetExecutive::shell_brep`], [`ShellOutcome`] | both `SmOffsetExecutive::ShellBrep` overloads, including input ownership |
//! | [`OffsetExecutive::do_solid_offset`] | `SmOffsetExecutive::DoSolidOffset` |
//! | [`OffsetExecutive::do_extended_offset`] | `SmOffsetExecutive::DoExtendedOffset` |
//! | `order_edgeuses_by_brep` (private) | `sm_OrderEdgeusesByBrep` |
//! | [`api_shell_brep_full`], [`ShellCopyIdentities`] | `SmApiShellBrepFull` (and its working copy's identities) |
//! | [`api_offset_brep_full`] | `SmApiOffsetBrepFull` |
//! | [`offset_brep_simple`], [`shell_brep_simple`] | (not upstream) the API calls with their own notification identities |
//! | [`faceuse_offset`] | `SmOffsetGeometryCreation::FaceuseOffset` over the owned B-rep |
//!
//! Deviations: original B-reps and the kernel are explicit arguments, not retained
//! pointers. Options replace scalar flag lists; owned results replace output pointers.
//! A shell outcome returns any separately retained input even on failure; dropping it
//! is the caller's choice. The pure shell API uses existing origin-qualified
//! `FaceKey`/`BrepRef` handles, with caller-unique `BrepId`s, not local face ids alone.
//! Its private working copy is notified under caller-supplied [`ShellCopyIdentities`]
//! (upstream allocates fresh objects in the API context).
//! Static entry points take non-null references/owned values (null is undefined
//! upstream); member null is native 1006, API null/invalid selection is native 1007.
//! Offset/member inputs are mutable so existing queries can populate lazy UV trim
//! geometry in the same arena; this adds no constructive capability or topology copy.
//! `sm_OrderEdgeusesByBrep` (void upstream) propagates a dangling-id error from its
//! face and edge-use queries; only a corrupt B-rep reaches it.
//! Source faces use the native owning-list order. Temporary allocation uses the real
//! empty constructor's void infinite region, tolerance, editing flag and caller-reserved
//! result key. No offset geometry is fabricated. Copied faces pass the actual region
//! and caller-owned notification stores to brep p7; mapped edges use p1's common
//! notification-aware builder exactly once, with the already-claimed traversal mark.
//! Face geometry and SSI are obtained through the owned cross-arena face adapter;
//! later topology construction and merge stages retain their explicit boundaries.
//! `DoSolidOffset` marks original edge uses in a set local to its edge loop (the
//! claimed mark is still acquired and released); the vertex caps reuse the last
//! copied shell face's loop points, as upstream's shared `sLoopPoints` array.
//! Continuations after missing calls are documented there, including ignored-status
//! calls. The executive's temporary Boolean scope and mark locks are restored on
//! every `Result` exit. The shell difference delegates native context/error state
//! to the real Boolean adapter; late deferred errors report actual operand ownership,
//! not parity with the native ownership flag.
//! Every entry point carries the source B-rep's own attributes ([`Brep::add_attribute`])
//! through the caller's stores to the returned B-rep (`attributes`).
//! There is no new recursion; all owned loops are bounded by existing arena/relation lengths.

use std::collections::BTreeSet;

use super::attributes;
use super::p1::{OffsetExecutive, OffsetOperation};
use crate::boolean::merge::p4::{
    BooleanKernels, KernelManifoldIntersector, ManifoldKernelAdapter, ManifoldNotificationKeys,
    ManifoldState,
};
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::a_object::BooleanOperation;
use crate::geom::attribute::AttributeStore;
use crate::geom::context::{with_mark_lock, MarkType, ALL_MARKS};
use crate::geom::extent2d::Extent2d;
use crate::geom::object::{FaceKey, NotifyOperation, ObjectRef};
use crate::heal::stitch::p1::{DefaultStitchCallback, Stitch};
use crate::heal::stitch::p2 as stitch;
use crate::heal::stitch::p2::MinGapRequest;
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::api_queries::{brep_is_manifold_solid, BrepRef};

use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p3 as bp3;
use crate::topology::brep::p4::{shrink_geometry, stitch_faces};
use crate::topology::brep::p7::{make_face_with_curves_with_kernel_context, FaceCurvesInput};
use crate::topology::proto_topology::p1::CopyBrepMap;

#[cfg(test)]
use crate::geom::attribute::ObjectKey;
use crate::offset::offset_geometry_creation::make_edges_on_analytics_exact;
use crate::sweep::topology_sweep::store::SweepIdentities;
use crate::topology::edge::{NotificationStores, NotifyState};
use crate::topology::edgeuse::{p1 as eu, p2 as eu2};
use crate::topology::face::p2 as face;
#[cfg(test)]
use crate::topology::topology::Topology;
use crate::topology::{Brep, BrepId, EdgeId, EdgeuseId, FaceId, FaceuseId, RegionId};

const OP_OFFSET: &str = "offset::offset_executive::p2::offset_brep";
const OP_SHELL: &str = "offset::offset_executive::p2::shell_brep";
const OP_SOLID: &str = "offset::offset_executive::p2::do_solid_offset";
const OP_EXTENDED: &str = "offset::offset_executive::p2::do_extended_offset";
const OP_API_SHELL: &str = "offset::offset_executive::p2::api_shell_brep_full";
const OP_API_OFFSET: &str = "offset::offset_executive::p2::api_offset_brep_full";

/// Caller-owned notification capability for an offset operation.
///
/// Source and result identities must be disjoint, even when their arena-local ids
/// coincide. Register existing source/result topology records under these keys;
/// constructors register newly created topology in the same stores. Reserve
/// `result_brep` before calling: it is assigned to the temporary before construction.
/// The context always comes from the supplied `Kernel`, never from a substitute.
pub struct OffsetNotifications<'a> {
    /// Actual topology metadata, shared by source and result notifications.
    pub records: &'a mut crate::topology::topology::NotifyRecords,
    /// Actual attributes associated with those object identities.
    pub attributes: &'a mut AttributeStore,
    /// Object identities in the source arena.
    pub original_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// Disjoint object identities in the result arena, including newly made entities.
    pub result_key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// Identities of source edge curves (the second argument is the curve type).
    /// Shell Booleans may split edges in either operand, not only the offset.
    pub original_curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
    /// Identities of result edge curves (the second argument is the curve type).
    pub result_curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
    /// Reserved region source-list handles for Boolean propagation. The first
    /// argument selects the result arena (`true`) or original arena (`false`);
    /// the returned pair identifies primary-source and other-source lists.
    /// Handles have type code zero and distinct keys disjoint from every object
    /// and other list handle in the shared stores. See `ManifoldNotificationKeys`.
    pub boolean_source_lists: &'a dyn Fn(bool, RegionId) -> [ObjectRef; 2],
    /// Reserved, caller-owned identity of the result B-rep.
    pub result_brep: ObjectRef,
    /// Identities of the B-reps the merging `PiecewiseMerge` creates (its merge
    /// B-rep, which becomes the result, and `SmMerge`'s stand-alone face B-reps), by
    /// the handles they receive in that merge's `SweepStore`: handle 0 is the offset
    /// temporary (which keeps the result identities above and is not asked for), each
    /// new B-rep takes the next handle. They must be disjoint from every other
    /// identity in the shared stores. `None` refuses a merging `PiecewiseMerge`.
    pub merge_identities: Option<&'a dyn SweepIdentities>,
}

impl core::fmt::Debug for OffsetNotifications<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OffsetNotifications")
            .field("result_brep", &self.result_brep)
            .finish_non_exhaustive()
    }
}

/// Signed distance and independent convex/concave offset choices.
///
/// The all-zero default matches the executive's false flags; these arguments have
/// no defaults on the upstream static functions. Negative zero is not an inset.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OffsetOptions {
    /// `dOffsetDistance`: negative for an inset; the creator receives its absolute value.
    pub distance: f64,
    /// `bDoExtendedOffset`: extend/intersect convex offsets instead of rounding them.
    pub extend_convex_edges: bool,
    /// `bDoSelfInt`: merge and trim concave self-intersections instead of stitching.
    pub merge_results: bool,
}

/// Shell options, including whether to join sheet boundaries to their offsets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShellOptions {
    /// Distance and convex/concave offset flags, forwarded without modification.
    pub offset: OffsetOptions,
    /// `bCreateOffsetSolid`: construct ruled side walls at lamina edges.
    pub create_offset_solid: bool,
    /// Opt-in (default `false`, SMLib's behaviour): when a sheet is thickened
    /// (`create_offset_solid` on a non-manifold input), consecutive side walls that lie
    /// on one plane come back as one face, as the source fork does
    /// (source issue #978): the edges between two walls are removed after the
    /// stitch, each planar wall first going onto an exact plane. The sheet's own splits
    /// and their offsets stay.
    pub merge_sheet_side_walls: bool,
}

/// Options for the direct solid-offset member, not for the static entry points.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SolidOffsetOptions {
    /// `bSkipStitchMerge` (default false). Extended dispatch ignores this flag upstream.
    pub skip_stitch_merge: bool,
}

/// Owned shell result and the input allocation, if native code would retain it.
///
/// `retained_input` is present on failures before consumption and on the successful
/// non-manifold/no-side-walls return. It is absent if the input was consumed or
/// returned as the result: the same B-rep is never represented by two owned values.
/// In particular a positive shell Boolean can consume the input before a late
/// deferred cache/transform error; its actual operand slot controls this report.
/// The real manifold query and strict radius guard run before offset construction.
#[derive(Debug)]
pub struct ShellOutcome {
    /// Native shell status and, on success, its owned B-rep.
    pub result: Result<Brep>,
    /// Input still owned separately by the caller, including on early errors.
    pub retained_input: Option<Brep>,
}

impl ShellOutcome {
    /// Whether the caller no longer owns the input separately.
    ///
    /// On native-comparable exits this matches `rbInputOwnershipTransferred`.
    /// A deferred exit reports the actual retained Rust allocation, not parity
    /// with a native flag: native code would continue through the missing callee.
    pub const fn input_ownership_transferred(&self) -> bool {
        self.retained_input.is_none()
    }
}

/// Opt-in (WR-545; the fork's source fork@cd85d34): with
/// `Kernel::fixes.shell_edges_on_analytics_exact`, the last step of `ShellBrep`,
/// `SmOffsetGeometryCreation::MakeEdgesOnAnalyticsExact(crContext, rpShellBrep)`, on
/// the shell's result under its notification identities. An error drops the shell.
fn make_shell_edges_exact(
    kernel: &mut Kernel,
    shell: &mut Brep,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<()> {
    if !kernel.fixes.shell_edges_on_analytics_exact {
        return Ok(());
    }
    let mut stores = NotificationStores {
        records: &mut *notifications.records,
        attributes: &mut *notifications.attributes,
        key_of: notifications.result_key_of,
    };
    make_edges_on_analytics_exact(kernel, shell, &mut stores)
}

/// Static offset entry point, with a non-null original and an owned result.
///
/// Only ordinary inner failures collapse to native `SM_ERR` (1001).
/// [`SolidError::Unported`] propagates so missing dependencies remain deferred.
/// Mutable input permits lazy UV trim geometry updates in the caller's arena.
pub fn offset_brep(
    kernel: &mut Kernel,
    original: &mut Brep,
    options: OffsetOptions,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<Brep> {
    // (not upstream) the source's own attributes go to the caller's stores, as
    // upstream's live on the objects the offset copies
    attributes::seed_attributes(
        original,
        notifications.original_key_of,
        notifications.attributes,
    )?;
    let original_key = original.key();
    let mut executive = configured_executive(original, options);
    let mut result = collapse_offset_status(executive.do_solid_offset(
        kernel,
        Some(original),
        SolidOffsetOptions::default(),
        notifications,
    ))?;
    // (not upstream) the attributes the notifications left on the result's objects
    attributes::finish_attributes(&mut result, original_key, notifications)?;
    Ok(result)
}

impl OffsetExecutive {
    /// Ownership-reporting static shell entry point (the thin overload needs no
    /// separate Rust method). `faces` are ids in the owned input, not in a copy.
    ///
    /// The manifold query deliberately precedes the strict distance guard. No
    /// caller-visible input is discarded on an early error. Unlike static offset,
    /// ordinary inner errors are returned unchanged. The owned input is borrowed
    /// mutably for lazy UV trim geometry; any retained input keeps those updates.
    pub fn shell_brep(
        kernel: &mut Kernel,
        original: Brep,
        options: ShellOptions,
        faces: &[FaceId],
        notifications: &mut OffsetNotifications<'_>,
    ) -> ShellOutcome {
        let original_key = original.key();
        let mut input = Some(original);
        let result = (|| {
            let original = required(input.as_mut(), OP_SHELL)?;
            // (not upstream) the source's own attributes go to the caller's stores, as
            // upstream's live on the objects the shell copies and edits
            attributes::seed_attributes(
                original,
                notifications.original_key_of,
                notifications.attributes,
            )?;
            // cpp:206–215: do not move the guard before this real query.
            let radius = options.offset.distance.abs();
            let tolerance = original.tolerance();
            let input_is_manifold = brep_is_manifold_solid(original)?;
            check_shell_radius(radius, tolerance)?;

            // cpp:217–251: the helper's scope drops any separately retained
            // executive temporary before querying the result's manifold status.
            let mut shell = run_shell_offset(kernel, original, options, faces, notifications)?;
            // Opt-in (WR-454, #380; source fork@fbe43bb): an inset of a solid whose
            // offset leaves nothing has no cavity to take away (the difference would hand
            // the input back unhollowed); an offset that is no closed solid bounds none.
            if kernel.fixes.shell_empty_cavity_fails
                && options.offset.distance < 0.0
                && input_is_manifold
            {
                if bp1::num_faces(&shell) == 0 {
                    return Err(invalid_input(OP_SHELL));
                }
                if !brep_is_manifold_solid(&shell)? {
                    return Err(SolidError::sm_err(OP_SHELL));
                }
            }
            if !brep_is_manifold_solid(&shell)? && !options.create_offset_solid {
                return Ok(shell);
            }
            OffsetExecutive::replace_implicit_offsets(
                kernel,
                &mut shell,
                tolerance,
                &mut NotificationStores {
                    records: &mut *notifications.records,
                    attributes: &mut *notifications.attributes,
                    key_of: notifications.result_key_of,
                },
            )?;

            if options.create_offset_solid && !input_is_manifold {
                // Continuation cpp:300–325: only after MergeBrep succeeds is the
                // original consumed. StitchFaces starts at tolerance / 10.0 and
                // runs at most four times, multiplying by 10.0 after each call;
                // stop when no lamina edges remain or the next tolerance exceeds
                // abs(distance) / 2.0. Preserve retained input on MergeBrep error,
                // but report ownership transferred on a later stitch error.
                // the sheet's faces as copied into the shell (#978, #1599; only with an opt-in)
                let side_walls =
                    options.merge_sheet_side_walls || kernel.fixes.thicken_planar_side_walls;
                let mut sheet_faces: Vec<FaceId> = Vec::new();
                {
                    let source = required(input.as_ref(), OP_SHELL)?;
                    let original_key_of = notifications.original_key_of;
                    let mut stores = NotificationStores {
                        records: &mut *notifications.records,
                        attributes: &mut *notifications.attributes,
                        key_of: notifications.result_key_of,
                    };
                    // SER(pBrepShell->MergeBrep(*pBrepToShell)): default maps (NULL)
                    // and bAddAnalytics = TRUE; the opt-in keeps the sheet-to-copy map
                    // (the fork's MergeBrep(*pBrepToShell, NULL, &sSheetToCopy): original to copy)
                    let mut sheet_to_copy = CopyBrepMap::new();
                    let reverse_map = side_walls.then_some(&mut sheet_to_copy);
                    NotifyState::from_kernel_stores(&mut stores).with_kernel_env(
                        kernel,
                        |kernel, env| {
                            bp3::merge_brep(
                                kernel,
                                env,
                                &mut shell,
                                source,
                                original_key_of,
                                None,
                                reverse_map,
                                true,
                            )
                        },
                    )?;
                    if side_walls {
                        for f in bp1::faces(source, None)? {
                            match sheet_to_copy.get_at(Some(EntityRef::Face(f)))? {
                                Some(EntityRef::Face(copy)) => sheet_faces.push(copy),
                                _ => {
                                    return Err(SolidError::null_pointer(OP_SHELL));
                                }
                            }
                        }
                    }
                }
                // delete pBrepToShell; rbInputOwnershipTransferred = TRUE
                input = None;

                // Stitch lamina edges, for 4 progressively larger tolerances (or up
                // to half the offset distance). SER_DELETE drops the shell.
                let mut lamina = 1;
                let mut stitch_tol_3d = tolerance / 10.0;
                let mut ii = 0;
                while ii < 4 && lamina > 0 {
                    lamina =
                        stitch_faces(kernel, &mut shell, stitch_tol_3d, None, false)?.lamina_edges;
                    stitch_tol_3d *= 10.0;
                    if stitch_tol_3d > options.offset.distance.abs() / 2.0 {
                        break;
                    }
                    ii += 1;
                }
                if side_walls {
                    // SER_DELETE: an error drops the shell
                    merge_sheet_side_walls(
                        kernel,
                        &mut shell,
                        &sheet_faces,
                        options.merge_sheet_side_walls,
                        notifications,
                    )?;
                }
                make_shell_edges_exact(kernel, &mut shell, notifications)?;
                return Ok(shell);
            }

            let mut shell = shell_difference(
                kernel,
                &mut input,
                shell,
                options.offset.distance,
                tolerance,
                notifications,
            )?;
            make_shell_edges_exact(kernel, &mut shell, notifications)?;
            Ok(shell)
        })();
        // (not upstream) the attributes the notifications left on the result's objects
        let result = result.and_then(|mut shell| {
            attributes::finish_attributes(&mut shell, original_key, notifications)?;
            Ok(shell)
        });
        ShellOutcome {
            result,
            retained_input: input,
        }
    }

    /// Builds a rounded solid-offset prefix, or dispatches to extended offset first.
    ///
    /// A null original returns native 1006. With extended dispatch the Boolean
    /// context flag is untouched, and `skip_stitch_merge` is not examined. Otherwise
    /// the flag is true throughout the operation and restored on every result.
    /// Even an empty input reaches `SmBrep::ShrinkGeometry`, before the skip flag.
    /// Mutable input allows copied-face queries to create lazy UV trim geometry in
    /// the original arena; the constructive prefix remains partial.
    pub fn do_solid_offset(
        &mut self,
        kernel: &mut Kernel,
        original: Option<&mut Brep>,
        options: SolidOffsetOptions,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<Brep> {
        // cpp:411–418: dispatch must precede NER and the Boolean scope.
        if self.extend_convex_edges() {
            return self.do_extended_offset(kernel, original, notifications);
        }
        let original = required(original, OP_SOLID)?;
        with_doing_boolean(kernel, |kernel| {
            self.allocate_offset_brep(notifications)?;
            // `sLoopPoints` keeps the last copied shell face's vertex-loop points
            // (CreateCurvesFromFace resets it); the vertex caps reuse it below.
            let mut loop_points = Vec::new();
            let faces = bp1::faces(original, None)?;
            for id in faces {
                if self.shell_faces().contains(&id) {
                    loop_points =
                        self.copy_shell_face_prefix(kernel, original, id, false, notifications)?;
                } else {
                    self.offset_face_prefix(kernel, original, id, false, notifications)?;
                }
            }

            // cpp:608 is unconditional, even with zero faces and skip=true.
            let mut temporary = required(self.take_offset_brep(), OP_SOLID)?;
            let status = shrink_geometry(kernel, &mut temporary);
            self.set_offset_brep(temporary);
            status?;

            // cpp:651–672: every edge use of every original edge, in GetEdges and
            // GetEdgeuses order, once the use relation holds any pair at all
            // (upstream tests `m_vEUToEU.GetAllSeconds`, not this use's seconds).
            let any_mapped = !self.mapped_original_edgeuses().is_empty();
            let mut offset_edgeuses = Vec::new();
            for edge in bp1::edges(original, None)? {
                for edgeuse in crate::topology::edge::edgeuses(original, edge, None, None)? {
                    if any_mapped {
                        offset_edgeuses.push(edgeuse);
                    }
                }
            }

            // cpp:684–722: the claimed mark only tags original edge uses here.
            with_mark_lock(kernel, ALL_MARKS, |kernel, mark_type| {
                let mut marked = BTreeSet::new();
                for &edgeuse in &offset_edgeuses {
                    marked.insert(edgeuse);
                    let mut radial = required(eu::radial(original, edgeuse)?, OP_SOLID)?;
                    if marked.contains(&radial) {
                        continue;
                    }
                    // FindElement of a null mate finds nothing.
                    let mate = eu::mate(original, radial)?;
                    if let Some(mate) = mate.filter(|mate| {
                        offset_edgeuses.contains(mate) && !offset_edgeuses.contains(&radial)
                    }) {
                        radial = mate;
                        if marked.contains(&radial) {
                            continue;
                        }
                    }
                    // SE upstream: propagate Unported, ignore other errors.
                    crate::error::ignore_status(self.mapped_edge_prefix(
                        kernel,
                        original,
                        edgeuse,
                        radial,
                        mark_type,
                        &marked,
                        notifications,
                    ))?;
                }
                Ok(())
            })??;

            // cpp:745–753: ruled side walls for lamina edges (SE).
            if self.create_offset_solid() {
                for &edgeuse in &offset_edgeuses {
                    let edge = required(eu::edge(original, edgeuse)?, OP_SOLID)?;
                    if eu::edge_is_lamina(original, edge)? {
                        let mut stores = NotificationStores {
                            records: &mut *notifications.records,
                            attributes: &mut *notifications.attributes,
                            key_of: notifications.result_key_of,
                        };
                        crate::error::ignore_status(self.build_side_wall_of_edgeuse_with(
                            kernel,
                            original,
                            Some(edgeuse),
                            Some(&mut NotifyState::from_kernel_stores(&mut stores)),
                        ))?;
                    }
                }
            }

            // cpp:776–846: vertex caps, in GetVertices order (SER).
            for vertex in bp1::vertices(original, None)? {
                let cap = self.build_offset_of_vertex(kernel, original, Some(vertex))?;
                let Some(surface) = cap.surface else {
                    continue;
                };
                let mut stores = NotificationStores {
                    records: &mut *notifications.records,
                    attributes: &mut *notifications.attributes,
                    key_of: notifications.result_key_of,
                };
                let new_face = self.make_cap_face(
                    kernel,
                    surface,
                    cap.trim_curves,
                    cap.curve_orientations,
                    &loop_points,
                    &mut NotifyState::from_kernel_stores(&mut stores),
                )?;
                self.relate_vertex_face(vertex, new_face);
            }

            // cpp:874–882: hand over the temporary unstitched.
            if options.skip_stitch_merge {
                return required(self.take_offset_brep(), OP_SOLID);
            }

            // cpp:887–902: a failed merge drops its partial result.
            let mut result = self.piecewise_merge(kernel, original, Some(&mut *notifications))?;
            // cpp:965: the stitch status is only asserted.
            crate::error::ignore_status(stitch::stitch_into_solid(kernel, &mut result))?;
            // cpp:1009–1027: label the regions only when MakeManifold succeeds.
            if crate::error::ignored(crate::topology::brep::p5::make_manifold(
                kernel,
                &mut result,
                None,
                false,
            ))?
            .is_some()
            {
                for region in bp1::regions(&result, None)? {
                    let is_void = Some(region) == result.infinite_region();
                    result.region_mut(region)?.is_void = i32::from(is_void);
                }
            }
            // cpp:1046: delete the temporary, if PiecewiseMerge left one.
            let _ = self.take_offset_brep();
            Ok(result)
        })
    }

    /// Builds an extended-offset prefix with the original's face-selection order.
    ///
    /// Unlike rounded offset, this does not set `doing_boolean`. Empty inputs still
    /// claim/release a traversal mark, remove excess faces when requested and call
    /// p1's real `piecewise_merge`; they never bypass stitching with an empty result.
    /// Mutable input keeps lazy UV trim geometry from copied-face and tangency
    /// queries in the original arena, without adding constructive capability.
    pub fn do_extended_offset(
        &mut self,
        kernel: &mut Kernel,
        original: Option<&mut Brep>,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<Brep> {
        let original = required(original, OP_EXTENDED)?;
        self.allocate_offset_brep(notifications)?;
        let faces = bp1::faces(original, None)?;
        for id in faces {
            if self.shell_faces().contains(&id) {
                // DoExtendedOffset builds no vertex caps from these loop points.
                let _ = self.copy_shell_face_prefix(kernel, original, id, true, notifications)?;
            } else {
                self.offset_face_prefix(kernel, original, id, true, notifications)?;
            }
        }

        let mapped = order_edgeuses_by_brep(original, &self.mapped_original_edgeuses())?;
        with_mark_lock(kernel, ALL_MARKS, |kernel, mark_type| {
            // The same claimed mark reaches both mapped-edge passes. Source and
            // result keys stay disjoint through p1's synchronization and Copy calls.
            let mut marked = BTreeSet::new();
            // Not upstream (WR-346): mate the faces beside each collapsed face across it,
            // the round's rails becoming one sharp edge (their ends are trimmed by
            // TrimExtendedEdge, which also looks across collapsed faces).
            let mut unbridged = 0_usize;
            if !self.collapsed_faces.is_empty() {
                for &edgeuse in &mapped {
                    let Some(radial) = eu::radial(original, edgeuse)? else {
                        continue;
                    };
                    if radial == edgeuse
                        || !self.radial_face_collapsed(original, radial)?
                        || !eu2::is_tangent_sector(
                            kernel,
                            original,
                            radial,
                            self.tangency_tol_degrees(),
                            false,
                            true,
                        )?
                        || marked.contains(&edgeuse)
                    {
                        continue;
                    }
                    let across =
                        self.edgeuse_across_collapsed_face(kernel, original, Some(radial), None)?;
                    let offset_edgeuse = self.offset_edgeuse(edgeuse);
                    let offset_across = across.and_then(|a| self.offset_edgeuse(a));
                    let (Some(across), Some(offset_edgeuse), Some(offset_across)) =
                        (across, offset_edgeuse, offset_across)
                    else {
                        unbridged += 1;
                        continue;
                    };
                    let mut stores = NotificationStores {
                        records: &mut *notifications.records,
                        attributes: &mut *notifications.attributes,
                        key_of: notifications.original_key_of,
                    };
                    let mated = crate::error::ignored(self.mate_edges_across_with_marked(
                        kernel,
                        original,
                        edgeuse,
                        across,
                        offset_edgeuse,
                        offset_across,
                        mark_type,
                        &marked,
                        &mut stores,
                        notifications.result_key_of,
                    ))?;
                    if mated.is_none() {
                        unbridged += 1;
                        continue;
                    }
                    marked.insert(edgeuse);
                    marked.insert(required(eu::mate(original, edgeuse)?, OP_EXTENDED)?);
                    marked.insert(across);
                    marked.insert(required(eu::mate(original, across)?, OP_EXTENDED)?);
                }
            }
            if self.extend_convex_edges() {
                for &edgeuse in &mapped {
                    let radial = required(eu::radial(original, edgeuse)?, OP_EXTENDED)?;
                    // (WR-346: mated across it above)
                    if self.radial_face_collapsed(original, radial)? {
                        continue;
                    }
                    if marked.contains(&radial) {
                        continue;
                    }
                    let Some(radial) = select_radial(original, radial, &mapped, &marked)? else {
                        continue;
                    };
                    if eu2::is_tangent_sector(
                        kernel,
                        original,
                        edgeuse,
                        self.tangency_tol_degrees(),
                        false,
                        true,
                    )? {
                        self.mapped_edge_prefix(
                            kernel,
                            original,
                            edgeuse,
                            radial,
                            mark_type,
                            &marked,
                            notifications,
                        )?;
                        marked.insert(radial);
                        marked.insert(required(eu::mate(original, radial)?, OP_EXTENDED)?);
                        marked.insert(edgeuse);
                        marked.insert(required(eu::mate(original, edgeuse)?, OP_EXTENDED)?);
                    }
                }
            }

            for &edgeuse in &mapped {
                let mut radial = required(eu::radial(original, edgeuse)?, OP_EXTENDED)?;
                // (WR-346: mated across it above)
                if self.radial_face_collapsed(original, radial)? {
                    continue;
                }
                marked.insert(edgeuse);
                if marked.contains(&radial) {
                    continue;
                }
                let edge = required(eu::edge(original, edgeuse)?, OP_EXTENDED)?;
                if !eu::edge_is_lamina(original, edge)? {
                    let Some(selected) = select_radial(original, radial, &mapped, &marked)? else {
                        continue;
                    };
                    radial = selected;
                }
                self.mapped_edge_prefix(
                    kernel,
                    original,
                    edgeuse,
                    radial,
                    mark_type,
                    &marked,
                    notifications,
                )?;
            }

            // Not upstream (WR-346): the offset of an edge along a collapsed face's end now
            // runs from the corner where the mated faces meet back to that corner: squeeze
            // it out. A collapsed face that could not be bridged leaves the offset open:
            // refuse it rather than build a body with holes.
            if !self.collapsed_faces.is_empty() {
                for &edgeuse in &mapped {
                    let Some(radial) = eu::radial(original, edgeuse)? else {
                        continue;
                    };
                    if radial == edgeuse
                        || !self.radial_face_collapsed(original, radial)?
                        || eu2::is_tangent_sector(
                            kernel,
                            original,
                            radial,
                            self.tangency_tol_degrees(),
                            false,
                            true,
                        )?
                    {
                        continue;
                    }
                    if !self.squeeze_collapsed_end_edge(kernel, edgeuse, notifications)? {
                        unbridged += 1;
                    }
                }
                if unbridged > 0 {
                    return Err(SolidError::Upstream {
                        operation: OP_EXTENDED,
                        code: crate::intersect::surface_drop_lib::p3::SM_ERR_DEGENERATE_SURFACE,
                        entity: None,
                    });
                }
            }

            // Not upstream (opt-in, fork source fork 5d428ae and 7ff0a94): a local
            // operation closes the loop gaps its step walls leave at their ends, then splits
            // the edges that run past a step wall's corner
            let step_walls = kernel.fixes.local_operation_step_walls
                && self.operation() == OffsetOperation::LocalOperation;
            if step_walls {
                self.close_loop_gaps(kernel, notifications)?;
                self.split_edges_at_step_corners(kernel, notifications)?;
            }

            // Not upstream (WR-341, source issue #977), opt-in
            // `Kernel::fixes.offset_excess_faces_after_merge`: the excess faces are found after
            // the merge only, where each merged face lies wholly on the offset or wholly in
            // the excess. Here an offset face is not trimmed yet, so one that bounds the
            // offset can have an edge inside the excess and be taken.
            let excess_after_merge = kernel.fixes.offset_excess_faces_after_merge;
            if !excess_after_merge {
                // cpp:1580: retain the same temporary on both success and failure.
                let mut temporary = required(self.take_offset_brep(), OP_EXTENDED)?;
                let status = self.remove_excess_faces(kernel, original, &mut temporary);
                self.set_offset_brep(temporary);
                status?;
            }

            if self.create_offset_solid() {
                for &edgeuse in &mapped {
                    let edge = required(eu::edge(original, edgeuse)?, OP_EXTENDED)?;
                    if eu::edge_is_lamina(original, edge)? {
                        // SE, not SER, upstream: propagate Unported, ignore other errors.
                        let mut stores = NotificationStores {
                            records: &mut *notifications.records,
                            attributes: &mut *notifications.attributes,
                            key_of: notifications.result_key_of,
                        };
                        crate::error::ignore_status(self.build_side_wall_of_edgeuse_with(
                            kernel,
                            original,
                            Some(edgeuse),
                            Some(&mut NotifyState::from_kernel_stores(&mut stores)),
                        ))?;
                    }
                }
            }

            // cpp:1613–1703: retain owning-list order and the active face mark.
            // Temporarily replace each eligible surface before computing unrestricted
            // trim bounds; restore the original before reporting an out-of-domain error.
            self.extend_result_faces_prefix(kernel, mark_type, notifications)?;

            // Not upstream (WR-345, source issue #854), opt-in
            // `Kernel::fixes.offset_resolve_crossing_loops`: an offset face whose loops cross
            // (an inner loop's offset grown past the outer loop's) is rebuilt from the
            // stretches of its loops that bound it, before the merge sees it.
            if kernel.fixes.offset_resolve_crossing_loops {
                self.resolve_crossing_loops(kernel, notifications)?;
            }

            let mut result = self.piecewise_merge(kernel, original, Some(&mut *notifications))?;
            // (WR-341: with the opt-in this also runs for a solid input whatever
            // `m_bCreateOffsetSolid` says: a sheet's side walls, built for an offset solid,
            // run from the sheet itself and would be taken for excess; a solid has none)
            let remove_after_merge = !self.create_offset_solid()
                || (excess_after_merge && brep_is_manifold_solid(original)?);
            if remove_after_merge {
                self.remove_excess_faces(kernel, original, &mut result)?;
            }
            // cpp:1772 is called even after stitching in PiecewiseMerge. Its
            // ordinary status is only asserted, but Unported must still defer.
            // Continuation cpp:1836–1920: MakeManifold status is ignored; on success
            // FindAndSetInfiniteRegion when >1 region (editing temporarily false),
            // refresh regions, set void flags by infinite-region identity, then
            // transfer result ownership and clear the executive temporary.
            if kernel.fixes.offset_stitch_at_body_tolerance {
                // Opt-in (WR-455, #855; source fork@601d804): one pass at the body's
                // tolerance. `StitchIntoSolid` doubles its tolerance until no lamina edge is
                // left, but the lamina edges a merge leaves are the rims of the cut-off
                // pieces `MakeManifold` removes next; escalating, it welds two of them into
                // one edge of a far larger tolerance. (SER: a failure propagates.)
                let mut callback = DefaultStitchCallback::new();
                let mut one_pass = Stitch::new(&mut callback, result.tolerance(), true, false);
                one_pass.fast_edge_compare = true;
                one_pass.split_edges_with_vertices = false;
                one_pass.making_manifold_solid = true;
                one_pass.do_stitching(kernel, &mut result, None, None, MinGapRequest::default())?;
            } else {
                crate::error::ignore_status(stitch::stitch_into_solid(kernel, &mut result))?;
            }
            // MakeManifold exists in p5, but its remaining metadata/topology
            // adapters must still propagate Unported even though native ignores
            // ordinary errors. Do not turn that partial path into empty success.
            if crate::error::ignored(crate::topology::brep::p5::make_manifold(
                kernel,
                &mut result,
                None,
                false,
            ))?
            .is_some()
            {
                if bp1::regions(&result, None)?.len() > 1 {
                    let previous = result.editing_enabled();
                    result.set_editing_enabled(false);
                    let status = stitch::find_and_set_infinite_region(
                        kernel,
                        &mut result,
                        None,
                        bp1::SetRegionVoidType::InfiniteRegionSwap,
                        None,
                    );
                    result.set_editing_enabled(previous);
                    status?;
                }
                for region in bp1::regions(&result, None)? {
                    let is_void = Some(region) == result.infinite_region();
                    result.region_mut(region)?.is_void = i32::from(is_void);
                }
            }
            let _ = self.take_offset_brep();
            Ok(result)
        })?
    }

    /// Not upstream (WR-346): whether the face of original edge use `radial` collapsed.
    fn radial_face_collapsed(&self, original: &Brep, radial: EdgeuseId) -> Result<bool> {
        Ok(eu::face(original, radial)?.is_some_and(|f| self.is_collapsed_face(f)))
    }

    /// Not upstream (WR-346): squeezes out the offset edge of original edge use
    /// `edgeuse` along a collapsed face's end, which the mating shrank to a corner; the
    /// pair leaves the edge use map. `false` (nothing done) when it has no single offset
    /// edge or its ends are not within ten edge tolerances.
    fn squeeze_collapsed_end_edge(
        &mut self,
        kernel: &mut Kernel,
        edgeuse: EdgeuseId,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<bool> {
        let Some(offset_edgeuse) = self.offset_edgeuse(edgeuse) else {
            return Ok(false);
        };
        let mut offset = required(self.take_offset_brep(), OP_EXTENDED)?;
        let status = self.squeeze_collapsed_end_edge_in(
            kernel,
            &mut offset,
            edgeuse,
            offset_edgeuse,
            notifications,
        );
        self.set_offset_brep(offset);
        status
    }

    fn squeeze_collapsed_end_edge_in(
        &mut self,
        kernel: &mut Kernel,
        offset: &mut Brep,
        edgeuse: EdgeuseId,
        offset_edgeuse: EdgeuseId,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<bool> {
        let Some(offset_edge) = eu::edge(offset, offset_edgeuse)? else {
            return Ok(false);
        };
        let v1 = required(
            crate::topology::edge::start_vertex(offset, offset_edge)?,
            OP_EXTENDED,
        )?;
        let v2 = required(
            crate::topology::edge::end_vertex(offset, offset_edge)?,
            OP_EXTENDED,
        )?;
        if v1 == v2
            || offset.vertex(v1)?.point.distance(offset.vertex(v2)?.point)
                > 10.0 * offset.edge(offset_edge)?.tolerance
        {
            return Ok(false);
        }
        self.edgeuse_to_edgeuses
            .remove_pair(edgeuse, offset_edgeuse)?;
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of: notifications.result_key_of,
        };
        NotifyState::from_kernel_stores(&mut stores).with_kernel_env(kernel, |kernel, env| {
            crate::topology::brep::p3::squeeze_edge(kernel, offset, env, offset_edge, v1)
        })?;
        Ok(true)
    }

    fn allocate_offset_brep(&mut self, notifications: &mut OffsetNotifications<'_>) -> Result<()> {
        let mut temporary = Brep::new_with_infinite_region()?;
        temporary.set_key(notifications.result_brep.key);
        let region = required(temporary.infinite_region(), OP_SOLID)?;
        // The empty constructor created this real topology record. Unlike source
        // records and the reserved B-rep key, it did not exist before this call.
        notifications
            .records
            .or_default((notifications.result_key_of)(EntityRef::Region(region)).key);
        temporary.set_tolerance(self.approximation_tol_3d());
        // SmOffsetExecutive.cpp:437 / 1073, after constructing the temporary.
        temporary.set_editing_enabled(true);
        self.set_offset_brep(temporary);
        Ok(())
    }

    fn copy_shell_face_prefix(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        id: FaceId,
        extended: bool,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<Vec<Vec3>> {
        let operation = if extended { OP_EXTENDED } else { OP_SOLID };
        // Both the 3D and UV outputs are requested, in this order, upstream.
        let curves =
            face::create_curves_from_face(kernel, original, id, OrientType::Same, true, true)?;
        // DoSolidOffset reuses this `sLoopPoints` array for its vertex caps.
        let loop_points = curves.loop_points.clone();
        let source_face = original.face(id)?;
        let source_surface = required(source_face.surface.as_ref(), operation)?;
        let surface = source_surface.copy_and_add_analytics(kernel)?;
        let input = FaceCurvesInput {
            loops: curves
                .curve_loops
                .into_iter()
                .map(|count| usize::try_from(count).map_err(|_| invalid_input(operation)))
                .collect::<Result<Vec<_>>>()?,
            curves_3d: curves
                .curves_3d
                .map(|curves| curves.into_iter().map(Some).collect()),
            uv_curves: curves.uv_curves,
            orientations: curves.curve_orients,
            loop_points: curves.loop_points,
        };
        let mut temporary = required(self.take_offset_brep(), operation)?;
        let region = temporary.infinite_region();
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of: notifications.result_key_of,
        };
        let made = make_face_with_curves_with_kernel_context(
            kernel,
            &mut temporary,
            region,
            input,
            surface,
            &source_face.uv_domain,
            OrientType::Same,
            None,
            &|face| (notifications.result_key_of)(EntityRef::Face(face)),
            &mut stores,
        );
        self.set_offset_brep(temporary);
        let made = made?;
        let new_face = required(made.face, operation)?;
        if extended {
            // cpp:1194: Copy notifies the original face with the new result face.
            let mut stores = NotificationStores {
                records: &mut *notifications.records,
                attributes: &mut *notifications.attributes,
                key_of: notifications.original_key_of,
            };
            crate::error::ignore_status(NotifyState::from_kernel_stores(&mut stores).with_env(
                kernel,
                |env| {
                    crate::topology::face::p3::notify(
                        original,
                        id,
                        env,
                        None,
                        NotifyOperation::Copy,
                        [
                            Some((notifications.result_key_of)(EntityRef::Face(new_face))),
                            None,
                            None,
                        ],
                    )
                },
            ))?;
        }
        let mut faceuse = required(face::upward_faceuse(original, id)?, operation)?;
        let shell = required(eu::faceuse_shell(original, faceuse)?, operation)?;
        let region = eu::shell_region(original, shell)?;
        let use_mate = if extended {
            region != original.infinite_region()
        } else {
            !region_is_void(original, required(region, operation)?)?
        };
        if use_mate {
            faceuse = required(eu::faceuse_mate(original, faceuse)?, operation)?;
        }
        // cpp:1196–1207 applies inset only to extended copied faces.
        if extended && self.inset() {
            faceuse = required(eu::faceuse_mate(original, faceuse)?, operation)?;
        }
        // cpp:498–506: copied rounded faces do not apply inset.
        self.relate_faceuse(faceuse, new_face);
        self.map_edgeuses_with_kernel_context(
            kernel,
            original,
            faceuse,
            new_face,
            &mut NotificationStores {
                records: &mut *notifications.records,
                attributes: &mut *notifications.attributes,
                key_of: notifications.original_key_of,
            },
            notifications.result_key_of,
        )?;
        Ok(loop_points)
    }

    fn offset_face_prefix(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        id: FaceId,
        extended: bool,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let operation = if extended { OP_EXTENDED } else { OP_SOLID };
        let faceuse = select_offset_faceuse(original, id, extended, self.inset(), operation)?;
        let (mut surface, ssi) = if self.local_faces.is_empty() {
            let creator = crate::offset::offset_geometry_creation::OffsetGeometryCreation::new(
                self.offset_distance(),
                self.approximation_tol_3d(),
            );
            super::face::build(kernel, &creator, original, faceuse, true)?
        } else {
            // SmLocalOperationGeometryCreation::FaceuseOffset: the surface given per face. The
            // creator object decides upstream (virtual call), not `m_eOffsetOperation`.
            super::face::build_local(original, &self.local_faces, &self.local_surfaces, faceuse)?
        };
        // Not upstream (WR-346, source issue #397), opt-in
        // `Kernel::fixes.offset_collapsed_rounds_mated`: an inset face whose offset collapses (a
        // convex round thinner than the wall) gets no offset face; the faces beside it are
        // mated across it in `do_extended_offset`, as at a sharp edge.
        if extended
            && surface.is_none()
            && kernel.fixes.offset_collapsed_rounds_mated
            && self.inset()
            && self.extend_convex_edges()
        {
            self.collapsed_faces.push(id);
            return Ok(());
        }
        if self.skip_offset_face_geometry(extended, surface.is_some(), ssi.curves_3d.len())? {
            return Ok(());
        }
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of: notifications.original_key_of,
        };
        self.build_offset_of_faceuse_with_kernel_context(
            kernel,
            original,
            faceuse,
            &mut surface,
            &ssi,
            &mut stores,
            notifications.result_key_of,
            notifications.result_curve_ref,
        )
    }

    /// Applies native null-surface and SSI policy using this executive's flags.
    fn skip_offset_face_geometry(
        &self,
        extended: bool,
        has_surface: bool,
        ssi_count: usize,
    ) -> Result<bool> {
        if !has_surface {
            return if extended {
                Err(SolidError::sm_err(OP_EXTENDED))
            } else {
                Ok(true)
            };
        }
        if extended
            && ((self.merge_results() && ssi_count > 0)
                || (self.extend_convex_edges() && ssi_count > 5))
        {
            return Err(SolidError::sm_err(OP_EXTENDED));
        }
        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn mapped_edge_prefix(
        &mut self,
        kernel: &mut Kernel,
        original: &mut Brep,
        edgeuse: EdgeuseId,
        radial: EdgeuseId,
        mark_type: MarkType,
        marked: &BTreeSet<EdgeuseId>,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of: notifications.original_key_of,
        };
        self.build_offset_of_edgeuses_with_marked(
            kernel,
            original,
            edgeuse,
            radial,
            mark_type,
            marked,
            &mut stores,
            notifications.result_key_of,
            notifications.result_curve_ref,
        )
    }

    fn extend_result_faces_prefix(
        &mut self,
        kernel: &mut Kernel,
        mark_type: MarkType,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let mut temporary = required(self.take_offset_brep(), OP_EXTENDED)?;
        let status = (|| {
            let faces = bp1::faces(&temporary, None)?;
            for id in faces {
                let key = (notifications.result_key_of)(EntityRef::Face(id));
                let record = notifications.records.or_default(key.key);
                if record.is_marked(&kernel.context, mark_type) {
                    continue;
                }
                if self.closing_faces().contains(&id) {
                    continue;
                }
                record.mark(Some(&kernel.context), mark_type);
                let surface = required(temporary.face(id)?.surface.as_ref(), OP_EXTENDED)?;
                let original_domain = surface.natural_uv_domain_with_nurb(kernel)?;
                // Face-owned surfaces use the caller's result-arena identities.
                let extension = self
                    .get_create_extended_surface(kernel, key.key.0, surface)?
                    .clone();
                let extension_domain = extension.natural_uv_domain_with_nurb(kernel)?;
                // cpp:1639: UNSURE retains the original; FALSE suppresses UV trim
                // creation. The optional modified-face list is not requested.
                let (original_surface, _) =
                    crate::geom::context::Context::with_notification_context(
                        kernel,
                        |kernel, context| {
                            crate::topology::brep::p2::replace_surface(
                                kernel,
                                &mut temporary,
                                &mut crate::topology::edge::NotifyEnv {
                                    records: &mut *notifications.records,
                                    attributes: &mut *notifications.attributes,
                                    context: Some(context),
                                    key_of: notifications.result_key_of,
                                },
                                id,
                                extension,
                                key,
                                crate::topology::brep::p2::OldSurface::Keep,
                                false,
                            )
                        },
                    )??;
                let trim_domain =
                    crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves(
                        kernel,
                        &mut temporary,
                        id,
                        false,
                        false,
                        false,
                    )?
                    .domain;
                let original_surface = required(original_surface, OP_EXTENDED)?;
                let mut original_surface = Some(original_surface);
                choose_extended_domain(
                    &trim_domain,
                    &original_domain,
                    &extension_domain,
                    crate::math::EFF_ZERO,
                    || {
                        let original_surface = required(original_surface.take(), OP_EXTENDED)?;
                        let restored = crate::geom::context::Context::with_notification_context(
                            kernel,
                            |kernel, context| {
                                crate::topology::brep::p2::replace_surface(
                                    kernel,
                                    &mut temporary,
                                    &mut crate::topology::edge::NotifyEnv {
                                        records: &mut *notifications.records,
                                        attributes: &mut *notifications.attributes,
                                        context: Some(context),
                                        key_of: notifications.result_key_of,
                                    },
                                    id,
                                    original_surface,
                                    key,
                                    crate::topology::brep::p2::OldSurface::Keep,
                                    false,
                                )
                            },
                        )?;
                        drop(restored);
                        Ok(())
                    },
                )?;
            }
            Ok(())
        })();
        self.set_offset_brep(temporary);
        status
    }
}

fn select_offset_faceuse(
    original: &Brep,
    face: FaceId,
    extended: bool,
    inset: bool,
    operation: &'static str,
) -> Result<FaceuseId> {
    let mut faceuse = required(face::upward_faceuse(original, face)?, operation)?;
    let shell = required(eu::faceuse_shell(original, faceuse)?, operation)?;
    let region = eu::shell_region(original, shell)?;
    // Native predicates differ: rounded compares identity, extended tests void.
    let use_mate = if extended {
        !region_is_void(original, required(region, operation)?)?
    } else {
        region != original.infinite_region()
    };
    if use_mate {
        faceuse = required(eu::faceuse_mate(original, faceuse)?, operation)?;
    }
    if inset {
        faceuse = required(eu::faceuse_mate(original, faceuse)?, operation)?;
    }
    Ok(faceuse)
}

/// The identities of the private working copy [`api_shell_brep_full`] makes of its
/// source (`new SmBrep(*pBrep, NULL, &sOriginalToCopyMap)`), which the shell then
/// runs on as its original. They must be disjoint from the source, result and every
/// other identity in the shared stores.
pub struct ShellCopyIdentities<'a> {
    /// Object identities in the working copy's arena (its B-rep is
    /// `EntityRef::Brep(BrepId::from_index(0))`).
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// Identities of the working copy's edge curves (the second argument is the
    /// curve type), as [`OffsetNotifications::original_curve_ref`].
    pub curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
    /// Region source-list handles of the working copy's arena, as the original-arena
    /// half of [`OffsetNotifications::boolean_source_lists`].
    pub boolean_source_lists: &'a dyn Fn(RegionId) -> [ObjectRef; 2],
}

impl core::fmt::Debug for ShellCopyIdentities<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ShellCopyIdentities")
            .finish_non_exhaustive()
    }
}

/// Pure shell API: NER, then validate each selected face before copying.
///
/// Each selection is `Some((origin_brep_id, face_id))`; `None` represents native
/// null. The caller must assign distinct `BrepId`s to distinct live B-reps, and
/// selections must be faces of the identified source (native 1007 otherwise, checked
/// before the copy, including when the distance is too small). Duplicates and order
/// are kept.
/// The source is copied with the real `SmBrep` copy constructor (attributes copied,
/// analytics added, as its defaults) under `copy`'s identities, each selection is
/// mapped to its copy through the original-to-copy map (`SM_ERR` for a missing or
/// non-face mapping), and [`OffsetExecutive::shell_brep`] runs on the copy with the
/// options unchanged. The working copy is dropped unless the shell consumed or
/// returned it; the source is never modified.
///
/// # Errors
///
/// Native 1007 for a null source or an invalid selection; those of the copy and of
/// the shell, unchanged (`SER`).
pub fn api_shell_brep_full(
    kernel: &mut Kernel,
    original: Option<BrepRef<'_, BrepId>>,
    options: ShellOptions,
    faces: &[Option<FaceKey>],
    copy: &ShellCopyIdentities<'_>,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<Brep> {
    let original = original.ok_or_else(|| invalid_input(OP_API_SHELL))?;
    validate_shell_faces(original, faces)?;

    // cpp:2389–2393: SmCopyBrepMap sOriginalToCopyMap; new SmBrep(*pBrep, NULL,
    // &sOriginalToCopyMap) with bCopyAttributes = bAddAnalytics = TRUE.
    let mut original_to_copy = CopyBrepMap::new();
    // (not upstream) the source's own attributes go to the caller's stores, so the
    // copy's Copy notifications carry them to the working copy (bCopyAttributes)
    attributes::seed_attributes(
        original.brep,
        notifications.original_key_of,
        notifications.attributes,
    )?;
    let mut working = {
        let source_key = notifications.original_key_of;
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of: copy.key_of,
        };
        NotifyState::from_kernel_stores(&mut stores).with_kernel_env(kernel, |kernel, env| {
            bp3::copy_brep(
                kernel,
                env,
                original.brep,
                source_key,
                None,
                Some(&mut original_to_copy),
                true,
                true,
            )
        })?
    };

    // (not upstream) the working copy's attributes are those the copy notified into the
    // caller's stores; the shell must not seed them a second time
    working.set_attributes(AttributeStore::default());

    // cpp:2395–2404: map every selection in order.
    let mut working_faces = Vec::with_capacity(faces.len());
    for (_, face) in faces.iter().flatten() {
        match original_to_copy.get_at(Some(EntityRef::Face(*face)))? {
            Some(EntityRef::Face(mapped)) => working_faces.push(mapped),
            _ => return Err(SolidError::sm_err(OP_API_SHELL)),
        }
    }

    // cpp:2406–2420: the working copy is the shell's original. A copy the shell
    // neither consumed nor returned is dropped with the outcome's retained input.
    let source_lists = |result: bool, region: RegionId| {
        if result {
            (notifications.boolean_source_lists)(true, region)
        } else {
            (copy.boolean_source_lists)(region)
        }
    };
    let mut working_notifications = OffsetNotifications {
        records: &mut *notifications.records,
        attributes: &mut *notifications.attributes,
        original_key_of: copy.key_of,
        result_key_of: notifications.result_key_of,
        original_curve_ref: copy.curve_ref,
        result_curve_ref: notifications.result_curve_ref,
        boolean_source_lists: &source_lists,
        result_brep: notifications.result_brep,
        merge_identities: notifications.merge_identities,
    };
    let outcome = OffsetExecutive::shell_brep(
        kernel,
        working,
        options,
        &working_faces,
        &mut working_notifications,
    );
    let mut result = outcome.result?;
    // (not upstream: the port's cached topology, true again for the caller)
    result.refresh_cached_topology();
    Ok(result)
}

/// Full-offset API: guard native null with 1007, then call the static executive.
/// Unlike the shell API this wrapper does not make a topology copy; lazy UV trim
/// geometry updates require mutable access to the caller's original arena.
pub fn api_offset_brep_full(
    kernel: &mut Kernel,
    original: Option<&mut Brep>,
    options: OffsetOptions,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<Brep> {
    let original = original.ok_or_else(|| invalid_input(OP_API_OFFSET))?;
    let mut result = offset_brep(kernel, original, options, notifications)?;
    // (not upstream: the port's cached topology, true again for the caller)
    result.refresh_cached_topology();
    Ok(result)
}

/// (not upstream) [`api_offset_brep_full`] with notification identities of its own:
/// for a caller that keeps no notification stores. The source's attributes
/// ([`Brep::add_attribute`]) reach the result as upstream's notifications carry them
/// (each offset face copies its source face's, `SmOffsetExecutive.cpp:1237`); the
/// source keeps its own, and its [`Brep::key`] is restored.
///
/// # Errors
///
/// Those of [`api_offset_brep_full`].
pub fn offset_brep_simple(
    kernel: &mut Kernel,
    original: &mut Brep,
    options: OffsetOptions,
) -> Result<Brep> {
    let saved_key = original.key();
    original.set_key(attributes::original_key(EntityRef::Brep(BrepId::from_index(0))).key);
    let mut records = attributes::simple_records(original);
    let mut store = AttributeStore::default();
    let mut notifications = OffsetNotifications {
        records: &mut records,
        attributes: &mut store,
        original_key_of: &attributes::original_key,
        result_key_of: &attributes::result_key,
        original_curve_ref: &attributes::original_curve,
        result_curve_ref: &attributes::result_curve,
        boolean_source_lists: &attributes::source_lists,
        result_brep: attributes::result_key(EntityRef::Brep(BrepId::from_index(0))),
        merge_identities: Some(&attributes::SimpleIdentities),
    };
    let result = api_offset_brep_full(kernel, Some(original), options, &mut notifications);
    original.set_key(saved_key);
    result
}

/// (not upstream) [`api_shell_brep_full`] with notification identities of its own,
/// on faces of `original` (`faces` removed: open faces; empty: a closed shell). The
/// source's attributes reach the result as upstream's notifications carry them; the
/// source is not changed.
///
/// # Errors
///
/// Those of [`api_shell_brep_full`].
pub fn shell_brep_simple(
    kernel: &mut Kernel,
    original: &Brep,
    options: ShellOptions,
    faces: &[FaceId],
) -> Result<Brep> {
    let source_id = BrepId::from_index(0);
    let mut source = original.clone();
    source.set_key(attributes::original_key(EntityRef::Brep(source_id)).key);
    let mut records = attributes::simple_records(&source);
    let mut store = AttributeStore::default();
    let mut notifications = OffsetNotifications {
        records: &mut records,
        attributes: &mut store,
        original_key_of: &attributes::original_key,
        result_key_of: &attributes::result_key,
        original_curve_ref: &attributes::original_curve,
        result_curve_ref: &attributes::result_curve,
        boolean_source_lists: &attributes::source_lists,
        result_brep: attributes::result_key(EntityRef::Brep(source_id)),
        merge_identities: Some(&attributes::SimpleIdentities),
    };
    let selections: Vec<Option<FaceKey>> = faces.iter().map(|&f| Some((source_id, f))).collect();
    api_shell_brep_full(
        kernel,
        Some(BrepRef {
            brep: &source,
            handle: source_id,
        }),
        options,
        &selections,
        &ShellCopyIdentities {
            key_of: &attributes::copy_key,
            curve_ref: &attributes::copy_curve,
            boolean_source_lists: &attributes::copy_source_lists,
        },
        &mut notifications,
    )
}

/// `SmOffsetGeometryCreation(distance, approx_tol_3d).FaceuseOffset(faceuse, TRUE)`: the
/// face's surface offset by `distance` along the normal of the source face use (against
/// it for an opposite one), made analytic when possible, its self-intersections skipped.
/// `None` when no offset surface was made.
///
/// # Errors
///
/// Those of `FaceuseOffset` (the face use's face, its surface, the offset surface).
pub fn faceuse_offset(
    kernel: &Kernel,
    original: &Brep,
    faceuse: FaceuseId,
    distance: f64,
    approx_tol_3d: f64,
) -> Result<Option<crate::surface::surface::Surface>> {
    let creator = crate::offset::offset_geometry_creation::OffsetGeometryCreation::new(
        distance,
        approx_tol_3d,
    );
    Ok(super::face::build(kernel, &creator, original, faceuse, true)?.0)
}

fn configured_executive(original: &Brep, options: OffsetOptions) -> OffsetExecutive {
    let mut executive = OffsetExecutive::new(
        OffsetOperation::SolidOffset,
        options.distance.abs(),
        original.tolerance(),
    );
    executive.set_inset(options.distance < 0.0);
    executive.set_extend_convex_edges(options.extend_convex_edges);
    executive.set_merge_results(options.merge_results);
    executive
}

fn configured_shell_executive(
    original: &Brep,
    options: ShellOptions,
    faces: &[FaceId],
) -> OffsetExecutive {
    let mut executive = configured_executive(original, options.offset);
    executive.set_shell_faces(faces);
    executive.set_create_offset_solid(options.create_offset_solid);
    executive
}

fn run_shell_offset(
    kernel: &mut Kernel,
    original: &mut Brep,
    options: ShellOptions,
    faces: &[FaceId],
    notifications: &mut OffsetNotifications<'_>,
) -> Result<Brep> {
    let mut executive = configured_shell_executive(original, options, faces);
    // Unlike offset_brep, shelling must not collapse ordinary inner statuses.
    executive.do_solid_offset(
        kernel,
        Some(original),
        SolidOffsetOptions::default(),
        notifications,
    )
}

fn shell_difference(
    kernel: &mut Kernel,
    original: &mut Option<Brep>,
    shell: Brep,
    distance: f64,
    tolerance: f64,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<Brep> {
    shell_difference_with(
        kernel,
        original,
        shell,
        distance,
        tolerance,
        notifications,
        |adapter, kernels, primary, other, attributes| {
            adapter.manifold_boolean(
                kernels,
                primary,
                other,
                &mut ManifoldState::default(),
                attributes,
                true,
                BooleanOperation::Difference,
                None,
            )
        },
    )
}

// The seam isolates the otherwise latent continuation from earlier unported
// manifold/offset queries. Production always runs the adapter's real Boolean.
fn shell_difference_with(
    kernel: &mut Kernel,
    original: &mut Option<Brep>,
    shell: Brep,
    distance: f64,
    tolerance: f64,
    notifications: &mut OffsetNotifications<'_>,
    boolean: impl FnOnce(
        &mut ManifoldKernelAdapter<'_>,
        &mut BooleanKernels<'_>,
        &mut Option<Brep>,
        &mut Option<Brep>,
        &mut AttributeStore,
    ) -> Result<bool>,
) -> Result<Brep> {
    let inset = distance < 0.0;
    let source_lists = |region| (notifications.boolean_source_lists)(!inset, region);
    let keys = ManifoldNotificationKeys {
        primary: if inset {
            notifications.original_key_of
        } else {
            notifications.result_key_of
        },
        other: if inset {
            notifications.result_key_of
        } else {
            notifications.original_key_of
        },
        primary_curve: &|e, kind| {
            Ok((if inset {
                notifications.original_curve_ref
            } else {
                notifications.result_curve_ref
            })(e, kind))
        },
        other_curve: &|e, kind| {
            Ok((if inset {
                notifications.result_curve_ref
            } else {
                notifications.original_curve_ref
            })(e, kind))
        },
        source_lists: &|region| Ok(source_lists(region)),
        actual_curve_identities: false,
    };
    let mut shell = Some(shell);
    let (primary, other) = if inset {
        (&mut *original, &mut shell)
    } else {
        (&mut shell, &mut *original)
    };
    let result =
        KernelManifoldIntersector::with_notifications(keys, notifications.records, |adapter| {
            adapter.configure_operands(
                kernel,
                required(primary.as_ref(), OP_SHELL)?,
                required(other.as_ref(), OP_SHELL)?,
                tolerance,
                20.0 * crate::math::PI / 180.0,
            )?;
            boolean(
                adapter,
                &mut BooleanKernels {
                    primary: kernel,
                    other: None,
                },
                primary,
                other,
                notifications.attributes,
            )
        });
    match result {
        // Difference assigns rpResult to primary, not a clone. A still-present
        // original on error remains in its caller slot; other may already have
        // been consumed before a late cache/inverse-transform error (p4).
        Ok(true) => required(primary.take(), OP_SHELL),
        Ok(false) => Err(SolidError::invalid_input(
            OP_SHELL,
            "Difference did not assign its result",
        )),
        Err(error) => Err(error),
    }
}

fn collapse_offset_status<T>(result: Result<T>) -> Result<T> {
    result.map_err(|error| {
        if error.is_unported() {
            error
        } else {
            SolidError::sm_err(OP_OFFSET)
        }
    })
}

/// The source fork's sheet-thicken wall merge (`ShellBrep`'s sheet branch after the
/// stitch, source issue #978; opt-in [`ShellOptions::merge_sheet_side_walls`]):
/// the walls are the faces other than the sheet's own (`sheet_faces`, the sheet's copies)
/// along an edge of the sheet; every topological edge between two walls goes (the others,
/// and every topological vertex, are kept), each wall it bounds first going onto its plane
/// ([`put_planar_face_on_its_plane`]). Editing is enabled meanwhile.
///
/// With [`crate::kernel::UpstreamFixes::thicken_planar_side_walls`] (WR-564, #1599; the
/// fork's source fork#62) every wall goes onto its plane first, in the shell's
/// face order, and its old surface's seams are deleted; the walls are then merged only
/// with `merge` (the #978 opt-in).
///
/// # Errors
///
/// Those of the plane replacement and of `RemoveTopologicalEdgesAndVertices`.
fn merge_sheet_side_walls(
    kernel: &mut Kernel,
    shell: &mut Brep,
    sheet_faces: &[FaceId],
    merge: bool,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<()> {
    // the walls: the faces along the sheet's boundary, other than the sheet's own
    let mut walls: BTreeSet<FaceId> = BTreeSet::new();
    for e in bp1::edges(shell, None)? {
        let edge_faces = crate::topology::edge::faces(shell, e, None)?;
        if !edge_faces.iter().any(|f| sheet_faces.contains(f)) {
            continue;
        }
        for f in edge_faces {
            if !sheet_faces.contains(&f) {
                walls.insert(f);
            }
        }
    }

    // keep every topological edge but those between two walls, and every topological vertex
    let topo_edges = bp1::topological_edges(kernel, shell)?;
    let keep_vertices = bp1::topological_vertices(kernel, shell)?;
    let mut keep_edges = Vec::new();
    let mut merging: Vec<FaceId> = Vec::new();
    for e in topo_edges {
        let edge_faces = crate::topology::edge::faces(shell, e, None)?;
        let between_walls = edge_faces.len() == 2 && edge_faces.iter().all(|f| walls.contains(f));
        if !between_walls {
            keep_edges.push(e);
            continue;
        }
        for f in edge_faces {
            if !merging.contains(&f) {
                merging.push(f);
            }
        }
    }
    let every_wall = kernel.fixes.thicken_planar_side_walls;
    if merging.is_empty() && !every_wall {
        return Ok(());
    }

    // SmTemporaryChangeValue<SmBoolean> sEditing(pBrepShell->m_bEditingEnabled, TRUE)
    let editing = shell.editing_enabled();
    shell.set_editing_enabled(true);
    let result = (|| {
        if every_wall {
            // every wall onto its plane, in GetFaces order (#1599)
            for f in bp1::faces(shell, None)? {
                if walls.contains(&f) {
                    put_planar_face_on_its_plane(kernel, shell, f, true, notifications)?;
                }
            }
        } else {
            for &f in &merging {
                put_planar_face_on_its_plane(kernel, shell, f, false, notifications)?;
            }
        }
        if !merge || merging.is_empty() {
            return Ok(());
        }
        crate::topology::brep::p5::remove_topological_edges_and_vertices(
            kernel,
            shell,
            Some(keep_edges.as_slice()),
            Some(keep_vertices.as_slice()),
        )
    })();
    shell.set_editing_enabled(editing);
    result
}

/// The fork's `sm_PutPlanarFaceOnItsPlane` (#978): a face whose surface is planar
/// (`IsPlanar(SM_EFF_ZERO)`) but not a plane goes onto a plane through it, normal the
/// way the surface's is at the face's UV mid point, x axis along the coordinate axis
/// least along the normal, over the face's tight box corners in the plane's parameters
/// with as much again round them; `ReplaceSurface(.., FALSE, NULL, TRUE)` remakes its
/// UV trim curves. A face on a plane, or on a surface that is not planar, is left.
///
/// With `delete_seams` (WR-564, #1599; the fork's source fork#62) the face's
/// edges that are seams of its old surface (`SmEdge::IsSeam`, found before the
/// replacement) are deleted after it: a surface closed round on itself (the wall
/// between a circle and its offset about one axis, a flat ring) bounds the face along
/// its seam too, which on the plane is a slit through the ring; `DeleteEdge` splits the
/// loop into the ring's outer and inner circles.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a face without surface; those of the planarity test, the
/// normal, the unitizations, the box, `MakeNurb` and `ReplaceSurface`.
fn put_planar_face_on_its_plane(
    kernel: &mut Kernel,
    b: &mut Brep,
    f: FaceId,
    delete_seams: bool,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<()> {
    use crate::nurbs::knots::Side;
    use crate::surface::plane::p1::Plane;
    use crate::surface::surface::Surface;

    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP_SHELL))?;
    if surface.is_kind_of(Plane::TYPE)? {
        return Ok(());
    }
    let planar = surface.is_planar(kernel, crate::math::EFF_ZERO)?;
    if !planar.is_planar {
        return Ok(());
    }
    let origin = planar.point;
    let mut normal = planar.normal;

    // the normal the face's surface has
    let mid = b.face(f)?.uv_domain.mid();
    let surface_normal = surface.evaluate_normal(kernel, mid, Side::Right, Side::Right)?;
    if surface_normal.dot(normal) < 0.0 {
        normal = -normal;
    }
    let normal = normal.unitize()?;

    // axes in the plane: x along the coordinate axis least along the normal
    let mut along = Vec3::new(1.0, 0.0, 0.0);
    if normal.y.abs() < normal.x.abs() && normal.y.abs() <= normal.z.abs() {
        along = Vec3::new(0.0, 1.0, 0.0);
    } else if normal.z.abs() < normal.x.abs() {
        along = Vec3::new(0.0, 0.0, 1.0);
    }
    let x_axis = (along - normal * along.dot(normal)).unitize()?;
    // cross product: XAxis x YAxis = Normal
    let y_axis = normal.cross(x_axis).unitize()?;

    // a domain holding the face: its box's corners in the plane's parameters, and as
    // much again round them
    let face_box = crate::query::face::p1::calculate_bounding_box(kernel, &*b, f, true)?;
    let (mut u_min, mut u_max) = (crate::math::BIG_DOUBLE, -crate::math::BIG_DOUBLE);
    let (mut v_min, mut v_max) = (crate::math::BIG_DOUBLE, -crate::math::BIG_DOUBLE);
    for corner in face_box.corners() {
        let to = corner - origin;
        u_min = u_min.min(to.dot(x_axis));
        u_max = u_max.max(to.dot(x_axis));
        v_min = v_min.min(to.dot(y_axis));
        v_max = v_max.max(to.dot(y_axis));
    }
    let margin = (u_max - u_min).max(v_max - v_min);
    let domain = Extent2d::from_points(
        crate::math::Point2::new(u_min - margin, v_min - margin),
        crate::math::Point2::new(u_max + margin, v_max + margin),
    );

    // the old surface's seams (#1599; only with the opt-in)
    let mut seams: Vec<EdgeId> = Vec::new();
    if delete_seams {
        for e in crate::topology::face::p4::edges(&*b, f, None, false, None)? {
            if crate::topology::edge::is_seam_of_surface(kernel, b, e, f)? {
                seams.push(e);
            }
        }
    }

    let mut plane = Plane::new(
        origin,
        x_axis,
        y_axis,
        crate::math::Vec2::new(1.0, 1.0),
        &domain,
    );
    plane.make_nurb()?;
    let face_ref = (notifications.result_key_of)(EntityRef::Face(f));
    let mut stores = NotificationStores {
        records: &mut *notifications.records,
        attributes: &mut *notifications.attributes,
        key_of: notifications.result_key_of,
    };
    NotifyState::from_kernel_stores(&mut stores).with_kernel_env(kernel, |kernel, env| {
        crate::topology::brep::p2::replace_surface(
            kernel,
            b,
            env,
            f,
            Surface::from(plane),
            face_ref,
            crate::topology::brep::p2::OldSurface::Delete,
            true,
        )?;
        for &e in &seams {
            crate::topology::brep::p4::delete_edge(
                kernel,
                b,
                &mut crate::topology::brep::p4::DeleteEnv::from_notify(env),
                e,
                None,
            )?;
        }
        Ok(())
    })
}

fn check_shell_radius(radius: f64, tolerance: f64) -> Result<()> {
    if radius < 2.0 * tolerance {
        Err(invalid_input(OP_SHELL))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtendedDomainChoice {
    Original,
    Extension,
}

fn choose_extended_domain(
    trim: &Extent2d,
    original: &Extent2d,
    extension: &Extent2d,
    tolerance: f64,
    restore_original: impl FnOnce() -> Result<()>,
) -> Result<ExtendedDomainChoice> {
    if trim.is_contained_by(original, tolerance) {
        restore_original()?;
        Ok(ExtendedDomainChoice::Original)
    } else if trim.is_contained_by(extension, tolerance) {
        Ok(ExtendedDomainChoice::Extension)
    } else {
        restore_original()?;
        Err(SolidError::sm_err(OP_EXTENDED))
    }
}

fn with_doing_boolean<T>(
    kernel: &mut Kernel,
    run: impl FnOnce(&mut Kernel) -> Result<T>,
) -> Result<T> {
    let previous = kernel.context.doing_boolean();
    kernel.context.set_doing_boolean(true);
    let result = run(kernel);
    kernel.context.set_doing_boolean(previous);
    result
}

fn validate_shell_faces(original: BrepRef<'_, BrepId>, faces: &[Option<FaceKey>]) -> Result<()> {
    for selection in faces {
        let Some((owner, face)) = selection else {
            return Err(invalid_input(OP_API_SHELL));
        };
        // Comparing FaceId alone would accept foreign face 0 as source face 0.
        if *owner != original.handle || original.brep.faces().get(*face).is_none() {
            return Err(invalid_input(OP_API_SHELL));
        }
    }
    Ok(())
}

fn select_radial(
    original: &Brep,
    radial: EdgeuseId,
    mapped: &[EdgeuseId],
    marked: &BTreeSet<EdgeuseId>,
) -> Result<Option<EdgeuseId>> {
    // cpp:1459 / 1532 evaluates GetMate before checking radial membership.
    let Some(mate) = eu::mate(original, radial)? else {
        return Ok(Some(radial));
    };
    if mapped.contains(&mate) && !mapped.contains(&radial) {
        Ok((!marked.contains(&mate)).then_some(mate))
    } else {
        Ok(Some(radial))
    }
}

fn region_is_void(brep: &Brep, region: RegionId) -> Result<bool> {
    Ok(brep.region(region)?.is_void() != 0)
}

fn required<T>(value: Option<T>, operation: &'static str) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(operation))
}

fn invalid_input(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: 1007,
        entity: None,
    }
}

/// `sm_OrderEdgeusesByBrep`: `edgeuses` ordered by `brep` (its faces, each face's
/// two face uses, their edge uses), not by identity: the B-rep order is walked from
/// its end and each listed edge use is placed at its first sighting. Edge uses not
/// found in `brep` go last, in their input order.
fn order_edgeuses_by_brep(brep: &Brep, edgeuses: &[EdgeuseId]) -> Result<Vec<EdgeuseId>> {
    let mut unplaced: BTreeSet<EdgeuseId> = edgeuses.iter().copied().collect();

    let mut brep_order = Vec::new();
    for f in bp1::faces(brep, None)? {
        let (faceuse1, faceuse2) = face::faceuses(brep, f)?;
        for faceuse in [faceuse1, faceuse2].into_iter().flatten() {
            brep_order.extend(eu::faceuse_edgeuses(brep, faceuse, false, None)?);
        }
    }

    let mut ordered = Vec::new();
    for &edgeuse in brep_order.iter().rev() {
        if unplaced.remove(&edgeuse) {
            ordered.push(edgeuse);
        }
    }
    for &edgeuse in edgeuses {
        if unplaced.contains(&edgeuse) {
            ordered.push(edgeuse);
        }
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{EntityRef, SM_ERR, SM_ERR_NULL_POINTER};
    use crate::geom::context::{MarkLock, MarkType};
    use crate::geom::extent2d::Extent2d;
    use crate::math::{Vec3, PI};
    use crate::surface::plane::Plane;
    use crate::surface::surface::Surface;
    use crate::topology::{Faceuse, FaceuseId, Region, Shell};

    /// `faceuse_offset` offsets a box face's plane: a plane parallel to it, `distance` away.
    #[test]
    fn faceuse_offset_of_a_box_face_is_a_plane_at_the_distance() {
        use crate::math::Point2;
        use crate::surface::surface::p2::SurfaceKind;
        let mut k = Kernel::default();
        let brep = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::new(0.0, 0.0, 0.0),
            1.0,
            2.0,
            3.0,
        )
        .unwrap();
        let f = bp1::faces(&brep, None).unwrap()[0];
        let faceuse = face::faceuses(&brep, f).unwrap().0.unwrap();
        let SurfaceKind::Plane(source) = face::surface(&brep, f).unwrap().unwrap().kind() else {
            panic!("a box face is a plane");
        };
        let on_source = source.evaluate_point(Point2::new(0.0, 0.0));
        let made = faceuse_offset(&k, &brep, faceuse, 0.25, 1e-6)
            .unwrap()
            .unwrap();
        let SurfaceKind::Plane(made) = made.kind() else {
            panic!("an offset plane stays a plane");
        };
        let [a, b, c] = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]
            .map(|(u, v)| made.evaluate_point(Point2::new(u, v)));
        let normal = (b - a).cross(c - a);
        let normal = normal * (1.0 / normal.length());
        for q in [a, b, c] {
            assert!(((q - on_source).dot(normal).abs() - 0.25).abs() < 1e-12);
        }
    }

    /// `sm_OrderEdgeusesByBrep`: the B-rep's edge uses come from the end of its
    /// face/face-use/edge-use order, each once; an edge use not in the B-rep goes last.
    #[test]
    fn edgeuses_are_ordered_by_their_brep_then_input_order() {
        let mut k = Kernel::default();
        let brep = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::new(0.0, 0.0, 0.0),
            1.0,
            2.0,
            3.0,
        )
        .unwrap();
        let mut brep_order = Vec::new();
        for f in bp1::faces(&brep, None).unwrap() {
            let (a, b) = face::faceuses(&brep, f).unwrap();
            for fu in [a, b].into_iter().flatten() {
                brep_order.extend(eu::faceuse_edgeuses(&brep, fu, false, None).unwrap());
            }
        }
        let last = *brep_order.last().unwrap();
        let first = brep_order[0];
        let foreign = [
            EdgeuseId::from_index(900_001),
            EdgeuseId::from_index(900_000),
        ];
        let input = vec![foreign[0], first, foreign[1], last];
        let ordered = order_edgeuses_by_brep(&brep, &input).unwrap();
        assert_eq!(ordered, vec![last, first, foreign[0], foreign[1]]);
    }

    fn fixture_key(entity: EntityRef, namespace: u64) -> ObjectRef {
        let (index, type_code) = match entity {
            EntityRef::Brep(id) => (id.index(), crate::geom::a_object::BREP_TYPE),
            EntityRef::Shell(id) => (id.index(), Shell::type_code()),
            EntityRef::Face(id) => (id.index(), face::TYPE),
            EntityRef::Faceuse(id) => (id.index(), 16022),
            EntityRef::Edge(id) => (id.index(), crate::topology::edge::TYPE),
            EntityRef::Edgeuse(id) => (id.index(), eu::TYPE),
            EntityRef::Vertex(id) => (id.index(), crate::topology::vertex::TYPE),
            EntityRef::Vertexuse(id) => (id.index(), crate::topology::vertexuse::TYPE),
            EntityRef::Loopuse(id) => (id.index(), crate::topology::loopuse::TYPE),
            EntityRef::Region(id) => (id.index(), Region::type_code()),
            EntityRef::Loop(id) => (id.index(), crate::topology::topology::TYPE),
        };
        ObjectRef {
            key: ObjectKey(
                namespace + u64::try_from(type_code).unwrap() * 1_000_000 + u64::from(index),
            ),
            type_code,
        }
    }

    const COPY_IDS: ShellCopyIdentities<'static> = ShellCopyIdentities {
        key_of: &copy_key,
        curve_ref: &|edge, type_code| ObjectRef {
            key: ObjectKey(
                3_000_000_000_000
                    + u64::try_from(type_code).unwrap() * 1_000_000
                    + u64::from(edge.index()),
            ),
            type_code,
        },
        boolean_source_lists: &|region| {
            [0_u64, 1].map(|list| ObjectRef {
                key: ObjectKey(4_000_000_000_000 + list * 1_000_000 + u64::from(region.index())),
                type_code: 0,
            })
        },
    };

    /// The API copied its source and ran the shell on the copy (the copy no longer
    /// defers; the shell's own status is whatever the fixture reaches).
    fn assert_copied(result: &Result<Brep>) {
        assert!(
            !matches!(result, Err(SolidError::Unported { .. })),
            "{result:?}"
        );
    }

    fn copy_key(entity: EntityRef) -> ObjectRef {
        fixture_key(entity, 2_000_000_000_000)
    }

    fn original_key(entity: EntityRef) -> ObjectRef {
        fixture_key(entity, 100_000_000_000)
    }

    fn result_key(entity: EntityRef) -> ObjectRef {
        fixture_key(entity, 200_000_000_000)
    }

    fn result_curve(edge: EdgeId, type_code: i64) -> ObjectRef {
        ObjectRef {
            key: ObjectKey(300_000_000_000 + u64::from(edge.index())),
            type_code,
        }
    }

    fn original_curve(edge: EdgeId, type_code: i64) -> ObjectRef {
        ObjectRef {
            key: ObjectKey(400_000_000_000 + u64::from(edge.index())),
            type_code,
        }
    }

    fn boolean_source_lists(result: bool, region: RegionId) -> [ObjectRef; 2] {
        [0, 1].map(|list| ObjectRef {
            key: ObjectKey(
                500_000_000_000
                    + u64::from(result) * 10_000_000_000
                    + u64::from(region.index()) * 2
                    + list,
            ),
            type_code: 0,
        })
    }

    /// Identities of the merge's new B-reps, disjoint from the fixture keys above
    /// (namespaces from 1000, curves and source lists from 700e9 / 800e9).
    struct MergeBodies;
    impl SweepIdentities for MergeBodies {
        fn topology(&self, brep: BrepId, entity: EntityRef) -> ObjectRef {
            crate::boolean::merge::p4::notification_key(1000 + brep.index(), false, entity)
        }
        fn owned_auxiliary_objects(&self, brep: BrepId, model: &Brep) -> Vec<ObjectKey> {
            model
                .edges()
                .iter()
                .filter(|(_, e)| e.curve.is_some())
                .map(|(edge, _)| merge_curve(brep, edge, 0).key)
                .collect()
        }
        fn curve(
            &self,
            brep: BrepId,
            edge: EdgeId,
            curve: &crate::curve::curve::Curve,
        ) -> ObjectRef {
            merge_curve(brep, edge, curve.type_code())
        }
        fn new_curve(&self, brep: BrepId, edge: EdgeId, type_code: i64) -> Result<ObjectRef> {
            Ok(merge_curve(brep, edge, type_code))
        }
        fn source_lists(&self, brep: BrepId, region: RegionId) -> Result<[ObjectRef; 2]> {
            Ok([0, 1].map(|list| ObjectRef {
                key: ObjectKey(
                    800_000_000_000
                        + u64::from(brep.index()) * 1_000_000_000
                        + u64::from(region.index()) * 2
                        + list,
                ),
                type_code: 0,
            }))
        }
    }

    fn merge_curve(brep: BrepId, edge: EdgeId, type_code: i64) -> ObjectRef {
        ObjectRef {
            key: ObjectKey(
                700_000_000_000 + u64::from(brep.index()) * 1_000_000_000 + u64::from(edge.index()),
            ),
            type_code,
        }
    }

    fn source_brep() -> Brep {
        let mut brep = Brep::new();
        brep.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
        brep
    }

    struct NotificationFixture {
        records: crate::topology::topology::NotifyRecords,
        attributes: AttributeStore,
    }

    impl NotificationFixture {
        fn new(original: &Brep) -> Self {
            let mut records = crate::topology::topology::NotifyRecords::new();
            records.insert(original.key(), Topology::new());
            records.insert(
                result_key(EntityRef::Brep(BrepId::from_index(0))).key,
                Topology::new(),
            );
            let entities = original
                .shells()
                .iter()
                .map(|(id, _)| EntityRef::Shell(id))
                .chain(original.faces().iter().map(|(id, _)| EntityRef::Face(id)))
                .chain(
                    original
                        .faceuses()
                        .iter()
                        .map(|(id, _)| EntityRef::Faceuse(id)),
                )
                .chain(original.edges().iter().map(|(id, _)| EntityRef::Edge(id)))
                .chain(
                    original
                        .edgeuses()
                        .iter()
                        .map(|(id, _)| EntityRef::Edgeuse(id)),
                )
                .chain(
                    original
                        .vertices()
                        .iter()
                        .map(|(id, _)| EntityRef::Vertex(id)),
                )
                .chain(
                    original
                        .vertexuses()
                        .iter()
                        .map(|(id, _)| EntityRef::Vertexuse(id)),
                )
                .chain(
                    original
                        .loopuses()
                        .iter()
                        .map(|(id, _)| EntityRef::Loopuse(id)),
                )
                .chain(
                    original
                        .regions()
                        .iter()
                        .map(|(id, _)| EntityRef::Region(id)),
                );
            for entity in entities {
                records.insert(original_key(entity).key, Topology::new());
            }
            Self {
                records,
                attributes: AttributeStore::default(),
            }
        }

        fn context(&mut self) -> OffsetNotifications<'_> {
            OffsetNotifications {
                records: &mut self.records,
                attributes: &mut self.attributes,
                original_key_of: &original_key,
                result_key_of: &result_key,
                original_curve_ref: &original_curve,
                result_curve_ref: &result_curve,
                boolean_source_lists: &boolean_source_lists,
                result_brep: result_key(EntityRef::Brep(BrepId::from_index(0))),
                merge_identities: Some(&MergeBodies),
            }
        }
    }

    fn executive() -> OffsetExecutive {
        OffsetExecutive::new(OffsetOperation::SolidOffset, 0.125, 1.0e-6)
    }

    fn run_face_prefix(
        exec: &mut OffsetExecutive,
        brep: &mut Brep,
        face: FaceId,
        extended: bool,
    ) -> Result<()> {
        let mut kernel = Kernel::default();
        let mut notifications = NotificationFixture::new(brep);
        exec.allocate_offset_brep(&mut notifications.context())?;
        exec.offset_face_prefix(
            &mut kernel,
            brep,
            face,
            extended,
            &mut notifications.context(),
        )
    }

    fn assert_status<T: core::fmt::Debug>(result: Result<T>, code: i32) {
        assert!(
            matches!(result.unwrap_err(), SolidError::Upstream { code: actual, .. } if actual == code)
        );
    }

    /// The B-rep box (`query.brep`) these paths used to stop at is ported: on the
    /// synthetic operands they now run the merge engine, whose outcome here is not
    /// compared with upstream (success, or an ordinary failure of the engine).
    fn assert_reaches_merge<T: core::fmt::Debug>(result: Result<T>) {
        if let Err(error) = result {
            assert!(
                !error.is_unported() && error.operation().starts_with("boolean::merge"),
                "{error:?}"
            );
        }
    }

    fn assert_dependency<T: core::fmt::Debug>(result: Result<T>, symbol: &str, owner: &str) {
        let error = result.unwrap_err();
        assert!(
            matches!(&error, SolidError::Unported { upstream, unit } if *upstream == symbol && *unit == owner),
            "{error:?}"
        );
    }

    fn one_face(surface: bool) -> (Brep, FaceId) {
        let mut brep = source_brep();
        let region = brep
            .add_region_record(Region {
                in_brep: true,
                is_void: 1,
                ..Region::default()
            })
            .unwrap();
        brep.set_infinite_region(Some(region));
        let shell = brep
            .add_shell_record(Shell {
                region: Some(region),
                ..Shell::default()
            })
            .unwrap();
        brep.region_mut(region).unwrap().shells.push(shell);
        let face = brep.add_face(shell).unwrap();
        let first = brep
            .add_faceuse_record(Faceuse {
                shell: Some(shell),
                face: Some(face),
                orientation: OrientType::Same,
                ..Faceuse::default()
            })
            .unwrap();
        let second = brep
            .add_faceuse_record(Faceuse {
                shell: Some(shell),
                face: Some(face),
                mate: Some(first),
                orientation: OrientType::Opposite,
                ..Faceuse::default()
            })
            .unwrap();
        brep.faceuse_mut(first).unwrap().mate = Some(second);
        let record = brep.face_mut(face).unwrap();
        record.faceuse = Some(first);
        record.uv_domain = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        if surface {
            record.surface = Some(Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)));
        }
        (brep, face)
    }

    fn source(brep: &Brep, id: u32) -> BrepRef<'_, BrepId> {
        BrepRef {
            brep,
            handle: BrepId::from_index(id),
        }
    }

    #[test]
    fn one_face_explicitly_sets_infinite_region_and_void_flag() {
        let (brep, face) = one_face(false);
        let shell = brep.face(face).unwrap().shell;
        let region = brep.shell(shell).unwrap().region.unwrap();
        assert_eq!(brep.infinite_region(), Some(region));
        let region = brep.region(region).unwrap();
        assert!(region.in_brep);
        assert_eq!(region.shells.as_slice(), &[shell]);
        assert_eq!(region.is_void(), 1);
    }

    #[test]
    fn extended_domain_selection_preserves_native_branch_order_and_restore_status() {
        let original = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let extension = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        let tolerance = crate::math::EFF_ZERO;
        let at_tolerance = Extent2d::from_bounds(-tolerance, 0.0, 1.0, 1.0);
        assert!(at_tolerance.is_contained_by(&original, tolerance));
        let beyond_tolerance = Extent2d::from_bounds(-tolerance * 1.01, 0.0, 1.0, 1.0);
        assert!(!beyond_tolerance.is_contained_by(&original, tolerance));
        let mut restored = false;
        assert_eq!(
            choose_extended_domain(&original, &original, &extension, tolerance, || {
                restored = true;
                Ok(())
            })
            .unwrap(),
            ExtendedDomainChoice::Original
        );
        assert!(restored);

        restored = false;
        let inside = Extent2d::from_bounds(-0.5, -0.5, 1.5, 1.5);
        assert_eq!(
            choose_extended_domain(&inside, &original, &extension, tolerance, || {
                restored = true;
                Ok(())
            })
            .unwrap(),
            ExtendedDomainChoice::Extension
        );
        assert!(!restored);

        let extension_boundary = Extent2d::from_bounds(-1.0, -0.5, 1.5, 1.5);
        assert!(extension_boundary.is_contained_by(&extension, tolerance));
        let outside_extension = Extent2d::from_bounds(-1.0 - tolerance * 1.01, -0.5, 1.5, 1.5);
        assert!(!outside_extension.is_contained_by(&extension, tolerance));

        restored = false;
        let outside = Extent2d::from_bounds(-2.0, -2.0, 3.0, 3.0);
        assert_status(
            choose_extended_domain(&outside, &original, &extension, tolerance, || {
                restored = true;
                Ok(())
            }),
            SM_ERR,
        );
        assert!(restored);

        let restore_error = SolidError::invalid_input(OP_EXTENDED, "restore failed");
        assert!(matches!(
            choose_extended_domain(&outside, &original, &extension, tolerance, || {
                Err(restore_error.clone())
            }),
            Err(SolidError::InvalidInput { .. })
        ));
        let dependency = SolidError::unported("restore", "unit");
        assert!(matches!(
            choose_extended_domain(&outside, &original, &extension, tolerance, || {
                Err(dependency.clone())
            }),
            Err(SolidError::Unported { .. })
        ));
    }

    #[test]
    fn both_offset_paths_stop_at_real_missing_surface_and_restore_boolean_context() {
        for extended in [false, true] {
            let (mut original, _) = one_face(false);
            let mut kernel = Kernel::default();
            let mut exec = executive();
            exec.set_extend_convex_edges(extended);
            assert!(matches!(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    exec.do_solid_offset(
                        &mut kernel,
                        Some(&mut original),
                        SolidOffsetOptions::default(),
                        &mut notifications.context(),
                    )
                },
                Err(SolidError::InvalidInput { .. })
            ));
            assert!(!kernel.context.doing_boolean());
            let temporary = exec.offset_brep().unwrap();
            assert_eq!(
                temporary.key(),
                result_key(EntityRef::Brep(BrepId::from_index(0))).key
            );
            assert_ne!(temporary.key(), original.key());
            assert_eq!(temporary.regions().len(), 1);
            let (id, region) = temporary.regions().iter().next().unwrap();
            assert_eq!(temporary.infinite_region(), Some(id));
            assert!(region.in_brep);
            assert_eq!(region.is_void(), 1);
            assert!(region.shells.is_empty());
            assert!(temporary.editing_enabled());
            assert_eq!(temporary.tolerance(), exec.approximation_tol_3d());
            assert!(temporary.shells().is_empty());
            assert!(temporary.faces().is_empty());
            assert!(temporary.edges().is_empty());
            assert!(temporary.vertices().is_empty());
        }
    }

    #[test]
    fn allocated_infinite_region_bypasses_p7_none_region_fallback() {
        let mut kernel = Kernel::default();
        let mut exec = executive();
        let mut notifications = NotificationFixture::new(&source_brep());
        exec.allocate_offset_brep(&mut notifications.context())
            .unwrap();
        let mut temporary = exec.take_offset_brep().unwrap();
        let region = temporary.infinite_region();
        assert!(region.is_some());
        assert_eq!(
            temporary.key(),
            result_key(EntityRef::Brep(BrepId::from_index(0))).key
        );
        assert!(notifications
            .records
            .contains_key(&result_key(EntityRef::Region(region.unwrap())).key));
        // Nonempty loops pass p7's initial guards. Its curve-count check is after
        // region lookup, so reaching it proves the stale None fallback was skipped.
        let input = FaceCurvesInput {
            loops: vec![3],
            curves_3d: Some(vec![None]),
            orientations: vec![OrientType::Same],
            ..FaceCurvesInput::default()
        };
        let result = make_face_with_curves_with_kernel_context(
            &mut kernel,
            &mut temporary,
            region,
            input,
            Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)),
            &Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0),
            OrientType::Same,
            None,
            &|face| result_key(EntityRef::Face(face)),
            &mut NotificationStores {
                records: &mut notifications.records,
                attributes: &mut notifications.attributes,
                key_of: &result_key,
            },
        );
        assert!(matches!(
            result,
            Err(SolidError::InvalidInput {
                operation: "topology::brep::p7",
                ..
            })
        ));
        assert_eq!(temporary.infinite_region(), region);
        assert_eq!(temporary.regions().len(), 1);
        assert!(temporary.faces().is_empty());
    }

    #[test]
    fn region_void_uses_native_truthiness_and_validates_the_region_id() {
        let (mut brep, _) = one_face(false);
        let region = brep.infinite_region().unwrap();
        for (flag, expected) in [(0, false), (1, true), (2, true), (-1, true)] {
            brep.region_mut(region).unwrap().is_void = flag;
            assert_eq!(region_is_void(&brep, region), Ok(expected));
        }
        let missing = RegionId::from_index(99);
        assert!(matches!(
            region_is_void(&brep, missing),
            Err(SolidError::NotFound { entity: EntityRef::Region(id), .. }) if id == missing
        ));
    }

    #[test]
    fn faceuse_selection_returns_exact_extended_orientation_and_inset_use() {
        for flag in [0, 1, 2, -1] {
            for inset in [false, true] {
                // Break each direction separately: an inset after selecting the
                // first mate must dereference that mate, not the upward use again.
                for missing_first in [false, true] {
                    let (mut brep, face) = one_face(false);
                    let region = brep.infinite_region().unwrap();
                    brep.region_mut(region).unwrap().is_void = flag;
                    let first = brep.face(face).unwrap().faceuse.unwrap();
                    let second = brep.faceuse(first).unwrap().mate.unwrap();
                    let broken = if missing_first { first } else { second };
                    brep.faceuse_mut(broken).unwrap().mate = None;
                    let selected = select_offset_faceuse(&brep, face, true, inset, OP_EXTENDED);
                    let should_flip = flag == 0;
                    let needs_broken_mate = if missing_first {
                        should_flip || inset
                    } else {
                        should_flip && inset
                    };
                    if needs_broken_mate {
                        assert_status(selected, SM_ERR_NULL_POINTER);
                    } else {
                        assert_eq!(
                            selected,
                            Ok(if should_flip ^ inset { second } else { first })
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn faceuse_selection_returns_exact_rounded_identity_and_inset_use() {
        for flag in [0, 1, 2, -1] {
            for infinite_matches in [false, true] {
                for inset in [false, true] {
                    for missing_first in [false, true] {
                        let (mut brep, face) = one_face(false);
                        let region = brep.infinite_region().unwrap();
                        brep.region_mut(region).unwrap().is_void = flag;
                        if !infinite_matches {
                            let other = brep.add_region_record(Region::default()).unwrap();
                            brep.set_infinite_region(Some(other));
                        }
                        let first = brep.face(face).unwrap().faceuse.unwrap();
                        let second = brep.faceuse(first).unwrap().mate.unwrap();
                        let broken = if missing_first { first } else { second };
                        brep.faceuse_mut(broken).unwrap().mate = None;
                        let selected = select_offset_faceuse(&brep, face, false, inset, OP_SOLID);
                        let should_flip = !infinite_matches;
                        let needs_broken_mate = if missing_first {
                            should_flip || inset
                        } else {
                            should_flip && inset
                        };
                        if needs_broken_mate {
                            assert_status(selected, SM_ERR_NULL_POINTER);
                        } else {
                            assert_eq!(
                                selected,
                                Ok(if should_flip ^ inset { second } else { first })
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rounded_null_region_identity_is_not_an_extended_void_query() {
        let (mut brep, face) = one_face(false);
        let shell = brep.face(face).unwrap().shell;
        let first = brep.face(face).unwrap().faceuse.unwrap();
        brep.faceuse_mut(first).unwrap().mate = None;
        brep.set_infinite_region(None);
        // A non-null shell region differs from a null infinite region.
        assert_status(
            run_face_prefix(&mut executive(), &mut brep, face, false),
            SM_ERR_NULL_POINTER,
        );
        brep.shell_mut(shell).unwrap().region = None;
        assert!(matches!(
            run_face_prefix(&mut executive(), &mut brep, face, false),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_status(
            run_face_prefix(&mut executive(), &mut brep, face, true),
            SM_ERR_NULL_POINTER,
        );
    }

    #[test]
    fn offset_face_post_geometry_policy_matches_null_and_ssi_boundaries() {
        // Synthetic policy evidence; normal executive calls skip SSI generation.
        for merge in [false, true] {
            for extend in [false, true] {
                let mut executive = executive();
                executive.set_merge_results(merge);
                executive.set_extend_convex_edges(extend);
                for count in [0, 1, 5, 6] {
                    let rejected =
                        matches!((merge, extend, count), (true, _, 1 | 5 | 6) | (_, true, 6));
                    let result = executive.skip_offset_face_geometry(true, true, count);
                    if rejected {
                        assert_status(result, SM_ERR);
                    } else {
                        assert_eq!(result, Ok(false));
                    }
                    assert_eq!(
                        executive.skip_offset_face_geometry(false, true, count),
                        Ok(false)
                    );
                    assert_eq!(
                        executive.skip_offset_face_geometry(false, false, count),
                        Ok(true)
                    );
                    assert_status(
                        executive.skip_offset_face_geometry(true, false, count),
                        SM_ERR,
                    );
                }
            }
        }
    }

    #[test]
    fn empty_edge_synchronization_is_a_no_op_not_a_geometry_dependency() {
        let mut original = source_brep();
        let start = original.add_vertex(Vec3::ZERO).unwrap();
        let end = original.add_vertex(Vec3::X).unwrap();
        let edge = original.add_edge(start, end).unwrap();
        let first = original
            .add_edgeuse_record(crate::topology::Edgeuse {
                edge: Some(edge),
                ..crate::topology::Edgeuse::default()
            })
            .unwrap();
        let second = original
            .add_edgeuse_record(crate::topology::Edgeuse {
                edge: Some(edge),
                ..crate::topology::Edgeuse::default()
            })
            .unwrap();
        let mut kernel = Kernel::default();
        for extended in [false, true] {
            let mut exec = executive();
            exec.set_extend_convex_edges(extended);
            assert_eq!(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    exec.mapped_edge_prefix(
                        &mut kernel,
                        &mut original,
                        first,
                        second,
                        MarkType::Mark,
                        &BTreeSet::new(),
                        &mut notifications.context(),
                    )
                },
                Ok(())
            );
            assert!(exec.offset_brep().is_none());
            assert_eq!(kernel.context.current_mark(MarkType::Mark), 1);
            let missing = EdgeuseId::from_index(99);
            assert!(matches!(
                { let mut notifications = NotificationFixture::new(&original); exec.mapped_edge_prefix(&mut kernel, &mut original, first, missing, MarkType::Mark, &BTreeSet::new(), &mut notifications.context()) },
                Err(SolidError::NotFound { entity: EntityRef::Edgeuse(id), .. }) if id == missing
            ));
        }
    }

    #[test]
    fn static_snapshot_preserves_sign_absolute_distance_tolerance_and_flags() {
        let mut brep = source_brep();
        brep.set_tolerance(3.25e-7);
        for distance in [0.125, -0.125, 0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY] {
            for extend_convex_edges in [false, true] {
                for merge_results in [false, true] {
                    let options = OffsetOptions {
                        distance,
                        extend_convex_edges,
                        merge_results,
                    };
                    let exec = configured_executive(&brep, options);
                    assert_eq!(exec.operation(), OffsetOperation::SolidOffset);
                    assert_eq!(exec.offset_distance().to_bits(), distance.abs().to_bits());
                    assert_eq!(exec.inset(), distance < 0.0);
                    assert_eq!(exec.approximation_tol_3d(), brep.tolerance());
                    assert_eq!(exec.tangency_tol_degrees(), (2.0 * PI / 180.0) * 180.0 / PI);
                    assert_eq!(exec.extend_convex_edges(), extend_convex_edges);
                    assert_eq!(exec.merge_results(), merge_results);
                    assert!(!exec.create_offset_solid());
                    assert!(exec.shell_faces().is_empty());
                }
            }
        }
        let exec = configured_executive(
            &brep,
            OffsetOptions {
                distance: f64::NAN,
                ..OffsetOptions::default()
            },
        );
        assert!(exec.offset_distance().is_nan());
        assert!(!exec.inset());
    }

    #[test]
    fn shell_snapshot_keeps_all_flags_and_the_ordered_duplicate_face_list() {
        let mut original = source_brep();
        original.set_tolerance(6.5e-7);
        let faces = [
            FaceId::from_index(9),
            FaceId::from_index(3),
            FaceId::from_index(9),
        ];
        for extend_convex_edges in [false, true] {
            for merge_results in [false, true] {
                for create_offset_solid in [false, true] {
                    let options = ShellOptions {
                        offset: OffsetOptions {
                            distance: -0.25,
                            extend_convex_edges,
                            merge_results,
                        },
                        create_offset_solid,
                        merge_sheet_side_walls: false,
                    };
                    let exec = configured_shell_executive(&original, options, &faces);
                    assert_eq!(exec.operation(), OffsetOperation::SolidOffset);
                    assert_eq!(exec.offset_distance(), 0.25);
                    assert!(exec.inset());
                    assert_eq!(exec.approximation_tol_3d(), original.tolerance());
                    assert_eq!(exec.extend_convex_edges(), extend_convex_edges);
                    assert_eq!(exec.merge_results(), merge_results);
                    assert_eq!(exec.create_offset_solid(), create_offset_solid);
                    assert_eq!(exec.shell_faces(), &faces);
                }
            }
        }
    }

    #[test]
    fn shell_inner_ordinary_status_is_not_collapsed_like_static_offset() {
        let mut original = source_brep();
        let shell = original.add_shell().unwrap();
        original.add_face(shell).unwrap();
        let mut kernel = Kernel::default();
        // Exercise exactly the post-query shell helper: the real public shell
        // entry point must still defer at IsManifoldSolid before reaching it.
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&original);
                run_shell_offset(
                    &mut kernel,
                    &mut original,
                    ShellOptions::default(),
                    &[],
                    &mut notifications.context(),
                )
            },
            SM_ERR_NULL_POINTER,
        );
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&original);
                offset_brep(
                    &mut kernel,
                    &mut original,
                    OffsetOptions::default(),
                    &mut notifications.context(),
                )
            },
            SM_ERR,
        );
        assert!(!kernel.context.doing_boolean());
    }

    #[test]
    fn defaults_are_false_and_the_direct_skip_default_is_false() {
        assert_eq!(OffsetOptions::default().distance, 0.0);
        assert!(!OffsetOptions::default().extend_convex_edges);
        assert!(!OffsetOptions::default().merge_results);
        assert!(!ShellOptions::default().create_offset_solid);
        assert!(!SolidOffsetOptions::default().skip_stitch_merge);
    }

    #[derive(Debug, Default)]
    struct NotificationLog(Vec<(NotifyOperation, ObjectRef, Option<ObjectRef>)>);

    impl crate::geom::context::NotifyCallback for NotificationLog {
        fn execute(
            &mut self,
            operation: NotifyOperation,
            caller: ObjectRef,
            data1: Option<ObjectRef>,
            _data2: Option<ObjectRef>,
            _data3: Option<ObjectRef>,
        ) -> Result<()> {
            self.0.push((operation, caller, data1));
            Ok(())
        }

        fn as_any(&self) -> Option<&dyn core::any::Any> {
            Some(self)
        }
    }

    #[test]
    fn face_geometry_error_precedes_builder_mutation_and_notifications() {
        let (mut original, face) = one_face(false);
        let mut fixture = NotificationFixture::new(&original);
        let mut executive = executive();
        executive
            .allocate_offset_brep(&mut fixture.context())
            .unwrap();
        let original_before = format!("{original:?}");
        let result_before = format!("{:?}", executive.offset_brep());
        let records_before = fixture.records.clone();
        let attributes_before = format!("{:?}", fixture.attributes);
        let mut kernel = Kernel::default();
        kernel
            .context
            .set_user_notify_callback(Some(Box::<NotificationLog>::default()));
        kernel
            .context
            .set_sys_notify_callback(Some(Box::<NotificationLog>::default()));
        assert!(matches!(
            executive.offset_face_prefix(
                &mut kernel,
                &mut original,
                face,
                false,
                &mut fixture.context()
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(format!("{original:?}"), original_before);
        assert_eq!(format!("{:?}", executive.offset_brep()), result_before);
        assert_eq!(fixture.records, records_before);
        assert_eq!(format!("{:?}", fixture.attributes), attributes_before);
        for callback in [
            kernel.context.user_notify_callback(),
            kernel.context.sys_notify_callback(),
        ] {
            let log = callback
                .unwrap()
                .as_any()
                .unwrap()
                .downcast_ref::<NotificationLog>()
                .unwrap();
            assert!(log.0.is_empty());
        }
    }

    #[test]
    fn face_builder_kernel_scope_restores_callbacks_on_noop_and_early_error() {
        for has_surface in [false, true] {
            let (mut original, face) = one_face(true);
            let faceuse = original.face(face).unwrap().faceuse.unwrap();
            let mut fixture = NotificationFixture::new(&original);
            let source_key = original_key(EntityRef::Face(face));
            let offset_key = result_key(EntityRef::Face(face));
            assert_ne!(source_key.key, offset_key.key);
            fixture
                .records
                .get_mut(&source_key.key)
                .unwrap()
                .set_user_index1(17);
            fixture
                .records
                .or_default(offset_key.key)
                .set_user_index1(93);
            let records_before = fixture.records.clone();
            let mut kernel = Kernel::default();
            let event = (NotifyOperation::Copy, source_key, Some(offset_key));
            kernel
                .context
                .set_user_notify_callback(Some(Box::new(NotificationLog(vec![event]))));
            kernel
                .context
                .set_sys_notify_callback(Some(Box::new(NotificationLog(vec![event]))));
            let mut executive = executive();
            let mut surface =
                has_surface.then(|| Surface::from(Plane::from_normal(Vec3::Z, Vec3::Z)));
            // No temporary exists: a non-null surface reaches the builder's actual
            // NER before construction; null preserves its legacy successful no-op.
            let status = executive.build_offset_of_faceuse_with_kernel_context(
                &mut kernel,
                &mut original,
                faceuse,
                &mut surface,
                &crate::offset::offset_geometry_creation::SsiData::default(),
                &mut NotificationStores {
                    records: &mut fixture.records,
                    attributes: &mut fixture.attributes,
                    key_of: &original_key,
                },
                &result_key,
                &result_curve,
            );
            if has_surface {
                assert_status(status, SM_ERR_NULL_POINTER);
                assert!(surface.is_some());
            } else {
                assert_eq!(status, Ok(()));
            }
            assert_eq!(fixture.records, records_before);
            for callback in [
                kernel.context.user_notify_callback(),
                kernel.context.sys_notify_callback(),
            ] {
                let log = callback
                    .unwrap()
                    .as_any()
                    .unwrap()
                    .downcast_ref::<NotificationLog>()
                    .unwrap();
                assert_eq!(log.0, [event]);
            }
        }
    }

    fn wire_use(brep: &mut Brep) -> EdgeuseId {
        use crate::topology::{Edgeuse, EdgeuseOwner, Vertexuse, VertexuseOwner};
        let start = brep.add_vertex(Vec3::ZERO).unwrap();
        let end = brep.add_vertex(Vec3::X).unwrap();
        let edge = brep.add_edge(start, end).unwrap();
        brep.edge_mut(edge).unwrap().curve = Some(crate::curve::curve::Curve::from(
            crate::curve::line::Line::from_points(Vec3::ZERO, Vec3::X, 3),
        ));
        brep.edge_mut(edge).unwrap().interval = crate::geom::extent1d::Extent1d::from_raw(0.0, 1.0);
        let mut first = None;
        for (vertex, orientation) in [(start, OrientType::Same), (end, OrientType::Opposite)] {
            let vertexuse = brep
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(vertex),
                    ..Vertexuse::default()
                })
                .unwrap();
            let edgeuse = brep
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    vertexuse: Some(vertexuse),
                    owner: EdgeuseOwner::Shell(None),
                    orientation,
                    ..Edgeuse::default()
                })
                .unwrap();
            brep.vertexuse_mut(vertexuse).unwrap().owner = VertexuseOwner::Edgeuse(Some(edgeuse));
            brep.vertex_mut(vertex).unwrap().vertexuses.push(vertexuse);
            brep.edge_mut(edge).unwrap().edgeuses.push(edgeuse);
            first.get_or_insert(edgeuse);
        }
        first.unwrap()
    }

    #[test]
    fn mapped_builder_notifies_real_kernel_and_copies_disjoint_records_and_attributes() {
        use crate::geom::attribute::{Attribute, Behavior};
        let mut original = source_brep();
        let old = wire_use(&mut original);
        let mut result = Brep::new_with_infinite_region().unwrap();
        let result_brep = result_key(EntityRef::Brep(BrepId::from_index(0)));
        result.set_key(result_brep.key);
        let new = wire_use(&mut result);
        let mut fixture = NotificationFixture::new(&original);
        let entities = [
            (EntityRef::Edge(EdgeId::from_index(0)), 37),
            (
                EntityRef::Vertex(crate::topology::VertexId::from_index(0)),
                11,
            ),
            (
                EntityRef::Vertex(crate::topology::VertexId::from_index(1)),
                53,
            ),
        ];
        for (entity, value) in entities {
            fixture
                .records
                .get_mut(&original_key(entity).key)
                .unwrap()
                .set_user_index1(value);
            fixture
                .records
                .insert(result_key(entity).key, Topology::new());
        }
        let attribute = fixture
            .attributes
            .insert(Attribute::new(77, Behavior::Copy))
            .unwrap();
        fixture
            .attributes
            .add_attribute(original_key(entities[0].0).key, attribute)
            .unwrap();
        let mut kernel = Kernel::default();
        kernel
            .context
            .set_user_notify_callback(Some(Box::<NotificationLog>::default()));
        let mut outer =
            MarkLock::new(std::slice::from_mut(&mut kernel.context), 0, ALL_MARKS).unwrap();
        let mut exec = executive();
        exec.set_offset_brep(result);
        exec.relate_edgeuses(old, new);
        let status = with_mark_lock(&mut kernel, ALL_MARKS, |kernel, mark| {
            assert_eq!(mark, MarkType::Mark2);
            let current = kernel.context.current_mark(mark);
            let result = exec.mapped_edge_prefix(
                kernel,
                &mut original,
                old,
                old,
                mark,
                &BTreeSet::new(),
                &mut fixture.context(),
            );
            assert_eq!(kernel.context.current_mark(mark), current);
            assert!(kernel.context.is_mark_locked(mark));
            result
        })
        .unwrap();
        // EdgeuseOffset dereferences the wire use's null face (undefined upstream)
        // only after the edge and vertex notifications below.
        assert!(
            matches!(&status, Err(SolidError::InvalidInput { reason, .. }) if *reason == "edge use without face"),
            "{status:?}"
        );
        for (entity, value) in entities {
            assert_ne!(original_key(entity).key, result_key(entity).key);
            assert_eq!(
                fixture
                    .records
                    .get(&original_key(entity).key)
                    .unwrap()
                    .user_index1(),
                value
            );
            assert_eq!(
                fixture
                    .records
                    .get(&result_key(entity).key)
                    .unwrap()
                    .user_index1(),
                value
            );
        }
        assert!(fixture
            .attributes
            .find_attribute(result_key(entities[0].0).key, 77)
            .is_some());
        let log = kernel
            .context
            .user_notify_callback()
            .unwrap()
            .as_any()
            .unwrap()
            .downcast_ref::<NotificationLog>()
            .unwrap();
        let expected: Vec<_> = entities
            .into_iter()
            .map(|(entity, _)| {
                (
                    NotifyOperation::Copy,
                    original_key(entity),
                    Some(result_key(entity)),
                )
            })
            .collect();
        assert_eq!(log.0, expected);
        assert!(kernel.context.is_mark_locked(MarkType::Mark));
        assert!(!kernel.context.is_mark_locked(MarkType::Mark2));
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
        assert_eq!(kernel.context.current_mark(MarkType::Mark2), 3);
        outer
            .release(std::slice::from_mut(&mut kernel.context))
            .unwrap();
    }

    #[test]
    fn kernel_context_synchronization_preserves_result_keys_on_post_split_error() {
        let mut original = source_brep();
        let old = wire_use(&mut original);
        let other = wire_use(&mut original);
        let mut kernel = Kernel::default();
        let mut result = Brep::new_with_infinite_region().unwrap();
        result.set_key(result_key(EntityRef::Brep(BrepId::from_index(0))).key);
        result.set_editing_enabled(true);
        let whole = wire_use(&mut result);
        let part = wire_use(&mut result);
        let part_edge = eu::edge(&result, part).unwrap().unwrap();
        let part_interval = crate::geom::extent1d::Extent1d::from_raw(0.25, 1.0);
        result.edge_mut(part_edge).unwrap().interval = part_interval;
        result
            .edge_mut(part_edge)
            .unwrap()
            .curve
            .as_mut()
            .unwrap()
            .edit_parameterization(&kernel, &part_interval)
            .unwrap();
        let mut fixture = NotificationFixture::new(&original);
        let source_records = (*fixture.records).clone();
        let entities = result
            .edges()
            .iter()
            .map(|(id, _)| EntityRef::Edge(id))
            .chain(
                result
                    .edgeuses()
                    .iter()
                    .map(|(id, _)| EntityRef::Edgeuse(id)),
            )
            .chain(
                result
                    .vertices()
                    .iter()
                    .map(|(id, _)| EntityRef::Vertex(id)),
            )
            .chain(
                result
                    .vertexuses()
                    .iter()
                    .map(|(id, _)| EntityRef::Vertexuse(id)),
            )
            .chain(result.regions().iter().map(|(id, _)| EntityRef::Region(id)));
        for entity in entities {
            fixture
                .records
                .insert(result_key(entity).key, Topology::new());
        }
        kernel
            .context
            .set_user_notify_callback(Some(Box::<NotificationLog>::default()));
        let mut exec = executive();
        exec.set_offset_brep(result);
        exec.relate_edgeuses(old, part);
        exec.relate_edgeuses(other, whole);
        // This ownerless wire fixture splits, but has no cw/ccw neighbours for
        // insert_breaks to append. Preserve the real partial edit and callbacks
        // on the ensuing null-pointer status, not a fabricated synchronized list.
        assert_status(
            exec.synchronize_offset_edgeuses_with_kernel_context(
                &mut kernel,
                Some(old),
                Some(other),
                &mut NotificationStores {
                    records: &mut fixture.records,
                    attributes: &mut fixture.attributes,
                    key_of: &result_key,
                },
                &result_curve,
            ),
            1006,
        );
        assert_eq!(exec.offset_brep().unwrap().edges().len(), 3);
        assert_eq!(exec.offset_brep().unwrap().vertices().len(), 5);
        for (key, record) in source_records {
            assert_eq!(fixture.records.get(&key), Some(&record));
        }
        let whole_edge = eu::edge(exec.offset_brep().unwrap(), whole)
            .unwrap()
            .unwrap();
        let log = kernel
            .context
            .user_notify_callback()
            .unwrap()
            .as_any()
            .unwrap()
            .downcast_ref::<NotificationLog>()
            .unwrap();
        assert!(log.0.iter().any(
            |(operation, caller, _)| *operation == NotifyOperation::PreEdit
                && *caller == result_key(EntityRef::Edge(whole_edge))
        ));
        assert!(log
            .0
            .iter()
            .all(|(_, caller, _)| caller.key.0 >= 200_000_000_000));
        assert!(!kernel.context.doing_boolean());
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 1);
    }

    #[test]
    fn null_and_invalid_selection_guards_do_not_touch_notification_capability() {
        use crate::geom::attribute::{Attribute, Behavior};
        let (original, face) = one_face(true);
        let mut fixture = NotificationFixture::new(&original);
        let attribute = fixture
            .attributes
            .insert(Attribute::new(81, Behavior::Copy))
            .unwrap();
        fixture
            .attributes
            .add_attribute(original.key(), attribute)
            .unwrap();
        let before = fixture.records.clone();
        let never_key =
            |_: EntityRef| -> ObjectRef { panic!("guard called a topology key callback") };
        let never_curve =
            |_: EdgeId, _: i64| -> ObjectRef { panic!("guard called a curve key callback") };
        let mut kernel = Kernel::default();
        {
            let mut notifications = OffsetNotifications {
                records: &mut fixture.records,
                attributes: &mut fixture.attributes,
                original_key_of: &never_key,
                result_key_of: &never_key,
                original_curve_ref: &never_curve,
                result_curve_ref: &never_curve,
                boolean_source_lists: &|_, _| panic!("guard called a source-list callback"),
                result_brep: result_key(EntityRef::Brep(BrepId::from_index(0))),
                merge_identities: None,
            };
            assert_status(
                api_offset_brep_full(
                    &mut kernel,
                    None,
                    OffsetOptions::default(),
                    &mut notifications,
                ),
                1007,
            );
            assert_status(
                api_shell_brep_full(
                    &mut kernel,
                    None,
                    ShellOptions::default(),
                    &[None],
                    &COPY_IDS,
                    &mut notifications,
                ),
                1007,
            );
            for faces in [
                vec![None],
                vec![Some((BrepId::from_index(8), face))],
                vec![Some((BrepId::from_index(7), FaceId::from_index(99)))],
            ] {
                assert_status(
                    api_shell_brep_full(
                        &mut kernel,
                        Some(source(&original, 7)),
                        ShellOptions::default(),
                        &faces,
                        &COPY_IDS,
                        &mut notifications,
                    ),
                    1007,
                );
            }
            for extended in [false, true] {
                let mut exec = executive();
                exec.set_extend_convex_edges(extended);
                assert_status(
                    exec.do_solid_offset(
                        &mut kernel,
                        None,
                        SolidOffsetOptions::default(),
                        &mut notifications,
                    ),
                    SM_ERR_NULL_POINTER,
                );
                assert!(exec.offset_brep().is_none());
            }
            assert_status(
                executive().do_extended_offset(&mut kernel, None, &mut notifications),
                SM_ERR_NULL_POINTER,
            );
        }
        assert_eq!(fixture.records, before);
        assert_eq!(
            fixture.attributes.find_attribute(original.key(), 81),
            Some(attribute)
        );
        assert!(!kernel.context.doing_boolean());
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 1);
    }

    #[test]
    fn native_api_null_and_member_null_statuses_are_distinct() {
        let mut kernel = Kernel::default();
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&source_brep());
                api_offset_brep_full(
                    &mut kernel,
                    None,
                    OffsetOptions::default(),
                    &mut notifications.context(),
                )
            },
            1007,
        );
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&source_brep());
                api_shell_brep_full(
                    &mut kernel,
                    None,
                    ShellOptions::default(),
                    &[None],
                    &COPY_IDS,
                    &mut notifications.context(),
                )
            },
            1007,
        );
        for extended in [false, true] {
            let mut exec = executive();
            exec.set_extend_convex_edges(extended);
            assert_status(
                {
                    let mut notifications = NotificationFixture::new(&source_brep());
                    exec.do_solid_offset(
                        &mut kernel,
                        None,
                        SolidOffsetOptions::default(),
                        &mut notifications.context(),
                    )
                },
                SM_ERR_NULL_POINTER,
            );
            assert!(exec.offset_brep().is_none());
            assert!(!kernel.context.doing_boolean());
        }
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&source_brep());
                executive().do_extended_offset(&mut kernel, None, &mut notifications.context())
            },
            SM_ERR_NULL_POINTER,
        );
    }

    #[test]
    fn extended_dispatch_ignores_skip_and_never_enters_boolean_scope() {
        let (mut original, _) = one_face(false);
        for previous in [false, true] {
            for skip_stitch_merge in [false, true] {
                let mut kernel = Kernel::default();
                kernel.context.set_doing_boolean(previous);
                let mut exec = executive();
                exec.set_extend_convex_edges(true);
                assert!(matches!(
                    {
                        let mut notifications = NotificationFixture::new(&original);
                        exec.do_solid_offset(
                            &mut kernel,
                            Some(&mut original),
                            SolidOffsetOptions { skip_stitch_merge },
                            &mut notifications.context(),
                        )
                    },
                    Err(SolidError::InvalidInput { .. })
                ));
                assert_eq!(kernel.context.doing_boolean(), previous);
                assert_eq!(exec.offset_brep().unwrap().tolerance(), 1.0e-6);
            }
        }
    }

    #[test]
    fn boolean_scope_restores_on_success_ordinary_error_and_deferred_error() {
        for previous in [false, true] {
            let mut kernel = Kernel::default();
            kernel.context.set_doing_boolean(previous);
            assert_eq!(
                with_doing_boolean(&mut kernel, |kernel| {
                    assert!(kernel.context.doing_boolean());
                    Ok(37)
                }),
                Ok(37)
            );
            assert_eq!(kernel.context.doing_boolean(), previous);
            for error in [
                SolidError::sm_err(OP_SOLID),
                SolidError::unported("SmBrep::ShrinkGeometry", "topology.brep_p4"),
            ] {
                let result: Result<()> = with_doing_boolean(&mut kernel, |kernel| {
                    assert!(kernel.context.doing_boolean());
                    Err(error.clone())
                });
                assert_eq!(result, Err(error));
                assert_eq!(kernel.context.doing_boolean(), previous);
            }
        }
    }

    #[test]
    fn rounded_face_offset_uses_real_infinite_region_before_geometry_creation() {
        let (mut original, _) = one_face(false);
        let mut kernel = Kernel::default();
        for inset in [false, true] {
            let mut exec = executive();
            exec.set_inset(inset);
            assert!(matches!(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    exec.do_solid_offset(
                        &mut kernel,
                        Some(&mut original),
                        SolidOffsetOptions::default(),
                        &mut notifications.context(),
                    )
                },
                Err(SolidError::InvalidInput { .. })
            ));
            assert!(!kernel.context.doing_boolean());
            assert!(exec.offset_brep().unwrap().faces().is_empty());
        }
    }

    #[test]
    fn faces_are_processed_in_native_order_without_grouping_selected_faces_first() {
        let (mut original, first) = one_face(false);
        let shell = original.face(first).unwrap().shell;
        let second = original.add_face(shell).unwrap();
        let mut kernel = Kernel::default();
        for extended in [false, true] {
            let mut exec = executive();
            exec.set_extend_convex_edges(extended);
            exec.set_shell_faces(&[second]);
            assert!(matches!(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    exec.do_solid_offset(
                        &mut kernel,
                        Some(&mut original),
                        SolidOffsetOptions::default(),
                        &mut notifications.context(),
                    )
                },
                Err(SolidError::InvalidInput { .. })
            ));
            let mut exec = executive();
            exec.set_extend_convex_edges(extended);
            exec.set_shell_faces(&[first]);
            // The first face's copy prefix reaches its missing surface before
            // even inspecting the malformed second face's absent faceuse.
            assert_status(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    exec.do_solid_offset(
                        &mut kernel,
                        Some(&mut original),
                        SolidOffsetOptions::default(),
                        &mut notifications.context(),
                    )
                },
                SM_ERR_NULL_POINTER,
            );
        }
    }

    #[test]
    fn empty_solid_always_shrinks_before_skip_or_merge() {
        let mut original = source_brep();
        for previous in [false, true] {
            for skip_stitch_merge in [false, true] {
                for merge in [false, true] {
                    let mut kernel = Kernel::default();
                    kernel.context.set_doing_boolean(previous);
                    let mut exec = executive();
                    exec.set_merge_results(merge);
                    exec.set_create_offset_solid(true);
                    let result = {
                        let mut notifications = NotificationFixture::new(&original);
                        exec.do_solid_offset(
                            &mut kernel,
                            Some(&mut original),
                            SolidOffsetOptions { skip_stitch_merge },
                            &mut notifications.context(),
                        )
                    };
                    assert_eq!(kernel.context.doing_boolean(), previous);
                    if skip_stitch_merge {
                        // The shrunk empty temporary is handed over unstitched.
                        let result = result.unwrap();
                        assert!(result.faces().is_empty());
                        assert_eq!(result.tolerance(), exec.approximation_tol_3d());
                        assert!(exec.offset_brep().is_none());
                    } else if merge {
                        // the empty offset merges into the new merge B-rep, which
                        // takes the original's tolerance; the temporary is deleted
                        let result = result.unwrap();
                        assert!(result.faces().is_empty());
                        assert_eq!(result.tolerance(), original.tolerance());
                        assert!(exec.offset_brep().is_none());
                    } else {
                        assert!(result.unwrap().faces().is_empty());
                        assert!(exec.offset_brep().is_none());
                    }
                }
            }
        }
    }

    #[test]
    fn copied_face_requests_real_curves_and_surface_before_p7_constructor() {
        let mut kernel = Kernel::default();
        for extended in [false, true] {
            let (mut original, face) = one_face(false);
            let mut exec = executive();
            exec.set_shell_faces(&[face, face]);
            exec.set_extend_convex_edges(extended);
            assert_status(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    exec.do_solid_offset(
                        &mut kernel,
                        Some(&mut original),
                        SolidOffsetOptions::default(),
                        &mut notifications.context(),
                    )
                },
                SM_ERR_NULL_POINTER,
            );
            assert_eq!(exec.shell_faces(), &[face, face]);
            assert!(!kernel.context.doing_boolean());
        }
        for extended in [false, true] {
            let (mut original, face) = one_face(true);
            let mut exec = executive();
            exec.set_shell_faces(&[face]);
            exec.set_extend_convex_edges(extended);
            // The loop-free fixture reaches p7's real input guard. The temporary
            // supplies its actual infinite region, not p7's stale None fallback.
            let mut notifications = NotificationFixture::new(&original);
            let error = exec
                .do_solid_offset(
                    &mut kernel,
                    Some(&mut original),
                    SolidOffsetOptions::default(),
                    &mut notifications.context(),
                )
                .unwrap_err();
            assert!(matches!(
                error,
                SolidError::InvalidInput {
                    operation: "topology::brep::p7",
                    ..
                }
            ));
            assert!(exec.offset_brep().unwrap().faces().is_empty());
        }
    }

    #[test]
    fn extension_reaches_real_replace_surface_with_result_identity_and_live_callback() {
        let mut kernel = Kernel::default();
        kernel
            .context
            .set_user_notify_callback(Some(Box::<NotificationLog>::default()));
        let (mut result, face) = one_face(true);
        let face_ref = result_key(EntityRef::Face(face));
        result.set_key(result_key(EntityRef::Brep(BrepId::from_index(0))).key);
        let old_domain = result.face(face).unwrap().uv_domain;
        result.face_mut(face).unwrap().rectangular_trim = 1;
        let mut fixture = NotificationFixture::new(&source_brep());
        let mut record = Topology::new();
        record.set_user_index1(912);
        fixture.records.insert(face_ref.key, record);
        let mut exec = executive();
        exec.insert_extended_surface(
            face_ref.key.0,
            Surface::from(Plane::from_normal(Vec3::Z, Vec3::Z)),
        );
        exec.set_offset_brep(result);
        assert_eq!(
            exec.extend_result_faces_prefix(&mut kernel, MarkType::Mark, &mut fixture.context()),
            Ok(())
        );
        // ReplaceSurface ran in full: PRE_EDIT, SetSurface's CHANGE_GEOMETRY (with the
        // surfaces' identities) and POST_EDIT, then the extended surface replaced the
        // face's surface and its trim domain was chosen
        let result = exec.offset_brep().unwrap();
        assert_eq!(result.faces().len(), 1);
        assert_eq!(
            result.face(face).unwrap().rectangular_trim,
            crate::topology::face::p2::UNSURE
        );
        assert_ne!(result.face(face).unwrap().uv_domain, old_domain);
        let record_after = fixture.records.get(&face_ref.key).unwrap();
        assert_eq!(record_after.user_index1(), 912);
        assert!(record_after.is_marked(&kernel.context, MarkType::Mark));
        let log = kernel
            .context
            .user_notify_callback()
            .unwrap()
            .as_any()
            .unwrap()
            .downcast_ref::<NotificationLog>()
            .unwrap();
        // two ReplaceSurface calls, each PRE_EDIT, CHANGE_GEOMETRY (the new plane's
        // identity as data 1) and POST_EDIT on the face
        let ops: Vec<_> = log.0.iter().map(|(op, caller, _)| (*op, *caller)).collect();
        let round = [
            (NotifyOperation::PreEdit, face_ref),
            (NotifyOperation::ChangeGeometry, face_ref),
            (NotifyOperation::PostEdit, face_ref),
        ];
        assert_eq!(ops, [round, round].concat());
        for (op, _, data1) in &log.0 {
            if *op == NotifyOperation::ChangeGeometry {
                assert_eq!(data1.map(|d| d.type_code), Some(15011));
            } else {
                assert_eq!(*data1, Some(face_ref));
            }
        }
    }

    #[test]
    fn extended_faces_skip_marked_before_geometry_or_notification() {
        let mut kernel = Kernel::default();
        kernel
            .context
            .set_user_notify_callback(Some(Box::<NotificationLog>::default()));
        let (result, face_id) = one_face(true);
        let original_trim = result.face(face_id).unwrap().rectangular_trim;
        let key = result_key(EntityRef::Face(face_id));
        let mut fixture = NotificationFixture::new(&source_brep());
        let mut record = Topology::new();
        record.set_user_index1(321);
        record.mark(Some(&kernel.context), MarkType::Mark);
        fixture.records.insert(key.key, record);
        let original_records = fixture.records.clone();
        let mut exec = executive();
        exec.insert_extended_surface(
            key.key.0,
            Surface::from(Plane::from_normal(Vec3::Z, Vec3::Z)),
        );
        exec.set_offset_brep(result);
        assert_eq!(
            exec.extend_result_faces_prefix(&mut kernel, MarkType::Mark, &mut fixture.context()),
            Ok(())
        );
        let result = exec.offset_brep().unwrap();
        assert_eq!(
            result.face(face_id).unwrap().rectangular_trim,
            original_trim
        );
        assert_eq!(fixture.records, original_records);
        assert_eq!(fixture.records.get(&key.key).unwrap().user_index1(), 321);
        let log = kernel
            .context
            .user_notify_callback()
            .unwrap()
            .as_any()
            .unwrap()
            .downcast_ref::<NotificationLog>()
            .unwrap();
        assert!(log.0.is_empty());
    }

    #[test]
    fn extended_face_is_marked_before_real_replace_surface() {
        let mut kernel = Kernel::default();
        let (result, face_id) = one_face(true);
        let key = result_key(EntityRef::Face(face_id));
        let mut fixture = NotificationFixture::new(&source_brep());
        let mut record = Topology::new();
        record.set_user_index1(654);
        fixture.records.insert(key.key, record);
        let mut exec = executive();
        exec.insert_extended_surface(
            key.key.0,
            Surface::from(Plane::from_normal(Vec3::Z, Vec3::Z)),
        );
        exec.set_offset_brep(result);
        assert_eq!(
            exec.extend_result_faces_prefix(&mut kernel, MarkType::Mark, &mut fixture.context()),
            Ok(())
        );
        let record = fixture.records.get(&key.key).unwrap();
        assert_eq!(record.user_index1(), 654);
        assert!(record.is_marked(&kernel.context, MarkType::Mark));
    }

    #[test]
    fn trim_domain_failure_is_generic_sm_err() {
        let original = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let extension = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        let outside = Extent2d::from_bounds(-2.0, -2.0, 3.0, 3.0);
        assert_status(
            choose_extended_domain(
                &outside,
                &original,
                &extension,
                crate::math::EFF_ZERO,
                || Ok(()),
            ),
            SM_ERR,
        );
    }

    #[test]
    fn empty_extended_uses_p1_stitch_choice_and_releases_its_mark_on_error() {
        let mut original = source_brep();
        for create_solid in [false, true] {
            for previous in [false, true] {
                let mut kernel = Kernel::default();
                kernel.context.set_doing_boolean(previous);
                let mut exec = executive();
                exec.set_create_offset_solid(create_solid);
                assert_reaches_merge({
                    let mut notifications = NotificationFixture::new(&original);
                    exec.do_extended_offset(
                        &mut kernel,
                        Some(&mut original),
                        &mut notifications.context(),
                    )
                });
                assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
                assert!(!kernel.context.is_mark_locked(MarkType::Mark));
                assert_eq!(kernel.context.doing_boolean(), previous);
            }
        }
    }

    #[test]
    fn extended_scope_preserves_an_outer_mark_lock_and_releases_its_own() {
        let mut original = source_brep();
        let mut kernel = Kernel::default();
        let mut outer =
            MarkLock::new(std::slice::from_mut(&mut kernel.context), 0, ALL_MARKS).unwrap();
        assert_eq!(outer.mark_type(), MarkType::Mark);
        assert_reaches_merge({
            let mut notifications = NotificationFixture::new(&original);
            executive().do_extended_offset(
                &mut kernel,
                Some(&mut original),
                &mut notifications.context(),
            )
        });
        assert!(kernel.context.is_mark_locked(MarkType::Mark));
        assert!(!kernel.context.is_mark_locked(MarkType::Mark2));
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
        assert_eq!(kernel.context.current_mark(MarkType::Mark2), 3);
        outer
            .release(std::slice::from_mut(&mut kernel.context))
            .unwrap();
    }

    #[test]
    fn stale_mapped_edgeuse_is_not_silently_skipped_on_the_empty_extended_path() {
        let mut original = source_brep();
        for extended in [false, true] {
            let mut kernel = Kernel::default();
            let mut exec = executive();
            exec.set_extend_convex_edges(extended);
            let missing = EdgeuseId::from_index(9);
            exec.relate_edgeuses(missing, EdgeuseId::from_index(4));
            let result = {
                let mut notifications = NotificationFixture::new(&original);
                exec.do_extended_offset(
                    &mut kernel,
                    Some(&mut original),
                    &mut notifications.context(),
                )
            };
            assert!(matches!(
                result,
                Err(SolidError::NotFound { entity: EntityRef::Edgeuse(id), .. })
                    if id == missing
            ));
            assert!(exec.offset_brep().is_some());
            assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
            assert!(!kernel.context.is_mark_locked(MarkType::Mark));
        }
    }

    #[test]
    fn empty_extended_merge_reaches_remove_faces_even_without_candidate_faces() {
        let mut original = source_brep();
        let mut kernel = Kernel::default();
        let mut exec = executive();
        exec.set_merge_results(true);
        let result = {
            let mut notifications = NotificationFixture::new(&original);
            exec.do_extended_offset(
                &mut kernel,
                Some(&mut original),
                &mut notifications.context(),
            )
        };
        // the empty offset merges into an empty result
        assert!(result.unwrap().faces().is_empty());
        // RemoveFaces ran with the temporary's real editing flag before the
        // piecewise merge, and again in RemoveExcessFaces on the merged result
        // (cpp:1772), releasing its nested mark each time.
        assert_eq!(kernel.context.current_mark(MarkType::Mark2), 5);
        assert!(!kernel.context.is_mark_locked(MarkType::Mark2));
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
        assert!(!kernel.context.is_mark_locked(MarkType::Mark));
    }

    #[test]
    fn nonempty_shell_list_skips_remove_faces_but_not_piecewise_merge_shrink() {
        let mut original = source_brep();
        let mut kernel = Kernel::default();
        let mut exec = executive();
        exec.set_merge_results(true);
        exec.set_shell_faces(&[FaceId::from_index(0)]);
        let result = {
            let mut notifications = NotificationFixture::new(&original);
            exec.do_extended_offset(
                &mut kernel,
                Some(&mut original),
                &mut notifications.context(),
            )
        };
        assert!(result.unwrap().faces().is_empty());
        assert!(!kernel.context.is_mark_locked(MarkType::Mark));
    }

    #[test]
    fn static_offset_collapses_only_ordinary_statuses() {
        for error in [
            SolidError::null_pointer(OP_SOLID),
            SolidError::invalid_input(OP_SOLID, "bad topology"),
            SolidError::Upstream {
                operation: OP_SOLID,
                code: 1023,
                entity: None,
            },
        ] {
            assert_status(collapse_offset_status::<()>(Err(error)), SM_ERR);
        }
        assert_eq!(collapse_offset_status(Ok(17)), Ok(17));
        assert_dependency(
            collapse_offset_status::<()>(Err(SolidError::unported(
                "SmBrep::ShrinkGeometry",
                "topology.brep_p4",
            ))),
            "SmBrep::ShrinkGeometry",
            "topology.brep_p4",
        );

        let mut original = source_brep();
        let shell = original.add_shell().unwrap();
        original.add_face(shell).unwrap(); // native invalid/dereferenced faceuse
        let mut kernel = Kernel::default();
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&original);
                offset_brep(
                    &mut kernel,
                    &mut original,
                    OffsetOptions::default(),
                    &mut notifications.context(),
                )
            },
            SM_ERR,
        );
        assert!(!kernel.context.doing_boolean());
    }

    #[test]
    fn static_and_api_offset_flags_select_the_real_empty_extended_path() {
        let mut original = source_brep();
        for extended in [false, true] {
            let mut kernel = Kernel::default();
            let options = OffsetOptions {
                distance: -0.25,
                extend_convex_edges: extended,
                merge_results: false,
            };
            let result = {
                let mut notifications = NotificationFixture::new(&original);
                api_offset_brep_full(
                    &mut kernel,
                    Some(&mut original),
                    options,
                    &mut notifications.context(),
                )
            };
            if extended {
                assert_reaches_merge(result);
            } else {
                // The rounded empty offset now completes.
                assert!(result.unwrap().faces().is_empty());
            }
            assert!(!kernel.context.doing_boolean());
        }
    }

    #[test]
    fn shell_boolean_continuation_uses_the_production_dependency_not_self_deferral() {
        for distance in [-0.25, 0.25, -0.0, 0.0] {
            let original = source_brep();
            let key = original.key();
            let mut fixture = NotificationFixture::new(&original);
            let mut input = Some(original);
            let mut kernel = Kernel::default();
            let mut shell = Brep::new();
            shell.set_key(result_key(EntityRef::Brep(BrepId::from_index(0))).key);
            let mut lock = MarkLock::new(std::slice::from_mut(&mut kernel.context), 0, 1).unwrap();
            let mark = lock.mark_type();
            let before = kernel.context.current_mark(mark);
            assert_reaches_merge(shell_difference(
                &mut kernel,
                &mut input,
                shell,
                distance,
                1.0e-6,
                &mut fixture.context(),
            ));
            // (an inset keeps the original as the primary operand; an outset's original is
            // the other operand, which a late engine error may already have consumed)
            if distance < 0.0 {
                assert_eq!(input.as_ref().unwrap().key(), key);
            }
            assert!(fixture.records.contains_key(&key));
            assert!(kernel.context.is_mark_locked(mark));
            assert_eq!(kernel.context.current_mark(mark), before);
            lock.release(std::slice::from_mut(&mut kernel.context))
                .unwrap();
        }
    }

    #[test]
    fn latent_shell_auto_tolerance_reaches_the_real_bounding_box_dependency() {
        for distance in [-0.25, 0.25, -0.0, 0.0] {
            let original = source_brep();
            let original_key_value = original.key();
            let mut fixture = NotificationFixture::new(&original);
            let mut input = Some(original);
            let mut kernel = Kernel::default();
            let result = shell_difference(
                &mut kernel,
                &mut input,
                Brep::new(),
                distance,
                crate::math::EFF_ZERO * 0.5,
                &mut fixture.context(),
            );
            // ManifoldBoolean resets DoingBoolean only on success: its SER exits leave it
            // set (SmMerge.cpp:2551, 3114, 4148)
            assert_eq!(kernel.context.doing_boolean(), result.is_err());
            assert_reaches_merge(result);
            if distance < 0.0 {
                assert_eq!(input.as_ref().unwrap().key(), original_key_value);
            }
            assert!(fixture.records.contains_key(&original_key_value));
        }
    }

    #[test]
    fn latent_shell_boolean_orders_operands_and_reports_real_consumption_on_every_exit() {
        use crate::boolean::merge::p4::{ManifoldIntersector, MergeEntity};
        for distance in [-0.25, 0.25, -0.0, 0.0] {
            for exit in [0, 1, 2] {
                // success, early error, post-consumption error
                let original = source_brep();
                let original_key_value = original.key();
                let mut fixture = NotificationFixture::new(&original);
                let mut input = Some(original);
                let mut shell = Brep::new();
                let shell_key = result_key(EntityRef::Brep(BrepId::from_index(0))).key;
                shell.set_key(shell_key);
                let mut kernel = Kernel::default();
                kernel.context.set_doing_boolean(true);
                let mut lock =
                    MarkLock::new(std::slice::from_mut(&mut kernel.context), 0, 1).unwrap();
                let mark = lock.mark_type();
                let before = kernel.context.current_mark(mark);
                let result = shell_difference_with(
                    &mut kernel,
                    &mut input,
                    shell,
                    distance,
                    3.125e-7,
                    &mut fixture.context(),
                    |adapter, kernels, primary, other, _| {
                        let inset = distance < 0.0;
                        assert_eq!(
                            primary.as_ref().unwrap().key(),
                            if inset { original_key_value } else { shell_key }
                        );
                        assert_eq!(
                            other.as_ref().unwrap().key(),
                            if inset { shell_key } else { original_key_value }
                        );
                        let vertex = EntityRef::Vertex(crate::topology::VertexId::from_index(0));
                        assert_eq!(
                            adapter.object_key(
                                false,
                                MergeEntity::Vertex(crate::topology::VertexId::from_index(0))
                            ),
                            (if inset { original_key } else { result_key })(vertex).key
                        );
                        assert_eq!(
                            adapter.intersection_tolerances(),
                            (3.125e-7, 20.0 * PI / 180.0)
                        );
                        assert_eq!(
                            adapter.merge.options,
                            crate::boolean::merge::p1::MergeOptions::default()
                        );
                        assert!(kernels.other.is_none());
                        assert!(kernels.primary.context.doing_boolean());
                        assert!(kernels.primary.context.is_mark_locked(mark));
                        assert_eq!(kernels.primary.context.current_mark(mark), before);
                        if exit == 1 {
                            return Err(SolidError::sm_err("test::early"));
                        }
                        *other = None;
                        if exit == 2 {
                            return Err(SolidError::unported(
                                "SmBrep::DeleteStaleCaches",
                                "topology.brep_p2",
                            ));
                        }
                        Ok(true)
                    },
                );
                if exit == 0 {
                    assert_eq!(
                        result.unwrap().key(),
                        if distance < 0.0 {
                            original_key_value
                        } else {
                            shell_key
                        }
                    );
                    assert!(input.is_none());
                } else {
                    assert!(result.is_err());
                    assert_eq!(input.is_some(), exit == 1 || distance < 0.0);
                }
                assert!(fixture.records.contains_key(&original_key_value));
                assert!(fixture.records.contains_key(&shell_key));
                assert!(kernel.context.doing_boolean());
                assert!(kernel.context.is_mark_locked(mark));
                lock.release(std::slice::from_mut(&mut kernel.context))
                    .unwrap();
            }
        }
    }

    #[test]
    fn latent_shell_boolean_routes_region_attributes_and_real_callbacks_after_operand_swap() {
        use crate::boolean::merge::p4::{ManifoldIntersector, MergeEntity};
        use crate::geom::attribute::{ids, Attribute, Behavior};
        for distance in [-0.25, 0.25, -0.0, 0.0] {
            let mut original = source_brep();
            let region = original
                .add_region_record(Region {
                    in_brep: true,
                    ..Region::default()
                })
                .unwrap();
            original.region_mut(region).unwrap().set_is_void(0);
            let mut shell = Brep::new();
            shell.set_key(result_key(EntityRef::Brep(BrepId::from_index(0))).key);
            assert_eq!(
                shell
                    .add_region_record(Region {
                        in_brep: true,
                        ..Region::default()
                    })
                    .unwrap(),
                region
            );
            shell.region_mut(region).unwrap().set_is_void(0);
            let mut fixture = NotificationFixture::new(&original);
            fixture
                .records
                .insert(result_key(EntityRef::Region(region)).key, Topology::new());
            let inset = distance < 0.0;
            let primary_key =
                (if inset { original_key } else { result_key })(EntityRef::Region(region)).key;
            let other_key =
                (if inset { result_key } else { original_key })(EntityRef::Region(region)).key;
            let merge = fixture
                .attributes
                .insert(Attribute::merge_region(primary_key, 0, other_key, 0))
                .unwrap();
            fixture
                .attributes
                .add_attribute(primary_key, merge)
                .unwrap();
            let color = fixture
                .attributes
                .insert(Attribute::color(81, Vec3::Y, Behavior::Copy))
                .unwrap();
            fixture
                .attributes
                .add_attribute(primary_key, color)
                .unwrap();
            let mut kernel = Kernel::default();
            kernel
                .context
                .set_user_notify_callback(Some(Box::new(NotificationLog::default())));
            let mut input = Some(original);
            assert_status(
                shell_difference_with(
                    &mut kernel,
                    &mut input,
                    shell,
                    distance,
                    1.0e-6,
                    &mut fixture.context(),
                    |adapter, kernels, primary, other, attributes| {
                        assert_eq!(
                            adapter.object_key(false, MergeEntity::Region(region)),
                            primary_key
                        );
                        adapter.propagate_regions(
                            kernels,
                            primary.as_ref().unwrap(),
                            other.as_ref().unwrap(),
                            attributes,
                            BooleanOperation::Difference,
                        )?;
                        Err(SolidError::sm_err("test::after-notify"))
                    },
                ),
                SM_ERR,
            );
            let log = kernel
                .context
                .user_notify_callback()
                .unwrap()
                .as_any()
                .unwrap()
                .downcast_ref::<NotificationLog>()
                .unwrap();
            let notice = log
                .0
                .iter()
                .find(|notice| notice.0 == NotifyOperation::RegPropagation)
                .unwrap();
            assert_eq!(notice.1.key, primary_key);
            assert_eq!(notice.2, Some(boolean_source_lists(!inset, region)[0]));
            assert!(fixture
                .attributes
                .find_attribute(primary_key, ids::OTHER_REGION_ID)
                .is_none());
            assert!(fixture.attributes.find_attribute(primary_key, 81).is_some());
            assert!(fixture.records.contains_key(&other_key));
            assert!(input.is_some());
        }
    }

    #[test]
    fn shell_manifold_query_precedes_strict_distance_guard_and_retains_input() {
        for distance in [0.0, -0.0, 1.0e-30, -1.0e-30, 0.25, -0.25] {
            let mut original = source_brep();
            original.set_tolerance(1.0e-5);
            let mut kernel = Kernel::default();
            let outcome = {
                let mut notifications = NotificationFixture::new(&original);
                OffsetExecutive::shell_brep(
                    &mut kernel,
                    original,
                    ShellOptions {
                        offset: OffsetOptions {
                            distance,
                            extend_convex_edges: true,
                            merge_results: true,
                        },
                        create_offset_solid: true,
                        merge_sheet_side_walls: false,
                    },
                    &[],
                    &mut notifications.context(),
                )
            };
            if distance.abs() < 2.0e-5 {
                assert!(!outcome.input_ownership_transferred());
                assert_status(outcome.result, 1007);
                assert_eq!(kernel.context.current_mark(MarkType::Mark), 1);
                let retained = outcome.retained_input.as_ref().unwrap();
                assert_eq!(retained.tolerance(), 1.0e-5);
                assert_eq!(retained.key(), source_brep().key());
                assert!(retained.faces().is_empty());
                assert!(retained.edges().is_empty());
                assert!(retained.vertices().is_empty());
                assert!(retained.regions().is_empty());
            } else {
                // the empty offset merges; a solid shell of a non-manifold input
                // merges the (empty) input in, consuming it, and stitches
                assert!(outcome.input_ownership_transferred());
                let shell = outcome.result.unwrap();
                assert!(shell.faces().is_empty());
                assert_eq!(kernel.context.current_mark(MarkType::Mark), 3);
            }
            assert!(!kernel.context.is_mark_locked(MarkType::Mark));
            assert!(!kernel.context.doing_boolean());
        }
    }

    #[test]
    fn shell_distance_boundary_is_strict_without_extra_finite_checks() {
        let tolerance: f64 = 0.125;
        let boundary = 2.0 * tolerance;
        assert_status(check_shell_radius(0.0, tolerance), 1007);
        assert_status(check_shell_radius(-0.0, tolerance), 1007);
        assert_status(
            check_shell_radius(f64::from_bits(boundary.to_bits() - 1), tolerance),
            1007,
        );
        assert_eq!(check_shell_radius(boundary, tolerance), Ok(()));
        assert_eq!(
            check_shell_radius(f64::from_bits(boundary.to_bits() + 1), tolerance),
            Ok(())
        );
        assert_eq!(check_shell_radius(f64::INFINITY, tolerance), Ok(()));
        assert_eq!(check_shell_radius(f64::NAN, tolerance), Ok(()));
        assert_eq!(check_shell_radius(0.0, 0.0), Ok(()));
        assert_eq!(check_shell_radius(0.0, -1.0), Ok(()));
    }

    #[test]
    fn shell_api_rejects_null_foreign_and_out_of_range_selection_before_copy() {
        let (original, face) = one_face(true);
        let mut kernel = Kernel::default();
        let good = Some((BrepId::from_index(7), face));
        for faces in [
            vec![None],
            vec![good, None],
            vec![Some((BrepId::from_index(8), face))],
            vec![good, Some((BrepId::from_index(8), face))],
            vec![Some((BrepId::from_index(7), FaceId::from_index(99)))],
        ] {
            assert_status(
                {
                    let mut notifications = NotificationFixture::new(&original);
                    api_shell_brep_full(
                        &mut kernel,
                        Some(source(&original, 7)),
                        ShellOptions::default(),
                        &faces,
                        &COPY_IDS,
                        &mut notifications.context(),
                    )
                },
                1007,
            );
        }
        assert!(original.face(face).unwrap().surface.is_some());
        assert_eq!(original.faces().len(), 1);
        assert_eq!(kernel.context.current_mark(MarkType::Mark), 1);
    }

    #[test]
    fn foreign_face_zero_cannot_alias_source_face_zero() {
        let (original, original_face) = one_face(true);
        let (foreign, foreign_face) = one_face(true);
        assert_eq!(original_face, foreign_face);
        assert_eq!(original_face.index(), 0);
        let mut kernel = Kernel::default();
        assert_status(
            {
                let mut notifications = NotificationFixture::new(&original);
                api_shell_brep_full(
                    &mut kernel,
                    Some(source(&original, 7)),
                    ShellOptions::default(),
                    &[Some((source(&foreign, 8).handle, foreign_face))],
                    &COPY_IDS,
                    &mut notifications.context(),
                )
            },
            1007,
        );
        assert_copied(&{
            let mut notifications = NotificationFixture::new(&original);
            api_shell_brep_full(
                &mut kernel,
                Some(source(&original, 7)),
                ShellOptions::default(),
                &[Some((BrepId::from_index(7), original_face))],
                &COPY_IDS,
                &mut notifications.context(),
            )
        });
    }

    #[test]
    fn api_validates_every_selection_and_keeps_duplicates_before_actual_copy() {
        let (original, face) = one_face(true);
        let good = Some((BrepId::from_index(3), face));
        assert_eq!(
            validate_shell_faces(source(&original, 3), &[good, good]),
            Ok(())
        );
        let mut kernel = Kernel::default();
        for distance in [0.0, -0.0, -0.5, 0.5] {
            assert_copied(&{
                let mut notifications = NotificationFixture::new(&original);
                api_shell_brep_full(
                    &mut kernel,
                    Some(source(&original, 3)),
                    ShellOptions {
                        offset: OffsetOptions {
                            distance,
                            extend_convex_edges: true,
                            merge_results: true,
                        },
                        create_offset_solid: true,
                        merge_sheet_side_walls: false,
                    },
                    &[good, good],
                    &COPY_IDS,
                    &mut notifications.context(),
                )
            });
        }
        // An empty list still copies; it does not jump to IsManifoldSolid or the
        // shell distance check. No source arena or geometry is changed.
        assert_copied(&{
            let mut notifications = NotificationFixture::new(&original);
            api_shell_brep_full(
                &mut kernel,
                Some(source(&original, 3)),
                ShellOptions::default(),
                &[],
                &COPY_IDS,
                &mut notifications.context(),
            )
        });
        assert_eq!(original.faces().len(), 1);
        assert_eq!(
            original.face(face).unwrap().uv_domain,
            Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn malformed_face_error_restores_context_and_retains_allocated_temporary() {
        let mut original = source_brep();
        let shell = original.add_shell().unwrap();
        let face = original.add_face(shell).unwrap();
        original.face_mut(face).unwrap().faceuse = Some(FaceuseId::from_index(99));
        for previous in [false, true] {
            let mut kernel = Kernel::default();
            kernel.context.set_doing_boolean(previous);
            let mut exec = executive();
            let result = {
                let mut notifications = NotificationFixture::new(&original);
                exec.do_solid_offset(
                    &mut kernel,
                    Some(&mut original),
                    SolidOffsetOptions::default(),
                    &mut notifications.context(),
                )
            };
            assert!(matches!(
                result,
                Err(SolidError::NotFound {
                    entity: EntityRef::Faceuse(id),
                    ..
                }) if id == FaceuseId::from_index(99)
            ));
            assert_eq!(kernel.context.doing_boolean(), previous);
            assert_eq!(exec.offset_brep().unwrap().tolerance(), 1.0e-6);
        }
    }

    /// A sheet of two squares side by side, stitched along their shared edge (`x = 10`).
    fn two_square_sheet(k: &mut Kernel) -> Brep {
        use crate::primitive::api_primitives::create_plane;
        let mut sheet = create_plane(k, Vec3::new(0.0, 0.0, 0.0), 10.0, 10.0).unwrap();
        let other = create_plane(k, Vec3::new(10.0, 0.0, 0.0), 10.0, 10.0).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let mut env = crate::topology::edge::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &result_key,
        };
        bp3::merge_brep(k, &mut env, &mut sheet, &other, &copy_key, None, None, true).unwrap();
        stitch_faces(k, &mut sheet, 1.0e-5, None, false).unwrap();
        sheet.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
        sheet
    }

    /// source issue #978: thickening a sheet makes one side wall per lamina
    /// edge, so where two consecutive boundary edges lie on one line their walls are two
    /// faces on one plane. With the opt-in they come back as one face: two squares side
    /// by side thickened 1 give 8 faces (2 sheet, 2 offset, 4 walls) instead of 10; the
    /// sheet's own split stays.
    #[test]
    fn issue_978_thickened_sheet_coplanar_walls_are_one_face() {
        let thicken = |merge: bool| -> Brep {
            let mut kernel = Kernel::default();
            let sheet = two_square_sheet(&mut kernel);
            assert_eq!(sheet.face_list().len(), 2);
            let mut notifications = NotificationFixture::new(&sheet);
            let outcome = OffsetExecutive::shell_brep(
                &mut kernel,
                sheet,
                ShellOptions {
                    offset: OffsetOptions {
                        distance: 1.0,
                        extend_convex_edges: true,
                        merge_results: true,
                    },
                    create_offset_solid: true,
                    merge_sheet_side_walls: merge,
                },
                &[],
                &mut notifications.context(),
            );
            outcome.result.unwrap()
        };
        let plain = thicken(false);
        let merged = thicken(true);
        assert_eq!(plain.face_list().len(), 10);
        assert_eq!(merged.face_list().len(), 8);
        // no planar wall is left on a non-plane surface after the merge
        let planes = merged
            .face_list()
            .iter()
            .filter(|&&f| {
                merged
                    .face(f)
                    .unwrap()
                    .surface
                    .as_ref()
                    .is_some_and(|s| s.is_kind_of(Plane::TYPE).unwrap())
            })
            .count();
        assert!(planes >= 4, "{planes}");
    }

    /// A 30 x 20 x 6 plate with an r = 1.5 through hole whose wall is 1 from the front
    /// side (y = 0): shelled 1.5 inward, the hole's offset crosses the front's.
    fn plate_with_hole_near_side(k: &mut Kernel) -> Brep {
        use crate::primitive::api_primitives::{create_box, create_cylinder};
        let plate = create_box(k, Vec3::new(0.0, 0.0, 0.0), 30.0, 20.0, 6.0).unwrap();
        let hole = create_cylinder(k, Vec3::new(15.0, 2.5, -1.0), 1.5, 8.0).unwrap();
        let mut part = crate::boolean::api_brep::boolean_difference(k, plate, hole).unwrap();
        part.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
        part
    }

    /// source issue #977: for a closed shell (no open face) `RemoveExcessFaces`
    /// ran before the merge, while the offset faces were untrimmed, and took faces that
    /// bound the offset (each with an edge inside the excess); the merged offset then
    /// enclosed no region. With the opt-in the excess is found after the merge only, and
    /// the closed shell of a plate whose hole offset crosses a side is a solid. (The
    /// hole's offset crosses the side's, so both runs also resolve the offset faces'
    /// crossing loops, #854, as the fork did before this fix.)
    #[test]
    fn issue_977_closed_shell_excess_faces_after_merge() {
        let shell = |after_merge: bool| -> Result<Brep> {
            let mut kernel = Kernel::default();
            kernel.fixes.offset_excess_faces_after_merge = after_merge;
            kernel.fixes.offset_resolve_crossing_loops = true;
            let part = plate_with_hole_near_side(&mut kernel);
            let mut notifications = NotificationFixture::new(&part);
            OffsetExecutive::shell_brep(
                &mut kernel,
                part,
                ShellOptions {
                    offset: OffsetOptions {
                        distance: -1.5,
                        extend_convex_edges: true,
                        merge_results: true,
                    },
                    create_offset_solid: false,
                    merge_sheet_side_walls: false,
                },
                &[],
                &mut notifications.context(),
            )
            .result
        };
        let fixed = shell(true).unwrap();
        assert!(brep_is_manifold_solid(&fixed).unwrap());
        // upstream's order (the default) loses the faces round the hole
        match shell(false) {
            Err(_) => {}
            Ok(b) => assert!(!brep_is_manifold_solid(&b).unwrap()),
        }
    }

    /// A frustum (r 20 at z = 0 to r 30 at z = 60, drafted about 9.46 degrees) built as
    /// the union of two half cones, so its side is two faces, and its top face(s).
    fn two_half_frustum(k: &mut Kernel) -> (Brep, Vec<FaceId>) {
        use crate::primitive::api_primitives::create_partial_cone;
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let a = create_partial_cone(k, origin, 20.0, 30.0, 60.0, 0.0, 180.0).unwrap();
        let b = create_partial_cone(k, origin, 20.0, 30.0, 60.0, 180.0, 360.0).unwrap();
        let mut part = crate::boolean::api_brep::boolean_union(k, a, b).unwrap();
        part.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
        let top = planar_faces_at_z(k, &part, 60.0);
        (part, top)
    }

    /// The plane faces of `part` lying at height `z`.
    fn planar_faces_at_z(k: &Kernel, part: &Brep, z: f64) -> Vec<FaceId> {
        part.face_list()
            .iter()
            .copied()
            .filter(|&f| {
                let face = part.face(f).unwrap();
                let s = face.surface.as_ref().unwrap();
                s.is_kind_of(Plane::TYPE).unwrap()
                    && (s.evaluate_point(k, face.uv_domain.mid()).unwrap().z - z).abs() < 1e-6
            })
            .collect()
    }

    /// source issue #469: with extended convex edges each edge's two offset
    /// faces are mated along it (`EdgeuseOffset`), but `PiecewiseMerge`'s `sNoSSI` lists
    /// only cap-face and tangent pairs, so the merge intersects them again; on a frustum
    /// built as two half faces, shelled 2 inward open at the top, the plane/offset-cone
    /// curve stops short of the inner seam and splits the rim. With the opt-in the mated
    /// pairs are related too: 7 faces, 12 edges and 8 vertices.
    #[test]
    fn issue_469_extended_offset_mated_edges_not_intersected_again() {
        let mut kernel = Kernel::default();
        kernel.fixes.offset_extended_edges_mated = true;
        let (part, top) = two_half_frustum(&mut kernel);
        assert!(!top.is_empty());
        let mut notifications = NotificationFixture::new(&part);
        let shell = OffsetExecutive::shell_brep(
            &mut kernel,
            part,
            ShellOptions {
                offset: OffsetOptions {
                    distance: -2.0,
                    extend_convex_edges: true,
                    merge_results: true,
                },
                create_offset_solid: false,
                merge_sheet_side_walls: false,
            },
            &top,
            &mut notifications.context(),
        )
        .result
        .unwrap();
        assert!(brep_is_manifold_solid(&shell).unwrap());
        assert_eq!(
            (
                shell.face_list().len(),
                shell.edge_list().len(),
                shell.vertex_list().len()
            ),
            (7, 12, 8)
        );
    }

    /// source issue #403 (WR-463): a sphere r10 cut by a box to a hemisphere has
    /// a rim of two arcs (270 and 90 degrees) of one circle. Shelled 2 inward open at the
    /// flat face, the offset of that circle is trimmed to the interval between two points
    /// for both arcs, so the flat face gets the long arc twice and the cavity is no solid.
    /// With the opt-in each arc takes the part of the circle it runs along: a solid of
    /// volume 2/3 pi (10^3 - 8^3).
    #[test]
    fn issue_403_hemisphere_rim_arcs_keep_their_own_trim() {
        use crate::primitive::api_primitives::{create_box, create_sphere};
        let shell = |fixed: bool| -> Result<(bool, f64)> {
            let mut kernel = Kernel::default();
            kernel.fixes.offset_closed_curve_trim_arc = fixed;
            let ball = create_sphere(&mut kernel, Vec3::new(0.0, 0.0, 0.0), 10.0).unwrap();
            let upper =
                create_box(&mut kernel, Vec3::new(-20.0, -20.0, 0.0), 40.0, 40.0, 20.0).unwrap();
            let mut part =
                crate::boolean::api_brep::boolean_intersection(&mut kernel, ball, upper).unwrap();
            part.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
            let flat = planar_faces_at_z(&kernel, &part, 0.0);
            assert_eq!(flat.len(), 1);
            let mut notifications = NotificationFixture::new(&part);
            OffsetExecutive::shell_brep(
                &mut kernel,
                part,
                ShellOptions {
                    offset: OffsetOptions {
                        distance: -2.0,
                        extend_convex_edges: true,
                        merge_results: true,
                    },
                    create_offset_solid: false,
                    merge_sheet_side_walls: false,
                },
                &flat,
                &mut notifications.context(),
            )
            .result
            .map(|mut shell| {
                let store = AttributeStore::default();
                let key_of = |_| ObjectKey(0);
                let attributes = crate::mass::integrator::properties::PropertyAttributes {
                    store: &store,
                    key_of: &key_of,
                };
                let volume = crate::mass::integrator::properties::brep_volume(
                    &kernel,
                    &mut shell,
                    1e-6,
                    &attributes,
                )
                .unwrap();
                (brep_is_manifold_solid(&shell).unwrap(), volume)
            })
        };
        // upstream: the cavity is no solid and encloses no volume
        let (solid, volume) = shell(false).unwrap();
        assert!(!solid && volume.abs() < 1e-9, "{solid} {volume}");
        let (solid, volume) = shell(true).unwrap();
        let expected = 2.0 / 3.0 * PI * (1000.0 - 512.0);
        assert!(
            solid && (volume - expected).abs() < 1e-6 * expected,
            "{volume}"
        );
    }

    /// A 40 x 40 x 10 plate with a 4 mm through hole at (4.5, 20), 0.5 from the side
    /// x = 0, and its top face.
    fn plate_with_hole_by_side(k: &mut Kernel) -> (Brep, Vec<FaceId>) {
        use crate::primitive::api_primitives::{create_box, create_cylinder};
        let plate = create_box(k, Vec3::new(0.0, 0.0, 0.0), 40.0, 40.0, 10.0).unwrap();
        let hole = create_cylinder(k, Vec3::new(4.5, 20.0, -1.0), 2.0, 12.0).unwrap();
        let mut part = crate::boolean::api_brep::boolean_difference(k, plate, hole).unwrap();
        part.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
        let top = planar_faces_at_z(k, &part, 10.0);
        (part, top)
    }

    /// source issue #854: an offset face's loops are each trimmed only where
    /// their own edges meet, so shelling this plate 1.5 inward open at the top grows
    /// the hole's offset (r 3.5) past the offset side x = 1.5, and the offset floor's
    /// loops cross; the merge cannot use it. With the opt-in the floor is rebuilt from
    /// the stretches of its loops that bound it, and the shell is a solid of the closed
    /// form's volume (the fork's C++ gives 4554.583397 against 4554.583419).
    #[test]
    fn issue_854_offset_face_crossing_loops_resolved() {
        let mut kernel = Kernel::default();
        kernel.fixes.offset_resolve_crossing_loops = true;
        let (part, top) = plate_with_hole_by_side(&mut kernel);
        assert_eq!(top.len(), 1);
        let mut notifications = NotificationFixture::new(&part);
        let mut shell = OffsetExecutive::shell_brep(
            &mut kernel,
            part,
            ShellOptions {
                offset: OffsetOptions {
                    distance: -1.5,
                    extend_convex_edges: true,
                    merge_results: true,
                },
                create_offset_solid: false,
                merge_sheet_side_walls: false,
            },
            &top,
            &mut notifications.context(),
        )
        .result
        .unwrap();
        assert!(brep_is_manifold_solid(&shell).unwrap());
        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = crate::mass::integrator::properties::PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        let volume = crate::mass::integrator::properties::brep_volume(
            &kernel,
            &mut shell,
            1e-6,
            &attributes,
        )
        .unwrap();
        assert!(
            (volume - 4554.583419).abs() < 1e-6 * 4554.583419,
            "volume {volume}"
        );
    }

    /// source issue #855: a 40 x 40 x 10 plate with a 4 mm hole at (4.99, 20)
    /// shelled 1.5 inward open at the top: the hole's offset (r 3.5) crosses the offset
    /// side x = 1.5 only 0.01 mm deep. Upstream's `StitchIntoSolid` doubles its tolerance
    /// until the cut-off rims of that crossing are welded, and the shell is no solid;
    /// with the opt-in one pass at the body's tolerance leaves a solid whose volume is
    /// the inner box less the hole's offset disk (cut by x = 1.5), over the height 8.5.
    /// `contacts_at_ends` adds the other opt-in of #855 (a tangential contact is found at
    /// a curve's end), which must not change this shell.
    #[test]
    fn issue_855_offset_stitch_keeps_the_bodys_tolerance() {
        let shell = |stitch: bool, contacts_at_ends: bool| -> Result<Option<f64>> {
            use crate::primitive::api_primitives::{create_box, create_cylinder};
            let mut kernel = Kernel::default();
            kernel.fixes.offset_resolve_crossing_loops = true;
            kernel.fixes.offset_stitch_at_body_tolerance = stitch;
            kernel.fixes.curve_contacts_at_curve_ends = contacts_at_ends;
            let plate =
                create_box(&mut kernel, Vec3::new(0.0, 0.0, 0.0), 40.0, 40.0, 10.0).unwrap();
            let hole =
                create_cylinder(&mut kernel, Vec3::new(4.99, 20.0, -1.0), 2.0, 12.0).unwrap();
            let mut part =
                crate::boolean::api_brep::boolean_difference(&mut kernel, plate, hole).unwrap();
            part.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
            let top = planar_faces_at_z(&kernel, &part, 10.0);
            let mut notifications = NotificationFixture::new(&part);
            let mut result = OffsetExecutive::shell_brep(
                &mut kernel,
                part,
                ShellOptions {
                    offset: OffsetOptions {
                        distance: -1.5,
                        extend_convex_edges: true,
                        merge_results: true,
                    },
                    create_offset_solid: false,
                    merge_sheet_side_walls: false,
                },
                &top,
                &mut notifications.context(),
            )
            .result?;
            if !brep_is_manifold_solid(&result)? {
                return Ok(None);
            }
            let store = AttributeStore::default();
            let key_of = |_| ObjectKey(0);
            let attributes = crate::mass::integrator::properties::PropertyAttributes {
                store: &store,
                key_of: &key_of,
            };
            let volume = crate::mass::integrator::properties::brep_volume(
                &kernel,
                &mut result,
                1e-6,
                &attributes,
            )?;
            Ok(Some(volume))
        };
        assert_eq!(shell(false, false).unwrap(), None);
        // 40 x 40 x 10 less the hole, less the cavity: 8.5 high over the inner box (37 x 37)
        // less the hole's offset disk (r 3.5, centre 3.49 from x = 1.5) cut by x = 1.5
        let expected = 4564.924657;
        for contacts_at_ends in [false, true] {
            let volume = shell(true, contacts_at_ends).unwrap().unwrap();
            assert!(
                (volume - expected).abs() < 1e-6 * expected,
                "volume {volume}"
            );
        }
    }

    /// source issue #380: a closed inward shell of a box whose wall is as thick
    /// as (or thicker than) the inradius leaves no cavity. Upstream returns the empty
    /// offset as a success (the fork: the body unhollowed); with the opt-in it fails, and a thinner wall still shells.
    #[test]
    fn issue_380_shell_with_no_cavity_fails_with_the_fix() {
        let shell = |fix: bool, wall: f64| -> Result<Brep> {
            use crate::primitive::api_primitives::create_box;
            let mut kernel = Kernel::default();
            kernel.fixes.shell_empty_cavity_fails = fix;
            let mut part =
                create_box(&mut kernel, Vec3::new(0.0, 0.0, 0.0), 10.0, 10.0, 10.0).unwrap();
            part.set_key(original_key(EntityRef::Brep(BrepId::from_index(0))).key);
            let mut notifications = NotificationFixture::new(&part);
            OffsetExecutive::shell_brep(
                &mut kernel,
                part,
                ShellOptions {
                    offset: OffsetOptions {
                        distance: -wall,
                        extend_convex_edges: true,
                        merge_results: true,
                    },
                    create_offset_solid: false,
                    merge_sheet_side_walls: false,
                },
                &[],
                &mut notifications.context(),
            )
            .result
        };
        for wall in [5.0, 6.0] {
            // upstream: an empty offset is "not a manifold solid", returned as a success
            let upstream = shell(false, wall).unwrap();
            assert_eq!(bp1::num_faces(&upstream), 0);
            assert_status(shell(true, wall), 1007);
        }
        let thin = shell(true, 1.0).unwrap();
        assert!(brep_is_manifold_solid(&thin).unwrap());
    }
}
