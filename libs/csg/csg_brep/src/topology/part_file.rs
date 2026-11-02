//! Reading an SMLib part file (`.smp`, ASCII or binary): the stand-alone curves and
//! surfaces, the Boolean tree nodes and every B-rep of a part, with the attributes
//! they share, as `SmBrepData::ReadPartFromFile` does.
//!
//! Upstream: `source/SMLib/src/SmBrepData.cpp`, `source/SMLib/inc/SmBrepData.h`,
//! `source/SMLib/src/SmAttribute.cpp`.
//!
//! | Rust                         | Upstream                                         |
//! |------------------------------|--------------------------------------------------|
//! | [`read_part_from_file`]      | `SmBrepData::ReadPartFromFile`                   |
//! | [`read_part_from_db`]        | `SmBrepData::ReadPartFromDB`                     |
//! | [`read_attribute_from_db`]   | `SmBrepData::ReadAttributeFromDB`                |
//! | [`create_attribute`]         | `SmAttribute::CreateAttribute`                   |
//! | [`Part`]                     | the `r3DCurves`, `rSurfaces`, `rBooleanTreeNodes` and `rBreps` out-params |
//! | [`PartEnv`]                  | the attribute heap and the new objects' addresses |
//!
//! Deviations:
//! - **Out-params -> return value.** The part is returned as a [`Part`]. On an error
//!   nothing is returned; upstream leaves what it read so far in the caller's arrays.
//! - **Files.** [`read_part_from_file`] takes the file's bytes (no filesystem on
//!   wasm32); a file that cannot be opened is the caller's error.
//! - **Identities.** Upstream objects are known by address. [`PartEnv::key_of`] names
//!   the topology objects of each B-rep (by B-rep index) and [`PartEnv::next_key`]
//!   hands out the keys of the geometry read and of the B-reps' geometry copies, which
//!   share one [`AttributeStore`] as upstream objects share the heap; the caller keeps
//!   the two key ranges apart.
//! - **ASCII lines.** `fin.getline(sBuff, MAXSIZE)` is [`InputStream::getline`] with
//!   `MAXSIZE` 256. `SSCANF(sBuff, "<text>%ld<text>", &x)` is `scan_long`, glibc's
//!   rules for the part before the conversion (white space in the format matches any
//!   white space, other characters must match) and for `%ld` (skips white space, an
//!   optional sign, decimal digits, clamped to the `long` range on overflow); the text
//!   after the conversion cannot change the value. A failed conversion stores nothing:
//!   the counts keep their initial `0`. The `%ld` read into a `ULONG` count wraps a
//!   negative value, as upstream. The `[Curve #%ld]`, `[Surface #%ld]` and
//!   `[Brep #%ld]` indices are never used and are not scanned. The uninitialised
//!   locals `lNodeValue` and `lTotalAttributes` are `0` when nothing is stored.
//! - **Counts (not upstream; upstream trusts them).** A header-declared count is
//!   [`SolidError::InvalidInput`] when it is above its sanity cap
//!   ([`MAX_PART_ITEMS`] curves, surfaces, tree nodes or attributes,
//!   [`MAX_PART_BREPS`] B-reps, [`MAX_ATTRIBUTE_VALUES`] values of one attribute) or
//!   when the remaining input cannot hold the smallest record of that kind
//!   ([`DatabaseReader::ensure_available`] with the per-kind minimum sizes below).
//!   Items are appended as they are read, never preallocated. Every loop over
//!   such a count fails closed: before each item after the first the stream must be
//!   good, and each item must consume input, else [`SolidError::InvalidInput`]
//!   (upstream would go on reading nothing for the rest of the count).
//! - **Attribute table.** The placeholder table `sAttributes` is
//!   [`AttributeSink::all`]; an index first seen past its end leaves `NULL` slots
//!   below it. `MakeTopologyFromData` gets the table with each `NULL` slot standing
//!   for an unused placeholder (no record refers to it: every index read made its
//!   own); the placeholder is deleted at the end. The table is kept in step with the
//!   slots as B-reps are read (new slots appended; a `NULL` slot a later B-rep fills is
//!   updated from that B-rep's records), not rebuilt per B-rep. The attribute section reaching a
//!   `NULL` slot (upstream dereferences it) or an index past the table (upstream reads
//!   past the array) is [`SolidError::InvalidInput`].
//! - **`CreateAttribute`.** The kernel registers no attribute callbacks, so none is
//!   tried. A colour attribute with fewer than 3 doubles or a tag attribute with fewer
//!   than 8 longs (upstream reads its uninitialised stack buffer) and a behaviour
//!   outside `SmAttributeBehaviorType` (upstream casts it) are
//!   [`SolidError::InvalidInput`].
//! - **Recursion.** None here; nested curves are bounded by [`read_curve_from_db`].

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
#[cfg(test)]
use crate::geom::attribute::ObjectKey;
use crate::geom::attribute::{ids, Attribute, AttributeHandle, AttributeStore, Behavior, TagData};
use crate::geom::object::ObjectRef;
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::surface::surface::Surface;
use crate::topology::brep::p10::{make_topology_from_data, MakeTopologyOptions, TopologyEnv};
use crate::topology::brep_data::{
    read_curve_from_db, read_surface_from_db, AttributeSink, BrepData, Keyed, TopologyData,
};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};
use crate::topology::Brep;

