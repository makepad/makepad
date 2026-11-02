//! Solver results: a solution end (one point of a solution space), a solution
//! (single value or range of values over a set of objects) and the sorted,
//! de-duplicating solution array the global solvers fill.
//!
//! Upstream: `source/SMLib/inc/SmSolutionArray.h`, `source/SMLib/src/SmSolutionArray.cpp`.
//!
//! | Rust                                          | Upstream                                                   |
//! |-----------------------------------------------|------------------------------------------------------------|
//! | [`MAX_TREES`], [`MAX_PARAMS`]                 | `SM_SA_MAX_TREES`, `SM_SA_MAX_TREES*2`                     |
//! | [`SolutionType`]                              | `SmSolutionType`                                           |
//! | [`SortKey`]                                   | `SmSortKeyType`                                            |
//! | [`SolutionEnd`], [`SolutionEnd::new`]         | `SmSolutionEnd`, `SmSolutionEnd()`                         |
//! | `Clone`/`Copy` for [`SolutionEnd`]            | `SmSolutionEnd::operator=`                                 |
//! | `Drop` (implicit)                             | `~SmSolutionEnd`                                           |
//! | [`SolutionEnd::reset`]                        | `SmSolutionEnd::ReSet`                                     |
//! | [`SolutionEnd::param`], [`SolutionEnd::param_mut`] | `SmSolutionEnd::operator[]` (const and non-const)     |
//! | [`Solution`], [`Solution::new`]               | `SmSolution`, `SmSolution()`                               |
//! | [`Solution::zeroed`]                          | a `smos_Calloc`/`smos_MemSet` zeroed `SmSolution` slot     |
//! | `Clone`/`Copy` for [`Solution`]               | `SmSolution(const SmSolution&)`, `SmSolution::operator=`   |
//! | `Drop` (implicit)                             | `~SmSolution`                                              |
//! | [`Solution::reset`]                           | `SmSolution::ReSet`                                        |
//! | [`Solution::classify_solution_end`], [`EndClass`] | `SmSolution::ClassifySolutionEnd` and its return values |
//! | [`Solution::index_of`]                        | `SmSolution::GetIndex`                                     |
//! | [`Solution::point`]                           | `SmSolution::GetPoint(ULONG, ...)`                         |
//! | [`Solution::point_of`]                        | `SmSolution::GetPoint(const SmObject*, ...)`               |
//! | [`Solution::object_index_for_parameter_index`] | `SmSolution::GetObjectIndexForParameterIndex`             |
//! | [`Solution::parameter_interval`]              | `SmSolution::GetParameterInterval`                         |
//! | [`Solution::parameter_interval_in`], [`ParameterDomains`] | `SmSolution::GetParameterInterval` (object domains read through a caller's resolver) |
//! | [`Solution::object_tolerance`]                | `SmSolution::GetObjectTolerance`                           |
//! | [`Solution::object_tolerance_with`]           | `SmSolution::GetObjectTolerance` (caller-resolved handle)   |
//! | [`Solution::is_equal`], [`Solution::is_equal_in`] | `SmSolution::IsEqual`                                  |
//! | `are_sequential` (private)                    | `sm_AreSequential`                                         |
//! | [`SolutionObject`], [`ObjectKind`]            | the `SmObject*` of `m_apObjects` and its `IsKindOf` tests  |
//! | [`SolutionArray`], [`SolutionArray::new`]     | `SmSolutionArray`, `SmSolutionArray()`                     |
//! | [`SolutionArray::with_capacity`], [`SolutionArray::try_with_capacity`] | `SmSolutionArray(ULONG, SmSolution* = NULL)`               |
//! | `Drop` (implicit)                             | `~SmSolutionArray`                                         |
//! | [`SolutionArray::len`]                        | `SmSolutionArray::GetSize`                                 |
//! | [`SolutionArray::set_size`], [`SolutionArray::try_set_size`] | `SmSolutionArray::SetSize`                                 |
//! | [`SolutionArray::reset`]                      | `SmSolutionArray::ReSet`                                   |
//! | [`SolutionArray::as_slice`]                   | `SmSolutionArray::GetDataArray`                            |
//! | [`SolutionArray::get`], [`SolutionArray::get_mut`] | `SmSolutionArray::operator[]`                         |
//! | [`SolutionArray::get_at`], [`SolutionArray::set_at`] | `SmSolutionArray::GetAt`, `SetAt`                   |
//! | [`SolutionArray::add`]                        | `SmSolutionArray::Add`                                     |
//! | `set_at_grow` (private)                       | `SmSolutionArray::SetAtGrow`                               |
//! | [`SolutionArray::append`]                     | `SmSolutionArray::Append`                                  |
//! | [`SolutionArray::add_sorted_solution`], [`SolutionArray::add_sorted_solution_in`] | `SmSolutionArray::AddSortedSolution` |
//! | [`SolutionArray::insert_at`], [`SolutionArray::try_insert_at`] | `SmSolutionArray::InsertAt(ULONG, const SmSolution&, ULONG)` |
//! | [`SolutionArray::remove_at`]                  | `SmSolutionArray::RemoveAt`                                |
//! | `OperandObject`, `SolutionArray::without_objects` (crate) | the non-null `this` / operand objects of upstream's solves |
//! | [`SolutionArray::swap`]                       | `SmSolutionArray::Swap`                                    |
//! | [`SolutionArray::TYPE`], [`SolutionArray::type_code`], [`SolutionArray::type_string`], [`SolutionArray::is_kind_of`] | `SmSolutionArray::GetType`, `GetTypeString`, `IsKindOf` |
//!
//! Deviations:
//! - **Objects.** A solution's objects are a caller-chosen handle `O`
//!   implementing [`SolutionObject`] (upstream `SmObject*`; a null pointer is
//!   `None`, pointer equality is `O: PartialEq`). The handle reports its
//!   [`ObjectKind`], which answers the `IsKindOf` tests made here, and, when it
//!   can reach its object, the geometry read here (natural intervals and domains,
//!   points, an edge's curve and interval, a face's surface and UV domain: the
//!   [`SolutionObject`] hooks). A caller holding the geometry behind opaque handles
//!   reads parameter domains through its own [`ParameterDomains`] instead
//!   ([`Solution::parameter_interval_in`], [`Solution::is_equal_in`],
//!   [`SolutionArray::add_sorted_solution_in`]). Tree nodes (`m_apNodes`) are opaque
//!   [`NodeId`]s.
//!   Solves whose caller gives no handle name `OperandObject` stand-ins internally
//!   (upstream's solutions always name the solve's curves and surfaces, whose
//!   natural intervals and domains `IdenticalSolutions` / `IsEqual` read) and hand
//!   the solutions back without objects (`SolutionArray::without_objects`).
//! - **Unresolved handles.** The geometry of a handle that cannot reach its object
//!   (a hook answering `None`; also a failed curve or surface evaluation, whose
//!   status upstream ignores), the parameter domain of a volume and
//!   `SmTol::GetZoneTol3d(const SmObject*)` return [`SolidError::Unported`] (see the
//!   `UNPORTED` markers).
//! - **Status out-params.** `GetIndex`, `GetPoint`,
//!   `GetObjectIndexForParameterIndex` and `GetParameterInterval` return
//!   `Ok(None)` for upstream's `SM_ERR` (their out-params then hold only
//!   initialisation values) and `Ok(Some(..))` with the out-params on
//!   `SM_SUCCESS`. `GetPoint` always reports the parameter (upstream fills
//!   `pOptSolParam` only when it is non-null). `AddSortedSolution` returns
//!   `None` for a rejected solution (upstream `SM_BIG_ULONG`).
//! - **Undefined behaviour becomes an error.** Upstream reads past
//!   `m_adParameters` / `m_apObjects` when `m_lNumVariables > 10` or
//!   `m_lNumObjects > 5`, dereferences a null object in `GetIndex`, `GetPoint`
//!   and `GetParameterInterval` (whose object search can stop on a null slot),
//!   and reads or writes outside the array for out-of-range `GetAt`, `SetAt`,
//!   `Swap` and `operator[]` (its `SM_ASSERT`s are compiled out); here those
//!   return [`SolidError::InvalidInput`] (`None` for [`SolutionArray::get`]).
//! - **Storage.** The array is a `Vec`; slots past the size are never visible
//!   upstream (every growth path zeroes them), so the allocation size
//!   (`m_lMaxSize`) and borrowed arrays (`m_bIsBorrowed`) have no counterpart.
//!   [`SolutionArray::with_capacity`] only reserves. The `try_` variants
//!   ([`SolutionArray::try_set_size`], [`SolutionArray::try_insert_at`],
//!   [`SolutionArray::try_with_capacity`]) take caller-chosen sizes and return
//!   [`SolidError::InvalidInput`] when the slots cannot be allocated or would take
//!   more than [`crate::limits::MAX_ALLOC_BYTES`] (upstream writes through the
//!   `NULL` of a failed `smos_Calloc`). New slots made by growth
//!   are [`Solution::zeroed`] (type [`SolutionType::NodesOnly`], as calloc'd
//!   memory reads upstream).
//! - `SmSolution::Dump` and `SmSolutionArray::Dump` print diagnostics only and are
//!   not ported; the `SM_DEBUG_CODE`/`GWC` blocks and `ERR` on an unknown sort key
//!   (impossible with the [`SortKey`] enum) have no counterpart.

use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::object;
use crate::geom::tree::NodeId;
use crate::limits::try_reserve_total;
use crate::math::{max3, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};

/// `SM_SA_MAX_TREES`: most objects (and tree nodes) a solution refers to.
pub const MAX_TREES: usize = 5;

/// `SM_SA_MAX_TREES*2`: parameters stored per solution end.
pub const MAX_PARAMS: usize = MAX_TREES * 2;

/// `SmSolutionArray_TYPE` (`CONT_BASE_TYPE + 20`).
const SM_SOLUTION_ARRAY_TYPE: i64 = 12020;

