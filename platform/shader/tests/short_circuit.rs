use makepad_shader::makepad_live_id::{id, LiveId};
use makepad_shader::shader_ir::*;
use makepad_shader::shader_ir_spirv::ir_to_spirv;
use makepad_shader::shader_ir_text::{Dialect, TextCx};
use makepad_shader::spirv::*;

fn module(op: BinOp, read: bool, nested: bool) -> IrModule {
    let mut m = IrModule::new();
    let array = m.ty(IrTy::Array(TY_F32, 0));
    m.globals.push(IrGlobal { kind: IrGlobalKind::Io(IrIo::Storage(false)), name: id!(buf), ty: array, binding: None });
    let mut f = IrFunction::new("guard".into(), TY_BOOL);
    for name in [id!(i), id!(n)] {
        f.params.push(IrParam { name, shadow: 0, ty: TY_U32, inout: false });
    }
    let i = f.expr(ExprKind::Param(0), TY_U32);
    let n = f.expr(ExprKind::Param(1), TY_U32);
    let a = f.expr(ExprKind::Binary(BinOp::Ge, i, n), TY_BOOL);
    let b = if read {
        let buf = f.expr(ExprKind::Global(0), array);
        let load = f.expr(ExprKind::Index(buf, i), TY_F32);
        let zero = f.expr(ExprKind::Lit(IrLit::F32(0.0)), TY_F32);
        f.expr(ExprKind::Binary(BinOp::Gt, load, zero), TY_BOOL)
    } else { f.expr(ExprKind::Binary(BinOp::Lt, i, n), TY_BOOL) };
    let b = if nested { f.expr(ExprKind::Binary(op, a, b), TY_BOOL) } else { b };
    let result = f.expr(ExprKind::Binary(op, a, b), TY_BOOL);
    f.body.push(Stmt::Return(result));
    m.functions.push(f);
    let mut entry = IrFunction::new("main".into(), TY_VOID);
    entry.entry = Some(IrEntry { stage: IrStage::Compute, inputs: vec![], outputs: vec![] });
    let i = entry.expr(ExprKind::Lit(IrLit::U32(0)), TY_U32);
    let call = entry.expr(ExprKind::Call(0, vec![i, i]), TY_BOOL);
    entry.body.push(Stmt::Expr(call));
    m.functions.push(entry);
    m
}

fn instructions(words: &[u32]) -> Vec<&[u32]> {
    let mut result = Vec::new();
    let mut at = 5;
    while at < words.len() {
        let len = (words[at] >> 16) as usize;
        assert!(len > 0);
        result.push(&words[at..at + len]);
        at += len;
    }
    result
}

#[test]
fn guarded_reads_and_pure_operands() {
    for op in [BinOp::LogicOr, BinOp::LogicAnd] {
        for (read, nested) in [(false, false), (true, false), (true, true)] {
            let m = module(op, read, nested);
            let words = ir_to_spirv(&m, 1, false).unwrap();
            let inst = instructions(&words);
            let count = |op| inst.iter().filter(|i| i[0] & 0xffff == op).count();
            assert_eq!(count(OP_SELECTION_MERGE), if read { 1 + usize::from(nested) } else { 0 });
            assert_eq!(count(OP_PHI), count(OP_SELECTION_MERGE));
            assert_eq!(count(if op == BinOp::LogicOr { OP_LOGICAL_OR } else { OP_LOGICAL_AND }), usize::from(!read));
            // Every phi parent is an actual predecessor, including nested RHS merges.
            for phi in inst.iter().filter(|i| i[0] & 0xffff == OP_PHI) {
                for parent in [phi[4], phi[6]] {
                    let start = inst.iter().position(|i| i[0] & 0xffff == OP_LABEL && i[1] == parent).unwrap();
                    assert!(inst[start + 1..].iter().take_while(|i| i[0] & 0xffff != OP_LABEL)
                        .any(|i| matches!(i[0] & 0xffff, OP_BRANCH | OP_BRANCH_CONDITIONAL)));
                }
            }
            if read {
                let branch = inst.iter().position(|i| i[0] & 0xffff == OP_BRANCH_CONDITIONAL).unwrap();
                let load = inst.iter().position(|i| i[0] & 0xffff == OP_LOAD).unwrap();
                assert!(branch < load, "no resource load may precede its guard");
                let merge = inst[branch - 1][1];
                let skipped_target = inst[branch][if op == BinOp::LogicOr { 2 } else { 3 }];
                assert_eq!(skipped_target, merge, "the short-circuit edge bypasses the RHS");
            }
            for dialect in [Dialect::Hlsl, Dialect::Metal, Dialect::Glsl, Dialect::Wgsl] {
                let mut text = String::new();
                let mut cx = TextCx::new(&m, dialect, false);
                let buffer_read = format!("{}.Load(", cx.global_name(0));
                cx.function(0, &mut text);
                if dialect == Dialect::Hlsl && read {
                    assert!(text.contains("[branch] if"), "{text}");
                    assert!(!text.contains(op.text()), "guarded read must not use FXC's eager logical op: {text}");
                    assert!(text.find("[branch] if").unwrap() < text.find(&buffer_read).unwrap(), "{text}");
                } else {
                    assert!(text.contains(op.text()), "native short-circuit or pure op: {text}");
                }
            }
        }
    }
}

