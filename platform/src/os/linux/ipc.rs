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

pub fn channel<TX, RX>() -> io::Result<(Channel<TX, RX>, Channel<RX, TX>)> {
    let (a, b) = UnixStream::pair()?;
    Ok((
        Channel {
            stream: a,
            _marker: PhantomData,
        },
        Channel {
            stream: b,
            _marker: PhantomData,
        },
    ))
}

impl<TX, RX> Clone for Channel<TX, RX> {
    fn clone(&self) -> Self {
        Self {
            stream: self.stream.try_clone().unwrap(),
            _marker: PhantomData,
        }
    }
}

// FIXME(eddyb) the `cfg(use_unstable_unix_socket_ancillary_data_2021)`
// implementation works on (and has been tested for) nightlies ranging
// from early 2021 to late 2023 (roughly matching 1.51 - 1.73 relases),
// but is provided here mostly for pedagogical reasons, as it's quite
// likely stabilization (in 2024 or later) will be blocked a redesign
// of the API, as per https://github.com/rust-lang/rust/issues/76915
// comments (also, note that this cfg has no exposed way of turning
// it on, short of passing it to `rustc` via `RUSTFLAGS=--cfg=...`).
#[cfg(use_unstable_unix_socket_ancillary_data_2021)]
mod sys {
    use super::*;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::{AncillaryData, SocketAncillary};

