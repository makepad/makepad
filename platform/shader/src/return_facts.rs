use crate::shader_ir::{ExprId, ExprKind, IrFunction, IrLit, IrModule, Stmt, TyId};
use makepad_live_id::LiveId;
use relax_api::relax_api;

/// What one returned component provably is.
#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReturnComponent {
    Const(f32),
    Param { component: u8, name: LiveId },
    Unknown,
}

impl ReturnComponent {
    fn agrees(self, other: Self) -> bool {
        match (self, other) {
            // A shader can distinguish the signs of zero.
            (Self::Const(a), Self::Const(b)) => a.to_bits() == b.to_bits(),
            _ => self == other,
        }
    }
}

/// What a function's returned value provably is, per component, from the IR.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FnReturnFacts {
    pub key_name: LiveId,
    pub components: [ReturnComponent; 4],
    /// Scalar or vector width. Other return types have no component facts.
    pub len: u8,
}

#[derive(Clone, Copy)]
enum LocalInit {
    Undeclared,
    Once(ExprId),
    Unknown,
}

struct Bindings {
    locals: Vec<LocalInit>,
    written_params: Vec<bool>,
}

impl Bindings {
    fn invalidate(&mut self, func: &IrFunction, mut place: ExprId) {
        // For an indexed write only the indexed value is modified.
        loop {
            match func.exprs[place as usize].kind {
                ExprKind::Field(base, _)
                | ExprKind::Swizzle(base, _, _)
                | ExprKind::Index(base, _)
                | ExprKind::Deref(base) => place = base,
                ExprKind::Local(id) => {
                    self.locals[id as usize] = LocalInit::Unknown;
                    return;
                }
                ExprKind::Param(i) => {
                    self.written_params[i as usize] = true;
                    return;
                }
                _ => return,
            }
        }
    }

    fn scan(&mut self, func: &IrFunction, body: &[Stmt], returns: &mut Vec<ExprId>) {
        for stmt in body {
            match *stmt {
                Stmt::Local(id, init) => {
                    let slot = &mut self.locals[id as usize];
                    *slot = match *slot {
                        LocalInit::Undeclared => LocalInit::Once(init),
                        _ => LocalInit::Unknown,
                    };
                }
                Stmt::Assign(place, _) | Stmt::CompoundAssign(place, _, _) => {
                    self.invalidate(func, place);
                }
                Stmt::If(_, ref yes, ref no) => {
                    self.scan(func, yes, returns);
                    self.scan(func, no, returns);
                }
                Stmt::For(id, _, _, ref body) => {
                    self.locals[id as usize] = LocalInit::Unknown;
                    self.scan(func, body, returns);
                }
                Stmt::Loop(ref body) => self.scan(func, body, returns),
                Stmt::Return(value) => returns.push(value),
                _ => {}
            }
        }
    }

    fn component(
        &self,
        func: &IrFunction,
        ir: &IrModule,
        mut value: ExprId,
        mut component: u8,
    ) -> ReturnComponent {
        // Follow just this component through constructors, swizzles and local reads.
        // A void return (NO_EXPR) has no value to follow.
        'value: while let Some(expr) = func.exprs.get(value as usize) {
            match &expr.kind {
                ExprKind::Lit(IrLit::F32(v) | IrLit::F16(v)) => return ReturnComponent::Const(*v),
                ExprKind::Lit(IrLit::AbstractFloat(v)) => return ReturnComponent::Const(*v as f32),
                ExprKind::Param(i) => {
                    let param = &func.params[*i as usize];
                    if param.inout || self.written_params[*i as usize] {
                        break;
                    }
                    return ReturnComponent::Param {
                        name: param.name,
                        component,
                    };
                }
                ExprKind::Local(id) => match self.locals[*id as usize] {
                    LocalInit::Once(init) => value = init,
                    _ => break,
                },
                ExprKind::Swizzle(base, lanes, n) if component < *n => {
                    value = *base;
                    component = lanes[component as usize];
                }
                ExprKind::Construct(args) => {
                    for &arg in args {
                        let n = width(ir, func.ty_of(arg));
                        if n == 0 {
                            break 'value;
                        }
                        if args.len() == 1 && n == 1 {
                            component = 0;
                        }
                        if component < n {
                            value = arg;
                            continue 'value;
                        }
                        component -= n;
                    }
                    break;
                }
                _ => break,
            }
        }
        ReturnComponent::Unknown
    }
}

fn width(ir: &IrModule, ty: TyId) -> u8 {
    if ir.is_abstract(ty) {
        1
    } else {
        ir.lanes(ty) as u8
    }
}

/// Facts shared by all returns of this function. A body without a return has no facts.
pub fn return_facts(func: &IrFunction, ir: &IrModule) -> Option<FnReturnFacts> {
    let mut bindings = Bindings {
        locals: vec![LocalInit::Undeclared; func.locals.len()],
        written_params: vec![false; func.params.len()],
    };
    let mut returns = Vec::new();
    bindings.scan(func, &func.body, &mut returns);
    let (&first, rest) = returns.split_first()?;
    for expr in &func.exprs {
        if let ExprKind::AddrOf(place) = expr.kind {
            bindings.invalidate(func, place);
        }
    }
    let len = width(ir, func.ret);
    let mut components = [ReturnComponent::Unknown; 4];
    for c in 0..len {
        let fact = bindings.component(func, ir, first, c);
        if fact != ReturnComponent::Unknown
            && rest
                .iter()
                .all(|&value| fact.agrees(bindings.component(func, ir, value, c)))
        {
            components[c as usize] = fact;
        }
    }
    Some(FnReturnFacts {
        key_name: func.key_name,
        components,
        len,
    })
}
