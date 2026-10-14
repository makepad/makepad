//! IPC ("inter-process communication") abstractions used on Linux.
//!
//! **NOTE**: the actual implementations may be portable to other OSes,
//! e.g. "UNIX domain sockets" are definitely not Linux-only, but there
//! may be other reasons to only *need* them on Linux such as macOS

use std::{
    io,
    marker::PhantomData,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
};

/// One endpoint of a bi-directional inter-process communication channel,
/// capable of sending/receiving both raw bytes and UNIX file descriptors,
/// encoded/decoded from/to the `TX`/`RX` types, with an ordering guarantee
/// (messages will be received in the same order that they were sent).
//
// FIXME(eddyb) should this be moved to a `mod channel` and renamed to e.g.
// `SenderReceiver`? (and mimicking `std::sync::mpsc` for `Sender`/`Receiver`)
pub struct Channel<TX, RX> {
    stream: UnixStream,
    _marker: PhantomData<(fn(TX) -> RX, fn(RX) -> TX)>,
}

mod sys {
    #![allow(non_camel_case_types)]

    // HACK(eddyb) `io::Error::other` stabilization is too recent.
    fn io_error_other(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
        io::Error::new(io::ErrorKind::Other, error)
    }

    use super::*;
    use std::{
        ffi::{c_int, c_void},
        ptr,
    };

    type socklen_t = u32;

    #[repr(C)]
    struct msghdr<IOV> {
        msg_name: *mut c_void,
        msg_namelen: socklen_t,
        msg_iov: *mut IOV,
        msg_iovlen: usize,
        msg_control: *mut c_void,
        msg_controllen: usize,
        msg_flags: c_int,
    }

    const SOL_SOCKET: c_int = 1;
    const SCM_RIGHTS: c_int = 1;

    #[repr(C)]
    struct cmsghdr {
        cmsg_len: usize,
        cmsg_level: c_int,
        cmsg_type: c_int,
    }
    const _: () = assert!(std::mem::size_of::<cmsghdr>() % std::mem::size_of::<usize>() == 0);

    extern "C" {
        fn sendmsg(
            sockfd: BorrowedFd<'_>,
            msg: *const msghdr<io::IoSlice<'_>>,
            flags: c_int,
        ) -> isize;
        fn recvmsg(
            sockfd: BorrowedFd<'_>,
            msg: *mut msghdr<io::IoSliceMut<'_>>,
            flags: c_int,
        ) -> isize;
    }

    #[repr(C)]
    struct CMsgBuf<FD, const FD_LEN: usize> {
        header: cmsghdr,
        fds: [FD; FD_LEN],
    }