/// The kind of solution a solver produced (`SmSolutionType`); the discriminants
/// are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum SolutionType {
    /// `SM_ST_NODES_ONLY`: only the tree nodes are meaningful (also the type of a
    /// zero-filled slot).
    NodesOnly = 0,
    /// `SM_ST_SINGLE_VALUE`: a single point; only `start` holds values.
    SingleValue = 1,
    /// `SM_ST_RANGE_OF_VALUES`: solutions over the range `start` .. `end`.
    RangeOfValues = 2,
    /// `SM_ST_POLY_SINGLE_VALUE`: a poly solver point; `start` holds positions.
    PolySingleValue = 3,
    /// `SM_ST_UNKNOWN`: uninitialised.
    #[default]
    Unknown = 4,
}

impl SolutionType {
    /// Every type, in upstream order.
    pub const ALL: [Self; 5] = [
        Self::NodesOnly,
        Self::SingleValue,
        Self::RangeOfValues,
        Self::PolySingleValue,
        Self::Unknown,
    ];

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.code() == code)
    }
}

/// Sort criterion of [`SolutionArray::add_sorted_solution`] (`SmSortKeyType`);
/// the discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum SortKey {
    /// `SM_SK_BY_SOLUTION_VALUE`: by solution value only.
    BySolutionValue = 0,
    /// `SM_SK_BY_FIRST_PARAMETER`: by the first parameter only.
    ByFirstParameter = 1,
    /// `SM_SK_BY_FIRST_TWO_PARAMETERS`: by the first parameter, ties resolved by
    /// the second.
    ByFirstTwoParameters = 2,
    /// `SM_SK_BY_VALUE_WITH_PARAMETER`: by value, ties resolved by the parameters.
    ByValueWithParameter = 3,
    /// `SM_SK_BY_ALL_PARAMETERS`: by the first parameter, then the next ones up to
    /// the first that differs.
    ByAllParameters = 4,
}

impl SortKey {
    /// Every key, in upstream order.
    pub const ALL: [Self; 5] = [
        Self::BySolutionValue,
        Self::ByFirstParameter,
        Self::ByFirstTwoParameters,
        Self::ByValueWithParameter,
        Self::ByAllParameters,
    ];

    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The key with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }
}

/// Where a solution end lies relative to a solution
/// ([`Solution::classify_solution_end`]); the discriminants are the upstream
/// return values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum EndClass {
    /// `-2`: below the start of the range (or, for a single value, the first
    /// differing parameter is smaller).
    BelowStart = -2,
    /// `-1`: matches the start of the range.
    AtStart = -1,
    /// `0`: matches the single value, or lies inside the range.
    Within = 0,
    /// `1`: matches the end of the range.
    AtEnd = 1,
    /// `2`: above the end of the range (or, for a single value, the first
    /// differing parameter is larger).
    AboveEnd = 2,
    /// `3`: no consistent classification (different variable or object counts
    /// or objects, unequal solution values at a matching point, or inconsistent
    /// parameters).
    Inconsistent = 3,
}

impl EndClass {
    /// The upstream return value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// True for [`EndClass::AtStart`], [`EndClass::Within`] and
    /// [`EndClass::AtEnd`] (upstream `abs(class) <= 1`).
    #[must_use]
    pub const fn is_at_or_within(self) -> bool {
        matches!(self, Self::AtStart | Self::Within | Self::AtEnd)
    }
}

/// What a solution object is, as far as the `IsKindOf` tests of this module go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    /// An `SmCurve` (any curve type).
    Curve,
    /// An `SmSurface` (any surface type).
    Surface,
    /// An `SmVolume`.
    Volume,
    /// An `SmVertex`.
    Vertex,
    /// An `SmEdge`.
    Edge,
    /// An `SmFace`.
    Face,
    /// An `SmPolyVertex`.
    PolyVertex,
    /// An `SmPolyEdge`.
    PolyEdge,
    /// An `SmPolyLoop`.
    PolyLoop,
    /// An `SmPolyFace` (not a composite one).
    PolyFace,
    /// An `SmCPolyFace` (a kind of `SmPolyFace`).
    CPolyFace,
    /// An `SmPolyShell`.
    PolyShell,
    /// An `SmPolyRegion`.
    PolyRegion,
    /// An `SmPolyBrep`.
    PolyBrep,
    /// Any other object.
    Other,
}

impl ObjectKind {
    /// `IsKindOf(<base>_TYPE)`: true when an object of this kind is a `base`.
    #[must_use]
    pub fn is_kind_of(self, base: Self) -> bool {
        self == base || (self == Self::CPolyFace && base == Self::PolyFace)
    }
}

/// A handle to an object a solution refers to (upstream `SmObject*`). Two
/// handles are the same object when they compare equal.
pub trait SolutionObject: Copy + PartialEq {
    /// The object's kind (answers upstream's `IsKindOf` tests).
    fn kind(&self) -> ObjectKind;

    /// For a curve object, its natural parameter interval
    /// (`SmCurve::GetNaturalInterval`) when the handle can reach the curve; `None`
    /// (the default) when it cannot.
    fn natural_interval(&self) -> Option<Extent1d> {
        None
    }

    /// For a surface object, its natural parameter domain
    /// (`SmSurface::GetNaturalUVDomain`) when the handle can reach the surface;
    /// `None` (the default) when it cannot.
    fn natural_uv_domain(&self) -> Option<crate::geom::extent2d::Extent2d> {
        None
    }

    /// For a poly vertex object, its point (`SmPolyVertex::GetPoint`, see
    /// [`crate::query::poly::PolyBrep::vertex_point`]) when the handle can reach the
    /// vertex; `None` (the default) when it cannot.
    fn poly_vertex_point(&self) -> Option<Point3> {
        None
    }

    /// For a vertex object, its point (`SmVertex::GetPoint`) when the handle can reach
    /// the vertex; `None` (the default) when it cannot.
    fn vertex_point(&self) -> Option<Point3> {
        None
    }

    /// For an edge object, the handle of its curve (`SmEdge::GetCurve`) when the
    /// handle can reach the edge and the edge has a curve; `None` (the default)
    /// otherwise.
    fn edge_curve(&self) -> Option<Self> {
        None
    }

    /// For an edge object, its parameter interval (`SmEdge::GetInterval`) when the
    /// handle can reach the edge; `None` (the default) when it cannot.
    fn edge_interval(&self) -> Option<Extent1d> {
        None
    }

    /// For a curve object, its point at `t` (`SmCurve::EvaluatePoint`) when the handle
    /// can reach the curve and the evaluation succeeds; `None` (the default)
    /// otherwise.
    fn curve_point(&self, t: f64) -> Option<Point3> {
        let _ = t;
        None
    }

    /// For a face object, the handle of its surface (`SmFace::GetSurface`) when the
    /// handle can reach the face and the face has a surface; `None` (the default)
    /// otherwise.
    fn face_surface(&self) -> Option<Self> {
        None
    }

    /// For a face object, its UV domain (`SmFace::GetUVDomain`) when the handle can
    /// reach the face; `None` (the default) when it cannot.
    fn face_uv_domain(&self) -> Option<crate::geom::extent2d::Extent2d> {
        None
    }

    /// For a surface object, its point at `(u, v)` (`SmSurface::EvaluatePoint`) when
    /// the handle can reach the surface and the evaluation succeeds; `None` (the
    /// default) otherwise.
    fn surface_point(&self, u: f64, v: f64) -> Option<Point3> {
        let _ = (u, v);
        None
    }
}

/// Where `GetParameterInterval` reads the parameter domain of a solution object
/// ([`Solution::parameter_interval_in`]). Upstream dereferences the `SmObject*`
/// (`SmEdge::GetInterval`, `SmCurve::GetNaturalInterval`, `SmFace::GetUVDomain`,
/// `SmSurface::GetNaturalUVDomain`); each method answers for one kind, `None` when
/// the handle cannot be resolved. The defaults ask the handle itself
/// ([`SolutionObject::edge_interval`], ...); a caller holding the geometry behind
/// opaque handles (a topology solver's objects) answers from that geometry instead.
pub trait ParameterDomains<O: SolutionObject> {
    /// `SmEdge::GetInterval` of edge handle `edge`.
    fn edge_interval(&self, edge: &O) -> Option<Extent1d> {
        edge.edge_interval()
    }
    /// `SmCurve::GetNaturalInterval` of curve handle `curve`.
    fn natural_interval(&self, curve: &O) -> Option<Extent1d> {
        curve.natural_interval()
    }
    /// `SmFace::GetUVDomain` of face handle `face`.
    fn face_uv_domain(&self, face: &O) -> Option<crate::geom::extent2d::Extent2d> {
        face.face_uv_domain()
    }
    /// `SmSurface::GetNaturalUVDomain` of surface handle `surface`.
    fn natural_uv_domain(&self, surface: &O) -> Option<crate::geom::extent2d::Extent2d> {
        surface.natural_uv_domain()
    }
}

/// [`ParameterDomains`] that asks each handle itself (the trait's defaults).
#[derive(Debug, Clone, Copy, Default)]
pub struct OwnDomains;

impl<O: SolutionObject> ParameterDomains<O> for OwnDomains {}

/// The solution object a solve names internally when its caller gives no handle
/// (`None`): upstream's solutions always name the solve's operands (`this` curve or
/// surface, the other curve, ...), and `IdenticalSolutions` / `IsEqual` merge
/// solutions within 1/10000th of their natural interval or domain, so the stand-in
/// reports the operand's natural interval ([`OperandObject::Curve`]) or natural UV
/// domain ([`OperandObject::Surface`]). The `u8` tells the operands apart. The
/// caller gets the solutions back without objects
/// ([`SolutionArray::without_objects`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OperandObject {
    /// A curve operand and its natural interval.
    Curve(u8, Extent1d),
    /// A surface operand and its natural UV domain.
    Surface(u8, crate::geom::extent2d::Extent2d),
}

impl SolutionObject for OperandObject {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve(..) => ObjectKind::Curve,
            Self::Surface(..) => ObjectKind::Surface,
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Curve(_, ivl) => Some(*ivl),
            Self::Surface(..) => None,
        }
    }

    fn natural_uv_domain(&self) -> Option<crate::geom::extent2d::Extent2d> {
        match self {
            Self::Curve(..) => None,
            Self::Surface(_, domain) => Some(*domain),
        }
    }
}

/// One point of a solution space (`SmSolutionEnd`): a solution value (typically
/// a distance) and the parameters of the objects at that point, concatenated in
/// object order (a curve takes one, a surface two, ...).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SolutionEnd {
    /// `m_dSolutionValue`.
    pub solution_value: f64,
    /// `m_adParameters`; unused entries are `0.0`.
    pub parameters: [f64; MAX_PARAMS],
}

