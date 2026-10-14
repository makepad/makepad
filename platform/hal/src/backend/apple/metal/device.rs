//! The Metal device of the HAL: device, queue, the two worker threads (submit; allocate / release
//! / compile), the per-batch arena, the completion mailbox and the numbers the ledger reconciles.
//! Moved from os/apple/metal.rs (`MetalCx::new`, `spawn_submitter`, `spawn_allocator`,
//! `MetalRenderSetup`, `MetalUniformChunk` / `bind_inline_uniforms`).

use super::timing::*;
use makepad_apple_sys::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What a finished piece of GPU work tells the backend thread. Completion handlers run on
/// Metal's threads: they push here and wake the pump; `Metal::step` turns them into events.
pub(crate) enum Done {
    Frame { sid: u32, target: crate::Id, frame: u64, gpu_ns: u64, present_ns: u64, gpu_span: [f64; 2], failed: bool, window: bool, kind: u32, incarnation: Arc<crate::font::FrameProgress> },
    Readback { sid: u32, incarnation: Arc<crate::font::FrameProgress>, req: u32, status: u32, rect: [u32; 4], pixels: Vec<u8> },
    Pick { sid: u32, incarnation: Arc<crate::font::FrameProgress>, req: u32, ids: Vec<u32> },
    PipelineReady { sid: u32, incarnation: Arc<crate::font::FrameProgress>, id: crate::Id },
    PipelineFailed { sid: u32, incarnation: Arc<crate::font::FrameProgress>, id: crate::Id, log: String },
    /// A recorded window frame is in its capture buffer (a retained `CVPixelBuffer`).
    Surface { sid: u32, incarnation: Arc<crate::font::FrameProgress>, window: crate::Id, w: u32, h: u32, buffer: usize },
}

impl Done {
    fn sid(&self) -> u32 {
        match self {
            Done::Frame { sid, .. } | Done::Readback { sid, .. } | Done::Pick { sid, .. } | Done::PipelineReady { sid, .. }
            | Done::PipelineFailed { sid, .. } | Done::Surface { sid, .. } => *sid,
        }
    }
}

#[derive(Clone)]
pub struct Mailbox {
    queue: Arc<Mutex<Vec<Done>>>,
    notify: crate::backend::SessionNotify,
}

impl Mailbox {
    pub fn new(notify: crate::backend::SessionNotify) -> Mailbox {
        Mailbox { queue: Arc::new(Mutex::new(Vec::new())), notify }
    }