    pub(super) fn stream_sendmsg<const FD_LEN: usize>(
        stream: &UnixStream,
        bytes: io::IoSlice<'_>,
        fds: &[BorrowedFd<'_>; FD_LEN],
    ) -> io::Result<()> {
        let mut ancillary_buffer = [0; 64];
        let mut ancillary = SocketAncillary::new(&mut ancillary_buffer);
        if !ancillary.add_fds(unsafe { &*(fds as *const [BorrowedFd<'_>] as *const [i32]) }) {
            return Err(io::Error::other(format!(
                "failed to send {FD_LEN} file descriptors: \
                 the resulting cmsg doesn't fit in {} bytes",
                ancillary.capacity()
            )));
        }
        let written_len = stream.send_vectored_with_ancillary(&[bytes], &mut ancillary)?;
        if written_len != bytes.len() {
            return Err(io::Error::other(format!(
                "partial write (only {written_len} out of {})",
                bytes.len()
            )));
        }
        Ok(())
    }

    pub(super) fn stream_recvmsg<const FD_LEN: usize>(
        stream: &UnixStream,
        bytes: io::IoSliceMut<'_>,
    ) -> io::Result<[OwnedFd; FD_LEN]> {
        let mut ancillary_buffer = [0; 64];
        let mut ancillary = SocketAncillary::new(&mut ancillary_buffer);
        let expected_len = bytes.len();
        let read_len = stream.recv_vectored_with_ancillary(&mut [bytes], &mut ancillary)?;
        let partial_read = read_len != expected_len;
        let (anciliary_truncated, anciliary_capacity) =
            (ancillary.truncated(), ancillary.capacity());

        // HACK(eddyb) this is painfully stateful so that it has a chance to
        // `close` *all* unwanted `OwnedFd`s, to avoid keeping *any* alive
        // (even without a malicious sender, any mistake could easily end up
        // leaking hundreds of file descriptors, and with e.g. DMA-BUF they'd
        // easily keep alive buffers totalling more than most GPUs have VRAM).
        let mut errors = vec![];
        let mut accepted_fds = [(); FD_LEN].map(|()| None);
        let mut accepted_fd_count = 0;
        for cmsg in ancillary.messages() {
            match cmsg {
                Err(err) => errors.push(format!("{err:?}")),
                Ok(AncillaryData::ScmRights(raw_fds)) => {
                    let is_first_scm_rights = accepted_fd_count == 0;
                    for raw_fd in raw_fds {
                        if raw_fd == -1 {
                            errors.push("invalid fd (-1) received".into());
                            continue;
                        }
                        // Using `OwnedFd` ensure all unwanted file descriptors
                        // are closed (see larger comment above for why).
                        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
                        if is_first_scm_rights {
                            // NOTE(eddyb) too few/many fds are handled later.
                            let i = accepted_fd_count;
                            accepted_fd_count += 1;
                            if let Some(slot) = accepted_fds.get_mut(i) {
                                *slot = Some(fd);
                            }
                        }
                    }
                    if !is_first_scm_rights {
                        errors.push("received more than one SCM_RIGHTS cmsg".into());
                    }
                }
                Ok(AncillaryData::ScmCredentials(_)) => {
                    errors.push("received unexpected SCM_CREDS-like cmsg".into());
                }
            }
        }
        if accepted_fd_count != FD_LEN {
            errors.push(format!(
                "wrong number of received fds: expected {FD_LEN}, got {accepted_fd_count}"
            ))
        }

        if partial_read {
            return Err(io::Error::other(format!(
                "partial read: only {read_len} out of {expected_len}"
            )));
        }
        if anciliary_truncated {
            return Err(io::Error::other(format!(
                "truncated anciliary buffer: received cmsg doesn't fit in {anciliary_capacity} bytes"
            )));
        }

        if errors.is_empty() {
            Ok(accepted_fds.map(Option::unwrap))
        } else {
            Err(io::Error::other(if errors.len() == 1 {
                errors.pop().unwrap()
            } else {
                format!("errors during receiving:\n  {}", errors.join("\n  "))
            }))
        }
    }
}
#[cfg(not(use_unstable_unix_socket_ancillary_data_2021))]
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
    /// Worker-side AF_UNIX connect with bounded cancellation, including a
    /// full listener backlog. A blocking UnixStream::connect cannot provide
    /// that guarantee merely by checking a deadline between attempts.
    pub fn connect_cancellable(path: &std::path::Path, stop: &std::sync::atomic::AtomicBool,
        timeout: std::time::Duration) -> io::Result<Self> {
        use std::{os::{fd::FromRawFd, unix::ffi::OsStrExt}, sync::atomic::Ordering, time::{Duration, Instant}};
        #[repr(C)]
        struct Address { family: u16, path: [u8; 108] }
        use crate::os::linux::v4l2_sys::{poll, pollfd};
        extern "C" {
            fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
            fn connect(fd: i32, address: *const Address, length: u32) -> i32;
        }
        let bytes = path.as_os_str().as_bytes();
        if bytes.is_empty() || bytes.len() >= 108 || bytes.contains(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid auxiliary socket path"));
        }
        let mut address = Address { family: 1, path: [0; 108] };
        address.path[..bytes.len()].copy_from_slice(bytes);
        let deadline = Instant::now() + timeout;
        let stopped = || {
            if stop.load(Ordering::Acquire) {
                Err(io::Error::new(io::ErrorKind::Interrupted, "auxiliary connect cancelled"))
            } else if Instant::now() >= deadline {
                Err(io::Error::new(io::ErrorKind::TimedOut, "auxiliary connect timed out"))
            } else { Ok(()) }
        };
        loop {
            stopped()?;
            let raw = unsafe { socket(1, 1 | 0x800 | 0x80000, 0) };
            if raw < 0 { return Err(io::Error::last_os_error()); }
            let stream = unsafe { UnixStream::from_raw_fd(raw) };
            let result = unsafe { connect(raw, &address, (2 + bytes.len() + 1) as u32) };
            if result == 0 {
                stream.set_nonblocking(false)?;
                return Ok(Self { stream, _marker: PhantomData });
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(115) { // EINPROGRESS
                loop {
                    stopped()?;
                    let mut fd = pollfd { fd: raw, events: 4, revents: 0 };
                    match unsafe { poll(&mut fd, 1, 5) } {
                        0 => continue,
                        -1 => {
                            let error = io::Error::last_os_error();
                            if error.kind() == io::ErrorKind::Interrupted { continue; }
                            return Err(error);
                        }
                        _ => {
                            if let Some(error) = stream.take_error()? { return Err(error); }
                            stream.peer_addr()?;
                            stream.set_nonblocking(false)?;
                            return Ok(Self { stream, _marker: PhantomData });
                        }
                    }
                }
            }
            if !matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                | io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) { return Err(error); }
            drop(stream);
            std::thread::sleep(Duration::from_millis(5));
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

    /// Enable child process inheritance (see [`InheritableChannel`]),
    /// i.e. remove the `CLOEXEC` flag (via `dup`, not `fcntl(F_{SET,GET}FD)`,
    /// due to the latter's misdesign as read/write instead of `fetch_{and,or}`,
    /// so they invite race conditions and should be deprecated and never used).
    pub fn into_child_process_inheritable(self) -> io::Result<InheritableChannel<TX, RX>> {
        extern "C" {
            fn dup(fd: BorrowedFd<'_>) -> Option<OwnedFd>;
        }
        Ok(InheritableChannel(Self {
            stream: unsafe { dup(self.stream.as_fd()) }
                .ok_or_else(|| io::Error::last_os_error())?
                .into(),
            _marker: PhantomData,
        }))
    }
}

/// A `Channel<TX, RX>` whose internal (UNIX domain socket) file descriptor will
/// persist in all child proceses (except for those which explicitly close it),
/// and which only provides conversions to/from file descriptors, and a way to
/// disable inheritance (i.e. re-enabling `CLOEXEC` semantics on it).
pub struct InheritableChannel<TX, RX>(Channel<TX, RX>);

impl<TX, RX> AsFd for InheritableChannel<TX, RX> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.stream.as_fd()
    }
}

impl<TX, RX> From<OwnedFd> for InheritableChannel<TX, RX> {
    fn from(fd: OwnedFd) -> Self {
        Self(Channel {
            stream: UnixStream::from(fd),
            _marker: PhantomData,
        })
    }
}

impl<TX, RX> InheritableChannel<TX, RX> {
    /// Disable child process inheritance, i.e. re-add the `CLOEXEC` flag
    /// (via `try_clone_to_owned` which uses `fcntl(F_DUPFD_CLOEXEC)`).
    pub fn into_uninheritable(self) -> io::Result<Channel<TX, RX>> {
        let Self(mut channel) = self;
        channel.stream = channel.stream.as_fd().try_clone_to_owned()?.into();
        Ok(channel)
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
