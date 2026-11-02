//! `SmBrep::AssertValid`: the whole-body topology check, its walk over every region,
//! shell, face, loop, edge and vertex, and the `check_brep` report source reads.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `SmSAGObject.cpp`, `SmOwningTopology.cpp`,
//! `SmTopology.cpp`, `SmRegion.cpp`, `SmShell.cpp`, `SmFace.cpp`, `SmFaceuse.cpp`,
//! `SmLoop.cpp`, `SmLoopuse.cpp`, `SmEdge.cpp`, `SmEdgeuse.cpp`, `SmVertex.cpp`,
//! `SmVertexuse.cpp`, `SmTopologyTraverser.cpp`, `SmCurve.cpp` (the virtual
//! `AssertValid`), `source/SMLib/inc/SmAssertArray.h` (the report macros).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`assert_valid`] | `SmBrep::AssertValid` |
//! | [`check_brep`], [`check_brep_with_keys`], [`BrepCheck`] | the capi `smlib_brep_check` sequence (`ValidatePointers`, then `AssertValid`) |
//! | [`CheckLevel`] | `SmAssertTestLevel` (`SM_LEVEL_0`..`SM_LEVEL_2`) |
//! | `brep::sub_topology` (private) | `SmBrep::AssertSubTopology`, the `SmBrep` branch of `SmTopologyTraverser::GetSubTopology` |
//! | `brep::tolerances` (private) | `SmBrep::ValidateAndUpdateTolerances(TRUE, ...)` |
//! | `brep::collect_regions`, `brep::adjacent_regions` (private) | `SmTopologyTraverser::CollectRegions`, `SmRegion::GetAdjacentRegions` |
//! | `base::topology`, `base::owning` (private) | `SmTopology::AssertValid`, `SmOwningTopology::AssertValid`, `SmSAGObject::AssertValid` |
//! | `shell::region`, `shell::shell` (private) | `SmRegion::AssertValid`, `SmShell::AssertValid`, `sm_GetShellVertices`, `sm_GetShellFaceuses`, `SmEdgeuse::GetConnectedEdgeuses` |
//! | `face::face`, `face::faceuse` (private) | `SmFace::AssertValid` (level 0 and the level-2 tests whose callees exist), `SmFaceuse::AssertValid`, `sm_AssertTestFace39` |
//! | `loops::loop_`, `loops::loopuse` (private) | `SmLoop::AssertValid`, `SmLoopuse::AssertValid` |
//! | `edge::edge`, `edge::edgeuse`, `edge::curve` (private) | `SmEdge::AssertValid`, `sm_AssertTestEdge14`, `SmEdgeuse::AssertValid`, the virtual `SmCurve::AssertValid` |
//! | `vertex::vertex`, `vertex::vertexuse` (private) | `SmVertex::AssertValid`, `SmVertexuse::AssertValid` |
//! | `seams` (private) | the callees other packages port (see that module) |
//!
//! Deviations: object pointers are arena identities; the report helpers implement the
//! release `SM_ASSERT_*_REPORT` macros (a test runs when the level is at least its
//! own, its predicate and value are evaluated lazily, and a passed test clears
//! `GetLastAdd`). `SM_LEVEL_GIVEN` (4) runs every test and skips the base classes, as
//! upstream; release builds ignore `pTestRequests`. Context identity, mark bounds and
//! the doubly linked list rings are intrinsic to the arenas, so those tests always pass
//! (they still run, in native order); list membership and owner back links are checked
//! against indices built once per call. The walks are bounded loops (no recursion);
//! the radial edge-use walks stop with [`SolidError::InvalidInput`] after visiting more
//! uses than the edge has (upstream loops forever). Calls into checks not yet ported go
//! through `seams` and return [`SolidError::Unported`]. Warning text (`WARN`,
//! `SE_MSG`, `smos_WriteBuffer`) and debug drawing are omitted. `CalculateBoundingBox`
//! in the validate-only tolerance pass is not called (its size is used only when
//! fixing). Statuses upstream ignores are ignored here too.

mod base;
mod brep;
mod edge;
mod face;
mod loops;
mod seams;
mod shell;
mod vertex;

use std::borrow::Cow;
use std::collections::BTreeMap;

use super::{object_info, validate_pointers, Object, SUCCESS};
use crate::error::{EntityRef, Result};
use crate::geom::assert_array::{
    obj_type_string, AssertArray, AssertReport, ObjectKey, ReportSpec,
};
use crate::geom::object::ObjectRef;
use crate::kernel::Kernel;
use crate::math::UNDEF_DOUBLE;
use crate::topology::{Brep, BrepId, NamingScheme};

/// `SM_LEVEL_GIVEN`: every test, no base classes.
const LEVEL_GIVEN: u8 = 4;