    /// A completion for its session: that session's pump looks (and drains every completion).
    pub(crate) fn push(&self, d: Done) {
        let sid = d.sid();
        let window = matches!(d, Done::Frame { kind, .. } if crate::backend::window_frame(kind));
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).push(d);
        (self.notify)(sid, window);
    }

    pub(crate) fn take(&self) -> Vec<Done> {
        std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Work for the allocator thread: driver calls that may block (pipeline compiles) and the final
/// release of Metal objects, so the backend thread never pays a deallocation.
pub enum AllocJob {
    Compile(Box<dyn FnOnce() + Send>),
    Release(Box<dyn Send>),
}

/// A committed command buffer on its way to the submit thread.
pub struct Submission {
    pub buffer: RcObjcId,
    pub entry: InFlightCb,
}

/// hardware: the command queue's pool, `newCommandQueueWithMaxCommandBufferCount` (the driver's
/// own backpressure; past it `commandBuffer` waits inside the driver). Metal's default of 64
/// holds barely two frames of per-pass buffers for a compositor with a 30-pass frame.
const QUEUE_MAX_COMMAND_BUFFERS: u64 = 256;

/// The arena's chunk size: the uniforms, inline instances and upload staging of a batch.
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
/// Free arena bytes kept for the next batches; beyond it the largest free chunks are released.
const ARENA_KEEP_BYTES: usize = 64 << 20;

/// A place in the arena.
#[derive(Clone, Copy)]
pub struct Loc {
    pub buffer: ObjcId,
    pub offset: u64,
}

struct Chunk {
    buffer: RcObjcId,
    contents: *mut u8,
    capacity: usize,
    used: usize,
    /// the newest command buffer that reads it: reusable from offset 0 once that one completed
    lease: u64,
}

pub struct Device {
    pub device: ObjcId,
    pub queue: ObjcId,
    /// 1x1 transparent texture bound where a draw's texture slot has no storage (sampling nil is
    /// a GPU fault)
    pub fallback: ObjcId,
    pub name: String,
    /// highest command buffer sequence number that COMPLETED; the one queue executes buffers in
    /// enqueue order, so completion of N implies everything below it
    pub completed: Arc<AtomicU64>,
    /// the sequence number of the newest command buffer created
    pub seq: u64,
    pub mailbox: Mailbox,
    alloc_tx: Option<SyncSender<AllocJob>>,
    alloc_thread: Option<std::thread::JoinHandle<()>>,
    submit_tx: Option<SyncSender<Submission>>,
    submit_thread: Option<std::thread::JoinHandle<()>>,
    /// encoded work the saturated submitter could not take yet, in order
    submit_retry: std::collections::VecDeque<Submission>,
    watchdog_stop: Arc<AtomicBool>,
    watchdog_thread: Option<std::thread::JoinHandle<()>>,
    pub in_flight: InFlightQueue,
    pub depth_write: RcObjcId,
    pub depth_no_write: RcObjcId,
    pub descriptor: RcObjcId,
    pub clock: Option<Option<Arc<PassClock>>>,
    chunks: Vec<Chunk>,
    /// batch-local: where a (pointer, length) of the caller's memory already sits in the arena
    pub dedup: HashMap<(usize, usize), Loc>,
    pub recommended_working_set: u64,
}

// SAFETY: Metal objects may be used from any thread; the backend is only ever driven under the
// Hal's state lock, one thread at a time.
unsafe impl Send for Device {}

impl Device {
    pub fn new(mailbox: Mailbox) -> Option<Device> {
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        if device == nil {
            return None;
        }
        let name = nsstring_to_string(unsafe { msg_send![device, name] });
        let recommended: u64 = unsafe {
            let available: bool = msg_send![device, respondsToSelector: sel!(recommendedMaxWorkingSetSize)];
            if available {
                msg_send![device, recommendedMaxWorkingSetSize]
            } else {
                0
            }
        };
        let queue: ObjcId = unsafe { msg_send![device, newCommandQueueWithMaxCommandBufferCount: QUEUE_MAX_COMMAND_BUFFERS] };
        if queue == nil {
            return None;
        }
        let fallback = unsafe {
            let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
            let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
            let _: () = msg_send![descriptor, setWidth: 1u64];
            let _: () = msg_send![descriptor, setHeight: 1u64];
            let _: () = msg_send![descriptor, setDepth: 1u64];
            let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
            let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
            let tex: ObjcId = msg_send![device, newTextureWithDescriptor: descriptor];
            let _: () = msg_send![descriptor, release];
            let zero: [u8; 4] = [0, 0, 0, 0];
            let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: 1, height: 1, depth: 1 } };
            let _: () = msg_send![tex, replaceRegion: region mipmapLevel: 0u64 withBytes: zero.as_ptr() as *const std::ffi::c_void bytesPerRow: 4u64];
            tex
        };
        let (depth_write, depth_no_write, descriptor) = unsafe {
            let desc = RcObjcId::from_owned(NonNull::new(msg_send![class!(MTLDepthStencilDescriptor), new])?);
            let _: () = msg_send![desc.as_id(), setDepthCompareFunction: MTLCompareFunction::LessEqual];
            let make = |write: bool| {
                let _: () = msg_send![desc.as_id(), setDepthWriteEnabled: write];
                NonNull::new(msg_send![device, newDepthStencilStateWithDescriptor: desc.as_id()]).map(RcObjcId::from_owned)
            };
            let write = make(true)?;
            let no_write = make(false)?;
            let pass = RcObjcId::from_owned(NonNull::new(msg_send![class!(MTLRenderPassDescriptor), new])?);
            let stencil: ObjcId = msg_send![pass.as_id(), stencilAttachment];
            let _: () = msg_send![stencil, setTexture: nil];
            (write, no_write, pass)
        };
        let completed = Arc::new(AtomicU64::new(0));
        let in_flight: InFlightQueue = Arc::new(Mutex::new(Default::default()));
        let (submit_tx, submit_thread) = spawn_submitter(in_flight.clone(), mailbox.clone());
        let (alloc_tx, alloc_thread) = spawn_allocator(mailbox.clone());
        let watchdog_stop = Arc::new(AtomicBool::new(false));
        let watchdog_thread = spawn_hang_watchdog(in_flight.clone(), completed.clone(), watchdog_stop.clone());
        let physical: u64 = unsafe {
            let info: ObjcId = msg_send![class!(NSProcessInfo), processInfo];
            msg_send![info, physicalMemory]
        };
        crate::gpu_limits::set_max_gpu_buffer_bytes(unsafe { msg_send![device, maxBufferLength] });
        crate::gpu_limits::set_gpu_working_set_bytes(if recommended != 0 { recommended } else { physical / 2 });
        let mut dev = Device {
            device,
            queue,
            fallback,
            name,
            completed,
            seq: 0,
            mailbox,
            alloc_tx: Some(alloc_tx),
            alloc_thread: Some(alloc_thread),
            submit_tx: Some(submit_tx),
            submit_thread: Some(submit_thread),
            submit_retry: Default::default(),
            watchdog_stop,
            watchdog_thread: Some(watchdog_thread),
            in_flight,
            depth_write,
            depth_no_write,
            descriptor,
            clock: None,
            chunks: Vec::new(),
            dedup: HashMap::new(),
            recommended_working_set: crate::gpu_limits::gpu_working_set_bytes(),
        };
        // the first batches' uniforms: two chunks, touched now so no draw waits for a page fault
        for _ in 0..2 {
            dev.add_chunk(CHUNK_BYTES);
        }
        Some(dev)
    }

    /// The pass clock of this device, probed once.
    pub fn pass_clock(&mut self) -> Option<Arc<PassClock>> {
        if self.clock.is_none() {
            self.clock = Some(PassClock::new(self.device));
        }
        self.clock.clone().flatten()
    }

    // ---- threads

    /// Hands a driver call to the allocator thread. A full queue runs it here: never dropped.
    pub fn compile(&self, job: Box<dyn FnOnce() + Send>) {
        match self.alloc_tx.as_ref().map(|tx| tx.try_send(AllocJob::Compile(job))) {
            Some(Ok(())) => {}
            Some(Err(TrySendError::Full(AllocJob::Compile(job)))) | Some(Err(TrySendError::Disconnected(AllocJob::Compile(job)))) => job(),
            _ => {}
        }
    }

    /// The final release of a Metal object happens on the allocator thread. A full queue
    /// releases it here.
    pub fn retire(&self, object: Box<dyn Send>) {
        if let Some(tx) = &self.alloc_tx {
            if let Err(TrySendError::Full(AllocJob::Release(o)) | TrySendError::Disconnected(AllocJob::Release(o))) = tx.try_send(AllocJob::Release(object)) {
                drop(o);
            }
        }
    }

    /// A new command buffer: enqueued at once so queue order is reserved, numbered, with the
    /// completion handler that publishes its number. Returns the buffer (owned by the caller)
    /// and its number.
    pub fn new_command_buffer(&mut self) -> (RcObjcId, u64) {
        let buffer: ObjcId = unsafe { msg_send![self.queue, commandBuffer] };
        let buffer = RcObjcId::from_unowned(NonNull::new(buffer).expect("Metal command buffer"));
        let () = unsafe { msg_send![buffer.as_id(), enqueue] };
        self.seq += 1;
        let seq = self.seq;
        let completed = self.completed.clone();
        let in_flight = self.in_flight.clone();
        let () = unsafe {
            msg_send![buffer.as_id(), addCompletedHandler: &objc_block!(move |cb: ObjcId| {
                completed.fetch_max(seq, Ordering::AcqRel);
                let entry = {
                    let mut queue = lock_in_flight(&in_flight);
                    queue.iter().position(|e| e.seq == seq).and_then(|at| queue.remove(at))
                };
                report_command_buffer_error(cb, seq, entry.as_ref());
            })]
        };
        (buffer, seq)
    }

    /// Commits a command buffer through the submit thread (`commit` never runs on the backend
    /// thread). Saturated: it stays queued, in order, for the next flush.
    pub fn commit(&mut self, buffer: RcObjcId, entry: InFlightCb) {
        self.submit_retry.push_back(Submission { buffer, entry });
        self.flush_submissions();
    }

    pub fn flush_submissions(&mut self) -> bool {
        while let Some(submission) = self.submit_retry.pop_front() {
            let Some(tx) = &self.submit_tx else { return false };
            if let Err(e) = tx.try_send(submission) {
                let (TrySendError::Full(v) | TrySendError::Disconnected(v)) = e;
                self.submit_retry.push_front(v);
                return false;
            }
        }
        true
    }

    // ---- the arena

    fn add_chunk(&mut self, capacity: usize) -> Option<usize> {
        // allocated right here and its pages touched, so no draw is skipped while a chunk sits
        // with a worker
        let buffer = NonNull::new(unsafe { msg_send![self.device, newBufferWithLength: capacity as u64 options: nil] }).map(RcObjcId::from_owned)?;
        let contents: *mut u8 = unsafe { msg_send![buffer.as_id(), contents] };
        if contents.is_null() {
            return None;
        }
        for offset in (0..capacity).step_by(4096) {
            unsafe { contents.add(offset).write_volatile(0) };
        }
        self.chunks.push(Chunk { buffer, contents, capacity, used: 0, lease: 0 });
        Some(self.chunks.len() - 1)
    }

    /// Copies `data` into the arena for command buffer `seq` (256-byte aligned: the constant
    /// address space needs it). The same (pointer, length) of one batch is copied once.
    pub fn arena_put(&mut self, data: &[u8], seq: u64) -> Option<Loc> {
        let key = (data.as_ptr() as usize, data.len());
        if let Some(loc) = self.dedup.get(&key).copied() {
            self.touch_lease(loc.buffer, seq);
            return Some(loc);
        }
        let loc = self.arena_copy(data, seq)?;
        self.dedup.insert(key, loc);
        Some(loc)
    }

    /// Like `arena_put` for data that does not live to the end of the batch (a buffer the caller
    /// frees right after): its address says nothing about its content, as the next such buffer
    /// of the same length is allocated at the same address, so it is never deduplicated.
    pub fn arena_put_transient(&mut self, data: &[u8], seq: u64) -> Option<Loc> {
        self.arena_copy(data, seq)
    }

    fn arena_copy(&mut self, data: &[u8], seq: u64) -> Option<Loc> {
        let completed = self.completed.load(Ordering::Acquire);
        let mut found = None;
        for (i, chunk) in self.chunks.iter_mut().enumerate() {
            // a chunk is an arena every command buffer appends to: a buffer still executing
            // reads only what was written before it was encoded, so later writes go past `used`;
            // it rewinds once the buffer that leased it completed
            if chunk.lease <= completed {
                chunk.used = 0;
            }
            let offset = (chunk.used + 255) & !255;
            if offset + data.len() <= chunk.capacity {
                found = Some((i, offset));
                break;
            }
        }
        let (i, offset) = match found {
            Some(f) => f,
            None => {
                let capacity = data.len().next_power_of_two().max(CHUNK_BYTES);
                let i = self.add_chunk(capacity)?;
                (i, 0)
            }
        };
        let chunk = &mut self.chunks[i];
        // SAFETY: offset + len <= capacity of a live shared buffer; the source is the caller's data
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), chunk.contents.add(offset), data.len()) };
        chunk.used = offset + data.len();
        chunk.lease = chunk.lease.max(seq);
        Some(Loc { buffer: chunk.buffer.as_id(), offset: offset as u64 })
    }

    fn touch_lease(&mut self, buffer: ObjcId, seq: u64) {
        if let Some(c) = self.chunks.iter_mut().find(|c| c.buffer.as_id() == buffer) {
            c.lease = c.lease.max(seq);
        }
    }

    /// The batch is over: the dedup memory is the caller's again, and free chunks beyond the
    /// keep size are released (largest first).
    pub fn arena_end_batch(&mut self) {
        self.dedup.clear();
        let completed = self.completed.load(Ordering::Acquire);
        let mut free: Vec<(usize, usize)> = self.chunks.iter().enumerate().filter(|(_, c)| c.lease <= completed).map(|(i, c)| (c.capacity, i)).collect();
        let mut total: usize = free.iter().map(|f| f.0).sum();
        if total <= ARENA_KEEP_BYTES {
            return;
        }
        free.sort_unstable_by(|a, b| b.cmp(a));
        let mut drop_idx = Vec::new();
        for (cap, i) in free {
            if total <= ARENA_KEEP_BYTES {
                break;
            }
            total -= cap;
            drop_idx.push(i);
        }
        drop_idx.sort_unstable_by(|a, b| b.cmp(a));
        for i in drop_idx {
            let chunk = self.chunks.swap_remove(i);
            self.retire(Box::new(chunk.buffer));
        }
    }

    /// The device's own count of what this process holds on it.
    pub fn device_allocated(&self) -> u64 {
        unsafe { msg_send![self.device, currentAllocatedSize] }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.watchdog_stop.store(true, Ordering::Release);
        self.submit_tx = None;
        self.alloc_tx = None;
        if let Some(t) = self.submit_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.alloc_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.watchdog_thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _: () = msg_send![self.queue, release];
            let _: () = msg_send![self.fallback, release];
        }
    }
}

