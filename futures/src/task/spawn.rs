use std::{error, fmt, future::Future};

pub trait Spawn {
    fn spawn(&self, future: impl Future<Output = ()> + 'static) -> Result<(), SpawnError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpawnError {
    _priv: (),
}

impl SpawnError {
    pub fn shutdown() -> Self {
        Self {
            _priv: ()
        }
    }
}

impl error::Error for SpawnError {}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "executor is shutdown")
    }
}