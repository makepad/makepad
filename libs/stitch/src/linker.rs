use {
    crate::{
        error::Error, extern_val::ExternVal, instance::Instance, module::Module, store::Store,
    },
    std::{collections::HashMap, fmt, sync::Arc},
};

/// A linker for defining imports and instantiating [`Module`]s.
#[derive(Clone, Debug)]
pub struct Linker {
    strings: StringInterner,
    defs: HashMap<(InternedString, InternedString), ExternVal>,
}

impl Linker {
    /// Creates a new [`Linker`].
    pub fn new() -> Self {
        Linker {
            strings: StringInterner::new(),
            defs: HashMap::new(),
        }
    }

    pub fn define(&mut self, module: &str, name: &str, val: impl Into<ExternVal>) {
        let module = self.strings.get_or_intern(module);
        let name = self.strings.get_or_intern(name);
        assert!(
            self.defs.insert((module, name), val.into()).is_none(),
            "duplicate definition"
        );
    }

    pub fn instantiate(&self, store: &mut Store, module: &Module) -> Result<Instance, Error> {
        module.instantiate(store, self)
    }

    pub(crate) fn lookup(&self, module: &str, name: &str) -> Option<ExternVal> {
        let module = self.strings.get(module)?;
        let name = self.strings.get(name)?;
        self.defs.get(&(module, name)).copied()
    }
}

/// An error that can occur when instantiating a module.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum InstantiateError {
    DefNotFound,
    ImportKindMismatch,
    FuncTypeMismatch,
    GlobalTypeMismatch,
    TableTypeMismatch,
    MemTypeMismatch,
}

impl fmt::Display for InstantiateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefNotFound => write!(f, "definition not found"),
            Self::ImportKindMismatch => write!(f, "import kind mismatch"),
            Self::FuncTypeMismatch => write!(f, "function type mismatch"),
            Self::GlobalTypeMismatch => write!(f, "global type mismatch"),
            Self::TableTypeMismatch => write!(f, "table type mismatch"),
            Self::MemTypeMismatch => write!(f, "memory type mismatch"),
        }
    }
}

impl std::error::Error for InstantiateError {}

#[derive(Clone, Debug)]
struct StringInterner {
    strings: Vec<Arc<str>>,
    indices: HashMap<Arc<str>, usize>,
}

impl StringInterner {
    fn new() -> Self {
        StringInterner {
            strings: Vec::new(),
            indices: HashMap::new(),
        }
    }

    fn get(&self, string: &str) -> Option<InternedString> {
        let index = self.indices.get(string).copied()?;
        Some(InternedString(index))
    }

    fn get_or_intern(&mut self, string: &str) -> InternedString {
        match self.indices.get(string).copied() {
            Some(index) => InternedString(index),
            None => {
                let index = self.strings.len();
                let string: Arc<str> = string.into();
                self.strings.push(string.clone());
                self.indices.insert(string, index);
                InternedString(index)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct InternedString(usize);