impl SolutionEnd {
    /// All zero (`SmSolutionEnd()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            solution_value: 0.0,
            parameters: [0.0; MAX_PARAMS],
        }
    }

    /// Zeroes the value and every parameter (`ReSet`).
    pub fn reset(&mut self) {
        self.solution_value = 0.0;
        self.parameters = [0.0; MAX_PARAMS];
    }

    /// Parameter `index` (`operator[] const`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= MAX_PARAMS`.
    pub fn param(&self, index: usize) -> Result<f64> {
        self.parameters
            .get(index)
            .copied()
            .ok_or(bad_param("geom::solution_array::SolutionEnd::param"))
    }

    /// Mutable parameter `index` (`operator[]`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= MAX_PARAMS`.
    pub fn param_mut(&mut self, index: usize) -> Result<&mut f64> {
        self.parameters
            .get_mut(index)
            .ok_or(bad_param("geom::solution_array::SolutionEnd::param_mut"))
    }
}

fn bad_param(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "solution parameter index out of range",
    }
}

fn null_object(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "null solution object",
    }
}

/// `sm_AreSequential`: true when `a`, `b`, `c` are strictly ascending or strictly
/// descending.
fn are_sequential(a: f64, b: f64, c: f64) -> bool {
    (a < b && b < c) || (a > b && b > c)
}

/// The result of a solver (`SmSolution`): one point, or a range of points, in
/// the joint parameter space of up to [`MAX_TREES`] objects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Solution<O> {
    /// `m_eSolutionType`.
    pub solution_type: SolutionType,
    /// `m_lNumVariables`: parameters used in `start`/`end` (at most
    /// [`MAX_PARAMS`]).
    pub num_variables: usize,
    /// `m_vStart`: the single value, or the start of a range.
    pub start: SolutionEnd,
    /// `m_vEnd`: the end of a range.
    pub end: SolutionEnd,
    /// `m_lNumObjects`: objects used in `objects` (at most [`MAX_TREES`]).
    pub num_objects: usize,
    /// `m_apObjects`; `None` is a null pointer.
    pub objects: [Option<O>; MAX_TREES],
    /// `m_apNodes`: tree nodes the solution was found in; `None` is a null
    /// pointer.
    pub nodes: [Option<NodeId>; MAX_TREES],
}

impl<O: Copy> Default for Solution<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: Copy> Solution<O> {
    /// An empty solution of type [`SolutionType::Unknown`] (`SmSolution()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            solution_type: SolutionType::Unknown,
            num_variables: 0,
            start: SolutionEnd::new(),
            end: SolutionEnd::new(),
            num_objects: 0,
            objects: [None; MAX_TREES],
            nodes: [None; MAX_TREES],
        }
    }

    /// An all-zero solution, as a zero-filled array slot reads upstream (type
    /// [`SolutionType::NodesOnly`]).
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            solution_type: SolutionType::NodesOnly,
            ..Self::new()
        }
    }
}

impl<O: SolutionObject> Solution<O> {
    /// Clears the type, the counts, the objects and the nodes (`ReSet`). The
    /// solution ends keep their values, as upstream.
    pub fn reset(&mut self) {
        self.solution_type = SolutionType::Unknown;
        self.num_variables = 0;
        self.num_objects = 0;
        self.objects = [None; MAX_TREES];
        self.nodes = [None; MAX_TREES];
    }

    /// Checks the counts upstream would use to index its fixed arrays.
    fn check(&self, operation: &'static str) -> Result<()> {
        if self.num_variables > MAX_PARAMS || self.num_objects > MAX_TREES {
            return Err(SolidError::InvalidInput {
                operation,
                reason: "solution variable or object count exceeds the fixed arrays",
            });
        }
        Ok(())
    }

    /// The first `num_objects` objects.
    fn used_objects(&self) -> &[Option<O>] {
        self.objects
            .get(..self.num_objects)
            .unwrap_or(&self.objects)
    }

    /// Object slot `index` (`m_apObjects[index]`).
    fn object(&self, index: usize, operation: &'static str) -> Result<Option<O>> {
        self.objects
            .get(index)
            .copied()
            .ok_or(SolidError::InvalidInput {
                operation,
                reason: "solution object index out of range",
            })
    }

    /// Classifies `other`'s start (`compare_start`, or when `other` is a single
    /// value) or end against this solution (`ClassifySolutionEnd`); see
    /// [`EndClass`].
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays.
    pub fn classify_solution_end(&self, other: &Self, compare_start: bool) -> Result<EndClass> {
        const OP: &str = "geom::solution_array::Solution::classify_solution_end";
        self.check(OP)?;
        other.check(OP)?;

        // can't compare different variable or object counts
        if self.num_variables != other.num_variables || self.num_objects != other.num_objects {
            return Ok(EndClass::Inconsistent);
        }

        // can't compare different solution objects
        for (a, b) in self.used_objects().iter().zip(other.used_objects()) {
            if a != b {
                return Ok(EndClass::Inconsistent);
            }
        }

        // Get OtherSolution target end
        let e = if compare_start || other.solution_type == SolutionType::SingleValue {
            &other.start
        } else {
            &other.end
        };

        // When thisSolution is a point
        if self.solution_type == SolutionType::SingleValue {
            let mut ret = EndClass::Inconsistent;

            // can't compare unequal solutionValues
            if EFF_ZERO_SQRT < (self.start.solution_value - e.solution_value).abs() {
                return Ok(ret);
            }

            // for every variable
            for i in 0..self.num_variables {
                let s = self.start.param(i)?;
                let o = e.param(i)?;
                let scaled_zero = EFF_ZERO * (1.0 + s.abs());

                // when parameters equal - continue to next param after setting classification as needed
                if (s - o).abs() < scaled_zero {
                    if ret != EndClass::Inconsistent && ret != EndClass::Within {
                        continue;
                    }
                    ret = EndClass::Within; // Match values
                    continue;
                }

                // else parameters are different

                // had a match and lost it
                if ret == EndClass::Within {
                    return Ok(EndClass::Inconsistent);
                }

                // remember if this is lower or higher
                if e.param(0)? < self.start.param(0)? {
                    ret = EndClass::BelowStart; // Less than
                } else {
                    ret = EndClass::AboveEnd; // Greater than
                }
                break;
            }
            return Ok(ret);
        }

        // else this Solution is an interval solution
        let mut ret = EndClass::Inconsistent;

        // for every Variable
        for i in 0..self.num_variables {
            let s = self.start.param(i)?;
            let t = self.end.param(i)?;
            let o = e.param(i)?;
            let scaled_zero = (1.0 + o.abs()).abs() * EFF_ZERO;

            // thisSolutionVariable Param range is degenerate
            if (s - o).abs() < scaled_zero && (t - o).abs() < scaled_zero {
                // Ambiguous situation just continue to next one
                continue;
            }

            // ThisSolution->Start = OtherSolution->End
            if (s - o).abs() < scaled_zero {
                // when current parameter classification conflicts with previous ones - return inconsitency
                if ret == EndClass::BelowStart
                    || ret == EndClass::AboveEnd
                    || ret == EndClass::AtEnd
                {
                    return Ok(EndClass::Inconsistent);
                }

                // don't change unlabeled and nonMinumum classifications
                if ret != EndClass::Inconsistent && ret != EndClass::AtStart {
                    continue;
                }

                // Solutions at same point must be the same
                if (self.start.solution_value - e.solution_value).abs() > EFF_ZERO_SQRT {
                    return Ok(EndClass::Inconsistent);
                }
                ret = EndClass::AtStart; // Match start
                continue;
            }

            // ThisSolution->End = OtherSolution->End
            if (t - o).abs() < scaled_zero {
                // when current parameter classification conflicts with previous ones - return inconsitency
                if ret == EndClass::BelowStart
                    || ret == EndClass::AboveEnd
                    || ret == EndClass::AtStart
                {
                    return Ok(EndClass::Inconsistent);
                }

                // don't change unlabeled and nonMaximum classifications
                if ret != EndClass::Inconsistent && ret != EndClass::AtEnd {
                    continue;
                }

                // Solutions at same point must be the same
                if (self.end.solution_value - e.solution_value).abs() > EFF_ZERO_SQRT {
                    return Ok(EndClass::Inconsistent);
                }
                ret = EndClass::AtEnd; // Match end
                continue;
            }

            // Ascending [This->Start, Other->End, This->End] and matching solution values
            if are_sequential(s, o, t) {
                if ret != EndClass::Inconsistent && ret != EndClass::Within {
                    return Ok(EndClass::Inconsistent);
                }
                ret = EndClass::Within; // Between start and end
                continue;
            }

            // Below Start, [Other->End, This->Start, This->End]
            if are_sequential(o, s, t) {
                if ret != EndClass::Inconsistent && ret != EndClass::BelowStart {
                    return Ok(EndClass::Inconsistent);
                }
                ret = EndClass::BelowStart; // Below Start
                continue;
            }

            // Above End, [This->Start, This->End, Other->End]
            if are_sequential(s, t, o) {
                if ret != EndClass::Inconsistent && ret != EndClass::AboveEnd {
                    return Ok(EndClass::Inconsistent);
                }
                ret = EndClass::AboveEnd; // Above End
                continue;
            }

            // If we make it to here then we have some inconsistency
            return Ok(EndClass::Inconsistent);
        }

        Ok(ret)
    }

    /// The object index and the index of its first parameter for `target`
    /// (`object_flag`) or for the first object that is not `target`
    /// (`!object_flag`) (`GetIndex`). `None` when there is no such object.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays, or a null object before the
    /// hit.
    pub fn index_of(&self, target: Option<O>, object_flag: bool) -> Result<Option<(usize, usize)>> {
        const OP: &str = "geom::solution_array::Solution::index_of";
        self.check(OP)?;
        let mut param_index = 0;

        // figure out the index for [pObject,bThisFlag]
        for (ii, &object) in self.used_objects().iter().enumerate() {
            // check for a hit
            if (object_flag && target == object) || (!object_flag && target != object) {
                return Ok(Some((ii, param_index)));
            }
            // This is not the index we want, move down to the next start
            // parameter based on the type of the object.
            let kind = object.ok_or(null_object(OP))?.kind();
            if kind.is_kind_of(ObjectKind::Curve) || kind.is_kind_of(ObjectKind::Edge) {
                param_index += 1;
            } else if kind.is_kind_of(ObjectKind::Face) || kind.is_kind_of(ObjectKind::Surface) {
                param_index += 2;
            }
        }

        // arrive here with no hits
        Ok(None)
    }

