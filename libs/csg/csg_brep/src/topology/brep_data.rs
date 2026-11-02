//! Serialized B-rep data (`SmBrepData` and its record classes): a B-rep flattened to
//! index-linked record arrays plus its curves and surfaces, and reading it from an
//! SMLib database stream (`.smb`, ASCII or binary).
//!
//! Upstream: `source/SMLib/src/SmBrepData.cpp`, `source/SMLib/inc/SmBrepData.h`.
//!
//! | Rust                                  | Upstream                                         |
//! |---------------------------------------|--------------------------------------------------|
//! | [`BrepData`]                          | `SmBrepData`                                     |
//! | [`BrepData::new`]                     | `SmBrepData::SmBrepData`                         |
//! | `Drop` (implicit)                     | `SmBrepData::~SmBrepData`                        |
//! | [`BrepData::healer_version`]          | `SmBrepData::GetHealerVersion`                   |
//! | [`BrepData::set_healer_version`]      | `SmBrepData::SetHealerVersion`                   |
//! | [`BrepData::read_from_db`]            | `SmBrepData::ReadFromDB`                         |
//! | [`BrepData::read_from_file`]          | `SmBrepData::ReadFromFile`                       |
//! | [`read_curve_from_db`]                | `SmBrepData::ReadCurveFromDB`                    |
//! | [`read_surface_from_db`]              | `SmBrepData::ReadSurfaceFromDB`                  |
//! | [`TopologyData`] / [`TopologyData::read_from_db`] | `SmTopologyData` / `SmTopologyData::ReadFromDB` |
//! | [`RegionData`] / [`RegionData::read_from_db`]     | `SmRegionData` / `SmRegionData::ReadFromDB`     |
//! | [`ShellData`] / [`ShellData::read_from_db`]       | `SmShellData` / `SmShellData::ReadFromDB`       |
//! | [`FaceuseData`] / [`FaceuseData::read_from_db`]   | `SmFaceuseData` / `SmFaceuseData::ReadFromDB`   |
//! | [`FaceData`] / [`FaceData::read_from_db`]         | `SmFaceData` / `SmFaceData::ReadFromDB`         |
//! | [`LoopData`] / [`LoopData::read_from_db`]         | `SmLoopData` / `SmLoopData::ReadFromDB`         |
//! | [`EuData`] / [`EuData::read_from_db`]             | `SmEUData` / `SmEUData::ReadFromDB`             |
//! | [`EdgeData`] / [`EdgeData::read_from_db`]         | `SmEdgeData` / `SmEdgeData::ReadFromDB`         |
//! | [`VertexData`] / [`VertexData::read_from_db`]     | `SmVertexData` / `SmVertexData::ReadFromDB`     |
//! | [`DataSourceType`]                    | `SmDataSourceType`                               |
//! | [`NO_OBJECT`]                         | `SM_NO_OBJECT`                                   |
//!
//! Deviations:
//! - **Out-params -> return values.** The readers return the new object instead of
//!   filling `rpNew*`. Upstream `rAllAttributes` plus each geometry object's attribute
//!   list are an [`AttributeSink`]: the [`AttributeStore`] owning the attributes, the
//!   index table, and a counter that hands each curve and surface read an
//!   [`ObjectKey`] (upstream: the object's address) under which its attributes are
//!   listed. The key is kept beside the object ([`Keyed`]). A counter that reaches
//!   `u64::MAX` fails with [`SolidError::InvalidInput`] instead of wrapping onto keys
//!   already handed out (upstream addresses never repeat).
//! - **Tolerance model.** The reference build uses `SM_USE_OLDTOL`: the zone
//!   tolerances are read for `version <= 43` and the `SM_USE_NEWTOL` lines (model size
//!   estimate, small-topology flags) are compiled out, so they are never read. The
//!   strong tolerance types are plain `f64` (read raw, no `fabs`).
//! - **Record arrays.** `SmTArray::SetSize` zero-fills new records without running
//!   their constructors, so a record starts with every field `0` (not the header's
//!   member defaults, which [`Default`] gives); the port starts each record read from
//!   a stream the same way ([`RegionData::zeroed`], ...). Records are appended one by
//!   one as they are read instead of preallocated, and a count larger than the
//!   remaining input is [`SolidError::InvalidInput`] before the first record
//!   ([`DatabaseReader::ensure_available`] with one byte per record: every record of
//!   a complete stream consumes at least one byte). That bound alone does not cap
//!   memory: upstream ignores each record's read status, so once the stream has
//!   failed or hit end of data every further record consumes no input and is still
//!   appended zero-filled, and a crafted count close to the remaining byte count would
//!   allocate `count * size_of::<record>()`. The port therefore also checks the stream
//!   before each record (and each composite item or segment) and returns
//!   [`SolidError::InvalidInput`] once it is no longer good. The check runs before a
//!   record, never after the last one, so a complete stream whose final record ends
//!   exactly at end of data still reads fully. Upstream would loop over the count on
//!   a failed stream; such inputs read uninitialised locals further on.
//! - **Statuses.** Upstream ignores the status of every record read and of
//!   `ReadIndexedAttributesFromDB`. Records can only fail on a short binary read or a
//!   missing stream, which the port treats as fatal errors (upstream: undefined
//!   values); an attribute line skip that fails keeps the indices read
//!   ([`AttributeStore::read_indexed_attributes_from_db_partial`]) and is ignored.
//! - **Uninitialised locals.** Where upstream extracts into an uninitialised local
//!   (counts, `dMin`/`dMax`, composite segment values) and the extraction stores
//!   nothing, the port uses `0`; such streams are malformed and never reach the C++
//!   reference in the difftest. An ASCII type header that stores nothing is
//!   [`SolidError::InvalidInput`].
//! - **Geometry ownership.** `m_bIsGeomBorrowed` is kept, but the port always owns its
//!   curves and surfaces (they are moved, not borrowed, by future `FromBrep` ports).
//! - **Files.** [`BrepData::read_from_file`] takes the file's bytes (no filesystem on
//!   wasm32); a file that cannot be opened is the caller's error. `ReadFromFile`'s
//!   `char sHeader[8]` words are capped at 7 bytes, as libstdc++ bounds `>> char*`
//!   by the buffer size in the optimised reference build.
//! - **Recursion.** Composite curves of databases up to version 32 nest through
//!   [`read_curve_from_db`]; the depth is bounded by
//!   [`DatabaseReader::enter_nested`] ([`crate::topology::database_io::MAX_READ_NESTING`]).

use crate::curve::composite_curve::CompositeCurve;
use crate::curve::curve::p2::{BSPLINE_CURVE_TYPE, COMPOSITE_CURVE_TYPE};
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::attribute::{AttributeHandle, AttributeStore, ObjectKey};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::surface::offset_surface::{OffsetSurface, SS_UNKNOWN};
use crate::surface::surface::p2::{B_SPLINE_SURFACE_TYPE, OFFSET_SURFACE_TYPE};
use crate::surface::surface::Surface;
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

/// `SM_NO_OBJECT`: index stored in a record for a null topology pointer (e.g. an edge
/// use without UV trim curve).
pub const NO_OBJECT: u64 = 9_999_999;

/// `SM_UNDEF_ULONG`: the "not set" value of the record index fields.
pub const UNDEF_ULONG: u64 = 123_456_789;

/// `UNSURE`: the "not set" value of an `SmBoolean`.
pub const UNSURE: i32 = 2;

/// `SM_ZONE_TOL_3D`.
const ZONE_TOL_3D: f64 = 1.0e-5;
/// `SM_APPROX_TOL_3D` (`SM_ZONE_TOL_3D / 2`).
const APPROX_TOL_3D: f64 = ZONE_TOL_3D / 2.0;
/// `SM_MODEL_SIZE_ESTIMATE`.
const MODEL_SIZE_ESTIMATE: f64 = 50.0;
/// `SM_LARGE_SMALL_SIZE_RATIO`.
const LARGE_SMALL_SIZE_RATIO: f64 = 1000.0;
/// `sizeof(char sHeader[8])` in `ReadFromFile`.
const HEADER_WORD: usize = 8;

const OP_READ: &str = "topology::brep_data::BrepData::read_from_db";
const OP_CURVE: &str = "topology::brep_data::read_curve_from_db";
const OP_SURFACE: &str = "topology::brep_data::read_surface_from_db";
const OP_STREAM: &str = "topology::brep_data::stream";
const OP_KEY: &str = "topology::brep_data::new_key";

