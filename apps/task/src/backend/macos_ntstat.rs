//! Per-process network traffic on macOS, from the kernel's network
//! statistics control (`com.apple.network.statistics`, xnu
//! `bsd/net/ntstat.{h,c}`) — the source `nettop` and Activity Monitor read.
//!
//! One `PF_SYSTEM` control socket lives on the sampler thread for the life
//! of the backend. At connect it subscribes to every TCP, UDP and QUIC
//! source, kernel and userland (`NSTAT_MSG_TYPE_ADD_ALL_SRCS`, with
//! `NSTAT_FILTER_USE_UPDATE_FOR_ADD | NSTAT_FILTER_SUPPRESS_SRC_ADDED`, so
//! the only messages that come back are updates and removals). Every tick
//! polls all sources (`NSTAT_MSG_TYPE_GET_UPDATE`, `srcref = ALL`, in
//! continuation fragments): each `NSTAT_MSG_TYPE_SRC_UPDATE` carries one
//! socket's lifetime `nstat_counts` and its descriptor, which names the
//! owning `pid` and its `upid` (the kernel's never-reused unique pid).
//!
//! On macOS this needs no privilege: `net.statistics_privcheck` is 0 there
//! and these providers are exempt from the entitlement check (only
//! `CONN_USERLAND` and `UDP_SUBFLOW` always require it, and are not used —
//! connection-level counts would count the same bytes twice anyway).
//!
//! **Attribution.** Traffic belongs to the descriptor's `pid`, the process
//! that owns the socket, not `epid` (the process it acts for, as when
//! `nsurlsessiond` downloads for an app): this is what `nettop` lists.
//!
//! **Monotonic per process.** A process' figure is the sum over its live
//! sockets plus the last counts of every socket it closed while we watched,
//! so it only grows for the life of the incarnation. A socket closed before
//! the backend connected is not counted. A pid that starts a new
//! incarnation (a different `ProcKey` start, or a higher `upid`) starts
//! from zero. A socket whose removal message was lost (receive buffer
//! full) is still retired: a complete poll that no longer reports it folds
//! its last counts into the closed total.
//!
//! **Layouts.** The descriptor structs are private and versioned. Their
//! sizes and the offsets read below were worked out from the current xnu
//! header (`activity_bitmap_t` is 24 bytes, `nstat_counts` 112) and every
//! update's length is checked against them: a message whose size does not
//! match turns the whole reader off, and every process reads `None` rather
//! than a number from the wrong bytes.

use makepad_widgets::log;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};

#[link(name = "System", kind = "dylib")]
extern "C" {
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn ioctl(fd: c_int, request: u64, ...) -> c_int;
    fn connect(fd: c_int, address: *const c_void, len: u32) -> c_int;
    fn send(fd: c_int, buffer: *const c_void, len: usize, flags: c_int) -> isize;
    fn recv(fd: c_int, buffer: *mut c_void, len: usize, flags: c_int) -> isize;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
    fn close(fd: c_int) -> c_int;
}

const PF_SYSTEM: c_int = 32;
const AF_SYS_CONTROL: u16 = 2;
const SOCK_DGRAM: c_int = 2;
const SYSPROTO_CONTROL: c_int = 2;
const SOL_SOCKET: c_int = 0xffff;
const SO_RCVBUF: c_int = 0x1002;
const SO_RCVTIMEO: c_int = 0x1006;
/// `_IOWR('N', 3, struct ctl_info)`, `sizeof(struct ctl_info)` = 100.
const CTLIOCGINFO: u64 = 0xC064_4E03;
const CONTROL_NAME: &[u8] = b"com.apple.network.statistics";

const NSTAT_PROVIDER_TCP_KERNEL: u32 = 2;
const NSTAT_PROVIDER_TCP_USERLAND: u32 = 3;
const NSTAT_PROVIDER_UDP_KERNEL: u32 = 4;
const NSTAT_PROVIDER_UDP_USERLAND: u32 = 5;
const NSTAT_PROVIDER_QUIC_USERLAND: u32 = 8;
const PROVIDERS: [u32; 5] = [
    NSTAT_PROVIDER_TCP_KERNEL,
    NSTAT_PROVIDER_TCP_USERLAND,
    NSTAT_PROVIDER_UDP_KERNEL,
    NSTAT_PROVIDER_UDP_USERLAND,
    NSTAT_PROVIDER_QUIC_USERLAND,
];

