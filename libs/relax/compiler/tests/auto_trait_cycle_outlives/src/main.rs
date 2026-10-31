// Nested auto-trait cycles through a `T: Send + 'static` impl (tokio/matrix-sdk shape: Arc<Mutex<..>>
// graphs whose cells require 'static): S0 <-> S1 <-> .. <-> S15, every S_i the head of its own cycle.
// `S_i: 'static` is global (no params, inference variables or local regions), so rustc registers no
// outlives constraint for it (rustc_infer outlives/obligations.rs register_type_outlives_constraint)
// and each head reaches its fixpoint on the first pass. Registering it made every nested head
// rerun: 2^16 evaluations of the innermost cycle (Robrix: 61k reruns, 16M goals, 70 s).
use std::marker::PhantomData;

pub struct Cell<T>(Option<Box<T>>, PhantomData<*const ()>);
unsafe impl<T: Send + 'static> Send for Cell<T> {}
unsafe impl<T: Sync + 'static> Sync for Cell<T> {}

pub struct S0 { next: Cell<S1>, prev: (), id: u32 }
pub struct S1 { next: Cell<S2>, prev: Option<Box<S0>>, id: u32 }
pub struct S2 { next: Cell<S3>, prev: Option<Box<S1>>, id: u32 }
pub struct S3 { next: Cell<S4>, prev: Option<Box<S2>>, id: u32 }
pub struct S4 { next: Cell<S5>, prev: Option<Box<S3>>, id: u32 }
pub struct S5 { next: Cell<S6>, prev: Option<Box<S4>>, id: u32 }
pub struct S6 { next: Cell<S7>, prev: Option<Box<S5>>, id: u32 }
pub struct S7 { next: Cell<S8>, prev: Option<Box<S6>>, id: u32 }
pub struct S8 { next: Cell<S9>, prev: Option<Box<S7>>, id: u32 }
pub struct S9 { next: Cell<S10>, prev: Option<Box<S8>>, id: u32 }
pub struct S10 { next: Cell<S11>, prev: Option<Box<S9>>, id: u32 }
pub struct S11 { next: Cell<S12>, prev: Option<Box<S10>>, id: u32 }
pub struct S12 { next: Cell<S13>, prev: Option<Box<S11>>, id: u32 }
pub struct S13 { next: Cell<S14>, prev: Option<Box<S12>>, id: u32 }
pub struct S14 { next: Cell<S15>, prev: Option<Box<S13>>, id: u32 }
pub struct S15 { next: u8, prev: Option<Box<S14>>, id: u32 }

fn assert_send<T: Send + Sync>(_: &T) -> u32 { 1 }

fn main() {
    let s = S0 { next: Cell(None, PhantomData), prev: (), id: 3 };
    let n = assert_send(&s) + assert_send(&Cell::<S7>(None, PhantomData));
    println!("auto_trait_cycle_outlives {} {}", n, s.id);
}
