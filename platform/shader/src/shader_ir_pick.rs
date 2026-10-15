//! Vulkan pick variants built as typed IR, alongside the existing Metal pick semantics:
//! coverage/discard, 24-bit IDs, sampled ID twins and the five neighbouring texels.
use super::*;

fn cover(b: &mut Bld, color: ExprId) -> ExprId {
    let mut w = b.lane(color, 0);
    for lane in 1..4 { let v = b.lane(color, lane); w = b.builtin(Builtin::Max, vec![w, v], TY_F32); }
    w
}

fn params(b: &mut Bld, block: GlobalId, vec4u: TyId) -> ExprId {
    let global = b.glob(block); b.e(ExprKind::Field(global, 0), vec4u)
}

pub(super) fn apply(output: &ShaderOutput, ir: &mut IrModule, info: &mut IrDrawInfo) -> Result<(), String> {
    let fragment = ir.function_by_name("fragment_main").ok_or("pick shader has no fragment entry")?;
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let vec2 = ir.vec_ty(2, IrScalar::F32);
    let vec4u = ir.vec_ty(4, IrScalar::U32);
    let next = ir.globals.iter().filter_map(|g| g.binding).max().unwrap_or(2).checked_add(1).ok_or("pick binding overflow")?;
    let block_type = ir.synthetic_struct("MpPickUniforms", vec![IrField { name: id!(values), ty: vec4u }]);
    let block = ir.global(IrGlobalKind::Io(IrIo::UniformBuffer), id!(_mp_pick_params), block_type);
    ir.globals[block as usize].binding = Some(next);
    info.pick_binding = Some(next); info.pick_texture_base = Some(next + 1);
    let weight = ir.global(IrGlobalKind::Io(IrIo::Uniform), id!(_mp_pick_weight), TY_F32);
    let selected = ir.global(IrGlobalKind::Io(IrIo::Uniform), id!(_mp_pick_selected), TY_U32);
    let textures: Vec<_> = ir.globals.iter().enumerate().filter_map(|(index, global)| match global.kind {
        IrGlobalKind::Io(IrIo::Texture(TextureType::Texture2d | TextureType::TextureVideo)) =>
            global.binding.map(|binding| (index as u32, binding - info.texture_binding_base)),
        _ => None,
    }).collect();
    let original_functions = ir.functions.len();
    let mut helpers = std::collections::HashMap::new();
    for (texture, slot) in textures {
        if slot >= u32::BITS || next.checked_add(1 + slot).is_none() {
            return Err("pick texture index exceeds its u32 mask/binding representation".into());
        }
        let ty = ir.ty(IrTy::Texture(TextureType::Texture2d));
        let name = LiveId::from_str(&format!("_mp_pick_twin_{slot}"));
        let twin = ir.global(IrGlobalKind::Io(IrIo::Texture(TextureType::Texture2d)), name, ty);
        ir.globals[twin as usize].binding = Some(next + 1 + slot);
        for pixel in [false, true] {
            let helper = sample_helper(ir, twin, slot, pixel, block, selected, weight);
            let id = ir.functions.len() as FuncId; ir.functions.push(helper); helpers.insert((texture, pixel), id);
        }
    }
    // Sample expressions keep their original color result, but update the private winning ID.
    for f in &mut ir.functions[..original_functions] {
        let count = f.exprs.len();
        for at in 0..count {
            let ExprKind::Sample(sample) = &f.exprs[at].kind else { continue };
            if f.exprs[at].ty != vec4 || f.ty_of(sample.coord) != vec2 { continue; }
            let ExprKind::Global(texture) = f.exprs[sample.tex as usize].kind else { continue };
            let pixel = output.samplers.get(sample.sampler as usize).is_some_and(|s| s.coord == SamplerCoord::Pixel);
            let Some(helper) = helpers.get(&(texture, pixel)) else { continue };
            let coord = sample.coord;
            let color = f.expr(f.exprs[at].kind.clone(), vec4);
            f.exprs[at].kind = ExprKind::Call(*helper, vec![color, coord]);
        }
    }
    let function = std::mem::replace(&mut ir.functions[fragment as usize], IrFunction::new(String::new(), TY_VOID));
    let mut b = Bld { f: function, m: ir };
    // The original color assignments are evaluated for coverage but replaced at the outputs.
    let mut first_color = None;
    b.f.body.retain(|stmt| match stmt {
        Stmt::Assign(place, value) if matches!(b.f.exprs[*place as usize].kind, ExprKind::EntryOut(_)) => {
            if first_color.is_none() { first_color = Some(*value); } false
        }
        Stmt::Return(_) => false,
        _ => true,
    });
    let zero = b.u32(0); let id = b.glob(selected);
    let zero_f = b.f32(0.0); let w = b.glob(weight);
    b.f.body.splice(0..0, [Stmt::Assign(id, zero), Stmt::Assign(w, zero_f)]);
    let data = params(&mut b, block, vec4u); let draw_id = b.lane(data, 0);
    let w = b.glob(weight); let zero = b.f32(0.0); let has = b.bin(BinOp::Gt, w, zero, TY_BOOL);
    let child = b.glob(selected); let pid = b.e(ExprKind::Select(has, child, draw_id), TY_U32);
    let c = if let Some(c) = first_color.filter(|c| b.f.ty_of(*c) == vec4) { cover(&mut b, c) } else { b.f32(1.0) };
    let threshold = b.f32(0.1); let transparent = b.bin(BinOp::Lt, c, threshold, TY_BOOL);
    let zero = b.u32(0); let empty = b.bin(BinOp::Eq, pid, zero, TY_BOOL);
    let discard = b.bin(BinOp::LogicOr, transparent, empty, TY_BOOL);
    b.f.body.push(Stmt::If(discard, vec![Stmt::Discard], Vec::new()));
    let mut lanes = Vec::new();
    for shift in [0, 8, 16] {
        let n = b.u32(shift); let shifted = b.bin(BinOp::Shr, pid, n, TY_U32);
        let mask = b.u32(255); let byte = b.bin(BinOp::BitAnd, shifted, mask, TY_U32);
        let value = b.convert(byte, TY_F32); let scale = b.f32(255.0); lanes.push(b.bin(BinOp::Div, value, scale, TY_F32));
    }
    lanes.push(b.f32(1.0)); let color = b.e(ExprKind::Construct(lanes), vec4);
    let outputs = b.f.entry.as_ref().unwrap().outputs.len();
    for i in 0..outputs {
        b.f.entry.as_mut().unwrap().outputs[i].ty = vec4;
        let dst = b.e(ExprKind::EntryOut(i as u32), vec4); b.assign(dst, color);
    }
    b.f.body.push(Stmt::Return(NO_EXPR));
    let function = b.f; ir.functions[fragment as usize] = function;
    Ok(())
}