const NSTAT_MSG_TYPE_SUCCESS: u32 = 0;
const NSTAT_MSG_TYPE_ERROR: u32 = 1;
const NSTAT_MSG_TYPE_ADD_ALL_SRCS: u32 = 1002;
const NSTAT_MSG_TYPE_GET_UPDATE: u32 = 1007;
const NSTAT_MSG_TYPE_SRC_REMOVED: u32 = 10002;
const NSTAT_MSG_TYPE_SRC_UPDATE: u32 = 10006;
const NSTAT_MSG_HDR_FLAG_CONTINUATION: u16 = 1 << 1;
const NSTAT_SRC_REF_ALL: u64 = u64::MAX;
const NSTAT_FILTER_SUPPRESS_SRC_ADDED: u64 = 0x0010_0000;
const NSTAT_FILTER_USE_UPDATE_FOR_ADD: u64 = 0x0020_0000;

/// `nstat_msg_hdr`: context u64, type u32, length u16, flags u16.
const HDR_LEN: usize = 16;
/// `nstat_msg_add_all_srcs`: hdr, filter, events, provider, target_pid,
/// target_uuid.
const ADD_ALL_LEN: usize = HDR_LEN + 8 + 8 + 4 + 4 + 16;
/// `nstat_msg_query_src_req`: hdr, srcref.
const QUERY_LEN: usize = HDR_LEN + 8;
/// `nstat_msg_src_update` up to its descriptor: hdr, srcref, event_flags,
/// `nstat_counts` (10 × u64 + 8 × u32 = 112), provider, reserved[4].
const UPDATE_HDR_LEN: usize = HDR_LEN + 8 + 8 + 112 + 8;
/// `nstat_counts` inside an update: rxpackets, rxbytes, txpackets, txbytes.
const COUNTS_AT: usize = HDR_LEN + 16;
const PROVIDER_AT: usize = COUNTS_AT + 112;
/// `sizeof(nstat_tcp_descriptor)` (also QUIC's): `pid` after four u64s,
/// the two transfer sizes, `activity_bitmap_t` and eleven u32s.
const TCP_DESC_LEN: usize = 344;
const TCP_PID_AT: usize = 116;
/// `sizeof(nstat_udp_descriptor)`: `pid` after four u64s, the bitmap, two
/// socket-address unions (28 each) and four u32s.
const UDP_DESC_LEN: usize = 280;
const UDP_PID_AT: usize = 128;
/// Both descriptors start with `upid`.
const UPID_AT: usize = 0;

/// Received / sent bytes and packets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetCounts {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
}

impl NetCounts {
    fn add(&mut self, other: &NetCounts) {
        self.rx_bytes = self.rx_bytes.saturating_add(other.rx_bytes);
        self.tx_bytes = self.tx_bytes.saturating_add(other.tx_bytes);
        self.rx_packets = self.rx_packets.saturating_add(other.rx_packets);
        self.tx_packets = self.tx_packets.saturating_add(other.tx_packets);
    }
}

struct Source {
    pid: u32,
    upid: u64,
    counts: NetCounts,
}

/// What a pid's closed sockets carried: for the newest `upid` only.
struct Closed {
    upid: u64,
    counts: NetCounts,
}

pub struct Ntstat {
    fd: c_int,
    context: u64,
    sources: HashMap<u64, Source>,
    closed: HashMap<u32, Closed>,
    /// The `ProcKey` start each pid's totals belong to.
    owners: HashMap<u32, u64>,
    /// Per pid, after the last poll: (upid, counts).
    totals: HashMap<u32, (u64, NetCounts)>,
    /// The sources a poll in progress has reported.
    reported: Option<HashSet<u64>>,
    /// An update did not have the layout this reader knows.
    broken: bool,
    buffer: Vec<u8>,
}

