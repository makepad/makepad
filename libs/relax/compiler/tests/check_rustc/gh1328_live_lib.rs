// GitHub #1328 use patterns (a library crate): constants used only in other constants, array
// lengths, patterns, statics, generic args, closures, macros, discriminants; methods used only
// from other methods, closures, fn pointers, trait impls; fields read through patterns, struct
// update and derives. rustc reports only the items marked DEAD.
mod shaders {
    pub(crate) const SH_QUAD: usize = 0;
    pub(crate) const SH_SIMPLE: usize = 1;
    pub(crate) const SH_TEXT: usize = 2;
    const SH_COUNT: usize = 3;
    pub(crate) const SHADER_NAMES: [&str; SH_COUNT] = ["quad", "simple", "text"];
    pub(crate) const SHADER_FLOATS: [usize; SH_COUNT] = [F_QUAD, F_SIMPLE, 4];
    const F_QUAD: usize = 8;
    const F_SIMPLE: usize = 6;
    const ONLY_IN_PATTERN: u8 = 7;
    const ONLY_IN_REPEAT: usize = 4;
    const ONLY_IN_STATIC: u32 = 9;
    static TABLE: [u32; 1] = [ONLY_IN_STATIC];
    const ONLY_IN_GENERIC: usize = 2;
    const ONLY_IN_CLOSURE: u32 = 5;
    const ONLY_IN_MACRO: u32 = 6;
    const ONLY_IN_ASSOC: u32 = 10;
    const ONLY_IN_DISCR: isize = 3;
    const ONLY_IN_CONST_FN: u32 = 11;
    const ONLY_IN_FIELD_TY: usize = 2;
    const ONLY_IN_TYPE_ALIAS: usize = 3;
    const ONLY_ASSERT: usize = 3;
    const _: () = assert!(ONLY_ASSERT == 3);
    type Buf = [u8; ONLY_IN_TYPE_ALIAS];
    pub(crate) struct Holder { pub(crate) v: [u8; ONLY_IN_FIELD_TY] }
    pub(crate) enum Kind { A = ONLY_IN_DISCR, B }
    pub(crate) struct Gen<const N: usize>;
    pub(crate) struct Assoc;
    impl Assoc { pub(crate) const X: u32 = ONLY_IN_ASSOC; }
    const fn cf() -> u32 { ONLY_IN_CONST_FN }
    pub(crate) const CF: u32 = cf();
    static FNS: [fn() -> u32; 2] = [fa, fb];
    fn fa() -> u32 { 1 }
    fn fb() -> u32 { 2 }
    macro_rules! m { () => { ONLY_IN_MACRO } }
    pub(crate) fn shader_floats(sh: usize) -> usize { SHADER_FLOATS[sh] }
    pub(crate) fn name(sh: usize) -> &'static str { SHADER_NAMES[sh] }
    pub(crate) fn all() -> u32 {
        let a = [0u8; ONLY_IN_REPEAT];
        let _g: Gen<ONLY_IN_GENERIC> = Gen;
        let c = || ONLY_IN_CLOSURE;
        let b: Buf = [0; 3];
        let k = match 7u8 { ONLY_IN_PATTERN => 1, _ => 0 };
        a.len() as u32 + TABLE[0] + c() + m!() + Assoc::X + CF + k + b.len() as u32 + Kind::A as u32 + Kind::B as u32
            + (SH_QUAD + SH_SIMPLE + SH_TEXT) as u32 + Holder { v: [0; 2] }.v.len() as u32 + FNS.iter().map(|f| f()).sum::<u32>()
    }
}
mod batch {
    use std::fmt;
    #[derive(Clone, Copy, Debug, Default)]
    pub(crate) struct Rect { pub(crate) x: f32, pub(crate) y: f32, pub(crate) w: f32, pub(crate) h: f32 }
    impl Rect {
        fn intersect_rect(&self, o: &Rect) -> Rect {
            Rect { x: self.x.max(o.x), y: self.y.max(o.y), w: self.w.min(o.w), h: self.h.min(o.h) }
        }
        pub(crate) fn clip(&self, o: &Rect) -> Rect { self.intersect_rect(o) }
    }
    pub(crate) struct Batcher { verts: Vec<f32>, count: usize, flushed: usize, label: String, dbg_only: u32, upd: u32, tuple: (u8, u8) }
    impl Batcher {
        pub(crate) fn new() -> Self { Batcher { verts: Vec::new(), count: 0, flushed: 0, label: String::new(), dbg_only: 0, upd: 1, tuple: (1, 2) } }
        fn add(&mut self, v: f32) { self.verts.push(v); self.count += 1; }
        fn flush(&mut self) { self.flushed += self.count; self.count = 0; }
        fn submit(&mut self) -> usize { let f: fn(&mut Batcher) = Self::flush; f(self); self.flushed }
        pub(crate) fn run(&mut self) { let add = |b: &mut Batcher, v| b.add(v); add(self, 1.0); }
        pub(crate) fn label_len(&self) -> usize { let Batcher { label, .. } = self; label.len() }
        pub(crate) fn clone_upd(&self) -> Batcher { Batcher { verts: Vec::new(), label: String::new(), ..Batcher { verts: Vec::new(), label: String::new(), ..*self } } }
        pub(crate) fn tup(&self) -> u8 { let (a, _) = self.tuple; a }
    }
    impl Clone for Batcher { fn clone(&self) -> Self { Batcher { verts: self.verts.clone(), label: self.label.clone(), ..*self } } }
    impl fmt::Debug for Batcher { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "{}", self.dbg_only) } }
    pub(crate) trait Sink { fn finish(&mut self) -> usize; }
    impl Sink for Batcher { fn finish(&mut self) -> usize { self.submit() } }
    impl Drop for Batcher { fn drop(&mut self) { let _ = self.upd; } }
    pub(crate) fn go() -> usize {
        let mut b = Batcher::new();
        b.run();
        let r = Rect::default().clip(&Rect::default());
        let _ = format!("{:?} {:?}", b.clone(), r);
        b.label_len() + b.clone_upd().count + b.tup() as usize + Sink::finish(&mut b)
    }
}
pub mod api {
    pub struct Ui { pub(crate) n: u32, hidden: u32 }
    impl Ui {
        pub fn new() -> Ui { Ui { n: 1, hidden: 2 } }
        pub fn total(&self) -> usize { crate::shaders::all() as usize + crate::batch::go() + crate::shaders::shader_floats(0) + crate::shaders::name(0).len() + self.n as usize }
        fn private_helper(&self) -> u32 { self.hidden }
        pub fn uses_helper(&self) -> u32 { self.private_helper() }
    }
    pub trait Draw { fn draw(&self) -> u32 { helper_in_default() } }
    fn helper_in_default() -> u32 { 3 }
}
mod inner {
    pub fn not_reachable() {} // DEAD
    pub struct NotReach { pub f: u32 } // DEAD
}
#[derive(PartialEq, Debug)]
pub(crate) struct OnlyDerive { a: u32 }
pub fn mk() -> bool { OnlyDerive { a: 1 } == OnlyDerive { a: 2 } }
fn truly_dead() {} // DEAD
const TRULY_DEAD: u32 = 0; // DEAD
struct DeadStruct { f: u32 } // DEAD
#[derive(Debug)]
pub struct DebugOnly { read_by_debug_only: u32 } // DEAD field: derived Debug is ignored
pub fn debug_only() -> String { format!("{:?}", DebugOnly { read_by_debug_only: 1 }) }
