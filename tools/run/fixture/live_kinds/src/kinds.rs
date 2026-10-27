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
