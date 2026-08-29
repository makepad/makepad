//! The peer table: a fixed array of slots shared between the network thread
//! (which allocates, updates and expires peers) and the audio thread (which
//! renders them). Everything the audio thread touches is atomic; there is no
//! lock and no allocation after construction.

use crate::jitter::JitterRing;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// Fixed capacity of the peer table.
pub const MAX_PEERS: usize = 32;

/// Pack an IPv4 socket address into a nonzero u64 (0 = none). IPv6 peers are
/// heard fine (their audio arrives and plays) but cannot be unicast targets;
/// this is a LAN IPv4 crate.
pub(crate) fn pack_addr(addr: SocketAddr) -> u64 {
    match addr.ip() {
        IpAddr::V4(ip) => ((u32::from(ip) as u64) << 16) | addr.port() as u64,
        IpAddr::V6(_) => 0,
    }
}

pub(crate) fn unpack_addr(packed: u64) -> Option<SocketAddr> {
    if packed == 0 {
        return None;
    }
    let ip = Ipv4Addr::from((packed >> 16) as u32);
    Some(SocketAddr::new(IpAddr::V4(ip), (packed & 0xFFFF) as u16))
}

pub(crate) struct PeerSlot {
    /// 0 = free, 1 = active. Stored last (Release) on allocation.
    pub state: AtomicU8,
    /// Bumped on every allocation so the audio side knows to reset its
    /// per-peer playout state when a slot is reused.
    pub generation: AtomicU32,
    pub sender: AtomicU64,
    /// Packed IPv4:port ([`pack_addr`]); 0 = unknown.
    pub addr: AtomicU64,
    pub last_seen_ms: AtomicU64,
    pub channel: AtomicU8,
    /// f32 bits; local playback gain for this peer.
    pub gain: AtomicU32,
    /// Local mute for this peer (skips rendering; presence stays).
    pub muted: AtomicBool,
    /// Written by the audio thread during rendering.
    pub talking: AtomicBool,
    /// Written by the audio thread: buffered audio in ms × 10.
    pub buffered_ms_x10: AtomicU32,
    /// Written by the audio thread: current jitter target in frames.
    pub target_frames: AtomicU32,
    pub packets: AtomicU64,
    pub ring: JitterRing,
}

impl PeerSlot {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            generation: AtomicU32::new(0),
            sender: AtomicU64::new(0),
            addr: AtomicU64::new(0),
            last_seen_ms: AtomicU64::new(0),
            channel: AtomicU8::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            talking: AtomicBool::new(false),
            buffered_ms_x10: AtomicU32::new(0),
            target_frames: AtomicU32::new(0),
            packets: AtomicU64::new(0),
            ring: JitterRing::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.load(Ordering::Acquire) == 1
    }
}

pub(crate) struct PeerTable {
    slots: Box<[PeerSlot]>,
}

impl PeerTable {
    pub fn new() -> Self {
        Self {
            slots: (0..MAX_PEERS).map(|_| PeerSlot::new()).collect(),
        }
    }

    pub fn slots(&self) -> &[PeerSlot] {
        &self.slots
    }

    pub fn find(&self, sender: u64) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.is_active() && s.sender.load(Ordering::Relaxed) == sender)
    }

    /// Network thread only. Returns the slot index, allocating a free slot
    /// for a new sender; `None` when the table is full.
    pub fn find_or_insert(&self, sender: u64, now_ms: u64) -> Option<usize> {
        if let Some(i) = self.find(sender) {
            return Some(i);
        }
        let i = self.slots.iter().position(|s| !s.is_active())?;
        let slot = &self.slots[i];
        slot.ring.reset();
        slot.sender.store(sender, Ordering::Relaxed);
        slot.addr.store(0, Ordering::Relaxed);
        slot.last_seen_ms.store(now_ms, Ordering::Relaxed);
        slot.channel.store(0, Ordering::Relaxed);
        slot.gain.store(1.0f32.to_bits(), Ordering::Relaxed);
        slot.muted.store(false, Ordering::Relaxed);
        slot.talking.store(false, Ordering::Relaxed);
        slot.buffered_ms_x10.store(0, Ordering::Relaxed);
        slot.target_frames.store(0, Ordering::Relaxed);
        slot.packets.store(0, Ordering::Relaxed);
        slot.generation.fetch_add(1, Ordering::AcqRel);
        slot.state.store(1, Ordering::Release);
        Some(i)
    }

    pub fn remove(&self, sender: u64) {
        if let Some(i) = self.find(sender) {
            self.slots[i].state.store(0, Ordering::Release);
        }
    }

    /// Free every peer not heard from within `timeout_ms`. Returns how many.
    pub fn expire(&self, now_ms: u64, timeout_ms: u64) -> usize {
        let mut n = 0;
        for slot in self.slots.iter() {
            if slot.is_active()
                && now_ms.saturating_sub(slot.last_seen_ms.load(Ordering::Relaxed)) > timeout_ms
            {
                slot.state.store(0, Ordering::Release);
                n += 1;
            }
        }
        n
    }

    pub fn active_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_active()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_pack_and_unpack() {
        let a: SocketAddr = "10.0.0.184:41531".parse().unwrap();
        assert_eq!(unpack_addr(pack_addr(a)), Some(a));
        let v6: SocketAddr = "[::1]:41531".parse().unwrap();
        assert_eq!(pack_addr(v6), 0);
        assert_eq!(unpack_addr(0), None);
    }

    #[test]
    fn allocation_reuse_and_expiry() {
        let t = PeerTable::new();
        assert_eq!(t.find(7), None);
        let i = t.find_or_insert(7, 100).unwrap();
        assert_eq!(t.find(7), Some(i));
        assert_eq!(t.find_or_insert(7, 200), Some(i));
        let gen1 = t.slots()[i].generation.load(Ordering::Relaxed);
        assert_eq!(t.active_count(), 1);
        // A second sender gets a different slot.
        let j = t.find_or_insert(8, 200).unwrap();
        assert_ne!(i, j);
        // Expiry frees only the stale one.
        t.slots()[i].last_seen_ms.store(200, Ordering::Relaxed);
        t.slots()[j].last_seen_ms.store(4000, Ordering::Relaxed);
        assert_eq!(t.expire(4300, 3000), 1);
        assert_eq!(t.find(7), None);
        assert_eq!(t.find(8), Some(j));
        // Reuse bumps the generation so the audio side resets.
        let k = t.find_or_insert(9, 4300).unwrap();
        assert_eq!(k, i);
        assert!(t.slots()[k].generation.load(Ordering::Relaxed) > gen1);
    }

    #[test]
    fn a_full_table_refuses_new_senders() {
        let t = PeerTable::new();
        for s in 0..MAX_PEERS as u64 {
            assert!(t.find_or_insert(1000 + s, 0).is_some());
        }
        assert_eq!(t.find_or_insert(5000, 0), None);
        // But existing senders still resolve.
        assert!(t.find_or_insert(1003, 0).is_some());
    }

    #[test]
    fn remove_frees_the_slot() {
        let t = PeerTable::new();
        t.find_or_insert(42, 0).unwrap();
        t.remove(42);
        assert_eq!(t.find(42), None);
        assert_eq!(t.active_count(), 0);
    }
}