const OP_PART: &str = "topology::part_file::read_part_from_db";
const OP_ATTRIBUTE: &str = "topology::part_file::read_attribute_from_db";
const OP_CREATE: &str = "topology::part_file::create_attribute";

/// Sanity cap on the stand-alone curves, surfaces, Boolean tree nodes and attributes a
/// part may declare (not upstream; upstream fixtures hold a few dozen).
pub const MAX_PART_ITEMS: u64 = 1_000_000;
/// Sanity cap on the B-reps a part may declare (not upstream; fixtures hold at most 51).
pub const MAX_PART_BREPS: u64 = 100_000;
/// Sanity cap on the longs, doubles or characters of one attribute (not upstream).
pub const MAX_ATTRIBUTE_VALUES: u64 = 1_000_000;

/// Smallest stand-alone curve or surface record: ASCII, the two separator lines and the
/// `Object Type <n>` header (at least 16 bytes); binary, the type and dimension longs
/// (a surface: the type long and the B-spline indices).
const MIN_GEOMETRY_BYTES: usize = 16;
/// Smallest B-rep record: ASCII, the separator lines, `//Brep Starts` and the section
/// count lines; binary, the colour (or attribute count), tolerance, flags and the
/// twelve section counts (at least 128 bytes either way).
const MIN_BREP_BYTES: usize = 128;
/// Smallest attribute record: ASCII, two separator lines and the five-number header
/// line (12 bytes); binary, five longs (40).
const MIN_ATTRIBUTE_BYTES_ASCII: usize = 12;
const MIN_ATTRIBUTE_BYTES_BINARY: usize = 40;

/// `MAXSIZE` of `SmBrepData.cpp`: the `getline` buffer size.
const MAXSIZE: usize = 256;

/// What a part file holds (upstream's out-params).
#[derive(Debug)]
pub struct Part {
    /// `r3DCurves`: the stand-alone curves, with their attribute keys.
    pub curves: Vec<Keyed<Curve>>,
    /// `rSurfaces`: the stand-alone surfaces, with their attribute keys.
    pub surfaces: Vec<Keyed<Surface>>,
    /// `rBooleanTreeNodes`: the Boolean tree nodes (see `SmMerge::BooleanTreeNodes`).
    pub boolean_tree_nodes: Vec<i64>,
    /// `rBreps`: the B-rep models.
    pub breps: Vec<Brep>,
}

/// The objects' identities and attributes of a part read (upstream: the heap and
/// the objects' addresses).
pub struct PartEnv<'a> {
    /// Owner of every attribute read and of every object's attribute list.
    pub attributes: &'a mut AttributeStore,
    /// The identity of each topology entity of the B-rep with the given index (in
    /// file order).
    pub key_of: &'a dyn Fn(usize, EntityRef) -> ObjectRef,
    /// The `SmTopology` records of the topology objects built, by identity.
    pub records: &'a mut crate::topology::topology::NotifyRecords,
    /// The next free identity for a geometry object (read or copied); incremented per
    /// object.
    pub next_key: u64,
}

/// `*rDB.GetInStreamPtr()` (upstream dereferences `NULL` without a stream).
fn stream(db: &mut DatabaseReader) -> Result<&mut InputStream> {
    db.in_stream_mut()
        .ok_or_else(|| SolidError::null_pointer(OP_PART))
}

/// `fin.getline(sBuff, MAXSIZE)`.
fn line(db: &mut DatabaseReader) -> Result<Vec<u8>> {
    Ok(stream(db)?.getline(MAXSIZE))
}

/// `std::isspace` in the "C" locale.
fn is_space(c: u8) -> bool {
    c == b' ' || (b'\t'..=b'\r').contains(&c)
}