fn sample_helper(ir: &mut IrModule, twin: GlobalId, slot: u32, pixel: bool, block: GlobalId, selected: GlobalId, weight: GlobalId) -> IrFunction {
    let vec4 = ir.vec_ty(4, IrScalar::F32); let vec2 = ir.vec_ty(2, IrScalar::F32);
    let vec2u = ir.vec_ty(2, IrScalar::U32); let vec2i = ir.vec_ty(2, IrScalar::I32); let vec4u = ir.vec_ty(4, IrScalar::U32);
    let mut b = Bld { f: IrFunction::new(format!("_mp_pick_sample_{slot}_{}", u32::from(pixel)), vec4), m: ir };
    b.f.params = vec![IrParam { name: id!(color), shadow: 0, ty: vec4, inout: false }, IrParam { name: id!(uv), shadow: 0, ty: vec2, inout: false }];
    let color = b.e(ExprKind::Param(0), vec4); let uv = b.e(ExprKind::Param(1), vec2);
    let coverage = cover(&mut b, color);
    let pk = params(&mut b, block, vec4u); let mask = b.lane(pk, 1); let bit = b.u32(1 << slot);
    let bit = b.bin(BinOp::BitAnd, mask, bit, TY_U32); let zero = b.u32(0); let absent = b.bin(BinOp::Eq, bit, zero, TY_BOOL);
    let previous = b.glob(weight); let smaller = b.bin(BinOp::Le, coverage, previous, TY_BOOL);
    let skip = b.bin(BinOp::LogicOr, absent, smaller, TY_BOOL); b.f.body.push(Stmt::If(skip, vec![Stmt::Return(color)], Vec::new()));
    let texture = b.glob(twin); let size = b.e(ExprKind::TexSize(texture), vec2u);
    let mut coords = Vec::new(); let mut highs = Vec::new();
    for lane in 0..2 {
        let size = b.lane(size, lane); let size_f = b.convert(size, TY_F32);
        let coord = b.lane(uv, lane); let scaled = if pixel { coord } else { b.bin(BinOp::Mul, coord, size_f, TY_F32) };
        coords.push(b.convert(scaled, TY_I32)); let size = b.convert(size, TY_I32); let one = b.i32(1); highs.push(b.bin(BinOp::Sub, size, one, TY_I32));
    }
    let local = b.f.local(id!(_mp_id), 0, TY_U32, true, LocalKind::Temp); let zero = b.u32(0); b.f.body.push(Stmt::Local(local, zero));
    for offset in [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]] {
        let mut xy = Vec::new();
        for lane in 0..2 { let delta = b.i32(offset[lane]); let x = b.bin(BinOp::Add, coords[lane], delta, TY_I32); let zero = b.i32(0);
            xy.push(b.builtin(Builtin::Clamp, vec![x, zero, highs[lane]], TY_I32)); }
        let xy = b.e(ExprKind::Construct(xy), vec2i); let lod = b.i32(0); let encoded = b.e(ExprKind::TexLoad(texture, xy, NO_EXPR, lod), vec4);
        let mut id = b.u32(0);
        for lane in 0..3 {
            let c = b.lane(encoded, lane); let scale = b.f32(255.0); let byte = b.bin(BinOp::Mul, c, scale, TY_F32); let half = b.f32(0.5);
            let byte = b.bin(BinOp::Add, byte, half, TY_F32); let byte = b.convert(byte, TY_U32); let shift = b.u32(lane as u32 * 8);
            let byte = b.bin(BinOp::Shl, byte, shift, TY_U32); id = b.bin(BinOp::BitOr, id, byte, TY_U32);
        }
        let place = b.e(ExprKind::Local(local), TY_U32); let zero = b.u32(0); let empty = b.bin(BinOp::Eq, place, zero, TY_BOOL);
        b.f.body.push(Stmt::If(empty, vec![Stmt::Assign(place, id)], Vec::new()));
    }
    let chosen = b.e(ExprKind::Local(local), TY_U32); let zero = b.u32(0); let found = b.bin(BinOp::Ne, chosen, zero, TY_BOOL);
    let dst = b.glob(selected); let dst_w = b.glob(weight);
    b.f.body.push(Stmt::If(found, vec![Stmt::Assign(dst, chosen), Stmt::Assign(dst_w, coverage)], Vec::new()));
    b.f.body.push(Stmt::Return(color)); b.f
}
