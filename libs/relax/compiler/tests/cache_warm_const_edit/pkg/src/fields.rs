// Structs run.py edits the field lists of while code reading them runs: no reader's text changes
// (the reader is in readers.rs, or below the struct). The edit moves a retyped field: its old slot
// goes dead and it comes again in the extension block, so a reader left on its old code reads the
// dead slot.
use std::sync::OnceLock;

// field_every: an array's length changes (the literal's text with it), read every way
#[derive(Clone, PartialEq, Debug)]
pub struct Frame {
    pub before: u64,
    pub u: [u64; 3],
    pub after: u64,
}
impl Frame {
    pub fn new() -> Frame {
        Frame { before: 1, u: [7; 3], after: 2 }
    }
    #[inline(never)]
    pub fn method(&self) -> u64 {
        self.u[0] + self.u.len() as u64 * 10
    }
    #[inline(always)]
    pub fn tiny(&self) -> u64 {
        self.u[0] + self.u.len() as u64 * 10
    }
    #[inline(never)]
    pub fn set(&mut self) {
        self.u[0] = 8;
    }
    // its text names the struct, not the field
    #[inline(never)]
    pub fn updated() -> Frame {
        Frame { before: 5, ..Frame::new() }
    }
    #[inline(never)]
    pub fn updated_self(&self) -> Self {
        Self { before: 5, ..self.clone() }
    }
    #[inline(never)]
    pub fn late(&self) -> u64 {
        self.u[0] + self.u.len() as u64 * 10
    }
    #[inline(never)]
    pub fn rest(&self) -> u64 {
        self.before + self.after
    }
}
pub trait Read {
    fn read(&self) -> u64;
}
impl Read for Frame {
    #[inline(never)]
    fn read(&self) -> u64 {
        self.u[0] + self.u.len() as u64 * 10
    }
}
pub struct Holds {
    pub frame: Frame,
}
// a value made before the edit: the retyped field reads as its new type's default
#[inline(never)]
pub fn held() -> &'static Frame {
    static HELD: OnceLock<Frame> = OnceLock::new();
    HELD.get_or_init(Frame::new)
}

// field_same_size: another type of the same size; the literal's text holds for both
pub struct SameSize {
    pub s: u64,
    pub t: u64,
}
#[inline(never)]
pub fn same_size_new() -> SameSize {
    SameSize { s: 5, t: 6 }
}

// field_two_kinds: two fields swap their types
pub struct TwoKinds {
    pub a: u64,
    pub b: u32,
}
#[inline(never)]
pub fn two_kinds_new() -> TwoKinds {
    TwoKinds { a: 1, b: 2 }
}

// field_order: the declaration order alone changes
pub struct Order {
    pub a: u64,
    pub b: u64,
}
#[inline(never)]
pub fn order_new() -> Order {
    Order { a: 1, b: 2 }
}

// field_widen: a lossless widening keeps the running value
pub struct Widen {
    pub w: u32,
}
#[inline(never)]
pub fn widen_new() -> Widen {
    Widen { w: 5 }
}
#[inline(never)]
pub fn widen_held() -> &'static Widen {
    static HELD: OnceLock<Widen> = OnceLock::new();
    HELD.get_or_init(widen_new)
}

// field_nested: the inner struct's field, reached through an outer one no edit touches
pub struct Inner {
    pub v: [u64; 2],
}
impl Inner {
    pub fn new() -> Inner {
        Inner { v: [5; 2] }
    }
}
pub struct Outer {
    pub pad: u64,
    pub inner: Inner,
}
#[inline(never)]
pub fn outer_new() -> Outer {
    Outer { pad: 1, inner: Inner::new() }
}

// field_generic: a generic struct
pub struct Gen<T> {
    pub x: [T; 2],
}
impl Gen<u64> {
    pub fn new() -> Gen<u64> {
        Gen { x: [5; 2] }
    }
}

// field_running_frame: read by a frame that never returns
pub struct Running {
    pub u: [u64; 3],
}
impl Running {
    pub fn new() -> Running {
        Running { u: [7; 3] }
    }
}

// field_blocked_frame: read by a thread's frame that blocks in std and never returns
pub struct Blocked {
    pub u: [u64; 3],
}
impl Blocked {
    pub fn new() -> Blocked {
        Blocked { u: [7; 3] }
    }
}
pub static BLOCKED_READ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[inline(never)]
pub fn blocked_reader() {
    let blocked = Blocked::new();
    loop {
        BLOCKED_READ.store(blocked.u[0] + blocked.u.len() as u64 * 10, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

// field_short_walk: the same, under a frame that makes its syscall itself: a walk of the blocked
// thread ends at it, so only the stack above it shows the reader
pub struct ShortWalk {
    pub u: [u64; 3],
}
impl ShortWalk {
    pub fn new() -> ShortWalk {
        ShortWalk { u: [7; 3] }
    }
}
pub static SHORT_WALK_READ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[inline(never)]
fn sleep_by_syscall() {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    unsafe {
        const NANOSLEEP: i64 = 35;
        let second: [i64; 2] = [1, 0];
        std::arch::asm!("syscall", inlateout("rax") NANOSLEEP => _, in("rdi") second.as_ptr(), in("rsi") 0, lateout("rcx") _, lateout("r11") _);
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    std::thread::sleep(std::time::Duration::from_secs(1));
}
#[inline(never)]
pub fn short_walk_reader() {
    let short = ShortWalk::new();
    loop {
        SHORT_WALK_READ.store(short.u[0] + short.u.len() as u64 * 10, std::sync::atomic::Ordering::Relaxed);
        sleep_by_syscall();
    }
}

// field_offset: a field's offset, folded in the fn that asks
pub struct OffsetRead {
    pub pad: u64,
    pub u: u32,
}
#[inline(never)]
pub fn offset_read_new() -> OffsetRead {
    OffsetRead { pad: 1, u: 5 }
}

// field_const_read: a const's initializer reads the field; its value depends on the field's type
pub struct ConstRead {
    pub u: u32,
}
pub const CONST_READ: ConstRead = ConstRead { u: 1 };
pub const CONST_READ_SIZE: usize = std::mem::size_of_val(&CONST_READ.u);

// field_const_fn: the same through a const fn
pub struct ConstFn {
    pub u: u32,
}
impl ConstFn {
    pub const fn size(&self) -> usize {
        std::mem::size_of_val(&self.u)
    }
}
pub const CONST_FN_SIZE: usize = (ConstFn { u: 1 }).size();

// field_static: a static holds a value of the struct
pub struct InStatic {
    pub s: u64,
}
pub static IN_STATIC: InStatic = InStatic { s: 3 };

// field_import: what the field's type names changes, the struct's text does not
pub mod ia {
    pub struct T(pub u32);
}
pub mod ib {
    pub struct T(pub u64);
}
use ia::T as Imported;
pub struct UsesImport {
    pub t: Imported,
}
#[inline(never)]
pub fn uses_import_new() -> UsesImport {
    UsesImport { t: Imported(1) }
}

// field_enum: a variant's payload, its reader naming only the variant
pub enum Pay {
    A([u64; 2]),
    B,
}
use Pay::*;
#[inline(never)]
pub fn pay_new() -> Pay {
    if std::env::var("NEVER_SET").is_ok() {
        return B;
    }
    A([5; 2])
}
#[inline(never)]
pub fn pay_read(p: &Pay) -> u64 {
    match p {
        A(v) => v.len() as u64,
        B => 0,
    }
}
