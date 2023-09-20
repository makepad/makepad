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
        mut bytes: io::IoSlice<'_>,
        fds: &[BorrowedFd<'_>; FD_LEN],
    ) -> io::Result<()> {
        let mut cmsg_buf = CMsgBuf {
            header: cmsghdr {
                cmsg_len: std::mem::size_of::<cmsghdr>() + FD_LEN * 4,
                cmsg_level: SOL_SOCKET,
                cmsg_type: SCM_RIGHTS,
            },
            fds: *fds,
        };

        let written_len = unsafe {
            sendmsg(
                stream.as_fd(),
                &msghdr {
                    msg_name: ptr::null_mut(),
                    msg_namelen: 0,
                    msg_iov: &mut bytes,
                    msg_iovlen: 1,
                    msg_control: &mut cmsg_buf as *mut _ as *mut _,
                    msg_controllen: std::mem::size_of_val(&cmsg_buf),
                    msg_flags: 0,
                },
                0,
            )
        };
        if written_len == -1 {
            return Err(io::Error::last_os_error());
        }
        if written_len as usize != bytes.len() {
            return Err(io::Error::other(format!(
                "partial write (only {written_len} out of {})",
                bytes.len()
            )));
        }
        Ok(())
    }

    pub(super) fn stream_recvmsg<const FD_LEN: usize>(
        stream: &UnixStream,
        mut bytes: io::IoSliceMut<'_>,
    ) -> io::Result<[OwnedFd; FD_LEN]> {
        let expected_len = bytes.len();

        let mut cmsg_buf = std::mem::MaybeUninit::<CMsgBuf<Option<OwnedFd>, FD_LEN>>::zeroed();
        let expected_cmsg_len = std::mem::size_of::<cmsghdr>() + FD_LEN * 4;
        let expected_msg_controllen = std::mem::size_of_val(&cmsg_buf);

        let mut msg = msghdr {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut bytes,
            msg_iovlen: 1,
            msg_control: &mut cmsg_buf as *mut _ as *mut _,
            msg_controllen: expected_msg_controllen,
            msg_flags: 0,
        };

        let read_len = unsafe { recvmsg(stream.as_fd(), &mut msg, 0) };
        if read_len == -1 {
            return Err(io::Error::last_os_error());
        }

        // FIXME(eddyb) all of these errors should close fds to prevent fd DOS,
        // but for now this is not particularly a notable surface of attack.

        if read_len as usize != expected_len {
            return Err(io::Error::other(format!(
                "partial read: only {read_len} out of {expected_len}"
            )));
        }

        if msg.msg_controllen != expected_msg_controllen {
            return Err(io::Error::other(format!(
                "recvmsg msg_controllen mismatch: got {}, expected {expected_msg_controllen}",
                msg.msg_controllen,
            )));
        }

        let cmsg = unsafe { cmsg_buf.assume_init() };
        if cmsg.header.cmsg_len != expected_cmsg_len {
            return Err(io::Error::other(format!(
                "recvmsg cmsg_len mismatch: got {}, expected {expected_cmsg_len}",
                cmsg.header.cmsg_len
            )));
        }

        if (cmsg.header.cmsg_level, cmsg.header.cmsg_type) != (SOL_SOCKET, SCM_RIGHTS) {
            return Err(io::Error::other(format!("unsupported non-SCM_RIGHTS CMSG")));
        }

        if cmsg.fds.iter().any(|fd| fd.is_none()) {
            return Err(io::Error::other(format!("recvmsg got invalid (-1) fds")));
        }

        Ok(cmsg.fds.map(Option::unwrap))
    }
}

impl<TX, RX> Channel<TX, RX> {
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

/// Encoding/decoding functionality that relies on each message being
/// encoded to a constant (and small) "packet" size, allowing the use
/// of 1:1 `sendmsg` and `recvmsg` calls, i.e. removing the need for
/// any kind of "packet framing" that a `SOCK_STREAM` needs to soundly
/// handle receiving a message's fds through multiple `recvmsg` calls.
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
