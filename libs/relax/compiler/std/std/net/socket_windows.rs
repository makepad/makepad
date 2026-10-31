//! The socket under TcpStream / TcpListener / UdpSocket, Windows half: an OwnedSocket over
//! Winsock (real std's sys/net/connection/socket/windows.rs). Same methods as socket_unix.rs.

use core::ffi::c_int;
use core::time::Duration;

use super::Shutdown;
use crate::io;
use crate::os::windows::io::{AsRawSocket, FromRawSocket, OwnedSocket, RawSocket};
use crate::sys::net as sn;

/// The Debug field name of the raw socket.
pub(crate) const RAW_FIELD: &str = "socket";

/// listen() backlog.
pub(crate) const BACKLOG: c_int = 128;

/// A Winsock socket with the helpers TcpStream/TcpListener/UdpSocket share.
pub(crate) struct Socket {
    sock: OwnedSocket,
}

/// SOCKET_ERROR -> the thread's WSAGetLastError.
pub(crate) fn cvt(r: c_int) -> io::Result<c_int> {
    if r == sn::SOCKET_ERROR {
        Err(sn::last_error())
    } else {
        Ok(r)
    }
}

/// Buffer lengths are `int` in Winsock: longer buffers are transferred in part.
fn wrlen(len: usize) -> c_int {
    if len > i32::MAX as usize { i32::MAX } else { len as c_int }
}