    pub(super) fn stream_sendmsg<const FD_LEN: usize>(
        stream: &UnixStream,
        bytes: io::IoSlice<'_>,
        fds: &[BorrowedFd<'_>; FD_LEN],
    ) -> io::Result<()> {
        use std::io::Write;
        const MSG_NOSIGNAL: c_int = 0x4000;
        // Send descriptors exactly once with one byte. Remaining bytes can
        // fragment without losing their association or duplicating handles.
        let prefix = [bytes.first().copied().unwrap_or(0)];
        let mut iov = io::IoSlice::new(&prefix);
        let mut control = CMsgBuf {
            header: cmsghdr { cmsg_len: std::mem::size_of::<cmsghdr>() + FD_LEN * 4,
                cmsg_level: SOL_SOCKET, cmsg_type: SCM_RIGHTS },
            fds: *fds,
        };
        let message = msghdr { msg_name: ptr::null_mut(), msg_namelen: 0,
            msg_iov: &mut iov, msg_iovlen: 1, msg_control: &mut control as *mut _ as *mut _,
            msg_controllen: std::mem::size_of_val(&control), msg_flags: 0 };
        let result = (|| {
            loop {
                match unsafe { sendmsg(stream.as_fd(), &message, MSG_NOSIGNAL) } {
                    1 => break,
                    -1 => {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
                    }
                    _ => return Err(io::Error::new(io::ErrorKind::WriteZero, "auxiliary prefix write failed")),
                }
            }
            (&*stream).write_all(bytes.get(1..).unwrap_or_default())
        })();
        if result.is_err() { let _ = stream.shutdown(std::net::Shutdown::Both); }
        result
    }

    pub(super) fn stream_recvmsg<const FD_LEN: usize>(
        stream: &UnixStream,
        mut bytes: io::IoSliceMut<'_>,
    ) -> io::Result<[OwnedFd; FD_LEN]> {
        use std::{io::Read, os::fd::FromRawFd};
        const MSG_CMSG_CLOEXEC: c_int = 0x40000000;
        const MSG_CTRUNC: c_int = 8;
        const MSG_TRUNC: c_int = 0x20;
        // Raw integers until validated; constructing an Option<OwnedFd>
        // directly over unvalidated ancillary bytes could close a wrong FD.
        let mut control = [0usize; 64];
        let mut prefix = [0u8];
        let capacity = std::mem::size_of_val(&control);
        let result = (|| {
            let mut iov = io::IoSliceMut::new(&mut prefix);
            let mut message = msghdr { msg_name: ptr::null_mut(), msg_namelen: 0,
                msg_iov: &mut iov, msg_iovlen: 1, msg_control: control.as_mut_ptr().cast(),
                msg_controllen: capacity, msg_flags: 0 };
            loop {
                message.msg_controllen = capacity;
                message.msg_flags = 0;
                match unsafe { recvmsg(stream.as_fd(), &mut message, MSG_CMSG_CLOEXEC) } {
                    1 => break,
                    -1 => {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
                    }
                    _ => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "auxiliary descriptor channel closed")),
                }
            }
            let mut owned: Vec<OwnedFd> = Vec::new();
            let mut malformed = message.msg_flags & (MSG_CTRUNC | MSG_TRUNC) != 0;
            let header_size = std::mem::size_of::<cmsghdr>();
            let length = message.msg_controllen.min(capacity);
            malformed |= message.msg_controllen > capacity;
            let mut offset = 0usize;
            while offset + header_size <= length {
                let header = unsafe { &*control.as_ptr().cast::<u8>().add(offset).cast::<cmsghdr>() };
                if header.cmsg_len < header_size || header.cmsg_len > length - offset {
                    malformed = true;
                    break;
                }
                if header.cmsg_level == SOL_SOCKET && header.cmsg_type == SCM_RIGHTS {
                    let data_len = header.cmsg_len - header_size;
                    malformed |= data_len % 4 != 0;
                    for index in 0..data_len / 4 {
                        let raw = unsafe { control.as_ptr().cast::<u8>().add(offset + header_size + index * 4).cast::<c_int>().read() };
                        if raw < 0 { malformed = true; }
                        else { owned.push(unsafe { OwnedFd::from_raw_fd(raw) }); }
                    }
                } else { malformed = true; }
                offset += header.cmsg_len.next_multiple_of(std::mem::size_of::<usize>());
            }
            if malformed || owned.len() != FD_LEN {
                return Err(io_error_other(format!("invalid auxiliary descriptors: received {}, expected {FD_LEN}", owned.len())));
            }
            // OwnedFd closes every accepted descriptor if the byte tail is
            // truncated. Never retry a partially consumed transaction.
            if let Some(first) = bytes.first_mut() { *first = prefix[0]; }
            (&*stream).read_exact(bytes.get_mut(1..).unwrap_or_default())?;
            owned.try_into().map_err(|_| io_error_other("auxiliary FD count changed"))
        })();
        if result.is_err() { let _ = stream.shutdown(std::net::Shutdown::Both); }
        result
    }

}

