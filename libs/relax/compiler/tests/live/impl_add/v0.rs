// 3.9: trait and inherent impls added; method resolution changes.
//  v1: `impl Display for Foo` is added and a Display value is stored as Box<dyn Display>.
//  v2: an inherent `name` is added that shadows the trait method `Named::name`: the caller's
//      result changes although the caller's text does not.
use std::cell::RefCell;
use std::fmt::Display;

struct Foo(u32);

trait Named {
    fn name(&self) -> String {
        "trait".to_string()
    }
}
impl Named for Foo {}

fn caller(f: &Foo) -> String {
    f.name()
}

thread_local! {
    static SHOWN: RefCell<Option<Box<dyn Display>>> = RefCell::new(None);
}

fn store() {}

#[test]
fn run() {
    store();
    let shown = SHOWN.with(|s| s.borrow().as_ref().map(|d| d.to_string()).unwrap_or_default());
    println!("out {} [{}]", caller(&Foo(1)), shown);
}
