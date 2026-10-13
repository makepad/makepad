//! A pointer-sized error representation, ported from rustc's io/error/repr_bitpacked.rs.
//! The low two bits tag a static message (00), boxed custom error (01), OS code (10),
//! or simple kind (11). Integers occupy the high 32 bits; NonNull preserves the null niche.

use alloc::boxed::Box;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};

use super::{Custom, ErrorData, ErrorKind, RawOsError, SimpleMessage};

const TAG_MASK: usize = 0b11;
const TAG_SIMPLE_MESSAGE: usize = 0b00;
const TAG_CUSTOM: usize = 0b01;
const TAG_OS: usize = 0b10;
const TAG_SIMPLE: usize = 0b11;

#[repr(transparent)]
pub(super) struct Repr(NonNull<()>, PhantomData<ErrorData<Box<Custom>>>);

// The stored values are Send + Sync; the marker retains their ownership and unwind traits.
unsafe impl Send for Repr {}
unsafe impl Sync for Repr {}

impl Repr {
    pub(super) fn new_custom(custom: Box<Custom>) -> Self {
        let ptr = Box::into_raw(custom).cast::<u8>();
        // Custom's alignment leaves both tag bits clear. Pointer arithmetic preserves provenance.
        let tagged = ptr.wrapping_add(TAG_CUSTOM).cast::<()>();
        // SAFETY: Adding the nonzero tag to an aligned pointer cannot produce null.
        Self(unsafe { NonNull::new_unchecked(tagged) }, PhantomData)
    }

    #[inline]
    pub(super) fn new_os(code: RawOsError) -> Self {
        let bits = ((code as usize) << 32) | TAG_OS;
        // SAFETY: The tag is nonzero even when code is zero; this pointer is never dereferenced.
        Self(unsafe { NonNull::new_unchecked(ptr::without_provenance_mut(bits)) }, PhantomData)
    }

    #[inline]
    pub(super) fn new_simple(kind: ErrorKind) -> Self {
        let bits = ((kind as usize) << 32) | TAG_SIMPLE;
        // SAFETY: The tag is nonzero even for the first kind; this pointer is never dereferenced.
        Self(unsafe { NonNull::new_unchecked(ptr::without_provenance_mut(bits)) }, PhantomData)
    }

    #[inline]
    pub(super) const fn new_simple_message(message: &'static SimpleMessage) -> Self {
        // SAFETY: References are nonnull and SimpleMessage's alignment supplies the zero tag.
        Self(unsafe { NonNull::new_unchecked(message as *const _ as *mut ()) }, PhantomData)
    }

    #[inline]
    pub(super) fn data(&self) -> ErrorData<&Custom> {
        // SAFETY: The encoding is valid and the shared borrow bounds the custom reference.
        unsafe { decode_repr(self.0, |ptr| &*ptr) }
    }

    #[inline]
    pub(super) fn data_mut(&mut self) -> ErrorData<&mut Custom> {
        // SAFETY: The encoding is valid and the exclusive borrow bounds the custom reference.
        unsafe { decode_repr(self.0, |ptr| &mut *ptr) }
    }

    #[inline]
    pub(super) fn into_data(self) -> ErrorData<Box<Custom>> {
        let this = ManuallyDrop::new(self);
        // SAFETY: Suppressing Repr's destructor transfers its box without a double drop.
        unsafe { decode_repr(this.0, |ptr| Box::from_raw(ptr)) }
    }
}

impl Drop for Repr {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: Repr owns the box and is being dropped exactly once.
        drop(unsafe { decode_repr(self.0, |ptr| Box::from_raw(ptr)) });
    }
}

// SAFETY: ptr must come from a live Repr. The caller controls the custom pointer's
// borrow or ownership and must not create aliases or reconstruct its box twice.
#[inline]
unsafe fn decode_repr<C, F: FnOnce(*mut Custom) -> C>(ptr: NonNull<()>, make_custom: F) -> ErrorData<C> {
    let bits = ptr.as_ptr().addr();
    match bits & TAG_MASK {
        TAG_OS => ErrorData::Os(((bits as i64) >> 32) as RawOsError),
        TAG_SIMPLE => {
            let kind = kind_from_prim((bits >> 32) as u32).unwrap_or_else(|| {
                // SAFETY: Only new_simple creates this tag, using a valid ErrorKind.
                unsafe { core::hint::unreachable_unchecked() }
            });
            ErrorData::Simple(kind)
        }
        TAG_SIMPLE_MESSAGE => {
            // SAFETY: This tag stores an unmodified, aligned static reference.
            ErrorData::SimpleMessage(unsafe { &*ptr.cast::<SimpleMessage>().as_ptr() })
        }
        TAG_CUSTOM => {
            let custom = ptr.as_ptr().cast::<u8>().wrapping_sub(TAG_CUSTOM).cast::<Custom>();
            ErrorData::Custom(make_custom(custom))
        }
        _ => unreachable!(),
    }
}

// Decode without assuming an enum layout, and require the list to remain exhaustive.
#[inline]
fn kind_from_prim(kind: u32) -> Option<ErrorKind> {
    macro_rules! from_prim {
        ($($variant:ident),* $(,)?) => {{
            const _: fn(ErrorKind) = |kind| match kind {
                $(ErrorKind::$variant => ()),*
            };
            match kind {
                $(value if value == ErrorKind::$variant as u32 => Some(ErrorKind::$variant),)*
                _ => None,
            }
        }};
    }
    from_prim!(
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
    )
}