fn skip_space(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| is_space(c)).count();
    s.get(n..).unwrap_or_default()
}

/// `sscanf(line, "<prefix>%ld...", &x)`: the converted value, or `None` when `line`
/// does not match `prefix` or holds no number where the conversion starts (nothing
/// is stored).
fn scan_long(line: &[u8], prefix: &[u8]) -> Option<i64> {
    let mut rest = line;
    for &f in prefix {
        if is_space(f) {
            rest = skip_space(rest);
        } else {
            let (&c, tail) = rest.split_first()?;
            if c != f {
                return None;
            }
            rest = tail;
        }
    }
    // %ld
    rest = skip_space(rest);
    let (negative, digits) = match rest.split_first() {
        Some((b'-', tail)) => (true, tail),
        Some((b'+', tail)) => (false, tail),
        _ => (false, rest),
    };
    let n = digits.iter().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 {
        return None;
    }
    // strtol: LONG_MIN / LONG_MAX on overflow
    let clamped = if negative { i64::MIN } else { i64::MAX };
    let mut value: i64 = 0;
    for &c in digits.get(..n)? {
        let d = i64::from(c - b'0');
        let next = if negative {
            value.checked_mul(10).and_then(|v| v.checked_sub(d))
        } else {
            value.checked_mul(10).and_then(|v| v.checked_add(d))
        };
        match next {
            Some(v) => value = v,
            None => return Some(clamped),
        }
    }
    Some(value)
}

/// A header-declared count as a loop bound: rejected above `cap` or when the remaining
/// input cannot hold `min_bytes` per item.
fn checked_count(
    db: &DatabaseReader,
    count: u64,
    cap: u64,
    min_bytes: usize,
    op: &'static str,
) -> Result<usize> {
    let too_big = SolidError::InvalidInput {
        operation: op,
        reason: "header-declared size exceeds the remaining input",
    };
    if count > cap {
        return Err(too_big);
    }
    let n = usize::try_from(count).map_err(|_| too_big)?;
    db.ensure_available(n, min_bytes, op)?;
    Ok(n)
}

/// The read position (`0` without a stream).
fn position(db: &DatabaseReader) -> usize {
    db.in_stream().map_or(0, InputStream::position)
}

/// Fail-closed check of item `i` of a counted loop, before it is read: from the second
/// item on, the stream must still be good (not upstream).
fn before_item(db: &DatabaseReader, i: usize, op: &'static str) -> Result<()> {
    if i > 0 && !db.in_stream().is_some_and(InputStream::good) {
        return Err(SolidError::invalid_input(
            op,
            "stream ended before the counted items",
        ));
    }
    Ok(())
}

/// Fail-closed check after an item of a counted loop: it must have consumed input
/// (not upstream).
fn after_item(db: &DatabaseReader, start: usize, op: &'static str) -> Result<()> {
    if position(db) == start {
        return Err(SolidError::invalid_input(
            op,
            "counted item consumed no input",
        ));
    }
    Ok(())
}

/// `SmBrepData::ReadPartFromFile`: reads a part from the contents of a database file
/// of `file_type` (see [`read_part_from_db`]).
///
/// # Errors
///
/// Those of [`read_part_from_db`].
pub fn read_part_from_file(
    k: &mut Kernel,
    data: Vec<u8>,
    file_type: FileType,
    env: &mut PartEnv<'_>,
) -> Result<Part> {
    // create databaseIO for operation
    let mut db = DatabaseReader::new();
    db.open_for_read(data, file_type);

    // pass the call along
    read_part_from_db(k, &mut db, env)
}

