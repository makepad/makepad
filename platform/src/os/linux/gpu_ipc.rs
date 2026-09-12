//! Framed, nonblocking GPU descriptor transport. A single worker owns each
//! channel; the UI exchanges packets with that worker through bounded queues.
//! The one-byte ancillary prefix separates FDs from stream fragmentation: the
//! receiver never attributes a later message's FD to an earlier header.
use std::{
    ffi::{c_int, c_void},
    io::{self, Read, Write},
    os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::net::UnixStream},
};

const HEADER_SIZE: usize = 40;
const SOL_SOCKET: c_int = 1;
const SCM_RIGHTS: c_int = 1;
const MSG_DONTWAIT: c_int = 0x40;
const MSG_NOSIGNAL: c_int = 0x4000;
const MSG_CMSG_CLOEXEC: c_int = 0x4000_0000;
const MSG_CTRUNC: c_int = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GpuFdKind {
    Memory = 1,
    Timeline = 2,
    FrameReady = 3,
    FrameReleased = 4,
}

/// No memory type index or process-local Vulkan handle crosses this channel.
/// `None` denotes an already-signaled SYNC_FD for frame ready/released only.
pub(crate) struct GpuFdPacket {
    pub kind: GpuFdKind,
    pub epoch: u64,
    pub image: u64,
    pub sequence: u64,
    pub plane: u32,
    pub fd: Option<OwnedFd>,
}

impl GpuFdPacket {
    fn header(&self) -> io::Result<[u8; HEADER_SIZE]> {
        if self.epoch == 0 || self.image == 0 || self.plane != 0 {
            return Err(invalid("invalid GPU transport epoch, image or plane"));
        }
        let memory = matches!(self.kind, GpuFdKind::Memory | GpuFdKind::Timeline);
        if (memory && (self.sequence != 0 || self.fd.is_none())) || (!memory && self.sequence == 0) {
            return Err(invalid("invalid GPU descriptor kind/sequence/FD combination"));
        }
        let mut bytes = [0; HEADER_SIZE];
        bytes[0] = 0x10 | u8::from(self.fd.is_some());
        bytes[1..4].copy_from_slice(b"MPG");
        bytes[4] = self.kind as u8;
        bytes[8..16].copy_from_slice(&self.epoch.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.image.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.plane.to_le_bytes());
        Ok(bytes)
    }

    fn decode(bytes: [u8; HEADER_SIZE], fd: Option<OwnedFd>) -> io::Result<Self> {
        let kind = match bytes[4] {
            1 => GpuFdKind::Memory,
            2 => GpuFdKind::Timeline,
            3 => GpuFdKind::FrameReady,
            4 => GpuFdKind::FrameReleased,
            _ => return Err(invalid("unknown GPU descriptor kind")),
        };
        let packet = Self {
            kind,
            epoch: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            image: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            sequence: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            plane: u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
            fd,
        };
        if packet.header()? != bytes { return Err(invalid("malformed GPU descriptor header")); }
        Ok(packet)
    }
}

struct Outgoing {
    packet: GpuFdPacket,
    bytes: [u8; HEADER_SIZE],
    written: usize,
}

struct Incoming {
    bytes: [u8; HEADER_SIZE],
    read: usize,
    fd: Option<OwnedFd>,
}

impl Default for Incoming {
    fn default() -> Self { Self { bytes: [0; HEADER_SIZE], read: 0, fd: None } }
}

pub(crate) struct GpuFdChannel {
    stream: UnixStream,
    outgoing: Option<Outgoing>,
    incoming: Incoming,
    failed: bool,
}