/// Where a B-rep's data came from (upstream `SmDataSourceType`); the discriminant is
/// the upstream value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataSourceType {
    /// `SM_DS_UNKNOWN`.
    #[default]
    Unknown = 0,
    /// `SM_DS_SMLIB`.
    SmLib = 1,
    /// `SM_DS_IGES`.
    Iges = 2,
    /// `SM_DS_STEP`.
    Step = 3,
    /// `SM_DS_STL`.
    Stl = 4,
    /// `SM_DS_SAT`.
    Sat = 5,
    /// `SM_DS` tag 6.
    Xt = 6,
    /// `SM_DS_VDAFS`.
    Vdafs = 7,
    /// `SM_DS_OPENNURBS`.
    OpenNurbs = 8,
    /// `SM_DS_DATAKIT`.
    Datakit = 9,
    /// `SM_DS` tag 10.
    Sld = 10,
    /// `SM_DS_USD`.
    Usd = 11,
    /// `SM_DS_PRC`.
    Prc = 12,
    /// `SM_DS_DGN`.
    Dgn = 13,
    /// `SM_DS_ALIAS`.
    Alias = 14,
}

impl DataSourceType {
    /// The upstream `SmDataSourceType` value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The data source for an upstream `SmDataSourceType` value.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::Unknown,
            1 => Self::SmLib,
            2 => Self::Iges,
            3 => Self::Step,
            4 => Self::Stl,
            5 => Self::Sat,
            6 => Self::Xt,
            7 => Self::Vdafs,
            8 => Self::OpenNurbs,
            9 => Self::Datakit,
            10 => Self::Sld,
            11 => Self::Usd,
            12 => Self::Prc,
            13 => Self::Dgn,
            14 => Self::Alias,
            _ => return None,
        })
    }
}

/// A geometry object read from a stream with the key its attributes are listed under
/// in the [`AttributeStore`] (upstream identifies the object by its address).
#[derive(Debug, Clone)]
pub struct Keyed<T> {
    /// Identity of the object in the attribute store.
    pub key: ObjectKey,
    /// The object.
    pub object: T,
}

/// The attribute side of a read (upstream `rAllAttributes` and the attribute lists of
/// the objects built): the store owning the attributes, the table of attributes by
/// file index (placeholders created as indices are seen) and the next object key.
#[derive(Debug)]
pub struct AttributeSink<'a> {
    /// Owner of the attributes and of every object's attribute list.
    pub store: &'a mut AttributeStore,
    /// Upstream `rAllAttributes`: attribute handle per file index (`None` = `NULL`).
    pub all: &'a mut Vec<Option<AttributeHandle>>,
    next_key: u64,
}

impl<'a> AttributeSink<'a> {
    /// A sink adding to `store` and `all`; geometry read gets the keys `first_key`,
    /// `first_key + 1`, ... in the order the objects are started.
    pub fn new(
        store: &'a mut AttributeStore,
        all: &'a mut Vec<Option<AttributeHandle>>,
        first_key: u64,
    ) -> Self {
        Self {
            store,
            all,
            next_key: first_key,
        }
    }

    /// The key the next geometry object will get.
    #[must_use]
    pub const fn next_key(&self) -> u64 {
        self.next_key
    }

    /// The key the next geometry object gets, advancing the counter.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] once the key domain is exhausted (`self.next_key`
    /// is `u64::MAX`, so no higher key could follow this one); the counter is then
    /// left unchanged and no key is handed out. Upstream identifies the objects by
    /// their address and has no such bound.
    fn new_key(&mut self) -> Result<ObjectKey> {
        let key = ObjectKey(self.next_key);
        self.next_key = self
            .next_key
            .checked_add(1)
            .ok_or(SolidError::invalid_input(OP_KEY, "object keys exhausted"))?;
        Ok(key)
    }

    /// `SmAttributeData::ReadIndexedAttributesFromDB` with its status ignored, as every
    /// caller in `SmBrepData.cpp` does.
    fn read_indices(&mut self, db: &mut DatabaseReader) -> Result<Vec<u64>> {
        Ok(self
            .store
            .read_indexed_attributes_from_db_partial(self.all, db)?
            .indices)
    }
}

// ---- stream helpers ---------------------------------------------------------------------

fn is_ascii(db: &DatabaseReader) -> bool {
    db.file_type() == FileType::Ascii
}

/// `*rDB.GetInStreamPtr()` (upstream dereferences `NULL` without a stream).
fn stream(db: &mut DatabaseReader) -> Result<&mut InputStream> {
    db.in_stream_mut()
        .ok_or_else(|| SolidError::null_pointer(OP_STREAM))
}

/// `rFileIn >> ulong`: stores only when the extraction stores.
fn ext_ulong(s: &mut InputStream, slot: &mut u64) {
    if let Some(v) = s.extract_ulong() {
        *slot = v;
    }
}

/// `rFileIn >> long`.
fn ext_long(s: &mut InputStream, slot: &mut i64) {
    if let Some(v) = s.extract_long() {
        *slot = v;
    }
}

/// `rFileIn >> SmBoolean` (an `int`).
fn ext_int(s: &mut InputStream, slot: &mut i32) {
    if let Some(v) = s.extract_int() {
        *slot = v;
    }
}

/// `rFileIn >> double`.
fn ext_double(s: &mut InputStream, slot: &mut f64) {
    if let Some(v) = s.extract_double() {
        *slot = v;
    }
}

/// `GOTO_NEXT_LINE` (`rFileIn.ignore(MAXSIZE, '\n')`, `MAXSIZE` 256).
fn next_line(db: &mut DatabaseReader) {
    db.go_to_next_line();
}

/// The record count as a loop bound, rejected when the remaining input cannot hold
/// that many records (see the module docs).
fn checked_count(db: &DatabaseReader, count: u64, op: &'static str) -> Result<usize> {
    let n = usize::try_from(count).map_err(|_| SolidError::InvalidInput {
        operation: op,
        reason: "header-declared size exceeds the remaining input",
    })?;
    db.ensure_available(n, 1, op)?;
    Ok(n)
}

/// Rejects the next record of a declared count once the stream has failed or reached
/// its end: from then on every extraction stores nothing and consumes no input, so
/// the remaining records could only be zero-filled copies (see the module docs).
/// Called before each record, never after the last one, so a complete stream whose
/// final record ends exactly at end of data reads fully.
fn ensure_more(s: Option<&InputStream>, op: &'static str) -> Result<()> {
    if s.is_some_and(InputStream::good) {
        Ok(())
    } else {
        Err(SolidError::InvalidInput {
            operation: op,
            reason: "stream ended before all declared records were read",
        })
    }
}

// ---- records ----------------------------------------------------------------------------

/// Serialized equivalent of `SmTopology` (upstream `SmTopologyData`): the fields every
/// record carries.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyData {
    /// `m_sAttributes`: indices of the record's attributes in the attribute table.
    pub attributes: Vec<u64>,
    /// `m_lFlags`: user bit array (persistent).
    pub flags: u64,
    /// `m_lUserIndex1` (persistent).
    pub user_index1: u64,
    /// `m_lUserIndex2` (persistent).
    pub user_index2: u64,
    /// `m_bIsSmallTopology` (never read: `SM_USE_NEWTOL` only).
    pub is_small_topology: i32,
}

impl Default for TopologyData {
    /// `SmTopologyData()`.
    fn default() -> Self {
        Self {
            attributes: Vec::new(),
            flags: UNDEF_ULONG,
            user_index1: UNDEF_ULONG,
            user_index2: UNDEF_ULONG,
            is_small_topology: 0,
        }
    }
}

impl TopologyData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            attributes: Vec::new(),
            flags: 0,
            user_index1: 0,
            user_index2: 0,
            is_small_topology: 0,
        }
    }

    /// `SmTopologyData::ReadFromDB`: for `version > 31` the flags and the two user
    /// indices, otherwise all three set to `0`.
    ///
    /// # Errors
    ///
    /// A short binary stream or a missing stream.
    pub fn read_from_db(&mut self, version: u64, db: &mut DatabaseReader) -> Result<()> {
        if version > 31 {
            if is_ascii(db) {
                let s = stream(db)?;
                ext_ulong(s, &mut self.flags);
                ext_ulong(s, &mut self.user_index1);
                ext_ulong(s, &mut self.user_index2);
            } else {
                self.flags = db.read_ulong()?;
                self.user_index1 = db.read_ulong()?;
                self.user_index2 = db.read_ulong()?;
            }
            // version > 43: m_bIsSmallTopology is read only under SM_USE_NEWTOL
        } else {
            self.flags = 0;
            self.user_index1 = 0;
            self.user_index2 = 0;
        }
        Ok(())
    }
}

/// Serialized `SmRegion` (upstream `SmRegionData`).
#[derive(Debug, Clone, PartialEq)]
pub struct RegionData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_lStartShell`: index of the region's first (outer) shell.
    pub start_shell: u64,
    /// `m_lNumShells`: number of shells.
    pub num_shells: u64,
    /// `m_bIsVoidFlag`.
    pub is_void: i32,
}

impl Default for RegionData {
    /// `SmRegionData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            start_shell: UNDEF_ULONG,
            num_shells: 0,
            is_void: UNSURE,
        }
    }
}

