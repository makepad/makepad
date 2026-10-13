//! io::Error / ErrorKind with real std's Display and Debug output.

use alloc::boxed::Box;
use alloc::string::String;
use core::fmt;

use core::error::Error as StdError;
use crate::sys;

#[cfg(target_pointer_width = "64")]
mod repr_bitpacked;
#[cfg(target_pointer_width = "64")]
use self::repr_bitpacked::Repr;
#[cfg(not(target_pointer_width = "64"))]
mod repr_unpacked;
#[cfg(not(target_pointer_width = "64"))]
use self::repr_unpacked::Repr;

pub type Result<T> = core::result::Result<T, Error>;
pub type RawOsError = i32;

pub struct Error {
    repr: Repr,
}

enum ErrorData<C> {
    Os(RawOsError),
    Simple(ErrorKind),
    SimpleMessage(&'static SimpleMessage),
    Custom(C),
}

// Both pointer variants reserve their low two bits for the packed tag.
#[repr(align(4))]
pub(crate) struct SimpleMessage {
    pub(crate) kind: ErrorKind,
    pub(crate) message: &'static str,
}

/// Creates a static-message error without allocating, including in const contexts.
macro_rules! const_error {
    ($kind:expr, $message:expr $(,)?) => {
        $crate::io::Error::from_static_message({
            const MESSAGE: $crate::io::SimpleMessage = $crate::io::SimpleMessage {
                kind: $kind,
                message: $message,
            };
            &MESSAGE
        })
    };
}
pub(crate) use const_error;

#[repr(align(4))]
struct Custom {
    kind: ErrorKind,
    error: Box<dyn StdError + Send + Sync>,
}

/// `Error::new(kind, "text")` stores the text in this (Debug prints it quoted, like real std).
struct StringError(String);

impl fmt::Debug for StringError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}
impl fmt::Display for StringError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
impl StdError for StringError {}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum ErrorKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    HostUnreachable,
    NetworkUnreachable,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    NetworkDown,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    ReadOnlyFilesystem,
    FilesystemLoop,
    StaleNetworkFileHandle,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    StorageFull,
    NotSeekable,
    QuotaExceeded,
    FileTooLarge,
    ResourceBusy,
    ExecutableFileBusy,
    Deadlock,
    CrossesDevices,
    TooManyLinks,
    InvalidFilename,
    ArgumentListTooLong,
    Interrupted,
    Unsupported,
    UnexpectedEof,
    OutOfMemory,
    InProgress,
    TooManyOpenFiles,
    Other,
    Uncategorized,
}