/// `SmBrepData::ReadPartFromDB`: reads the part summary (curve, surface, tree node and
/// B-rep counts; the curve count carries the database version and the surface count
/// the healer version, each as millions), then the stand-alone curves (dimension 3)
/// and surfaces, the Boolean tree nodes and the B-reps, each read with attributes and
/// built by `MakeTopologyFromData(pBrepData, sAttributes, FALSE)`. For a version
/// above 30 the attributes follow; each replaces its placeholder on every object
/// using it, and the placeholders are deleted.
///
/// # Errors
///
/// Those of [`read_curve_from_db`], [`read_surface_from_db`],
/// [`BrepData::read_from_db`], [`make_topology_from_data`] and
/// [`read_attribute_from_db`]; a short binary stream, a missing stream, a count
/// exceeding the remaining input; [`SolidError::InvalidInput`] for an attribute
/// index past the placeholder table or on a `NULL` slot.
pub fn read_part_from_db(
    k: &mut Kernel,
    db: &mut DatabaseReader,
    env: &mut PartEnv<'_>,
) -> Result<Part> {
    let ascii = db.file_type() == FileType::Ascii;
    let mut version: u64 = 30;
    let mut healer_version: u64 = 0;

    // SmTArray<SmAttribute*> sAttributes;
    let mut all: Vec<Option<AttributeHandle>> = Vec::new();

    db.begin_reading();

    let mut total_curves: u64 = 0;
    let mut total_surfaces: u64 = 0;
    let mut total_breps: u64 = 0;
    let mut total_tree_nodes: u64 = 0;

    // read counts for stand-alone curves
    if ascii {
        line(db)?; // //[Output Summary]
        if let Some(v) = scan_long(&line(db)?, b"//   (1) ") {
            total_curves = v as u64;
        }
    } else {
        total_curves = db.read_ulong()?;
    }

    // extract version number embedded within the curve count
    if total_curves > 1_000_000 {
        version = total_curves / 1_000_000;
        total_curves %= 1_000_000;
        db.set_version(version);
    }
    // else: WARN "Database being read has no lDBVersionNumber"

    // read counts for stand-alone surfaces
    if ascii {
        if let Some(v) = scan_long(&line(db)?, b"//   (2) ") {
            total_surfaces = v as u64;
        }
    } else {
        // version >= 40: ignore(sizeof(long long) - sizeof(long)) skips nothing on LP64
        total_surfaces = db.read_ulong()?;
    }

    // extract Healer version number embedded within the Surface count
    if total_surfaces > 1_000_000 {
        healer_version = total_surfaces / 1_000_000;
        total_surfaces %= 1_000_000;
    }
    // else: WARN "Database being read has no lHealerVersion"

    // read counts for stand-alone boolean trees, and brep Models
    if ascii {
        if let Some(v) = scan_long(&line(db)?, b"//   (3) ") {
            total_tree_nodes = v as u64;
        }
        if let Some(v) = scan_long(&line(db)?, b"//   (4) ") {
            total_breps = v as u64;
        }
    } else {
        total_tree_nodes = db.read_ulong()?;
        total_breps = db.read_ulong()?;
    }

    // stand-alone curves - no approximations
    let geometry_bytes = MIN_GEOMETRY_BYTES;
    let n = checked_count(db, total_curves, MAX_PART_ITEMS, geometry_bytes, OP_PART)?;
    let mut curves = Vec::new();
    for i in 0..n {
        before_item(db, i, OP_PART)?;
        let start = position(db);
        if ascii {
            line(db)?;
            line(db)?; // [Curve #i]
        }
        let mut attrs = AttributeSink::new(&mut *env.attributes, &mut all, env.next_key);
        let c = read_curve_from_db(k, 3, version, &mut attrs, db, true)?;
        env.next_key = attrs.next_key();
        curves.push(c);
        after_item(db, start, OP_PART)?;
    }

    // stand-alone surfaces - no approximations
    let n = checked_count(db, total_surfaces, MAX_PART_ITEMS, geometry_bytes, OP_PART)?;
    let mut surfaces = Vec::new();
    for j in 0..n {
        before_item(db, j, OP_PART)?;
        let start = position(db);
        if ascii {
            line(db)?;
            line(db)?; // [Surface #j]
        }
        let mut attrs = AttributeSink::new(&mut *env.attributes, &mut all, env.next_key);
        let s = read_surface_from_db(k, version, &mut attrs, db, true)?;
        env.next_key = attrs.next_key();
        surfaces.push(s);
        after_item(db, start, OP_PART)?;
    }

    // boolean trees
    let node_bytes = if ascii { 1 } else { 8 };
    let n = checked_count(db, total_tree_nodes, MAX_PART_ITEMS, node_bytes, OP_PART)?;
    if ascii {
        // Read Line With title
        line(db)?;
        line(db)?;
    }
    let mut boolean_tree_nodes = Vec::new();
    for m in 0..n {
        before_item(db, m, OP_PART)?;
        let start = position(db);
        let node = if ascii {
            scan_long(&line(db)?, b"").unwrap_or(0)
        } else {
            db.read_long()?
        };
        boolean_tree_nodes.push(node);
        after_item(db, start, OP_PART)?;
    }

    // ASCII white space
    if ascii {
        line(db)?;
        line(db)?;
    }

    // Brep Models
    let n = checked_count(db, total_breps, MAX_PART_BREPS, MIN_BREP_BYTES, OP_PART)?;
    let mut breps = Vec::new();
    let mut table = PlaceholderTable::default();
    for i in 0..n {
        before_item(db, i, OP_PART)?;
        let start = position(db);
        if ascii {
            line(db)?;
            line(db)?;
            line(db)?; // [Brep #k]
        }

        // allocate an empty and initialized SmBrep object
        let mut b = Brep::new_with_infinite_region()?;

        // read the BrepData
        let mut attrs = AttributeSink::new(&mut *env.attributes, &mut all, env.next_key);
        let mut data = BrepData::read_from_db(k, version, &mut attrs, db, true)?;
        env.next_key = attrs.next_key();

        // Set pBrepData Healer Version
        data.set_healer_version(healer_version);

        // Build Brep Model from BrepData
        //   FALSE: do not force creation of uv trim curves.
        table.update(env.attributes, &all, &data)?;
        let key_of = env.key_of;
        let key = move |e: EntityRef| key_of(i, e);
        let mut topology_env = TopologyEnv {
            all_attributes: &table.handles,
            attributes: &mut *env.attributes,
            key_of: &key,
            records: &mut *env.records,
            next_key: env.next_key,
        };
        let options = MakeTopologyOptions {
            make_uv_trim_curves: false,
            ..MakeTopologyOptions::default()
        };
        make_topology_from_data(k, &mut b, &mut data, &mut topology_env, options)?;
        env.next_key = topology_env.next_key;
        breps.push(b);
        after_item(db, start, OP_PART)?;
    }

    // Now read in attributes - the placeholders collected in the table are replaced
    // by the real attributes, which are in the same order in the file.
    if version > 30 {
        let replaced = replace_attributes(db, env.attributes, &all);
        // SmObjsDelete<SmAttribute *> sCleanAttributes(&sAttributes)
        let cleaned = delete_placeholders(env.attributes, all.iter().flatten().copied());
        replaced?;
        cleaned?;
    }
    if let Some(h) = table.unused {
        delete_placeholders(env.attributes, [h])?;
    }

    db.end_reading();

    Ok(Part {
        curves,
        surfaces,
        boolean_tree_nodes,
        breps,
    })
}