impl RegionData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            start_shell: 0,
            num_shells: 0,
            is_void: 0,
        }
    }

    /// `SmRegionData::ReadFromDB`: index (ASCII), common fields, shell count, first
    /// shell, void flag, then (`version > 30` and `persist_attribs`) the attribute
    /// indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_ulong(s, &mut self.num_shells);
            ext_ulong(s, &mut self.start_shell);
            ext_int(s, &mut self.is_void);
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            self.num_shells = db.read_ulong()?;
            self.start_shell = db.read_ulong()?;
            self.is_void = db.read_boolean()?;
        }
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized `SmShell` (upstream `SmShellData`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_lShellType`: 0 faceuses, 1 wire edge, 2 vertex.
    pub shell_type: u64,
    /// `m_lVertex`: the vertex of a vertex shell.
    pub vertex: u64,
    /// `m_lEdge`: the first edge of a wire-edge shell.
    pub edge: u64,
    /// `m_lFaceuseStart`: the first faceuse of a faceuse shell.
    pub faceuse_start: u64,
    /// `m_lNumFaceuses`.
    pub num_faceuses: u64,
}

impl Default for ShellData {
    /// `SmShellData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            shell_type: UNDEF_ULONG,
            vertex: UNDEF_ULONG,
            edge: UNDEF_ULONG,
            faceuse_start: UNDEF_ULONG,
            num_faceuses: 0,
        }
    }
}

impl ShellData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            shell_type: 0,
            vertex: 0,
            edge: 0,
            faceuse_start: 0,
            num_faceuses: 0,
        }
    }

    /// `SmShellData::ReadFromDB`: index (ASCII), common fields, shell type and the
    /// type's fields (the obsolete orientation flag of faceuse shells is read and
    /// dropped; an unknown type reads nothing more), then the attribute indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        // used to remove m_bOrientation - kept to keep the smb file structure constant
        let mut orientation: i32 = 1;
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_ulong(s, &mut self.shell_type);
            match self.shell_type {
                0 => {
                    ext_int(s, &mut orientation);
                    ext_ulong(s, &mut self.num_faceuses);
                    ext_ulong(s, &mut self.faceuse_start);
                }
                1 => ext_ulong(s, &mut self.edge),
                2 => ext_ulong(s, &mut self.vertex),
                _ => {}
            }
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            self.shell_type = db.read_ulong()?;
            match self.shell_type {
                0 => {
                    self.num_faceuses = db.read_ulong()?;
                    self.faceuse_start = db.read_ulong()?;
                    orientation = db.read_boolean()?;
                }
                1 => self.edge = db.read_ulong()?,
                2 => self.vertex = db.read_ulong()?,
                _ => {}
            }
        }
        let _ = orientation;
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized `SmFaceuse` (upstream `SmFaceuseData`).
#[derive(Debug, Clone, PartialEq)]
pub struct FaceuseData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_lFace`: the owning face.
    pub face: u64,
    /// `m_bOrientation`: TRUE when the faceuse has the surface's orientation.
    pub orientation: i32,
}

impl Default for FaceuseData {
    /// `SmFaceuseData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            face: UNDEF_ULONG,
            orientation: UNSURE,
        }
    }
}

impl FaceuseData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            face: 0,
            orientation: 0,
        }
    }

    /// `SmFaceuseData::ReadFromDB`: index (ASCII), common fields, face, orientation,
    /// then the attribute indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_ulong(s, &mut self.face);
            ext_int(s, &mut self.orientation);
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            self.face = db.read_ulong()?;
            self.orientation = db.read_boolean()?;
        }
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized `SmFace` (upstream `SmFaceData`).
#[derive(Debug, Clone, PartialEq)]
pub struct FaceData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_sZoneTol3d` (`SM_USE_OLDTOL`).
    pub zone_tol_3d: f64,
    /// `m_lSurface`: the face's surface.
    pub surface: u64,
    /// `m_lStartLoop`: the first (outer) loop.
    pub start_loop: u64,
    /// `m_lNumLoops`.
    pub num_loops: u64,
    /// `m_vUVDomain`: the face's domain on its surface.
    pub uv_domain: Extent2d,
    /// `m_bRectangularTrim`.
    pub rectangular_trim: i32,
}

impl Default for FaceData {
    /// `SmFaceData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            zone_tol_3d: 0.0,
            surface: UNDEF_ULONG,
            start_loop: UNDEF_ULONG,
            num_loops: 0,
            uv_domain: Extent2d::new(),
            rectangular_trim: UNSURE,
        }
    }
}

impl FaceData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            zone_tol_3d: 0.0,
            surface: 0,
            start_loop: 0,
            num_loops: 0,
            uv_domain: Extent2d::from_bounds(0.0, 0.0, 0.0, 0.0),
            rectangular_trim: 0,
        }
    }

    /// `SmFaceData::ReadFromDB`: index (ASCII), common fields, zone tolerance,
    /// surface, first loop, loop count, rectangular-trim flag and the UV domain (in
    /// binary the domain comes first), then the attribute indices. The domain is set
    /// with `SetMinMax`, whose failure (min > max) leaves it unchanged.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        let mut min = Point2::UNDEF;
        let mut max = Point2::UNDEF;
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_double(s, &mut self.zone_tol_3d);
            ext_ulong(s, &mut self.surface);
            ext_ulong(s, &mut self.start_loop);
            ext_ulong(s, &mut self.num_loops);
            ext_int(s, &mut self.rectangular_trim);
            next_line(db);
            let s = stream(db)?;
            ext_double(s, &mut min.x);
            ext_double(s, &mut max.x);
            ext_double(s, &mut min.y);
            ext_double(s, &mut max.y);
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            min.x = db.read_double()?;
            max.x = db.read_double()?;
            min.y = db.read_double()?;
            max.y = db.read_double()?;
            self.zone_tol_3d = db.read_double()?;
            self.surface = db.read_ulong()?;
            self.start_loop = db.read_ulong()?;
            self.num_loops = db.read_ulong()?;
            self.rectangular_trim = db.read_boolean()?;
        }
        // status ignored: min > max leaves the domain unchanged
        let _ = self.uv_domain.set_min_max(min, max);
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized `SmLoop` (upstream `SmLoopData`).
#[derive(Debug, Clone, PartialEq)]
pub struct LoopData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_lLoopType`: 0 edge-use loop, 1 vertex loop.
    pub loop_type: u64,
    /// `m_lVertex`: the vertex of a vertex loop.
    pub vertex: u64,
    /// `m_lStartEU`: the first edge use of an edge-use loop.
    pub start_eu: u64,
    /// `m_lNumEU`: number of edge uses.
    pub num_eu: u64,
    /// `m_sLUUpperAttributes`: attribute indices of the upper loop use.
    pub lu_upper_attributes: Vec<u64>,
    /// `m_sLULowerAttributes`: attribute indices of the lower loop use.
    pub lu_lower_attributes: Vec<u64>,
}

impl Default for LoopData {
    /// `SmLoopData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            loop_type: UNDEF_ULONG,
            vertex: UNDEF_ULONG,
            start_eu: UNDEF_ULONG,
            num_eu: 0,
            lu_upper_attributes: Vec::new(),
            lu_lower_attributes: Vec::new(),
        }
    }
}

impl LoopData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            loop_type: 0,
            vertex: 0,
            start_eu: 0,
            num_eu: 0,
            lu_upper_attributes: Vec::new(),
            lu_lower_attributes: Vec::new(),
        }
    }

    /// `SmLoopData::ReadFromDB`: index (ASCII), common fields, loop type and its
    /// fields, then the loop's, upper and lower loop use attribute indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_ulong(s, &mut self.loop_type);
            match self.loop_type {
                0 => {
                    ext_ulong(s, &mut self.num_eu);
                    ext_ulong(s, &mut self.start_eu);
                }
                1 => ext_ulong(s, &mut self.vertex),
                _ => {}
            }
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            self.loop_type = db.read_ulong()?;
            match self.loop_type {
                0 => {
                    self.num_eu = db.read_ulong()?;
                    self.start_eu = db.read_ulong()?;
                }
                1 => self.vertex = db.read_ulong()?,
                _ => {}
            }
        }
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
            self.lu_upper_attributes = attrs.read_indices(db)?;
            self.lu_lower_attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized pair of mated `SmEdgeuse`s (upstream `SmEUData`).
#[derive(Debug, Clone, PartialEq)]
pub struct EuData {
    /// Common record fields (of the primary edge use).
    pub topology: TopologyData,
    /// `m_lEUType`: 0 face edge, 1 loop vertex, 2 wire edge, 3 vertex at a pole.
    pub eu_type: u64,
    /// `m_lEdge` (types 0 and 2).
    pub edge: u64,
    /// `m_lVertex` (types 1 and 3).
    pub vertex: u64,
    /// `m_lShell` (type 2).
    pub shell: u64,
    /// `m_lLoop` (types 0, 1, 3).
    pub loop_: u64,
    /// `m_bOrientation`: the primary use's orientation relative to the edge.
    pub orientation: i32,
    /// `m_lUVCurve`: the use's UV trim curve ([`NO_OBJECT`] for none).
    pub uv_curve: u64,
    /// `m_lNextEU`: next radial edge use (negative `-(i+1)`: the mate of record `i`).
    pub next_eu: i64,
    /// `m_lMateNextEU`: the mate's next radial edge use.
    pub mate_next_eu: i64,
    /// `m_bDownwardLU` (not persistent).
    pub downward_lu: bool,
    /// `m_lMateFlags`.
    pub mate_flags: u64,
    /// `m_lMateUserIndex1`.
    pub mate_user_index1: u64,
    /// `m_lMateUserIndex2`.
    pub mate_user_index2: u64,
    /// `m_sMateAttributes`: attribute indices of the mate.
    pub mate_attributes: Vec<u64>,
}