/// WSASocketW without handle inheritance; Windows 7 without the SP1 update rejects
/// WSA_FLAG_NO_HANDLE_INHERIT, so retry without it and clear HANDLE_FLAG_INHERIT afterwards.
fn wsa_socket(family: c_int, ty: c_int, protocol: c_int, info: *const sn::WsaProtocolInfoW) -> io::Result<Socket> {
    let s = unsafe { sn::WSASocketW(family, ty, protocol, info, 0, sn::WSA_FLAG_OVERLAPPED | sn::WSA_FLAG_NO_HANDLE_INHERIT) };
    if s != sn::INVALID_SOCKET {
        return Ok(Socket::from_raw(s));
    }
    let error = unsafe { sn::WSAGetLastError() };
    if error != sn::WSAEPROTOTYPE && error != sn::WSAEINVAL {
        return Err(io::Error::from_raw_os_error(error));
    }
    let s = unsafe { sn::WSASocketW(family, ty, protocol, info, 0, sn::WSA_FLAG_OVERLAPPED) };
    if s == sn::INVALID_SOCKET {
        return Err(sn::last_error());
    }
    let sock = Socket::from_raw(s);
    if unsafe { sn::SetHandleInformation(s as usize as *mut core::ffi::c_void, sn::HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(sock)
}

impl Socket {
    pub(crate) fn new(family: c_int, ty: c_int) -> io::Result<Socket> {
        sn::init();
        wsa_socket(family, ty, 0, core::ptr::null())
    }
    pub(crate) fn from_raw(s: RawSocket) -> Socket {
        Socket { sock: unsafe { OwnedSocket::from_raw_socket(s) } }
    }
    pub(crate) fn from_owned(sock: OwnedSocket) -> Socket {
        Socket { sock }
    }
    pub(crate) fn raw(&self) -> RawSocket {
        self.sock.as_raw_socket()
    }
    pub(crate) fn owned(&self) -> &OwnedSocket {
        &self.sock
    }
    pub(crate) fn into_owned(self) -> OwnedSocket {
        self.sock
    }
    pub(crate) fn set_int(&self, level: c_int, name: c_int, v: c_int) -> io::Result<()> {
        cvt(unsafe { sn::setsockopt(self.raw(), level, name, &v as *const c_int as *const u8, 4) })?;
        Ok(())
    }
    pub(crate) fn get_int(&self, level: c_int, name: c_int) -> io::Result<c_int> {
        let mut v: c_int = 0;
        let mut len: sn::socklen_t = 4;
        cvt(unsafe { sn::getsockopt(self.raw(), level, name, &mut v as *mut c_int as *mut u8, &mut len as *mut sn::socklen_t) })?;
        Ok(v)
    }
    /// recv; a socket shut down for reading reads 0 (EOF) as on unix.
    fn recv_with_flags(&self, buf: &mut [u8], flags: c_int) -> io::Result<usize> {
        let r = unsafe { sn::recv(self.raw(), buf.as_mut_ptr(), wrlen(buf.len()), flags) };
        if r == sn::SOCKET_ERROR {
            let error = unsafe { sn::WSAGetLastError() };
            if error == sn::WSAESHUTDOWN {
                Ok(0)
            } else {
                Err(io::Error::from_raw_os_error(error))
            }
        } else {
            Ok(r as usize)
        }
    }
    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv_with_flags(buf, 0)
    }
    pub(crate) fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv_with_flags(buf, sn::MSG_PEEK)
    }
    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let r = cvt(unsafe { sn::send(self.raw(), buf.as_ptr(), wrlen(buf.len()), 0) })?;
        Ok(r as usize)
    }
    pub(crate) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let h = match how {
            Shutdown::Read => sn::SD_RECEIVE,
            Shutdown::Write => sn::SD_SEND,
            Shutdown::Both => sn::SD_BOTH,
        };
        cvt(unsafe { sn::shutdown(self.raw(), h) })?;
        Ok(())
    }
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let mut v: u32 = nonblocking as u32;
        cvt(unsafe { sn::ioctlsocket(self.raw(), sn::FIONBIO, &mut v as *mut u32) })?;
        Ok(())
    }
    /// SO_RCVTIMEO / SO_SNDTIMEO are a DWORD of milliseconds on Windows (0 = none), rounded
    /// up, INFINITE past u32::MAX (real std's dur2timeout).
    pub(crate) fn set_timeout(&self, dur: Option<Duration>, kind: c_int) -> io::Result<()> {
        let ms: u32 = match dur {
            Some(d) => {
                let nanos = d.subsec_nanos();
                let round = if nanos % 1_000_000 > 0 { 1 } else { 0 };
                let ms = match d.as_secs().checked_mul(1000) {
                    Some(v) => match v.checked_add((nanos / 1_000_000) as u64 + round) {
                        Some(v) => v,
                        None => sn::INFINITE as u64,
                    },
                    None => sn::INFINITE as u64,
                };
                let ms = if ms > u32::MAX as u64 { sn::INFINITE } else { ms as u32 };
                if ms == 0 {
                    return Err(io::Error::ZERO_TIMEOUT);
                }
                ms
            }
            None => 0,
        };
        cvt(unsafe { sn::setsockopt(self.raw(), sn::SOL_SOCKET, kind, &ms as *const u32 as *const u8, 4) })?;
        Ok(())
    }
    pub(crate) fn timeout(&self, kind: c_int) -> io::Result<Option<Duration>> {
        let raw = self.get_int(sn::SOL_SOCKET, kind)? as u32;
        if raw == 0 {
            Ok(None)
        } else {
            Ok(Some(Duration::new((raw / 1000) as u64, (raw % 1000) * 1_000_000)))
        }
    }
    pub(crate) fn take_error(&self) -> io::Result<Option<io::Error>> {
        let e = self.get_int(sn::SOL_SOCKET, sn::SO_ERROR)?;
        if e == 0 {
            Ok(None)
        } else {
            Ok(Some(io::Error::from_raw_os_error(e)))
        }
    }
    /// WSADuplicateSocketW + WSASocketW from its protocol info (real std's
    /// BorrowedSocket::try_clone_to_owned).
    pub(crate) fn try_clone(&self) -> io::Result<Socket> {
        let mut info = sn::WsaProtocolInfoW {
            dw_service_flags1: 0,
            dw_service_flags2: 0,
            dw_service_flags3: 0,
            dw_service_flags4: 0,
            dw_provider_flags: 0,
            provider_id: sn::Guid { data1: 0, data2: 0, data3: 0, data4: [0; 8] },
            dw_catalog_entry_id: 0,
            protocol_chain: sn::WsaProtocolChain { chain_len: 0, chain_entries: [0; 7] },
            i_version: 0,
            i_address_family: 0,
            i_max_sock_addr: 0,
            i_min_sock_addr: 0,
            i_socket_type: 0,
            i_protocol: 0,
            i_protocol_max_offset: 0,
            i_network_byte_order: 0,
            i_security_scheme: 0,
            dw_message_size: 0,
            dw_provider_reserved: 0,
            sz_protocol: [0; 256],
        };
        cvt(unsafe { sn::WSADuplicateSocketW(self.raw(), sn::GetCurrentProcessId(), &mut info as *mut sn::WsaProtocolInfoW) })?;
        wsa_socket(info.i_address_family, info.i_socket_type, info.i_protocol, &info as *const sn::WsaProtocolInfoW)
    }
    pub(crate) fn name(&self, peer: bool) -> io::Result<(sn::SockaddrStorage, sn::socklen_t)> {
        let mut buf = sn::zeroed();
        let mut len: sn::socklen_t = 128;
        let r = unsafe {
            if peer {
                sn::getpeername(self.raw(), sn::storage_mut_ptr(&mut buf), &mut len as *mut sn::socklen_t)
            } else {
                sn::getsockname(self.raw(), sn::storage_mut_ptr(&mut buf), &mut len as *mut sn::socklen_t)
            }
        };
        cvt(r)?;
        Ok((buf, len))
    }
    pub(crate) fn accept(&self) -> io::Result<(Socket, sn::SockaddrStorage, sn::socklen_t)> {
        let mut buf = sn::zeroed();
        let mut len: sn::socklen_t = 128;
        let s = unsafe { sn::accept(self.raw(), sn::storage_mut_ptr(&mut buf), &mut len as *mut sn::socklen_t) };
        if s == sn::INVALID_SOCKET {
            return Err(sn::last_error());
        }
        Ok((Socket::from_raw(s), buf, len))
    }
    pub(crate) fn connect(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t) -> io::Result<()> {
        cvt(unsafe { sn::connect(self.raw(), sn::storage_ptr(buf), len) })?;
        Ok(())
    }
    /// Non-blocking connect + select for writability (real std's Windows connect_timeout).
    pub(crate) fn connect_timeout(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t, timeout: Duration) -> io::Result<()> {
        self.set_nonblocking(true)?;
        let r = self.connect(buf, len);
        self.set_nonblocking(false)?;
        let e = match r {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        if e.raw_os_error() != Some(sn::WSAEWOULDBLOCK) {
            return Err(e);
        }
        if timeout.is_zero() {
            return Err(io::Error::ZERO_TIMEOUT);
        }
        let secs = if timeout.as_secs() > i32::MAX as u64 { i32::MAX } else { timeout.as_secs() as i32 };
        let mut tv = sn::Timeval { tv_sec: secs, tv_usec: timeout.subsec_micros() as i32 };
        if tv.tv_sec == 0 && tv.tv_usec == 0 {
            tv.tv_usec = 1;
        }
        let mut fds = sn::FdSet { fd_count: 1, fd_array: [0; 64] };
        fds.fd_array[0] = self.raw();
        let mut writefds = fds;
        let mut errorfds = fds;
        let n = cvt(unsafe { sn::select(1, core::ptr::null_mut(), &mut writefds as *mut sn::FdSet, &mut errorfds as *mut sn::FdSet, &tv as *const sn::Timeval) })?;
        if n == 0 {
            return Err(io::const_error!(io::ErrorKind::TimedOut, "connection timed out"));
        }
        if writefds.fd_count != 1 {
            if let Some(e) = self.take_error()? {
                return Err(e);
            }
        }
        Ok(())
    }
    pub(crate) fn bind(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t) -> io::Result<()> {
        cvt(unsafe { sn::bind(self.raw(), sn::storage_ptr(buf), len) })?;
        Ok(())
    }
    pub(crate) fn listen(&self, backlog: c_int) -> io::Result<()> {
        cvt(unsafe { sn::listen(self.raw(), backlog) })?;
        Ok(())
    }
    /// setsockopt with an arbitrary option value (ip_mreq / ipv6_mreq).
    pub(crate) fn set_raw_opt(&self, level: c_int, name: c_int, value: *const u8, len: usize) -> io::Result<()> {
        cvt(unsafe { sn::setsockopt(self.raw(), level, name, value, len as sn::socklen_t) })?;
        Ok(())
    }
    /// recvfrom: the byte count and the sender's address. A socket shut down for reading
    /// reads 0 (with whatever address recvfrom left), as real std.
    pub(crate) fn recv_from(&self, buf: &mut [u8], peek: bool) -> io::Result<(usize, sn::SockaddrStorage)> {
        let mut addr = sn::zeroed();
        let mut len: sn::socklen_t = 128;
        let flags = if peek { sn::MSG_PEEK } else { 0 };
        let r = unsafe {
            sn::recvfrom(self.raw(), buf.as_mut_ptr(), wrlen(buf.len()), flags,
                sn::storage_mut_ptr(&mut addr), &mut len as *mut sn::socklen_t)
        };
        if r == sn::SOCKET_ERROR {
            let error = unsafe { sn::WSAGetLastError() };
            if error == sn::WSAESHUTDOWN {
                Ok((0, addr))
            } else {
                Err(io::Error::from_raw_os_error(error))
            }
        } else {
            Ok((r as usize, addr))
        }
    }
    pub(crate) fn send_to(&self, buf: &[u8], addr: &sn::SockaddrStorage, len: sn::socklen_t) -> io::Result<usize> {
        let r = cvt(unsafe { sn::sendto(self.raw(), buf.as_ptr(), wrlen(buf.len()), 0, sn::storage_ptr(addr), len) })?;
        Ok(r as usize)
    }
}