/// The placeholder table as `MakeTopologyFromData` takes it, kept in step with the
/// slots: each `NULL` slot is `unused` (made once, referenced by no record).
#[derive(Debug, Default)]
struct PlaceholderTable {
    handles: Vec<AttributeHandle>,
    unused: Option<AttributeHandle>,
}

impl PlaceholderTable {
    /// The handle standing for a `NULL` slot.
    fn unused(&mut self, store: &mut AttributeStore) -> Result<AttributeHandle> {
        if let Some(h) = self.unused {
            return Ok(h);
        }
        let h = store.insert(Attribute::new(ids::UNKNOWN, Behavior::StandaloneReference))?;
        self.unused = Some(h);
        Ok(h)
    }

    /// Brings the table up to date after B-rep `data` was read into `all`: appends the
    /// new slots, and updates the `NULL` slots the B-rep's records filled (a slot is
    /// filled only when an index naming it is read, so those records list it).
    fn update(
        &mut self,
        store: &mut AttributeStore,
        all: &[Option<AttributeHandle>],
        data: &BrepData,
    ) -> Result<()> {
        // slots filled below the old end: only indices this B-rep's records name
        let old_len = self.handles.len();
        if self.unused.is_some() {
            for &i in record_indices(data) {
                let Ok(i) = usize::try_from(i) else {
                    continue;
                };
                if i >= old_len {
                    continue;
                }
                if let (Some(slot), Some(Some(h))) = (self.handles.get_mut(i), all.get(i)) {
                    *slot = *h;
                }
            }
        }
        // new slots
        for slot in all.iter().skip(old_len) {
            let h = match slot {
                Some(h) => *h,
                None => self.unused(store)?,
            };
            self.handles.push(h);
        }
        Ok(())
    }
}

/// The attribute indices of one record.
fn topo(t: &TopologyData) -> core::slice::Iter<'_, u64> {
    t.attributes.iter()
}

/// Every attribute index the records of `data` list (the B-rep's own included).
fn record_indices(data: &BrepData) -> impl Iterator<Item = &u64> {
    data.attributes
        .iter()
        .chain(data.regions.iter().flat_map(|r| topo(&r.topology)))
        .chain(data.shells.iter().flat_map(|s| topo(&s.topology)))
        .chain(data.faceuses.iter().flat_map(|f| topo(&f.topology)))
        .chain(data.faces.iter().flat_map(|f| topo(&f.topology)))
        .chain(data.loops.iter().flat_map(|l| {
            topo(&l.topology)
                .chain(l.lu_upper_attributes.iter())
                .chain(l.lu_lower_attributes.iter())
        }))
        .chain(
            data.edgeuses
                .iter()
                .flat_map(|e| topo(&e.topology).chain(e.mate_attributes.iter())),
        )
        .chain(data.edges.iter().flat_map(|e| topo(&e.topology)))
        .chain(data.vertices.iter().flat_map(|v| topo(&v.topology)))
}

