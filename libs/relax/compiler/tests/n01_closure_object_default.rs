// A closure's annotated return type `-> Result<Box<dyn Executor>, String>`
// has the object type's default lifetime `'static` (Box<T> has no lifetime
// bound: rustc_hir_analysis collect/resolve_bound_vars.rs object lifetime
// defaults), also when the closure is called through a blanket
// `impl<F: Fn(..) -> ..> Factory for F` instance (makepad flow
// ExecutorRegistry::register / ExecutorFactory::create<closure>).
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct Node {
    pub type_name: String,
    pub weight: u32,
}

pub struct Registry {
    pub scale: u32,
}

pub trait Executor {
    fn run(&mut self) -> u32;
}

pub trait ExecutorFactory: Send + Sync {
    fn create(&self, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String>;
}

impl<F> ExecutorFactory for F
where
    F: Fn(&Node, &Registry) -> Result<Box<dyn Executor>, String> + Send + Sync,
{
    fn create(&self, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String> {
        self(node, registry)
    }
}

#[derive(Default)]
pub struct ExecutorRegistry {
    factories: BTreeMap<String, Arc<dyn ExecutorFactory>>,
}

impl ExecutorRegistry {
    pub fn register(&mut self, owner: impl Into<String>, factory: impl ExecutorFactory + 'static) -> Result<(), String> {
        let owner = owner.into();
        if self.factories.contains_key(&owner) {
            return Err(format!("executor owner `{owner}` is already registered"));
        }
        self.factories.insert(owner, Arc::new(factory));
        Ok(())
    }

    pub fn create(&self, owner: &str, node: &Node, registry: &Registry) -> Result<Box<dyn Executor>, String> {
        self.factories
            .get(owner)
            .ok_or_else(|| format!("node `{}` needs unregistered job owner `{owner}`", node.type_name))?
            .create(node, registry)
    }
}

struct Check {
    value: u32,
}

impl Executor for Check {
    fn run(&mut self) -> u32 {
        self.value
    }
}

fn register(executors: &mut ExecutorRegistry) -> Result<(), String> {
    executors.register("gen.validate", |node: &Node, _: &Registry| -> Result<Box<dyn Executor>, String> {
        match node.type_name.as_str() {
            "Validate" | "ValidateBrief" => Ok(Box::new(Check { value: node.weight })),
            other => Err(format!("unsupported check node `{other}`")),
        }
    })?;
    executors.register("stage.text", |node: &Node, registry: &Registry| -> Result<Box<dyn Executor>, String> {
        Ok(Box::new(Check { value: node.weight * registry.scale }))
    })
}

#[test]
fn closure_factories_with_object_returns() {
    let mut executors = ExecutorRegistry::default();
    register(&mut executors).unwrap();
    let registry = Registry { scale: 3 };
    let node = Node { type_name: "Validate".into(), weight: 5 };
    assert_eq!(executors.create("gen.validate", &node, &registry).unwrap().run(), 5);
    assert_eq!(executors.create("stage.text", &node, &registry).unwrap().run(), 15);
    let other = Node { type_name: "Other".into(), weight: 1 };
    assert_eq!(executors.create("gen.validate", &other, &registry).err().unwrap(), "unsupported check node `Other`");
    assert!(executors.create("missing", &node, &registry).is_err());
    assert!(executors.register("stage.text", |_: &Node, _: &Registry| -> Result<Box<dyn Executor>, String> { Err(String::new()) }).is_err());
}