impl Default for EuData {
    /// `SmEUData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            eu_type: UNDEF_ULONG,
            edge: UNDEF_ULONG,
            vertex: UNDEF_ULONG,
            shell: UNDEF_ULONG,
            loop_: UNDEF_ULONG,
            orientation: UNSURE,
            uv_curve: UNDEF_ULONG,
            next_eu: UNDEF_ULONG as i64,
            mate_next_eu: UNDEF_ULONG as i64,
            downward_lu: false,
            mate_flags: 0,
            mate_user_index1: UNDEF_ULONG,
            mate_user_index2: UNDEF_ULONG,
            mate_attributes: Vec::new(),
        }
    }
}

impl EuData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            eu_type: 0,
            edge: 0,
            vertex: 0,
            shell: 0,
            loop_: 0,
            orientation: 0,
            uv_curve: 0,
            next_eu: 0,
            mate_next_eu: 0,
            downward_lu: false,
            mate_flags: 0,
            mate_user_index1: 0,
            mate_user_index2: 0,
            mate_attributes: Vec::new(),
        }
    }

    /// `SmEUData::ReadFromDB`: index (ASCII), common fields, the mate's flags and user
    /// indices (`version > 31`, else `0`), the use type and its fields, orientation,
    /// UV curve and (type 0) the two radial links, then the use's and the mate's
    /// attribute indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        let ascii = is_ascii(db);
        if ascii {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
        }

        self.topology.read_from_db(version, db)?;

        if version > 31 {
            if ascii {
                let s = stream(db)?;
                ext_ulong(s, &mut self.mate_flags);
                ext_ulong(s, &mut self.mate_user_index1);
                ext_ulong(s, &mut self.mate_user_index2);
            } else {
                self.mate_flags = db.read_ulong()?;
                self.mate_user_index1 = db.read_ulong()?;
                self.mate_user_index2 = db.read_ulong()?;
            }
        } else {
            self.mate_flags = 0;
            self.mate_user_index1 = 0;
            self.mate_user_index2 = 0;
        }

        if ascii {
            let s = stream(db)?;
            ext_ulong(s, &mut self.eu_type);
            match self.eu_type {
                0 => {
                    ext_ulong(s, &mut self.edge);
                    ext_ulong(s, &mut self.loop_);
                    ext_int(s, &mut self.orientation);
                    ext_ulong(s, &mut self.uv_curve);
                    ext_long(s, &mut self.next_eu);
                    ext_long(s, &mut self.mate_next_eu);
                }
                1 | 3 => {
                    ext_ulong(s, &mut self.vertex);
                    ext_ulong(s, &mut self.loop_);
                    ext_int(s, &mut self.orientation);
                    ext_ulong(s, &mut self.uv_curve);
                }
                2 => {
                    ext_ulong(s, &mut self.edge);
                    ext_ulong(s, &mut self.shell);
                    ext_int(s, &mut self.orientation);
                    ext_ulong(s, &mut self.uv_curve);
                }
                _ => {}
            }
            next_line(db);
        } else {
            self.eu_type = db.read_ulong()?;
            match self.eu_type {
                0 => {
                    self.edge = db.read_ulong()?;
                    self.loop_ = db.read_ulong()?;
                }
                1 | 3 => {
                    self.vertex = db.read_ulong()?;
                    self.loop_ = db.read_ulong()?;
                }
                2 => {
                    self.edge = db.read_ulong()?;
                    self.shell = db.read_ulong()?;
                }
                _ => {}
            }
            self.orientation = db.read_boolean()?;
            self.uv_curve = db.read_ulong()?;
            if self.eu_type == 0 {
                self.next_eu = db.read_long()?;
                self.mate_next_eu = db.read_long()?;
            }
        }

        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
            self.mate_attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized `SmEdge` (upstream `SmEdgeData`).
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_sZoneTol3d` (`SM_USE_OLDTOL`).
    pub zone_tol_3d: f64,
    /// `m_vInterval`: the edge's interval on its curve.
    pub interval: Extent1d,
    /// `m_lCurve`: the edge's 3D curve.
    pub curve: u64,
    /// `m_lStartVertex`.
    pub start_vertex: u64,
    /// `m_lEndVertex`.
    pub end_vertex: u64,
    /// `m_lPrimEU`: primary edge use (negative `-(i+1)`: the mate of record `i`).
    pub prim_eu: i64,
}

impl Default for EdgeData {
    /// `SmEdgeData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            zone_tol_3d: 0.0,
            interval: Extent1d::new(),
            curve: UNDEF_ULONG,
            start_vertex: UNDEF_ULONG,
            end_vertex: UNDEF_ULONG,
            prim_eu: UNDEF_ULONG as i64,
        }
    }
}

impl EdgeData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            zone_tol_3d: 0.0,
            interval: Extent1d::from_raw(0.0, 0.0),
            curve: 0,
            start_vertex: 0,
            end_vertex: 0,
            prim_eu: 0,
        }
    }

    /// `SmEdgeData::ReadFromDB`: index (ASCII), common fields, zone tolerance, curve,
    /// start and end vertex, primary edge use and the interval (set with `SetMinMax`,
    /// whose failure leaves it unchanged), then the attribute indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        // uninitialised upstream (see the module docs)
        let mut min = 0.0;
        let mut max = 0.0;
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_double(s, &mut self.zone_tol_3d);
            ext_ulong(s, &mut self.curve);
            ext_ulong(s, &mut self.start_vertex);
            ext_ulong(s, &mut self.end_vertex);
            ext_long(s, &mut self.prim_eu);
            ext_double(s, &mut min);
            ext_double(s, &mut max);
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            self.zone_tol_3d = db.read_double()?;
            self.curve = db.read_ulong()?;
            self.start_vertex = db.read_ulong()?;
            self.end_vertex = db.read_ulong()?;
            self.prim_eu = db.read_long()?;
            min = db.read_double()?;
            max = db.read_double()?;
        }
        // status ignored: min > max leaves the interval unchanged
        let _ = self.interval.set_min_max(min, max);
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

/// Serialized `SmVertex` (upstream `SmVertexData`).
#[derive(Debug, Clone, PartialEq)]
pub struct VertexData {
    /// Common record fields.
    pub topology: TopologyData,
    /// `m_sZoneTol3d` (`SM_USE_OLDTOL`).
    pub zone_tol_3d: f64,
    /// `m_vPoint`: the vertex position.
    pub point: Point3,
}

impl Default for VertexData {
    /// `SmVertexData()`.
    fn default() -> Self {
        Self {
            topology: TopologyData::default(),
            zone_tol_3d: 0.0,
            point: Vec3::UNDEF,
        }
    }
}

impl VertexData {
    /// The zero-filled state `SmTArray::SetSize` leaves.
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            topology: TopologyData::zeroed(),
            zone_tol_3d: 0.0,
            point: Vec3::new(0.0, 0.0, 0.0),
        }
    }

    /// `SmVertexData::ReadFromDB`: index (ASCII), common fields, zone tolerance and
    /// the point, then the attribute indices.
    ///
    /// # Errors
    ///
    /// A short binary stream, a missing stream, an oversized attribute list.
    pub fn read_from_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<()> {
        if is_ascii(db) {
            let mut index = 0;
            ext_ulong(stream(db)?, &mut index);
            self.topology.read_from_db(version, db)?;
            let s = stream(db)?;
            ext_double(s, &mut self.zone_tol_3d);
            ext_double(s, &mut self.point.x);
            ext_double(s, &mut self.point.y);
            ext_double(s, &mut self.point.z);
            next_line(db);
        } else {
            self.topology.read_from_db(version, db)?;
            self.zone_tol_3d = db.read_double()?;
            self.point.x = db.read_double()?;
            self.point.y = db.read_double()?;
            self.point.z = db.read_double()?;
        }
        if version > 30 && persist_attribs {
            self.topology.attributes = attrs.read_indices(db)?;
        }
        Ok(())
    }
}

// ---- the B-rep data ---------------------------------------------------------------------

