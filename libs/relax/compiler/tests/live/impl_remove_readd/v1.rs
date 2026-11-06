// `impl Display for Foo` is removed while the stored Box<dyn Display> lives (applied live), then
// added again with another body: a new coercion would take the vtable that holds the removed
// impl's code, so that edit restarts.
use std::cell::RefCell;
use std::fmt::Display;

struct Foo(u32);

trait Named {
    fn name(&self) -> String {
        "trait".to_string()
    }
}
impl Named for Foo {}
impl Foo {
    fn name(&self) -> String {
        "inherent".to_string()
    }
}

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
