//! A ring of shared GPU textures a child fills and its host reads: frames
//! from another process with no CPU copy, each with its own timestamp.
//!
//! The host owns the textures (`Cx::create_shared_texture`) and a small
//! shared-memory control block; it sends their handles and the block's
//! names once ([`RingSetup`], over the hosted-app channel). The child
//! ([`ProducerRing`]) takes a free slot, writes a frame into its texture on
//! the GPU, stamps it and marks it ready; the host ([`HostRing::poll`])
//! takes ready slots and gives each back when it is done with it.
//!
//! Two modes, switched by the host at any time:
//! - display (latest wins): the child never waits; when no slot is free it
//!   reuses the oldest ready one the host has not taken, else drops the
//!   frame. The host shows the newest and skips the rest.
//! - capture (lossless): every frame reaches the host in order. A child with
//!   no free slot waits until the host gives one back (back-pressure: the
//!   page slows down instead of losing frames).
//!
//! Slot states live in the shared block (free, writing, ready, held); the
//! child waits on the host's `free` semaphore and the host's waiter thread
//! wakes the UI on the child's `ready` one. Nothing polls on a timer and no
//! wait has a timeout: closing the ring posts both.
//!
//! A slot's handle names its texture in the process holding it: an IOSurface
//! id on macOS (the same in both), a named NT handle on Windows (the same in
//! both), on Linux a DMA-BUF the host passes over a unix socket of its own
//! (`RingSetup::fds`, one descriptor per slot) and each side registers with
//! `makepad_hal::dmabuf::copy` under a handle of its own.

use std::io;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use makepad_platform::makepad_micro_serde::*;
use makepad_platform::thread::{SignalToUI, ThreadOptions};
use makepad_platform::*;

use crate::shm::{self, Semaphore, SharedMem};

const MAGIC: u32 = 0x4d50_5247; // "MPRG"
const VERSION: u32 = 1;
const HEADER: usize = 64;
const H_SLOTS: usize = 8;
const H_PROBES: usize = 12;
const H_CAPTURE: usize = 16;
const H_CLOSED: usize = 20;
const H_SEQ: usize = 24;
const H_DROPPED: usize = 32;
const S_STATE: usize = 0;
const S_WIDTH: usize = 4;
const S_SEQ: usize = 8;
const S_HEIGHT: usize = 16;
const S_UNIX_MS: usize = 24;
const S_ELAPSED_NS: usize = 32;
const S_EPOCH: usize = 40;
const S_PROBES: usize = 48;

const FREE: u32 = 0;
const WRITING: u32 = 1;
const READY: u32 = 2;
const HELD: u32 = 3;

fn stride(probes: usize) -> usize {
    (S_PROBES + probes).div_ceil(8) * 8
}

/// What a host asks of a ring.
#[derive(Clone, Debug, PartialEq)]
pub struct RingSpec {
    /// Slots: 3 is enough to display; capture wants headroom for the
    /// consumer's latency (each slot is one texture of `width` x `height`).
    pub slots: usize,
    pub width: u32,
    pub height: u32,
    pub capture: bool,
    /// Pixels the child reads back with every frame (`[x, y]` in the frame),
    /// one luminance byte each: a few hundred bytes of identity (a frame
    /// counter strip), never the picture.
    pub probes: Vec<[u32; 2]>,
}

/// One slot's texture as the child sees it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct RingSlot {
    /// The platform's shared-texture handle (see the module docs).
    pub handle: u64,
    pub width: u32,
    pub height: u32,
    /// Linux: the slot's DMA-BUF layout (`makepad_hal::dmabuf::copy::Layout`
    /// words); empty elsewhere.
    pub layout: Vec<u64>,
}

/// The setup a host sends its child, over the custom channel.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct RingSetup {
    /// The owner's id for the ring (a browser view's id).
    pub ring: u64,
    pub generation: u64,
    pub shm: String,
    pub len: u64,
    pub free_sem: String,
    pub ready_sem: String,
    pub slots: Vec<RingSlot>,
    /// `x, y` pairs.
    pub probes: Vec<u32>,
    /// Linux: the unix socket the host hands the slots' DMA-BUFs over (one
    /// per slot, in order, to the first connection); empty elsewhere.
    pub fds: String,
}

#[derive(SerJson, DeJson)]
struct SetupEnvelope {
    makepad_ring: RingSetup,
}

impl RingSetup {
    pub fn to_json(&self) -> String {
        SetupEnvelope { makepad_ring: self.clone() }.serialize_json()
    }