impl<TX, RX> Channel<TX, RX> {
    /// One connect attempt. None while nothing listens at `path` yet: the host
    /// creates its listener when it gives the child a window, which for a warm
    /// (dormant) instance is only its adoption, any time after startup.
    pub fn try_connect(path: &std::path::Path) -> io::Result<Option<Self>> {
        match UnixStream::connect(path) {
            Ok(stream) => Ok(Some(Self { stream, _marker: PhantomData })),
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound
                | io::ErrorKind::ConnectionRefused | io::ErrorKind::Interrupted) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { stream: self.stream.try_clone()?, _marker: PhantomData })
    }
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
    /// Interrupt a worker-owned transaction during shutdown. A cloned handle
    /// is only for cancellation: exactly one reader and one writer may frame
    /// transactions on a stream, including across cloned endpoints.
    pub fn shutdown(&self) -> io::Result<()> {
        self.stream.shutdown(std::net::Shutdown::Both)
    }
    pub fn send<const TX_BYTE_LEN: usize, const TX_FD_LEN: usize>(&self, msg: TX) -> io::Result<()>
    where
        TX: FixedSizeEncoding<TX_BYTE_LEN, TX_FD_LEN>,
    {
        assert_ne!(
            TX_FD_LEN,
            0,
            "Channel<{}, _> unsupported (lacks file descriptors)",
            std::any::type_name::<TX>()
        );

        let (bytes, fds) = msg.encode();
        sys::stream_sendmsg(&self.stream, io::IoSlice::new(&bytes), &fds)
    }

    pub fn recv<const RX_BYTE_LEN: usize, const RX_FD_LEN: usize>(&self) -> io::Result<RX>
    where
        RX: FixedSizeEncoding<RX_BYTE_LEN, RX_FD_LEN>,
    {
        assert_ne!(
            RX_FD_LEN,
            0,
            "Channel<_, {}> unsupported (lacks file descriptors)",
            std::any::type_name::<TX>()
        );

        // FIXME(eddyb) this should use `io::BorrowedBuf` when that's stabilized.
        let mut bytes = [0; RX_BYTE_LEN];
        let fds = sys::stream_recvmsg(&self.stream, io::IoSliceMut::new(&mut bytes))?;
        Ok(RX::decode(bytes, fds))
    }

}

impl<TX, RX> From<UnixStream> for Channel<TX, RX> {
    fn from(stream: UnixStream) -> Self {
        Self { stream, _marker: PhantomData }
    }
}

/// Type with no values to make it impossible to send on a channel endpoint,
/// or receive on its opposite counterpart, if that direction is unused.
pub enum Never {}

/// Fixed-size payloads. The stable transport associates descriptors with
/// exactly one prefix byte, then completes the remaining payload across any
/// stream fragmentation. A descriptor-only payload uses a zero prefix byte.
//
// HACK(eddyb) using const generics instead of associated consts
// only to be able to use the compile-time constants in array types.
pub trait FixedSizeEncoding<const BYTE_LEN: usize, const FD_LEN: usize> {
    // HACK(eddyb) avoids repeating the value inside an `impl`.
    const BYTE_LEN: usize = BYTE_LEN;
    const FD_LEN: usize = FD_LEN;

    fn encode(&self) -> ([u8; BYTE_LEN], [BorrowedFd<'_>; FD_LEN]);
    fn decode(bytes: [u8; BYTE_LEN], fds: [OwnedFd; FD_LEN]) -> Self;
}

// HACK(eddyb) simple `(OnlyBytes, OnlyFds)` to make it easier for const generics.
impl<
        const BYTE_LEN: usize,
        const FD_LEN: usize,
        A: FixedSizeEncoding<BYTE_LEN, 0>,
        B: FixedSizeEncoding<0, FD_LEN>,
    > FixedSizeEncoding<BYTE_LEN, FD_LEN> for (A, B)
{
    fn encode(&self) -> ([u8; BYTE_LEN], [BorrowedFd<'_>; FD_LEN]) {
        let ((bytes, []), ([], fds)) = (self.0.encode(), self.1.encode());
        (bytes, fds)
    }
    fn decode(bytes: [u8; BYTE_LEN], fds: [OwnedFd; FD_LEN]) -> Self {
        (A::decode(bytes, []), B::decode([], fds))
    }
}

macro_rules! fixed_size_le_prim_impls {
    ($($ty:ident)*) => {
        $(impl FixedSizeEncoding<{(Self::BITS / 8) as usize}, 0> for $ty {
            fn encode(&self) -> ([u8; Self::BYTE_LEN], [BorrowedFd<'_>; 0]) {
                (self.to_le_bytes(), [])
            }
            fn decode(bytes: [u8; Self::BYTE_LEN], []: [OwnedFd; 0]) -> Self {
                Self::from_le_bytes(bytes)
            }
        })*
    }
}
fixed_size_le_prim_impls!(u16 u32 u64 u128);

impl FixedSizeEncoding<0, 1> for OwnedFd {
    fn encode(&self) -> ([u8; 0], [BorrowedFd<'_>; 1]) {
        ([], [self.as_fd()])
    }
    fn decode([]: [u8; 0], [fd]: [OwnedFd; 1]) -> Self {
        fd
    }
}
