//! Lower intrinsic calls to the ordinary MIR operations used by CTFE.
use crate::mir::*;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::TyKind;

/// Port of rustc_mir_transform/src/lower_intrinsics.rs:13-339.
/// Intrinsic identity follows const_eval/interp/exec.rs:509-520. Relax's
/// assignments and intrinsics are unboxed, and Use has no WithRetag argument.
pub fn lower_intrinsics(prog: &Program, tcx: &Tcx, body: &mut Body) {
    for i in 0..body.basic_blocks.len() {
        let term = body.basic_blocks[i].terminator.as_ref().expect("missing terminator");
        let TerminatorKind::Call { func, args, destination, target, .. } = &term.kind else { continue; };
        let func_ty = match func {
            Operand::Copy(place) | Operand::Move(place) => place.ty(body, tcx).expect("invalid callee place").ty,
            Operand::Constant(c) => c.const_.ty(&tcx.tys),
            Operand::RuntimeChecks(_) => continue,
        };
        let TyKind::FnDef(def, generic_args, _) = *tcx.tys.kind(func_ty) else { continue; };
        let path = prog.def_path(def);
        if !path.contains("intrinsic_decls::") && !path.starts_with("core::intrinsics::") { continue; }
        let name = prog.name(def);
        let source_info = term.source_info;
        let assign = |rv| StatementKind::Assign(destination.clone(), rv);
        let mut next = *target;
        let statement = match name {
            // lower_intrinsics.rs:23-25.
            "unreachable" => { next = None; None }
            // lower_intrinsics.rs:26-42.
            "ub_checks" | "overflow_checks" | "contract_checks" => {
                let checks = match name {
                    "ub_checks" => RuntimeChecks::UbChecks,
                    "overflow_checks" => RuntimeChecks::OverflowChecks,
                    _ => RuntimeChecks::ContractChecks,
                };
                Some(assign(Rvalue::Use(Operand::RuntimeChecks(checks))))
            }
            // lower_intrinsics.rs:43-60.
            "forget" => Some(assign(Rvalue::Use(Operand::Constant(Box::new(ConstOperand {
                span: source_info.span,
                user_ty: None,
                const_: Const::Val(ConstValue::ZeroSized, tcx.tys.unit),
            }))))),
            // lower_intrinsics.rs:61-92.
            "copy_nonoverlapping" => {
                let [src, dst, count] = args.as_slice() else { panic!("wrong arguments for copy_nonoverlapping"); };
                Some(StatementKind::Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping {
                    src: src.clone(), dst: dst.clone(), count: count.clone(),
                }))
            }
            "assume" => {
                let [arg] = args.as_slice() else { panic!("wrong arguments for assume"); };
                Some(StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(arg.clone())))
            }
            // lower_intrinsics.rs:93-150,203-219. Offset uses the same binary form.
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "three_way_compare"
            | "unchecked_add" | "unchecked_sub" | "unchecked_mul" | "unchecked_div"
            | "unchecked_rem" | "unchecked_shl" | "unchecked_shr"
            | "add_with_overflow" | "sub_with_overflow" | "mul_with_overflow" | "offset" => {
                let [lhs, rhs] = args.as_slice() else { panic!("wrong arguments for {name}"); };
                let op = match name {
                    "wrapping_add" => BinOp::Add,
                    "wrapping_sub" => BinOp::Sub,
                    "wrapping_mul" => BinOp::Mul,
                    "three_way_compare" => BinOp::Cmp,
                    "unchecked_add" => BinOp::AddUnchecked,
                    "unchecked_sub" => BinOp::SubUnchecked,
                    "unchecked_mul" => BinOp::MulUnchecked,
                    "unchecked_div" => BinOp::Div,
                    "unchecked_rem" => BinOp::Rem,
                    "unchecked_shl" => BinOp::ShlUnchecked,
                    "unchecked_shr" => BinOp::ShrUnchecked,
                    "add_with_overflow" => BinOp::AddWithOverflow,
                    "sub_with_overflow" => BinOp::SubWithOverflow,
                    "mul_with_overflow" => BinOp::MulWithOverflow,
                    "offset" => BinOp::Offset,
                    _ => unreachable!(),
                };
                Some(assign(Rvalue::BinaryOp(op, Box::new((lhs.clone(), rhs.clone())))))
            }
            // lower_intrinsics.rs:151-182. Keep the read even without a return edge.
            "read_via_copy" => {
                let [Operand::Copy(place) | Operand::Move(place)] = args.as_slice() else {
                    panic!("read_via_copy requires a local");
                };
                assert!(place.projection.is_empty(), "read_via_copy requires a local");
                let deref = Place { local: place.local, projection: vec![PlaceElem::Deref] };
                Some(assign(Rvalue::Use(Operand::Copy(deref))))
            }
            // lower_intrinsics.rs:184-202.
            "discriminant_value" => {
                let [Operand::Copy(place) | Operand::Move(place)] = args.as_slice() else {
                    panic!("discriminant_value requires a place");
                };
                let mut place = place.clone();
                place.projection.push(PlaceElem::Deref);
                Some(assign(Rvalue::Discriminant(place)))
            }
            // lower_intrinsics.rs:220-265. TyKind pointer/reference mutability is a bool.
            "slice_get_unchecked" => {
                let [Operand::Copy(place) | Operand::Move(place), Operand::Copy(index) | Operand::Move(index)] = args.as_slice() else {
                    panic!("slice_get_unchecked requires places");
                };
                assert!(!place.projection.contains(&PlaceElem::Deref), "slice pointer must not be indirect");
                assert!(index.projection.is_empty(), "slice index must be a local");
                let mut place = place.clone();
                place.projection.extend([PlaceElem::Deref, PlaceElem::Index(index.local)]);
                let ret_ty = tcx.tys.list(generic_args)[0];
                let rv = match *tcx.tys.kind(ret_ty) {
                    TyKind::Ptr(false, _) => Rvalue::RawPtr(RawPtrKind::Const, place),
                    TyKind::Ptr(true, _) => Rvalue::RawPtr(RawPtrKind::Mut, place),
                    TyKind::Ref(false, _, _) => Rvalue::Ref(Region::ERASED, BorrowKind::Shared, place),
                    TyKind::Ref(true, _, _) => Rvalue::Ref(Region::ERASED,
                        BorrowKind::Mut { kind: MutBorrowKind::Default }, place),
                    _ => panic!("unknown slice_get_unchecked return type"),
                };
                Some(assign(rv))
            }
            // lower_intrinsics.rs:266-290. Do not use MirPatch: its Unreachable
            // replacement clears statements, including this required cast.
            "transmute" | "transmute_unchecked" => {
                let [arg] = args.as_slice() else { panic!("wrong arguments for {name}"); };
                let dst_ty = destination.ty(body, tcx).expect("invalid transmute destination").ty;
                Some(assign(Rvalue::Cast(CastKind::Transmute, arg.clone(), dst_ty)))
            }
            // lower_intrinsics.rs:291-317.
            "aggregate_raw_ptr" => {
                let [data, meta] = args.as_slice() else { panic!("wrong arguments for aggregate_raw_ptr"); };
                let pointer_ty = tcx.tys.list(generic_args)[0];
                let TyKind::Ptr(mutable, pointee_ty) = *tcx.tys.kind(pointer_ty) else {
                    panic!("aggregate_raw_ptr must return a raw pointer");
                };
                let kind = AggregateKind::RawPtr(pointee_ty, if mutable { Mutability::Mut } else { Mutability::Not });
                Some(assign(Rvalue::Aggregate(Box::new(kind), vec![data.clone(), meta.clone()])))
            }
            // lower_intrinsics.rs:318-334.
            "ptr_metadata" => {
                let [ptr] = args.as_slice() else { panic!("wrong arguments for ptr_metadata"); };
                Some(assign(Rvalue::UnaryOp(UnOp::PtrMetadata, ptr.clone())))
            }
            _ => continue,
        };
        // Only these three operations may diverge in rustc's lowering.
        if !matches!(name, "unreachable" | "read_via_copy" | "transmute" | "transmute_unchecked") {
            assert!(next.is_some(), "nondiverging intrinsic without a return target");
        }
        let block = &mut body.basic_blocks[i];
        if let Some(kind) = statement { block.statements.push(Statement { source_info, kind }); }
        block.terminator.as_mut().unwrap().kind = match next {
            Some(target) => TerminatorKind::Goto { target },
            None => TerminatorKind::Unreachable,
        };
    }
}
