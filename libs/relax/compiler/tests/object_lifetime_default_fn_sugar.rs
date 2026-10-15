use std::collections::BTreeMap;
use std::sync::Arc;
pub struct Node;
pub struct Registry;
pub trait Executor { fn go(&self) -> u32; }
struct E1;
impl Executor for E1 { fn go(&self) -> u32 { 7 } }
pub trait ExecutorFactory: Send + Sync {
    fn create(&self, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String>;
}
impl<F> ExecutorFactory for F
where
    F: Fn(&Node, &Registry) -> Result<Box<dyn Executor>, String> + Send + Sync,
{
    fn create(&self, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String> { self(node, registry) }
}
#[derive(Clone, Default)]
pub struct Providers { factories: BTreeMap<String, Arc<dyn ExecutorFactory>> }
impl Providers {
    pub fn add(&mut self, k: &str, f: impl Fn(&Node, &Registry) -> Result<Box<dyn Executor>, String> + Send + Sync + 'static) {
        self.factories.insert(k.to_string(), Arc::new(f));
    }
    pub fn create(&self, k: &str) -> Result<Box<dyn Executor>, String> {
        self.factories.get(k).ok_or("none")?.create(&Node, &Registry)
    }
}
#[test]
fn default_object_lifetimes() {
    let mut p = Providers::default();
    p.add("a", |_, _| Ok(Box::new(E1)));
    let q = p.clone();
    assert_eq!(q.create("a").unwrap().go(), 7);
}

// A default method also inherits callable bounds from its trait's owner.
trait TraitFactory<F>
where
    F: Fn(&Node, &Registry) -> Result<Box<dyn Executor>, String>,
{
    fn create(&self, f: &F) -> Result<Box<dyn Executor>, String> {
        f(&Node, &Registry)
    }
}

impl<F> TraitFactory<F> for ()
where
    F: Fn(&Node, &Registry) -> Result<Box<dyn Executor>, String>,
{}

#[test]
fn trait_callable_output_defaults_to_static() {
    fn factory(_: &Node, _: &Registry) -> Result<Box<dyn Executor>, String> {
        Ok(Box::new(E1))
    }
    let executor: Box<dyn Executor + 'static> = TraitFactory::create(&(), &factory).unwrap();
    assert_eq!(executor.go(), 7);
}

struct BorrowedExecutor<'a>(&'a u32);
impl Executor for BorrowedExecutor<'_> {
    fn go(&self) -> u32 { *self.0 }
}

struct BorrowingFactory<F>(F);
impl<'a, F> BorrowingFactory<F>
where
    F: Fn() -> Box<dyn Executor + 'a>,
{
    fn create(&self) -> Box<dyn Executor + 'a> { (self.0)() }
}

#[test]
fn inherited_callable_output_preserves_explicit_lifetime() {
    let value = 11;
    let factory = BorrowingFactory(|| -> Box<dyn Executor + '_> {
        Box::new(BorrowedExecutor(&value))
    });
    assert_eq!(factory.create().go(), 11);
}