    /// The 3D point and parameter (`[u 0 0]` on curves and edges, `[u v 0]` on
    /// surfaces and faces, zero otherwise) of object `index`, at the end of a
    /// range when `get_end`, else at the start (`GetPoint(ULONG, ...)`). `None`
    /// (upstream `SM_ERR`) for an index out of range or an object kind without
    /// a point.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays or a null object
    /// is reached; [`SolidError::Unported`] for object geometry.
    /// The public query adapter currently resolves geometry in
    /// `query::api_queries::solution_point`; keep its `GetPoint` semantics in sync
    /// when wiring these object-geometry operations.
    pub fn point(&self, index: usize, get_end: bool) -> Result<Option<(Point3, Vec3)>> {
        const OP: &str = "geom::solution_array::Solution::point";
        self.check(OP)?;

        // Not an error if bGetEnd is passed True for a point solution.
        let get_end = get_end && self.solution_type == SolutionType::RangeOfValues;

        // check state - lIndex out of range
        if index >= self.num_objects {
            return Ok(None);
        }

        // local - parameter index associated with the lIndex object
        let mut first_param = 0;
        let param = |end: &SolutionEnd, start: &SolutionEnd, i: usize| -> Result<f64> {
            if get_end {
                end.param(i)
            } else {
                start.param(i)
            }
        };

        // for every object in this solution
        for i in 0..=index {
            let object = self.object(i, OP)?.ok_or(null_object(OP))?;
            let kind = object.kind();

            // convert object index into parameter index
            if i != index {
                if kind.is_kind_of(ObjectKind::Curve) {
                    first_param += 1;
                }
                if kind.is_kind_of(ObjectKind::Edge) {
                    first_param += 1;
                }
                if kind.is_kind_of(ObjectKind::Face) {
                    first_param += 2;
                }
                continue;
            }

            // found the lIndex object
            if kind.is_kind_of(ObjectKind::Vertex) {
                return vertex_point(&object).map(|p| Some((p, Vec3::ZERO)));
            } else if kind.is_kind_of(ObjectKind::Edge) {
                let curve = edge_curve(&object)?;
                let d = param(&self.end, &self.start, first_param)?;
                let p = evaluate_curve(&curve, d)?;
                return Ok(Some((p, Vec3::new(d, 0.0, 0.0))));
            } else if kind.is_kind_of(ObjectKind::Curve) {
                let d = param(&self.end, &self.start, first_param)?;
                let p = evaluate_curve(&object, d)?;
                return Ok(Some((p, Vec3::new(d, 0.0, 0.0))));
            } else if kind.is_kind_of(ObjectKind::Face) {
                let surface = face_surface(&object)?;
                let u = param(&self.end, &self.start, first_param)?;
                let v = param(&self.end, &self.start, first_param + 1)?;
                let p = evaluate_surface(&surface, u, v)?;
                return Ok(Some((p, Vec3::new(u, v, 0.0))));
            } else if kind.is_kind_of(ObjectKind::PolyVertex) {
                return poly_vertex_point(&object).map(|p| Some((p, Vec3::ZERO)));
            } else if kind.is_kind_of(ObjectKind::Surface) {
                let u = param(&self.end, &self.start, first_param)?;
                let v = param(&self.end, &self.start, first_param + 1)?;
                let p = evaluate_surface(&object, u, v)?;
                return Ok(Some((p, Vec3::new(u, v, 0.0))));
            } else if kind.is_kind_of(ObjectKind::PolyEdge) || kind.is_kind_of(ObjectKind::PolyFace)
            {
                // PolyEdges and PolyFaces do not have parameterized geometry: the
                // point exactly on the Poly topology is stored in the solution.
                let p = Vec3::new(
                    self.start.param(3)?,
                    self.start.param(4)?,
                    self.start.param(5)?,
                );
                return Ok(Some((p, Vec3::new(self.start.param(0)?, 0.0, 0.0))));
            }
        }

        // arrive here with an unhandled type
        Ok(None)
    }

    /// [`Solution::point`] of `target` (`object_flag`) or of the first object
    /// that is not `target` (`!object_flag`) (`GetPoint(const SmObject*, ...)`).
    /// `None` when [`Solution::index_of`] finds no such object. When the object
    /// has no point the result is the uninitialised point and a zero parameter
    /// (upstream ignores the inner status).
    ///
    /// # Errors
    ///
    /// As [`Solution::point`].
    pub fn point_of(
        &self,
        target: Option<O>,
        object_flag: bool,
        get_end: bool,
    ) -> Result<Option<(Point3, Vec3)>> {
        let Some((index, _)) = self.index_of(target, object_flag)? else {
            return Ok(None);
        };

        // pass the call along
        Ok(Some(
            self.point(index, get_end)?
                .unwrap_or((Vec3::UNDEF, Vec3::ZERO)),
        ))
    }

    /// The object index and domain direction (`0` = u, `1` = v, `2` = w) of
    /// solution parameter `param_index` (`GetObjectIndexForParameterIndex`).
    /// `None` (upstream `SM_ERR`) for an index outside the variables, no
    /// objects, a null object or an object kind without parameters.
    ///
    /// As upstream, every pass of the search looks at the object at the
    /// current result index, which stays `0` until the search stops.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays.
    pub fn object_index_for_parameter_index(
        &self,
        param_index: usize,
    ) -> Result<Option<(usize, usize)>> {
        const OP: &str = "geom::solution_array::Solution::object_index_for_parameter_index";
        self.check(OP)?;
        let mut object_index = 0;
        let mut domain_dir = 0;

        // check input
        if param_index >= self.num_variables || self.num_objects == 0 {
            return Ok(None);
        }

        let mut p_index = 0;

        // else iter through all the domains
        for ii in 0..self.num_objects {
            // some solvers actually don't store objects for every parameter value.
            let Some(object) = self.object(object_index, OP)? else {
                return Ok(None);
            };
            let kind = object.kind();

            if kind.is_kind_of(ObjectKind::Edge) || kind.is_kind_of(ObjectKind::Curve) {
                // things with 1 parameter
                if p_index + 1 > param_index {
                    domain_dir = 0;
                    object_index = ii;
                    break;
                }
                p_index += 1;
            } else if kind.is_kind_of(ObjectKind::Face) || kind.is_kind_of(ObjectKind::Surface) {
                // things with 2 parameters
                if p_index + 2 > param_index {
                    domain_dir = param_index - p_index;
                    object_index = ii;
                    break;
                }
                p_index += 2;
            } else if kind.is_kind_of(ObjectKind::Volume) {
                // things with 3 parameters
                if p_index + 3 > param_index {
                    domain_dir = param_index - p_index;
                    object_index = ii;
                    break;
                }
                p_index += 3;
            } else if !(kind.is_kind_of(ObjectKind::Vertex)
                || kind.is_kind_of(ObjectKind::PolyBrep)
                || kind.is_kind_of(ObjectKind::PolyRegion)
                || kind.is_kind_of(ObjectKind::PolyShell)
                || kind.is_kind_of(ObjectKind::PolyLoop)
                || kind.is_kind_of(ObjectKind::CPolyFace)
                || kind.is_kind_of(ObjectKind::PolyFace)
                || kind.is_kind_of(ObjectKind::PolyEdge)
                || kind.is_kind_of(ObjectKind::PolyVertex))
            {
                // error (things without parameters go on to the next pass)
                return Ok(None);
            }
        }

        // all done
        Ok(Some((object_index, domain_dir)))
    }

    /// The parameter interval of the object behind solution parameter
    /// `param_index` (`GetParameterInterval`). `None` (upstream `SM_ERR`) when
    /// [`Solution::object_index_for_parameter_index`] fails or the object has no
    /// parameters.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays;
    /// [`SolidError::Unported`] for the parameter domain of the object.
    pub fn parameter_interval(&self, param_index: usize) -> Result<Option<Extent1d>> {
        self.parameter_interval_in(param_index, &OwnDomains)
    }

    /// [`Solution::parameter_interval`] reading the objects' parameter domains through
    /// `domains`.
    ///
    /// # Errors
    ///
    /// As [`Solution::parameter_interval`].
    pub fn parameter_interval_in(
        &self,
        param_index: usize,
        domains: &dyn ParameterDomains<O>,
    ) -> Result<Option<Extent1d>> {
        const OP: &str = "geom::solution_array::Solution::parameter_interval";

        // get the object and associated domain dir
        let Some((object_index, domain_dir)) =
            self.object_index_for_parameter_index(param_index)?
        else {
            return Ok(None);
        };
        let object = self.object(object_index, OP)?.ok_or(null_object(OP))?;
        let kind = object.kind();

        // Get the parameter interval from pObject
        let ivl = if kind.is_kind_of(ObjectKind::Edge) {
            edge_interval(domains, &object)?
        } else if kind.is_kind_of(ObjectKind::Curve) {
            curve_natural_interval(domains, &object)?
        } else if kind.is_kind_of(ObjectKind::Face) {
            let domain = face_uv_domain(domains, &object)?;
            if domain_dir == 0 {
                domain.u_interval()
            } else {
                domain.v_interval()
            }
        } else if kind.is_kind_of(ObjectKind::Surface) {
            let domain = surface_natural_uv_domain(domains, &object)?;
            if domain_dir == 0 {
                domain.u_interval()
            } else {
                domain.v_interval()
            }
        } else if kind.is_kind_of(ObjectKind::Volume) {
            let domain = volume_natural_param_domain(&object)?;
            if domain_dir == 0 {
                domain.u_interval()
            } else if domain_dir == 1 {
                domain.v_interval()
            } else {
                domain.w_interval()
            }
        } else {
            // includes things without parameters
            return Ok(None);
        };

        // all done
        Ok(Some(ivl))
    }