    pub fn parse(json: &str) -> Option<RingSetup> {
        if !json.contains("\"makepad_ring\"") {
            return None;
        }
        SetupEnvelope::deserialize_json(json).ok().map(|e| e.makepad_ring)
    }
}

/// What a filled slot says about its frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RingFrameInfo {
    /// The producer's frame number in this ring, from 1, without gaps in
    /// capture mode.
    pub seq: u64,
    /// The part of the slot holding the frame, from its top-left.
    pub width: u32,
    pub height: u32,
    /// When the producer got the frame: Unix milliseconds (the clock its
    /// audio packets are stamped on) and its monotonic nanoseconds.
    pub unix_ms: i64,
    pub elapsed_ns: u64,
    /// The producer's content epoch (a page's committed navigation).
    pub epoch: u64,
    pub probes: Vec<u8>,
}

/// The block both sides map, and its semaphores.
struct RingShared {
    mem: SharedMem,
    free: Semaphore,
    ready: Semaphore,
    slots: usize,
    probes: usize,
    /// Host only: who holds each slot (the display, leases).
    holds: Vec<AtomicU32>,
    leases: AtomicUsize,
    /// Linux: the slots' DMA-BUFs as this process registered them.
    #[cfg(target_os = "linux")]
    local: Vec<u64>,
}

#[cfg(target_os = "linux")]
impl Drop for RingShared {
    fn drop(&mut self) {
        for handle in &self.local {
            makepad_platform::makepad_hal::dmabuf::hal_dmabuf_release(*handle);
        }
    }
}

impl RingShared {
    fn state(&self, slot: usize) -> &AtomicU32 {
        self.mem.atomic_u32(HEADER + slot * stride(self.probes) + S_STATE)
    }

    fn info(&self, slot: usize) -> RingFrameInfo {
        let base = HEADER + slot * stride(self.probes);
        let read_u32 = |at: usize| self.mem.atomic_u32(base + at).load(Ordering::Relaxed);
        let read_u64 = |at: usize| self.mem.atomic_u64(base + at).load(Ordering::Relaxed);
        RingFrameInfo {
            seq: read_u64(S_SEQ),
            width: read_u32(S_WIDTH),
            height: read_u32(S_HEIGHT),
            unix_ms: read_u64(S_UNIX_MS) as i64,
            elapsed_ns: read_u64(S_ELAPSED_NS),
            epoch: read_u64(S_EPOCH),
            probes: unsafe { self.mem.bytes(base + S_PROBES, self.probes) }.to_vec(),
        }
    }

    fn closed(&self) -> bool {
        self.mem.atomic_u32(H_CLOSED).load(Ordering::Acquire) != 0
    }

    fn capture(&self) -> bool {
        self.mem.atomic_u32(H_CAPTURE).load(Ordering::Acquire) != 0
    }

    fn hold(&self, slot: usize) {
        self.holds[slot].fetch_add(1, Ordering::AcqRel);
    }

    /// The last holder gives the slot back to the producer.
    fn unhold(&self, slot: usize) {
        if self.holds[slot].fetch_sub(1, Ordering::AcqRel) == 1 {
            self.state(slot).store(FREE, Ordering::Release);
            self.free.post();
        }
    }
}

/// A captured frame the consumer holds; dropping it gives the slot back.
/// Send: a recorder worker encodes from the slot's shared handle and drops it.
pub struct RingLease {
    shared: Arc<RingShared>,
    slot: usize,
    /// The slot's shared-texture handle (what an encoder reads).
    pub handle: u64,
    pub slot_width: u32,
    pub slot_height: u32,
    pub info: RingFrameInfo,
}

impl RingLease {
    pub fn slot(&self) -> usize {
        self.slot
    }

    /// The frame's pixels on the CPU (BGRA words, `info.width` x
    /// `info.height`): one read for a picture someone asked for (a still, a
    /// check), never for every frame. None where the platform cannot map a
    /// shared texture yet.
    pub fn read_bgra(&self) -> Option<Vec<u32>> {
        read_shared_bgra(self.handle, self.info.width as usize, self.info.height as usize)
    }
}