/// The submit thread: `commit` never runs on the backend thread. Owned by the device (joined on
/// drop when its sender goes).
fn spawn_submitter(in_flight: InFlightQueue, mailbox: Mailbox) -> (SyncSender<Submission>, std::thread::JoinHandle<()>) {
    let (tx, rx): (SyncSender<Submission>, Receiver<Submission>) = sync_channel(16);
    let thread = std::thread::Builder::new()
        .name("makepad-metal-submit".into())
        .spawn(move || {
            let _ = &mailbox;
            while let Ok(mut submission) = rx.recv() {
                let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
                submission.entry.committed_at = Some(Instant::now());
                let buffer = submission.buffer.as_id();
                lock_in_flight(&in_flight).push_back(submission.entry);
                let _: () = unsafe { msg_send![buffer, commit] };
                drop(submission.buffer);
                let _: () = unsafe { msg_send![pool, release] };
            }
        })
        .expect("Metal submission worker");
    (tx, thread)
}

/// The allocator thread: pipeline compiles and the release of retired Metal objects, off the
/// backend thread.
fn spawn_allocator(_mailbox: Mailbox) -> (SyncSender<AllocJob>, std::thread::JoinHandle<()>) {
    let (tx, rx) = sync_channel::<AllocJob>(256);
    let thread = std::thread::Builder::new()
        .name("makepad-metal-alloc".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
                match job {
                    AllocJob::Compile(f) => f(),
                    AllocJob::Release(o) => drop(o),
                }
                let _: () = unsafe { msg_send![pool, release] };
            }
        })
        .expect("Metal allocation worker");
    (tx, thread)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dev: &Device, loc: Loc, len: usize) -> Vec<u8> {
        let chunk = dev.chunks.iter().find(|c| c.buffer.as_id() == loc.buffer).expect("chunk of the location");
        unsafe { std::slice::from_raw_parts(chunk.contents.add(loc.offset as usize), len).to_vec() }
    }

    /// Texture uploads come from buffers the caller frees right after (the font service's atlas
    /// rectangles): the next one of the same length lands on the same address with other
    /// content, and must not be taken for the first (garbled and missing glyphs in small
    /// monospace labels, whose rectangles all have one size).
    #[test]
    fn transient_buffers_of_one_address_are_not_deduplicated() {
        let Some(mut dev) = Device::new(Mailbox::new(Arc::new(|_, _| {}))) else { return };
        let (_, seq) = dev.new_command_buffer();
        let mut seen = Vec::new();
        for i in 0..8u8 {
            let data = vec![i + 1; 400];
            let loc = dev.arena_put_transient(&data, seq).expect("room");
            assert_eq!(read(&dev, loc, 400), data, "upload {i} has its own bytes");
            seen.push((loc.offset, i));
            drop(data);
        }
        let mut offsets: Vec<u64> = seen.iter().map(|s| s.0).collect();
        offsets.dedup();
        assert_eq!(offsets.len(), 8, "each upload has its own place");
        // the keyed path still shares one copy of one live slice
        let live = vec![9u8; 400];
        let a = dev.arena_put(&live, seq).unwrap();
        let b = dev.arena_put(&live, seq).unwrap();
        assert_eq!((a.offset, a.buffer), (b.offset, b.buffer));
    }
}
