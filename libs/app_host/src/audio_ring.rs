//! Sound from a child to its host through shared memory: a lock-free
//! single-producer single-consumer ring of records (stream starts, packets
//! of interleaved f32, stops), each stamped on the same clock as the
//! child's frames ([`crate::frame_ring`]). The host creates the ring and
//! sends its name once ([`AudioRingSetup`]); the child writes, the host
//! reads on its beat. A full ring refuses a record: the child keeps it and
//! writes it later, nothing is dropped on the way.

use std::io;
use std::sync::atomic::Ordering;

use makepad_platform::makepad_micro_serde::*;

use crate::shm::{self, SharedMem};

const MAGIC: u32 = 0x4d50_4152; // "MPAR"
const VERSION: u32 = 1;
const HEADER: usize = 64;
const H_CAPACITY: usize = 8;
const H_WRITE: usize = 16;
const H_READ: usize = 24;
const H_CLOSED: usize = 32;

const PAD: u32 = 0;
const STARTED: u32 = 1;
const PACKET: u32 = 2;
const STOPPED: u32 = 3;
const ERROR: u32 = 4;

/// One captured packet: `frames * channels` interleaved samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioRingPacket {
    /// The stream it belongs to (a source that goes quiet and sounds again
    /// starts the next one).
    pub epoch: u32,
    pub channels: u32,
    pub frames: u32,
    /// Presentation time, Unix milliseconds.
    pub pts_ms: i64,
    /// When the producer got it: Unix milliseconds and its monotonic
    /// nanoseconds (the clock of its frames).
    pub unix_ms: i64,
    pub elapsed_ns: u64,
    /// The producer's content epoch when the stream started.
    pub content_epoch: u64,
    pub samples: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioRecord {
    Started { epoch: u32, sample_rate: u32, channels: u32, frames_per_buffer: u32 },
    Packet(AudioRingPacket),
    Stopped,
    Error(String),
}

