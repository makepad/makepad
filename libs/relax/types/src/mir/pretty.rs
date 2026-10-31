//! Deterministic dumps following rustc_middle/src/mir/pretty.rs:380,535.
//! Type names use Relax's existing typeck::ty_to_string, which also needs Program.
use super::*;
use crate::program::Program;
use crate::tcx::Tcx;
use std::fmt::Write;

pub fn print_body(prog: &Program, tcx: &Tcx, name: &str, body: &Body) -> String {
    Printer { prog, tcx }.body(name, body)
}
struct Printer<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
}
impl Printer<'_> {
    fn ty(&self, ty: TyId) -> String {
        crate::typeck::ty_to_string(false, self.prog, self.tcx, ty)
    }
    fn generic_args(&self, args: &[crate::consts::GenericArg]) -> String {
        let mut s = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i != 0 { s.push_str(", "); }
            match *arg {
                crate::consts::GenericArg::Type(ty) => s.push_str(&self.ty(ty)),
                crate::consts::GenericArg::Lifetime(r) => write!(s, "{:?}", self.tcx.tys.region_kind(r)).unwrap(),
                crate::consts::GenericArg::Const(c) => s.push_str(&self.ty(self.tcx.tys.intern(crate::types::TyKind::Const(c)))),
            }
        }
        s
    }
    fn body(&self, name: &str, body: &Body) -> String {
        let mut s = format!("fn {name}(");
        for i in 1..=body.arg_count as usize {
            if i != 1 {
                s.push_str(", ");
            }
            let ty = body.local_decls.get(i).map(|d| self.ty(d.ty)).unwrap_or_else(|| "<missing>".into());
            write!(s, "_{i}: {ty}").unwrap();
        }
        let ret = body.local_decls.first().map(|d| self.ty(d.ty)).unwrap_or_else(|| "<missing>".into());
        writeln!(s, ") -> {ret} {{").unwrap();
        for (i, decl) in body.local_decls.iter().enumerate() {
            if i != 0 && i <= body.arg_count as usize {
                continue;
            }
            writeln!(s, "    let {}_{i}: {};", if decl.mutability == Mutability::Mut { "mut " } else { "" }, self.ty(decl.ty)).unwrap();
        }
        for (i, annotation) in body.user_type_annotations.iter().enumerate() {
            writeln!(s, "    user_ty#{i}: {:?} // inferred {}", annotation.user_ty.value.kind, self.ty(annotation.inferred_ty)).unwrap();
        }
        for (i, upvar) in body.upvars.iter().enumerate() {
            writeln!(s, "    upvar#{i}: {} {:?} {} // {}", upvar.name, upvar.capture, self.ty(upvar.ty), if upvar.mutability == Mutability::Mut { "mut" } else { "imm" }).unwrap();
        }
        for debug in &body.var_debug_info {
            writeln!(s, "    debug {} => {};", debug.name, self.place(&debug.place)).unwrap();
        }
        for (i, bb) in body.basic_blocks.iter().enumerate() {
            writeln!(s, "\n    bb{i}{}: {{", if bb.is_cleanup { " (cleanup)" } else { "" }).unwrap();
            for stmt in &bb.statements {
                writeln!(s, "        {};", self.statement(&stmt.kind)).unwrap();
            }
            match &bb.terminator {
                Some(term) => writeln!(s, "        {};", self.terminator(&term.kind)).unwrap(),
                None => writeln!(s, "        <missing terminator>;").unwrap(),
            }
            s.push_str("    }\n");
        }
        s.push_str("}\n");
        for (i, promoted) in body.promoted.iter().enumerate() {
            s.push('\n');
            s.push_str(&self.body(&format!("{name}::promoted[{i}]"), promoted));
        }
        s
    }
    fn place(&self, place: &Place) -> String {
        let mut s = format!("_{}", place.local.0);
        for elem in &place.projection {
            s = match elem {
                PlaceElem::Deref => format!("(*{s})"),
                PlaceElem::Field(field, ty) => format!("({s}.{}: {})", field.0, self.ty(*ty)),
                PlaceElem::Index(local) => format!("{s}[_{}]", local.0),
                PlaceElem::ConstantIndex { offset, min_length, from_end } => {
                    format!("{s}[{}{offset} of {min_length}]", if *from_end { "-" } else { "" })
                }
                PlaceElem::Subslice { from, to, from_end } => format!("{s}[{from}..{}{to}]", if *from_end { "-" } else { "" }),
                PlaceElem::Downcast(name, variant) => format!(
                    "({s} as {})",
                    match name {
                        Some(name) => self.prog.syms.str(*name).to_string(),
                        None => format!("variant#{}", variant.0),
                    }
                ),
                PlaceElem::OpaqueCast(ty) => format!("({s} as opaque {})", self.ty(*ty)),
                PlaceElem::Subtype(ty) => format!("({s} as subtype {})", self.ty(*ty)),
            };
        }
        s
    }
    fn constant(&self, c: &Const) -> String {
        match c {
            Const::Ty(ty, ct) => format!("{}: {}", self.ty(self.tcx.tys.intern(crate::types::TyKind::Const(*ct))), self.ty(*ty)),
            Const::Unevaluated { def, args, promoted, ty } => {
                let mut s = format!("def#{}::<{}>", def.0, self.generic_args(args));
                if let Some(id) = promoted {
                    write!(s, "::promoted[{}]", id.0).unwrap();
                }
                write!(s, ": {}", self.ty(*ty)).unwrap();
                s
            }
            Const::Val(value, ty) => {
                let value = match value {
                    ConstValue::Scalar(Scalar::Int { bits, size }) => format!("0x{bits:x}:{size}B"),
                    ConstValue::Scalar(Scalar::Ptr { provenance, offset }) => {
                        let flags = if provenance.immutable() { "<immutable>" } else if provenance.shared_ref() { "<shared>" } else { "" };
                        format!("alloc{}+{offset}{flags}", provenance.alloc.0)
                    },
                    ConstValue::ZeroSized => "zst".into(),
                    ConstValue::Slice { alloc, meta } => format!("slice(alloc{}, {meta})", alloc.0),
                    ConstValue::Indirect { alloc, offset } => format!("indirect(alloc{}+{offset})", alloc.0),
                };
                format!("{value}: {}", self.ty(*ty))
            }
        }
    }
    fn operand(&self, op: &Operand) -> String {
        match op {
            Operand::Copy(place) => format!("copy {}", self.place(place)),
            Operand::Move(place) => format!("move {}", self.place(place)),
            Operand::RuntimeChecks(checks) => format!("{checks:?}"),
            Operand::Constant(c) => format!("const {}", self.constant(&c.const_)),
        }
    }
    fn operands(&self, ops: &[Operand]) -> String {
        let mut s = String::new();
        for (i, op) in ops.iter().enumerate() {
            if i != 0 {
                s.push_str(", ");
            }
            s.push_str(&self.operand(op));
        }
        s
    }
    fn aggregate(&self, kind: &AggregateKind) -> String {
        match kind {
            AggregateKind::Array(ty) => format!("Array<{}>", self.ty(*ty)),
            AggregateKind::Tuple => "Tuple".into(),
            AggregateKind::Adt(def, variant, args, user, field) => {
                format!("Adt(def#{}, variant#{}, <{}>, user={user:?}, field={field:?})", def.0, variant.0, self.generic_args(args))
            }
            AggregateKind::Closure(id, args) => format!("Closure({}:{}, <{}>)", id.file, id.expr, self.generic_args(args)),
            AggregateKind::Coroutine(id, args) => format!("Coroutine({}:{}, <{}>)", id.file, id.expr, self.generic_args(args)),
            AggregateKind::CoroutineClosure(id, args) => format!("CoroutineClosure({}:{}, <{}>)", id.file, id.expr, self.generic_args(args)),
            AggregateKind::RawPtr(ty, mutable) => format!("RawPtr({}, {mutable:?})", self.ty(*ty)),
        }
    }
    fn rvalue(&self, rv: &Rvalue) -> String {
        match rv {
            Rvalue::Use(op) => self.operand(op),
            Rvalue::Repeat(op, len) => format!("[{}; {}]", self.operand(op), self.constant(len)),
            Rvalue::Ref(region, kind, place) => format!("&{:?} {kind:?} {}", region, self.place(place)),
            Rvalue::ThreadLocalRef(def) => format!("ThreadLocalRef(def#{})", def.0),
            Rvalue::RawPtr(kind, place) => format!("&raw {kind:?} {}", self.place(place)),
            Rvalue::Cast(kind, op, ty) => format!("{} as {} ({kind:?})", self.operand(op), self.ty(*ty)),
            Rvalue::BinaryOp(op, args) => format!("{op:?}({}, {})", self.operand(&args.0), self.operand(&args.1)),
            Rvalue::UnaryOp(op, arg) => format!("{op:?}({})", self.operand(arg)),
            Rvalue::Discriminant(place) => format!("discriminant({})", self.place(place)),
            Rvalue::Aggregate(kind, ops) => format!("{}({})", self.aggregate(kind), self.operands(ops)),
            Rvalue::CopyForDeref(place) => format!("CopyForDeref({})", self.place(place)),
        }
    }
    fn user_projection(&self, user: &UserTypeProjection) -> String {
        let mut s = format!("user_ty#{}", user.base.0);
        for elem in &user.projection {
            s = match elem {
                UserTypeProjectionElem::Deref => format!("(*{s})"),
                UserTypeProjectionElem::Field(field) => format!("({s}.{})", field.0),
                UserTypeProjectionElem::Index => format!("{s}[_]"),
                UserTypeProjectionElem::ConstantIndex { offset, min_length, from_end } => {
                    format!("{s}[{}{offset} of {min_length}]", if *from_end { "-" } else { "" })
                }
                UserTypeProjectionElem::Subslice { from, to, from_end } => format!("{s}[{from}..{}{to}]", if *from_end { "-" } else { "" }),
                UserTypeProjectionElem::Downcast(name, variant) => match name {
                    Some(name) => format!("({s} as {})", self.prog.syms.str(*name)),
                    None => format!("({s} as variant#{})", variant.0),
                },
                UserTypeProjectionElem::OpaqueCast => format!("({s} as opaque)"),
                UserTypeProjectionElem::Subtype => format!("({s} as subtype)"),
            };
        }
        s
    }
    fn statement(&self, stmt: &StatementKind) -> String {
        match stmt {
            StatementKind::Assign(place, rv) => format!("{} = {}", self.place(place), self.rvalue(rv)),
            StatementKind::FakeRead(cause, place) => format!("FakeRead({cause:?}, {})", self.place(place)),
            StatementKind::SetDiscriminant { place, variant_index } => format!("discriminant({}) = {}", self.place(place), variant_index.0),
            StatementKind::StorageLive(local) => format!("StorageLive(_{})", local.0),
            StatementKind::StorageDead(local) => format!("StorageDead(_{})", local.0),
            StatementKind::PlaceMention(place) => format!("PlaceMention({})", self.place(place)),
            StatementKind::AscribeUserType(place, user, variance) => {
                format!("AscribeUserType({}, {}, {variance:?})", self.place(place), self.user_projection(user))
            }
            StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(op)) => format!("assume({})", self.operand(op)),
            StatementKind::Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping { src, dst, count }) => {
                format!("copy_nonoverlapping({}, {}, {})", self.operand(src), self.operand(dst), self.operand(count))
            }
            StatementKind::ConstEvalCounter => "ConstEvalCounter".into(),
            StatementKind::Nop => "nop".into(),
            StatementKind::BackwardIncompatibleDropHint { place, reason } => format!("BackwardIncompatibleDropHint({}, {reason:?})", self.place(place)),
        }
    }
    fn unwind(&self, unwind: UnwindAction) -> String {
        match unwind {
            UnwindAction::Continue => "unwind continue".into(),
            UnwindAction::Unreachable => "unwind unreachable".into(),
            UnwindAction::Terminate(reason) => format!("unwind terminate({reason:?})"),
            UnwindAction::Cleanup(bb) => format!("unwind: bb{}", bb.0),
        }
    }
    fn assert_msg(&self, msg: &AssertKind) -> String {
        match msg {
            AssertKind::BoundsCheck { len, index } => format!("BoundsCheck({}, {})", self.operand(len), self.operand(index)),
            AssertKind::Overflow(op, a, b) => format!("Overflow({op:?}, {}, {})", self.operand(a), self.operand(b)),
            AssertKind::OverflowNeg(op) => format!("OverflowNeg({})", self.operand(op)),
            AssertKind::DivisionByZero(op) => format!("DivisionByZero({})", self.operand(op)),
            AssertKind::RemainderByZero(op) => format!("RemainderByZero({})", self.operand(op)),
            AssertKind::ResumedAfterReturn(kind) => format!("ResumedAfterReturn({kind:?})"),
            AssertKind::ResumedAfterPanic(kind) => format!("ResumedAfterPanic({kind:?})"),
            AssertKind::MisalignedPointerDereference { required, found } => {
                format!("MisalignedPointerDereference({}, {})", self.operand(required), self.operand(found))
            }
            AssertKind::NullPointerDereference => "NullPointerDereference".into(),
        }
    }
    fn terminator(&self, term: &TerminatorKind) -> String {
        match term {
            TerminatorKind::Goto { target } => format!("goto -> bb{}", target.0),
            TerminatorKind::SwitchInt { discr, targets } => {
                let mut s = format!("switchInt({}) -> [", self.operand(discr));
                for (i, target) in targets.targets.iter().enumerate() {
                    match targets.values.get(i) {
                        Some(value) => write!(s, "{value}: bb{}, ", target.0).unwrap(),
                        None => write!(s, "<missing value>: bb{}, ", target.0).unwrap(),
                    }
                }
                write!(s, "otherwise: bb{}]", targets.otherwise.0).unwrap();
                s
            }
            TerminatorKind::UnwindResume => "resume".into(),
            TerminatorKind::UnwindTerminate(reason) => format!("terminate({reason:?})"),
            TerminatorKind::Return => "return".into(),
            TerminatorKind::Unreachable => "unreachable".into(),
            TerminatorKind::Drop { place, target, unwind, replace } => format!(
                "drop({}) -> [return: bb{}, {}]{}",
                self.place(place),
                target.0,
                self.unwind(*unwind),
                if *replace { " // replace" } else { "" }
            ),
            TerminatorKind::Call { func, args, destination, target, unwind, .. } => format!(
                "{} = {}({}) -> [return: {}, {}]",
                self.place(destination),
                self.operand(func),
                self.operands(args),
                match target {
                    Some(bb) => format!("bb{}", bb.0),
                    None => "unreachable".into(),
                },
                self.unwind(*unwind)
            ),
            TerminatorKind::TailCall { func, args, .. } => format!("tailcall {}({})", self.operand(func), self.operands(args)),
            TerminatorKind::Assert { cond, expected, msg, target, unwind } => format!(
                "assert({}{}, {}) -> [success: bb{}, {}]",
                if *expected { "" } else { "!" },
                self.operand(cond),
                self.assert_msg(msg),
                target.0,
                self.unwind(*unwind)
            ),
            TerminatorKind::Yield { value, resume, resume_arg, drop } => format!(
                "yield({}) -> [resume: bb{}, {}{}]",
                self.operand(value),
                resume.0,
                self.place(resume_arg),
                match drop {
                    Some(bb) => format!(", drop: bb{}", bb.0),
                    None => String::new(),
                }
            ),
            TerminatorKind::CoroutineDrop => "coroutine_drop".into(),
            TerminatorKind::FalseEdge { real_target, imaginary_target } => {
                format!("falseEdge -> [real: bb{}, imaginary: bb{}]", real_target.0, imaginary_target.0)
            }
            TerminatorKind::FalseUnwind { real_target, unwind } => {
                format!("falseUnwind -> [real: bb{}, {}]", real_target.0, self.unwind(*unwind))
            }
            TerminatorKind::InlineAsm(_) => "asm!(<unsupported>)".into(),
        }
    }
}