/// The top-left `width` x `height` BGRA pixels of a shared texture.
#[cfg(target_os = "macos")]
pub fn read_shared_bgra(handle: u64, width: usize, height: usize) -> Option<Vec<u32>> {
    use std::os::raw::c_void;
    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
        fn IOSurfaceLookup(id: u32) -> *mut c_void;
        fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
        fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
        fn IOSurfaceLock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceUnlock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceGetBaseAddress(surface: *mut c_void) -> *mut c_void;
        fn IOSurfaceGetBytesPerRow(surface: *mut c_void) -> usize;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    const READ_ONLY: u32 = 1;
    unsafe {
        let surface = IOSurfaceLookup(u32::try_from(handle).ok()?);
        if surface.is_null() {
            return None;
        }
        let (w, h) = (width.min(IOSurfaceGetWidth(surface)), height.min(IOSurfaceGetHeight(surface)));
        let mut out = None;
        if IOSurfaceLock(surface, READ_ONLY, std::ptr::null_mut()) == 0 {
            let base = IOSurfaceGetBaseAddress(surface) as *const u8;
            let row = IOSurfaceGetBytesPerRow(surface);
            if !base.is_null() {
                let mut pixels = Vec::with_capacity(w * h);
                for y in 0..h {
                    let line = std::slice::from_raw_parts(base.add(y * row) as *const u32, w);
                    pixels.extend_from_slice(line);
                }
                out = Some(pixels);
            }
            IOSurfaceUnlock(surface, READ_ONLY, std::ptr::null_mut());
        }
        CFRelease(surface as *const c_void);
        out
    }
}

/// Linux: `handle` is a DMA-BUF this process registered (a ring slot).
#[cfg(target_os = "linux")]
pub fn read_shared_bgra(handle: u64, width: usize, height: usize) -> Option<Vec<u32>> {
    let mut bytes = vec![0u8; width * height * 4];
    let region = [0, 0, width as u32, height as u32];
    let read = makepad_platform::makepad_hal::dmabuf::hal_dmabuf_read(handle, &region, &mut bytes);
    if read != bytes.len() as i64 {
        return None;
    }
    Some(bytes.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect())
}