/// `delete` of each placeholder still alive.
fn delete_placeholders(
    store: &mut AttributeStore,
    handles: impl IntoIterator<Item = AttributeHandle>,
) -> Result<()> {
    for h in handles {
        if store.is_live(h) {
            store.destroy(h)?;
        }
    }
    Ok(())
}

/// The attribute section of [`read_part_from_db`]: the attribute count, then each
/// attribute, which replaces the placeholder of the same index on every object using
/// it (an attribute without users is dropped).
fn replace_attributes(
    db: &mut DatabaseReader,
    store: &mut AttributeStore,
    all: &[Option<AttributeHandle>],
) -> Result<()> {
    let ascii = db.file_type() == FileType::Ascii;

    // read total attribute count (uninitialised upstream when nothing is stored)
    let mut total: u64 = 0;
    if ascii {
        line(db)?;
        if let Some(v) = scan_long(&line(db)?, b"") {
            total = v as u64;
        }
    } else {
        total = db.read_ulong()?;
    }

    // read each attribute (bounded as the other counted sections)
    let min_bytes = if ascii {
        MIN_ATTRIBUTE_BYTES_ASCII
    } else {
        MIN_ATTRIBUTE_BYTES_BINARY
    };
    let total = checked_count(db, total, MAX_PART_ITEMS, min_bytes, OP_PART)?;
    for j in 0..total {
        before_item(db, j, OP_PART)?;
        let start = position(db);
        if ascii {
            // clear ascii white space
            line(db)?;
            line(db)?;
        }

        // read the attribute
        let attribute = read_attribute_from_db(db)?;
        after_item(db, start, OP_PART)?;

        // Replace and update attribute in sAttributes array
        let slot = all.get(j).ok_or(SolidError::InvalidInput {
            operation: OP_PART,
            reason: "attribute index past the attribute table",
        })?;
        let temp = slot.ok_or(SolidError::InvalidInput {
            operation: OP_PART,
            reason: "attribute table slot is NULL",
        })?;

        // get every object using this attribute
        let users = store.users(temp)?;

        // User may have been deleted by heal sequence in MakeTopologyFromData
        if users.is_empty() {
            continue; // delete pAttribute
        }
        let new = store.insert(attribute)?;
        // for every attribute user: substitute attribute
        for user in users {
            store.remove_attribute(user, temp, false)?;
            store.add_attribute(user, new)?;
        }
    }
    Ok(())
}