impl Ntstat {
    /// Connect and subscribe; `None` (logged) when the control is missing or
    /// refuses us.
    pub fn open() -> Option<Self> {
        // SAFETY: plain socket calls on buffers we own and size exactly.
        unsafe {
            let fd = socket(PF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL);
            if fd < 0 {
                log!("task: ntstat socket failed; per-process network off");
                return None;
            }
            let mut info = [0u8; 100];
            info[4..4 + CONTROL_NAME.len()].copy_from_slice(CONTROL_NAME);
            if ioctl(fd, CTLIOCGINFO, info.as_mut_ptr()) != 0 {
                log!("task: ntstat control not found; per-process network off");
                close(fd);
                return None;
            }
            let id = u32::from_ne_bytes([info[0], info[1], info[2], info[3]]);
            // struct sockaddr_ctl: len, family, ss_sysaddr, id, unit, reserved[5].
            let mut address = [0u8; 32];
            address[0] = 32;
            address[1] = PF_SYSTEM as u8;
            address[2..4].copy_from_slice(&AF_SYS_CONTROL.to_ne_bytes());
            address[4..8].copy_from_slice(&id.to_ne_bytes());
            if connect(fd, address.as_ptr().cast(), 32) != 0 {
                log!("task: ntstat connect refused; per-process network off");
                close(fd);
                return None;
            }
            let rcvbuf: c_int = 4 * 1024 * 1024;
            setsockopt(fd, SOL_SOCKET, SO_RCVBUF, (&rcvbuf as *const c_int).cast(), 4);
            // A poll's answer is already queued when send returns; the
            // timeout only bounds a kernel that never answers.
            let timeout: [i64; 2] = [0, 200_000];
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, timeout.as_ptr().cast(), 16);
            let mut this = Self {
                fd,
                context: 0,
                sources: HashMap::new(),
                closed: HashMap::new(),
                owners: HashMap::new(),
                totals: HashMap::new(),
                reported: None,
                broken: false,
                buffer: vec![0u8; 256 * 1024],
            };
            for provider in PROVIDERS {
                let mut request = [0u8; ADD_ALL_LEN];
                let context = this.next_context();
                put_hdr(&mut request, context, NSTAT_MSG_TYPE_ADD_ALL_SRCS, 0);
                request[16..24].copy_from_slice(&(NSTAT_FILTER_SUPPRESS_SRC_ADDED | NSTAT_FILTER_USE_UPDATE_FOR_ADD).to_ne_bytes());
                request[32..36].copy_from_slice(&provider.to_ne_bytes());
                if !this.request(&request, context).is_some_and(|continued| !continued) {
                    log!("task: ntstat refused provider {provider}; per-process network off");
                    return None;
                }
            }
            Some(this)
        }
    }

    fn next_context(&mut self) -> u64 {
        self.context += 1;
        self.context
    }

    /// Send `request` and read until the kernel answers `context`:
    /// `Some(continuation)` on success, `None` on an error, a timeout or a
    /// layout mismatch. Updates and removals read on the way are applied.
    fn request(&mut self, request: &[u8], context: u64) -> Option<bool> {
        // SAFETY: request is a live buffer of its own length.
        if unsafe { send(self.fd, request.as_ptr().cast(), request.len(), 0) } != request.len() as isize {
            return None;
        }
        loop {
            // SAFETY: recv writes at most buffer.len() bytes into it.
            let read = unsafe { recv(self.fd, self.buffer.as_mut_ptr().cast(), self.buffer.len(), 0) };
            if read <= 0 {
                return None;
            }
            let mut at = 0usize;
            let end = read as usize;
            let mut answer = None;
            // A datagram may carry several messages back to back.
            while at + HDR_LEN <= end {
                let message_context = read_u64(&self.buffer, at);
                let kind = read_u32(&self.buffer, at + 8);
                let length = u16::from_ne_bytes([self.buffer[at + 12], self.buffer[at + 13]]) as usize;
                let flags = u16::from_ne_bytes([self.buffer[at + 14], self.buffer[at + 15]]);
                if length < HDR_LEN || at + length > end {
                    break;
                }
                match kind {
                    NSTAT_MSG_TYPE_SRC_UPDATE => {
                        let message = self.buffer[at..at + length].to_vec();
                        if !self.apply_update(&message) {
                            self.broken = true;
                            return None;
                        }
                    }
                    NSTAT_MSG_TYPE_SRC_REMOVED if length >= HDR_LEN + 8 => {
                        let srcref = read_u64(&self.buffer, at + HDR_LEN);
                        self.retire(srcref);
                    }
                    NSTAT_MSG_TYPE_SUCCESS if message_context == context => {
                        answer = Some(Some(flags & NSTAT_MSG_HDR_FLAG_CONTINUATION != 0));
                    }
                    NSTAT_MSG_TYPE_ERROR if message_context == context => answer = Some(None),
                    _ => {}
                }
                at += length;
            }
            if let Some(answer) = answer {
                return answer;
            }
        }
    }

    /// One `SRC_UPDATE`: false when its layout is not the one this reader
    /// was written for.
    fn apply_update(&mut self, message: &[u8]) -> bool {
        if message.len() < UPDATE_HDR_LEN {
            return false;
        }
        let provider = read_u32(message, PROVIDER_AT);
        let (desc_len, pid_at) = match provider {
            NSTAT_PROVIDER_TCP_KERNEL | NSTAT_PROVIDER_TCP_USERLAND | NSTAT_PROVIDER_QUIC_USERLAND => (TCP_DESC_LEN, TCP_PID_AT),
            NSTAT_PROVIDER_UDP_KERNEL | NSTAT_PROVIDER_UDP_USERLAND => (UDP_DESC_LEN, UDP_PID_AT),
            // Nothing else was subscribed to.
            _ => return true,
        };
        if message.len() != UPDATE_HDR_LEN + desc_len {
            log!("task: ntstat provider {provider} update is {} bytes, expected {}; per-process network off", message.len(), UPDATE_HDR_LEN + desc_len);
            return false;
        }
        let srcref = read_u64(message, HDR_LEN);
        if let Some(reported) = &mut self.reported {
            reported.insert(srcref);
        }
        let desc = &message[UPDATE_HDR_LEN..];
        let counts = NetCounts {
            rx_packets: read_u64(message, COUNTS_AT),
            rx_bytes: read_u64(message, COUNTS_AT + 8),
            tx_packets: read_u64(message, COUNTS_AT + 16),
            tx_bytes: read_u64(message, COUNTS_AT + 24),
        };
        self.sources.insert(srcref, Source { pid: read_u32(desc, pid_at), upid: read_u64(desc, UPID_AT), counts });
        true
    }

    /// A socket is gone: its last counts join its process' closed total.
    fn retire(&mut self, srcref: u64) {
        let Some(source) = self.sources.remove(&srcref) else { return };
        let closed = self.closed.entry(source.pid).or_insert(Closed { upid: source.upid, counts: NetCounts::default() });
        if source.upid > closed.upid {
            *closed = Closed { upid: source.upid, counts: NetCounts::default() };
        }
        if source.upid == closed.upid {
            closed.counts.add(&source.counts);
        }
    }

    /// Poll every source once. False when the reader has to be dropped (a
    /// layout it cannot read); a poll that merely failed keeps the last
    /// totals.
    pub fn poll(&mut self) -> bool {
        let context = self.next_context();
        let mut request = [0u8; QUERY_LEN];
        put_hdr(&mut request, context, NSTAT_MSG_TYPE_GET_UPDATE, NSTAT_MSG_HDR_FLAG_CONTINUATION);
        request[16..24].copy_from_slice(&NSTAT_SRC_REF_ALL.to_ne_bytes());
        let before: Vec<u64> = self.sources.keys().copied().collect();
        self.reported = Some(HashSet::with_capacity(before.len()));
        // The kernel answers a large poll in fragments, each ending in a
        // success flagged CONTINUATION; the same context asks for the next.
        let complete = loop {
            match self.request(&request, context) {
                Some(true) => continue,
                Some(false) => break true,
                None => break false,
            }
        };
        let reported = self.reported.take().unwrap_or_default();
        if self.broken {
            return false;
        }
        if complete {
            // A complete poll reports every live source; one it did not was
            // closed and its removal never reached us.
            for srcref in before {
                if !reported.contains(&srcref) {
                    self.retire(srcref);
                }
            }
        }
        self.rebuild_totals();
        true
    }

    fn rebuild_totals(&mut self) {
        self.totals.clear();
        let mut merge = |pid: u32, upid: u64, counts: &NetCounts| {
            let entry = self.totals.entry(pid).or_insert((upid, NetCounts::default()));
            if upid > entry.0 {
                *entry = (upid, NetCounts::default());
            }
            if upid == entry.0 {
                entry.1.add(counts);
            }
        };
        for source in self.sources.values() {
            merge(source.pid, source.upid, &source.counts);
        }
        for (pid, closed) in &self.closed {
            merge(*pid, closed.upid, &closed.counts);
        }
    }

    /// The traffic of the incarnation of `pid` that started at `start` (its
    /// `ProcKey` start): zero for a process with no sockets. A different
    /// start than last time is a new process on a reused pid, and what the
    /// old one closed is forgotten.
    pub fn counts(&mut self, pid: u32, start: u64) -> NetCounts {
        if self.owners.insert(pid, start).is_some_and(|previous| previous != start) {
            self.closed.remove(&pid);
            self.totals.remove(&pid);
            // Its live sockets, if any, are the new process' own (the
            // kernel drops a dead process' sources); keep them.
            for source in self.sources.values().filter(|source| source.pid == pid) {
                let entry = self.totals.entry(pid).or_insert((source.upid, NetCounts::default()));
                entry.1.add(&source.counts);
            }
        }
        self.totals.get(&pid).map(|(_, counts)| *counts).unwrap_or_default()
    }

    /// Forget pids that are no longer running.
    pub fn retain_pids(&mut self, alive: &HashSet<u32>) {
        self.closed.retain(|pid, _| alive.contains(pid));
        self.owners.retain(|pid, _| alive.contains(pid));
    }
}

fn put_hdr(buffer: &mut [u8], context: u64, kind: u32, flags: u16) {
    let length = buffer.len() as u16;
    buffer[0..8].copy_from_slice(&context.to_ne_bytes());
    buffer[8..12].copy_from_slice(&kind.to_ne_bytes());
    buffer[12..14].copy_from_slice(&length.to_ne_bytes());
    buffer[14..16].copy_from_slice(&flags.to_ne_bytes());
}

fn read_u32(buffer: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(buffer[at..at + 4].try_into().unwrap())
}

fn read_u64(buffer: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(buffer[at..at + 8].try_into().unwrap())
}

impl Drop for Ntstat {
    fn drop(&mut self) {
        // SAFETY: we own the descriptor.
        unsafe { close(self.fd) };
    }
}