/// The test level of a check (`SmAssertTestLevel`): `Level0` runs the fewest tests,
/// `Level2` all of them. source's "Basic" check is `Level0`, "Full" is `Level2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckLevel {
    /// `SM_LEVEL_0`.
    Level0 = 0,
    /// `SM_LEVEL_1`: adds the geometry checks.
    Level1 = 1,
    /// `SM_LEVEL_2`: every test.
    Level2 = 2,
}

impl CheckLevel {
    /// The upstream enumerator value (0, 1 or 2).
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// The result of [`check_brep`]: the pointer check, then (when it found nothing) the
/// whole-body `AssertValid`, as the capi `smlib_brep_check` runs them.
#[derive(Debug, Clone)]
pub struct BrepCheck {
    /// `ValidatePointers`' native status (1000 success, 1001 a required pointer
    /// failed, 0 an early boolean return).
    pub pointers_status: i32,
    /// The pointer check's reports, in native order.
    pub pointer_reports: AssertArray,
    /// `AssertValid`'s return; `None` when it was skipped because the pointer check
    /// reported failures.
    pub assert_valid: Option<bool>,
    /// `AssertValid`'s reports, in native order.
    pub reports: AssertArray,
}

impl BrepCheck {
    /// The failed reports (`!m_bOK`): pointer reports first, then `AssertValid`'s.
    pub fn failures(&self) -> impl Iterator<Item = &AssertReport> {
        self.pointer_reports
            .reports()
            .iter()
            .chain(self.reports.reports())
            .filter(|r| !r.ok)
    }

    /// The number of report lines ([`BrepCheck::lines`]), including the synthetic
    /// lines for a failed status without a report.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.lines().len()
    }

    /// `true` when the check produced no failure line.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.failure_count() == 0
    }

    /// One line per failure, exactly as the capi formats them:
    /// `<reporting type>::Test #<n> <name>: <message> [owner <owner type>]`, plus
    /// `SmBrep::ValidatePointers failed with status N` when the pointer check failed
    /// without a report, `AssertValid skipped: ValidatePointers failed` when it was
    /// skipped, and `SmBrep::AssertValid failed without a report`.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let pointer_failures = self.pointer_reports.reports().iter().any(|r| !r.ok);
        if self.pointers_status != SUCCESS && self.pointer_reports.is_empty() {
            out.push(format!(
                "SmBrep::ValidatePointers failed with status {}",
                self.pointers_status
            ));
        }
        out.extend(
            self.pointer_reports
                .reports()
                .iter()
                .filter(|r| !r.ok)
                .map(report_line),
        );
        if pointer_failures {
            out.push("AssertValid skipped: ValidatePointers failed".to_string());
            return out;
        }
        if self.assert_valid == Some(false) && self.reports.is_empty() {
            out.push("SmBrep::AssertValid failed without a report".to_string());
        }
        out.extend(
            self.reports
                .reports()
                .iter()
                .filter(|r| !r.ok)
                .map(report_line),
        );
        out
    }
}

/// The capi line of one report.
fn report_line(r: &AssertReport) -> String {
    format!(
        "{}::Test #{} {}: {} [owner {}]",
        obj_type_string(r.reporting_type),
        r.test_index,
        r.name,
        r.format_log_message(),
        r.owner_type_string.unwrap_or("NULL"),
    )
}

/// The arena identities of the crate's imported B-reps
/// ([`crate::topology::brep_cache::arena_identity`]).
fn arena(e: EntityRef) -> ObjectRef {
    use crate::topology::brep_cache::{arena_identity, CacheObject};
    match e {
        EntityRef::Edge(x) => arena_identity(CacheObject::Edge(x)),
        EntityRef::Face(x) => arena_identity(CacheObject::Face(x)),
        EntityRef::Vertex(x) => arena_identity(CacheObject::Vertex(x)),
        EntityRef::Brep(_) => arena_identity(CacheObject::Brep),
        other => crate::topology::loopuse::notify_ref(other),
    }
}

/// The caller keys a [`NamingScheme::Caller`] B-rep recorded for every edge, face and
/// edge use (`Brep::geometry_owner_keys`, the only entities the check names as
/// geometry owners); `None` when one of them has none.
fn recorded_caller_keys(b: &Brep) -> Option<BTreeMap<EntityRef, ObjectRef>> {
    let entities = b
        .edges()
        .ids()
        .map(EntityRef::Edge)
        .chain(b.faces().ids().map(EntityRef::Face))
        .chain(b.edgeuses().ids().map(EntityRef::Edgeuse));
    let mut keys = BTreeMap::new();
    for e in entities {
        keys.insert(e, b.geometry_owner_key(e)?);
    }
    Some(keys)
}