impl ErrorKind {
    fn as_str(&self) -> &'static str {
        match *self {
            ErrorKind::AddrInUse => "address in use",
            ErrorKind::AddrNotAvailable => "address not available",
            ErrorKind::AlreadyExists => "entity already exists",
            ErrorKind::ArgumentListTooLong => "argument list too long",
            ErrorKind::BrokenPipe => "broken pipe",
            ErrorKind::ConnectionAborted => "connection aborted",
            ErrorKind::ConnectionRefused => "connection refused",
            ErrorKind::ConnectionReset => "connection reset",
            ErrorKind::CrossesDevices => "cross-device link or rename",
            ErrorKind::Deadlock => "deadlock",
            ErrorKind::DirectoryNotEmpty => "directory not empty",
            ErrorKind::ExecutableFileBusy => "executable file busy",
            ErrorKind::FileTooLarge => "file too large",
            ErrorKind::FilesystemLoop => "filesystem loop or indirection limit (e.g. symlink loop)",
            ErrorKind::HostUnreachable => "host unreachable",
            ErrorKind::InProgress => "in progress",
            ErrorKind::Interrupted => "operation interrupted",
            ErrorKind::InvalidData => "invalid data",
            ErrorKind::InvalidFilename => "invalid filename",
            ErrorKind::InvalidInput => "invalid input parameter",
            ErrorKind::IsADirectory => "is a directory",
            ErrorKind::NetworkDown => "network down",
            ErrorKind::NetworkUnreachable => "network unreachable",
            ErrorKind::NotADirectory => "not a directory",
            ErrorKind::NotConnected => "not connected",
            ErrorKind::NotFound => "entity not found",
            ErrorKind::NotSeekable => "seek on unseekable file",
            ErrorKind::Other => "other error",
            ErrorKind::OutOfMemory => "out of memory",
            ErrorKind::PermissionDenied => "permission denied",
            ErrorKind::QuotaExceeded => "quota exceeded",
            ErrorKind::ReadOnlyFilesystem => "read-only filesystem or storage medium",
            ErrorKind::ResourceBusy => "resource busy",
            ErrorKind::StaleNetworkFileHandle => "stale network file handle",
            ErrorKind::StorageFull => "no storage space",
            ErrorKind::TimedOut => "timed out",
            ErrorKind::TooManyLinks => "too many links",
            ErrorKind::TooManyOpenFiles => "too many open files",
            ErrorKind::Uncategorized => "uncategorized error",
            ErrorKind::UnexpectedEof => "unexpected end of file",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::WouldBlock => "operation would block",
            ErrorKind::WriteZero => "write zero",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// GetLastError code -> ErrorKind (real std's sys::io::error::windows::decode_error_kind).
#[cfg(windows)]
pub fn decode_error_kind(code: i32) -> ErrorKind {
    crate::sys::os::decode_error_kind(code)
}

/// errno -> ErrorKind (real std's sys::io::error::unix::decode_error_kind).
#[cfg(unix)]
pub fn decode_error_kind(errno: i32) -> ErrorKind {
    use crate::sys::os as e;
    if errno == e::E2BIG {
        ErrorKind::ArgumentListTooLong
    } else if errno == e::EADDRINUSE {
        ErrorKind::AddrInUse
    } else if errno == e::EADDRNOTAVAIL {
        ErrorKind::AddrNotAvailable
    } else if errno == e::EBUSY {
        ErrorKind::ResourceBusy
    } else if errno == e::ECONNABORTED {
        ErrorKind::ConnectionAborted
    } else if errno == e::ECONNREFUSED {
        ErrorKind::ConnectionRefused
    } else if errno == e::ECONNRESET {
        ErrorKind::ConnectionReset
    } else if errno == e::EDEADLK {
        ErrorKind::Deadlock
    } else if errno == e::EDQUOT {
        ErrorKind::QuotaExceeded
    } else if errno == e::EEXIST {
        ErrorKind::AlreadyExists
    } else if errno == e::EFBIG {
        ErrorKind::FileTooLarge
    } else if errno == e::EHOSTUNREACH {
        ErrorKind::HostUnreachable
    } else if errno == e::EINTR {
        ErrorKind::Interrupted
    } else if errno == e::EINVAL {
        ErrorKind::InvalidInput
    } else if errno == e::EISDIR {
        ErrorKind::IsADirectory
    } else if errno == e::ELOOP {
        ErrorKind::FilesystemLoop
    } else if errno == e::ENOENT {
        ErrorKind::NotFound
    } else if errno == e::ENOMEM {
        ErrorKind::OutOfMemory
    } else if errno == e::ENOSPC {
        ErrorKind::StorageFull
    } else if errno == e::ENOSYS {
        ErrorKind::Unsupported
    } else if errno == e::EMLINK {
        ErrorKind::TooManyLinks
    } else if errno == e::ENAMETOOLONG {
        ErrorKind::InvalidFilename
    } else if errno == e::ENETDOWN {
        ErrorKind::NetworkDown
    } else if errno == e::ENETUNREACH {
        ErrorKind::NetworkUnreachable
    } else if errno == e::ENOTCONN {
        ErrorKind::NotConnected
    } else if errno == e::ENOTDIR {
        ErrorKind::NotADirectory
    } else if errno == e::ENOTEMPTY {
        ErrorKind::DirectoryNotEmpty
    } else if errno == e::EPIPE {
        ErrorKind::BrokenPipe
    } else if errno == e::EROFS {
        ErrorKind::ReadOnlyFilesystem
    } else if errno == e::ESPIPE {
        ErrorKind::NotSeekable
    } else if errno == e::ESTALE {
        ErrorKind::StaleNetworkFileHandle
    } else if errno == e::ETIMEDOUT {
        ErrorKind::TimedOut
    } else if errno == e::ETXTBSY {
        ErrorKind::ExecutableFileBusy
    } else if errno == e::EXDEV {
        ErrorKind::CrossesDevices
    } else if errno == e::EINPROGRESS {
        ErrorKind::InProgress
    } else if errno == e::EMFILE || errno == e::ENFILE {
        ErrorKind::TooManyOpenFiles
    } else if errno == e::EOPNOTSUPP {
        ErrorKind::Unsupported
    } else if errno == e::EACCES || errno == e::EPERM {
        ErrorKind::PermissionDenied
    } else if errno == e::EAGAIN || errno == e::EWOULDBLOCK {
        ErrorKind::WouldBlock
    } else {
        ErrorKind::Uncategorized
    }
}

impl Error {
    pub const INVALID_UTF8: Error = const_error!(ErrorKind::InvalidData, "stream did not contain valid UTF-8");
    pub const READ_EXACT_EOF: Error = const_error!(ErrorKind::UnexpectedEof, "failed to fill whole buffer");
    pub const WRITE_ALL_EOF: Error = const_error!(ErrorKind::WriteZero, "failed to write whole buffer");
    pub const ZERO_TIMEOUT: Error = const_error!(ErrorKind::InvalidInput, "cannot set a 0 duration timeout");
    pub const NO_ADDRESSES: Error = const_error!(ErrorKind::InvalidInput, "could not resolve to any addresses");

    /// Stores the aligned descriptor directly so static errors remain const and allocation-free.
    pub(crate) const fn from_static_message(message: &'static SimpleMessage) -> Error {
        Error { repr: Repr::new_simple_message(message) }
    }

    pub fn new<E: Into<Box<dyn StdError + Send + Sync>>>(kind: ErrorKind, error: E) -> Error {
        Error::from_box(kind, error.into())
    }

    /// Error::new for text (what `Error::new(kind, "..")` with a &str/String builds).
    pub(crate) fn from_string(kind: ErrorKind, s: String) -> Error {
        Error::from_box(kind, Box::new(StringError(s)))
    }

    fn from_box(kind: ErrorKind, error: Box<dyn StdError + Send + Sync>) -> Error {
        Error { repr: Repr::new_custom(Box::new(Custom { kind, error })) }
    }

    pub fn other<E: Into<Box<dyn StdError + Send + Sync>>>(error: E) -> Error {
        Error::from_box(ErrorKind::Other, error.into())
    }

    pub fn last_os_error() -> Error {
        Error::from_raw_os_error(sys::errno())
    }

    pub fn from_raw_os_error(code: RawOsError) -> Error {
        Error { repr: Repr::new_os(code) }
    }

    pub fn raw_os_error(&self) -> Option<RawOsError> {
        match self.repr.data() {
            ErrorData::Os(c) => Some(c),
            _ => None,
        }
    }

    pub fn get_ref(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        match self.repr.data() {
            ErrorData::Custom(c) => Some(&*c.error),
            _ => None,
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut (dyn StdError + Send + Sync + 'static)> {
        match self.repr.data_mut() {
            ErrorData::Custom(c) => Some(&mut *c.error),
            _ => None,
        }
    }

    pub fn into_inner(self) -> Option<Box<dyn StdError + Send + Sync>> {
        match self.repr.into_data() {
            ErrorData::Custom(c) => Some(c.error),
            _ => None,
        }
    }

    /// Extracts a custom error of type `E`, returning this error unchanged on a mismatch.
    pub fn downcast<E: StdError + Send + Sync + 'static>(self) -> core::result::Result<E, Self> {
        match self.repr.data() {
            ErrorData::Custom(c) if c.error.is::<E>() => {},
            _ => return Err(self),
        }
        match self.repr.into_data() {
            ErrorData::Custom(c) => {
                let ptr = Box::into_raw(c.error);
                // SAFETY: The type was checked above and into_data transferred the only owner.
                Ok(unsafe { *Box::from_raw(ptr as *mut E) })
            }
            _ => unreachable!(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        match self.repr.data() {
            ErrorData::Os(code) => decode_error_kind(code),
            ErrorData::Simple(kind) => kind,
            ErrorData::SimpleMessage(message) => message.kind,
            ErrorData::Custom(c) => c.kind,
        }
    }

    pub(crate) fn is_interrupted(&self) -> bool {
        match self.repr.data() {
            ErrorData::Os(code) => code == sys::os::EINTR,
            ErrorData::Simple(kind) => kind == ErrorKind::Interrupted,
            ErrorData::SimpleMessage(message) => message.kind == ErrorKind::Interrupted,
            ErrorData::Custom(c) => c.kind == ErrorKind::Interrupted,
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error { repr: Repr::new_simple(kind) }
    }
}

impl From<alloc::ffi::NulError> for Error {
    fn from(_: alloc::ffi::NulError) -> Error {
        const_error!(ErrorKind::InvalidInput, "data provided contains a nul byte")
    }
}

/// `"<strerror text>"` for the Debug output of an OS error.
struct OsMessage(i32);

impl fmt::Debug for OsMessage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\"{}\"", sys::error_string(self.0))
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.repr.data() {
            ErrorData::Os(code) => f
                .debug_struct("Os")
                .field("code", &code)
                .field("kind", &decode_error_kind(code))
                .field("message", &OsMessage(code))
                .finish(),
            ErrorData::Custom(c) => f.debug_struct("Custom").field("kind", &c.kind).field("error", &c.error).finish(),
            ErrorData::Simple(kind) => f.debug_tuple("Kind").field(&kind).finish(),
            ErrorData::SimpleMessage(message) => f.debug_struct("Error").field("kind", &message.kind).field("message", &message.message).finish(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.repr.data() {
            ErrorData::Os(code) => write!(f, "{} (os error {})", sys::error_string(code), code),
            ErrorData::Custom(c) => fmt::Display::fmt(&*c.error, f),
            ErrorData::Simple(kind) => f.write_str(kind.as_str()),
            ErrorData::SimpleMessage(message) => fmt::Display::fmt(message.message, f),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self.repr.data() {
            ErrorData::Custom(c) => c.error.source(),
            _ => None,
        }
    }
}