/// `SmBrepData::ReadAttributeFromDB`: reads one attribute (id, behaviour and its
/// longs, doubles and characters) and makes it with [`create_attribute`].
///
/// # Errors
///
/// A short binary stream, a missing stream, a count exceeding the remaining input;
/// those of [`create_attribute`].
pub fn read_attribute_from_db(db: &mut DatabaseReader) -> Result<Attribute> {
    // uninitialised upstream when nothing is stored
    let mut id: u64 = 0;
    let mut behavior: i64 = 0;
    let mut num_long: u64 = 0;
    let mut num_double: u64 = 0;
    let mut num_char: u64 = 0;
    let mut longs = Vec::new();
    let mut doubles = Vec::new();
    let mut chars = Vec::new();

    if db.file_type() == FileType::Ascii {
        let s = stream(db)?;
        if let Some(v) = s.extract_ulong() {
            id = v;
        }
        if let Some(v) = s.extract_long() {
            behavior = v;
        }
        if let Some(v) = s.extract_ulong() {
            num_long = v;
        }
        if let Some(v) = s.extract_ulong() {
            num_double = v;
        }
        if let Some(v) = s.extract_ulong() {
            num_char = v;
        }
        db.go_to_next_line();

        // read the longs ("<n> " each: at least two bytes)
        let n = checked_count(db, num_long, MAX_ATTRIBUTE_VALUES, 2, OP_ATTRIBUTE)?;
        for i in 0..n {
            before_item(db, i, OP_ATTRIBUTE)?;
            let start = position(db);
            longs.push(stream(db)?.extract_long().unwrap_or(0));
            after_item(db, start, OP_ATTRIBUTE)?;
        }
        if num_long > 0 {
            db.go_to_next_line();
        }
        // read the doubles
        let n = checked_count(db, num_double, MAX_ATTRIBUTE_VALUES, 2, OP_ATTRIBUTE)?;
        for i in 0..n {
            before_item(db, i, OP_ATTRIBUTE)?;
            let start = position(db);
            doubles.push(stream(db)?.extract_double().unwrap_or(0.0));
            after_item(db, start, OP_ATTRIBUTE)?;
        }
        if num_double > 0 {
            db.go_to_next_line();
        }
        // read the characters
        let n = checked_count(db, num_char, MAX_ATTRIBUTE_VALUES, 1, OP_ATTRIBUTE)?;
        for i in 0..n {
            before_item(db, i, OP_ATTRIBUTE)?;
            let start = position(db);
            chars.push(stream(db)?.get().unwrap_or(b'0'));
            after_item(db, start, OP_ATTRIBUTE)?;
        }
        if num_char > 0 {
            db.go_to_next_line();
        }
    } else {
        id = db.read_ulong()?;
        behavior = db.read_long()?;
        num_long = db.read_ulong()?;
        num_double = db.read_ulong()?;
        num_char = db.read_ulong()?;

        if num_long > 0 {
            let n = checked_count(db, num_long, MAX_ATTRIBUTE_VALUES, 8, OP_ATTRIBUTE)?;
            longs = db.read_longs(n)?.into_iter().map(|v| v as i64).collect();
        }
        if num_double > 0 {
            let n = checked_count(db, num_double, MAX_ATTRIBUTE_VALUES, 8, OP_ATTRIBUTE)?;
            doubles = db.read_doubles(n)?;
        }
        if num_char > 0 {
            let n = checked_count(db, num_char, MAX_ATTRIBUTE_VALUES, 1, OP_ATTRIBUTE)?;
            chars = db.read_characters(n)?;
        }
    }

    create_attribute(id, behavior, &longs, &doubles, &chars)
}