/// Windows: `handle` is the slot's shared-texture key (its NT name).
#[cfg(windows)]
pub fn read_shared_bgra(handle: u64, width: usize, height: usize) -> Option<Vec<u32>> {
    let mut bytes = vec![0u8; width * height * 4];
    let region = [0, 0, width as u32, height as u32];
    let read = makepad_platform::makepad_hal::shared_texture::hal_shared_read(handle, &region, &mut bytes);
    if read != bytes.len() as i64 {
        return None;
    }
    Some(bytes.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn read_shared_bgra(_handle: u64, _width: usize, _height: usize) -> Option<Vec<u32>> {
    None
}

impl Drop for RingLease {
    fn drop(&mut self) {
        self.shared.leases.fetch_sub(1, Ordering::AcqRel);
        self.shared.unhold(self.slot);
    }
}

/// The frame a host shows.
#[derive(Clone)]
pub struct RingShown {
    pub texture: Texture,
    pub info: RingFrameInfo,
    /// The slot's allocation; `info.width/height` of it hold the frame.
    pub alloc: [u32; 2],
}

/// The host's side of a ring.
pub struct HostRing {
    id: u64,
    generation: u64,
    spec: RingSpec,
    textures: Vec<Texture>,
    shared: Option<Arc<RingShared>>,
    setup: Option<RingSetup>,
    setup_taken: bool,
    shown: Option<usize>,
    previous: Option<usize>,
    error: Option<String>,
    /// Linux: the socket the slots' DMA-BUFs go out on.
    fds: String,
    /// Capture: the frame number the consumer takes next (0: any).
    next_seq: u64,
}

/// Cut a sorted list of ready `(seq, slot)` at its first gap after
/// `next_seq` (a frame its producer has numbered but not yet marked ready).
fn keep_in_order(ready: &mut Vec<(u64, usize)>, next_seq: &mut u64) {
    let mut expected = *next_seq;
    let mut keep = 0;
    for (seq, _) in ready.iter() {
        if expected != 0 && *seq != expected {
            break;
        }
        expected = seq + 1;
        keep += 1;
    }
    ready.truncate(keep);
    *next_seq = expected;
}

impl HostRing {
    /// Allocate the slots' textures. The handles arrive asynchronously; the
    /// setup is ready to send once they all have (see `take_setup`).
    pub fn new(cx: &mut Cx, id: u64, generation: u64, spec: RingSpec) -> HostRing {
        let textures = (0..spec.slots.max(2)).map(|_| cx.create_shared_texture(spec.width as usize, spec.height as usize)).collect();
        HostRing { id, generation, spec, textures, shared: None, setup: None, setup_taken: false, shown: None, previous: None, error: None, fds: String::new(), next_seq: 0 }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn spec(&self) -> &RingSpec {
        &self.spec
    }

    /// Why the ring cannot run, if it cannot.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The setup to send the child, once (None until the handles are known
    /// and after it was taken).
    pub fn take_setup(&mut self, cx: &mut Cx) -> Option<String> {
        if self.setup_taken || self.error.is_some() {
            return None;
        }
        if self.setup.is_none() {
            let mut slots = Vec::with_capacity(self.textures.len());
            for texture in &self.textures {
                match cx.shared_texture_handle(texture) {
                    Some(handle) if handle != 0 => slots.push(RingSlot { handle, width: self.spec.width, height: self.spec.height, layout: Vec::new() }),
                    _ => return None,
                }
            }
            #[cfg(target_os = "linux")]
            let exports = match self.textures.iter().map(|texture| cx.shared_texture_dmabuf(texture)).collect::<Result<Vec<_>, _>>() {
                Ok(exports) => exports,
                Err(error) => {
                    log!("frame ring {}: {error}", self.id);
                    self.error = Some(error);
                    return None;
                }
            };
            #[cfg(target_os = "linux")]
            let slots = match linux::local_slots(slots, &exports) {
                Ok(slots) => slots,
                Err(error) => {
                    log!("frame ring {}: {error}", self.id);
                    self.error = Some(error);
                    return None;
                }
            };
            #[cfg(target_os = "linux")]
            let slots = match linux::serve_fds(cx, self.id, exports) {
                Ok(path) => {
                    self.fds = path;
                    slots
                }
                Err(error) => {
                    for slot in &slots {
                        makepad_platform::makepad_hal::dmabuf::hal_dmabuf_release(slot.handle);
                    }
                    log!("frame ring {}: {error}", self.id);
                    self.error = Some(error.to_string());
                    return None;
                }
            };
            match self.open_shared(slots) {
                Ok(setup) => self.setup = Some(setup),
                Err(error) => {
                    log!("frame ring {}: {error}", self.id);
                    self.error = Some(error.to_string());
                    return None;
                }
            }
            self.start_waiter(cx);
        }
        self.setup_taken = true;
        self.setup.as_ref().map(RingSetup::to_json)
    }

    fn open_shared(&mut self, slots: Vec<RingSlot>) -> io::Result<RingSetup> {
        let name = shm::unique_name('r');
        let path = shm::file_path(&name);
        let probes = self.spec.probes.len();
        let len = HEADER + slots.len() * stride(probes);
        let mem = SharedMem::create(&path, len)?;
        let free = Semaphore::create(&shm::unique_name('f'))?;
        let ready = Semaphore::create(&shm::unique_name('w'))?;
        mem.atomic_u32(0).store(MAGIC, Ordering::Relaxed);
        mem.atomic_u32(4).store(VERSION, Ordering::Relaxed);
        mem.atomic_u32(H_SLOTS).store(slots.len() as u32, Ordering::Relaxed);
        mem.atomic_u32(H_PROBES).store(probes as u32, Ordering::Relaxed);
        mem.atomic_u32(H_CAPTURE).store(self.spec.capture as u32, Ordering::Release);
        let setup = RingSetup {
            ring: self.id,
            generation: self.generation,
            shm: path.display().to_string(),
            len: len as u64,
            free_sem: free.name().to_string(),
            ready_sem: ready.name().to_string(),
            slots: slots.clone(),
            probes: self.spec.probes.iter().flat_map(|p| [p[0], p[1]]).collect(),
            fds: self.fds.clone(),
        };
        self.shared = Some(Arc::new(RingShared {
            mem,
            free,
            ready,
            slots: slots.len(),
            probes,
            holds: (0..slots.len()).map(|_| AtomicU32::new(0)).collect(),
            leases: AtomicUsize::new(0),
            #[cfg(target_os = "linux")]
            local: slots.iter().map(|slot| slot.handle).collect(),
        }));
        Ok(setup)
    }

    /// The thread that sleeps on the child's `ready` posts and wakes the UI.
    fn start_waiter(&self, cx: &mut Cx) {
        let Some(shared) = self.shared.clone() else { return };
        let started = cx.thread_spawner().spawn_worker(ThreadOptions { name: Some(format!("frame-ring-{}", self.id).into()), ..Default::default() }, move || loop {
            shared.ready.wait();
            if shared.closed() {
                return;
            }
            SignalToUI::set_ui_signal();
        });
        if let Ok(worker) = started {
            worker.detach();
        }
    }

    /// Switch between display and capture (see the module docs).
    pub fn set_capture(&mut self, on: bool) {
        self.spec.capture = on;
        self.next_seq = 0;
        if let Some(shared) = &self.shared {
            shared.mem.atomic_u32(H_CAPTURE).store(on as u32, Ordering::Release);
            // A producer waiting for a slot in capture mode re-checks.
            shared.free.post();
        }
    }

    /// Take what the child made ready. Updates the shown frame (the newest);
    /// in capture mode returns every new frame, oldest first, each to be
    /// dropped when consumed. True in the second value when the shown frame
    /// changed.
    pub fn poll(&mut self) -> (Vec<RingLease>, bool) {
        let Some(shared) = self.shared.clone() else { return (Vec::new(), false) };
        let mut ready: Vec<(u64, usize)> = (0..shared.slots)
            .filter(|slot| shared.state(*slot).load(Ordering::Acquire) == READY)
            .map(|slot| (shared.info(slot).seq, slot))
            .collect();
        ready.sort_unstable();
        let capture = self.spec.capture;
        // Capture hands frames over in order: a slot scanned before its
        // producer marked it ready while a later one was already marked comes
        // with the next poll, ahead of everything after it.
        if capture {
            keep_in_order(&mut ready, &mut self.next_seq);
        }
        let mut leases = Vec::new();
        let mut newest = None;
        let count = ready.len();
        for (index, (_, slot)) in ready.into_iter().enumerate() {
            let last = index + 1 == count;
            if !capture && !last {
                // Display: a frame a newer one replaces is given back unseen.
                if shared.state(slot).compare_exchange(READY, FREE, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    shared.free.post();
                }
                continue;
            }
            if shared.state(slot).compare_exchange(READY, HELD, Ordering::AcqRel, Ordering::Acquire).is_err() {
                // The producer took it back (display mode).
                continue;
            }
            if capture {
                shared.hold(slot);
                shared.leases.fetch_add(1, Ordering::AcqRel);
                leases.push(RingLease {
                    shared: shared.clone(),
                    slot,
                    handle: 0,
                    slot_width: self.spec.width,
                    slot_height: self.spec.height,
                    info: shared.info(slot),
                });
            }
            // An older captured frame is held by its lease alone.
            if last {
                newest = Some(slot);
            }
        }
        if let Some(setup) = &self.setup {
            for lease in &mut leases {
                lease.handle = setup.slots[lease.slot].handle;
            }
        }
        let changed = match newest {
            Some(slot) => {
                shared.hold(slot);
                // The previous frame may still be read by a frame in flight
                // on the GPU: it goes back one frame later.
                if let Some(old) = self.previous.take() {
                    shared.unhold(old);
                }
                self.previous = self.shown.replace(slot);
                true
            }
            None => false,
        };
        (leases, changed)
    }

    /// The newest frame (display).
    pub fn shown(&self) -> Option<RingShown> {
        let slot = self.shown?;
        let shared = self.shared.as_ref()?;
        Some(RingShown { texture: self.textures[slot].clone(), info: shared.info(slot), alloc: [self.spec.width, self.spec.height] })
    }

    /// Hold the shown frame as a lease (display mode too): its slot is not
    /// written again until the lease is dropped.
    pub fn hold_shown(&self) -> Option<RingLease> {
        let slot = self.shown?;
        let shared = self.shared.clone()?;
        shared.hold(slot);
        shared.leases.fetch_add(1, Ordering::AcqRel);
        let handle = self.setup.as_ref()?.slots[slot].handle;
        let info = shared.info(slot);
        Some(RingLease { shared, slot, handle, slot_width: self.spec.width, slot_height: self.spec.height, info })
    }

    /// Captured frames still held by consumers.
    pub fn leases(&self) -> usize {
        self.shared.as_ref().map_or(0, |shared| shared.leases.load(Ordering::Acquire))
    }

    /// Frames the producer dropped for want of a slot (display mode).
    pub fn dropped(&self) -> u64 {
        self.shared.as_ref().map_or(0, |shared| shared.mem.atomic_u64(H_DROPPED).load(Ordering::Relaxed))
    }
}

impl HostRing {
    /// End the producer's side: it takes no more slots (a waiting one wakes
    /// and gives up). Frames already taken stay valid while their leases
    /// and this ring live; a replaced ring is closed and kept until then.
    pub fn close(&mut self) {
        if let Some(shared) = &self.shared {
            if shared.mem.atomic_u32(H_CLOSED).swap(1, Ordering::AcqRel) == 0 {
                // Wake a producer waiting for a slot and the waiter thread.
                shared.free.post();
                shared.ready.post();
            }
        }
        self.setup_taken = true;
    }
}

impl Drop for HostRing {
    fn drop(&mut self) {
        self.close();
    }
}

/// The child's side of a ring.
pub struct ProducerRing {
    shared: RingShared,
    pub setup: RingSetup,
}

/// Windows: the slots this process opened by name are let go with the ring.
#[cfg(windows)]
impl Drop for ProducerRing {
    fn drop(&mut self) {
        for slot in &self.setup.slots {
            makepad_platform::makepad_hal::shared_texture::release(slot.handle);
        }
    }
}

impl ProducerRing {
    pub fn open(setup: RingSetup) -> io::Result<ProducerRing> {
        let mem = SharedMem::open(std::path::Path::new(&setup.shm), setup.len as usize)?;
        if mem.atomic_u32(0).load(Ordering::Acquire) != MAGIC || mem.atomic_u32(4).load(Ordering::Acquire) != VERSION {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame ring: not a ring of this version"));
        }
        let slots = mem.atomic_u32(H_SLOTS).load(Ordering::Acquire) as usize;
        let probes = mem.atomic_u32(H_PROBES).load(Ordering::Acquire) as usize;
        if slots != setup.slots.len() || probes * 2 != setup.probes.len() || HEADER + slots * stride(probes) > mem.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame ring: setup and block disagree"));
        }
        let free = Semaphore::open(&setup.free_sem)?;
        let ready = Semaphore::open(&setup.ready_sem)?;
        #[allow(unused_mut)]
        let mut setup = setup;
        #[cfg(target_os = "linux")]
        let local = linux::receive_slots(&mut setup)?;
        Ok(ProducerRing {
            shared: RingShared {
                mem,
                free,
                ready,
                slots,
                probes,
                holds: Vec::new(),
                leases: AtomicUsize::new(0),
                #[cfg(target_os = "linux")]
                local,
            },
            setup,
        })
    }

    pub fn probes(&self) -> impl Iterator<Item = [u32; 2]> + '_ {
        self.setup.probes.chunks_exact(2).map(|p| [p[0], p[1]])
    }

    pub fn closed(&self) -> bool {
        self.shared.closed()
    }

    /// A slot to write the next frame into. In capture mode this waits for
    /// the host to give one back; in display mode it reuses the oldest
    /// unread ready slot, or returns None (the frame is dropped). None also
    /// once the host closed the ring.
    pub fn acquire(&self) -> Option<usize> {
        let shared = &self.shared;
        loop {
            if shared.closed() {
                return None;
            }
            for slot in 0..shared.slots {
                if shared.state(slot).compare_exchange(FREE, WRITING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    return Some(slot);
                }
            }
            if !shared.capture() {
                let oldest = (0..shared.slots)
                    .filter(|slot| shared.state(*slot).load(Ordering::Acquire) == READY)
                    .min_by_key(|slot| shared.info(*slot).seq);
                if let Some(slot) = oldest {
                    if shared.state(slot).compare_exchange(READY, WRITING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                        return Some(slot);
                    }
                    continue;
                }
                shared.mem.atomic_u64(H_DROPPED).fetch_add(1, Ordering::Relaxed);
                return None;
            }
            shared.free.wait();
        }
    }

    /// The frame is in the slot's texture (the GPU work finished): stamp it
    /// and hand it to the host.
    #[allow(clippy::too_many_arguments)]
    pub fn publish(&self, slot: usize, width: u32, height: u32, unix_ms: i64, elapsed_ns: u64, epoch: u64, probes: &[u8]) {
        let shared = &self.shared;
        let base = HEADER + slot * stride(shared.probes);
        let seq = shared.mem.atomic_u64(H_SEQ).fetch_add(1, Ordering::AcqRel) + 1;
        shared.mem.atomic_u32(base + S_WIDTH).store(width, Ordering::Relaxed);
        shared.mem.atomic_u32(base + S_HEIGHT).store(height, Ordering::Relaxed);
        shared.mem.atomic_u64(base + S_SEQ).store(seq, Ordering::Relaxed);
        shared.mem.atomic_u64(base + S_UNIX_MS).store(unix_ms as u64, Ordering::Relaxed);
        shared.mem.atomic_u64(base + S_ELAPSED_NS).store(elapsed_ns, Ordering::Relaxed);
        shared.mem.atomic_u64(base + S_EPOCH).store(epoch, Ordering::Relaxed);
        let out = unsafe { shared.mem.bytes(base + S_PROBES, shared.probes) };
        let n = out.len().min(probes.len());
        out[..n].copy_from_slice(&probes[..n]);
        out[n..].fill(0);
        shared.state(slot).store(READY, Ordering::Release);
        shared.ready.post();
    }

    /// The slot was not filled after all.
    pub fn abandon(&self, slot: usize) {
        self.shared.state(slot).store(FREE, Ordering::Release);
        self.shared.free.post();
    }
}

/// Linux: the slots' DMA-BUFs between host and child.
#[cfg(target_os = "linux")]
mod linux {
    use super::RingSlot;
    use makepad_platform::makepad_hal::dmabuf::{copy, hal_dmabuf_register};
    use makepad_platform::makepad_hal::linux_ipc::{Channel, Never};
    use makepad_platform::thread::ThreadOptions;
    use makepad_platform::Cx;
    use std::io;
    use std::os::fd::{IntoRawFd, OwnedFd};
    use std::os::unix::net::UnixListener;
    use std::time::{Duration, Instant};

    /// How long the host waits for its child to come for the descriptors.
    const SERVE_FOR: Duration = Duration::from_secs(60);

    /// The slots with this process's handles: each export registered (a
    /// descriptor of its own) for reading the slot here (stills, encoders).
    pub fn local_slots(mut slots: Vec<RingSlot>, exports: &[(OwnedFd, [u64; 6])]) -> Result<Vec<RingSlot>, String> {
        let mut made = Vec::new();
        for (slot, (fd, layout)) in slots.iter_mut().zip(exports) {
            let handle = fd.try_clone().map(|fd| hal_dmabuf_register(fd.into_raw_fd(), layout)).unwrap_or(0);
            if handle == 0 {
                for handle in made {
                    makepad_platform::makepad_hal::dmabuf::hal_dmabuf_release(handle);
                }
                return Err("frame ring: a slot's DMA-BUF was refused".into());
            }
            made.push(handle);
            slot.handle = handle;
            slot.layout = layout.to_vec();
        }
        Ok(slots)
    }

    /// Listen on a socket of the ring's own and give the first connection
    /// every descriptor (`(slot, fd)`, in order), then stop. Returns its path.
    pub fn serve_fds(cx: &mut Cx, ring: u64, exports: Vec<(OwnedFd, [u64; 6])>) -> io::Result<String> {
        let path = crate::shm::file_path(&crate::shm::unique_name('d')).with_extension("sock");
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let shown = path.display().to_string();
        let started = cx.thread_spawner().spawn_worker(ThreadOptions { name: Some(format!("frame-ring-fds-{ring}").into()), ..Default::default() }, move || {
            let deadline = Instant::now() + SERVE_FOR;
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break None,
                }
            };
            let _ = std::fs::remove_file(&path);
            let Some(stream) = stream else { return };
            if stream.set_nonblocking(false).is_err() {
                return;
            }
            let channel: Channel<(u64, OwnedFd), Never> = Channel::from(stream);
            for (slot, (fd, _)) in exports.into_iter().enumerate() {
                if channel.send((slot as u64, fd)).is_err() {
                    return;
                }
            }
        });
        match started {
            Ok(worker) => {
                worker.detach();
                Ok(shown)
            }
            Err(_) => Err(io::Error::other("frame ring: no thread for the descriptor socket")),
        }
    }

    /// The child: take the slots' descriptors and register them; the slots'
    /// handles become this process's. A setup without a socket carries no
    /// textures to receive (the protocol tests).
    pub fn receive_slots(setup: &mut super::RingSetup) -> io::Result<Vec<u64>> {
        if setup.fds.is_empty() {
            return Ok(Vec::new());
        }
        let channel: Channel<Never, (u64, OwnedFd)> = Channel::try_connect(std::path::Path::new(&setup.fds))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "frame ring: the descriptor socket is gone"))?;
        channel.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut local = Vec::with_capacity(setup.slots.len());
        let result = (|| {
            for (index, slot) in setup.slots.iter_mut().enumerate() {
                let (at, fd) = channel.recv()?;
                let layout = copy::Layout::from_words(&slot.layout).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame ring: slot layout"))?;
                if at != index as u64 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame ring: descriptors out of order"));
                }
                let handle = hal_dmabuf_register(fd.into_raw_fd(), &layout.to_words());
                local.push(handle);
                slot.handle = handle;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for handle in local {
                makepad_platform::makepad_hal::dmabuf::hal_dmabuf_release(handle);
            }
            return Err(error);
        }
        Ok(local)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Producer and host over one block, without textures: the protocol
    /// alone (the host side is driven through its shared block directly).
    fn pair(slots: usize, capture: bool) -> (Arc<RingShared>, ProducerRing) {
        let name = shm::unique_name('r');
        let path = shm::file_path(&name);
        let len = HEADER + slots * stride(2);
        let mem = SharedMem::create(&path, len).unwrap();
        mem.atomic_u32(0).store(MAGIC, Ordering::Relaxed);
        mem.atomic_u32(4).store(VERSION, Ordering::Relaxed);
        mem.atomic_u32(H_SLOTS).store(slots as u32, Ordering::Relaxed);
        mem.atomic_u32(H_PROBES).store(2, Ordering::Relaxed);
        mem.atomic_u32(H_CAPTURE).store(capture as u32, Ordering::Relaxed);
        let free = Semaphore::create(&shm::unique_name('f')).unwrap();
        let ready = Semaphore::create(&shm::unique_name('w')).unwrap();
        let setup = RingSetup {
            ring: 1,
            generation: 1,
            shm: path.display().to_string(),
            len: len as u64,
            free_sem: free.name().into(),
            ready_sem: ready.name().into(),
            slots: (0..slots).map(|i| RingSlot { handle: i as u64 + 1, width: 4, height: 4, layout: Vec::new() }).collect(),
            probes: vec![0, 0, 1, 1],
            fds: String::new(),
        };
        let host = Arc::new(RingShared {
            mem,
            free,
            ready,
            slots,
            probes: 2,
            holds: (0..slots).map(|_| AtomicU32::new(0)).collect(),
            leases: AtomicUsize::new(0),
            #[cfg(target_os = "linux")]
            local: Vec::new(),
        });
        let producer = ProducerRing::open(setup).unwrap();
        (host, producer)
    }

    fn take_ready(host: &RingShared, next_seq: &mut u64) -> Vec<(u64, usize)> {
        let mut ready: Vec<_> = (0..host.slots).filter(|s| host.state(*s).load(Ordering::Acquire) == READY).map(|s| (host.info(s).seq, s)).collect();
        ready.sort_unstable();
        keep_in_order(&mut ready, next_seq);
        for (_, slot) in &ready {
            host.state(*slot).store(HELD, Ordering::Release);
            host.hold(*slot);
        }
        ready
    }

    #[test]
    fn capture_is_lossless_and_waits_for_the_host() {
        let (host, producer) = pair(3, true);
        let producer = Arc::new(producer);
        let made = 50u64;
        let thread = {
            let producer = producer.clone();
            std::thread::spawn(move || {
                for n in 0..made {
                    let slot = producer.acquire().expect("a slot");
                    producer.publish(slot, 4, 4, n as i64, n, 0, &[n as u8, 0]);
                }
            })
        };
        let mut seen = Vec::new();
        let mut next_seq = 0;
        while seen.len() < made as usize {
            for (seq, slot) in take_ready(&host, &mut next_seq) {
                assert_eq!(host.info(slot).probes[0], (seq - 1) as u8);
                seen.push(seq);
                host.unhold(slot);
            }
            std::thread::yield_now();
        }
        thread.join().unwrap();
        assert_eq!(seen, (1..=made).collect::<Vec<_>>(), "every frame, in order");
    }

    #[test]
    fn display_never_waits() {
        let (host, producer) = pair(3, false);
        for n in 0..10 {
            if let Some(slot) = producer.acquire() {
                producer.publish(slot, 4, 4, n, 0, 0, &[]);
            }
        }
        // Nothing was taken: the producer kept reusing the oldest ready slot.
        let ready = take_ready(&host, &mut 0);
        assert_eq!(ready.len(), 3);
        assert_eq!(ready.last().unwrap().0, 10);
    }

    #[test]
    fn closing_wakes_a_waiting_producer() {
        let (host, producer) = pair(2, true);
        for _ in 0..2 {
            let slot = producer.acquire().unwrap();
            producer.publish(slot, 4, 4, 0, 0, 0, &[]);
        }
        let thread = std::thread::spawn(move || producer.acquire());
        host.mem.atomic_u32(H_CLOSED).store(1, Ordering::Release);
        host.free.post();
        assert_eq!(thread.join().unwrap(), None);
    }
}
