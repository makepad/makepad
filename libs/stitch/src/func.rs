use {
    crate::{
        aliased_box::AliasableBox,
        decode::{Decode, DecodeError, Decoder},
        error::Error,
        exec,
        instance::Instance,
        stack::StackGuard,
        store::{Handle, InternedFuncType, Store, StoreId, UnguardedHandle},
        val::{Val, ValType},
        wrap::Wrap,
    },
    std::{error, fmt, mem, sync::Arc},
};

/// A WebAssembly function.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Func(pub(crate) Handle<FuncEntity>);

impl Func {
    pub fn wrap<Ts, U>(store: &mut Store, f: impl Wrap<Ts, U>) -> Self {
        let (type_, trampoline) = f.wrap();
        let type_ = store.get_or_intern_type(&type_);
        Self(store.insert_func(HostFuncEntity::new(type_, trampoline).into()))
    }

    pub(crate) fn new_wasm(
        store: &mut Store,
        type_: &FuncType,
        instance: Instance,
        code: UncompiledCode,
    ) -> Self {
        let type_ = store.get_or_intern_type(type_);
        Self(store.insert_func(WasmFuncEntity::new(type_, instance, code).into()))
    }

    /// Returns the [`FuncType`] of this [`Func`].
    pub fn type_(self, store: &Store) -> &FuncType {
        store.resolve_type(self.0.as_ref(store).interned_type())
    }

    pub fn compile(self, store: &mut Store) {
        let FuncEntity::Wasm(func) = self.0.as_mut(store) else {
            return;
        };
        let instance = func.instance().clone();
        let code = match mem::replace(func.code_mut(), Code::Compiling) {
            Code::Uncompiled(code) => store
                .engine()
                .clone()
                .compile(store, self, &instance, &code),
            Code::Compiling => panic!(),
            Code::Compiled(state) => state,
        };
        let FuncEntity::Wasm(func) = self.0.as_mut(store) else {
            return;
        };
        *func.code_mut() = Code::Compiled(code);
    }

    pub fn call(self, store: &mut Store, params: &[Val], results: &mut [Val]) -> Result<(), Error> {
        let type_ = self.type_(store);
        if params.len() != type_.params().len() {
            return Err(FuncError::ParamCountMismatch)?;
        }
        if results.len() != type_.results().len() {
            return Err(FuncError::ResultCountMismatch)?;
        }
        for (params, param_type) in params.iter().zip(type_.params().iter().copied()) {
            if params.type_() != param_type {
                return Err(FuncError::ParamTypeMismatch)?;
            }
        }
        exec::exec(store, self, params, results)
    }

    /// Converts the given [`UnguardedFunc`] to a [`Func`].
    ///
    /// # Safety
    ///
    /// The given [`UnguardedFunc`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(func: UnguardedFunc, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(func, store_id))
    }

    /// Converts this [`Func`] to an [`UnguardedFunc`].
    ///
    /// # Panics
    ///
    /// This [`Func`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedFunc {
        self.0.to_unguarded(store_id)
    }
}

/// An unguarded version of [`Func`].
pub(crate) type UnguardedFunc = UnguardedHandle<FuncEntity>;

/// The type of a [`Func`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FuncType {
    params_results: Arc<[ValType]>,
    param_count: usize,
}

impl FuncType {
    pub fn new(
        params: impl IntoIterator<Item = ValType>,
        results: impl IntoIterator<Item = ValType>,
    ) -> Self {
        let mut params_results = params.into_iter().collect::<Vec<_>>();
        let param_count = params_results.len();
        params_results.extend(results);
        Self {
            params_results: params_results.into(),
            param_count,
        }
    }

    pub fn params(&self) -> &[ValType] {
        &self.params_results[..self.param_count]
    }

    pub fn results(&self) -> &[ValType] {
        &self.params_results[self.param_count..]
    }

    pub(crate) fn from_val_type(type_: Option<ValType>) -> FuncType {
        thread_local! {
            static TYPES: [FuncType; 7] = [
                FuncType::new(vec![], vec![]),
                FuncType::new(vec![], vec![ValType::I32]),
                FuncType::new(vec![], vec![ValType::I64]),
                FuncType::new(vec![], vec![ValType::F32]),
                FuncType::new(vec![], vec![ValType::F64]),
                FuncType::new(vec![], vec![ValType::FuncRef]),
                FuncType::new(vec![], vec![ValType::ExternRef]),
            ];
        }

        TYPES.with(|types| match type_ {
            None => types[0].clone(),
            Some(ValType::I32) => types[1].clone(),
            Some(ValType::I64) => types[2].clone(),
            Some(ValType::F32) => types[3].clone(),
            Some(ValType::F64) => types[4].clone(),
            Some(ValType::FuncRef) => types[5].clone(),
            Some(ValType::ExternRef) => types[6].clone(),
        })
    }

    pub(crate) fn callee_stack_slot_count(&self) -> usize {
        self.params().len().max(self.results().len()) + 4
    }
}

