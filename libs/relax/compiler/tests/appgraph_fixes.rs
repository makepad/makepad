// rustc-accepted code the deep app check found Relax rejecting (bring-up appgraph, b7989c60a0):
// libs/model (a block's `use Enum::*` shadows the module's glob struct of the same name, variants
// are in the type namespace too), std path.rs (`self.cmp(o)` on `&C` with C both Iterator and Ord
// is Ord::cmp by value at the `&C` step), box3d (`module::Alias { .. }` struct literals), live_video
// (an inherent `from(&self)` one autoderef down beats From::from, which has no self), memchr / box3d
// (NEON / SSE vectors are Debug), cc (Command::get_args / get_envs / get_current_dir), cc again
// (`impl IntoIterator` -> into_iter() -> map / collect through the associated type's bounds)
use std::cmp::Ordering;
use std::sync::Arc;

mod m {
    #[derive(Debug, Default, PartialEq)]
    pub struct Opts { pub n: u32 }
    #[derive(Debug)]
    pub enum Op { Opts { name: String, options: Opts }, Other }
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Transform { pub p: f32, pub q: f32 }
    pub type WorldTransform = Transform;
}
use m::*;

fn make(k: u32) -> Op {
    use Op::*;
    match k {
        0 => Opts { name: "ab".into(), options: m::Opts { n: 3 } },
        _ => Other,
    }
}
fn size(o: &Op) -> usize {
    use Op::*;
    match o {
        Opts { name, options } => name.len() + options.n as usize,
        Other => 0,
    }
}

#[test]
fn block_glob_variant_shadows_struct() {
    assert_eq!(size(&make(0)), 5);
    assert_eq!(size(&make(1)), 0);
    assert_eq!(Opts::default(), m::Opts { n: 0 });
}

#[derive(Clone)]
struct C<'a> { s: &'a [u8], i: usize }
impl<'a> Iterator for C<'a> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> { let r = self.s.get(self.i).copied(); self.i += 1; r }
}
impl<'a> PartialEq for C<'a> { fn eq(&self, o: &C<'a>) -> bool { self.s == o.s } }
impl<'a> Eq for C<'a> {}
impl<'a> PartialOrd for C<'a> { fn partial_cmp(&self, other: &C<'a>) -> Option<Ordering> { Some(self.cmp(other)) } }
impl<'a> Ord for C<'a> { fn cmp(&self, o: &C<'a>) -> Ordering { self.s.cmp(o.s) } }

#[test]
fn ord_cmp_on_a_reference_to_an_iterator() {
    let a = C { s: b"ab", i: 0 };
    let b = C { s: b"ac", i: 0 };
    assert_eq!(PartialOrd::partial_cmp(&a, &b), Some(Ordering::Less));
    assert!(a < b);
}

#[test]
fn struct_literal_through_a_path_to_an_alias() {
    let t = crate::m::WorldTransform { p: 1.0, q: 2.0 };
    let u = m::WorldTransform { p: 3.0, ..t };
    let m::WorldTransform { p, .. } = u;
    assert_eq!((t.q, p), (2.0, 3.0));
}

struct Budget { id: u32 }
struct Lease { budget: Arc<Budget> }
impl Lease {
    fn from(&self, b: &Arc<Budget>) -> bool { Arc::ptr_eq(&self.budget, b) }
}
struct Src { lease: Arc<Lease> }

#[test]
fn inherent_method_below_an_arc_beats_a_selfless_trait_fn() {
    let b = Arc::new(Budget { id: 1 });
    let s = Src { lease: Arc::new(Lease { budget: b.clone() }) };
    assert!(s.lease.from(&b));
    assert_eq!(b.id, 1);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn simd_vectors_are_debug() {
    use std::arch::aarch64::*;
    unsafe {
        let a = vdupq_n_u8(3);
        let f = vdupq_n_f32(1.5);
        assert_eq!(format!("{:?}", a), "uint8x16_t(3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3)");
        let d: &dyn std::fmt::Debug = &f;
        assert_eq!(format!("{:?}", d), "float32x4_t(1.5, 1.5, 1.5, 1.5)");
    }
}

#[test]
fn command_getters() {
    let mut c = std::process::Command::new("cc");
    c.arg("-O2").args(["-c", "x.c"]).env("B", "2").env("A", "1").env_remove("C").current_dir("/tmp");
    let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, ["-O2", "-c", "x.c"]);
    assert_eq!(c.get_args().len(), 3);
    assert_eq!(format!("{:?}", c.get_args()), r#"CommandArgs { inner: ["-O2", "-c", "x.c"] }"#);
    let envs: Vec<String> = c.get_envs().map(|(k, v)| format!("{:?}={:?}", k, v)).collect();
    assert_eq!(envs, [r#""A"=Some("1")"#, r#""B"=Some("2")"#, r#""C"=None"#]);
    assert_eq!(c.get_current_dir(), Some(std::path::Path::new("/tmp")));
}

struct Tool { env: Vec<(String, String)> }
impl Tool {
    fn env(&self) -> impl IntoIterator<Item = &(String, String)> { self.env.iter() }
}

#[test]
fn opaque_into_iterator_chains() {
    let t = Tool { env: vec![("A".into(), "1".into()), ("B".into(), "2".into())] };
    let v: Vec<(String, String)> = t.env().into_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(v[1].0, "B");
    assert_eq!(t.env().into_iter().count(), 2);
}