    /// The zone tolerance of object `index`, `0.0` for an index outside the
    /// objects or a null object (`GetObjectTolerance`), for handles nothing can
    /// resolve: see [`Solution::object_tolerance_with`].
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays;
    /// [`SolidError::Unported`] for a non-null object (zone tolerances).
    pub fn object_tolerance(&self, index: usize) -> Result<f64> {
        self.object_tolerance_with(index, |_| {
            // UNPORTED(geom.tol): SmTol::GetZoneTol3d(const SmObject*) on an opaque handle
            // (callers that can resolve it use `object_tolerance_with`)
            // Proof (unreachable): no library caller (object_tolerance is used by tests and
            // difftest); the library uses object_tolerance_with
            // (GlobalSolve::object_zone_tol_3d); 0 hits in the green difftest sweep.
            Err(SolidError::Unported {
                upstream: "SmTol::GetZoneTol3d(const SmObject*)",
                unit: "geom.tol",
            })
        })
    }

    /// The zone tolerance of object `index` (`GetObjectTolerance`): `0.0` for an index
    /// outside the objects or a null object, else `zone_tol_3d` of the object (the
    /// caller's `SmTol::GetZoneTol3d(const SmObject*)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when a variable or object count exceeds the
    /// fixed arrays; those of `zone_tol_3d`.
    pub fn object_tolerance_with(
        &self,
        index: usize,
        zone_tol_3d: impl FnOnce(O) -> Result<f64>,
    ) -> Result<f64> {
        const OP: &str = "geom::solution_array::Solution::object_tolerance";
        self.check(OP)?;

        // no tolerance for invalid indices
        if index >= self.num_objects {
            return Ok(0.0);
        }

        // pass the call along
        match self.object(index, OP)? {
            Some(o) => zone_tol_3d(o),
            None => Ok(0.0),
        }
    }

    /// True when `other` is the same solution (`IsEqual`): same object and
    /// variable counts, type and objects, solution values within
    /// `SM_EFF_ZERO_SQRT`, and every parameter (both ends of a range) within
    /// the larger of a scaled zero and `1e-4` of the parameter's interval
    /// length.
    ///
    /// # Errors
    ///
    /// As [`Solution::parameter_interval`].
    pub fn is_equal(&self, other: &Self) -> Result<bool> {
        self.is_equal_in(other, &OwnDomains)
    }

    /// [`Solution::is_equal`] reading the objects' parameter domains through
    /// `domains` ([`Solution::parameter_interval_in`]).
    ///
    /// # Errors
    ///
    /// As [`Solution::parameter_interval_in`].
    pub fn is_equal_in(&self, other: &Self, domains: &dyn ParameterDomains<O>) -> Result<bool> {
        const OP: &str = "geom::solution_array::Solution::is_equal";
        self.check(OP)?;
        other.check(OP)?;
        let (s1, s2) = (self, other);

        // different ObjectCounts, VariableCounts, SolutionValues, SolutionTypes
        if s1.num_objects != s2.num_objects
            || s1.num_variables != s2.num_variables
            || s1.solution_type != s2.solution_type
            || EFF_ZERO_SQRT < (s1.start.solution_value - s2.start.solution_value).abs()
        {
            return Ok(false);
        }

        // unequal ObjectPointers or parameterValues
        for (a, b) in s1.used_objects().iter().zip(s2.used_objects()) {
            if a != b {
                return Ok(false);
            }
        }

        // ParameterValues off by more than ScaledZero or more than 1/10000th of the parameter length
        for i in 0..s1.num_variables {
            let ivl1 = s1.parameter_interval_in(i, domains)?;
            let ivl2 = s2.parameter_interval_in(i, domains)?;
            let length1 = ivl1.map_or(0.0, |ivl| 0.0001 * ivl.length());
            let length2 = ivl2.map_or(0.0, |ivl| 0.0001 * ivl.length());

            let a = s1.start.param(i)?;
            let scaled_zero = max3((1.0 + a.abs()).abs() * EFF_ZERO, length1, length2);

            if (a - s2.start.param(i)?).abs() > scaled_zero
                || (s1.solution_type == SolutionType::RangeOfValues
                    && (s1.end.param(i)? - s2.end.param(i)?).abs() > scaled_zero)
            {
                return Ok(false);
            }
        }

        // All tests passed - identical Solutions
        Ok(true)
    }
}

// ---- calls into units not ported yet -----------------------------------------
//
// Each takes the object and the values upstream passes so that wiring the real
// call is a local change.

/// `SmVertex::GetPoint`: the handle's [`SolutionObject::vertex_point`].
fn vertex_point<O: SolutionObject>(vertex: &O) -> Result<Point3> {
    if let Some(p) = vertex.vertex_point() {
        return Ok(p);
    }
    // UNPORTED(topology.vertex): SmVertex::GetPoint (a handle that cannot reach its
    // vertex)
    // Proof (unreachable): no library caller (Solution::point/point_of are used by tests
    // and difftest); the library resolves through query::api_queries::solution_point; 0
    // hits in the green difftest sweep.
    Err(SolidError::Unported {
        upstream: "SmVertex::GetPoint",
        unit: "topology.vertex",
    })
}

/// `SmEdge::GetCurve`: the handle's [`SolutionObject::edge_curve`].
fn edge_curve<O: SolutionObject>(edge: &O) -> Result<O> {
    if let Some(c) = edge.edge_curve() {
        return Ok(c);
    }
    // UNPORTED(topology.edge): SmEdge::GetCurve (a handle that cannot reach its edge)
    // Proof (unreachable): no library caller (Solution::point/point_of are used by tests
    // and difftest); the library resolves through query::api_queries::solution_point; 0
    // hits in the green difftest sweep.
    Err(SolidError::Unported {
        upstream: "SmEdge::GetCurve",
        unit: "topology.edge",
    })
}

/// `SmEdge::GetInterval`: [`ParameterDomains::edge_interval`].
fn edge_interval<O: SolutionObject>(
    domains: &dyn ParameterDomains<O>,
    edge: &O,
) -> Result<Extent1d> {
    if let Some(ivl) = domains.edge_interval(edge) {
        return Ok(ivl);
    }
    // UNPORTED(topology.edge): SmEdge::GetInterval (a handle that cannot reach its edge)
    Err(SolidError::Unported {
        upstream: "SmEdge::GetInterval",
        unit: "topology.edge",
    })
}

/// `SmCurve::EvaluatePoint` at `t` (its status is ignored upstream: `SE`): the
/// handle's [`SolutionObject::curve_point`].
fn evaluate_curve<O: SolutionObject>(curve: &O, t: f64) -> Result<Point3> {
    if let Some(p) = curve.curve_point(t) {
        return Ok(p);
    }
    // UNPORTED(curve.curve_p2): SmCurve::EvaluatePoint (a handle that cannot reach its
    // curve, or a failed evaluation, whose point upstream leaves as it was)
    // Proof (unreachable): no library caller (Solution::point/point_of are used by tests
    // and difftest); the library resolves through query::api_queries::solution_point; 0
    // hits in the green difftest sweep.
    Err(SolidError::Unported {
        upstream: "SmCurve::EvaluatePoint",
        unit: "curve.curve_p2",
    })
}

/// `SmCurve::GetNaturalInterval`: [`ParameterDomains::natural_interval`].
fn curve_natural_interval<O: SolutionObject>(
    domains: &dyn ParameterDomains<O>,
    curve: &O,
) -> Result<Extent1d> {
    if let Some(ivl) = domains.natural_interval(curve) {
        return Ok(ivl);
    }
    // Upstream reads the SmCurve the handle points to; every library path resolves it
    // (the 1.0.1 audit of every IdenticalSolutions and IsEqual reader):
    // - the curve solvers' IdenticalSolutions resolve a curve handle to the curve its
    //   tree decomposes (TreeCurves: CcGlobalSolver, CpGlobalSolver, CciGlobalSolver,
    //   CurvePropertyGs), the curve/surface and surface solvers to their operands
    //   (SolveOperands, LsiGlobalSolver, CsGlobalSolver, SpGlobalSolver, SsGlobalSolver,
    //   CsiGlobalSolver, TopologySolver); trimming-tool loops read their curve store
    //   (StoreDomains);
    // - GlobalCoincidenceChecker's merge of its split parts resolves the two curves'
    //   handles to their curves (curve::curve::p6 MergeSolver, CurvePair);
    // - the solvers that keep GlobalSolver's default IdenticalSolutions
    //   (InteriorSolver, AdvSurfaceIntersector, GlobalSolver itself) never reach it from
    //   their own solves: their LocalSolve is GlobalSolver::local_solve (no SurfaceTrace
    //   overrides it) or AdvSurfaceIntersector's SolveBranch, which store NodesOnly or
    //   leaf solutions with SolutionArray::add; the GlobalSolver instances that call
    //   AddSortedSolution name an interval-carrying CurveHandle (surface::plane::p2) or a
    //   surface (SurfOfRevolution::drop_point_fast_using's OperandObject::Surface);
    // - SolutionArray::add_sorted_solution over OwnDomains is called only by
    //   SmSurfPtGlobalSolver's seam solutions (surface::surface_point_solve::p2), which
    //   name the OperandObject::Surface stand-in of Surface::global_point_solve_in;
    //   Solution::is_equal, Solution::parameter_interval and
    //   GlobalSolver::identical_solutions over OwnDomains have test callers only.
    // A handle reaching here names no curve the caller can resolve: invalid input.
    Err(SolidError::invalid_input(
        "geom::solution_array::Solution::parameter_interval",
        "curve handle without a natural interval",
    ))
}

/// `SmFace::GetSurface`: the handle's [`SolutionObject::face_surface`].
fn face_surface<O: SolutionObject>(face: &O) -> Result<O> {
    if let Some(sf) = face.face_surface() {
        return Ok(sf);
    }
    // UNPORTED(topology.face_p2): SmFace::GetSurface (a handle that cannot reach its
    // face)
    // Proof (unreachable): no library caller (Solution::point/point_of are used by tests
    // and difftest); the library resolves through query::api_queries::solution_point; 0
    // hits in the green difftest sweep.
    Err(SolidError::Unported {
        upstream: "SmFace::GetSurface",
        unit: "topology.face_p2",
    })
}

