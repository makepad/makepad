// `impl Display for Foo` is removed while the stored Box<dyn Display> lives, made by generic code
// (`boxed::<Foo>`) that `store` still calls: the call is checked, E0277.
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

fn boxed<T: Display + 'static>(t: T) -> Box<dyn Display> {
    Box::new(t)
}

fn store() {
    SHOWN.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(boxed(Foo(9)));
        }
    });
}

#[test]
fn run() {
    store();
    let shown = SHOWN.with(|s| s.borrow().as_ref().map(|d| d.to_string()).unwrap_or_default());
    println!("out {} [{}]", caller(&Foo(1)), shown);
}
