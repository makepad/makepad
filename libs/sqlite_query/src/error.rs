//! One error type for the whole engine.
//!
//! Every on-disk inconsistency is reported as [`Error::Corrupt`] with a short
//! human-readable reason. The engine never panics on malformed input: all page,
//! cell and record parsing is bounds-checked and turns bad bytes into this
//! error.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Underlying file IO failed.
    Io(std::io::Error),
    /// The file does not start with the SQLite 3 magic string.
    NotADatabase,
    /// The file is a SQLite database but violates the format in some way.
    Corrupt(String),
    /// A valid database that uses a format feature this engine does not
    /// implement (encrypted/reserved-space schemes, unknown WAL versions, ...).
    Unsupported(String),
    /// SQL could not be tokenized, parsed, planned or bound.
    Sql(String),
    /// The statement exceeded its row or step budget.
    Budget(String),
    /// Another connection holds a conflicting lock (SQLITE_BUSY).
    Busy(String),
    /// A NOT NULL, UNIQUE, PRIMARY KEY or CHECK constraint was violated.
    Constraint(String),
}

impl Error {
    pub fn corrupt(msg: impl Into<String>) -> Self {
        Error::Corrupt(msg.into())
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Error::Unsupported(msg.into())
    }
    pub fn sql(msg: impl Into<String>) -> Self {
        Error::Sql(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::NotADatabase => write!(f, "not a SQLite database"),
            Error::Corrupt(m) => write!(f, "database corrupt: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Sql(m) => write!(f, "sql: {m}"),
            Error::Budget(m) => write!(f, "budget exceeded: {m}"),
            Error::Busy(m) => write!(f, "database is busy: {m}"),
            Error::Constraint(m) => write!(f, "constraint failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