/// `SmFace::GetUVDomain`: [`ParameterDomains::face_uv_domain`].
fn face_uv_domain<O: SolutionObject>(
    domains: &dyn ParameterDomains<O>,
    face: &O,
) -> Result<crate::geom::extent2d::Extent2d> {
    if let Some(d) = domains.face_uv_domain(face) {
        return Ok(d);
    }
    // UNPORTED(topology.face_p2): SmFace::GetUVDomain (a handle that cannot reach its
    // face)
    Err(SolidError::Unported {
        upstream: "SmFace::GetUVDomain",
        unit: "topology.face_p2",
    })
}

/// `SmSurface::EvaluatePoint` at `(u, v)` (its status is ignored upstream: `SE`): the
/// handle's [`SolutionObject::surface_point`].
fn evaluate_surface<O: SolutionObject>(surface: &O, u: f64, v: f64) -> Result<Point3> {
    if let Some(p) = surface.surface_point(u, v) {
        return Ok(p);
    }
    // UNPORTED(surface.surface_p2): SmSurface::EvaluatePoint (a handle that cannot
    // reach its surface, or a failed evaluation, whose point upstream leaves as it was)
    // Proof (unreachable): no library caller (Solution::point/point_of are used by tests
    // and difftest); the library resolves through query::api_queries::solution_point; 0
    // hits in the green difftest sweep.
    Err(SolidError::Unported {
        upstream: "SmSurface::EvaluatePoint",
        unit: "surface.surface_p2",
    })
}

/// `SmSurface::GetNaturalUVDomain`: [`ParameterDomains::natural_uv_domain`].
fn surface_natural_uv_domain<O: SolutionObject>(
    domains: &dyn ParameterDomains<O>,
    surface: &O,
) -> Result<crate::geom::extent2d::Extent2d> {
    if let Some(domain) = domains.natural_uv_domain(surface) {
        return Ok(domain);
    }
    // UNPORTED(surface.surface_p2): SmSurface::GetNaturalUVDomain (a handle that
    // cannot reach its surface)
    Err(SolidError::Unported {
        upstream: "SmSurface::GetNaturalUVDomain",
        unit: "surface.surface_p2",
    })
}

/// `SmVolume::GetNaturalParamDomain`. The volume itself is ported
/// (`surface::volume::Volume::natural_param_domain`), but a solution object is an
/// opaque handle `O` with no way to reach it yet.
fn volume_natural_param_domain<O>(_volume: &O) -> Result<crate::geom::extent3d::Extent3d> {
    // UNPORTED(surface.volume_p1): SmVolume::GetNaturalParamDomain (needs a handle-to-volume
    // lookup on SolutionObject)
    Err(SolidError::Unported {
        upstream: "SmVolume::GetNaturalParamDomain",
        unit: "surface.volume_p1",
    })
}

/// `SmPolyVertex::GetPoint` through the handle ([`SolutionObject::poly_vertex_point`]).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a handle that cannot reach its poly vertex.
fn poly_vertex_point<O: SolutionObject>(poly_vertex: &O) -> Result<Point3> {
    poly_vertex
        .poly_vertex_point()
        .ok_or(SolidError::InvalidInput {
            operation: "geom::solution_array::poly_vertex_point",
            reason: "solution object handle cannot reach its poly vertex",
        })
}

/// A growable array of solutions (`SmSolutionArray`), the result container of
/// the global solvers.
#[derive(Debug, Clone, PartialEq)]
pub struct SolutionArray<O> {
    /// `m_pData[0 .. m_lSize]`.
    data: Vec<Solution<O>>,
}

impl<O: SolutionObject> Default for SolutionArray<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: SolutionObject> SolutionArray<O> {
    /// `SmSolutionArray_TYPE`.
    pub const TYPE: i64 = SM_SOLUTION_ARRAY_TYPE;

    /// An empty array (`SmSolutionArray()`).
    #[must_use]
    pub const fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// An empty array with room for `capacity` solutions
    /// (`SmSolutionArray(nDataSize, NULL)`).
    ///
    /// For small capacities; use [`SolutionArray::try_with_capacity`] for
    /// capacities that come from caller input.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    /// [`SolutionArray::with_capacity`] for capacities that come from caller input.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the room cannot be allocated (upstream's
    /// `smos_Calloc` returns `NULL`).
    pub fn try_with_capacity(capacity: usize) -> Result<Self> {
        let mut a = Self::new();
        a.reserve_to(
            capacity,
            "geom::solution_array::SolutionArray::try_with_capacity",
        )?;
        Ok(a)
    }

    /// `SmSolutionArray_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmSolutionArray_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmSolutionArray_TYPE"
    }

    /// True for `SmSolutionArray_TYPE` and its base `SmObject_TYPE` (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == SM_SOLUTION_ARRAY_TYPE || object::is_kind_of(t)
    }

    /// Number of solutions held (`GetSize`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True when no solution is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Sets the size, zero-filling new slots ([`Solution::zeroed`]) and dropping
    /// solutions past `size` (`SetSize`).
    ///
    /// For sizes bounded by data already held. Like any `Vec`, it aborts when the
    /// slots cannot be allocated; use [`SolutionArray::try_set_size`] for sizes that
    /// come from caller input.
    pub fn set_size(&mut self, size: usize) {
        self.data.resize(size, Solution::zeroed());
    }

    /// [`SolutionArray::set_size`] for sizes that come from caller input.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the slots cannot be allocated (upstream's
    /// `smos_Calloc` returns `NULL` and is written through); the array is then
    /// unchanged.
    pub fn try_set_size(&mut self, size: usize) -> Result<()> {
        self.reserve_to(size, "geom::solution_array::SolutionArray::try_set_size")?;
        self.set_size(size);
        Ok(())
    }

    /// Makes room for a total of `size` solutions, `InvalidInput` when that cannot
    /// be allocated or would take more than [`crate::limits::MAX_ALLOC_BYTES`].
    fn reserve_to(&mut self, size: usize, operation: &'static str) -> Result<()> {
        if try_reserve_total(&mut self.data, size) {
            Ok(())
        } else {
            Err(too_large(operation))
        }
    }

    /// Removes every solution (`ReSet`).
    pub fn reset(&mut self) {
        self.data.clear();
    }

    /// The solutions (`GetDataArray`).
    #[must_use]
    pub fn as_slice(&self) -> &[Solution<O>] {
        &self.data
    }

    /// Solution `index`, `None` past the size (`operator[]`).
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Solution<O>> {
        self.data.get(index)
    }

    /// Mutable solution `index`, `None` past the size (`operator[]`).
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Solution<O>> {
        self.data.get_mut(index)
    }

    /// A copy of solution `index` (`GetAt`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index past the size.
    pub fn get_at(&self, index: usize) -> Result<Solution<O>> {
        self.data
            .get(index)
            .copied()
            .ok_or(bad_index("geom::solution_array::SolutionArray::get_at"))
    }

    /// Replaces solution `index` (`SetAt`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index past the size.
    pub fn set_at(&mut self, index: usize, solution: &Solution<O>) -> Result<()> {
        let slot = self
            .data
            .get_mut(index)
            .ok_or(bad_index("geom::solution_array::SolutionArray::set_at"))?;
        *slot = *solution;
        Ok(())
    }

    /// Sets solution `index`, growing the array to `index + 1` when needed
    /// (`SetAtGrow`).
    fn set_at_grow(&mut self, index: usize, solution: &Solution<O>) {
        if index >= self.data.len() {
            self.set_size(index + 1);
        }
        if let Some(slot) = self.data.get_mut(index) {
            *slot = *solution;
        }
    }

    /// The solutions with every object cleared (null pointers), in another handle
    /// type: what a caller that named no objects gets back from a solve that named
    /// [`OperandObject`]s internally.
    #[must_use]
    pub(crate) fn without_objects<P: SolutionObject>(&self) -> SolutionArray<P> {
        let mut out = SolutionArray::new();
        for s in &self.data {
            out.data.push(Solution {
                solution_type: s.solution_type,
                num_variables: s.num_variables,
                start: s.start,
                end: s.end,
                num_objects: s.num_objects,
                objects: [None; MAX_TREES],
                nodes: s.nodes,
            });
        }
        out
    }

    /// Appends `solution`; returns its index (`Add`).
    pub fn add(&mut self, solution: &Solution<O>) -> usize {
        let index = self.data.len();
        self.set_at_grow(index, solution);
        index
    }

    /// Appends every solution of `source`; returns the old size (`Append`).
    pub fn append(&mut self, source: &Self) -> usize {
        let old_size = self.data.len();
        self.data.extend_from_slice(&source.data);
        old_size
    }

    /// Inserts `count` copies of `solution` at `index` (`InsertAt`). An index
    /// at or past the size first grows the array to `index + count`
    /// (zero-filled).
    pub fn insert_at(&mut self, index: usize, solution: &Solution<O>, count: usize) {
        if index >= self.data.len() {
            // adding after the end of the array
            self.set_size(index.saturating_add(count));
        } else {
            // inserting in the middle of the array: shift old data up to fill gap
            let tail = self.data.split_off(index);
            self.data.resize(index + count, Solution::zeroed());
            self.data.extend(tail);
        }

        // insert new value in the gap
        for slot in self.data.iter_mut().skip(index).take(count) {
            *slot = *solution;
        }
    }

    /// [`SolutionArray::insert_at`] for an index and count that come from caller
    /// input.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the grown size overflows or cannot be
    /// allocated (upstream's `smos_Calloc` returns `NULL` and is written through);
    /// the array is then unchanged.
    pub fn try_insert_at(
        &mut self,
        index: usize,
        solution: &Solution<O>,
        count: usize,
    ) -> Result<()> {
        const OP: &str = "geom::solution_array::SolutionArray::try_insert_at";
        let base = if index >= self.data.len() {
            index
        } else {
            self.data.len()
        };
        let size = base.checked_add(count).ok_or_else(|| too_large(OP))?;
        self.reserve_to(size, OP)?;
        self.insert_at(index, solution, count);
        Ok(())
    }

    /// Removes `count` solutions at `index` (`RemoveAt`). As upstream, when
    /// `index + count` passes the size nothing moves and the size still drops
    /// by `count` (to at least zero).
    pub fn remove_at(&mut self, index: usize, count: usize) {
        let size = self.data.len();

        // just remove a range
        if let Some(stop) = index.checked_add(count) {
            if size > stop {
                self.data.copy_within(stop..size, index);
            }
        }

        // adjust the size
        self.data.truncate(size.saturating_sub(count));
    }

    /// Swaps solutions `index1` and `index2` (`Swap`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index past the size.
    pub fn swap(&mut self, index1: usize, index2: usize) -> Result<()> {
        if index1 >= self.data.len() || index2 >= self.data.len() {
            return Err(bad_index("geom::solution_array::SolutionArray::swap"));
        }
        // when needed, swap the solutions
        if index1 != index2 {
            self.data.swap(index1, index2);
        }
        Ok(())
    }

    /// Adds `new` in sorted position by `key`, smaller first
    /// (`AddSortedSolution`). Returns the index it was added at, or `None` when
    /// it was rejected: its value is `SM_BIG_DOUBLE` (uninitialised), or it
    /// [`Solution::is_equal`] a solution already held, which `new` then
    /// replaces when its value is smaller in magnitude. Solutions are never
    /// merged.
    ///
    /// # Errors
    ///
    /// Errors of [`Solution::is_equal`] and of the parameter reads.
    pub fn add_sorted_solution(
        &mut self,
        new: &Solution<O>,
        key: SortKey,
    ) -> Result<Option<usize>> {
        self.add_sorted_solution_in(new, key, &OwnDomains)
    }

    /// [`SolutionArray::add_sorted_solution`] comparing solutions with
    /// [`Solution::is_equal_in`] over `domains` (the parameter domains of the
    /// solutions' objects).
    ///
    /// # Errors
    ///
    /// Errors of [`Solution::is_equal_in`] and of the parameter reads.
    pub fn add_sorted_solution_in(
        &mut self,
        new: &Solution<O>,
        key: SortKey,
        domains: &dyn ParameterDomains<O>,
    ) -> Result<Option<usize>> {
        // skip BigSolutions - unitialized values
        if new.start.solution_value == BIG_DOUBLE {
            return Ok(None);
        }

        // Now do insertion sort
        let mut found_place = false;
        let mut insertion_index = self.data.len();

        // for every current solution
        for (ii, old) in self.data.iter_mut().enumerate() {
            // don't add duplicate solutions (SolValues within ZeroSqrt and ParamValues within ScaledZero)
            if old.is_equal_in(new, domains)? {
                // save the better solution - improve tolerances
                if new.start.solution_value.abs() < old.start.solution_value.abs() {
                    *old = *new;
                }
                return Ok(None);
            }

            let old_v = old.start.solution_value;
            let new_v = new.start.solution_value;
            let mut by_all_parameters = false;
            match key {
                SortKey::BySolutionValue => {
                    if old_v > new_v {
                        insertion_index = ii;
                        found_place = true;
                    }
                }
                SortKey::ByFirstParameter => {
                    if old.start.param(0)? > new.start.param(0)? {
                        insertion_index = ii;
                        found_place = true;
                    }
                }
                SortKey::ByFirstTwoParameters => {
                    let (o0, n0) = (old.start.param(0)?, new.start.param(0)?);
                    let scale = (1.0 + o0) * EFF_ZERO_SQRT;
                    if (o0 > n0 + scale)
                        || ((o0 - n0).abs() < scale && old.start.param(1)? > new.start.param(1)?)
                    {
                        insertion_index = ii;
                        found_place = true;
                    }
                }
                SortKey::ByValueWithParameter => {
                    let scale = (1.0 + old_v) * EFF_ZERO_SQRT;

                    // when sSolutionValues are unique
                    if (old_v - new_v).abs() > scale {
                        // Just use standard by value sorting
                        if old_v > new_v {
                            insertion_index = ii;
                            found_place = true;
                        }
                    } else {
                        // else SolutionValues are the same within tolerance:
                        // fall through and sort by parameters
                        by_all_parameters = true;
                    }
                }
                SortKey::ByAllParameters => by_all_parameters = true,
            }

            if by_all_parameters {
                old.check("geom::solution_array::SolutionArray::add_sorted_solution")?;
                for jj in 0..old.num_variables {
                    let (o, n) = (old.start.param(jj)?, new.start.param(jj)?);
                    let scale = (1.0 + o) * EFF_ZERO_SQRT;

                    // if old param is bigger by tolerance - insert here
                    if o > n + scale {
                        insertion_index = ii;
                        found_place = true;
                        break;
                    }
                    // else if old param is smaller by tolerance - go onto next tgt
                    else if o < n - scale {
                        break;
                    }
                    // else old param is equal by tolerance to new param
                    //  - go onto next param if there is one or else go onto next tgt
                }
            }

            if found_place {
                break;
            }
        }

        // insert the solution in its spot or at the end of the array
        if found_place {
            self.insert_at(insertion_index, new, 1);
        } else {
            insertion_index = self.add(new);
        }

        // all done
        Ok(Some(insertion_index))
    }
}

