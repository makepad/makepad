use {
    crate::{
        data::UnguardedData,
        elem::UnguardedElem,
        error::Error,
        extern_::UnguardedExtern,
        extern_ref::UnguardedExternRef,
        func::{Code, Func, FuncEntity, InstrSlot, UnguardedFunc},
        func_ref::UnguardedFuncRef,
        global::UnguardedGlobal,
        mem::UnguardedMem,
        ops::*,
        stack::{Stack, StackGuard, StackSlot},
        store::{Handle, Store, UnguardedInternedFuncType},
        table::UnguardedTable,
        trap::Trap,
        val::{UnguardedVal, Val},
    },
    std::{hint, mem, ptr},
};

pub(crate) type Instr = unsafe extern "C" fn(Ip, Sp, Md, Ms, Ix, Sx, Dx, Cx) -> ControlFlowBits;

pub(crate) type Ip = *mut InstrSlot;
pub(crate) type Sp = *mut StackSlot;
pub(crate) type Md = *mut u8;
pub(crate) type Ms = u32;
pub(crate) type Ix = u64;
pub(crate) type Sx = f32;
pub(crate) type Dx = f64;
pub(crate) type Cx<'a> = *mut Context<'a>;

#[derive(Debug)]
pub(crate) struct Context<'a> {
    pub(crate) store: &'a mut Store,
    pub(crate) stack: Option<StackGuard>,
    pub(crate) error: Option<Error>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ControlFlow {
    Stop,
    Trap(Trap),
    Error,
}

impl ControlFlow {
    pub(crate) fn from_bits(bits: usize) -> Option<Self> {
        if bits == 0 {
            Some(Self::Stop)
        } else if bits & 0x03 == 2 {
            Trap::from_usize(bits >> 2).map(Self::Trap)
        } else if bits & 0x03 == 3 {
            Some(Self::Error)
        } else {
            None
        }
    }

    pub(crate) fn to_bits(self) -> ControlFlowBits {
        match self {
            Self::Stop => 0,
            Self::Trap(trap) => trap.to_usize() << 2 | 2,
            Self::Error => 3,
        }
    }
}

pub(crate) type ControlFlowBits = usize;

pub(crate) fn exec(
    store: &mut Store,
    func: Func,
    args: &[Val],
    results: &mut [Val],
) -> Result<(), Error> {
    let mut stack = Stack::lock();
    let mut ptr = stack.ptr();
    for arg in args.iter().copied() {
        let arg = arg.to_unguarded(store.id());
        unsafe { arg.write_to_stack(&mut ptr) };
    }
    let type_ = func.type_(store).clone();
    func.compile(store);
    match func.0.as_mut(store) {
        FuncEntity::Wasm(func) => {
            let Code::Compiled(state) = func.code_mut() else {
                panic!();
            };
            let mut trampoline = [
                call as InstrSlot,
                state.code.as_mut_ptr() as InstrSlot,
                type_.callee_stack_slot_count() * mem::size_of::<StackSlot>(),
                stop as InstrSlot,
            ];
            let ptr = stack.ptr();
            let mut context = Context {
                store,
                stack: Some(stack),
                error: None,
            };
            loop {
                match ControlFlow::from_bits(unsafe {
                    next_instr(
                        trampoline.as_mut_ptr(),
                        ptr,
                        ptr::null_mut(),
                        0,
                        0,
                        0.0,
                        0.0,
                        &mut context as *mut _,
                    )
                })
                .unwrap()
                {
                    ControlFlow::Stop => break,
                    ControlFlow::Trap(trap) => {
                        drop(context.stack.take().unwrap());
                        return Err(trap)?;
                    }
                    ControlFlow::Error => {
                        drop(context.stack.take().unwrap());
                        return Err(context.error.take().unwrap());
                    }
                }
            }
            stack = context.stack.take().unwrap();
            stack.set_ptr(ptr);
        }
        FuncEntity::Host(func) => {
            let ptr = stack.ptr();
            stack.set_ptr(unsafe { ptr.add(type_.callee_stack_slot_count()) });
            stack = func.trampoline().clone().call(store, stack)?;
            stack.set_ptr(ptr);
        }
    }
    let mut ptr = stack.ptr();
    for result in results.iter_mut() {
        unsafe {
            *result = Val::from_unguarded(
                UnguardedVal::read_from_stack(&mut ptr, result.type_()),
                store.id(),
            );
        }
    }
    Ok(())
}

macro_rules! r#try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(trap) => return ControlFlow::Trap(trap).to_bits(),
        }
    };
}

// Control instructions

pub(crate) unsafe extern "C" fn unreachable(
    _ip: Ip,
    _sp: Sp,
    _md: Md,
    _ms: Ms,
    _ix: Ix,
    _sx: Sx,
    _dx: Dx,
    _cx: Cx,
) -> ControlFlowBits {
    ControlFlow::Trap(Trap::Unreachable).to_bits()
}

