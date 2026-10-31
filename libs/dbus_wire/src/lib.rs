//! D-Bus wire protocol without dependencies or an event-loop runtime.
//!
//! Build bodies with [`Writer`] or [`Value`]. On a worker, connect with
//! [`Connection::session`], poll its fd for reads (and writes while
//! `wants_write()`), then call `read_ready()` and `flush()`. Drain messages
//! with `pop_message()` or match replies with `take_reply(serial)`.
//! [`ObjectServer::dispatch`] handles exported calls and returns a message
//! to enqueue with `Connection::send`. Setup and `call` are blocking; none
//! of the readiness methods wait. Unix fd passing is not negotiated.
//!
//! Generated methods build a [`Body`] with a declared signature and decode it
//! through a borrowing reader. Body construction validates the typed writes:
//!
//! ```
//! use makepad_dbus_wire::{Body, Writer};
//! let mut w = Writer::new();
//! w.str("example")?;
//! let array = w.begin_array("u")?;
//! w.u32(42);
//! w.end_array(array)?;
//! let body = Body::from_writer("sau", w)?;
//! let mut r = body.reader();
//! assert_eq!(r.str()?, "example");
//! let mut array = r.array("u")?;
//! assert_eq!(array.u32()?, 42);
//! array.finish()?;
//! r.finish()?;
//! # Ok::<(), makepad_dbus_wire::Error>(())
//! ```
//!
//! The codec follows the [D-Bus wire specification](https://dbus.freedesktop.org/doc/dbus-specification.html).

mod codec;
#[cfg(unix)]
mod connection;
mod message;
mod server;
mod signature;

pub use codec::{ArrayToken, Endian, Reader, Value, Writer};
#[cfg(unix)]
pub use connection::{parse_address, parse_auth_line, Address, AuthReply, Connection};
pub use message::{Body, Header, Message, MessageType, NO_REPLY_EXPECTED};
pub use server::{MethodCall, MethodError, MethodResult, ObjectHandler, ObjectServer};
pub use signature::{validate_object_path, Signature};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Io(std::io::Error),
    Timeout,
    Remote { name: String, detail: String },
}

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Io(error) => error.fmt(f),
            Self::Timeout => f.write_str("D-Bus operation timed out"),
            Self::Remote { name, detail } => write!(f, "{name}: {detail}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

// Limits defined by the D-Bus protocol, not application resource budgets.
pub(crate) const MAX_ARRAY: usize = 1 << 26;
pub(crate) const MAX_MESSAGE: usize = 1 << 27;
pub(crate) const MAX_DEPTH: usize = 64;