fn too_large(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "solution array too large to allocate",
    }
}

fn bad_index(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "solution index out of range",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Obj(u32, ObjectKind);

    impl SolutionObject for Obj {
        fn kind(&self) -> ObjectKind {
            self.1
        }
    }

    fn single(value: f64, params: &[f64]) -> Solution<Obj> {
        let mut s = Solution::new();
        s.solution_type = SolutionType::SingleValue;
        s.num_variables = params.len();
        s.start.solution_value = value;
        for (i, &p) in params.iter().enumerate() {
            *s.start.param_mut(i).unwrap() = p;
        }
        s
    }

    fn range(params0: &[f64], params1: &[f64]) -> Solution<Obj> {
        let mut s = single(0.0, params0);
        s.solution_type = SolutionType::RangeOfValues;
        for (i, &p) in params1.iter().enumerate() {
            *s.end.param_mut(i).unwrap() = p;
        }
        s
    }

    #[test]
    fn enums_and_types() {
        for t in SolutionType::ALL {
            assert_eq!(SolutionType::from_code(t.code()), Some(t));
        }
        assert_eq!(SolutionType::from_code(5), None);
        for k in SortKey::ALL {
            assert_eq!(SortKey::from_code(k.code()), Some(k));
        }
        assert!(ObjectKind::CPolyFace.is_kind_of(ObjectKind::PolyFace));
        assert!(!ObjectKind::PolyFace.is_kind_of(ObjectKind::CPolyFace));
        let a: SolutionArray<Obj> = SolutionArray::new();
        assert_eq!(a.type_code(), 12020);
        assert!(a.is_kind_of(12020) && a.is_kind_of(object::OBJECT_TYPE) && !a.is_kind_of(14001));
        assert_eq!(a.type_string(), "SmSolutionArray_TYPE");
        assert_eq!(
            Solution::<Obj>::zeroed().solution_type,
            SolutionType::NodesOnly
        );
        assert_eq!(Solution::<Obj>::new().solution_type, SolutionType::Unknown);
    }

    #[test]
    fn end_access() {
        let mut e = SolutionEnd::new();
        *e.param_mut(9).unwrap() = 2.0;
        assert_eq!(e.param(9).unwrap(), 2.0);
        assert!(e.param(10).is_err() && e.param_mut(10).is_err());
        e.solution_value = 1.0;
        e.reset();
        assert_eq!(e, SolutionEnd::new());
    }

    #[test]
    fn classify_points_and_ranges() {
        let a = single(0.0, &[1.0, 2.0]);
        assert_eq!(
            a.classify_solution_end(&single(0.0, &[1.0, 2.0]), true)
                .unwrap()
                .code(),
            0
        );
        assert_eq!(
            a.classify_solution_end(&single(0.0, &[0.5, 2.0]), true)
                .unwrap()
                .code(),
            -2
        );
        assert_eq!(
            a.classify_solution_end(&single(0.0, &[1.5, 2.0]), true)
                .unwrap()
                .code(),
            2
        );
        assert_eq!(
            a.classify_solution_end(&single(0.0, &[1.0, 3.0]), true)
                .unwrap()
                .code(),
            3
        );
        assert_eq!(
            a.classify_solution_end(&single(1.0, &[1.0, 2.0]), true)
                .unwrap()
                .code(),
            3
        );
        assert_eq!(
            a.classify_solution_end(&single(0.0, &[1.0]), true)
                .unwrap()
                .code(),
            3
        );
        let r = range(&[0.0, 0.0], &[1.0, 1.0]);
        assert_eq!(
            r.classify_solution_end(&single(0.0, &[0.5, 0.5]), true)
                .unwrap()
                .code(),
            0
        );
        assert_eq!(
            r.classify_solution_end(&single(0.0, &[0.0, 0.0]), true)
                .unwrap()
                .code(),
            -1
        );
        assert_eq!(
            r.classify_solution_end(&single(0.0, &[1.0, 1.0]), true)
                .unwrap()
                .code(),
            1
        );
        assert_eq!(
            r.classify_solution_end(&single(0.0, &[-1.0, -1.0]), true)
                .unwrap()
                .code(),
            -2
        );
        assert_eq!(
            r.classify_solution_end(&single(0.0, &[2.0, 2.0]), true)
                .unwrap()
                .code(),
            2
        );
        assert_eq!(
            r.classify_solution_end(&single(0.0, &[2.0, 0.5]), true)
                .unwrap()
                .code(),
            3
        );
        let other = range(&[5.0, 5.0], &[0.5, 0.5]);
        assert_eq!(r.classify_solution_end(&other, false).unwrap().code(), 0);
        let mut bad = single(0.0, &[]);
        bad.num_variables = 11;
        assert!(a.classify_solution_end(&bad, true).is_err());
    }

    #[test]
    fn indices_and_points() {
        let mut s = single(0.0, &[0.25, 0.0, 0.0, 7.0, 8.0, 9.0]);
        let objs = [
            Obj(1, ObjectKind::Curve),
            Obj(2, ObjectKind::Face),
            Obj(3, ObjectKind::PolyEdge),
            Obj(4, ObjectKind::Volume),
        ];
        s.num_objects = 4;
        for (i, o) in objs.iter().enumerate() {
            s.objects[i] = Some(*o);
        }
        assert_eq!(s.index_of(Some(objs[2]), true).unwrap(), Some((2, 3)));
        assert_eq!(s.index_of(Some(objs[0]), false).unwrap(), Some((1, 1)));
        assert_eq!(
            s.index_of(Some(Obj(9, ObjectKind::Other)), true).unwrap(),
            None
        );
        let (p, q) = s.point(2, true).unwrap().unwrap();
        assert_eq!(
            (p, q),
            (Vec3::new(7.0, 8.0, 9.0), Vec3::new(0.25, 0.0, 0.0))
        );
        assert_eq!(s.point(3, false).unwrap(), None);
        assert_eq!(s.point(4, false).unwrap(), None);
        assert!(matches!(
            s.point(0, false),
            Err(SolidError::Unported { .. })
        ));
        assert!(matches!(
            s.point(1, false),
            Err(SolidError::Unported { .. })
        ));
        assert_eq!(
            s.point_of(Some(objs[3]), true, false).unwrap(),
            Some((Vec3::UNDEF, Vec3::ZERO))
        );
        assert_eq!(s.point_of(None, true, false).unwrap(), None);
        // null objects are rejected where upstream dereferences them
        s.objects[0] = None;
        assert!(s.index_of(Some(objs[2]), true).is_err());
        assert!(s.point(2, false).is_err());
        assert_eq!(s.object_tolerance(0).unwrap(), 0.0);
        assert_eq!(s.object_tolerance(7).unwrap(), 0.0);
        assert!(s.object_tolerance(1).is_err());
    }

    #[test]
    fn parameter_index_scans_first_object() {
        let mut s = single(0.0, &[0.0; 6]);
        s.num_objects = 3;
        s.objects = [
            Some(Obj(1, ObjectKind::Surface)),
            Some(Obj(2, ObjectKind::Curve)),
            Some(Obj(3, ObjectKind::Curve)),
            None,
            None,
        ];
        assert_eq!(s.object_index_for_parameter_index(1).unwrap(), Some((0, 1)));
        // passes 2..: the first object is examined again with a grown base index
        assert_eq!(s.object_index_for_parameter_index(3).unwrap(), Some((1, 1)));
        assert_eq!(s.object_index_for_parameter_index(5).unwrap(), Some((2, 1)));
        assert_eq!(s.object_index_for_parameter_index(6).unwrap(), None);
        s.objects[0] = Some(Obj(1, ObjectKind::Vertex));
        assert_eq!(s.object_index_for_parameter_index(2).unwrap(), Some((0, 0)));
        assert_eq!(s.parameter_interval(2).unwrap(), None);
        s.objects[0] = Some(Obj(1, ObjectKind::Other));
        assert_eq!(s.object_index_for_parameter_index(2).unwrap(), None);
        s.objects[0] = None;
        assert_eq!(s.object_index_for_parameter_index(2).unwrap(), None);
        s.objects[0] = Some(Obj(1, ObjectKind::Volume));
        assert!(matches!(
            s.parameter_interval(0),
            Err(SolidError::Unported { .. })
        ));
    }

    #[test]
    fn a_curve_handle_without_its_interval_is_invalid_input() {
        // every library path resolves a curve handle's interval (see
        // `curve_natural_interval`): one that cannot is a caller error, not a deferral
        let mut s = single(0.0, &[0.5]);
        s.num_objects = 1;
        s.objects[0] = Some(Obj(1, ObjectKind::Curve));
        assert!(matches!(
            s.parameter_interval(0),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn array_editing() {
        let mut a: SolutionArray<Obj> = SolutionArray::with_capacity(8);
        assert!(a.is_empty());
        for v in 0..4 {
            assert_eq!(a.add(&single(f64::from(v), &[])), v as usize);
        }
        a.insert_at(1, &single(9.0, &[]), 2);
        let vals = |a: &SolutionArray<Obj>| {
            a.as_slice()
                .iter()
                .map(|s| s.start.solution_value)
                .collect::<Vec<_>>()
        };
        assert_eq!(vals(&a), [0.0, 9.0, 9.0, 1.0, 2.0, 3.0]);
        a.remove_at(1, 2);
        assert_eq!(vals(&a), [0.0, 1.0, 2.0, 3.0]);
        a.remove_at(3, 2); // past the end: nothing moves, size drops by 2
        assert_eq!(vals(&a), [0.0, 1.0]);
        a.insert_at(4, &single(5.0, &[]), 1);
        assert_eq!(vals(&a), [0.0, 1.0, 0.0, 0.0, 5.0]);
        assert_eq!(a.get(2).unwrap().solution_type, SolutionType::NodesOnly);
        a.swap(0, 4).unwrap();
        assert!(a.swap(0, 5).is_err());
        assert_eq!(a.get_at(0).unwrap().start.solution_value, 5.0);
        a.set_at(1, &single(7.0, &[])).unwrap();
        assert!(a.set_at(5, &single(7.0, &[])).is_err() && a.get_at(5).is_err());
        let mut b = SolutionArray::new();
        b.add(&single(8.0, &[]));
        assert_eq!(a.append(&b), 5);
        assert_eq!(vals(&a), [5.0, 7.0, 0.0, 0.0, 0.0, 8.0]);
        a.set_size(2);
        assert_eq!(a.len(), 2);
        a.get_mut(0).unwrap().start.solution_value = -1.0;
        a.reset();
        assert!(a.is_empty() && a.get(0).is_none());
        a.remove_at(0, 3);
        assert!(a.is_empty());
    }

    /// Fuzz regression: caller-chosen sizes, indices and counts that cannot be
    /// allocated return `InvalidInput` instead of aborting, leaving the array as is.
    #[test]
    fn huge_sizes_fail_softly() {
        let mut a: SolutionArray<Obj> = SolutionArray::new();
        a.add(&single(1.0, &[]));
        a.add(&single(2.0, &[]));
        let keep = a.clone();
        // Byte counts past isize::MAX, so the reservation fails on every target; sizes
        // the OS merely cannot back (2^33 solutions, ~2.5 TB) are granted lazily on
        // macOS and the fill then commits them.
        assert!(a.try_set_size(1 << 58).is_err());
        assert!(a.try_set_size(usize::MAX).is_err());
        assert!(a
            .try_insert_at(1 << 57, &single(3.0, &[]), 1 << 57)
            .is_err());
        assert!(a.try_insert_at(usize::MAX, &single(3.0, &[]), 1).is_err());
        assert!(a.try_insert_at(1, &single(3.0, &[]), usize::MAX).is_err());
        assert!(SolutionArray::<Obj>::try_with_capacity(usize::MAX).is_err());
        assert_eq!(a, keep);
        a.try_set_size(3).unwrap();
        a.try_insert_at(1, &single(9.0, &[]), 2).unwrap();
        assert_eq!(a.len(), 5);
        assert_eq!(a.get(1).unwrap().start.solution_value, 9.0);
        assert_eq!(a.get(4).unwrap().solution_type, SolutionType::NodesOnly);
    }

    #[test]
    fn sorted_insertion() {
        let mut a: SolutionArray<Obj> = SolutionArray::new();
        for (v, want) in [(3.0, 0), (1.0, 0), (2.0, 1), (5.0, 3)] {
            assert_eq!(
                a.add_sorted_solution(&single(v, &[v]), SortKey::BySolutionValue)
                    .unwrap(),
                Some(want)
            );
        }
        // duplicate: rejected, the smaller-magnitude value kept
        assert_eq!(
            a.add_sorted_solution(&single(2.0 + 1e-7, &[2.0]), SortKey::BySolutionValue)
                .unwrap(),
            None
        );
        assert_eq!(
            a.add_sorted_solution(&single(BIG_DOUBLE, &[0.0]), SortKey::BySolutionValue)
                .unwrap(),
            None
        );
        assert_eq!(a.len(), 4);

        let mut b: SolutionArray<Obj> = SolutionArray::new();
        for p in [[1.0, 5.0], [1.0, 2.0], [0.5, 9.0], [1.0, 3.0]] {
            b.add_sorted_solution(&single(0.0, &p), SortKey::ByAllParameters)
                .unwrap();
        }
        let firsts: Vec<_> = b
            .as_slice()
            .iter()
            .map(|s| (s.start.parameters[0], s.start.parameters[1]))
            .collect();
        assert_eq!(firsts, [(0.5, 9.0), (1.0, 2.0), (1.0, 3.0), (1.0, 5.0)]);

        let mut c: SolutionArray<Obj> = SolutionArray::new();
        c.add_sorted_solution(&single(1.0, &[2.0]), SortKey::ByValueWithParameter)
            .unwrap();
        assert_eq!(
            c.add_sorted_solution(&single(1.0, &[1.0]), SortKey::ByValueWithParameter)
                .unwrap(),
            Some(0)
        );
        assert_eq!(
            c.add_sorted_solution(&single(0.5, &[9.0]), SortKey::ByValueWithParameter)
                .unwrap(),
            Some(0)
        );
        assert_eq!(
            c.add_sorted_solution(&single(0.0, &[0.0, 1.0]), SortKey::ByFirstTwoParameters)
                .unwrap(),
            Some(0)
        );
        assert_eq!(
            c.add_sorted_solution(&single(0.0, &[99.0]), SortKey::ByFirstParameter)
                .unwrap(),
            Some(4)
        );

        // curves as first objects need their parameter intervals: a handle that reports
        // none is invalid input (no library path names one, see `curve_natural_interval`)
        let mut d = single(0.0, &[0.5]);
        d.num_objects = 1;
        d.objects[0] = Some(Obj(1, ObjectKind::Curve));
        let mut e: SolutionArray<Obj> = SolutionArray::new();
        e.add_sorted_solution(&d, SortKey::BySolutionValue).unwrap();
        assert!(matches!(
            e.add_sorted_solution(&d, SortKey::BySolutionValue),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn equality() {
        let a = range(&[1.0], &[2.0]);
        assert!(a.is_equal(&range(&[1.0], &[2.0])).unwrap());
        assert!(!a.is_equal(&range(&[1.0], &[2.1])).unwrap());
        assert!(!a.is_equal(&single(0.0, &[1.0])).unwrap());
        let mut b = range(&[1.0], &[2.0]);
        b.num_objects = 1;
        b.objects[0] = Some(Obj(1, ObjectKind::PolyVertex));
        assert!(!a.is_equal(&b).unwrap());
        let mut c = b;
        assert!(b.is_equal(&c).unwrap());
        c.objects[0] = Some(Obj(2, ObjectKind::PolyVertex));
        assert!(!b.is_equal(&c).unwrap());
        let mut r = a;
        r.reset();
        assert_eq!(r.solution_type, SolutionType::Unknown);
        assert_eq!(r.start.parameters[0], 1.0);
    }
}