/// Serialized `SmBrep` (upstream `SmBrepData`): the B-rep's properties, its topology
/// as index-linked record arrays and its geometry.
#[derive(Debug, Clone)]
pub struct BrepData {
    /// `m_eDataSourceType`.
    pub data_source_type: DataSourceType,
    /// `m_bManifold`: TRUE for a manifold solid whose shells are not duplicated.
    pub manifold: i32,
    /// `m_sZoneTol3d` (`SM_USE_OLDTOL`).
    pub zone_tol_3d: f64,
    /// `m_dThisModelSizeEstimate`.
    pub model_size_estimate: f64,
    /// `m_dThisLargeSmallSizeRatio`.
    pub large_small_size_ratio: f64,
    /// `m_sApproxTol3d`.
    pub approx_tol_3d: f64,
    /// `m_lStartRegion`: index of the first (infinite) region.
    pub start_region: u64,
    /// `m_lNumRegions`.
    pub num_regions: u64,
    /// `m_vColor`.
    pub color: Vec3,
    /// `m_bIsGeomBorrowed`.
    pub is_geom_borrowed: bool,
    /// `m_lHealerVersion`: healer version when the file was written.
    pub healer_version: u64,
    /// `m_vRegions`.
    pub regions: Vec<RegionData>,
    /// `m_vShells`.
    pub shells: Vec<ShellData>,
    /// `m_vCFaces`: `[count, n0, face.., n1, face.., ...]` of the old composite faces.
    pub cfaces: Vec<u64>,
    /// `m_vFaceuses`.
    pub faceuses: Vec<FaceuseData>,
    /// `m_vFaces`.
    pub faces: Vec<FaceData>,
    /// `m_vLoops`.
    pub loops: Vec<LoopData>,
    /// `m_vCEdges`: `[count, n0, edge.., ...]` of the old composite edges.
    pub cedges: Vec<u64>,
    /// `m_vEdgeuses`.
    pub edgeuses: Vec<EuData>,
    /// `m_vEdges`.
    pub edges: Vec<EdgeData>,
    /// `m_vVertices`.
    pub vertices: Vec<VertexData>,
    /// `m_vUVCurves`: the UV trim curves.
    pub uv_curves: Vec<Keyed<Curve>>,
    /// `m_v3DCurves`: the edge curves.
    pub curves_3d: Vec<Keyed<Curve>>,
    /// `m_vSurfaces`: the face surfaces.
    pub surfaces: Vec<Keyed<Surface>>,
    /// `m_vAttributes`: indices of the B-rep's attributes.
    pub attributes: Vec<u64>,
}

impl Default for BrepData {
    fn default() -> Self {
        Self::new(0, DataSourceType::Unknown)
    }
}

impl BrepData {
    /// An empty B-rep data object (`SmBrepData(bManifold, eDSType)`; upstream
    /// defaults: `FALSE`, [`DataSourceType::Unknown`]).
    #[must_use]
    pub fn new(manifold: i32, data_source_type: DataSourceType) -> Self {
        Self {
            data_source_type,
            manifold,
            zone_tol_3d: ZONE_TOL_3D,
            model_size_estimate: MODEL_SIZE_ESTIMATE,
            large_small_size_ratio: LARGE_SMALL_SIZE_RATIO,
            approx_tol_3d: APPROX_TOL_3D,
            start_region: 0,
            num_regions: 0,
            color: Vec3::new(0.0, 0.0, 0.0),
            is_geom_borrowed: false,
            healer_version: 0,
            regions: Vec::new(),
            shells: Vec::new(),
            cfaces: Vec::new(),
            faceuses: Vec::new(),
            faces: Vec::new(),
            loops: Vec::new(),
            cedges: Vec::new(),
            edgeuses: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
            uv_curves: Vec::new(),
            curves_3d: Vec::new(),
            surfaces: Vec::new(),
            attributes: Vec::new(),
        }
    }

    /// `GetHealerVersion`.
    #[must_use]
    pub const fn healer_version(&self) -> u64 {
        self.healer_version
    }

    /// `SetHealerVersion`.
    pub fn set_healer_version(&mut self, healer_version: u64) {
        self.healer_version = healer_version;
    }

    /// `SmBrepData::ReadFromDB`: reads a B-rep written with database version `version`
    /// (attributes only when `version > 30` and `persist_attribs`): the properties
    /// (color or attribute indices, zone tolerance for `version <= 43`, manifold
    /// flag, the obsolete composites flag, region count and start), then the region,
    /// shell, composite face, faceuse, face, loop, composite edge, edge use, edge and
    /// vertex sections, the UV curves (dimension 2), the 3D curves (dimension 3) and the
    /// surfaces. ASCII streams have comment lines between the sections, which are
    /// skipped. The new object is built with `SmBrepData(TRUE, SM_DS_SMLIB)`.
    ///
    /// # Errors
    ///
    /// Those of [`read_curve_from_db`] and [`read_surface_from_db`]; a short binary
    /// stream, a missing stream, a count exceeding the remaining input.
    pub fn read_from_db(
        k: &Kernel,
        version: u64,
        attrs: &mut AttributeSink<'_>,
        db: &mut DatabaseReader,
        persist_attribs: bool,
    ) -> Result<Self> {
        // uninitialised upstream (see the module docs)
        let mut count: u64 = 0;
        let mut index: u64 = 0;

        // allocate new BrepData object
        let mut bd = Self::new(1, DataSourceType::SmLib);
        let ascii = is_ascii(db);

        // read Brep properties: color, tol, bManifold, bmakeComposites, numRegions,
        // StartRegion
        if ascii {
            next_line(db); // Brep Starts
            if version > 30 && persist_attribs {
                bd.attributes = attrs.read_indices(db)?;
            } else {
                let s = stream(db)?;
                ext_double(s, &mut bd.color.x);
                ext_double(s, &mut bd.color.y);
                ext_double(s, &mut bd.color.z);
                next_line(db);
            }
            // tolerance: version > 43 is the NewTol model (compiled out)
            if version <= 43 {
                ext_double(stream(db)?, &mut bd.zone_tol_3d);
            }
            next_line(db);
            // manifold flag
            ext_int(stream(db)?, &mut bd.manifold);
            next_line(db);
            // obsolete makeComposites flag
            let mut make_composites: i32 = 0;
            ext_int(stream(db)?, &mut make_composites);
            next_line(db);
            next_line(db); // blank line
            next_line(db); // Region Data
            ext_ulong(stream(db)?, &mut bd.num_regions);
            next_line(db);
            ext_ulong(stream(db)?, &mut bd.start_region);
            next_line(db);
            next_line(db); // column comment
        } else {
            if version > 30 && persist_attribs {
                bd.attributes = attrs.read_indices(db)?;
            } else {
                let c = db.read_doubles(3)?;
                bd.color = Vec3::new(
                    c.first().copied().unwrap_or(0.0),
                    c.get(1).copied().unwrap_or(0.0),
                    c.get(2).copied().unwrap_or(0.0),
                );
            }
            if version <= 43 {
                bd.zone_tol_3d = db.read_double()?;
            }
            bd.manifold = db.read_boolean()?;
            let _make_composites = db.read_boolean()?;
            bd.num_regions = db.read_ulong()?;
            bd.start_region = db.read_ulong()?;
        }

        // Region Data
        let n = checked_count(db, bd.num_regions, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = RegionData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.regions.push(r);
        }

        // Shells
        if ascii {
            next_line(db); // blank line
            next_line(db); // Shell Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            next_line(db); // faceuse shell columns
            next_line(db); // wire edge shell columns
            next_line(db); // vertex shell columns
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = ShellData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.shells.push(r);
        }

        // Composite Faces
        bd.cfaces = read_composite_list(db, &mut count, &mut index)?;

        // Faceuse Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // Faceuse Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            next_line(db); // columns
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = FaceuseData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.faceuses.push(r);
        }