/// [`check_brep_with_keys`] with the geometry-owner identities of `b`'s naming scheme:
/// the Euler operators' notification identities for a [`NamingScheme::Notify`]
/// B-rep, the caller keys recorded where a [`NamingScheme::Caller`] B-rep was built
/// (an imported body, [`crate::topology::brep::p3::api_brep_copy`]), else the arena
/// identities ([`crate::topology::brep_cache::arena_identity`]).
///
/// # Errors
///
/// [`crate::SolidError::InvalidInput`] for a [`NamingScheme::Caller`] B-rep without a
/// recorded key for each edge, face and edge use (pass the maker's keys to
/// [`check_brep_with_keys`]); those of [`check_brep_with_keys`].
pub fn check_brep(k: &mut Kernel, b: &mut Brep, level: CheckLevel) -> Result<BrepCheck> {
    match b.naming() {
        NamingScheme::Notify => {
            check_brep_with_keys(k, b, level, &crate::topology::loopuse::notify_ref)
        }
        NamingScheme::Caller => {
            let recorded = recorded_caller_keys(b).ok_or(crate::SolidError::invalid_input(
                "topology::brep::validation::check_brep",
                "body names geometry owners by its maker's keys and does not record them \
                 all; use check_brep_with_keys",
            ))?;
            let keys = |e: EntityRef| recorded.get(&e).copied().unwrap_or_else(|| arena(e));
            check_brep_with_keys(k, b, level, &keys)
        }
        _ => check_brep_with_keys(k, b, level, &arena),
    }
}

/// The capi `smlib_brep_check(brep, level)`: [`validate_pointers`] at `level` (its
/// default `SM_NO_WALK`) into its own report list; when it reports no failure, [`assert_valid`] at `level` with the
/// walk. `keys` gives the identities the geometry owners use (as for
/// [`validate_pointers`]).
///
/// # Errors
///
/// Those of [`validate_pointers`] and [`assert_valid`], including
/// [`crate::SolidError::Unported`] from a check not yet ported.
pub fn check_brep_with_keys(
    k: &mut Kernel,
    b: &mut Brep,
    level: CheckLevel,
    keys: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<BrepCheck> {
    let mut pointer_reports = AssertArray::new();
    let pointers_status =
        validate_pointers(k, b, Some(&mut pointer_reports), level.code(), false, keys)?;
    let mut reports = AssertArray::new();
    let assert_valid = if pointer_reports.reports().iter().any(|r| !r.ok) {
        None
    } else {
        Some(assert_valid(
            k,
            b,
            Some(&mut reports),
            level.code(),
            true,
            keys,
        )?)
    };
    Ok(BrepCheck {
        pointers_status,
        pointer_reports,
        assert_valid,
        reports,
    })
}

/// `SmBrep::AssertValid(pAList, eTestLevel, eWalkTree)`: the B-rep checks (base
/// classes, `ValidatePointers` at level 0, the validate-only tolerance pass, Brep
/// tests 0-21, `ContainsCoincidentTopology`) and, with `walk`, `AssertValid` of every
/// region, shell, face, loop, edge and vertex. Failed checks are appended to `reports`
/// in native order. `level` is `SM_LEVEL_0`..`SM_LEVEL_GIVEN` (0..4). `keys` gives the
/// geometry owners' identities (as for [`validate_pointers`]). Returns `false` when a
/// check failed (at once when `ValidatePointers` does not succeed).
///
/// # Errors
///
/// [`crate::SolidError::InvalidInput`] for a level above 4 or broken topology links
/// upstream would dereference; [`crate::SolidError::Unported`] from a check not yet
/// ported; those of the geometric callees.
pub fn assert_valid(
    k: &mut Kernel,
    b: &mut Brep,
    reports: Option<&mut AssertArray>,
    level: u8,
    walk: bool,
    keys: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<bool> {
    if level > LEVEL_GIVEN {
        return Err(super::invalid("invalid assert test level"));
    }
    let index = base::Index::new(b);
    let mut cx = Cx {
        rep: Rep {
            array: reports,
            level,
        },
        keys,
        index,
        edge_sets: edge::EdgeSets::default(),
    };
    brep::brep(k, b, &mut cx, walk)
}

/// The state of one `AssertValid` run: the reports, the requested level, the geometry
/// owner identities and the list membership index.
struct Cx<'a> {
    rep: Rep<'a>,
    keys: &'a dyn Fn(EntityRef) -> ObjectRef,
    index: base::Index,
    edge_sets: edge::EdgeSets,
}

impl Cx<'_> {
    /// Whether the base-class checks run (`eTestLevel != SM_LEVEL_GIVEN`).
    const fn bases(&self) -> bool {
        self.rep.level != LEVEL_GIVEN
    }
}

