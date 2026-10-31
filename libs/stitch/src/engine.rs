use {
    crate::{
        code::{CompiledCode, UncompiledCode},
        compile::Compiler,
        config::Extensions,
        func::Func,
        instance::Instance,
        store::Store,
    },
    std::sync::{Arc, Mutex},
};

/// A Wasm engine.
#[derive(Clone, Debug)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    /// Creates a new [`Engine`].
    pub fn new() -> Engine {
        Self::new_with_extensions(Extensions::default())
    }

    /// Creates a new [`Engine`] with the given nonstandard [`Extensions`]
    /// enabled.
    pub fn new_with_extensions(extensions: Extensions) -> Engine {
        Engine {
            inner: Arc::new(EngineInner {
                extensions,
                compilers: Mutex::new(Pool::new()),
            }),
        }
    }

    /// The nonstandard [`Extensions`] enabled for this [`Engine`].
    pub fn extensions(&self) -> Extensions {
        self.inner.extensions
    }

    pub(crate) fn compile(
        &self,
        store: &mut Store,
        func: Func,
        instance: &Instance,
        code: &UncompiledCode,
    ) -> CompiledCode {
        let mut compiler = self.inner.compilers.lock().unwrap().pop_or_default();
        let result = compiler.compile(store, func, instance, code);
        self.inner.compilers.lock().unwrap().push(compiler);
        result
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct EngineInner {
    extensions: Extensions,
    compilers: Mutex<Pool<Compiler>>,
}

#[derive(Debug)]
struct Pool<T> {
    items: Vec<T>,
}

impl<T> Pool<T>
where
    T: Default,
{
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn pop_or_default(&mut self) -> T {
        self.items.pop().unwrap_or_default()
    }

    fn push(&mut self, item: T) {
        self.items.push(item);
    }
}
