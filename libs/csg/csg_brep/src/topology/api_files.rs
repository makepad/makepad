//! `SM_API` files and conversion: reading a B-rep or a part from a database file,
//! writing a B-rep to one, turning a B-rep's geometry into NURBS, and growing a caller's curve output array.
//!
//! Upstream: `source/SM_API/src/SmApiImportExport.cpp`, `source/SM_API/src/SmApiBrep.cpp`,
//! `source/SM_API/inc/SmApiImportExport.h`, `source/SM_API/inc/SmApiBrep.h`.
//!
//! | Rust                         | Upstream                                  |
//! |------------------------------|-------------------------------------------|
//! | [`read_brep_from_file`]      | `SmApiReadBrepFromFile`                   |
//! | [`read_part_from_file`]      | `SmApiReadPartFromFile`                   |
//! | [`write_brep_to_file`]       | `SmApiWriteBrepToFile`                    |
//! | [`turn_to_nurbs`]            | `SmApiTurnToNurbs`                        |
//! | [`add_curve_output_slots`]   | `SmApiAddCurveOutputSlots` (file static)  |
//!
//! Deviations:
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`].
//! - **Files.** The readers take the file's contents (the library reads no files: it
//!   must build for `wasm32-unknown-unknown`); `None` is upstream's null file name
//!   (`SM_ERR_INVALID_INPUT`). A file that cannot be opened is the caller's error.
//!   [`write_brep_to_file`] returns the file's contents (`pFileName` has no
//!   counterpart, so neither has its `NULL` check); the B-rep's topology records and
//!   attributes are the caller's ([`FromBrepEnv`]).
//! - **Identities.** The new objects are named through the caller's environment
//!   ([`TopologyEnv`], [`PartEnv`]), as [`p7::read_from_file`] and
//!   [`part_file::read_part_from_file`] take it. [`turn_to_nurbs`] is a plain entry
//!   point ([`p5::turn_to_nurbs_plain`]: the kernel's identities and callbacks).
//! - **Null B-rep.** A null B-rep is `None` (`SM_ERR_INVALID_INPUT`).
//! - **Statuses.** The `SM_ERR_INVALID_INPUT` and `SM_ERR_OUT_OF_MEMORY` the wrappers
//!   return themselves are [`SolidError::Upstream`] with that code.
//! - **Appending a part.** Upstream appends the four arrays one after the other and
//!   resets them all when an append fails. The port reserves the four first and returns
//!   `SM_ERR_OUT_OF_MEMORY` with the arrays untouched (they are empty: anything else is
//!   refused before the read).
//! - **Output slots.** The slots are `None` entries of a `Vec<Option<T>>`; the capacity
//!   is reserved fallibly (`try_reserve_exact`), so a failed allocation is
//!   `SM_ERR_OUT_OF_MEMORY` with the vector unchanged, as upstream's. Upstream keeps the
//!   new size below `SM_BIG_ULONG`; so does the port.
//! - `SM_ASSERT_VALID` is not ported.
//! - **Recursion.** None.

use crate::error::{Result, SolidError, SM_ERR_INVALID_INPUT, SM_ERR_OUT_OF_MEMORY};
use crate::geom::tol::USE_DEFAULT;
use crate::kernel::Kernel;
use crate::topology::brep::p10::TopologyEnv;
use crate::topology::brep::{p3, p5, p7};
use crate::topology::brep_data_write::{self, FromBrepEnv};
use crate::topology::database_io::FileType;
use crate::topology::part_file::{self, Part, PartEnv};
use crate::topology::Brep;

const OP_READ_BREP: &str = "topology::api_files::read_brep_from_file";
const OP_READ_PART: &str = "topology::api_files::read_part_from_file";
const OP_TURN: &str = "topology::api_files::turn_to_nurbs";
const OP_WRITE_BREP: &str = "topology::api_files::write_brep_to_file";
const OP_SLOTS: &str = "topology::api_files::add_curve_output_slots";

/// `SM_BIG_ULONG` (`SmMath.h`): the size no `SmTArray` reaches.
pub const BIG_ULONG: u64 = 99_999_999;

/// An `SmApiStatus` the wrapper returns itself.
const fn status(operation: &'static str, code: i32) -> SolidError {
    SolidError::Upstream {
        operation,
        code,
        entity: None,
    }
}

/// `bAscii ? SM_ASCII : SM_BINARY`.
const fn file_type(ascii: bool) -> FileType {
    if ascii {
        FileType::Ascii
    } else {
        FileType::Binary
    }
}

/// `SmApiReadBrepFromFile`: a new B-rep (`new SmBrep()`, [`p3::new_brep`] with the
/// default model size estimate and size ratio) read from `data`, the contents
/// of an ASCII (`ascii`) or binary database file, by `SmBrep::ReadFromFile`
/// ([`p7::read_from_file`]), its UV trim curves rebuilt with `rebuild_uv_trim_curves`.
/// `env` names the new objects. On an error no B-rep is returned (upstream deletes it
/// and leaves `rpBrep` null).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no file (`None`); those of [`p7::read_from_file`].
pub fn read_brep_from_file(
    k: &mut Kernel,
    data: Option<Vec<u8>>,
    ascii: bool,
    rebuild_uv_trim_curves: bool,
    env: &mut TopologyEnv<'_>,
) -> Result<Brep> {
    let Some(data) = data else {
        return Err(status(OP_READ_BREP, SM_ERR_INVALID_INPUT));
    };

    let mut brep = p3::new_brep(k, USE_DEFAULT, USE_DEFAULT)?;
    p7::read_from_file(
        k,
        &mut brep,
        data,
        file_type(ascii),
        rebuild_uv_trim_curves,
        env,
    )?;
    Ok(brep)
}

/// `SmApiReadPartFromFile`: reads the part in `data`, the contents of an ASCII
/// (`ascii`) or binary part file, by `SmBrepData::ReadPartFromFile`
/// ([`part_file::read_part_from_file`]) and appends its curves, surfaces, Boolean tree
/// nodes and B-reps to `out`, which must be empty. `env` names the new objects. On an
/// error `out` is unchanged.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no file (`None`) or a non-empty `out`;
/// `SM_ERR_OUT_OF_MEMORY` when the arrays cannot grow; those of
/// [`part_file::read_part_from_file`].
pub fn read_part_from_file(
    k: &mut Kernel,
    data: Option<Vec<u8>>,
    ascii: bool,
    out: &mut Part,
    env: &mut PartEnv<'_>,
) -> Result<()> {
    let Some(data) = data else {
        return Err(status(OP_READ_PART, SM_ERR_INVALID_INPUT));
    };
    if !out.curves.is_empty()
        || !out.surfaces.is_empty()
        || !out.boolean_tree_nodes.is_empty()
        || !out.breps.is_empty()
    {
        return Err(status(OP_READ_PART, SM_ERR_INVALID_INPUT));
    }

    let part = part_file::read_part_from_file(k, data, file_type(ascii), env)?;

    let oom = |_| status(OP_READ_PART, SM_ERR_OUT_OF_MEMORY);
    out.curves
        .try_reserve_exact(part.curves.len())
        .map_err(oom)?;
    out.surfaces
        .try_reserve_exact(part.surfaces.len())
        .map_err(oom)?;
    out.boolean_tree_nodes
        .try_reserve_exact(part.boolean_tree_nodes.len())
        .map_err(oom)?;
    out.breps.try_reserve_exact(part.breps.len()).map_err(oom)?;
    out.curves.extend(part.curves);
    out.surfaces.extend(part.surfaces);
    out.boolean_tree_nodes.extend(part.boolean_tree_nodes);
    out.breps.extend(part.breps);
    Ok(())
}

/// `SmApiWriteBrepToFile`: `brep` as an SMLib B-rep file, ASCII (`ascii`) or binary, in
/// native formats (`SmBrep::WriteToFile(name, type, TRUE, FALSE, 0.0)`,
/// [`brep_data_write::write_brep_to_file`]); returns the file's contents.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no B-rep; those of
/// [`brep_data_write::write_brep_to_file`].
pub fn write_brep_to_file(
    k: &Kernel,
    brep: Option<&Brep>,
    env: &FromBrepEnv<'_>,
    ascii: bool,
) -> Result<Vec<u8>> {
    let Some(brep) = brep else {
        return Err(status(OP_WRITE_BREP, SM_ERR_INVALID_INPUT));
    };
    brep_data_write::write_brep_to_file(k, brep, env, file_type(ascii))
}

/// `SmApiTurnToNurbs`: replaces the analytic and periodic surfaces and the edge curves
/// of `brep` with NURBS in place (`SmBrep::TurnToNURBS`, [`p5::turn_to_nurbs_plain`]).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no B-rep; those of [`p5::turn_to_nurbs_plain`].
pub fn turn_to_nurbs(k: &mut Kernel, brep: Option<&mut Brep>) -> Result<()> {
    let Some(brep) = brep else {
        return Err(status(OP_TURN, SM_ERR_INVALID_INPUT));
    };
    p5::turn_to_nurbs_plain(k, brep)
}

/// `SmApiAddCurveOutputSlots`: appends `additional` empty (`None`) slots to `curves`
/// after its existing outputs. Nothing changes for `0`.
///
/// # Errors
///
/// `SM_ERR_OUT_OF_MEMORY` when the new size would reach [`BIG_ULONG`] or the slots
/// cannot be allocated; `curves` is then unchanged.
pub fn add_curve_output_slots<T>(curves: &mut Vec<Option<T>>, additional: u64) -> Result<()> {
    let old_size = curves.len() as u64;
    if additional == 0 {
        return Ok(());
    }
    if additional >= BIG_ULONG.wrapping_sub(old_size) {
        return Err(status(OP_SLOTS, SM_ERR_OUT_OF_MEMORY));
    }
    let n = usize::try_from(additional).map_err(|_| status(OP_SLOTS, SM_ERR_OUT_OF_MEMORY))?;
    curves
        .try_reserve_exact(n)
        .map_err(|_| status(OP_SLOTS, SM_ERR_OUT_OF_MEMORY))?;
    curves.resize_with(curves.len() + n, || None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::{AttributeStore, ObjectKey};
    use crate::geom::object::ObjectRef;
    use crate::topology::topology::NotifyRecords;
    use crate::EntityRef;

    fn code(r: &Result<impl Sized>) -> Option<i32> {
        match r {
            Err(SolidError::Upstream { code, .. }) => Some(*code),
            _ => None,
        }
    }

    fn key_of(e: EntityRef) -> ObjectRef {
        let (type_code, i) = match e {
            EntityRef::Brep(x) => (16011, x.index()),
            EntityRef::Face(x) => (16002, x.index()),
            EntityRef::Edge(x) => (16016, x.index()),
            EntityRef::Vertex(x) => (16021, x.index()),
            _ => (16000, 0),
        };
        ObjectRef {
            type_code,
            key: ObjectKey(((type_code as u64) << 32) | u64::from(i)),
        }
    }

    fn empty_part() -> Part {
        Part {
            curves: Vec::new(),
            surfaces: Vec::new(),
            boolean_tree_nodes: Vec::new(),
            breps: Vec::new(),
        }
    }

    #[test]
    fn null_inputs_are_invalid_input() {
        let mut k = Kernel::default();
        let mut attributes = AttributeStore::default();
        let mut records = NotifyRecords::new();
        let mut env = TopologyEnv {
            all_attributes: &[],
            attributes: &mut attributes,
            key_of: &key_of,
            records: &mut records,
            next_key: 1 << 50,
        };
        let r = read_brep_from_file(&mut k, None, true, false, &mut env);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        assert_eq!(
            code(&turn_to_nurbs(&mut k, None)),
            Some(SM_ERR_INVALID_INPUT)
        );

        let mut store = AttributeStore::default();
        let mut part_records = NotifyRecords::new();
        let part_key = |_: usize, e: EntityRef| key_of(e);
        let mut part_env = PartEnv {
            attributes: &mut store,
            key_of: &part_key,
            records: &mut part_records,
            next_key: 1 << 60,
        };
        let mut out = empty_part();
        let r = read_part_from_file(&mut k, None, true, &mut out, &mut part_env);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));

        // a non-empty output array is refused before the read
        out.boolean_tree_nodes.push(7);
        let r = read_part_from_file(&mut k, Some(Vec::new()), true, &mut out, &mut part_env);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        assert_eq!(out.boolean_tree_nodes, vec![7]);
        out.boolean_tree_nodes.clear();
        out.breps.push(Brep::new());
        let r = read_part_from_file(&mut k, Some(Vec::new()), true, &mut out, &mut part_env);
        assert_eq!(code(&r), Some(SM_ERR_INVALID_INPUT));
        assert_eq!(out.breps.len(), 1);
    }

    #[test]
    fn output_slots_grow_by_none_and_refuse_big_sizes() {
        let mut curves: Vec<Option<u32>> = vec![Some(1), Some(2)];
        add_curve_output_slots(&mut curves, 0).unwrap();
        assert_eq!(curves, vec![Some(1), Some(2)]);
        add_curve_output_slots(&mut curves, 3).unwrap();
        assert_eq!(curves, vec![Some(1), Some(2), None, None, None]);
        let r = add_curve_output_slots(&mut curves, BIG_ULONG - 5);
        assert_eq!(code(&r), Some(SM_ERR_OUT_OF_MEMORY));
        let r = add_curve_output_slots(&mut curves, u64::MAX);
        assert_eq!(code(&r), Some(SM_ERR_OUT_OF_MEMORY));
        assert_eq!(curves.len(), 5);
    }
}
