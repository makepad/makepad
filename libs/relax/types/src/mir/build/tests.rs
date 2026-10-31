use super::cfg::{BlockAndExtension, Cfg};
use super::thir::{FieldPat, Pat, PatKind};
use super::*;
use crate::mir::{BindingMode, ByRef, FieldIdx, Local, Mutability, Place, SourceInfo, TerminatorKind, VariantIdx};
use crate::program::Sym;
use crate::tcx::Tcx;

#[test]
fn cfg_blocks_terminate_once_and_cleanup_is_marked() {
    let mut cfg = Cfg::default();
    let (a, b) = (cfg.start_new_block(), cfg.start_new_cleanup_block());
    assert!(!cfg.block_data(a).is_cleanup && cfg.block_data(b).is_cleanup);
    cfg.push_storage_live(a, SourceInfo::default(), Local(1));
    cfg.goto(a, SourceInfo::default(), b);
    assert!(matches!(cfg.block_data(a).terminator.as_ref().unwrap().kind, TerminatorKind::Goto { target } if target == b));
    assert_eq!(cfg.block_data(a).statements.len(), 1);
    let super::cfg::BlockAnd(block, ()) = a.unit();
    assert_eq!(block, a);
}

#[test]
fn place_builder_projects_in_order() {
    let tcx = Tcx::new();
    let builder = PlaceBuilder::from(Local(1)).deref().field(FieldIdx(2), tcx.tys.i32_).downcast(None, VariantIdx(1)).index(Local(3));
    assert_eq!(builder.base(), PlaceBase::Local(Local(1)));
    assert_eq!(builder.projection().len(), 4);
    let place = Place::local(Local(1));
    assert_eq!(PlaceBuilder::from(place.clone()), PlaceBuilder::from(Local(1)));
}

#[test]
fn pat_walk_visits_subpatterns_and_bindings() {
    let tcx = Tcx::new();
    let sp = crate::mir::Span::default();
    let t = tcx.tys.i32_;
    let wild = |ty| Pat { ty, span: sp, extra: None, kind: PatKind::Wild };
    let binding = Pat { ty: t, span: sp, extra: None, kind: PatKind::Binding {
        name: Sym::default(), mode: BindingMode(ByRef::No, Mutability::Not), var: thir::LocalVarId(7), ty: t, subpattern: None, is_primary: true, is_shorthand: false } };
    let leaf = Pat { ty: t, span: sp, extra: None, kind: PatKind::Leaf { subpatterns: vec![
        FieldPat { field: FieldIdx(0), pattern: wild(t) }, FieldPat { field: FieldIdx(1), pattern: binding.clone() }] } };
    let mut count = 0;
    leaf.walk_always(|_| count += 1);
    assert_eq!(count, 3);
    let mut bindings = 0;
    leaf.each_binding(|_, by_ref, _, _| { assert_eq!(by_ref, ByRef::No); bindings += 1 });
    assert_eq!(bindings, 1);
    assert!(binding.simple_ident().is_some() && leaf.simple_ident().is_none());
    let never = Pat { ty: t, span: sp, extra: None, kind: PatKind::Or { pats: vec![Pat { ty: t, span: sp, extra: None, kind: PatKind::Never }].into() } };
    assert!(never.is_never_pattern() && !leaf.is_never_pattern());
}

#[test]
fn drop_tree_links_exits_through_shared_drops() {
    use super::scope::{DropData, DropKind, DropTree, ExitScopes, ROOT_NODE};
    let mut cfg = Cfg::default();
    let (a, b, root) = (cfg.start_new_block(), cfg.start_new_block(), cfg.start_new_block());
    let info = SourceInfo::default();
    for blk in [a, b] {
        cfg.terminate(blk, info, TerminatorKind::UnwindResume);
    }
    let mut tree = DropTree::new();
    let value = DropData { source_info: info, local: Local(2), kind: DropKind::Value };
    let storage = DropData { source_info: info, local: Local(3), kind: DropKind::Storage };
    let d1 = tree.add_drop(value, ROOT_NODE);
    assert_eq!(tree.add_drop(value, ROOT_NODE), d1, "identical (next, local) nodes are shared");
    let d2 = tree.add_drop(storage, d1);
    tree.add_entry_point(a, d2); // a: StorageDead(_3) then Drop(_2)
    tree.add_entry_point(b, d1); // b: Drop(_2) only
    let blocks = tree.build_mir::<ExitScopes>(&mut cfg, Some(root)).unwrap();
    let (blk2, blk1) = (blocks[d2].unwrap(), blocks[d1].unwrap());
    assert!(matches!(cfg.terminator(a).kind, TerminatorKind::Goto { target } if target == blk2));
    assert!(matches!(cfg.terminator(b).kind, TerminatorKind::Goto { target } if target == blk1));
    assert!(matches!(cfg.terminator(blk1).kind, TerminatorKind::Drop { target, .. } if target == root));
    assert!(matches!(cfg.terminator(blk2).kind, TerminatorKind::Goto { target } if target == blk1));
    assert!(matches!(cfg.block_data(blk2).statements[0].kind, crate::mir::StatementKind::StorageDead(Local(3))));
}