        // Face Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // Face Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            next_line(db); // columns
            next_line(db); // domain columns
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = FaceData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.faces.push(r);
        }

        // Loop Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // Loop Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            next_line(db); // edge use loop columns
            next_line(db); // vertex loop columns
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = LoopData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.loops.push(r);
        }

        // Composite Edges
        bd.cedges = read_composite_list(db, &mut count, &mut index)?;

        // Edgeuse Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // Edgeuse Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            for _ in 0..4 {
                next_line(db); // columns of the four edge use types
            }
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = EuData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.edgeuses.push(r);
        }

        // Edge Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // Edge Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            next_line(db); // columns
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = EdgeData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.edges.push(r);
        }

        // Vertex Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // Vertex Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
            next_line(db); // columns
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            let mut r = VertexData::zeroed();
            r.read_from_db(version, attrs, db, persist_attribs)?;
            bd.vertices.push(r);
        }

        // UV-Curve Data
        if ascii {
            next_line(db); // blank line
            next_line(db); // UV-Curve Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            if ascii {
                ext_ulong(stream(db)?, &mut index);
                next_line(db);
            }
            let c = read_curve_from_db(k, 2, version, attrs, db, persist_attribs)?;
            bd.uv_curves.push(c);
        }

        // 3D-Curve Data
        if ascii {
            next_line(db);
            next_line(db); // 3D-Curve Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            if ascii {
                ext_ulong(stream(db)?, &mut index);
                next_line(db);
            }
            let c = read_curve_from_db(k, 3, version, attrs, db, persist_attribs)?;
            bd.curves_3d.push(c);
        }

        // Surface Data
        if ascii {
            next_line(db);
            next_line(db); // Surface Data
            ext_ulong(stream(db)?, &mut count);
            next_line(db);
        } else {
            count = db.read_ulong()?;
        }
        let n = checked_count(db, count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            if ascii {
                ext_ulong(stream(db)?, &mut index);
                next_line(db);
            }
            let s = read_surface_from_db(k, version, attrs, db, persist_attribs)?;
            bd.surfaces.push(s);
        }

        Ok(bd)
    }

    /// `SmBrepData::ReadFromFile`: reads one B-rep from the contents of a database file
    /// of `file_type`, without attributes. The version comes from the header
    /// (`Version <n>` in ASCII, `'V'` and a long in binary; a file not starting with
    /// `V`, after at most one leading newline, is version 30), and from version 42 on
    /// the healer version (`Healer Version <n>` / `'H'` and a long) follows; it is
    /// stored in the result.
    ///
    /// # Errors
    ///
    /// Those of [`Self::read_from_db`]; a short header.
    pub fn read_from_file(k: &Kernel, data: Vec<u8>, file_type: FileType) -> Result<Self> {
        let mut db = DatabaseReader::new();
        db.open_for_read(data, file_type);

        // virtual preRead for derived classes - base class does nothing
        db.begin_reading();

        let ascii = file_type == FileType::Ascii;
        let first = {
            let s = stream(&mut db)?;
            // (char)rFileIn.get(): EOF becomes (char)-1
            let mut c = s.get().unwrap_or(0xFF);
            if c == 10 {
                c = s.get().unwrap_or(0xFF);
            }
            s.unget(); // peek was not working in VS10
            c
        };

        // uninitialised upstream when the ASCII extraction stores nothing
        let mut version: u64 = 0;
        let mut healer_version: u64 = 0;
        if first != b'V' {
            version = 30;
        } else if ascii {
            let s = stream(&mut db)?;
            let _ = s.extract_word_limited(HEADER_WORD); // "Version"
            ext_ulong(s, &mut version);
            next_line(&mut db);
        } else {
            let _header = db.read_char()?; // 'V'
                                           // status ignored upstream; a short read is an error here
            version = db.read_ulong()?;
            // version >= 40: ignore(sizeof(long long) - sizeof(long)) skips nothing on
            // LP64
        }

        db.set_version(version);

        // SM_CURRENT_DATABASE_VERSION 42 added the healer version to the files.
        if version > 41 {
            if ascii {
                let s = stream(&mut db)?;
                let _ = s.extract_word_limited(HEADER_WORD); // "Healer"
                let _ = s.extract_word_limited(HEADER_WORD); // "Version"
                ext_ulong(s, &mut healer_version);
                next_line(&mut db);
            } else {
                let _header = db.read_char()?; // 'H'
                healer_version = db.read_ulong()?;
            }
        }

        // pass the call along, without attributes
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        let mut bd = Self::read_from_db(k, version, &mut attrs, &mut db, false)?;
        bd.set_healer_version(healer_version);

        // virtual postRead for derived classes - base class does nothing
        db.end_reading();
        Ok(bd)
    }
}

/// A composite face or edge section: the count line and, when nonzero, the column
/// comment and per entry `[index] n item0 .. item(n-1)`, flattened to
/// `[count, n0, items.., n1, items.., ...]` (empty for a zero count).
fn read_composite_list(
    db: &mut DatabaseReader,
    count: &mut u64,
    index: &mut u64,
) -> Result<Vec<u64>> {
    let ascii = is_ascii(db);
    if ascii {
        next_line(db); // blank line
        next_line(db); // section title
        ext_ulong(stream(db)?, count);
        next_line(db);
    } else {
        *count = db.read_ulong()?;
    }
    let mut list = Vec::new();
    if *count != 0 {
        if ascii {
            next_line(db); // columns
        }
        list.push(*count);
        let n = checked_count(db, *count, OP_READ)?;
        for _ in 0..n {
            ensure_more(db.in_stream(), OP_READ)?;
            // composite faces initialise these to 0, composite edges leave them
            // uninitialised (see the module docs)
            let mut item_count: u64 = 0;
            let mut item: u64 = 0;
            if ascii {
                let s = stream(db)?;
                ext_ulong(s, index);
                ext_ulong(s, &mut item_count);
                list.push(item_count);
                let m = checked_count(db, item_count, OP_READ)?;
                let s = stream(db)?;
                for _ in 0..m {
                    ensure_more(Some(s), OP_READ)?;
                    ext_ulong(s, &mut item);
                    list.push(item);
                }
                next_line(db);
            } else {
                item_count = db.read_ulong()?;
                list.push(item_count);
                let m = checked_count(db, item_count, OP_READ)?;
                for _ in 0..m {
                    ensure_more(db.in_stream(), OP_READ)?;
                    item = db.read_ulong()?;
                    list.push(item);
                }
            }
        }
    }
    Ok(list)
}

/// `SmBrepData::ReadCurveFromDB`: reads one curve. For `version > 32` the type
/// header carries the type and dimension and the curve is read by
/// [`Curve::read_from_db`]; up to version 32 only B-spline curves (read with
/// dimension `dim`) and composite curves (segment count, dimension, closed flag, then
/// per segment the sense, two gaps and the segment curve, read recursively) exist.
/// With `version > 30` and `persist_attribs` the curve's attribute indices follow and
/// each indexed attribute is added to the curve.
///
/// # Errors
///
/// `SM_ERR` for another type up to version 32; `SM_ERR_NULL_POINTER` for an
/// attribute index whose table slot is empty; [`SolidError::InvalidInput`] for an
/// index past the table (upstream reads past the array), an ASCII type header that
/// could not be read, or an exhausted [`AttributeSink`] key domain; those of the
/// curve readers.
pub fn read_curve_from_db(
    k: &Kernel,
    dim: u64,
    version: u64,
    attrs: &mut AttributeSink<'_>,
    db: &mut DatabaseReader,
    persist_attribs: bool,
) -> Result<Keyed<Curve>> {
    let key = attrs.new_key()?;
    let bad_type = || SolidError::InvalidInput {
        operation: OP_CURVE,
        reason: "curve type header could not be read",
    };

    // branch on database version number
    let curve = if version > 32 {
        // read curve type
        let header = db.read_type(true)?;
        let ty = header.type_code.ok_or_else(bad_type)?;
        let read_dim = header.dim.ok_or_else(bad_type)?;
        // pass the call to appropriate derived class ReadFromDB call
        Curve::read_from_db(k, ty, db, read_dim, version)?
    } else {
        // database versions 32 and before - limited to BSplineCurves and
        // SmCompositeCurve
        let ty = db.read_type(false)?.type_code.ok_or_else(bad_type)?;
        match ty {
            BSPLINE_CURVE_TYPE => Curve::from(BSplineCurve::read_from_db(db, dim, version)?),
            COMPOSITE_CURVE_TYPE => {
                db.enter_nested(OP_CURVE)?;
                let r = read_old_composite(k, dim, version, attrs, db, persist_attribs);
                db.leave_nested();
                r?
            }
            _ => return Err(SolidError::sm_err(OP_CURVE)),
        }
    };

    // handle attributes
    if version > 30 && persist_attribs {
        let indices = attrs.read_indices(db)?;
        for i in indices {
            let slot = usize::try_from(i)
                .ok()
                .and_then(|i| attrs.all.get(i))
                .ok_or(SolidError::InvalidInput {
                    operation: OP_CURVE,
                    reason: "attribute index past the attribute table",
                })?;
            let h = slot.ok_or_else(|| SolidError::null_pointer(OP_CURVE))?;
            attrs.store.add_attribute(key, h)?;
        }
    }

    Ok(Keyed { key, object: curve })
}