#[test]
fn calls_division_and_vectors() {
    for op in [BinOp::LogicOr, BinOp::LogicAnd] {
        let mut m = module(op, true, false);
        let mut first = IrFunction::new("first".into(), TY_BOOL);
        let yes = first.expr(ExprKind::Lit(IrLit::Bool(true)), TY_BOOL);
        first.body.push(Stmt::Return(yes));
        m.functions.push(first);
        let f = &mut m.functions[0];
        let Stmt::Return(logic) = f.body[0] else { unreachable!() };
        let call = f.expr(ExprKind::Call(2, vec![]), TY_BOOL);
        let eq = f.expr(ExprKind::Binary(BinOp::Eq, call, logic), TY_BOOL);
        f.body[0] = Stmt::Return(eq);
        let mut text = String::new();
        TextCx::new(&m, Dialect::Hlsl, false).function(0, &mut text);
        assert_eq!(text.matches("first(").count(), 1, "{text}");
        assert!(text.find("first(").unwrap() < text.find("[branch] if").unwrap(), "left call must precede RHS branch: {text}");

        // A call or division is guarded even when it has no resource operand.
        for division in [false, true] {
            let f = &mut m.functions[0];
            let rhs = if division {
                let one = f.expr(ExprKind::Lit(IrLit::F32(1.0)), TY_F32);
                let div = f.expr(ExprKind::Binary(BinOp::Div, one, one), TY_F32);
                f.expr(ExprKind::Binary(BinOp::Gt, div, one), TY_BOOL)
            } else { call };
            let a = f.expr(ExprKind::Lit(IrLit::Bool(true)), TY_BOOL);
            let result = f.expr(ExprKind::Binary(op, a, rhs), TY_BOOL);
            f.body[0] = Stmt::Return(result);
            let words = ir_to_spirv(&m, 1, false).unwrap();
            assert!(instructions(&words).iter().any(|i| i[0] & 0xffff == OP_PHI));
            let mut text = String::new();
            TextCx::new(&m, Dialect::Hlsl, false).function(0, &mut text);
            assert!(text.contains("[branch] if"), "{text}");
        }

        // Boolean vectors remain componentwise even for a nontrivial RHS.
        let vector = m.vec_ty(2, IrScalar::Bool);
        let f = &mut m.functions[2];
        f.ret = vector;
        let v = f.expr(ExprKind::Construct(vec![yes; 2]), vector);
        f.body[0] = Stmt::Return(v);
        let f = &mut m.functions[0];
        f.ret = vector;
        let yes = f.expr(ExprKind::Lit(IrLit::Bool(true)), TY_BOOL);
        let v = f.expr(ExprKind::Construct(vec![yes; 2]), vector);
        let rhs = f.expr(ExprKind::Call(2, vec![]), vector);
        let result = f.expr(ExprKind::Binary(op, v, rhs), vector);
        f.body[0] = Stmt::Return(result);
        m.functions[1].exprs[1].ty = vector;
        let words = ir_to_spirv(&m, 1, false).unwrap();
        assert!(!instructions(&words).iter().any(|i| i[0] & 0xffff == OP_PHI));
        let mut text = String::new();
        TextCx::new(&m, Dialect::Hlsl, false).function(0, &mut text);
        assert!(!text.contains("[branch]"), "{text}");
        assert!(text.contains(op.text()), "{text}");
    }
}
