use {
    crate::{
        decode::DecodeError, func::FuncError, global::GlobalError, linker::InstantiateError,
        mem::MemError, table::TableError, trap::Trap,
    },
    std::{error, fmt},
};

/// An error that can occur when operating on a [`Module`](crate::Module) or [`Func`](crate::Func).
#[derive(Debug)]
pub enum Error {
    Decode(DecodeError),
    Instantiate(InstantiateError),
    Func(FuncError),
    Table(TableError),
    Memory(MemError),
    Global(GlobalError),
    Trap(Trap),
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Error::Decode(error) => Some(error),
            Error::Instantiate(error) => Some(error),
            Error::Func(error) => Some(error),
            Error::Table(error) => Some(error),
            Error::Memory(error) => Some(error),
            Error::Global(error) => Some(error),
            Error::Trap(error) => Some(error),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Decode(_) => write!(f, "decode error"),
            Error::Instantiate(_) => write!(f, "instantiate error"),
            Error::Func(_) => write!(f, "function error"),
            Error::Table(_) => write!(f, "table error"),
            Error::Memory(_) => write!(f, "memory error"),
            Error::Global(_) => write!(f, "global error"),
            Error::Trap(_) => write!(f, "trap"),
        }
    }
}

impl From<DecodeError> for Error {
    fn from(error: DecodeError) -> Self {
        Error::Decode(error)
    }
}

impl From<InstantiateError> for Error {
    fn from(error: InstantiateError) -> Self {
        Error::Instantiate(error)
    }
}

impl From<FuncError> for Error {
    fn from(error: FuncError) -> Self {
        Error::Func(error)
    }
}

impl From<TableError> for Error {
    fn from(error: TableError) -> Self {
        Error::Table(error)
    }
}

impl From<MemError> for Error {
    fn from(error: MemError) -> Self {
        Error::Memory(error)
    }
}

impl From<GlobalError> for Error {
    fn from(error: GlobalError) -> Self {
        Error::Global(error)
    }
}

impl From<Trap> for Error {
    fn from(trap: Trap) -> Self {
        Error::Trap(trap)
    }
}
