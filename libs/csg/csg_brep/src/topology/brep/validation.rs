//! B-rep pointer validation and ordered coincidence checks.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`,
//! `source/SMLib/src/SmEdge.cpp`, `source/SMLib/src/NL_CrvBasic.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`validate_pointers`] | `SmBrep::ValidatePointers` |
//! | [`contains_coincident_topology`] | `SmBrep::ContainsCoincidentTopology` |
//! | `check_edge_geometry` | `SmEdge::ValidateGeometry(FALSE)` |
//! | `check_edgeuse_geometry` | `sm_CheckEdgeuseGeometry` |
//!
//! Deviations: object pointers are arena identities; out-parameters are reports and
//! returned pairs. Context identity and the three owning list-head objects are
//! intrinsic to a Rust B-rep. Invalid arena links return errors instead of reading
//! freed memory. The native 100000-step loop guard is retained. There is no
//! input-driven recursion. Local indices cache list membership and spatial
//! candidates; results retain native traversal order. Release builds ignore test
//! requests and run tests whose level is at most the requested level, as upstream.

mod assert_valid;
mod coincidence;
mod pointers;
pub(crate) mod spatial;

pub use assert_valid::{assert_valid, check_brep, check_brep_with_keys, BrepCheck, CheckLevel};
pub use coincidence::{contains_coincident_topology, CoincidenceResult};
pub use pointers::validate_pointers;

use crate::error::{EntityRef, Result, SolidError};
use crate::geom::assert_array::{AssertArray, AssertReport, ObjectKey, ReportSpec};
use crate::math::UNDEF_DOUBLE;
use crate::topology::{Brep, BrepId, EdgeuseId, FaceId};

const OP: &str = "topology::brep::validation";
const BREP_TYPE: i64 = 16011;
const SUCCESS: i32 = 1000;
const ERROR: i32 = 1001;

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| invalid("missing topology link"))
}

#[derive(Clone, Copy)]
enum Object {
    Topology(EntityRef),
    Surface(FaceId),
    Trim(EdgeuseId),
}

impl From<EntityRef> for Object {
    fn from(entity: EntityRef) -> Self {
        Self::Topology(entity)
    }
}

fn topology_type(entity: EntityRef) -> (i64, &'static str, u32) {
    match entity {
        EntityRef::Brep(x) => (BREP_TYPE, "SmBrep", x.index()),
        EntityRef::Region(x) => (16012, "SmRegion", x.index()),
        EntityRef::Shell(x) => (16013, "SmShell", x.index()),
        EntityRef::Face(x) => (16002, "SmFace", x.index()),
        EntityRef::Faceuse(x) => (16022, "SmFaceuse", x.index()),
        EntityRef::Loop(x) => (16003, "SmLoop", x.index()),
        EntityRef::Loopuse(x) => (16004, "SmLoopuse", x.index()),
        EntityRef::Edge(x) => (16016, "SmEdge", x.index()),
        EntityRef::Edgeuse(x) => (16005, "SmEdgeuse", x.index()),
        EntityRef::Vertex(x) => (16021, "SmVertex", x.index()),
        EntityRef::Vertexuse(x) => (16006, "SmVertexuse", x.index()),
    }
}

fn object_info(b: &Brep, object: Object) -> Result<(ObjectKey, i64, &'static str)> {
    let (space, ty, name, index) = match object {
        Object::Topology(e) => {
            let (ty, name, index) = topology_type(e);
            (ty as u64, ty, name, index)
        }
        Object::Surface(f) => {
            let s = required(b.face(f)?.surface.as_ref())?;
            (20002, 15001, s.class_string(), f.index())
        }
        Object::Trim(eu) => {
            // GetUVTrimCurve may resolve to the mate's stored curve.
            let (_, owner) = crate::topology::edgeuse::p1::uv_trim_curve(b, eu)?;
            let owner = required(owner)?;
            let c = required(b.edgeuse(owner)?.uv_trim_curve.as_ref())?;
            (20003, 14033, c.class_string(), owner.index())
        }
    };
    Ok((ObjectKey((space << 32) | u64::from(index)), ty, name))
}

struct Reports<'a> {
    array: Option<&'a mut AssertArray>,
    level: u8,
}

impl Reports<'_> {
    fn check(
        &mut self,
        b: &Brep,
        index: u64,
        passed: bool,
        owner: impl Into<Object>,
    ) -> Result<bool> {
        self.record(
            b,
            1,
            index,
            passed,
            owner.into(),
            None,
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )
    }

    fn value(
        &mut self,
        b: &Brep,
        index: u64,
        passed: bool,
        owner: impl Into<Object>,
        tol: f64,
        value: f64,
    ) -> Result<bool> {
        self.record(b, 1, index, passed, owner.into(), None, tol, value)
    }

    fn pair(
        &mut self,
        b: &Brep,
        index: u64,
        passed: bool,
        owner: EntityRef,
        other: EntityRef,
        tol: f64,
        value: f64,
    ) -> Result<bool> {
        self.record(
            b,
            3,
            index,
            self.level < 2 || passed,
            owner.into(),
            Some(other.into()),
            tol,
            value,
        )
    }

    fn record(
        &mut self,
        b: &Brep,
        list: u64,
        index: u64,
        passed: bool,
        owner: Object,
        other: Option<Object>,
        tol: f64,
        value: f64,
    ) -> Result<bool> {
        let Some(array) = self.array.as_deref_mut() else {
            return Ok(passed);
        };
        if passed {
            array.set_last_add(false);
            return Ok(true);
        }
        let (owner, owner_type, owner_name) = object_info(b, owner)?;
        let (other, other_type, other_name) = match other {
            Some(o) => {
                let (key, ty, name) = object_info(b, o)?;
                (Some(key), ty, name)
            }
            None => (None, 0, "NULL"),
        };
        array.add_unique_delete_duplicate(AssertReport::new(ReportSpec {
            reporter: Some(
                object_info(b, Object::Topology(EntityRef::Brep(BrepId::from_index(0))))?.0,
            ),
            test_index: index,
            test_class: BREP_TYPE,
            owner: Some(owner),
            owner_type,
            owner_type_string: Some(owner_name),
            other,
            other_type,
            other_type_string: Some(other_name),
            tol3d: crate::geom::tol::tol3d(tol),
            value,
            list_index: list,
            message: None,
        })?);
        Ok(false)
    }
}
