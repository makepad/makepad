// A field reached through an autoderef of a reference (`get().desc.ty`, `src(0)?.desc.ty`)
// is a place in the pointee: the scrutinee borrows it, nothing is owned by the statement
// (rustc as_place.rs: the base `get()` becomes a temporary of type &T, the autoderef a Deref
// projection, the field a place behind it; dropping that temporary drops only the reference).
// Relax took `get().desc` for a value expression (rvalue_rooted followed the field chain to
// the call without looking at the autoderef), copied `desc` into a temporary and dropped the
// copy at the statement's end: the pointee's `desc` (its name String) was freed while the
// tensor still owned it. Found as the phone WM's SIGSEGV in the domain heap on 'llm-turns'
// (the on-device Translator: makepad_ai_llm vulkan_exec select(), `match src(0)?.desc.ty`).
use std::cell::RefCell;

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: String) { LOG.with(|l| l.borrow_mut().push(s)); }
fn take() -> Vec<String> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }
fn v(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }

struct D(&'static str);
impl Drop for D { fn drop(&mut self) { log(format!("drop {}", self.0)); } }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ty { F32, I32 }

struct Desc { name: Option<String>, ty: Ty, d: D }
struct Tensor { desc: Desc, ne: [i64; 2] }

fn mk(name: &'static str, ty: Ty) -> Tensor {
    Tensor { desc: Desc { name: Some(name.to_string()), ty, d: D(name) }, ne: [4, 1] }
}
fn get(ts: &[Tensor], i: usize) -> &Tensor { &ts[i] }
fn getr(ts: &[Tensor], i: usize) -> Result<&Tensor, String> { ts.get(i).ok_or_else(|| "missing".to_string()) }

struct Pair(D, u32);
fn pair_of(t: &(Pair, D)) -> &(Pair, D) { t }

#[test]
fn match_on_a_field_behind_a_returned_reference_drops_nothing() {
    take(); // a runner may run the tests on one thread
    let ts = vec![mk("a", Ty::F32), mk("b", Ty::I32)];
    let n = match get(&ts, 0).desc.ty { Ty::F32 => 1, _ => 2 };
    assert_eq!(n, 1);
    let n = match get(&ts, 1).desc.ty { Ty::F32 => 1, ty => ty as u32 + 10 };
    assert_eq!(n, 11);
    assert_eq!(take(), Vec::<String>::new());
    assert_eq!(get(&ts, 0).desc.name.as_deref(), Some("a"));
}

#[test]
fn match_on_a_field_behind_try_drops_nothing() {
    take(); // a runner may run the tests on one thread
    fn sel(ts: &[Tensor]) -> Result<u32, String> {
        let src = |i: usize| -> Result<&Tensor, String> { getr(ts, i) };
        let a = match src(0)?.desc.ty { Ty::F32 => 1, ty => ty as u32 };
        let b = match getr(ts, 1)?.desc.ty { Ty::F32 => 1, _ => 2 };
        Ok(a * 10 + b)
    }
    let ts = vec![mk("a", Ty::F32), mk("b", Ty::I32)];
    for _ in 0..3 {
        assert_eq!(sel(&ts), Ok(12));
    }
    assert_eq!(take(), Vec::<String>::new());
    assert_eq!(ts[0].desc.name.as_deref(), Some("a"));
    assert_eq!(ts[1].desc.name.as_deref(), Some("b"));
}

#[test]
fn if_let_and_nested_fields_behind_a_reference_drop_nothing() {
    take(); // a runner may run the tests on one thread
    let ts = vec![mk("a", Ty::F32)];
    let x = if let Ty::F32 = get(&ts, 0).desc.ty { 1 } else { 2 };
    assert_eq!(x, 1);
    let y = match ts.get(0).unwrap().desc.ty { Ty::F32 => 3, _ => 4 };
    assert_eq!(y, 3);
    let t = (Pair(D("p"), 7), D("q"));
    let z = match pair_of(&t).0 .1 { 7 => 5, _ => 6 };
    assert_eq!(z, 5);
    assert_eq!(take(), Vec::<String>::new());
    drop(t);
    assert_eq!(take(), v(&["drop p", "drop q"]));
}

#[test]
fn let_pattern_on_a_field_behind_a_reference_drops_nothing() {
    take(); // a runner may run the tests on one thread
    let ts = vec![mk("a", Ty::I32)];
    {
        let Desc { ty, .. } = get(&ts, 0).desc;
        assert_eq!(ty, Ty::I32);
        let Desc { ty: Ty::I32, ref name, .. } = get(&ts, 0).desc else { panic!() };
        assert_eq!(name.as_deref(), Some("a"));
    }
    assert_eq!(take(), Vec::<String>::new());
    drop(ts);
    assert_eq!(take(), v(&["drop a"]));
}

#[test]
fn a_field_of_a_value_still_drops_with_its_temporary() {
    take(); // a runner may run the tests on one thread
    // the rvalue case stays: `mk().desc.ty` is a place in one temporary of the statement
    let n = match mk("t", Ty::F32).desc.ty { Ty::F32 => 1, _ => 2 };
    assert_eq!(n, 1);
    assert_eq!(take(), v(&["drop t"]));
}
