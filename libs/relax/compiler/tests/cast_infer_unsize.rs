// `expr as _` whose target is only known after the cast was checked (rustc: casts are checked
// at the end of typeck, check_casts; a cast whose target is an unsize of its source is that
// coercion, cast.rs try_coercion_cast): `Arc::new(v) as _` inside a closure's return becomes a fat
// `Arc<dyn Send + Sync>`. Also `type_name` of trait objects with auto traits. Lines "| " are
// compared with rustc --test.
use std::any::Any;
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;

struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        println!("| drop {}", self.0);
    }
}

enum Stage {
    Lines(Vec<D>),
    Empty,
}

impl Stage {
    fn into_retire(self) -> Option<Arc<dyn Send + Sync>> {
        match self {
            Stage::Lines(chunks) => (!chunks.is_empty()).then(|| Arc::new(chunks) as _),
            Stage::Empty => None,
        }
    }
}

fn boxed(n: u32) -> Option<Box<dyn Debug + Send + Sync>> {
    Some(n).map(|n| Box::new(n * 2) as _)
}

fn rc_any(s: &str) -> Rc<dyn Any> {
    let r = Rc::new(s.to_string()) as _;
    r
}

#[test]
fn a01_then_closure_arc() {
    let r = Stage::Lines(vec![D(1), D(2)]).into_retire();
    println!("| some {}", r.is_some());
    println!("| size {}", std::mem::size_of_val(&r));
    drop(r);
    println!("| none {}", Stage::Lines(Vec::new()).into_retire().is_none());
    println!("| empty {}", Stage::Empty.into_retire().is_none());
}

#[test]
fn a02_map_closure_box() {
    let b = boxed(21).unwrap();
    println!("| {:?}", b);
    let v: Vec<Box<dyn Debug + Send + Sync>> = (0..3).map(|i| Box::new(format!("s{}", i)) as _).collect();
    println!("| {:?}", v);
}

#[test]
fn a03_let_annotation() {
    let a: Arc<dyn Debug + Send + Sync> = Arc::new(D(3)) as _;
    let b: Box<dyn Any + Send> = Box::new(7u8) as _;
    println!("| {}", b.downcast_ref::<u8>().copied().unwrap_or(0));
    let r = rc_any("hi");
    println!("| {:?}", r.downcast_ref::<String>());
    let c: Rc<dyn Debug> = Rc::new([1, 2, 3]) as _;
    println!("| {:?}", c);
    let s: Box<[u16]> = Box::new([4u16, 5]) as _;
    println!("| {:?}", s);
    drop(a);
}

#[test]
fn a04_dyn_type_names() {
    println!("| {}", std::any::type_name::<dyn Send + Sync>());
    println!("| {}", std::any::type_name::<&(dyn Sync + Send)>());
    println!("| {}", std::any::type_name::<*const (dyn Any + Send + Sync)>());
    println!("| {}", std::any::type_name::<&dyn Debug>());
}

impl Debug for D {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "D({})", self.0)
    }
}
