// More inference / resolution cases from the runner apps' compile check; each = rustc.
// Lines "| " are compared with rustc.

// a `use` that binds both namespaces of a name whose value side comes from a later import
// (`mod parse;` + `pub use parse::parse;` at the root, `use crate::parse;` in a module)
mod parse {
    use crate::M;
    pub fn parse(x: &[u8]) -> Result<M, ()> {
        Ok(M(x.len() as u32))
    }
}
mod model {
    use crate::parse;
    pub struct M(pub u32);
    impl M {
        pub fn parse(bytes: &[u8]) -> Result<Self, ()> {
            parse(bytes)
        }
    }
}
pub use model::*;
pub use parse::parse;

#[test]
fn j01_both_namespaces() {
    println!("| {}", M::parse(b"abc").unwrap().0);
}

// `impl Trait` nested in another one's bound is an anonymous param too
struct B {
    deps: Vec<String>,
}
impl B {
    fn with_deps(mut self, deps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.deps.extend(deps.into_iter().map(Into::into));
        self
    }
}
#[test]
fn j02_nested_apit() {
    let b = B { deps: vec![] }.with_deps(["a", "b"]).with_deps(vec![String::from("c")]);
    println!("| {:?}", b.deps);
}

// several impls of a trait for the receiver; `T: Copy` rules one out (Vec<String>: Extend<String>
// only), which fixes `Into::into`'s target
#[test]
fn j03_copy_bound_selects() {
    let mut v: Vec<String> = vec![];
    v.extend(["a", "b"].into_iter().map(Into::into));
    let mut w: Vec<String> = vec![];
    w.extend(["c"].into_iter().map(|x| x.into()));
    let mut n: Vec<u32> = vec![1];
    n.extend([2u32, 3].iter());
    println!("| {:?} {:?} {:?}", v, w, n);
}

// core's built-in macros import by name
use core::compile_error as _ce;
#[allow(unused_imports)]
use core::prelude::v1::derive;
#[test]
fn j04_builtin_macro_imports() {
    println!("| ok");
}
