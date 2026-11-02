// One probe per edit kind (tools/run/fixture/live_kinds.py). Each edit rewrites one marked
// item; its probe's printed value must change. Keep every item on the lines the driver's edits
// match (they replace exact text).
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

// fn body
pub fn body() -> u64 { 1 }

// called straight from main's loop (a frame that never returns)
#[inline]
pub fn tiny() -> u64 { 1 }

// const
pub const K: u64 = 10;
pub fn konst() -> u64 { K }

// immutable static
pub static S: u64 = 100;
pub fn stat() -> u64 { S }

// static with interior mutability
pub static COUNTER: AtomicU64 = AtomicU64::new(0);
pub fn counter() -> u64 { COUNTER.fetch_add(1, Ordering::Relaxed) }

// thread_local
thread_local! { static TL: Cell<u64> = Cell::new(5); }
pub fn tl() -> u64 { TL.with(|c| c.get()) }

// struct layout, held in State across ticks
pub struct Rec { pub a: u64, pub c: u64 }
pub fn rec_new() -> Rec { Rec { a: 1, c: 2 } }
pub fn rec_val(r: &Rec) -> u64 { r.a * 10 + r.c }

// field type change
pub struct Wide { pub x: u32 }
pub fn wide_val(w: &Wide) -> u64 { w.x as u64 }

// enum variant set
pub enum Mode { A, B }
pub fn mode_val(m: &Mode) -> u64 { match m { Mode::A => 1, Mode::B => 2 } }

// fn signature
pub fn sig(a: u64) -> u64 { a }
pub fn sig_val() -> u64 { sig(1) }

// item set: added fn, removed fn, added struct+impl, added mod
pub fn newfn_val() -> u64 { 0 }
pub fn doomed() -> u64 { 3 }
pub fn del_val() -> u64 { doomed() }
pub fn news_val() -> u64 { 0 }
pub fn mod_val() -> u64 { 0 }

// traits: a method reached through a vtable (a boxed value made at startup), an impl added,
// an impl removed, a trait's item list
pub trait Shape { fn area(&self) -> u64; }
pub struct Sq;
impl Shape for Sq { fn area(&self) -> u64 { 1 } }
pub struct Tri;
pub fn impl_add_val() -> u64 { 0 }
pub struct Circ;
impl Shape for Circ { fn area(&self) -> u64 { 5 } }
pub fn impl_rm_val() -> u64 { Circ.area() }
pub trait Named { fn name_len(&self) -> u64; }
impl Named for Sq { fn name_len(&self) -> u64 { 2 } }
pub fn trait_items_val(s: &dyn Named) -> u64 { s.name_len() }

// generic fn
pub fn gen<T: Into<u64>>(t: T) -> u64 { t.into() }
pub fn gen_val() -> u64 { gen(1u32) }

// closure made per call, closure stored at startup
pub fn clo_val() -> u64 { let f = |x: u64| x + 1; f(0) }
pub fn make_clo() -> Box<dyn Fn() -> u64> { Box::new(|| 1) }

// macro_rules
macro_rules! mac { () => { 1 } }
pub fn mac_val() -> u64 { mac!() }

// use
use std::cmp::max as pick;
pub fn use_val() -> u64 { pick(1, 2) }

// type alias
pub type Num = u64;
pub fn alias_val() -> u64 { std::mem::size_of::<Num>() as u64 }