/// The composite curve branch of [`read_curve_from_db`] (versions up to 32).
fn read_old_composite(
    k: &Kernel,
    dim: u64,
    version: u64,
    attrs: &mut AttributeSink<'_>,
    db: &mut DatabaseReader,
    persist_attribs: bool,
) -> Result<Curve> {
    // SE(SM_ERR): "untested code" signal, compiled out of the release build
    let ascii = is_ascii(db);
    // uninitialised upstream (see the module docs)
    let mut num_segments: u64 = 0;
    let mut dimension: u64 = 0;
    let mut closed: u64 = 0;
    if ascii {
        let s = stream(db)?;
        ext_ulong(s, &mut num_segments);
        ext_ulong(s, &mut dimension);
        ext_ulong(s, &mut closed);
        next_line(db);
    } else {
        num_segments = db.read_ulong()?;
        dimension = db.read_ulong()?;
        closed = db.read_ulong()?;
    }
    let is_closed = closed != 0;

    let mut curves: Vec<Curve> = Vec::new();
    let mut senses: Vec<bool> = Vec::new();
    let mut gaps: Vec<f64> = Vec::new();

    // for every segment
    let n = checked_count(db, num_segments, OP_CURVE)?;
    for ii in 0..n {
        ensure_more(db.in_stream(), OP_CURVE)?;
        let mut gap_start = 0.0;
        let mut gap_end = 0.0;
        let mut same_sense: u64 = 0;
        if ascii {
            let s = stream(db)?;
            ext_ulong(s, &mut same_sense);
            ext_double(s, &mut gap_start);
            ext_double(s, &mut gap_end);
            next_line(db);
        } else {
            same_sense = db.read_ulong()?;
            gap_start = db.read_double()?;
            gap_end = db.read_double()?;
        }
        if ii == 0 {
            gaps.push(gap_start);
        }
        gaps.push(gap_end);
        senses.push(same_sense != 0);

        // recursively read the contained curve
        let c = read_curve_from_db(k, dim, version, attrs, db, persist_attribs)?;
        curves.push(c.object);
    }

    Ok(Curve::from(CompositeCurve::with_curves(
        dimension,
        curves,
        is_closed,
        Some(&senses),
        Some(&gaps),
    )?))
}