pub(crate) unsafe extern "C" fn br(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let target = *ip.cast();
    let ip = target;
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn br_if_z_s(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (cond, ip): (u32, _) = read_stack(ip, sp);
    let (target, ip) = read_imm(ip);
    let ip = if cond == 0 { target } else { ip };
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn br_if_z_r(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let cond: u32 = read_reg(ix, sx, dx);
    let (target, ip) = read_imm(ip);
    let ip = if cond == 0 { target } else { ip };
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn br_if_nz_s(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (cond, ip): (u32, _) = read_stack(ip, sp);
    let (target, ip) = read_imm(ip);
    let ip = if cond != 0 { target } else { ip };
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn br_if_nz_r(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let cond: u32 = read_reg(ix, sx, dx);
    let (target, ip) = read_imm(ip);
    let ip = if cond != 0 { target } else { ip };
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn br_table_s(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (target_idx, ip): (u32, _) = read_stack(ip, sp);
    let (target_count, ip): (u32, _) = read_imm(ip);
    let targets: *mut Ip = ip.cast();
    let ip = *targets.add(target_idx.min(target_count) as usize);
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn br_table_r(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let target_idx: u32 = read_reg(ix, sx, dx);
    let (target_count, ip): (u32, _) = read_imm(ip);
    let targets: *mut Ip = ip.cast();
    let ip = *targets.add(target_idx.min(target_count) as usize);
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn return_(
    _ip: Ip,
    sp: Sp,
    _md: Md,
    _ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let old_sp = sp;
    let ip = *old_sp.offset(-4).cast();
    let sp = *old_sp.offset(-3).cast();
    let md = *old_sp.offset(-2).cast();
    let ms = *old_sp.offset(-1).cast();
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn call(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (target, ip) = read_imm(ip);
    let (offset, ip) = read_imm(ip);
    let new_sp: Sp = sp.cast::<u8>().add(offset).cast();
    *new_sp.offset(-4).cast() = ip;
    *new_sp.offset(-3).cast() = sp;
    *new_sp.offset(-2).cast() = md;
    *new_sp.offset(-1).cast() = ms;
    let ip = target;
    let sp = new_sp;
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn call_host(
    ip: Ip,
    sp: Sp,
    _md: Md,
    _ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (func, ip): (UnguardedFunc, _) = read_imm(ip);
    let (offset, ip) = read_imm(ip);
    let (mem, ip): (Option<UnguardedMem>, _) = read_imm(ip);
    (*cx)
        .stack
        .as_mut()
        .unwrap_unchecked()
        .set_ptr(sp.cast::<u8>().add(offset).cast());
    let FuncEntity::Host(func) = func.as_ref() else {
        hint::unreachable_unchecked();
    };
    let stack = match func
        .trampoline()
        .clone()
        .call((*cx).store, (*cx).stack.take().unwrap_unchecked())
    {
        Ok(stack) => stack,
        Err(error) => {
            (*cx).error = Some(error);
            return ControlFlow::Error.to_bits();
        }
    };
    (*cx).stack = Some(stack);
    let md;
    let ms;
    if let Some(mut mem) = mem {
        let data = mem.as_mut().bytes_mut();
        md = data.as_mut_ptr();
        ms = data.len() as u32;
    } else {
        md = ptr::null_mut();
        ms = 0;
    }
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn call_indirect(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (table_offset, ip): (u32, _) = read_stack(ip, sp);
    let (table, ip): (UnguardedTable, _) = read_imm(ip);
    let (type_, ip): (UnguardedInternedFuncType, _) = read_imm(ip);
    let (stack_offset, ip) = read_imm(ip);
    let (mem, ip): (Option<UnguardedMem>, _) = read_imm(ip);
    let func = r#try!(table
        .as_ref()
        .downcast_ref::<UnguardedFuncRef>()
        .unwrap_unchecked()
        .get(table_offset)
        .ok_or(Trap::TableAccessOutOfBounds));
    let mut func = r#try!(func.ok_or(Trap::ElemUninited));
    if func
        .as_ref()
        .interned_type()
        .to_unguarded((*(*cx).store).id())
        != type_
    {
        return ControlFlow::Trap(Trap::TypeMismatch).to_bits();
    }
    Func(Handle::from_unguarded(func, (*(*cx).store).id())).compile(&mut *(*cx).store);
    match func.as_mut() {
        FuncEntity::Wasm(func) => {
            let Code::Compiled(state) = func.code_mut() else {
                hint::unreachable_unchecked();
            };
            let target = state.code.as_mut_ptr();
            let new_sp: Sp = sp.cast::<u8>().add(stack_offset).cast();
            *new_sp.offset(-4).cast() = ip;
            *new_sp.offset(-3).cast() = sp;
            *new_sp.offset(-2).cast() = md;
            *new_sp.offset(-1).cast() = ms;
            let ip = target;
            let sp = new_sp;
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
        FuncEntity::Host(func) => {
            (*cx)
                .stack
                .as_mut()
                .unwrap_unchecked()
                .set_ptr(sp.cast::<u8>().add(stack_offset).cast());
            let stack = match func
                .trampoline()
                .clone()
                .call((*cx).store, (*cx).stack.take().unwrap_unchecked())
            {
                Ok(stack) => stack,
                Err(error) => {
                    (*cx).error = Some(error);
                    return ControlFlow::Error.to_bits();
                }
            };
            (*cx).stack = Some(stack);
            let md;
            let ms;
            if let Some(mut mem) = mem {
                let data = mem.as_mut().bytes_mut();
                md = data.as_mut_ptr();
                ms = data.len() as u32;
            } else {
                md = ptr::null_mut();
                ms = 0;
            }
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    }
}

// Reference instructions

macro_rules! ref_is_null {
    ($ref_is_null_s:ident, $ref_is_null_r:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $ref_is_null_s(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x, ip): ($T, _) = read_stack(ip, sp);

            // Perform operation
            let y = x.is_none() as u32;

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $ref_is_null_r(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x: $T = read_reg(ix, sx, dx);

            // Perform operation
            let y = x.is_none() as u32;

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

ref_is_null!(
    ref_is_null_func_ref_s,
    ref_is_null_func_ref_r,
    UnguardedFuncRef
);
ref_is_null!(
    ref_is_null_extern_ref_s,
    ref_is_null_extern_ref_r,
    UnguardedExternRef
);

// Parametric instructions

macro_rules! select {
    ($select_sss:ident, $select_rss:ident, $select_srs:ident, $select_ssr:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $select_sss(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (cond, ip): (u32, _) = read_stack(ip, sp);
            let (x1, ip): ($T, _) = read_stack(ip, sp);
            let (x0, ip): ($T, _) = read_stack(ip, sp);

            // Perform operation
            let y = if cond != 0 { x0 } else { x1 };

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $select_rss(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (cond, ip): (u32, _) = read_stack(ip, sp);
            let (x1, ip): ($T, _) = read_stack(ip, sp);
            let x0: $T = read_reg(ix, sx, dx);

            // Perform operation
            let y = if cond != 0 { x0 } else { x1 };

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $select_srs(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (cond, ip): (u32, _) = read_stack(ip, sp);
            let x1: $T = read_reg(ix, sx, dx);
            let (x0, ip): ($T, _) = read_stack(ip, sp);

            // Perform operation
            let y = if cond != 0 { x0 } else { x1 };

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $select_ssr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let cond: u32 = read_reg(ix, sx, dx);
            let (x1, ip): ($T, _) = read_stack(ip, sp);
            let (x0, ip): ($T, _) = read_stack(ip, sp);

            // Perform operation
            let y = if cond != 0 { x0 } else { x1 };

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

macro_rules! select_float {
    (
        $select_sss:ident,
        $select_rss:ident,
        $select_srs:ident,
        $select_ssr:ident,
        $select_rsr:ident,
        $select_srr:ident,
        $T:ty
    ) => {
        select!($select_sss, $select_rss, $select_srs, $select_ssr, $T);

        pub(crate) unsafe extern "C" fn $select_rsr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let cond: u32 = read_reg(ix, sx, dx);
            let (x1, ip): ($T, _) = read_stack(ip, sp);
            let x0: $T = read_reg(ix, sx, dx);

            // Perform operation
            let y = if cond != 0 { x0 } else { x1 };

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $select_srr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let cond: u32 = read_reg(ix, sx, dx);
            let x1 = read_reg(ix, sx, dx);
            let (x0, ip): ($T, _) = read_stack(ip, sp);

            // Perform operation
            let y = if cond != 0 { x0 } else { x1 };

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

select!(
    select_i32_sss,
    select_i32_rss,
    select_i32_srs,
    select_i32_ssr,
    i32
);
select!(
    select_i64_sss,
    select_i64_rss,
    select_i64_srs,
    select_i64_ssr,
    i64
);
select_float!(
    select_f32_sss,
    select_f32_rss,
    select_f32_srs,
    select_f32_ssr,
    select_f32_rsr,
    select_f32_srr,
    f32
);
select_float!(
    select_f64_sss,
    select_f64_rss,
    select_f64_srs,
    select_f64_ssr,
    select_f64_rsr,
    select_f64_srr,
    f64
);
select!(
    select_func_ref_sss,
    select_func_ref_rss,
    select_func_ref_srs,
    select_func_ref_ssr,
    UnguardedFuncRef
);
select!(
    select_extern_ref_sss,
    select_extern_ref_rss,
    select_extern_ref_srs,
    select_extern_ref_ssr,
    UnguardedExternRef
);

// Variable instructions

macro_rules! global_get {
    ($global_get_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $global_get_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (global, ip): (UnguardedGlobal, _) = read_imm(ip);

            // Perform operation
            let val = global
                .as_ref()
                .downcast_ref::<$T>()
                .unwrap_unchecked()
                .get();

            // Write result
            let ip = write_stack(ip, sp, val);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

global_get!(global_get_i32, i32);
global_get!(global_get_i64, i64);
global_get!(global_get_f32, f32);
global_get!(global_get_f64, f64);
global_get!(global_get_raw_func_ref, UnguardedFuncRef);
global_get!(global_get_raw_extern_ref, UnguardedExternRef);

macro_rules! global_set {
    ($global_set_t_s:ident, $global_set_t_r:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $global_set_t_s(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (val, ip) = read_stack(ip, sp);
            let (mut global, ip): (UnguardedGlobal, _) = read_imm(ip);

            // Perform operation
            global
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .set(val);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $global_set_t_r(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let val = read_reg(ix, sx, dx);
            let (mut global, ip): (UnguardedGlobal, _) = read_imm(ip);

            // Perform operation
            global
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .set(val);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

global_set!(global_set_i32_s, global_set_i32_r, i32);
global_set!(global_set_i64_s, global_set_i64_r, i64);
global_set!(global_set_f32_s, global_set_f32_r, f32);
global_set!(global_set_f64_s, global_set_f64_r, f64);
global_set!(
    global_set_func_ref_s,
    global_set_func_ref_r,
    UnguardedFuncRef
);
global_set!(
    global_set_extern_ref_s,
    global_set_extern_ref_r,
    UnguardedExternRef
);

// Table instructions

macro_rules! table_get {
    ($table_get_t_s:ident, $table_get_t_r:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_get_t_s(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (idx, ip) = read_stack(ip, sp);
            let (table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            let val = r#try!(table
                .as_ref()
                .downcast_ref::<$T>()
                .unwrap_unchecked()
                .get(idx)
                .ok_or(Trap::TableAccessOutOfBounds));

            // Write result
            let ip = write_stack(ip, sp, val);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $table_get_t_r(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let idx = read_reg(ix, sx, dx);
            let (table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            let val = r#try!(table
                .as_ref()
                .downcast_ref::<$T>()
                .unwrap_unchecked()
                .get(idx)
                .ok_or(Trap::TableAccessOutOfBounds));

            // Write result
            let ip = write_stack(ip, sp, val);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_get!(table_get_func_ref_s, table_get_func_ref_r, UnguardedFuncRef);
table_get!(
    table_get_extern_ref_s,
    table_get_extern_ref_r,
    UnguardedExternRef
);

macro_rules! table_set {
    ($table_get_t_ss:ident, $table_set_t_rs:ident, $table_set_t_sr:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_get_t_ss(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (idx, ip) = read_stack(ip, sp);
            let (val, ip) = read_stack(ip, sp);
            let (mut table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            r#try!(table
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .set(idx, val)
                .map_err(|_| Trap::TableAccessOutOfBounds));

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $table_set_t_rs(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let idx = read_reg(ix, sx, dx);
            let (val, ip) = read_stack(ip, sp);
            let (mut table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            r#try!(table
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .set(idx, val)
                .map_err(|_| Trap::TableAccessOutOfBounds));

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $table_set_t_sr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (idx, ip) = read_stack(ip, sp);
            let val = read_reg(ix, sx, dx);
            let (mut table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            r#try!(table
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .set(idx, val)
                .map_err(|_| Trap::TableAccessOutOfBounds));

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_set!(
    table_set_func_ref_ss,
    table_set_func_ref_rs,
    table_set_func_ref_sr,
    UnguardedFuncRef
);
table_set!(
    table_set_extern_ref_ss,
    table_set_extern_ref_rs,
    table_set_extern_ref_sr,
    UnguardedExternRef
);

macro_rules! table_size {
    ($table_size_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_size_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            let size = table
                .as_ref()
                .downcast_ref::<$T>()
                .unwrap_unchecked()
                .size();

            // Write result
            let ip = write_stack(ip, sp, size);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_size!(table_size_func_ref, UnguardedFuncRef);
table_size!(table_size_extern_ref, UnguardedExternRef);

macro_rules! table_grow {
    ($table_grow_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_grow_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (count, ip): (u32, _) = read_stack(ip, sp);
            let (val, ip) = read_stack(ip, sp);

            // Perform operation
            let (mut table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            let old_size = table
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .grow(val, count)
                .unwrap_or(u32::MAX);

            // Write result
            let ip = write_stack(ip, sp, old_size);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_grow!(table_grow_func_ref, UnguardedFuncRef);
table_grow!(table_grow_extern_ref, UnguardedExternRef);

macro_rules! table_fill {
    ($table_fill_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_fill_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (count, ip): (u32, _) = read_stack(ip, sp);
            let (val, ip) = read_stack(ip, sp);
            let (idx, ip): (u32, _) = read_stack(ip, sp);
            let (mut table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            r#try!(table
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .fill(idx, val, count));

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_fill!(table_fill_func_ref, UnguardedFuncRef);
table_fill!(table_fill_extern_ref, UnguardedExternRef);

macro_rules! table_copy {
    ($table_copy_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_copy_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (count, ip): (u32, _) = read_stack(ip, sp);
            let (src_offset, ip): (u32, _) = read_stack(ip, sp);
            let (dst_offset, ip): (u32, _) = read_stack(ip, sp);
            let (mut dst_table, ip): (UnguardedTable, _) = read_imm(ip);
            let (src_table, ip): (UnguardedTable, _) = read_imm(ip);

            // Perform operation
            r#try!(if dst_table == src_table {
                dst_table
                    .as_mut()
                    .downcast_mut::<$T>()
                    .unwrap_unchecked()
                    .copy_within(dst_offset, src_offset, count)
            } else {
                dst_table
                    .as_mut()
                    .downcast_mut::<$T>()
                    .unwrap_unchecked()
                    .copy(
                        dst_offset,
                        src_table.as_ref().downcast_ref::<$T>().unwrap_unchecked(),
                        src_offset,
                        count,
                    )
            });

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_copy!(table_copy_func_ref, UnguardedFuncRef);
table_copy!(table_copy_extern_ref, UnguardedExternRef);

macro_rules! table_init {
    ($table_init_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $table_init_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (count, ip): (u32, _) = read_stack(ip, sp);
            let (src_offset, ip): (u32, _) = read_stack(ip, sp);
            let (dst_offset, ip): (u32, _) = read_stack(ip, sp);
            let (mut dst_table, ip): (UnguardedTable, _) = read_imm(ip);
            let (src_elem, ip): (UnguardedElem, _) = read_imm(ip);

            // Perform operation
            r#try!(dst_table
                .as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .init(
                    dst_offset,
                    src_elem.as_ref().downcast_ref::<$T>().unwrap_unchecked(),
                    src_offset,
                    count
                ));
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

table_init!(table_init_func_ref, UnguardedFuncRef);
table_init!(table_init_extern_ref, UnguardedExternRef);

macro_rules! elem_drop {
    ($elem_drop_t:ident, $T:ty) => {
        pub(crate) unsafe extern "C" fn $elem_drop_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (mut elem, ip): (UnguardedElem, _) = read_imm(ip);

            // Perform operation
            elem.as_mut()
                .downcast_mut::<$T>()
                .unwrap_unchecked()
                .drop_elems();

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

elem_drop!(elem_drop_func_ref, UnguardedFuncRef);
elem_drop!(elem_drop_extern_ref, UnguardedExternRef);

// Memory instructions

macro_rules! load {
    ($load_s:ident, $load_r:ident, $T:ty, $U:ty) => {
        pub(crate) unsafe extern "C" fn $load_s(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (dyn_offset, ip): (u32, _) = read_stack(ip, sp);
            let (static_offset, ip): (u32, _) = read_imm(ip);

            // Perform operation
            let offset = dyn_offset as u64 + static_offset as u64;
            if offset + mem::size_of::<$T>() as u64 > ms as u64 {
                return ControlFlow::Trap(Trap::MemAccessOutOfBounds).to_bits();
            }
            let mut bytes = [0u8; mem::size_of::<$T>()];
            ptr::copy_nonoverlapping(md.add(offset as usize), bytes.as_mut_ptr(), bytes.len());
            let y = <$T>::from_le_bytes(bytes) as $U;

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $load_r(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let dyn_offset: u32 = read_reg(ix, sx, dx);
            let (static_offset, ip): (u32, _) = read_imm(ip);

            // Perform operation
            let offset = dyn_offset as u64 + static_offset as u64;
            if offset + mem::size_of::<$T>() as u64 > ms as u64 {
                return ControlFlow::Trap(Trap::MemAccessOutOfBounds).to_bits();
            }
            let mut bytes = [0u8; mem::size_of::<$T>()];
            ptr::copy_nonoverlapping(md.add(offset as usize), bytes.as_mut_ptr(), bytes.len());
            let y = <$T>::from_le_bytes(bytes) as $U;

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

macro_rules! store {
    ($store_ss:ident, $store_rs:ident, $store_sr:ident, $T:ty, $U:ty) => {
        pub(crate) unsafe extern "C" fn $store_ss(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x, ip): ($T, _) = read_stack(ip, sp);
            let (dyn_offset, ip): (u32, _) = read_stack(ip, sp);
            let (static_offset, ip): (u32, _) = read_imm(ip);

            // Perform operation
            let offset = dyn_offset as u64 + static_offset as u64;
            if offset + mem::size_of::<$T>() as u64 > ms as u64 {
                return ControlFlow::Trap(Trap::MemAccessOutOfBounds).to_bits();
            }
            let bytes = (x as $U).to_le_bytes();
            ptr::copy_nonoverlapping(bytes.as_ptr(), md.add(offset as usize), bytes.len());

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $store_rs(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x, ip): ($T, _) = read_stack(ip, sp);
            let dyn_offset: u32 = read_reg(ix, sx, dx);
            let (static_offset, ip): (u32, _) = read_imm(ip);

            // Perform operation
            let offset = dyn_offset as u64 + static_offset as u64;
            if offset + mem::size_of::<$T>() as u64 > ms as u64 {
                return ControlFlow::Trap(Trap::MemAccessOutOfBounds).to_bits();
            }
            let bytes = (x as $U).to_le_bytes();
            ptr::copy_nonoverlapping(bytes.as_ptr(), md.add(offset as usize), bytes.len());

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $store_sr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x: $T = read_reg(ix, sx, dx);
            let (dyn_offset, ip): (u32, _) = read_stack(ip, sp);
            let (static_offset, ip): (u32, _) = read_imm(ip);

            // Perform operation
            let offset = dyn_offset as u64 + static_offset as u64;
            if offset + mem::size_of::<$T>() as u64 > ms as u64 {
                return ControlFlow::Trap(Trap::MemAccessOutOfBounds).to_bits();
            }
            let bytes = (x as $U).to_le_bytes();
            ptr::copy_nonoverlapping(bytes.as_ptr(), md.add(offset as usize), bytes.len());

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

macro_rules! store_float {
    ($store_ss:ident, $store_rs:ident, $store_sr:ident, $store_rr:ident, $T:ty, $U:ty) => {
        store!($store_ss, $store_rs, $store_sr, $T, $U);

        pub(crate) unsafe extern "C" fn $store_rr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x: $T = read_reg(ix, sx, dx);
            let dyn_offset: u32 = read_reg(ix, sx, dx);
            let (static_offset, ip): (u32, _) = read_imm(ip);

            // Perform operation
            let offset = dyn_offset as u64 + static_offset as u64;
            if offset + mem::size_of::<$T>() as u64 > ms as u64 {
                return ControlFlow::Trap(Trap::MemAccessOutOfBounds).to_bits();
            }
            let bytes = (x as $U).to_le_bytes();
            ptr::copy_nonoverlapping(bytes.as_ptr(), md.add(offset as usize), bytes.len());

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

load!(i32_load_s, i32_load_r, i32, i32);
load!(i64_load_s, i64_load_r, i64, i64);
load!(f32_load_s, f32_load_r, f32, f32);
load!(f64_load_s, f64_load_r, f64, f64);
load!(i32_load8_s_s, i32_load8_s_r, i8, i32);
load!(i32_load8_u_s, i32_load8_u_r, u8, u32);
load!(i32_load16_s_s, i32_load16_s_r, i16, i32);
load!(i32_load16_u_s, i32_load16_u_r, u16, u32);
load!(i64_load8_s_s, i64_load8_s_r, i8, i64);
load!(i64_load8_u_s, i64_load8_u_r, u8, u64);
load!(i64_load16_s_s, i64_load16_s_r, i16, i64);
load!(i64_load16_u_s, i64_load16_u_r, u16, u64);
load!(i64_load32_s_s, i64_load32_s_r, i32, i64);
load!(i64_load32_u_s, i64_load32_u_r, u32, u64);
store!(i32_store_ss, i32_store_rs, i32_store_sr, i32, i32);
store!(i64_store_ss, i64_store_rs, i64_store_sr, i64, i64);
store_float!(
    f32_store_ss,
    f32_store_rs,
    f32_store_sr,
    f32_store_rr,
    f32,
    f32
);
store_float!(
    f64_store_ss,
    f64_store_rs,
    f64_store_sr,
    f64_store_rr,
    f64,
    f64
);
store!(i32_store8_ss, i32_store8_rs, i32_store8_sr, u32, u8);
store!(i32_store16_ss, i32_store16_rs, i32_store16_sr, u32, u16);
store!(i64_store8_ss, i64_store8_rs, i64_store8_sr, u64, u8);
store!(i64_store16_ss, i64_store16_rs, i64_store16_sr, u64, u16);
store!(i64_store32_ss, i64_store32_rs, i64_store32_sr, u64, u32);

pub(crate) unsafe extern "C" fn memory_size(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    // Read operands
    let (mem, ip): (UnguardedMem, _) = read_imm(ip);

    // Perform operation
    let size = mem.as_ref().size();

    // Write result
    let ip = write_stack(ip, sp, size);

    // Execute next instruction
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn memory_grow(
    ip: Ip,
    sp: Sp,
    _md: Md,
    _ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    // Read operands
    let (count, ip): (u32, _) = read_stack(ip, sp);
    let (mut mem, ip): (UnguardedMem, _) = read_imm(ip);

    // Perform operation
    (*cx).stack.as_mut().unwrap_unchecked().set_ptr(sp);
    let old_size = mem
        .as_mut()
        .grow_with_stack(count, (*cx).stack.as_mut().unwrap_unchecked())
        .unwrap_or(u32::MAX);
    let bytes = mem.as_mut().bytes_mut();
    let md = bytes.as_mut_ptr();
    let ms = bytes.len() as u32;

    // Write result
    let ip = write_stack(ip, sp, old_size);

    // Execute next instruction
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn memory_fill(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    // Read operands
    let (count, ip) = read_stack(ip, sp);
    let (val, ip): (u32, _) = read_stack(ip, sp);
    let (idx, ip) = read_stack(ip, sp);
    let (mut mem, ip): (UnguardedMem, _) = read_imm(ip);

    // Perform operation
    r#try!(mem.as_mut().fill(idx, val as u8, count));

    // Execute next instruction
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn memory_copy(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    // Read operands
    let (count, ip): (u32, _) = read_stack(ip, sp);
    let (src_idx, ip): (u32, _) = read_stack(ip, sp);
    let (dst_idx, ip): (u32, _) = read_stack(ip, sp);
    let (mut mem, ip): (UnguardedMem, _) = read_imm(ip);

    // Perform operation
    r#try!(mem.as_mut().copy_within(dst_idx, src_idx, count));

    // Execute next instruction
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn memory_init(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    // Read operands
    let (count, ip): (u32, _) = read_stack(ip, sp);
    let (src_idx, ip): (u32, _) = read_stack(ip, sp);
    let (dst_idx, ip): (u32, _) = read_stack(ip, sp);
    let (mut dst_mem, ip): (UnguardedMem, _) = read_imm(ip);
    let (src_data, ip): (UnguardedData, _) = read_imm(ip);

    // Perform operation
    r#try!(dst_mem
        .as_mut()
        .init(dst_idx, src_data.as_ref(), src_idx, count));

    // Execute next instruction
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn data_drop(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    // Read operands
    let (mut data, ip): (UnguardedData, _) = read_imm(ip);

    // Perform operation
    data.as_mut().drop_bytes();

    // Execute next instruction
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

// Numeric instructions

macro_rules! un_op {
    ($un_op_s:ident, $un_op_r:ident, $f:expr) => {
        pub(crate) unsafe extern "C" fn $un_op_s(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x, ip) = read_stack(ip, sp);

            // Perform operation
            let y = r#try!($f(x));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $un_op_r(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x = read_reg(ix, sx, dx);

            // Perform operation
            let y = r#try!($f(x));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

macro_rules! bin_op {
    ($bin_op_ss:ident, $bin_op_rs:ident, $f:expr) => {
        pub(crate) unsafe extern "C" fn $bin_op_ss(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            let (x1, ip) = read_stack(ip, sp);
            let (x0, ip) = read_stack(ip, sp);
            let y = r#try!($f(x0, x1));
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }

        pub(crate) unsafe extern "C" fn $bin_op_rs(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x1, ip) = read_stack(ip, sp);
            let x0 = read_reg(ix, sx, dx);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

macro_rules! bin_op_noncommutative {
    ($bin_op_ss:ident, $bin_op_rs:ident, $bin_op_sr:ident, $f:expr) => {
        bin_op!($bin_op_ss, $bin_op_rs, $f);

        pub(crate) unsafe extern "C" fn $bin_op_sr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x1 = read_reg(ix, sx, dx);
            let (x0, ip) = read_stack(ip, sp);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

un_op!(i32_eqz_s, i32_eqz_r, <u32 as IntOps>::eqz);
bin_op!(i32_eq_ss, i32_eq_rs, <u32 as RelOps>::eq);
bin_op!(i32_ne_ss, i32_ne_rs, <u32 as RelOps>::ne);
bin_op_noncommutative!(i32_lt_s_ss, i32_lt_s_rs, i32_lt_s_sr, <i32 as RelOps>::lt);
bin_op_noncommutative!(i32_lt_u_ss, i32_lt_u_rs, i32_lt_u_sr, <u32 as RelOps>::lt);
bin_op_noncommutative!(i32_gt_s_ss, i32_gt_s_rs, i32_gt_s_sr, <i32 as RelOps>::gt);
bin_op_noncommutative!(i32_gt_u_ss, i32_gt_u_rs, i32_gt_u_sr, <u32 as RelOps>::gt);
bin_op_noncommutative!(i32_le_s_ss, i32_le_s_rs, i32_le_s_sr, <i32 as RelOps>::le);
bin_op_noncommutative!(i32_le_u_ss, i32_le_u_rs, i32_le_u_sr, <u32 as RelOps>::le);
bin_op_noncommutative!(i32_ge_s_ss, i32_ge_s_rs, i32_ge_s_sr, <i32 as RelOps>::ge);
bin_op_noncommutative!(i32_ge_u_ss, i32_ge_u_rs, i32_ge_u_sr, <u32 as RelOps>::ge);

un_op!(i64_eqz_s, i64_eqz_r, <u64 as IntOps>::eqz);
bin_op!(i64_eq_ss, i64_eq_rs, <u64 as RelOps>::eq);
bin_op!(i64_ne_ss, i64_ne_rs, <u64 as RelOps>::ne);
bin_op_noncommutative!(i64_lt_s_ss, i64_lt_s_rs, i64_lt_s_sr, <i64 as RelOps>::lt);
bin_op_noncommutative!(i64_lt_u_ss, i64_lt_u_rs, i64_lt_u_sr, <u64 as RelOps>::lt);
bin_op_noncommutative!(i64_gt_s_ss, i64_gt_s_rs, i64_gt_s_sr, <i64 as RelOps>::gt);
bin_op_noncommutative!(i64_gt_u_ss, i64_gt_u_rs, i64_gt_u_sr, <u64 as RelOps>::gt);
bin_op_noncommutative!(i64_le_s_ss, i64_le_s_rs, i64_le_s_sr, <i64 as RelOps>::le);
bin_op_noncommutative!(i64_le_u_ss, i64_le_u_rs, i64_le_u_sr, <u64 as RelOps>::le);
bin_op_noncommutative!(i64_ge_s_ss, i64_ge_s_rs, i64_ge_s_sr, <i64 as RelOps>::ge);
bin_op_noncommutative!(i64_ge_u_ss, i64_ge_u_rs, i64_ge_u_sr, <u64 as RelOps>::ge);

bin_op!(f32_eq_ss, f32_eq_rs, <f32 as RelOps>::eq);
bin_op!(f32_ne_ss, f32_ne_rs, <f32 as RelOps>::ne);
bin_op_noncommutative!(f32_lt_ss, f32_lt_rs, f32_lt_sr, <f32 as RelOps>::lt);
bin_op_noncommutative!(f32_gt_ss, f32_gt_rs, f32_gt_sr, <f32 as RelOps>::gt);
bin_op_noncommutative!(f32_le_ss, f32_le_rs, f32_le_sr, <f32 as RelOps>::le);
bin_op_noncommutative!(f32_ge_ss, f32_ge_rs, f32_ge_sr, <f32 as RelOps>::ge);

bin_op!(f64_eq_ss, f64_eq_rs, <f64 as RelOps>::eq);
bin_op!(f64_ne_ss, f64_ne_rs, <f64 as RelOps>::ne);
bin_op_noncommutative!(f64_lt_ss, f64_lt_rs, f64_lt_sr, <f64 as RelOps>::lt);
bin_op_noncommutative!(f64_gt_ss, f64_gt_rs, f64_gt_sr, <f64 as RelOps>::gt);
bin_op_noncommutative!(f64_le_ss, f64_le_rs, f64_le_sr, <f64 as RelOps>::le);
bin_op_noncommutative!(f64_ge_ss, f64_ge_rs, f64_ge_sr, <f64 as RelOps>::ge);

un_op!(i32_clz_s, i32_clz_r, <u32 as IntOps>::clz);
un_op!(i32_ctz_s, i32_ctz_r, <u32 as IntOps>::ctz);
un_op!(i32_popcnt_s, i32_popcnt_r, <u32 as IntOps>::popcnt);
bin_op!(i32_add_ss, i32_add_rs, <u32 as IntOps>::add);
bin_op_noncommutative!(i32_sub_ss, i32_sub_rs, i32_sub_sr, <u32 as IntOps>::sub);
bin_op!(i32_mul_ss, i32_mul_rs, <u32 as IntOps>::mul);
bin_op_noncommutative!(
    i32_div_s_ss,
    i32_div_s_rs,
    i32_div_s_sr,
    <i32 as IntOps>::div
);
bin_op_noncommutative!(
    i32_div_u_ss,
    i32_div_u_rs,
    i32_div_u_sr,
    <u32 as IntOps>::div
);
bin_op_noncommutative!(
    i32_rem_s_ss,
    i32_rem_s_rs,
    i32_rem_s_sr,
    <i32 as IntOps>::rem
);
bin_op_noncommutative!(
    i32_rem_u_ss,
    i32_rem_u_rs,
    i32_rem_u_sr,
    <u32 as IntOps>::rem
);
bin_op!(i32_and_ss, i32_and_rs, <u32 as IntOps>::and);
bin_op!(i32_or_ss, i32_or_rs, <u32 as IntOps>::or);
bin_op!(i32_xor_ss, i32_xor_rs, <u32 as IntOps>::xor);
bin_op_noncommutative!(i32_shl_ss, i32_shl_rs, i32_shl_sr, <u32 as IntOps>::shl);
bin_op_noncommutative!(
    i32_shr_s_ss,
    i32_shr_s_rs,
    i32_shr_s_sr,
    <i32 as IntOps>::shr
);
bin_op_noncommutative!(
    i32_shr_u_ss,
    i32_shr_u_rs,
    i32_shr_u_sr,
    <u32 as IntOps>::shr
);
bin_op_noncommutative!(i32_rotl_ss, i32_rotl_rs, i32_rotl_sr, <u32 as IntOps>::rotl);
bin_op_noncommutative!(i32_rotr_ss, i32_rotr_rs, i32_rotr_sr, <u32 as IntOps>::rotr);

un_op!(i64_clz_s, i64_clz_r, <u64 as IntOps>::clz);
un_op!(i64_ctz_s, i64_ctz_r, <u64 as IntOps>::ctz);
un_op!(i64_popcnt_s, i64_popcnt_r, <u64 as IntOps>::popcnt);
bin_op!(i64_add_ss, i64_add_rs, <u64 as IntOps>::add);
bin_op_noncommutative!(i64_sub_ss, i64_sub_rs, i64_sub_sr, <u64 as IntOps>::sub);
bin_op!(i64_mul_ss, i64_mul_rs, <u64 as IntOps>::mul);
bin_op_noncommutative!(
    i64_div_s_ss,
    i64_div_s_rs,
    i64_div_s_sr,
    <i64 as IntOps>::div
);
bin_op_noncommutative!(
    i64_div_u_ss,
    i64_div_u_rs,
    i64_div_u_sr,
    <u64 as IntOps>::div
);
bin_op_noncommutative!(
    i64_rem_s_ss,
    i64_rem_s_rs,
    i64_rem_s_sr,
    <i64 as IntOps>::rem
);
bin_op_noncommutative!(
    i64_rem_u_ss,
    i64_rem_u_rs,
    i64_rem_u_sr,
    <u64 as IntOps>::rem
);
bin_op!(i64_and_ss, i64_and_rs, <u64 as IntOps>::and);
bin_op!(i64_or_ss, i64_or_rs, <u64 as IntOps>::or);
bin_op!(i64_xor_ss, i64_xor_rs, <u64 as IntOps>::xor);
bin_op_noncommutative!(i64_shl_ss, i64_shl_rs, i64_shl_sr, <u64 as IntOps>::shl);
bin_op_noncommutative!(
    i64_shr_s_ss,
    i64_shr_s_rs,
    i64_shr_s_sr,
    <i64 as IntOps>::shr
);
bin_op_noncommutative!(
    i64_shr_u_ss,
    i64_shr_u_rs,
    i64_shr_u_sr,
    <u64 as IntOps>::shr
);
bin_op_noncommutative!(i64_rotl_ss, i64_rotl_rs, i64_rotl_sr, <u64 as IntOps>::rotl);
bin_op_noncommutative!(i64_rotr_ss, i64_rotr_rs, i64_rotr_sr, <u64 as IntOps>::rotr);

un_op!(f32_abs_s, f32_abs_r, <f32 as FloatOps>::abs);
un_op!(f32_neg_s, f32_neg_r, <f32 as FloatOps>::neg);
un_op!(f32_ceil_s, f32_ceil_r, <f32 as FloatOps>::ceil);
un_op!(f32_floor_s, f32_floor_r, <f32 as FloatOps>::floor);
un_op!(f32_trunc_s, f32_trunc_r, <f32 as FloatOps>::trunc);
un_op!(f32_nearest_s, f32_nearest_r, <f32 as FloatOps>::nearest);
un_op!(f32_sqrt_s, f32_sqrt_r, <f32 as FloatOps>::sqrt);
bin_op!(f32_add_ss, f32_add_rs, <f32 as FloatOps>::add);
bin_op_noncommutative!(f32_sub_ss, f32_sub_rs, f32_sub_sr, <f32 as FloatOps>::sub);
bin_op!(f32_mul_ss, f32_mul_rs, <f32 as FloatOps>::mul);
bin_op_noncommutative!(f32_div_ss, f32_div_rs, f32_div_sr, <f32 as FloatOps>::div);
bin_op!(f32_min_ss, f32_min_rs, <f32 as FloatOps>::min);
bin_op!(f32_max_ss, f32_max_rs, <f32 as FloatOps>::max);
bin_op_noncommutative!(
    f32_copysign_ss,
    f32_copysign_rs,
    f32_copysign_sr,
    <f32 as FloatOps>::copysign
);

un_op!(f64_abs_s, f64_abs_r, <f64 as FloatOps>::abs);
un_op!(f64_neg_s, f64_neg_r, <f64 as FloatOps>::neg);
un_op!(f64_ceil_s, f64_ceil_r, <f64 as FloatOps>::ceil);
un_op!(f64_floor_s, f64_floor_r, <f64 as FloatOps>::floor);
un_op!(f64_trunc_s, f64_trunc_r, <f64 as FloatOps>::trunc);
un_op!(f64_nearest_s, f64_nearest_r, <f64 as FloatOps>::nearest);
un_op!(f64_sqrt_s, f64_sqrt_r, <f64 as FloatOps>::sqrt);
bin_op!(f64_add_ss, f64_add_rs, <f64 as FloatOps>::add);
bin_op_noncommutative!(f64_sub_ss, f64_sub_rs, f64_sub_sr, <f64 as FloatOps>::sub);
bin_op!(f64_mul_ss, f64_mul_rs, <f64 as FloatOps>::mul);
bin_op_noncommutative!(f64_div_ss, f64_div_rs, f64_div_sr, <f64 as FloatOps>::div);
bin_op!(f64_min_ss, f64_min_rs, <f64 as FloatOps>::min);
bin_op!(f64_max_ss, f64_max_rs, <f64 as FloatOps>::max);
bin_op_noncommutative!(
    f64_copysign_ss,
    f64_copysign_rs,
    f64_copysign_sr,
    <f64 as FloatOps>::copysign
);

un_op!(i32_wrap_i64_s, i32_wrap_i64_r, <u32 as Wrap<u64>>::wrap);
un_op!(
    i32_trunc_f32_s_s,
    i32_trunc_f32_s_r,
    <i32 as Trunc<f32>>::trunc
);
un_op!(
    i32_trunc_f32_u_s,
    i32_trunc_f32_u_r,
    <u32 as Trunc<f32>>::trunc
);
un_op!(
    i32_trunc_f64_s_s,
    i32_trunc_f64_s_r,
    <i32 as Trunc<f64>>::trunc
);
un_op!(
    i32_trunc_f64_u_s,
    i32_trunc_f64_u_r,
    <u32 as Trunc<f64>>::trunc
);
un_op!(
    i64_extend_i32_s_s,
    i64_extend_i32_s_r,
    <i64 as Extend<i32>>::extend
);
un_op!(
    i64_extend_i32_u_s,
    i64_extend_i32_u_r,
    <u64 as Extend<u32>>::extend
);
un_op!(
    i64_trunc_f32_s_s,
    i64_trunc_f32_s_r,
    <i64 as Trunc<f32>>::trunc
);
un_op!(
    i64_trunc_f32_u_s,
    i64_trunc_f32_u_r,
    <u64 as Trunc<f32>>::trunc
);
un_op!(
    i64_trunc_f64_s_s,
    i64_trunc_f64_s_r,
    <i64 as Trunc<f64>>::trunc
);
un_op!(
    i64_trunc_f64_u_s,
    i64_trunc_f64_u_r,
    <u64 as Trunc<f64>>::trunc
);
un_op!(
    f32_convert_i32_s_s,
    f32_convert_i32_s_r,
    <f32 as Convert<i32>>::convert
);
un_op!(
    f32_convert_i32_u_s,
    f32_convert_i32_u_r,
    <f32 as Convert<u32>>::convert
);
un_op!(
    f32_convert_i64_s_s,
    f32_convert_i64_s_r,
    <f32 as Convert<i64>>::convert
);
un_op!(
    f32_convert_i64_u_s,
    f32_convert_i64_u_r,
    <f32 as Convert<u64>>::convert
);
un_op!(
    f32_demote_f64_s,
    f32_demote_f64_r,
    <f32 as Demote<f64>>::demote
);
un_op!(
    f64_convert_i32_s_s,
    f64_convert_i32_s_r,
    <f64 as Convert<i32>>::convert
);
un_op!(
    f64_convert_i32_u_s,
    f64_convert_i32_u_r,
    <f64 as Convert<u32>>::convert
);
un_op!(
    f64_convert_i64_s_s,
    f64_convert_i64_s_r,
    <f64 as Convert<i64>>::convert
);
un_op!(
    f64_convert_i64_u_s,
    f64_convert_i64_u_r,
    <f64 as Convert<u64>>::convert
);
un_op!(
    f64_promote_f32_s,
    f64_promote_f32_r,
    <f64 as Promote<f32>>::promote
);
un_op!(
    i32_reinterpret_f32_s,
    i32_reinterpret_f32_r,
    <u32 as Reinterpret<f32>>::reinterpret
);
un_op!(
    i64_reinterpret_f64_s,
    i64_reinterpret_f64_r,
    <u64 as Reinterpret<f64>>::reinterpret
);
un_op!(
    f32_reinterpret_i32_s,
    f32_reinterpret_i32_r,
    <f32 as Reinterpret<u32>>::reinterpret
);
un_op!(
    f64_reinterpret_i64_s,
    f64_reinterpret_i64_r,
    <f64 as Reinterpret<u64>>::reinterpret
);

un_op!(
    i32_extend8_s_s,
    i32_extend8_s_r,
    <i32 as ExtendN<i8>>::extend_n
);
un_op!(
    i32_extend16_s_s,
    i32_extend16_s_r,
    <i32 as ExtendN<i16>>::extend_n
);
un_op!(
    i64_extend8_s_s,
    i64_extend8_s_r,
    <i64 as ExtendN<i8>>::extend_n
);
un_op!(
    i64_extend16_s_s,
    i64_extend16_s_r,
    <i64 as ExtendN<i16>>::extend_n
);
un_op!(
    i64_extend32_s_s,
    i64_extend32_s_r,
    <i64 as ExtendN<i32>>::extend_n
);

un_op!(
    i32_trunc_sat_f32_s_s,
    i32_trunc_sat_f32_s_r,
    <i32 as Trunc<f32>>::trunc_sat
);
un_op!(
    i32_trunc_sat_f32_u_s,
    i32_trunc_sat_f32_u_r,
    <u32 as Trunc<f32>>::trunc_sat
);
un_op!(
    i32_trunc_sat_f64_s_s,
    i32_trunc_sat_f64_s_r,
    <i32 as Trunc<f64>>::trunc_sat
);
un_op!(
    i32_trunc_sat_f64_u_s,
    i32_trunc_sat_f64_u_r,
    <u32 as Trunc<f64>>::trunc_sat
);
un_op!(
    i64_trunc_sat_f32_s_s,
    i64_trunc_sat_f32_s_r,
    <i64 as Trunc<f32>>::trunc_sat
);
un_op!(
    i64_trunc_sat_f32_u_s,
    i64_trunc_sat_f32_u_r,
    <u64 as Trunc<f32>>::trunc_sat
);
un_op!(
    i64_trunc_sat_f64_s_s,
    i64_trunc_sat_f64_s_r,
    <i64 as Trunc<f64>>::trunc_sat
);
un_op!(
    i64_trunc_sat_f64_u_s,
    i64_trunc_sat_f64_u_r,
    <u64 as Trunc<f64>>::trunc_sat
);

// Miscellaneous instructions

macro_rules! copy_imm_to_stack {
    ($copy_imm_to_stack_t:ident, $T:ty) => {
        /// Copies an immediate value to the stack.
        pub(crate) unsafe extern "C" fn $copy_imm_to_stack_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read immediate value
            let (x, ip): ($T, _) = read_imm(ip);

            // Write value to stack
            let ip = write_stack(ip, sp, x);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

copy_imm_to_stack!(copy_imm_to_stack_i32, i32);
copy_imm_to_stack!(copy_imm_to_stack_i64, i64);
copy_imm_to_stack!(copy_imm_to_stack_f32, f32);
copy_imm_to_stack!(copy_imm_to_stack_f64, f64);
copy_imm_to_stack!(copy_imm_to_stack_func_ref, UnguardedFuncRef);
copy_imm_to_stack!(copy_imm_to_stack_extern_ref, UnguardedExternRef);

macro_rules! copy_stack {
    ($copy_stack_t:ident, $T:ty) => {
        /// Copies a value within the stack.
        pub(crate) unsafe extern "C" fn $copy_stack_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read value from stack
            let (x, ip): ($T, _) = read_stack(ip, sp);

            // Write value to stack
            let ip = write_stack(ip, sp, x);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

copy_stack!(copy_stack_i32, i32);
copy_stack!(copy_stack_i64, i64);
copy_stack!(copy_stack_f32, f32);
copy_stack!(copy_stack_f64, f64);
copy_stack!(copy_stack_func_ref, UnguardedFuncRef);
copy_stack!(copy_stack_extern_ref, UnguardedExternRef);

macro_rules! copy_reg_to_stack {
    ($copy_reg_to_stack_t:ident, $T:ty) => {
        /// Copies a value from a register to the stack.
        pub(crate) unsafe extern "C" fn $copy_reg_to_stack_t(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read value from register
            let x: $T = read_reg(ix, sx, dx);

            // Write value to stack
            let ip = write_stack(ip, sp, x);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        }
    };
}

copy_reg_to_stack!(copy_reg_to_stack_i32, i32);
copy_reg_to_stack!(copy_reg_to_stack_i64, i64);
copy_reg_to_stack!(copy_reg_to_stack_f32, f32);
copy_reg_to_stack!(copy_reg_to_stack_f64, f64);
copy_reg_to_stack!(copy_reg_to_stack_func_ref, UnguardedFuncRef);
copy_reg_to_stack!(copy_reg_to_stack_extern_ref, UnguardedExternRef);

pub(crate) unsafe extern "C" fn stop(
    _ip: Ip,
    _sp: Sp,
    _md: Md,
    _ms: Ms,
    _ix: Ix,
    _sx: Sx,
    _dx: Dx,
    _cx: Cx,
) -> ControlFlowBits {
    ControlFlow::Stop.to_bits()
}

pub(crate) unsafe extern "C" fn compile(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let mut func: UnguardedFunc = *ip.cast();
    Func(Handle::from_unguarded(func, (*(*cx).store).id())).compile((*cx).store);
    let FuncEntity::Wasm(func) = func.as_mut() else {
        hint::unreachable_unchecked();
    };
    let Code::Compiled(state) = func.code_mut() else {
        hint::unreachable_unchecked();
    };
    *ip.cast() = state.code.as_mut_ptr();
    let ip = ip.offset(-1);
    *ip.cast() = call as Instr;
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

pub(crate) unsafe extern "C" fn enter(
    ip: Ip,
    sp: Sp,
    _md: Md,
    _ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (func, ip): (UnguardedFunc, _) = read_imm(ip);
    let (mem, ip): (Option<UnguardedMem>, _) = read_imm(ip);
    let FuncEntity::Wasm(func) = func.as_ref() else {
        hint::unreachable_unchecked();
    };
    let Code::Compiled(state) = func.code() else {
        hint::unreachable_unchecked();
    };
    let stack_height =
        usize::try_from(sp.offset_from((*cx).stack.as_mut().unwrap_unchecked().base_ptr()))
            .unwrap_unchecked();
    if state.max_stack_slot_count > Stack::SIZE - stack_height {
        return ControlFlow::Trap(Trap::StackOverflow).to_bits();
    }
    ptr::write_bytes(sp, 0, state.local_count);
    let md;
    let ms;
    if let Some(mut mem) = mem {
        let data = mem.as_mut().bytes_mut();
        md = data.as_mut_ptr();
        ms = data.len() as u32;
    } else {
        md = ptr::null_mut();
        ms = 0;
    }
    next_instr(ip, sp, md, ms, ix, sx, dx, cx)
}

// Helper functions

/// Executes the next instruction.
pub(crate) unsafe fn next_instr(
    ip: Ip,
    sp: Sp,
    md: Md,
    ms: Ms,
    ix: Ix,
    sx: Sx,
    dx: Dx,
    cx: Cx,
) -> ControlFlowBits {
    let (instr, ip): (Instr, _) = read_imm(ip);
    (instr)(ip, sp, md, ms, ix, sx, dx, cx)
}

/// Reads an immediate value.
unsafe fn read_imm<T>(ip: Ip) -> (T, Ip)
where
    T: Copy,
{
    let val = *ip.cast();
    let ip = ip.add(1);
    (val, ip)
}

/// Reads a value from the stack.
unsafe fn read_stack<T>(ip: Ip, sp: Sp) -> (T, Ip)
where
    T: Copy + std::fmt::Debug,
{
    let (offset, ip) = read_imm(ip);
    let x = *sp.cast::<u8>().offset(offset).cast::<T>();
    (x, ip)
}

/// Writes a value to the stack.
unsafe fn write_stack<T>(ip: Ip, sp: Sp, x: T) -> Ip
where
    T: Copy + std::fmt::Debug,
{
    let (offset, ip) = read_imm(ip);
    *sp.cast::<u8>().offset(offset).cast() = x;
    ip
}

/// Reads a value from a register.
fn read_reg<T>(ix: Ix, sx: Sx, dx: Dx) -> T
where
    T: ReadReg,
{
    T::read_reg(ix, sx, dx)
}

trait ReadReg {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self;
}

impl ReadReg for i32 {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self {
        ix as i32
    }
}

impl ReadReg for u32 {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self {
        ix as u32
    }
}

impl ReadReg for i64 {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self {
        ix as i64
    }
}

impl ReadReg for u64 {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self {
        ix as u64
    }
}

impl ReadReg for f32 {
    fn read_reg(_ix: Ix, sx: Sx, _dx: Dx) -> Self {
        sx
    }
}

impl ReadReg for f64 {
    fn read_reg(_ix: Ix, _sx: Sx, dx: Dx) -> Self {
        dx
    }
}

impl ReadReg for UnguardedFuncRef {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self {
        UnguardedFunc::new(ix as *mut _)
    }
}

impl ReadReg for UnguardedExternRef {
    fn read_reg(ix: Ix, _sx: Sx, _dx: Dx) -> Self {
        UnguardedExtern::new(ix as *mut _)
    }
}

// Writes a value to a register.
fn write_reg<T>(ix: Ix, sx: Sx, dx: Dx, x: T) -> (Ix, Sx, Dx)
where
    T: WriteReg,
{
    T::write_reg(ix, sx, dx, x)
}

trait WriteReg {
    fn write_reg(ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx);
}

impl WriteReg for i32 {
    fn write_reg(_ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (x as u32 as Ix, sx, dx)
    }
}

impl WriteReg for u32 {
    fn write_reg(_ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (x as Ix, sx, dx)
    }
}

impl WriteReg for i64 {
    fn write_reg(_ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (x as Ix, sx, dx)
    }
}

impl WriteReg for u64 {
    fn write_reg(_ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (x as Ix, sx, dx)
    }
}

impl WriteReg for f32 {
    fn write_reg(ix: Ix, _sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (ix, x, dx)
    }
}

impl WriteReg for f64 {
    fn write_reg(ix: Ix, sx: Sx, _dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (ix, sx, x)
    }
}

impl WriteReg for UnguardedFuncRef {
    fn write_reg(_ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (x.map_or(ptr::null_mut(), |ptr| ptr.as_ptr()) as Ix, sx, dx)
    }
}

impl WriteReg for UnguardedExternRef {
    fn write_reg(_ix: Ix, sx: Sx, dx: Dx, x: Self) -> (Ix, Sx, Dx) {
        (x.map_or(ptr::null_mut(), |ptr| ptr.as_ptr()) as Ix, sx, dx)
    }
}