// the construct kinds (live_kinds.py): one item and one probe each
pub fn apit(it: impl IntoIterator<Item = u64>) -> u64 { it.into_iter().sum() }
pub fn apit_val() -> u64 { apit([1, 2]) }
pub fn apit2(a: u64) -> u64 { a }
pub fn apit2_val() -> u64 { apit2(1) }
pub fn apit3_val() -> u64 { 0 }
pub fn gen2<A: Into<u64>>(a: A) -> u64 { a.into() }
pub fn gen2_val() -> u64 { gen2(1u32) }
pub struct Arr<const N: usize>;
impl<const N: usize> Arr<N> { pub fn len() -> u64 { N as u64 } }
pub fn cgen_val() -> u64 { Arr::<2>::len() }
pub fn gstruct_val() -> u64 { 0 }
pub fn tblanket_val() -> u64 { 0 }
pub trait Gone { fn g(&self) -> u64; }
impl Gone for u8 { fn g(&self) -> u64 { 6 } }
pub fn trm_val() -> u64 { 1u8.g() }
pub trait Def { fn d(&self) -> u64 { 1 } }
impl Def for u8 {}
pub fn tdef_val() -> u64 { 1u8.d() }
pub trait AC { const V: u64; }
impl AC for u8 { const V: u64 = 1; }
pub fn aconst_val() -> u64 { <u8 as AC>::V }
pub trait AT { type Out; }
impl AT for u8 { type Out = u32; }
pub fn atype_val() -> u64 { std::mem::size_of::<<u8 as AT>::Out>() as u64 }
pub fn ccap_val() -> u64 { let a = 1u64; let f = || a; f() }
pub fn cnest_val() -> u64 { (|| (|| 1u64)())() }
pub fn rpit() -> impl Iterator<Item = u64> { (0..2u64).map(|x| x) }
pub fn rpit_val() -> u64 { rpit().count() as u64 }
pub fn sadd_val() -> u64 { 0 }
static OLD_S: u64 = 13;
pub fn srm_val() -> u64 { OLD_S }
pub fn cadd_val() -> u64 { 0 }
const OLD_C: u64 = 16;
pub fn crm_val() -> u64 { OLD_C }
const LEN: usize = 2;
pub fn clen_val() -> u64 { [0u8; LEN].len() as u64 }
pub fn madd_val() -> u64 { 0 }
macro_rules! m3 { () => { 19 } }
pub fn mrm_val() -> u64 { m3!() }
pub fn mfn_val() -> u64 { 0 }
mod oldm { pub fn v() -> u64 { 22 } }
pub fn modrm_val() -> u64 { oldm::v() }
pub enum E2 { A, B }
pub fn evrm_of(e: E2) -> u64 { match e { E2::A => 1, E2::B => 2 } }
pub fn evrm_val() -> u64 { evrm_of(E2::A) }
pub enum E3 { A(u8), B }
impl E3 { pub fn v(&self) -> u64 { match self { E3::A(x) => *x as u64, E3::B => 0 } } }
pub fn epay_val() -> u64 { E3::A(1).v() }
#[derive(Clone, Copy)]
pub enum E4 { A = 1 }
pub fn edis_val() -> u64 { E4::A as u64 }
struct D1;
pub fn dadd_val() -> u64 { 0 }
#[derive(Clone)]
struct D2(u64);
pub fn drm_val() -> u64 { D2(3).clone().0 }
#[cfg(any())]
pub fn cfg_on() -> u64 { 2 }
#[cfg(not(any()))]
pub fn cfg_on() -> u64 { 1 }
pub fn cfg2_val() -> u64 { let mut n = 1; #[cfg(any())] { n = 2; } n }
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = Box::pin(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}
async fn af() -> u64 { 1 }
pub fn async1_val() -> u64 { block_on(af()) }
pub fn async2_val() -> u64 { 0 }
pub fn dropi_val() -> u64 { 0 }
pub fn life_of(a: &u64) -> u64 { *a }
pub fn life_val() -> u64 { life_of(&1) }
extern "C" fn ec() -> u64 { 1 }
pub fn ecfn_val() -> u64 { let f: extern "C" fn() -> u64 = ec; f() }
fn tbl_a() -> u64 { 1 }
static TABLE: [fn() -> u64; 1] = [tbl_a];
pub fn fptr_val() -> u64 { TABLE[0]() }
pub fn mfile_val() -> u64 { 0 }
#[inline(never)]
pub fn partial_landing_val() -> u64 { 0 }
pub fn in_use() -> u64 { 1 }
pub trait Kept { fn k(&self) -> u64; }
impl Kept for u16 { fn k(&self) -> u64 { 1 } }
pub fn rmuse_val() -> u64 { in_use() + 1u16.k() }

pub fn report2() -> String {
    format!(
        "apit={} apit2={} apit3={} gen2={} cgen={} gstruct={} tblanket={} trm={} tdef={} aconst={} atype={} ccap={} cnest={} rpit={} sadd={} srm={} cadd={} crm={} clen={} madd={} mrm={} mfn={} modrm={} evrm={} epay={} edis={} dadd={} drm={} cfg1={} cfg2={} async1={} async2={} dropi={} life={} ecfn={} fptr={} mfile={} partial_landing={} rmuse={}",
        apit_val(), apit2_val(), apit3_val(), gen2_val(), cgen_val(), gstruct_val(), tblanket_val(), trm_val(), tdef_val(), aconst_val(), atype_val(), ccap_val(), cnest_val(), rpit_val(), sadd_val(), srm_val(), cadd_val(), crm_val(), clen_val(), madd_val(), mrm_val(), mfn_val(), modrm_val(), evrm_val(), epay_val(), edis_val(), dadd_val(), drm_val(), cfg_on(), cfg2_val(), async1_val(), async2_val(), dropi_val(), life_val(), ecfn_val(), fptr_val(), mfile_val(), partial_landing_val(), rmuse_val(),
    )
}

pub struct State {
    pub rec: Rec,
    pub wide: Wide,
    pub mode: Mode,
    pub shape: Box<dyn Shape>,
    pub named: Box<dyn Named>,
    pub clo: Box<dyn Fn() -> u64>,
}

impl State {
    pub fn new() -> State {
        State { rec: rec_new(), wide: Wide { x: 7 }, mode: Mode::A, shape: Box::new(Sq), named: Box::new(Sq), clo: make_clo() }
    }
}

pub fn report(st: &mut State) -> String {
    format!(
        "body={} const={} static={} counter={} tls={} field_add={} field_remove={} field_reorder={} field_type={} enum_variant={} signature={} fn_added={} fn_removed={} struct_added={} mod_added={} vtable={} impl_added={} impl_removed={} trait_items={} generic={} closure={} closure_stored={} macro={} use={} alias={}",
        body(), konst(), stat(), counter(), tl(),
        rec_val(&st.rec), rec_val(&st.rec), rec_val(&st.rec), wide_val(&st.wide), mode_val(&st.mode),
        sig_val(), newfn_val(), del_val(), news_val(), mod_val(),
        st.shape.area(), impl_add_val(), impl_rm_val(), trait_items_val(&*st.named),
        gen_val(), clo_val(), (st.clo)(), mac_val(), use_val(), alias_val(),
    )
}