/// `SmAttribute::CreateAttribute`: a colour id ([`ids::COLOR`]) makes a vector
/// attribute from the first three doubles, a tag id ([`ids::TAG`]) a tag attribute
/// from the first eight longs, any other id a generic attribute holding the values.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a colour with fewer than 3 doubles, a tag with
/// fewer than 8 longs or a behaviour that is not an `SmAttributeBehaviorType` value.
pub fn create_attribute(
    attribute_id: u64,
    behavior: i64,
    longs: &[i64],
    doubles: &[f64],
    chars: &[u8],
) -> Result<Attribute> {
    let behavior = i32::try_from(behavior)
        .map_err(|_| SolidError::invalid_input(OP_CREATE, "behaviour out of range"))
        .and_then(Behavior::from_code)?;
    match attribute_id {
        ids::COLOR => match doubles {
            [x, y, z, ..] => Ok(Attribute::vector3d(
                attribute_id,
                Vec3::new(*x, *y, *z),
                behavior,
            )),
            _ => Err(SolidError::invalid_input(
                OP_CREATE,
                "colour attribute without 3 doubles",
            )),
        },
        ids::TAG => match longs {
            [a, b, c, d, e, f, g, h, ..] => {
                let tag = TagData {
                    primary_id: *a,
                    primary_ordinal: *b,
                    secondary_id_type: *c,
                    secondary_id: *d,
                    secondary_ordinal: *e,
                    third_id_type: *f,
                    third_id: *g,
                    third_ordinal: *h,
                };
                Ok(Attribute::tag(attribute_id, tag, behavior))
            }
            _ => Err(SolidError::invalid_input(
                OP_CREATE,
                "tag attribute without 8 longs",
            )),
        },
        // Just create a generic attribute
        _ => Ok(Attribute::generic(
            attribute_id,
            behavior,
            longs,
            doubles,
            chars,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(line: &str, prefix: &str) -> Option<i64> {
        scan_long(line.as_bytes(), prefix.as_bytes())
    }

    #[test]
    fn scan_long_follows_sscanf() {
        let p1 = "//   (1) ";
        assert_eq!(scan("//   (1) 32000000 Curves", p1), Some(32_000_000));
        assert_eq!(scan("//  (1)   -7 x", p1), Some(-7));
        assert_eq!(scan("// (2) 5", p1), None);
        assert_eq!(scan("   12 // Total", ""), Some(12));
        assert_eq!(scan("x", ""), None);
        assert_eq!(scan("", ""), None);
        assert_eq!(scan("99999999999999999999", ""), Some(i64::MAX));
        assert_eq!(scan("-99999999999999999999", ""), Some(i64::MIN));
    }

    #[test]
    fn getline_follows_libstdcxx() {
        let mut s = InputStream::new(b"ab\n\ncd".to_vec());
        assert_eq!(s.getline(MAXSIZE), b"ab");
        assert!(s.good());
        assert_eq!(s.getline(MAXSIZE), b"");
        assert!(s.good());
        assert_eq!(s.getline(MAXSIZE), b"cd");
        assert!(s.eof());
        assert!(!s.fail());
        assert_eq!(s.getline(MAXSIZE), b"");
        assert!(s.fail());

        // a full buffer before the newline fails and leaves the rest
        let mut s = InputStream::new(b"abcdef\n".to_vec());
        assert_eq!(s.getline(4), b"abc");
        assert!(s.fail());
        // exactly n - 1 bytes then the newline is fine
        let mut s = InputStream::new(b"abc\nx".to_vec());
        assert_eq!(s.getline(4), b"abc");
        assert!(s.good());
    }

    fn key_of(_: usize, _: EntityRef) -> ObjectRef {
        ObjectRef {
            type_code: 0,
            key: ObjectKey(0),
        }
    }

    fn read(text: &str) -> Result<Part> {
        let mut k = Kernel::default();
        let mut store = AttributeStore::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut env = PartEnv {
            attributes: &mut store,
            key_of: &key_of,
            records: &mut records,
            next_key: 1 << 50,
        };
        let data = text.as_bytes().to_vec();
        read_part_from_file(&mut k, data, FileType::Ascii, &mut env)
    }

    const HEAD: &str = "//[Output Summary]\n//   (1) 32000000 Curves\n//   (2) 0 Surfaces\n";

    #[test]
    fn empty_part_reads() -> Result<()> {
        let text = format!(
            "{HEAD}//   (3) 2 Boolean Tree Nodes\n//   (4) 0 Breps\n\n// Boolean Trees\n\
             7\n-3\n\n\n\n0 // Total number of attributes\n"
        );
        let part = read(&text)?;
        assert!(part.curves.is_empty());
        assert!(part.surfaces.is_empty());
        assert!(part.breps.is_empty());
        assert_eq!(part.boolean_tree_nodes, vec![7, -3]);
        Ok(())
    }

    #[test]
    fn counts_beyond_the_input_are_rejected() {
        let text = "//[Output Summary]\n//   (1) 32999999 Curves\n";
        assert!(matches!(read(text), Err(SolidError::InvalidInput { .. })));
        let text = format!("{HEAD}//   (3) 0 Boolean Tree Nodes\n//   (4) 9000000000 Breps\n");
        assert!(matches!(read(&text), Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn counted_loops_fail_closed() {
        // four tree nodes declared, the stream ends after two
        let text = format!(
            "{HEAD}//   (3) 4 Boolean Tree Nodes\n//   (4) 0 Breps\n\n// Boolean Trees\n7\n-3"
        );
        assert!(matches!(read(&text), Err(SolidError::InvalidInput { .. })));
        // more B-reps than the cap
        let text = format!("{HEAD}//   (3) 0 Boolean Tree Nodes\n//   (4) 100001 Breps\n");
        assert!(matches!(read(&text), Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn attribute_past_the_table_is_rejected() {
        let text = format!(
            "{HEAD}//   (3) 0 Boolean Tree Nodes\n//   (4) 0 Breps\n\n// Boolean Trees\n\n\n\n\
             1 // Total number of attributes\n\nAttribute #0\n100 0 0 3 0\n1 2 3\n"
        );
        assert!(matches!(read(&text), Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn create_attribute_by_id() -> Result<()> {
        let rgb = [0.5, 0.25, 1.0];
        let c = create_attribute(ids::COLOR, 0, &[], &rgb, &[])?;
        assert_eq!(c.vector3d_value(), Some(Vec3::new(0.5, 0.25, 1.0)));
        assert!(create_attribute(ids::COLOR, 0, &[], &[0.5], &[]).is_err());
        let tag = [1, 2, 3, 4, 5, 6, 7, 8];
        let t = create_attribute(ids::TAG, 1, &tag, &[], &[])?;
        assert_eq!(t.behavior(), Behavior::Reference);
        assert!(create_attribute(ids::TAG, 0, &[1, 2], &[], &[]).is_err());
        let g = create_attribute(5000, 3, &[9], &[1.5], b"ab")?;
        assert_eq!(g.attribute_id(), 5000);
        assert!(create_attribute(5000, 9, &[], &[], &[]).is_err());
        Ok(())
    }
}
