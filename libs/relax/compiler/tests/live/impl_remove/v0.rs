// 3.9 / 7.3: `impl Display for Foo` is removed while the stored Box<dyn Display> lives (no coercion
//      remains in the source): the stored box keeps working through its frozen vtable.
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
impl std::fmt::Display for Foo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "foo{}", self.0)
    }
}

fn caller(f: &Foo) -> String {
    f.name()
}

thread_local! {
    static SHOWN: RefCell<Option<Box<dyn Display>>> = RefCell::new(None);
}

fn store() {
    SHOWN.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(Box::new(Foo(9)));
        }
    });
}

#[test]
fn run() {
    store();
    let shown = SHOWN.with(|s| s.borrow().as_ref().map(|d| d.to_string()).unwrap_or_default());
    println!("out {} [{}]", caller(&Foo(1)), shown);
}