impl Decode for FuncType {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        if decoder.read_byte()? != 0x60 {
            return Err(DecodeError::new("malformed function type"))?;
        }
        let mut param_result_types: Vec<_> = decoder.decode_iter()?.collect::<Result<_, _>>()?;
        let param_count = param_result_types.len();
        let result_types = decoder.decode_iter()?;
        param_result_types.reserve(result_types.size_hint().0);
        for result_type in result_types {
            param_result_types.push(result_type?);
        }
        Ok(Self {
            params_results: param_result_types.into(),
            param_count,
        })
    }
}

/// An error that can occur when operating on a [`Func`].
#[derive(Clone, Copy, Debug)]
pub enum FuncError {
    ParamCountMismatch,
    ParamTypeMismatch,
    ResultCountMismatch,
}

impl fmt::Display for FuncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParamCountMismatch => write!(f, "function parameter count mismatch"),
            Self::ParamTypeMismatch => write!(f, "function parameter type mismatch"),
            Self::ResultCountMismatch => write!(f, "function result count mismatch"),
        }
    }
}

impl error::Error for FuncError {}

/// The representation of a [`Func`] in a [`Store`].
#[derive(Debug)]
pub enum FuncEntity {
    Wasm(WasmFuncEntity),
    Host(HostFuncEntity),
}

impl FuncEntity {
    pub(crate) fn interned_type(&self) -> InternedFuncType {
        match self {
            Self::Wasm(func) => func.interned_type(),
            Self::Host(func) => func.interned_type(),
        }
    }
}

impl From<WasmFuncEntity> for FuncEntity {
    fn from(func: WasmFuncEntity) -> Self {
        Self::Wasm(func)
    }
}

impl From<HostFuncEntity> for FuncEntity {
    fn from(func: HostFuncEntity) -> Self {
        Self::Host(func)
    }
}

#[derive(Debug)]
pub(crate) struct WasmFuncEntity {
    type_: InternedFuncType,
    instance: Instance,
    code: Code,
}

impl WasmFuncEntity {
    fn new(type_: InternedFuncType, instance: Instance, code: UncompiledCode) -> WasmFuncEntity {
        WasmFuncEntity {
            type_,
            instance,
            code: Code::Uncompiled(code),
        }
    }

    pub(crate) fn interned_type(&self) -> InternedFuncType {
        self.type_
    }

    pub(crate) fn instance(&self) -> &Instance {
        &self.instance
    }

    pub(crate) fn code(&self) -> &Code {
        &self.code
    }

    pub(crate) fn code_mut(&mut self) -> &mut Code {
        &mut self.code
    }
}

#[derive(Debug)]
pub(crate) enum Code {
    Uncompiled(UncompiledCode),
    Compiling,
    Compiled(CompiledCode),
}

#[derive(Clone, Debug)]
pub(crate) struct UncompiledCode {
    pub(crate) locals: Box<[ValType]>,
    pub(crate) expr: Arc<[u8]>,
}

impl Decode for UncompiledCode {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        use std::iter;

        let mut code_decoder = decoder.decode_decoder()?;
        Ok(Self {
            locals: {
                let mut locals = Vec::new();
                for _ in 0u32..code_decoder.decode()? {
                    let count = code_decoder.decode()?;
                    if count > usize::try_from(u32::MAX).unwrap() - locals.len() {
                        return Err(DecodeError::new("too many locals"));
                    }
                    locals.extend(iter::repeat(code_decoder.decode::<ValType>()?).take(count));
                }
                locals.into()
            },
            expr: code_decoder.read_bytes_until_end().into(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct CompiledCode {
    pub(crate) max_stack_slot_count: usize,
    pub(crate) local_count: usize,
    pub(crate) code: AliasableBox<[InstrSlot]>,
}

pub(crate) type InstrSlot = usize;

#[derive(Debug)]
pub struct HostFuncEntity {
    type_: InternedFuncType,
    trampoline: HostFuncTrampoline,
}

impl HostFuncEntity {
    pub(crate) fn new(type_: InternedFuncType, trampoline: HostFuncTrampoline) -> Self {
        Self { type_, trampoline }
    }

    pub(crate) fn interned_type(&self) -> InternedFuncType {
        self.type_
    }

    pub(crate) fn trampoline(&self) -> &HostFuncTrampoline {
        &self.trampoline
    }
}

#[derive(Clone)]
pub struct HostFuncTrampoline {
    inner: Arc<dyn Fn(&mut Store, StackGuard) -> Result<StackGuard, Error> + Send + Sync + 'static>,
}

impl HostFuncTrampoline {
    pub fn new(
        inner: impl Fn(&mut Store, StackGuard) -> Result<StackGuard, Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub(crate) fn call(&self, store: &mut Store, stack: StackGuard) -> Result<StackGuard, Error> {
        (self.inner)(store, stack)
    }
}

impl fmt::Debug for HostFuncTrampoline {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("HostFuncTrampoline").finish()
    }
}