impl AudioRecord {
    fn encode(&self, out: &mut Vec<u8>) {
        out.clear();
        let put32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        put32(out, 0); // length, below
        match self {
            AudioRecord::Started { epoch, sample_rate, channels, frames_per_buffer } => {
                put32(out, STARTED);
                for v in [*epoch, *sample_rate, *channels, *frames_per_buffer] {
                    put32(out, v);
                }
            }
            AudioRecord::Packet(p) => {
                put32(out, PACKET);
                for v in [p.epoch, p.channels, p.frames, 0] {
                    put32(out, v);
                }
                for v in [p.pts_ms as u64, p.unix_ms as u64, p.elapsed_ns, p.content_epoch] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                for s in &p.samples {
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
            AudioRecord::Stopped => put32(out, STOPPED),
            AudioRecord::Error(text) => {
                put32(out, ERROR);
                out.extend_from_slice(text.as_bytes());
            }
        }
        let len = out.len();
        out[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        out.resize(len.div_ceil(8) * 8, 0);
    }

    fn decode(record: &[u8]) -> Option<AudioRecord> {
        let u32_at = |at: usize| record.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let u64_at = |at: usize| record.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        let len = u32_at(0)? as usize;
        let record = record.get(..len)?;
        Some(match u32_at(4)? {
            STARTED => AudioRecord::Started { epoch: u32_at(8)?, sample_rate: u32_at(12)?, channels: u32_at(16)?, frames_per_buffer: u32_at(20)? },
            PACKET => {
                let (epoch, channels, frames) = (u32_at(8)?, u32_at(12)?, u32_at(16)?);
                let samples = record.get(56..)?.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>();
                if samples.len() != (frames * channels) as usize {
                    return None;
                }
                AudioRecord::Packet(AudioRingPacket {
                    epoch,
                    channels,
                    frames,
                    pts_ms: u64_at(24)? as i64,
                    unix_ms: u64_at(32)? as i64,
                    elapsed_ns: u64_at(40)?,
                    content_epoch: u64_at(48)?,
                    samples,
                })
            }
            STOPPED => AudioRecord::Stopped,
            ERROR => AudioRecord::Error(String::from_utf8_lossy(record.get(8..)?).into_owned()),
            _ => return None,
        })
    }
}

/// The setup a host sends its child.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AudioRingSetup {
    /// The owner's id for the ring (a browser view's id).
    pub ring: u64,
    pub shm: String,
    pub len: u64,
}

#[derive(SerJson, DeJson)]
struct SetupEnvelope {
    makepad_audio_ring: AudioRingSetup,
}

impl AudioRingSetup {
    pub fn to_json(&self) -> String {
        SetupEnvelope { makepad_audio_ring: self.clone() }.serialize_json()
    }

    pub fn parse(json: &str) -> Option<AudioRingSetup> {
        if !json.contains("\"makepad_audio_ring\"") {
            return None;
        }
        SetupEnvelope::deserialize_json(json).ok().map(|e| e.makepad_audio_ring)
    }
}

fn capacity(mem: &SharedMem) -> u64 {
    mem.atomic_u64(H_CAPACITY).load(Ordering::Acquire)
}

/// The host's (reading) side.
pub struct HostAudioRing {
    mem: SharedMem,
    setup: AudioRingSetup,
}

impl HostAudioRing {
    /// A ring of `capacity` bytes (a second of 48 kHz stereo is 384 KB).
    pub fn new(ring: u64, capacity: usize) -> io::Result<HostAudioRing> {
        let capacity = capacity.div_ceil(8) * 8;
        let path = shm::file_path(&shm::unique_name('a'));
        let len = HEADER + capacity;
        let mem = SharedMem::create(&path, len)?;
        mem.atomic_u32(0).store(MAGIC, Ordering::Relaxed);
        mem.atomic_u32(4).store(VERSION, Ordering::Relaxed);
        mem.atomic_u64(H_CAPACITY).store(capacity as u64, Ordering::Release);
        let setup = AudioRingSetup { ring, shm: path.display().to_string(), len: len as u64 };
        Ok(HostAudioRing { mem, setup })
    }

    pub fn setup(&self) -> &AudioRingSetup {
        &self.setup
    }

    /// Every record written since the last call, oldest first.
    pub fn poll(&mut self) -> Vec<AudioRecord> {
        let mem = &self.mem;
        let cap = capacity(mem);
        let write = mem.atomic_u64(H_WRITE).load(Ordering::Acquire);
        let mut read = mem.atomic_u64(H_READ).load(Ordering::Relaxed);
        let mut out = Vec::new();
        while read < write {
            let pos = (read % cap) as usize;
            let head = unsafe { mem.bytes(HEADER + pos, 8) };
            let len = u32::from_le_bytes(head[0..4].try_into().unwrap()) as usize;
            let kind = u32::from_le_bytes(head[4..8].try_into().unwrap());
            let span = len.div_ceil(8) * 8;
            if span == 0 || pos + span > cap as usize {
                // A corrupt record: nothing after it can be trusted.
                read = write;
                break;
            }
            if kind != PAD {
                if let Some(record) = AudioRecord::decode(unsafe { mem.bytes(HEADER + pos, span) }) {
                    out.push(record);
                }
            }
            read += span as u64;
        }
        mem.atomic_u64(H_READ).store(read, Ordering::Release);
        out
    }
}

impl Drop for HostAudioRing {
    fn drop(&mut self) {
        self.mem.atomic_u32(H_CLOSED).store(1, Ordering::Release);
    }
}

/// The child's (writing) side.
pub struct ProducerAudioRing {
    mem: SharedMem,
    scratch: Vec<u8>,
    pub setup: AudioRingSetup,
}

impl ProducerAudioRing {
    pub fn open(setup: AudioRingSetup) -> io::Result<ProducerAudioRing> {
        let mem = SharedMem::open(std::path::Path::new(&setup.shm), setup.len as usize)?;
        if mem.atomic_u32(0).load(Ordering::Acquire) != MAGIC || mem.atomic_u32(4).load(Ordering::Acquire) != VERSION {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "audio ring: not a ring of this version"));
        }
        if HEADER as u64 + capacity(&mem) > mem.len() as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "audio ring: capacity exceeds the block"));
        }
        Ok(ProducerAudioRing { mem, scratch: Vec::new(), setup })
    }

    pub fn closed(&self) -> bool {
        self.mem.atomic_u32(H_CLOSED).load(Ordering::Acquire) != 0
    }

    /// Write one record; false when the ring has no room for it now (keep
    /// it and try again) or it can never fit.
    pub fn write(&mut self, record: &AudioRecord) -> bool {
        record.encode(&mut self.scratch);
        let mem = &self.mem;
        let cap = capacity(mem);
        let need = self.scratch.len() as u64;
        if need > cap {
            return false;
        }
        let mut write = mem.atomic_u64(H_WRITE).load(Ordering::Relaxed);
        let read = mem.atomic_u64(H_READ).load(Ordering::Acquire);
        let pos = write % cap;
        let tail = cap - pos;
        let pad = if need > tail { tail } else { 0 };
        if cap - (write - read) < pad + need {
            return false;
        }
        if pad > 0 {
            let head = unsafe { mem.bytes(HEADER + pos as usize, 8) };
            head[0..4].copy_from_slice(&(pad as u32).to_le_bytes());
            head[4..8].copy_from_slice(&PAD.to_le_bytes());
            write += pad;
        }
        let pos = (write % cap) as usize;
        unsafe { mem.bytes(HEADER + pos, self.scratch.len()) }.copy_from_slice(&self.scratch);
        mem.atomic_u64(H_WRITE).store(write + need, Ordering::Release);
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn packet(n: u32) -> AudioRecord {
        AudioRecord::Packet(AudioRingPacket {
            epoch: 1,
            channels: 2,
            frames: 480,
            pts_ms: n as i64 * 10,
            unix_ms: 7,
            elapsed_ns: 8,
            content_epoch: 9,
            samples: (0..960).map(|i| (n * 1000 + i) as f32).collect(),
        })
    }

    #[test]
    fn records_cross_in_order_through_wraps() {
        let mut host = HostAudioRing::new(1, 16 * 1024).unwrap();
        let mut producer = ProducerAudioRing::open(host.setup().clone()).unwrap();
        assert!(producer.write(&AudioRecord::Started { epoch: 1, sample_rate: 48000, channels: 2, frames_per_buffer: 480 }));
        let mut got = host.poll();
        let mut written = 0;
        for n in 0..200 {
            while !producer.write(&packet(n)) {
                got.extend(host.poll());
            }
            written += 1;
        }
        while !producer.write(&AudioRecord::Stopped) {
            got.extend(host.poll());
        }
        got.extend(host.poll());
        assert_eq!(got.len(), written + 2);
        assert!(matches!(got[0], AudioRecord::Started { sample_rate: 48000, .. }));
        for (n, record) in got[1..=written].iter().enumerate() {
            assert_eq!(*record, packet(n as u32));
        }
        assert_eq!(got.last(), Some(&AudioRecord::Stopped));
    }

    #[test]
    fn a_full_ring_refuses_and_keeps_order() {
        let mut host = HostAudioRing::new(1, 6000).unwrap();
        let mut producer = ProducerAudioRing::open(host.setup().clone()).unwrap();
        assert!(producer.write(&packet(0)));
        assert!(!producer.write(&packet(1)), "the second packet does not fit yet");
        assert_eq!(host.poll(), vec![packet(0)]);
        assert!(producer.write(&packet(1)));
        assert_eq!(host.poll(), vec![packet(1)]);
    }
}
