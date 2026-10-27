// ported from N:rustc_const_eval/src/const_eval/valtrees.rs @8d1a764
use crate::const_eval::{ErrorHandled, EvalToValTreeResult, ValTreeCreationError};
use crate::consts::{Const, ScalarInt, ValTree};
use crate::fxhash::{HashMap, HashSet};
use crate::jit::Unit;
use crate::types::{TyId, TyKind};

type Place = (u64, TyId, Option<u64>);

/// valtrees.rs:86-209: visit the typed value, retaining field types and enum
/// variant indices. The provider has already executed and validated the initializer.
pub fn const_to_valtree(u: &Unit, addr: u64, ty: TyId) -> EvalToValTreeResult {
    const_to_valtree_inner(u, (addr, ty, None), &mut 0, &mut HashSet::default(), &mut HashMap::default()).map_err(ErrorHandled::Reported)
}
fn scalar(addr: u64, size: u8) -> ScalarInt {
    let mut bytes = [0; 16];
    unsafe { std::ptr::copy_nonoverlapping(addr as *const u8, bytes.as_mut_ptr(), size as usize); }
    ScalarInt::truncate_from_uint(u128::from_le_bytes(bytes), size).0
}
fn const_to_valtree_inner(u: &Unit, place: Place, nodes: &mut usize, visited: &mut HashSet<Place>, settled: &mut HashMap<Place, Result<ValTree, ValTreeCreationError>>) -> Result<ValTree, ValTreeCreationError> {
    if let Some(result) = settled.get(&place) { return result.clone(); }
    if !visited.insert(place) { return Err(ValTreeCreationError::CyclicConst); }
    if *nodes >= 100_000 { return Err(ValTreeCreationError::NodesOverflow); }
    let (addr, ty, meta) = place;
    let tys = &u.tcx.tys;
    let result = match tys.kind(ty).clone() {
        TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char => {
            *nodes += 1;
            Ok(ValTree::from_scalar_int(tys, scalar(addr, u.layout_of(ty).size as u8)))
        }
        TyKind::FnDef(..) => { *nodes += 1; Ok(ValTree::from_branches(tys, &[])) }
        TyKind::Ref(_, inner, _) => {
            // pointer-sized words (a wasm32 word holds the window offset: syms::read_ptr)
            let pb = u.ptr_bytes() as u64;
            let ptr = crate::syms::read_ptr(u, addr);
            let meta = match tys.kind(inner) { TyKind::Str | TyKind::Slice(_) => Some(scalar(addr + pb, pb as u8).to_bits(pb as u8) as u64), _ => None };
            const_to_valtree_inner(u, (ptr, inner, meta), nodes, visited, settled)
        }
        TyKind::Array(..) | TyKind::Slice(_) | TyKind::Str => {
            let (elem, len) = match tys.kind(ty) {
                TyKind::Array(elem, len) => (*elem, len.try_to_target_usize(tys).ok_or(ValTreeCreationError::ErrorHandled("unresolved array length"))?),
                TyKind::Slice(elem) => (*elem, meta.ok_or(ValTreeCreationError::ErrorHandled("missing slice length"))?),
                TyKind::Str => (tys.intern(TyKind::Int(crate::program::Prim::U8)), meta.ok_or(ValTreeCreationError::ErrorHandled("missing string length"))?),
                _ => unreachable!(),
            };
            let stride = u.layout_of(elem).size as u64;
            let mut branches = Vec::new();
            for i in 0..len {
                let tree = const_to_valtree_inner(u, (addr + i * stride, elem, None), nodes, visited, settled)?;
                branches.push(Const::new_value(tys, tree, elem));
            }
            if branches.is_empty() { *nodes += 1; }
            Ok(ValTree::from_branches(tys, &branches))
        }
        TyKind::Tuple(fields) => {
            let layout = u.layout_of(ty);
            let mut branches = Vec::with_capacity(tys.list(fields).len());
            for (i, &field) in tys.list(fields).iter().enumerate() {
                let tree = const_to_valtree_inner(u, (addr + layout.fields[i] as u64, field, None), nodes, visited, settled)?;
                branches.push(Const::new_value(tys, tree, field));
            }
            if branches.is_empty() { *nodes += 1; }
            Ok(ValTree::from_branches(tys, &branches))
        }
        TyKind::Adt(def, args, _) => {
            let adt = &u.tcx.adts[&def];
            if adt.is_union { return Err(ValTreeCreationError::NonSupportedType(ty)); }
            let layout = u.layout_of(ty);
            if layout.uninhabited { return Err(ValTreeCreationError::ErrorHandled("uninhabited value")); }
            let variant = match layout.enc {
                crate::layout::TagEnc::Single(v) => v as usize,
                _ if adt.is_enum => {
                    let (off, relax_backend::rir::Mem::Int(size, _)) = layout.tag.ok_or(ValTreeCreationError::ErrorHandled("missing enum tag"))? else { return Err(ValTreeCreationError::NonSupportedType(ty)); };
                    let disc = crate::lower::decode_tag_const(u, scalar(addr + off as u64, size).to_bits(size) as i128, layout, adt).ok_or(ValTreeCreationError::ErrorHandled("missing enum discriminant"))?;
                    adt.discriminants(&u.tcx, &u.prog).iter().position(|(_, v)| v.signed_value() == disc).ok_or(ValTreeCreationError::ErrorHandled("invalid enum tag"))?
                }
                _ => 0,
            };
            let mut branches = Vec::new();
            if adt.is_enum {
                let tree = ValTree::from_scalar_int(tys, ScalarInt::try_from_uint(variant as u128, 4).unwrap());
                branches.push(Const::new_value(tys, tree, tys.u32_));
            }
            for i in 0..adt.variants[variant].fields.len() {
                let field = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), def, variant as u32, i as u32, tys.list(args));
                let off = if adt.is_enum { layout.variant_fields[variant][i] } else { layout.fields[i] };
                let tree = const_to_valtree_inner(u, (addr + off as u64, field, None), nodes, visited, settled)?;
                branches.push(Const::new_value(tys, tree, field));
            }
            if branches.is_empty() { *nodes += 1; }
            Ok(ValTree::from_branches(tys, &branches))
        }
        _ => Err(ValTreeCreationError::NonSupportedType(ty)),
    };
    visited.remove(&place);
    settled.insert(place, result.clone());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jit_query_preserves_aggregate_types_and_reports_panics() {
        isolated_test!(jit_query_preserves_aggregate_types_and_reports_panics);
        const CHILD: &str = "MAKEPAD_RUN_VALTREE_QUERY_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "valtrees::tests::jit_query_preserves_aggregate_types_and_reports_panics", "--test-threads=1"])
                .env(CHILD, "1").status().unwrap();
            assert!(status.success()); return;
        }
        crate::borrowck::lazy::disable_gandalf();
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        let dir = std::env::temp_dir().join(format!("relax-valtree-query-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, br#"
            const fn aggregate() -> (u8, [u16; 2]) { (7, [11, 13]) }
            const VALUE: (u8, [u16; 2]) = aggregate();
            const fn fail() -> usize { panic!("const evaluation panic") }
            const BAD: usize = fail();
            const AFTER: usize = 5usize.count_ones() as usize;
        "#).unwrap();
        let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "valtree_query", false).unwrap();
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        let u = Unit::new(prog, tcx, core);
        let query = |name| {
            let def = u.prog.defs.iter().enumerate().find_map(|(i, d)| {
                let id = crate::program::DefId(i as u32);
                (d.krate == krate && u.prog.name(id) == name).then_some(id)
            }).unwrap();
            u.tcx.const_eval_resolve(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), def, &[])
        };
        let value = query("VALUE").unwrap();
        let crate::consts::ValTreeKind::Branch(fields) = value.kind() else { panic!("tuple valtree") };
        assert_eq!(fields.len(), 2);
        let crate::consts::ConstKind::Value(first) = u.tcx.tys.const_kind(fields[0]) else { panic!() };
        assert_eq!(first.valtree.try_to_leaf().unwrap().to_bits(1), 7);
        let crate::consts::ConstKind::Value(array) = u.tcx.tys.const_kind(fields[1]) else { panic!() };
        assert!(matches!(u.tcx.tys.kind(array.ty), TyKind::Array(..)));
        let crate::consts::ValTreeKind::Branch(elems) = array.valtree.kind() else { panic!() };
        assert_eq!(elems.len(), 2);
        for (i, expected) in [11, 13].into_iter().enumerate() {
            let crate::consts::ConstKind::Value(value) = u.tcx.tys.const_kind(elems[i]) else { panic!() };
            assert_eq!(value.valtree.try_to_leaf().unwrap().to_bits(2), expected);
        }
        assert!(matches!(query("BAD"), Err(ErrorHandled::Reported(_))));
        assert_eq!(query("AFTER").unwrap().try_to_leaf().unwrap().to_bits(8), 2);
        drop(u);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }
}