impl GpuFdChannel {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, outgoing: None, incoming: Incoming::default(), failed: false })
    }

    /// Capacity is exactly one packet. The caller retains a full-queue packet
    /// and retries after flush, so neither descriptor nor release is dropped.
    pub fn enqueue(&mut self, packet: GpuFdPacket) -> Result<(), (io::Error, GpuFdPacket)> {
        if self.failed { return Err((disconnected(), packet)); }
        if self.outgoing.is_some() {
            return Err((io::Error::new(io::ErrorKind::WouldBlock, "GPU descriptor slot is full"), packet));
        }
        let bytes = match packet.header() {
            Ok(bytes) => bytes,
            Err(error) => return Err((error, packet)),
        };
        self.outgoing = Some(Outgoing { packet, bytes, written: 0 });
        Ok(())
    }

    /// `true` means the slot is empty. A partial write retains the cursor;
    /// ancillary data is sent exactly once, attached only to byte zero.
    pub fn flush(&mut self) -> io::Result<bool> {
        if self.failed { return Err(disconnected()); }
        let result = self.flush_inner();
        if result.is_err() { self.fail(); }
        result
    }

    fn flush_inner(&mut self) -> io::Result<bool> {
        let Some(outgoing) = &mut self.outgoing else { return Ok(true); };
        if outgoing.written == 0 {
            match send_prefix(&self.stream, outgoing.bytes[0], outgoing.packet.fd.as_ref()) {
                Ok(()) => {
                    outgoing.written = 1;
                    // The kernel now owns its reference. Retaining the sender
                    // FD until this point is essential if the socket was full.
                    outgoing.packet.fd.take();
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        while outgoing.written < HEADER_SIZE {
            match (&self.stream).write(&outgoing.bytes[outgoing.written..]) {
                Ok(0) => return Err(disconnected()),
                Ok(count) => outgoing.written += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        self.outgoing = None;
        Ok(true)
    }

    pub fn try_recv(&mut self) -> io::Result<Option<GpuFdPacket>> {
        if self.failed { return Err(disconnected()); }
        let result = self.recv_inner();
        if result.is_err() { self.fail(); }
        result
    }

    fn recv_inner(&mut self) -> io::Result<Option<GpuFdPacket>> {
        if self.incoming.read == 0 {
            let (prefix, fd) = match recv_prefix(&self.stream) {
                Ok(value) => value,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) => return Err(error),
            };
            if prefix != (0x10 | u8::from(fd.is_some())) { return Err(invalid("invalid GPU ancillary prefix")); }
            self.incoming.bytes[0] = prefix;
            self.incoming.fd = fd;
            self.incoming.read = 1;
        }
        while self.incoming.read < HEADER_SIZE {
            match (&self.stream).read(&mut self.incoming.bytes[self.incoming.read..]) {
                Ok(0) => return Err(disconnected()),
                Ok(count) => self.incoming.read += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        let incoming = std::mem::take(&mut self.incoming);
        GpuFdPacket::decode(incoming.bytes, incoming.fd).map(Some)
    }

    fn fail(&mut self) {
        self.failed = true;
        self.outgoing.take();
        self.incoming = Incoming::default();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
fn disconnected() -> io::Error { io::Error::new(io::ErrorKind::BrokenPipe, "GPU descriptor channel closed") }

#[repr(C)]
struct IoVec { base: *mut c_void, len: usize }
#[repr(C)]
struct MsgHeader {
    name: *mut c_void, name_len: u32,
    iov: *mut IoVec, iov_len: usize,
    control: *mut c_void, control_len: usize, flags: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct ControlHeader { len: usize, level: c_int, kind: c_int }

extern "C" {
    fn sendmsg(fd: c_int, msg: *const MsgHeader, flags: c_int) -> isize;
    fn recvmsg(fd: c_int, msg: *mut MsgHeader, flags: c_int) -> isize;
}

fn send_prefix(stream: &UnixStream, mut byte: u8, fd: Option<&OwnedFd>) -> io::Result<()> {
    let mut control = [0usize; 4];
    let header_size = std::mem::size_of::<ControlHeader>();
    let mut iov = IoVec { base: (&mut byte as *mut u8).cast(), len: 1 };
    let mut message = MsgHeader { name: std::ptr::null_mut(), name_len: 0,
        iov: &mut iov, iov_len: 1, control: std::ptr::null_mut(), control_len: 0, flags: 0 };
    if let Some(fd) = fd {
        unsafe {
            control.as_mut_ptr().cast::<ControlHeader>().write(ControlHeader {
                len: header_size + 4, level: SOL_SOCKET, kind: SCM_RIGHTS,
            });
            control.as_mut_ptr().cast::<u8>().add(header_size).cast::<c_int>().write(fd.as_raw_fd());
        }
        message.control = control.as_mut_ptr().cast();
        message.control_len = (header_size + 4).next_multiple_of(std::mem::size_of::<usize>());
    }
    loop {
        match unsafe { sendmsg(stream.as_raw_fd(), &message, MSG_DONTWAIT | MSG_NOSIGNAL) } {
            1 => return Ok(()),
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
            }
            _ => return Err(disconnected()),
        }
    }
}

fn recv_prefix(stream: &UnixStream) -> io::Result<(u8, Option<OwnedFd>)> {
    let mut byte = 0u8;
    let mut control = [0usize; 16];
    let header_size = std::mem::size_of::<ControlHeader>();
    let mut iov = IoVec { base: (&mut byte as *mut u8).cast(), len: 1 };
    let mut message = MsgHeader { name: std::ptr::null_mut(), name_len: 0,
        iov: &mut iov, iov_len: 1, control: control.as_mut_ptr().cast(),
        control_len: std::mem::size_of_val(&control), flags: 0 };
    loop {
        match unsafe { recvmsg(stream.as_raw_fd(), &mut message, MSG_DONTWAIT | MSG_CMSG_CLOEXEC) } {
            1 => break,
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
            }
            _ => return Err(disconnected()),
        }
    }
    let mut received = None;
    let mut invalid_control = message.flags & MSG_CTRUNC != 0;
    let mut offset = 0usize;
    while offset + header_size <= message.control_len {
        let header = unsafe { control.as_ptr().cast::<u8>().add(offset).cast::<ControlHeader>().read() };
        if header.len < header_size || header.len > message.control_len - offset {
            invalid_control = true;
            break;
        }
        if header.level == SOL_SOCKET && header.kind == SCM_RIGHTS {
            let length = header.len - header_size;
            if length % 4 != 0 { invalid_control = true; }
            for index in 0..length / 4 {
                let raw = unsafe { control.as_ptr().cast::<u8>().add(offset + header_size + index * 4).cast::<c_int>().read() };
                if raw < 0 { invalid_control = true; continue; }
                let fd = unsafe { OwnedFd::from_raw_fd(raw) };
                if received.is_some() { invalid_control = true; } else { received = Some(fd); }
            }
        } else { invalid_control = true; }
        offset += header.len.next_multiple_of(std::mem::size_of::<usize>());
    }
    if invalid_control { return Err(invalid("unexpected or truncated GPU ancillary data")); }
    Ok((byte, received))
}
