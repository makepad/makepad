use {
    crate::{
        code::{CompiledCode, UncompiledCode},
        compile::Compiler,
        decode::DecodeError,
        func::{Func, FuncType},
        instance::Instance,
        module::ModuleBuilder,
        store::Store,
        validate::Validator,
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
        Engine {
            inner: Arc::new(EngineInner {
                validators: Mutex::new(Pool::new()),
                compilers: Mutex::new(Pool::new()),
            }),
        }
    }

    pub(crate) fn validate(
        &self,
        type_: &FuncType,
        module: &ModuleBuilder,
        code: &UncompiledCode,
    ) -> Result<(), DecodeError> {
        let mut validator = self.inner.validators.lock().unwrap().pop_or_default();
        let result = validator.validate(type_, module, code);
        self.inner.validators.lock().unwrap().push(validator);
        result
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
    validators: Mutex<Pool<Validator>>,
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