/// `SmBrepData::ReadSurfaceFromDB`: reads one surface. The type header comes first;
/// for `version > 32` the surface is read by [`Surface::read_from_db`]; up to version
/// 32 only B-spline surfaces and offset surfaces (radius, then a B-spline base)
/// exist. With `version > 30` and `persist_attribs` the surface's attribute indices
/// follow and each indexed attribute present in the table is added to the surface.
///
/// # Errors
///
/// `SM_ERR` for another type up to version 32; those of
/// [`OffsetSurface::new`] for an offset surface; [`SolidError::InvalidInput`] for an attribute index past the table (upstream reads
/// past the array), an ASCII type header that could not be read, or an exhausted
/// [`AttributeSink`] key domain; those of the surface readers.
pub fn read_surface_from_db(
    k: &Kernel,
    version: u64,
    attrs: &mut AttributeSink<'_>,
    db: &mut DatabaseReader,
    persist_attribs: bool,
) -> Result<Keyed<Surface>> {
    let key = attrs.new_key()?;

    // read surface type
    let ty = db
        .read_type(false)?
        .type_code
        .ok_or(SolidError::InvalidInput {
            operation: OP_SURFACE,
            reason: "surface type header could not be read",
        })?;

    // branch on database version number
    let surface = if version > 32 {
        Surface::read_from_db(k, ty, db, version)?
    } else {
        match ty {
            B_SPLINE_SURFACE_TYPE => Surface::from(BSplineSurface::read_from_db(db, version)?),
            OFFSET_SURFACE_TYPE => {
                // read offsetRadius
                let mut offset_radius = 0.0;
                if is_ascii(db) {
                    ext_double(stream(db)?, &mut offset_radius);
                    next_line(db);
                } else {
                    offset_radius = db.read_double()?;
                }
                // read the base surface
                let base = BSplineSurface::read_from_db(db, version)?;
                Surface::from(OffsetSurface::new(
                    k,
                    offset_radius,
                    Surface::from(base),
                    true,
                    SS_UNKNOWN,
                )?)
            }
            _ => return Err(SolidError::sm_err(OP_SURFACE)),
        }
    };

    // for current versions - read attributes
    if version > 30 && persist_attribs {
        let indices = attrs.read_indices(db)?;
        for i in indices {
            let slot = usize::try_from(i)
                .ok()
                .and_then(|i| attrs.all.get(i))
                .ok_or(SolidError::InvalidInput {
                    operation: OP_SURFACE,
                    reason: "attribute index past the attribute table",
                })?;
            if let Some(h) = *slot {
                attrs.store.add_attribute(key, h)?;
            }
        }
    }

    Ok(Keyed {
        key,
        object: surface,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::{Attribute, Behavior};

    fn ascii_reader(text: &str) -> DatabaseReader {
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        db
    }

    fn binary_reader(bytes: Vec<u8>) -> DatabaseReader {
        let mut db = DatabaseReader::new();
        db.open_for_read(bytes, FileType::Binary);
        db
    }

    struct Bin(Vec<u8>);
    impl Bin {
        fn l(mut self, v: u64) -> Self {
            self.0.extend(v.to_ne_bytes());
            self
        }
        fn d(mut self, v: f64) -> Self {
            self.0.extend(v.to_ne_bytes());
            self
        }
        fn b(mut self, v: i32) -> Self {
            self.0.extend(v.to_ne_bytes());
            self
        }
    }

    const DISK_HEAD: &str = "//Brep Starts
1.0 0.0 0.0 - Color
0.000001 - Brep Tolerance
0 - Manifold
0 - Allow Composites

//Region Data
1 Regions In Brep
0 - Start Region
//cols
0 1 0 1

//Shell Data
1 Shells In Brep
//a
//b
//c
0 0 1 2 0

//Composite Face Data
0 Composite Faces In Brep

//Faceuse Data
2 Faceuses In Brep
//cols
0 0 1
1 0 0

//Face Data
1 Faces In Brep
//a
//b
0 0.000001 0 0 1 0
-5.0 5.0 -1.5 3.5

//Loop Data
1 Loops In Brep
//a
//b
0 0 2 0

//Composite Edge Data
1 Composite Edges In Brep
//cols
0 2 0 1

//Edgeuse Data
2 Edgeuses In Brep
//a
//b
//c
//d
0 0 0 0 1 0 -1 0
1 0 1 0 1 1 -2 1

//Edge Data
1 Edges In Brep
//cols
0 0.000001 0 0 1 0 2.0 1.0

//Vertex Data
1 Vertices In Brep
//cols
0 0.000001 5.0 0.0 0.0

// UV-Curve Data
0 UV-Curves In Brep

// 3D-Curve Data
0 3D-Curves In Brep

// Surface Data
0 Surfaces In Brep
";

    #[test]
    fn reads_version_30_ascii_topology() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        let mut db = ascii_reader(DISK_HEAD);
        let bd = BrepData::read_from_db(&k, 30, &mut attrs, &mut db, true).unwrap();
        assert_eq!(bd.color, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(bd.zone_tol_3d, 0.000001);
        assert_eq!(bd.manifold, 0);
        assert_eq!(bd.data_source_type, DataSourceType::SmLib);
        assert_eq!((bd.num_regions, bd.start_region), (1, 0));
        let r = &bd.regions[0];
        assert_eq!((r.num_shells, r.start_shell, r.is_void), (1, 0, 1));
        assert_eq!(r.topology.flags, 0);
        let s = &bd.shells[0];
        assert_eq!((s.shell_type, s.num_faceuses, s.faceuse_start), (0, 2, 0));
        assert_eq!(bd.faceuses.len(), 2);
        assert_eq!(bd.faceuses[1].orientation, 0);
        let f = &bd.faces[0];
        assert_eq!(f.uv_domain, Extent2d::from_bounds(-5.0, -1.5, 5.0, 3.5));
        assert_eq!(bd.loops[0].num_eu, 2);
        assert_eq!(bd.cedges, vec![1, 2, 0, 1]);
        assert!(bd.cfaces.is_empty());
        assert_eq!(bd.edgeuses[1].next_eu, -2);
        assert_eq!(bd.edgeuses[1].uv_curve, 1);
        // min > max: SetMinMax fails and the zero-filled interval stays
        assert_eq!(bd.edges[0].interval, Extent1d::from_raw(0.0, 0.0));
        assert_eq!(bd.vertices[0].point, Vec3::new(5.0, 0.0, 0.0));
        assert!(bd.uv_curves.is_empty() && bd.surfaces.is_empty());
    }

    #[test]
    fn reads_binary_records_and_attributes() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let existing = store
            .insert(Attribute::new(7, Behavior::StandaloneReference))
            .unwrap();
        let mut all = vec![Some(existing)];
        // version 45, attributes: brep [0], one region, empty everywhere else
        let bytes = Bin(Vec::new())
            .l(1)
            .l(0) // brep attributes: [0]
            .b(1) // manifold
            .b(0) // composites
            .l(1) // regions
            .l(0) // start region
            .l(5) // flags
            .l(6)
            .l(7)
            .l(1) // num shells
            .l(0) // start shell
            .b(0) // void
            .l(1)
            .l(3) // region attributes: [3]
            .l(0) // shells
            .l(0) // cfaces
            .l(0) // faceuses
            .l(0) // faces
            .l(0) // loops
            .l(0) // cedges
            .l(0) // edgeuses
            .l(0) // edges
            .l(0) // vertices
            .l(0) // uv curves
            .l(0) // 3d curves
            .l(0) // surfaces
            .d(0.0);
        let mut db = binary_reader(bytes.0);
        let mut attrs = AttributeSink::new(&mut store, &mut all, 100);
        let bd = BrepData::read_from_db(&k, 45, &mut attrs, &mut db, true).unwrap();
        assert_eq!(bd.attributes, vec![0]);
        assert_eq!(bd.zone_tol_3d, ZONE_TOL_3D); // not read for version > 43
        assert_eq!(bd.regions[0].topology.attributes, vec![3]);
        assert_eq!(bd.regions[0].topology.flags, 5);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], Some(existing));
        assert!(all[1].is_none() && all[2].is_none() && all[3].is_some());
    }

    #[test]
    fn huge_counts_are_rejected() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        let bytes = Bin(Vec::new())
            .d(0.0)
            .d(0.0)
            .d(0.0)
            .d(1e-5)
            .b(0)
            .b(0)
            .l(u64::MAX)
            .l(0);
        let mut db = binary_reader(bytes.0);
        assert!(matches!(
            BrepData::read_from_db(&k, 30, &mut attrs, &mut db, false),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut db = binary_reader(Bin(Vec::new()).d(0.0).0);
        assert!(BrepData::read_from_db(&k, 30, &mut attrs, &mut db, false).is_err());
    }

    /// [`DISK_HEAD`] with one bilinear B-spline surface whose last knot ends exactly
    /// at end of data (no trailing newline).
    fn disk_with_surface_at_eof() -> String {
        DISK_HEAD.replace(
            "0 Surfaces In Brep\n",
            "1 Surfaces In Brep\n0 - Surface:\nSurface Type: 15002\n1 1\n1 1\n0\n\
             0 0 0\n1 0 0\n0 1 0\n1 1 1\n0\n0\n1\n1\n0\n0\n1\n1",
        )
    }

    #[test]
    fn last_record_ending_at_eof_reads_fully() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        let text = disk_with_surface_at_eof();
        assert!(!text.ends_with('\n'));
        let mut db = ascii_reader(&text);
        let bd = BrepData::read_from_db(&k, 30, &mut attrs, &mut db, true).unwrap();
        assert_eq!(bd.surfaces.len(), 1);
        assert_eq!(bd.vertices.len(), 1);
        // the final extraction hit end of data
        assert!(db.in_stream().is_some_and(InputStream::eof));
        // the same stream with a trailing newline reads the same records
        let mut db = ascii_reader(&format!("{text}\n"));
        let bd2 = BrepData::read_from_db(&k, 30, &mut attrs, &mut db, true).unwrap();
        assert_eq!(bd2.surfaces.len(), 1);
        assert_eq!(bd2.vertices, bd.vertices);
    }

    #[test]
    fn huge_count_on_a_failed_ascii_stream_is_rejected() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        // Vertex count close to the remaining byte count, then end of data: every
        // further record would consume nothing (zero-filled, ~80 bytes each in memory).
        let head = DISK_HEAD.split("//Vertex Data").next().unwrap().to_owned();
        let pad = 1 << 20;
        let text = format!(
            "{head}//Vertex Data\n{pad} Vertices In Brep\n//cols\n{}",
            " ".repeat(pad + 64)
        );
        let mut db = ascii_reader(&text);
        let start = std::time::Instant::now();
        let r = BrepData::read_from_db(&k, 30, &mut attrs, &mut db, true);
        assert!(
            matches!(
                r,
                Err(SolidError::InvalidInput { reason, .. })
                    if reason == "stream ended before all declared records were read"
            ),
            "{r:?}"
        );
        assert!(start.elapsed().as_secs() < 5);
    }

    #[test]
    fn huge_count_on_a_failed_binary_stream_is_rejected() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        // One composite edge declaring as many items as there are bytes left, then
        // end of data.
        let pad = 1usize << 20;
        let mut bytes = Bin(Vec::new())
            .d(0.0)
            .d(0.0)
            .d(0.0)
            .d(1e-5)
            .b(0)
            .b(0)
            .l(0) // regions
            .l(0) // start region
            .l(0) // shells
            .l(1) // cfaces
            .l(pad as u64) // items of the composite face
            .0;
        bytes.extend(std::iter::repeat_n(0u8, pad));
        let mut db = binary_reader(bytes);
        let r = BrepData::read_from_db(&k, 30, &mut attrs, &mut db, false);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        // a huge record count ends the same way
        let mut bytes = Bin(Vec::new())
            .d(0.0)
            .d(0.0)
            .d(0.0)
            .d(1e-5)
            .b(0)
            .b(0)
            .l(pad as u64) // regions
            .l(0)
            .0;
        bytes.extend(std::iter::repeat_n(0u8, pad));
        let mut db = binary_reader(bytes);
        assert!(BrepData::read_from_db(&k, 30, &mut attrs, &mut db, false).is_err());
    }

    #[test]
    fn key_counter_exhaustion_is_rejected() {
        let mut store = AttributeStore::new();
        let mut all = Vec::new();
        // The counter starts at `AttributeSink::new`'s `first_key`, which the
        // `BrepData` readers pass in, so a sink created there has no key to hand out:
        // the last `u64` key would have no successor.
        let mut attrs = AttributeSink::new(&mut store, &mut all, u64::MAX);
        let r = attrs.new_key();
        assert!(
            matches!(
                r,
                Err(SolidError::InvalidInput { operation, reason })
                    if operation == "topology::brep_data::new_key"
                        && reason == "object keys exhausted"
            ),
            "{r:?}"
        );
        // a rejected allocation leaves the counter where it was
        assert_eq!(attrs.next_key(), u64::MAX);
        assert!(attrs.new_key().is_err());
        assert_eq!(attrs.next_key(), u64::MAX);

        // one key before the boundary is still handed out, without wrapping the counter
        let mut attrs = AttributeSink::new(&mut store, &mut all, u64::MAX - 1);
        assert_eq!(attrs.new_key().unwrap(), ObjectKey(u64::MAX - 1));
        assert_eq!(attrs.next_key(), u64::MAX);
        assert!(attrs.new_key().is_err());
        assert_eq!(attrs.next_key(), u64::MAX);

        // normal allocation control
        let mut attrs = AttributeSink::new(&mut store, &mut all, 5);
        assert_eq!(attrs.new_key().unwrap(), ObjectKey(5));
        assert_eq!(attrs.next_key(), 6);
        assert_eq!(attrs.new_key().unwrap(), ObjectKey(6));

        // the readers propagate the sink's failure instead of reusing a key
        let k = Kernel::default();
        let mut db = ascii_reader("Object Type 14033\n1\n1\n0\n3\n0 0 0\n1 1 1\n0\n0\n1\n1\n1 0\n");
        let mut attrs = AttributeSink::new(&mut store, &mut all, u64::MAX);
        let r = read_curve_from_db(&k, 3, 31, &mut attrs, &mut db, true);
        assert!(
            matches!(
                r,
                Err(SolidError::InvalidInput { operation, .. })
                    if operation == "topology::brep_data::new_key"
            ),
            "{r:?}"
        );
        assert_eq!(attrs.next_key(), u64::MAX);
    }

    #[test]
    fn old_curve_types_and_attribute_slots() {
        let k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut all = vec![None];
        let mut attrs = AttributeSink::new(&mut store, &mut all, 0);
        // a version 31 line curve type is SM_ERR
        let mut db = ascii_reader("Object Type 14001\n");
        assert!(matches!(
            read_curve_from_db(&k, 3, 31, &mut attrs, &mut db, true),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        // version 31 B-spline line segment with an attribute index into an empty slot
        let text = "Object Type 14033\n1\n1\n0\n3\n0 0 0\n1 1 1\n0\n0\n1\n1\n1 0\n";
        let mut db = ascii_reader(text);
        let mut store2 = AttributeStore::new();
        let mut all2: Vec<Option<AttributeHandle>> = vec![None];
        let mut attrs2 = AttributeSink::new(&mut store2, &mut all2, 0);
        let c = read_curve_from_db(&k, 3, 31, &mut attrs2, &mut db, true);
        // index 0 is listed: slot 0 got a placeholder, so the curve gets it
        let c = c.unwrap();
        assert_eq!(c.object.type_code(), BSPLINE_CURVE_TYPE);
        assert_eq!(attrs2.store.attributes(c.key).len(), 1);
    }

    #[test]
    fn read_from_file_headers() {
        let k = Kernel::default();
        let text = format!("Version 45\nHealer Version 3\n{DISK_HEAD}");
        // version 45 without attributes: no tolerance read, records have flags
        let bd = BrepData::read_from_file(&k, text.into_bytes(), FileType::Ascii);
        let bd = bd.unwrap();
        assert_eq!(bd.healer_version(), 3);
        let bd = BrepData::read_from_file(&k, DISK_HEAD.as_bytes().to_vec(), FileType::Ascii);
        assert_eq!(bd.unwrap().healer_version(), 0);
        let mut d = BrepData::default();
        assert_eq!(d.manifold, 0);
        d.set_healer_version(9);
        assert_eq!(d.healer_version(), 9);
        assert_eq!(DataSourceType::Alias.code(), 14);
        assert_eq!(RegionData::default().start_shell, UNDEF_ULONG);
    }
}