/// The reports of a run and its level (`pAList`, `eTestLevel`).
struct Rep<'a> {
    array: Option<&'a mut AssertArray>,
    level: u8,
}

/// The key of a curve or surface object (see the `seams` key note): the shim's
/// identity space and index.
fn geometry_key(space: u64, index: u32) -> ObjectKey {
    ObjectKey((space << 32) | u64::from(index))
}

/// One report of the release macros: who runs it, about which object, and its
/// tolerance, value and optional message.
struct Test<'m> {
    class: i64,
    list: u64,
    index: u64,
    reporter: Object,
    owner: Object,
    owner_type: i64,
    tol: f64,
    message: Option<Cow<'m, str>>,
}

impl Rep<'_> {
    /// `SM_ASSERT_TEST_NEED`: the test of level `test_level` runs
    /// (`!(eTestLevel < TestLevel)`).
    const fn need(&self, test_level: u8) -> bool {
        self.level >= test_level
    }

    /// `SM_ASSERT_TEST_NEED(.., test_level)` (no test requests): the level is
    /// `SM_LEVEL_0`..`SM_LEVEL_2` and at least `test_level`.
    const fn test_need(&self, test_level: u8) -> bool {
        self.level <= 2 && self.level >= test_level
    }

    /// `SM_ASSERT_BOOLEAN_REPORT(index, ..)` in a method of class `class` run by
    /// `this`: `ok` is the already level-gated predicate.
    fn this_bool(
        &mut self,
        b: &Brep,
        class: i64,
        this: impl Into<Object>,
        index: u64,
        ok: bool,
    ) -> Result<bool> {
        let this = this.into();
        self.add(
            b,
            ok,
            Test {
                class,
                list: 0,
                index,
                reporter: this,
                owner: this,
                owner_type: class,
                tol: UNDEF_DOUBLE,
                message: None,
            },
            || UNDEF_DOUBLE,
        )
    }

    /// `SM_ASSERT_VALUE_REPORT(index, .., tol, value, ..)` in a method of class
    /// `class` run by `this`; `value` is evaluated only when the test fails.
    fn this_value(
        &mut self,
        b: &Brep,
        class: i64,
        this: impl Into<Object>,
        index: u64,
        ok: bool,
        tol: f64,
        value: impl FnOnce() -> f64,
    ) -> Result<bool> {
        let this = this.into();
        self.add(
            b,
            ok,
            Test {
                class,
                list: 0,
                index,
                reporter: this,
                owner: this,
                owner_type: class,
                tol,
                message: None,
            },
            value,
        )
    }

    /// The release `SM_ASSERT_OBJ_*_REPORT` body: on a pass clears `GetLastAdd`, on a
    /// failure adds the report (when there is a list) and returns `false`.
    fn add(
        &mut self,
        b: &Brep,
        ok: bool,
        test: Test<'_>,
        value: impl FnOnce() -> f64,
    ) -> Result<bool> {
        let Some(array) = self.array.as_deref_mut() else {
            return Ok(ok);
        };
        if ok {
            array.set_last_add(false);
            return Ok(true);
        }
        let (reporter, _, _) = object_info(b, test.reporter)?;
        let (owner, _, owner_name) = object_info(b, test.owner)?;
        array.add_unique_delete_duplicate(AssertReport::new(ReportSpec {
            reporter: Some(reporter),
            test_index: test.index,
            test_class: test.class,
            owner: Some(owner),
            owner_type: test.owner_type,
            owner_type_string: Some(owner_name),
            other: None,
            other_type: 0,
            other_type_string: Some("NULL"),
            tol3d: crate::geom::tol::tol3d(test.tol),
            value: value(),
            list_index: test.list,
            message: test.message.map(|m| Cow::Owned(m.into_owned())),
        })?);
        Ok(false)
    }
}

/// The B-rep object (`this` of `SmBrep::AssertValid`).
const fn brep_object() -> Object {
    Object::Topology(EntityRef::Brep(BrepId::from_index(0)))
}

/// The class codes of the methods running the tests (`GetClassType()`).
mod class {
    pub(super) use crate::geom::assert_array::labels::{
        BREP_TYPE as BREP, EDGEUSE_TYPE as EDGEUSE, EDGE_TYPE as EDGE, FACEUSE_TYPE as FACEUSE,
        FACE_TYPE as FACE, LOOPUSE_TYPE as LOOPUSE, LOOP_TYPE as LOOP,
        OWNING_TOPOLOGY_TYPE as OWNING_TOPOLOGY, SHELL_TYPE as SHELL, TOPOLOGY_TYPE as TOPOLOGY,
        VERTEXUSE_TYPE as VERTEXUSE, VERTEX_TYPE as VERTEX,
    };
}
