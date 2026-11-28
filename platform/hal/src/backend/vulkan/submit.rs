//! One command buffer per frame (plus independent upload/readback batches). The submission
//! worker owns queue access; a separate completion worker waits fences and reads timestamps.

use super::device::{error, lock, Device, Worker};
use super::pipeline::{Pipeline, RenderPass};
use super::resources::{buffer_usage, Buffer, Image};
use super::surface::{Present, Shown};
use crate::font::FrameProgress;
use crate::types::*;
use makepad_vulkan::vk;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

relax_api::option_layouts! {
    ([u8; 16], Placed),
}

#[derive(Clone)]
pub(super) struct Token { pub sid: u32, pub incarnation: Arc<FrameProgress> }

#[derive(Default, PartialEq)]
pub(super) struct UploadCounts {
    arenas_taken: u64, copy_cmds: u64, copy_regions: u64, upload_barriers: u64, submissions: u64, driver_uploads: u64,
}

impl UploadCounts {
    pub fn credit(&self, stats: &mut crate::session::Stats) {
        stats.arenas_taken += self.arenas_taken;
        stats.copy_cmds += self.copy_cmds;
        stats.copy_regions += self.copy_regions;
        stats.upload_barriers += self.upload_barriers;
        stats.submissions += self.submissions;
        stats.driver_uploads += self.driver_uploads;
    }
}

/// A copy or flush may be recorded after its uploader changed. Indices held by queued work
/// still name that uploader. The usual single-owner batch needs no list allocation.
#[derive(Default)]
pub(super) struct BatchCounts {
    pub current: UploadCounts,
    pub earlier: Vec<(Token, UploadCounts)>,
}

impl BatchCounts {
    fn owner(&mut self, index: usize) -> &mut UploadCounts {
        if index == self.earlier.len() { &mut self.current } else { &mut self.earlier[index].1 }
    }
}

pub(super) enum Event {
    ArenaCreated,
    BatchCounts(BatchCounts),
    #[cfg(target_os = "linux")]
    Capture { window: Id, surface: Arc<crate::backend::linux::media::capture_surface::Surface> },
    Presented { target: Id, frame: u64 },
    /// `gpu_span`: the frame's first and last timestamps in seconds of the device's clock (the
    /// frame timer orders a frame's passes by it). `ran`: the GPU did the frame's work.
    Frame { target: Id, kind: u32, frame: u64, window: bool, gpu_ns: u64, gpu_span: [f64; 2], ran: bool },
    DispatchTime { name: String, gpu_ns: u64 },
    BufferLeases { req: u32, status: u32, leases: Vec<crate::lease::Lease> },
    Readback { req: u32, status: u32, rect: [u32; 4], lease: Option<crate::lease::Lease> },
    Shared { id: Id, handle: u64 },
}
pub(super) struct Done { pub token: Token, pub time: u64, pub event: Event }

#[derive(Clone)]
pub(super) struct Mailbox { events: Arc<Mutex<Vec<Done>>>, pub notify: crate::backend::SessionNotify }
impl Mailbox {
    pub fn new(notify: crate::backend::SessionNotify) -> Self { Self { events: Arc::new(Mutex::new(Vec::new())), notify } }
    /// A completion for its session: that session's pump looks (and drains every completion).
    pub fn push(&self, token: Token, event: Event) {
        let sid = token.sid;
        let window = match &event { Event::Frame { kind, .. } => crate::backend::window_frame(*kind), Event::Presented { .. } => true, _ => false };
        lock(&self.events).push(Done { token, event, time: crate::clock_ns() }); (self.notify)(sid, window);
    }
    /// Counters nobody waits for: delivered with the session's next completion or submit, without
    /// waking it (a wake per batch would cost every frame one).
    pub fn push_quiet(&self, token: Token, event: Event) {
        lock(&self.events).push(Done { token, event, time: 0 });
    }
    pub fn take(&self) -> Vec<Done> { std::mem::take(&mut *lock(&self.events)) }
}

pub(super) enum Ticket {
    BufferLeases { token: Token, req: u32, ranges: Vec<(Arc<Buffer>, u64, u64, crate::lease::Lease)> },
    #[cfg(target_os = "linux")]
    Capture { token: Token, window: Id, size: [u32;2], format: u32, buffer: Arc<Buffer> },
    Frame { token: Token, target: Id, kind: u32, frame: u64, window: bool },
    Read { token: Token, req: u32, rect: [u32; 4], format: u32, staging: Arc<Buffer>, stencil: Option<Arc<Buffer>> },
    Shared { token: Token, id: Id, handle: u64 },
    Copy { token: Token, req: u32, rect: [u32; 4] },
    Host { reply: std::sync::mpsc::SyncSender<Option<crate::backend::Pixels>>, image: Arc<Image>, staging: Arc<Buffer> },
    /// Frame capture: a packet's buffer or texture snapshot, read after its transfer fence.
    FrameCap { staging: Arc<Buffer>, len: usize, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send> },
}

/// How a batch ended: its work ran, it was never submitted (its recording or presentation
/// failed for a reason other than a lost device: its frame is dropped, the device lives on), or
/// the driver lost the device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome { Ran, Dropped, Lost }

/// How long a submission or fence wait that ran out of memory waits before it tries again (the
/// host has been asked for room, `Device::starved`). The queue keeps its order meanwhile.
const MEMORY_RETRY: std::time::Duration = std::time::Duration::from_millis(16);

/// Out of host or device memory: a submission or wait that may succeed once there is room.
fn out_of_memory(e: vk::Result) -> bool {
    matches!(e, vk::Result::ERROR_OUT_OF_DEVICE_MEMORY | vk::Result::ERROR_OUT_OF_HOST_MEMORY)
}

/// A driver call answered VK_ERROR_DEVICE_LOST (as formatted by `device::error`).
pub(super) fn lost_error(e: &str) -> bool { e.contains("ERROR_DEVICE_LOST") }

/// Counts what `Swapchain::show` answered (profiled).
fn count(shown: &Result<Shown, String>) {
    crate::profile::count(match shown {
        Ok(Shown::Presented) => "vk show: presented",
        Ok(Shown::Dropped) => "vk show: dropped",
        Ok(Shown::NotReady) => "vk show: no free image (retried)",
        Err(_) => "vk show: error",
    });
}

/// How a frame's show ended, as its outcome and whether it was presented; `None` when no
/// swapchain image was free (it may try again).
fn ended(shown: Result<Shown, String>) -> Option<(Outcome, bool)> {
    match shown {
        Ok(Shown::Presented) => Some((Outcome::Ran, true)),
        Ok(Shown::Dropped) => Some((Outcome::Ran, false)),
        Ok(Shown::NotReady) => None,
        // the frame ran; only its presentation failed (lost only if the driver says so)
        Err(e) => {
            eprintln!("[hal-vulkan] present: {e}");
            Some((if lost_error(&e) { Outcome::Lost } else { Outcome::Ran }, false))
        }
    }
}

/// How often a frame that found no free swapchain image tries again, and how many tries it gets
/// (about one second) before it completes unshown.
const SHOW_RETRY: std::time::Duration = std::time::Duration::from_millis(8);
const SHOW_TRIES: u32 = 125;
/// The completion worker waits for a batch's fence in slices of this long, and says once what a
/// batch still unfinished after FENCE_STALL_SAY waits on (the wait itself never gives up).
const FENCE_SLICE_NS: u64 = 250_000_000;
const FENCE_STALL_SAY: std::time::Duration = std::time::Duration::from_secs(2);

/// The Vulkan objects of a finished batch, reset for the next one. A command pool, fence and
/// timestamp pool made and destroyed per batch cost about a millisecond each way (NVIDIA); a
/// page of live effects submits dozens of target frames a beat.
pub(super) struct Slot {
    dev: Arc<Device>,
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    /// the timestamp pool and how many queries it holds (a profiled frame's batch grew it)
    query: (vk::QueryPool, u32),
}

impl Drop for Slot {
    fn drop(&mut self) { unsafe {
        if self.query.0 != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.query.0, None); }
        self.dev.raw.destroy_fence(self.fence, None);
        self.dev.raw.destroy_command_pool(self.pool, None);
    } }
}

/// What one backend's finished batches leave for the next ones: the reset slots and the upload
/// arenas no batch holds. Shared with its batches in flight (each returns its own).
///
/// Nothing here waits: a session's event loop bounds its beats by its frames on the GPU
/// (`FRAMES_IN_FLIGHT`, every path that beats), so the batches in flight are those of a couple
/// of beats per session, however many target frames a beat has. Unbounded, a submit worker
/// slower than the renderer queued seconds of frames, each batch on a fresh slot, descriptor
/// pools and arena (a phone: 1,439 slots made in 27 s). A wait for room here bounded that by
/// holding the UI thread inside its submit, under the HAL's state lock, for the GPU to finish
/// the same beat's earlier batches (17 to 19 batches a beat against 8 allowed: a beat of 75 ms
/// of GPU work held its UI thread 75 ms in the submit, and every other session's submit and
/// pump with it).
#[derive(Default)]
pub(super) struct Spare {
    /// reset slots in the order batches returned them, the oldest taken first: an app that keeps
    /// submitting goes round all of them, so none idles while it runs and its bursts find theirs.
    /// Each has the HAL time of the first sweep after its return (`sweep_slots`).
    slots: Mutex<VecDeque<(Slot, Option<u64>)>>,
    arenas: Mutex<Arenas>,
    /// Each live window owns its keep. Batches and returned arenas hold weak references,
    /// so a fence signalled after the window closes cannot give it a keep again.
    targets: Mutex<crate::FastMap<u64, Arc<()>>>,
    /// readback staging (CPU-cached, `readback::STAGING`) whose readback was read: the next
    /// readback takes one back instead of allocating on the submitting thread (a 3.9 MB one took
    /// 1 to 4 ms there on NVIDIA, up to 44 ms on a loaded machine, every band of a page export)
    readbacks: Mutex<Arenas>,
    /// batches made and not yet dropped (completed or abandoned)
    in_flight: AtomicUsize,
    /// an arena is being made ahead (`make_ahead`)
    ahead: AtomicBool,
    /// the thread that makes arenas ahead, started with the first one (`None`: it could not start,
    /// submits make their arenas)
    maker: OnceLock<Option<Worker>>,
}
pub(super) type Spares = Arc<Spare>;

impl Spare {
    /// Command buffer slots alive: the batches in flight and the reset ones kept for reuse
    /// (each with its command and timestamp pools).
    pub fn slots(&self) -> u64 {
        (self.in_flight() + lock(&self.slots).len()) as u64
    }

    /// Batches made and not yet dropped.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Releases the reset slots no batch took for `SLOT_IDLE` of HAL time (`now`: `World::now`, so
    /// a moved test or automation clock moves it too). Slots given back since the last sweep are
    /// stamped with `now` first. Taken oldest first, the idle ones are every slot of an app that
    /// stopped submitting, or the part of a pool that 5 s of its batches no longer go round.
    pub fn sweep_slots(&self, now: u64) {
        let gone: Vec<_> = {
            let mut slots = lock(&self.slots);
            for (_, since) in slots.iter_mut().rev() {
                if since.is_some() { break; }
                *since = Some(now);
            }
            let idle = SLOT_IDLE.as_nanos() as u64;
            let n = slots.partition_point(|(_, since)| since.is_some_and(|t| now.saturating_sub(t) >= idle));
            slots.drain(..n).collect()
        };
        drop(gone);
    }

    /// A live window keeps its last frame's returned arenas through idle. Other frame kinds
    /// return their arenas to the ordinary idle pool.
    pub fn frame_target(&self, affinity: u64, kind: u32) -> Weak<()> {
        if kind & !target::FLAGS != target::WINDOW { return Weak::new(); }
        Arc::downgrade(lock(&self.targets).entry(affinity).or_default())
    }

    /// Closing the window lets its last frame's arenas idle out.
    pub fn forget(&self, affinity: u64) {
        lock(&self.targets).remove(&affinity);
    }

    pub fn forget_session(&self, sid: u32) {
        let session = Batch::affinity_of(sid, Id::NONE) >> 32;
        lock(&self.targets).retain(|affinity, _| affinity >> 32 != session);
    }

    /// Stamps returns with HAL time and releases idle arenas except a live window's latest
    /// frame set. Superseding the set or closing the window makes it eligible again.
    pub fn trim(&self, now: u64) {
        let released = lock(&self.arenas).trim(now);
        let staging = lock(&self.readbacks).trim(now);
        drop((released, staging));
    }

    /// The first eligible arena's expiry, or zero to stamp a new return on the next pump.
    pub fn trim_due(&self) -> Option<u64> {
        let uploads = lock(&self.arenas).trim_due();
        let staging = lock(&self.readbacks).trim_due();
        uploads.into_iter().chain(staging).min()
    }

    /// A readback's staging of at least `size` bytes: one a read readback gave back, else a new one.
    pub fn readback_staging(&self, dev: &Arc<Device>, size: u64) -> Result<Arc<Buffer>, String> {
        let kept = lock(&self.readbacks).take(size, size, 0);
        match kept {
            Some(arena) => Ok(arena.buffer),
            None => Buffer::scratch(dev.clone(), crate::ledger::Pool::Staging, size, super::readback::STAGING),
        }
    }

    /// A readback's staging after its pixels were read: kept for the next readback (released once
    /// idle for `ARENA_IDLE`, or beyond `ARENA_KEEP`).
    fn readback_read(&self, staging: Arc<Buffer>) {
        let released = lock(&self.readbacks).give(std::iter::once(staging), 0, &Weak::new());
        drop(released);
    }

    /// Releases every free arena now (the app went to the background).
    #[cfg(target_os = "android")]
    pub fn release_all(&self) {
        let released = lock(&self.arenas).release_all();
        let staging = lock(&self.readbacks).release_all();
        drop((released, staging));
    }

    /// Makes a free arena of `size` on the arena thread, unless one is already on its way: the
    /// batch that took the pool's last arena of a size leaves the next one to that thread instead
    /// of to a later submit (making an 8 MiB arena took 1.4 ms on NVIDIA, a 32 MiB one 5.3 ms).
    fn make_ahead(self: &Arc<Self>, dev: &Arc<Device>, size: u64, owner: &Option<(Token, Mailbox)>) {
        if self.ahead.swap(true, Ordering::AcqRel) { return; }
        let maker = self.maker.get_or_init(|| Worker::new("makepad-vulkan-arenas")
            .map_err(|e| eprintln!("[hal-vulkan] no arena thread ({e}): submits make their upload arenas")).ok());
        // without the thread `ahead` stays set: nothing is made ahead
        let Some(maker) = maker else { return };
        let (spare, dev, owner) = (self.clone(), dev.clone(), owner.clone());
        maker.push(move || {
            // out of memory: no spare (a submit that needs one asks for room)
            if let Ok(arena) = Buffer::scratch(dev, crate::ledger::Pool::UploadArena, size, buffer_usage()) {
                crate::profile::count("upload arena made ahead");
                let released = lock(&spare.arenas).give(std::iter::once(arena), 0, &Weak::new());
                drop(released);
                if let Some((token, mailbox)) = owner { mailbox.push(token, Event::ArenaCreated); }
            }
            spare.ahead.store(false, Ordering::Release);
        });
    }
}

/// The smallest upload arena; arenas grow by powers of two up to `ARENA_MAX`.
const ARENA_MIN: u64 = 256 * 1024;
/// The largest arena a batch asks for: a frame with more upload bytes spreads them over several
/// arenas, as do texture regions and staged APPEND writes. An indivisible upload larger
/// than this (e.g. a single texture row) gets an arena of its own size.
const ARENA_MAX: u64 = 8 << 20;
/// Free arena bytes kept for the next batches; beyond it the largest free arenas are released.
const ARENA_KEEP: u64 = 64 << 20;
/// HAL time since the first trim after return before an arena can be released. A live
/// window's last frame keeps its arenas across idle until superseded, forgotten or evicted.
const ARENA_IDLE: std::time::Duration = std::time::Duration::from_secs(5);
/// A reset command buffer slot no batch took for this long is released.
const SLOT_IDLE: std::time::Duration = ARENA_IDLE;

/// The size of a new arena for `need` bytes: a power of two in [ARENA_MIN, ARENA_MAX], or a
/// single upload larger than ARENA_MAX rounded up to whole ARENA_MIN.
fn arena_size(need: u64) -> u64 {
    if need > ARENA_MAX { need.div_ceil(ARENA_MIN) * ARENA_MIN } else { need.next_power_of_two().clamp(ARENA_MIN, ARENA_MAX) }
}

struct FreeArena {
    buffer: Arc<Buffer>,
    since: Option<u64>,
    affinity: u64,
    keep: Weak<()>,
    survived_idle: bool,
}

impl FreeArena {
    fn due(&self) -> Option<u64> {
        (self.keep.strong_count() == 0).then(|| self.since.map_or(0, |t| t.saturating_add(ARENA_IDLE.as_nanos() as u64)))
    }
}

/// Upload arenas (host-visible buffers for uniforms, inline instances and upload staging) whose
/// batch's fence signalled, or readback staging that was read. A batch takes them back instead
/// of allocating: making one costs a vkAllocateMemory + map (NVIDIA: milliseconds for
/// megabytes). A live window keeps the uploads returned by its latest frame through idle, so its
/// first input can reuse them. Any batch may still take them, and ARENA_KEEP evicts them.
#[derive(Default)]
struct Arenas {
    free: Vec<FreeArena>,
    bytes: u64,
    /// Real time of the last take, for reporting synchronous allocations during a run of uploads.
    lent: Option<u64>,
}

impl Arenas {
    fn in_use(&self) -> bool {
        self.lent.is_some_and(|lent| crate::clock_ns().saturating_sub(lent) <= ARENA_IDLE.as_nanos() as u64)
    }

    fn holds(&self, want: u64) -> bool {
        self.free.iter().any(|a| a.buffer.size >= want)
    }

    /// Prefer this target's smallest fit, then any smallest fit, oldest first among equals.
    /// If none fits the preferred size, take the largest arena that can hold the upload.
    fn take(&mut self, want: u64, len: u64, affinity: u64) -> Option<FreeArena> {
        self.lent = Some(crate::clock_ns());
        let fits = |min: u64| self.free.iter().enumerate().filter(move |(_, a)| a.buffer.size >= min);
        let order = |(_, a): &(usize, &FreeArena)| (a.buffer.size, a.since.unwrap_or(u64::MAX));
        let own = fits(want).filter(|(_, a)| affinity != 0 && a.affinity == affinity).min_by_key(order);
        let i = own.or_else(|| fits(want).min_by_key(order))
            .or_else(|| fits(len).max_by_key(|(_, a)| a.buffer.size)).map(|(i, _)| i)?;
        let arena = self.free.swap_remove(i);
        self.bytes -= arena.buffer.bytes;
        Some(arena)
    }

    fn release_all(&mut self) -> Vec<Arc<Buffer>> {
        self.bytes = 0;
        self.free.drain(..).map(|a| a.buffer).collect()
    }

    fn trim_due(&self) -> Option<u64> {
        self.free.iter().filter_map(FreeArena::due).min()
    }

    /// Stamp newly returned arenas even while kept, so surplus arenas that already idled
    /// are eligible at the first trim after their target replaces or releases its set.
    fn trim(&mut self, now: u64) -> Vec<Arc<Buffer>> {
        let mut released = Vec::new();
        self.free.retain_mut(|a| {
            let since = *a.since.get_or_insert(now);
            a.survived_idle = now.saturating_sub(since) >= ARENA_IDLE.as_nanos() as u64;
            if a.survived_idle && a.keep.strong_count() == 0 {
                self.bytes -= a.buffer.bytes;
                released.push(a.buffer.clone());
                false
            } else { true }
        });
        released
    }

    /// Takes back a finished batch's arenas, replacing its target's earlier keep; returns the
    /// largest while over ARENA_KEEP, for the caller to free outside the lock.
    fn give(&mut self, arenas: impl Iterator<Item = Arc<Buffer>>, affinity: u64, keep: &Weak<()>) -> Vec<Arc<Buffer>> {
        if keep.strong_count() != 0 {
            for a in &mut self.free {
                if Weak::ptr_eq(&a.keep, keep) { a.keep = Weak::new(); }
            }
        }
        for buffer in arenas {
            self.bytes += buffer.bytes;
            self.free.push(FreeArena { buffer, since: None, affinity, keep: keep.clone(), survived_idle: false });
        }
        let mut released = Vec::new();
        while self.bytes > ARENA_KEEP {
            let Some((i, _)) = self.free.iter().enumerate().max_by_key(|(_, a)| a.buffer.size) else { break };
            let a = self.free.swap_remove(i);
            self.bytes -= a.buffer.bytes;
            released.push(a.buffer);
        }
        released
    }
}

/// Where an upload landed (`Batch::put`): its arena (the buffer, its size and its serial) and
/// the byte offset.
#[derive(Clone, Copy)]
pub(super) struct Placed { pub buffer: vk::Buffer, pub serial: u64, pub at: u64 }
impl Placed { pub(super) const NONE: Self = Self { buffer: vk::Buffer::null(), serial: 0, at: 0 }; }

#[derive(Clone, Copy, Default)]
struct AppendEnd {
    end: u64,
    arena: vk::Buffer,
    group: usize,
}

pub(super) struct Batch {
    pub dev: Arc<Device>,
    spares: Spares,
    /// Set once the GPU finished with this batch: its objects may be reset and reused.
    reusable: bool,
    pool: vk::CommandPool,
    pub cb: vk::CommandBuffer,
    fence: vk::Fence,
    /// The timestamp pool this batch writes and its size in queries.
    query: (vk::QueryPool, u32),
    /// The slot's timestamp pool when this batch writes no timestamps (kept for the next one).
    idle_query: (vk::QueryPool, u32),
    /// The GPU profiler's timed passes when this batch encodes a recorded frame.
    pub prof: Option<super::prof::BatchProf>,
    /// The recorded frame this batch presents (its display time goes to the profiler).
    pub prof_key: Option<crate::gpu_prof::FrameKey>,
    pub dispatch_query: u32,
    pub dispatch_times: Vec<(Token, String, u32)>,
    pub images: Vec<Arc<Image>>,
    pub buffers: Vec<Arc<Buffer>>,
    pub pipelines: Vec<Arc<Pipeline>>,
    pub compute_pipelines: Vec<Arc<super::compute::Program>>,
    pub compute_descriptors: Vec<super::compute::Descriptors>,
    pub passes: Vec<Arc<RenderPass>>,
    pub framebuffers: Vec<vk::Framebuffer>,
    /// The kept descriptor sets' pools this batch bound sets of (`sets::KeptSets::set`): they go
    /// once the GPU is done with it.
    pub generations: Vec<Arc<super::sets::Generation>>,
    pub tickets: Vec<Ticket>,
    pub scanout: Option<(Arc<dyn super::surface::FramePresenter>, Arc<Image>, Token, u32, Id, Id, u32, u64)>,
    pub present: Option<Present>,
    #[cfg(target_os = "android")]
    pub native_frame: Option<(crate::backend::android::native_layer::Frame, Option<Arc<super::share::sync::Semaphore>>)>,
    #[cfg(target_os = "android")]
    native_signal: Option<Arc<super::share::sync::Semaphore>>,
    /// The frame image a frame tap gets after the fence (`crate::frame_tap::offer`): its
    /// window as the core presents it (session, window) and an image nothing writes while held.
    pub tap: Option<(u32, Id, Arc<Image>)>,
    /// the arena being filled and its used bytes
    arena: Option<(Arc<Buffer>, u64)>,
    /// The first slab updater using each arena owns its eventual non-coherent flush.
    arenas: Vec<(Arc<Buffer>, Option<usize>)>,
    copy_arena: usize,
    /// The session requesting the next upload allocation, including one made on the arena thread.
    pub arena_owner: Option<(Token, Mailbox)>,
    counts: BatchCounts,
    frame_owner: Option<usize>,
    /// Set once the batch carries a frame (`begin_frame`, or made by `frame_batch`): a later
    /// frame never takes it over, it is submitted first.
    pub frame: bool,
    /// Copies still to record, grouped by source arena and destination slab.
    append_copies: Vec<(vk::Buffer, vk::Buffer, Vec<vk::BufferCopy>, usize)>,
    /// End of the last queued write per slab and its copy group. A rewind starts another ordered flush.
    append_ends: crate::FastMap<vk::Buffer, AppendEnd>,
    /// Buffer kinds in this flush that can have readers or writers beyond instance inputs.
    append_other_uses: bool,
    /// what the batch draws for (a frame's session and target, `Batch::affinity_of`), 0 for
    /// none: it takes back the arenas the last batch for the same target gave back
    pub affinity: u64,
    pub keep: Weak<()>,
    /// the upload bytes this batch still expects (the frame's uniforms):
    /// sizes the next arena it needs
    reserve: u64,
    /// arenas this batch had to allocate (none in steady frames): the frame stats' `arena_allocs`
    pub allocs: u64,
    /// this batch made its slot, until counted in its session's stats
    slot_made: bool,
    /// arenas this batch made while the pool was in use (no spare was ready), their bytes and the
    /// time it took: said once at `finish`
    grew: (u64, u64, u64),
    /// The pick block this pass uploaded last (`put_pick`).
    pick_block: Option<([u8; 16], Placed)>,
    /// When `finish` handed the batch to the submit worker, while the frame profile records (0:
    /// not recording): its submit queue wait, vkQueueSubmit and submit->rendered times.
    finished_ns: u64,
    #[cfg(target_os = "linux")]
    pub external_waits: Vec<(Arc<super::share::sync::Semaphore>, u64)>,
    #[cfg(target_os = "linux")]
    pub external_signals: Vec<(Arc<super::share::sync::Semaphore>, u64)>,
    #[cfg(target_os = "linux")]
    pub release_fence: Option<(Arc<super::share::sync::Semaphore>, crate::completion::CompletionReply)>,
    /// Hosted images (a host's shared swapchain image with this writer's timeline) this batch
    /// rendered a frame into. Only these publish a frame on their timeline (`finish`): a batch
    /// that merely reads a hosted image (a snapshot or grab of the window) must neither wait
    /// for the host's release nor publish a value the host never hears of -- that left the
    /// host rejecting the frame, never releasing the image, and the reading batch waiting on
    /// the GPU until the driver declared the device lost (`may_read`).
    #[cfg(target_os = "linux")]
    pub hosted_frames: Vec<Arc<Image>>,
}

impl Batch {
    pub fn set_upload_owner(&mut self, token: Token, mailbox: Mailbox) {
        if let Some((old, mailbox)) = &self.arena_owner {
            if Arc::ptr_eq(&old.incarnation, &token.incarnation) { return; }
            let current = std::mem::take(&mut self.counts.current);
            if current != UploadCounts::default() {
                mailbox.push_quiet(old.clone(), Event::BatchCounts(BatchCounts { current, earlier: Vec::new() }));
            }
            self.counts.earlier.push((old.clone(), UploadCounts::default()));
        }
        self.arena_owner = Some((token, mailbox));
    }

    pub fn set_frame_owner(&mut self, token: Token, mailbox: Mailbox) {
        self.set_upload_owner(token, mailbox);
        self.frame_owner = Some(self.counts.earlier.len());
    }

    /// What an unfinished batch waits on, for the GPU-stall message: the host timelines it
    /// waits for (value wanted against the semaphore's value now) and what it carries.
    fn stall_report(&self) -> String {
        let mut out = String::new();
        #[cfg(target_os = "linux")]
        {
            for (sem, want) in &self.external_waits {
                let now = unsafe { self.dev.raw.get_semaphore_counter_value(sem.raw) };
                let now = now.map_or_else(|e| format!("{e:?}"), |v| v.to_string());
                out.push_str(&format!("waits for external timeline {:?} to reach {want} (now {now}); ", sem.raw));
            }
            if !self.hosted_frames.is_empty() { out.push_str(&format!("renders {} hosted frame(s); ", self.hosted_frames.len())); }
            if self.release_fence.is_some() { out.push_str("exports a release fence; "); }
        }
        if out.is_empty() { out.push_str("no external waits (the GPU itself is behind or hung); "); }
        let frames = self.tickets.iter().filter(|t| matches!(t, Ticket::Frame { .. })).count();
        out.push_str(&format!("{frames} frame(s), {} image(s); later batches queue behind it", self.images.len()));
        out
    }

    /// Makes the pumps of the sessions this batch reports to look (an out-of-memory retry asks
    /// for room: their pumps answer with GpuPressure).
    fn notify_sessions(&self, mailbox: &Mailbox) {
        let mut sids: Vec<u32> = Vec::new();
        for ticket in &self.tickets {
            let sid = match ticket {
                #[cfg(target_os = "linux")]
                Ticket::Capture { token, .. } => token.sid,
                Ticket::Frame { token, .. } | Ticket::Read { token, .. } | Ticket::Shared { token, .. } | Ticket::Copy { token, .. } | Ticket::BufferLeases { token, .. } => token.sid,
                Ticket::Host { .. } | Ticket::FrameCap { .. } => continue,
            };
            if !sids.contains(&sid) { sids.push(sid); }
        }
        if let Some((_, _, token, ..)) = &self.scanout { if !sids.contains(&token.sid) { sids.push(token.sid); } }
        for sid in sids { (mailbox.notify)(sid, false); }
    }

    /// A batch on a reset slot. `queries`: timestamps it writes (0: none; 2: its own start and
    /// end; more: a profiled frame's passes, `prof`).
    pub fn new(dev: Arc<Device>, spares: &Spares, queries: u32, reserve: u64) -> Result<Self, String> {
        spares.in_flight.fetch_add(1, Ordering::AcqRel);
        let spare = lock(&spares.slots).pop_front();
        let slot_made = spare.is_none();
        if slot_made { crate::profile::count("vk batch slot made"); }
        let mut b = Self { dev, spares: spares.clone(), reusable: false, pool: vk::CommandPool::null(), cb: vk::CommandBuffer::null(), fence: vk::Fence::null(),
            query: (vk::QueryPool::null(), 0), idle_query: (vk::QueryPool::null(), 0), prof: None, prof_key: None, dispatch_query: 2, dispatch_times: Vec::new(),
            images: Vec::new(), buffers: Vec::new(), pipelines: Vec::new(), compute_pipelines: Vec::new(), compute_descriptors: Vec::new(), passes: Vec::new(), framebuffers: Vec::new(),
            generations: Vec::new(), tickets: Vec::new(), scanout: None, present: None, tap: None, arena: None, arenas: Vec::new(), affinity: 0, keep: Weak::new(), reserve, allocs: 0, slot_made, grew: (0, 0, 0),
            arena_owner: None, counts: Default::default(), frame_owner: None, copy_arena: 0, frame: false, append_copies: Default::default(), append_ends: Default::default(), append_other_uses: false,
            #[cfg(target_os = "android")] native_frame: None,
            #[cfg(target_os = "android")] native_signal: None,
            pick_block: None, finished_ns: 0,
            #[cfg(target_os = "linux")] external_waits: Vec::new(),
            #[cfg(target_os = "linux")] external_signals: Vec::new(),
            #[cfg(target_os = "linux")] release_fence: None,
            #[cfg(target_os = "linux")] hosted_frames: Vec::new() };
        if let Some((mut slot, _)) = spare {
            // the slot's objects move into this batch; the emptied slot destroys nothing
            b.pool = std::mem::replace(&mut slot.pool, vk::CommandPool::null());
            b.cb = slot.cb;
            b.fence = std::mem::replace(&mut slot.fence, vk::Fence::null());
            b.idle_query = std::mem::replace(&mut slot.query, (vk::QueryPool::null(), 0));
        }
        unsafe {
            if b.pool == vk::CommandPool::null() {
                // its one command buffer is reset alone when the batch's objects are reused
                b.pool = b.dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(b.dev.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(error)?;
                b.cb = b.dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(b.pool)
                    .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(error)?[0];
                b.fence = b.dev.raw.create_fence(&vk::FenceCreateInfo::default(), None).map_err(error)?;
            }
            b.dev.raw.begin_command_buffer(b.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(error)?;
        }
        b.start_queries(queries)?;
        Ok(b)
    }

    /// The batch's timestamp pool for `queries` and its first stamp, unless it writes them already
    /// (a batch without them keeps its slot's pool idle).
    fn start_queries(&mut self, queries: u32) -> Result<(), String> {
        if queries == 0 || self.dev.timestamp_bits == 0 || self.query.0 != vk::QueryPool::null() { return Ok(()); }
        let queries = queries.max(2);
        unsafe {
            self.query = std::mem::replace(&mut self.idle_query, (vk::QueryPool::null(), 0));
            if self.query.1 < queries {
                // a profiled frame needs more than the slot's pool holds: a larger one, kept
                // with the slot after (sized in powers of two, so steady frames reuse it)
                if self.query.0 != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.query.0, None); }
                let count = queries.next_power_of_two();
                self.query = (vk::QueryPool::null(), 0);
                self.query = (self.dev.raw.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(count), None).map_err(error)?, count);
            }
            self.dev.raw.cmd_reset_query_pool(self.cb, self.query.0, 0, queries);
            self.dev.raw.cmd_write_timestamp(self.cb, vk::PipelineStageFlags::TOP_OF_PIPE, self.query.0, 0);
        }
        Ok(())
    }

    /// Makes this batch of uploads (texture regions, staged slab writes) the next frame's. The
    /// frame's timestamps start here and its staged copies are recorded next, before any pass:
    /// the frame is one submission, its GPU time includes the copies, and its uniforms continue
    /// in the arena the uploads filled when they fit there (else one sized by `reserve`), as one
    /// frame's uploads did when they all went through its own batch.
    pub fn begin_frame(&mut self, queries: u32, reserve: u64) -> Result<(), String> {
        self.start_queries(queries)?;
        self.flush_appends();
        // uniforms that do not fit after the uploads start an arena sized for all of them
        if self.arena.as_ref().is_some_and(|(arena, used)| arena.size.saturating_sub(*used) < reserve) { self.arena = None; }
        self.reserve = self.reserve.saturating_add(reserve);
        self.frame = true;
        Ok(())
    }

    /// The arena affinity of a frame's batches: its session and target.
    pub fn affinity_of(sid: u32, target: Id) -> u64 { 1 << 63 | u64::from(sid) << 32 | u64::from(target.index) }

    /// Counts a new slot once, even when several commands record into its batch.
    pub fn take_slot_made(&mut self) -> u64 { u64::from(std::mem::take(&mut self.slot_made)) }

    /// The GPU profiler's timestamp at query `q` of this batch's pool, written once all the work
    /// recorded before it finished (bottom of the pipe: consecutive pairs tile the batch).
    pub fn stamp(&self, q: u32) {
        if self.query.0 != vk::QueryPool::null() {
            unsafe { self.dev.raw.cmd_write_timestamp(self.cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, self.query.0, q); }
        }
    }

    /// Whether this batch may read `image`: any image but a hosted one; a hosted image in the
    /// batch that rendered its frame (the read is recorded before the image goes to the host),
    /// or while the host has given it back (the writer owns it; nothing is published).
    pub fn may_read(&self, image: &Arc<Image>) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if image.timeline.as_ref().is_some_and(|t| t.producer)
            && !self.hosted_frames.iter().any(|f| Arc::ptr_eq(f, image)) && !image.hosted_writable() {
            return Err("the hosted window's newest frame is with its host".into());
        }
        #[cfg(not(target_os = "linux"))]
        let _ = image;
        Ok(())
    }

    /// `image` received a frame in this batch (a pass rendered into it).
    pub fn rendered(&mut self, image: &Arc<Image>) {
        #[cfg(target_os = "linux")]
        if image.timeline.as_ref().is_some_and(|t| t.producer) && !self.hosted_frames.iter().any(|f| Arc::ptr_eq(f, image)) {
            self.hosted_frames.push(image.clone());
        }
        #[cfg(not(target_os = "linux"))]
        let _ = image;
    }

    /// Sixteen also satisfies the byte alignment of HAL's largest texel/compression block.
    fn align(&self) -> u64 {
        self.dev.properties.limits.min_uniform_buffer_offset_alignment.max(self.dev.properties.limits.min_storage_buffer_offset_alignment).max(16)
    }

    /// A readback's staging (`readback::record_read`): one a read readback gave back, else a new one.
    pub fn readback_staging(&self, size: u64) -> Result<Arc<Buffer>, String> {
        self.spares.readback_staging(&self.dev, size)
    }

    pub fn put(&mut self, bytes: &[u8]) -> Result<Placed, String> {
        self.put_aligned(bytes, self.align(), bytes.len().max(16) as u64)
    }

    pub(super) fn put_aligned(&mut self, bytes: &[u8], align: u64, len: u64) -> Result<Placed, String> {
        let step = len.div_ceil(align) * align;
        let write = |arena: &Buffer, at: u64| -> Result<(), String> {
            arena.write_unflushed(at, bytes)?;
            let mut offset = bytes.len() as u64;
            while offset < len {
                let n = (len - offset).min(256);
                arena.write_unflushed(at + offset, &[0u8; 256][..n as usize])?;
                offset += n;
            }
            Ok(())
        };
        // the arena being filled, when it has room (the usual upload: a draw's uniform block)
        if let Some((arena, used)) = self.arena.as_mut() {
            let at = used.div_ceil(align) * align;
            if at.checked_add(len).is_some_and(|end| end <= arena.size) {
                write(arena, at)?;
                *used = at + len;
                self.reserve = self.reserve.saturating_sub(step);
                return Ok(Placed { buffer: arena.buffer, serial: arena.serial, at });
            }
        }
        // the rest of the frame's uploads in one arena when it fits under ARENA_MAX
        let arena = self.next_arena(arena_size(self.reserve.min(ARENA_MAX).max(len)), len)?;
        write(&arena, 0)?;
        let placed = Placed { buffer: arena.buffer, serial: arena.serial, at: 0 };
        self.arena = Some((arena, len));
        self.reserve = self.reserve.saturating_sub(step);
        Ok(placed)
    }

    /// Buffer copies need no row or descriptor alignment. Pack consecutive slab writes together.
    pub fn append_parts(&mut self, slab: Arc<Buffer>, kind: u16, parts: BufferParts<'_>) -> Result<(), String> {
        let dst = slab.buffer;
        let mut last = self.append_ends.get(&dst).copied().unwrap_or_default();
        if last.end == 0 { self.buffers.push(slab); }
        let other_uses = kind & (buffer_kind::KIND_MASK | buffer_kind::INDIRECT) != buffer_kind::INSTANCE;
        let prior_reserve = self.reserve;
        self.reserve = self.reserve.saturating_add(parts.total());
        let result = (|| {
            // SAFETY: the parts are borrowed for this hal_submit call.
            for (off, data) in unsafe { parts.iter() } {
                if data.is_empty() { continue; }
                if off < last.end {
                    self.flush_appends();
                    last = AppendEnd::default();
                }
                let len = data.len() as u64;
                if len <= ARENA_MAX {
                    let src = self.put_aligned(data, 1, len)?;
                    self.queue_append(&mut last, src, dst, off, len);
                } else {
                    // Reserve every piece first. An allocation error must not leave half a write queued.
                    let mut pieces = Vec::with_capacity(data.len().div_ceil(ARENA_MAX as usize));
                    for bytes in data.chunks(ARENA_MAX as usize) {
                        let size = bytes.len() as u64;
                        pieces.push((self.put_aligned(bytes, 1, size)?, size));
                    }
                    let mut at = off;
                    for (src, size) in pieces {
                        self.queue_append(&mut last, src, dst, at, size);
                        at += size;
                    }
                }
                self.append_other_uses |= other_uses;
            }
            Ok(())
        })();
        // Keep completed writes ordered even if a later placement failed.
        if last.end != 0 { self.append_ends.insert(dst, last); }
        self.reserve = prior_reserve;
        result
    }

    fn queue_append(&mut self, last: &mut AppendEnd, src: Placed, dst: vk::Buffer, off: u64, size: u64) {
        // The batch keeps every arena it fills, so a slab never returns to an earlier arena.
        if last.arena != src.buffer {
            last.arena = src.buffer;
            last.group = self.append_copies.len();
            let owner = self.counts.earlier.len();
            self.append_copies.push((src.buffer, dst, Vec::new(), owner));
            // Placements move forward through the arenas, including the pieces of a large write.
            while self.arenas[self.copy_arena].0.buffer != src.buffer { self.copy_arena += 1; }
            self.arenas[self.copy_arena].1.get_or_insert(owner);
        }
        last.end = off + size;
        let regions = &mut self.append_copies[last.group].2;
        if let Some(last) = regions.last_mut() {
            if last.src_offset + last.size == src.at && last.dst_offset + last.size == off && last.size + size <= ARENA_MAX {
                last.size += size;
                return;
            }
        }
        regions.push(vk::BufferCopy { src_offset: src.at, dst_offset: off, size });
    }

    pub fn flush_appends(&mut self) {
        let Some(&(_, _, _, barrier_owner)) = self.append_copies.last() else { return; };
        unsafe {
            for (src, dst, regions, owner) in self.append_copies.drain(..) {
                self.dev.raw.cmd_copy_buffer(self.cb, src, dst, &regions);
                let counts = self.counts.owner(owner);
                counts.copy_cmds += 1;
                counts.copy_regions += regions.len() as u64;
            }
            let mut stages = vk::PipelineStageFlags::VERTEX_INPUT | vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER
                | vk::PipelineStageFlags::TRANSFER;
            if self.dev.compute { stages |= vk::PipelineStageFlags::COMPUTE_SHADER; }
            let mut access = vk::AccessFlags::VERTEX_ATTRIBUTE_READ | vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE;
            if std::mem::take(&mut self.append_other_uses) {
                stages = vk::PipelineStageFlags::ALL_COMMANDS;
                access = vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE;
            }
            self.dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, stages, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(access)], &[], &[]);
        }
        self.counts.owner(barrier_owner).upload_barriers += 1;
        self.append_ends.clear();
    }

    /// Places an upload of whole rows of `row` bytes (a texture region's texels) over as many
    /// arenas as it takes and calls `piece(placed, first_row, rows)` for each part: the room left
    /// in the arena being filled, then free arenas, then new ones. A region larger than an arena
    /// so spreads over the pool's arenas like the frame's other uploads, and only a single row
    /// larger than ARENA_MAX gets an arena of its own size (a 32 MiB texture made one at its
    /// submit: 5.3 ms of the frame).
    pub fn put_rows(&mut self, bytes: &[u8], row: u64, mut piece: impl FnMut(Placed, u64, u64)) -> Result<(), String> {
        let align = self.align();
        let total = bytes.len() as u64 / row;
        let mut first = 0;
        while first < total {
            if let Some((arena, used)) = self.arena.as_mut() {
                let at = used.div_ceil(align) * align;
                let rows = (arena.size.saturating_sub(at) / row).min(total - first);
                if rows > 0 {
                    arena.write_unflushed(at, &bytes[(first * row) as usize..((first + rows) * row) as usize])?;
                    *used = at + rows * row;
                    piece(Placed { buffer: arena.buffer, serial: arena.serial, at }, first, rows);
                    first += rows;
                    continue;
                }
            }
            let left = (total - first) * row;
            let arena = self.next_arena(arena_size(self.reserve.max(left).min(ARENA_MAX).max(row)), row)?;
            self.arena = Some((arena, 0));
        }
        Ok(())
    }

    /// The batch's next arena: a free one of at least `want` bytes (else the largest that holds
    /// `len`), or a new one of `want`. Taking the pool's last arena of that size has the next one
    /// made ahead on the arena thread; a submit that finds none while the uploads go on makes it
    /// here and says so at `finish`.
    fn next_arena(&mut self, want: u64, len: u64) -> Result<Arc<Buffer>, String> {
        let (reused, in_use, short) = {
            let mut arenas = lock(&self.spares.arenas);
            let in_use = arenas.in_use();
            let reused = arenas.take(want, len, self.affinity);
            // Preserve normal make-ahead during active uploads. Resuming with an arena kept
            // through idle needs no speculative replacement for the spare just trimmed.
            let resumed = reused.as_ref().is_some_and(|a| a.affinity == self.affinity && a.survived_idle);
            (reused, in_use, !resumed && !arenas.holds(want))
        };
        let arena = match reused {
            Some(a) => a.buffer,
            None => {
                self.allocs += 1;
                crate::profile::count("upload arena allocs");
                let made_at = std::time::Instant::now();
                let made = match Buffer::scratch(self.dev.clone(), crate::ledger::Pool::UploadArena, want, buffer_usage()) {
                    Ok(b) => b,
                    // out of memory: the pool's free arenas go first (and the host is
                    // asked for room), then one more try
                    Err(e) if super::pipeline::out_of_memory(&e) => {
                        let released = lock(&self.spares.arenas).release_all();
                        drop(released);
                        self.dev.refused("upload arena");
                        Buffer::scratch(self.dev.clone(), crate::ledger::Pool::UploadArena, want, buffer_usage())?
                    }
                    Err(e) => return Err(e),
                };
                if in_use { self.grew = (self.grew.0 + 1, self.grew.1 + want, self.grew.2 + made_at.elapsed().as_nanos() as u64); }
                if let Some((token, mailbox)) = &self.arena_owner { mailbox.push(token.clone(), Event::ArenaCreated); }
                made
            }
        };
        if short { self.spares.make_ahead(&self.dev, want, &self.arena_owner); }
        self.arenas.push((arena.clone(), None));
        self.counts.current.arenas_taken += 1;
        Ok(arena)
    }

    /// Upload a draw's pick block (its pick id and pick-twin mask), unless the pass's previous
    /// one held the same: the draws of one widget or part share it.
    pub fn put_pick(&mut self, bytes: [u8; 16]) -> Result<Placed, String> {
        if let Some((_, placed)) = self.pick_block.filter(|(held, _)| *held == bytes) { return Ok(placed); }
        let placed = self.put(&bytes)?;
        self.pick_block = Some((bytes, placed));
        Ok(placed)
    }

    pub fn finish(mut self, submit: &Worker, complete: Arc<Worker>, mailbox: Mailbox) {
        self.flush_appends();
        if crate::profile::on() { self.finished_ns = crate::clock_ns(); }
        if self.grew.0 > 0 {
            eprintln!("[hal-vulkan] upload arenas: no spare was ready, a submit made {} ({} KB) in {:.2} ms",
                self.grew.0, self.grew.1 >> 10, self.grew.2 as f64 / 1e6);
        }
        #[cfg(not(target_os = "android"))]
        let later = submit.later();
        #[cfg(target_os = "linux")]
        let mut published: Vec<(Arc<super::share::sync::Timeline>, u64)> = Vec::new();
        let ended = (|| unsafe {
            for (arena, slab_owner) in &self.arenas {
                if arena.flush()? {
                    if let Some(owner) = slab_owner { self.counts.owner(*owner).driver_uploads += 1; }
                }
            }
            // A shared target or imported producer image is handed back after every use.
            // FrameDone/Readback/TextureShared are delivered only after this release finishes.
            let mut external = HashSet::new();
            for image in &self.images {
                if image.external.is_some() && external.insert(image.image) {
                    // a hosted frame rendered here waits for the host's release of the previous
                    // one and publishes this one (`hosted_frames`)
                    #[cfg(target_os = "linux")]
                    if let Some(timeline) = image.timeline.as_ref().filter(|t| t.producer && self.hosted_frames.iter().any(|f| Arc::ptr_eq(f, image))) {
                        let (wait, signal) = timeline.submitted()?;
                        published.push((timeline.clone(), wait));
                        self.external_waits.push((timeline.semaphore.clone(), wait));
                        self.external_signals.push((timeline.semaphore.clone(), signal));
                    }
                    image.release_external(self.cb);
                }
            }
            if self.query.0 != vk::QueryPool::null() { self.dev.raw.cmd_write_timestamp(self.cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, self.query.0, 1); }
            self.dev.raw.end_command_buffer(self.cb).map_err(error)
        })();
        // a batch that is not submitted publishes no frame: its hosted images stay the writer's
        #[cfg(target_os = "linux")]
        if ended.is_err() { for (timeline, wait) in &published { timeline.unsubmitted(*wait); } }
        if let Some((token, mailbox)) = &self.arena_owner {
            let owner = self.frame_owner.unwrap_or(self.counts.earlier.len());
            self.counts.owner(owner).submissions += 1;
            mailbox.push_quiet(token.clone(), Event::BatchCounts(std::mem::take(&mut self.counts)));
        }
        submit.push(move || {
            let cb = [self.cb];
            let waits: Vec<vk::Semaphore> = Vec::new();
            let signals: Vec<vk::Semaphore> = Vec::new();
            let stages: Vec<vk::PipelineStageFlags> = Vec::new();
            #[cfg(target_os = "android")]
            let signals = {
                let mut signals = signals;
                self.native_signal = self.native_frame.as_ref().and_then(|(_, sem)| sem.clone());
                if let Some(sem) = &self.native_signal { signals.push(sem.raw); }
                signals
            };
            #[cfg(target_os = "linux")]
            let (mut waits, mut signals, mut stages) = (waits, signals, stages);
            #[cfg(target_os = "linux")]
            let mut wait_values = vec![0; waits.len()];
            #[cfg(target_os = "linux")]
            let mut signal_values = vec![0; signals.len()];
            #[cfg(target_os = "linux")]
            {
                for (sem, value) in &self.external_waits { waits.push(sem.raw); stages.push(vk::PipelineStageFlags::ALL_COMMANDS); wait_values.push(*value); }
                for (sem, value) in &self.external_signals { signals.push(sem.raw); signal_values.push(*value); }
                if let Some((sem, _)) = &self.release_fence { signals.push(sem.raw); signal_values.push(0); }
            }
            let info = vk::SubmitInfo::default().command_buffers(&cb).wait_semaphores(&waits)
                .wait_dst_stage_mask(&stages).signal_semaphores(&signals);
            #[cfg(target_os = "linux")]
            let mut timeline = vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&wait_values).signal_semaphore_values(&signal_values);
            #[cfg(target_os = "linux")]
            let info = if self.external_waits.iter().chain(&self.external_signals).any(|(_, v)| *v != 0) {
                info.next_timeline_semaphore_submit_info(&mut timeline)
            } else { info };
            let outcome = match &ended {
                Err(e) if lost_error(e) => { eprintln!("[hal-vulkan] batch recording: {e}"); Outcome::Lost }
                Err(e) => { eprintln!("[hal-vulkan] batch recording: {e}; its frame is dropped"); Outcome::Dropped }
                Ok(()) => {
                    crate::profile::since(crate::profile::Metric::SubmitQueue, self.finished_ns);
                    let at = if self.finished_ns != 0 { crate::clock_ns() } else { 0 };
                    let outcome = self.submit_until_room(&info, &mailbox);
                    crate::profile::since(crate::profile::Metric::QueueSubmit, at);
                    outcome
                }
            };
            #[cfg(target_os = "android")]
            if outcome == Outcome::Ran {
                if let Some(sem) = &self.native_signal {
                    match sem.export(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD) {
                        Ok(fd) => { if let Some((frame, _)) = self.native_frame.take() { frame.post(fd); } }
                        Err(e) => eprintln!("[hal-android-layer] fence export: {e}; posting after GPU completion"),
                    }
                }
            }
            #[cfg(target_os = "linux")]
            if outcome == Outcome::Ran {
                if let Some((sem, reply)) = &self.release_fence {
                    // Export only after queue submission. Retain the semaphore in the batch
                    // through its fence. If export fails, complete with a signaled release
                    // only after the GPU has actually finished.
                    match sem.export(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD) {
                        Ok(fd) => reply.send(Ok(fd)),
                        Err(error) => eprintln!("[hal-vulkan] release FD export failed; waiting on completion worker: {error}"),
                    }
                }
            }
            // a presenter that waits on the GPU itself (an XR compositor waits for the work
            // submitted before an image's release) takes its frame now, not at the fence
            if outcome == Outcome::Ran && self.scanout.as_ref().is_some_and(|s| s.0.at_queue()) {
                if let Some(scanout) = self.scanout.take() { hand_over(scanout, self.prof_key, mailbox.clone()); }
            }
            match self.present.take() {
                Some(present) if outcome == Outcome::Ran => {
                    // A frame of this window still waiting for an image is superseded by this one.
                    let waiting = lock(&present.chain.pending).take();
                    if let Some(waiting) = waiting { waiting(true); }
                    #[cfg(target_os = "android")]
                    self.hand_to_presenter(present, complete, mailbox);
                    #[cfg(not(target_os = "android"))]
                    self.show(present, SHOW_TRIES, later, complete, mailbox);
                }
                _ => self.deliver(outcome, true, complete, mailbox),
            }
        });
    }

    /// Submit worker: vkQueueSubmit. Out of memory is not a lost device: the host is asked for
    /// room (`Device::starved`, the backend raises GPU pressure) and the same submission is
    /// tried again -- a failed vkQueueSubmit leaves its command buffers and semaphores as they
    /// were -- while the queue keeps its order (later batches wait behind this one).
    fn submit_until_room(&self, info: &vk::SubmitInfo, mailbox: &Mailbox) -> Outcome {
        let mut tries = 0u64;
        loop {
            let result = { let _queue = lock(&self.dev.queue_lock); unsafe { self.dev.raw.queue_submit(self.dev.queue, std::slice::from_ref(info), self.fence) } };
            match result {
                Ok(()) => {
                    if tries > 0 { eprintln!("[hal-vulkan] submit: went through after {tries} tries out of GPU memory"); }
                    return Outcome::Ran;
                }
                Err(e) if out_of_memory(e) && !self.dev.lost.load(Ordering::Acquire) => {
                    if tries == 0 { eprintln!("[hal-vulkan] submit: {e:?}; asking for room and trying again"); }
                    tries += 1;
                    self.dev.refused("submit");
                    self.notify_sessions(mailbox);
                    std::thread::sleep(MEMORY_RETRY);
                }
                Err(e) => {
                    eprintln!("[hal-vulkan] submit: {e:?}");
                    return if e == vk::Result::ERROR_DEVICE_LOST || self.dev.lost.load(Ordering::Acquire) { Outcome::Lost } else { Outcome::Dropped };
                }
            }
        }
    }

    /// Queue worker: show the frame in its window (`Swapchain::show`). With no free swapchain
    /// image it waits on this worker, retried every SHOW_RETRY, without reporting the frame:
    /// its FrameDone follows the present (or the frame's end: superseded by a newer frame,
    /// swapchain replaced, worker closing, SHOW_TRIES exhausted -- then it is not presented).
    #[cfg(not(target_os = "android"))]
    fn show(self, present: Present, tries: u32, later: super::device::Later, complete: Arc<Worker>, mailbox: Mailbox) {
        // (profiled: the acquire, copy and present of one frame on the submit worker)
        let at = if crate::profile::on() { crate::clock_ns() } else { 0 };
        let shown = present.chain.show(&present.image);
        if at != 0 { crate::profile::since(crate::profile::Metric::ToPresent, at); }
        count(&shown);
        match ended(shown) {
            Some((outcome, shown)) => self.deliver(outcome, shown, complete, mailbox),
            None if tries <= 1 => self.deliver(Outcome::Ran, false, complete, mailbox),
            None => {
                let chain = present.chain.clone();
                let frame = present.frame;
                let retry_list = later.clone();
                *lock(&chain.pending) = Some(Box::new(move |give_up: bool| {
                    if give_up { self.deliver(Outcome::Ran, false, complete, mailbox); }
                    else { self.show(present, tries - 1, retry_list, complete, mailbox); }
                }));
                lock(&later).push((std::time::Instant::now() + SHOW_RETRY, Box::new(move |closing: bool| {
                    let waiting = lock(&chain.pending).take();
                    if let Some(waiting) = waiting {
                        waiting(closing || chain.latest.load(Ordering::Acquire) != frame);
                    }
                })));
            }
        }
    }

    /// Android: the frame goes to the present worker (`Present::presenter`), which shows it
    /// (`Swapchain::show` waits there for the frame's copy into the image, so this worker never
    /// waits for the GPU).
    #[cfg(target_os = "android")]
    fn hand_to_presenter(self, present: Present, complete: Arc<Worker>, mailbox: Mailbox) {
        present.chain.queued.store(present.frame, Ordering::Release);
        let presenter = present.presenter.clone();
        presenter.push(move || self.present_frame(present, SHOW_TRIES, complete, mailbox));
    }

    /// Present worker (Android): shows the frame, unless a frame handed on after it superseded
    /// it (it completes unshown, so a present worker that fell behind shows the newest frame
    /// next). With no free image it tries again SHOW_RETRY later, as a timed job of this worker,
    /// so a newer frame handed on meanwhile is not held up by it.
    #[cfg(target_os = "android")]
    fn present_frame(self, present: Present, tries: u32, complete: Arc<Worker>, mailbox: Mailbox) {
        if present.chain.queued.load(Ordering::Acquire) != present.frame {
            crate::profile::count("vk show: superseded");
            return self.deliver(Outcome::Ran, false, complete, mailbox);
        }
        // (profiled: the acquire, copy, its wait and the present of one frame)
        let at = if crate::profile::on() { crate::clock_ns() } else { 0 };
        let shown = present.chain.show(&present.image);
        if at != 0 { crate::profile::since(crate::profile::Metric::ToPresent, at); }
        count(&shown);
        match ended(shown) {
            Some((outcome, shown)) => self.deliver(outcome, shown, complete, mailbox),
            None if tries <= 1 => self.deliver(Outcome::Ran, false, complete, mailbox),
            None => {
                let later = present.presenter.later();
                lock(&later).push((std::time::Instant::now() + SHOW_RETRY, Box::new(move |closing: bool| {
                    if closing { self.deliver(Outcome::Ran, false, complete, mailbox); }
                    else { self.present_frame(present, tries - 1, complete, mailbox); }
                })));
            }
        }
    }

    /// The frame's completion: FrameDone and its events after the GPU fence. `shown == false`
    /// clears the window flag of its frame tickets (no presentation is reported). Only the
    /// driver's VK_ERROR_DEVICE_LOST marks the device lost; a wait out of memory waits again.
    fn deliver(mut self, outcome: Outcome, shown: bool, complete: Arc<Worker>, mailbox: Mailbox) {
        if !shown || outcome != Outcome::Ran {
            for t in &mut self.tickets { if let Ticket::Frame { window, .. } = t { *window = false; } }
        }
        complete.push(move || {
            let mut outcome = outcome;
            let mut tries = 0u64;
            let waited = std::time::Instant::now();
            let mut stall_said = false;
            while outcome == Outcome::Ran {
                match unsafe { self.dev.raw.wait_for_fences(&[self.fence], true, FENCE_SLICE_NS) } {
                    Ok(()) => break,
                    // Not an abort: the wait goes on. A batch the GPU has not finished long after
                    // its submit says why, once, instead of a silent wait behind which every later
                    // batch queues (the driver then spins in vkQueueSubmit).
                    Err(vk::Result::TIMEOUT) => {
                        if !stall_said && waited.elapsed() >= FENCE_STALL_SAY {
                            stall_said = true;
                            eprintln!("[hal-vulkan] GPU stall: a batch submitted {:.1} s ago has not finished; {}",
                                waited.elapsed().as_secs_f64(), self.stall_report());
                        }
                    }
                    Err(e) if out_of_memory(e) && !self.dev.lost.load(Ordering::Acquire) => {
                        if tries == 0 { eprintln!("[hal-vulkan] fence: {e:?}; asking for room and waiting again"); }
                        tries += 1;
                        self.dev.refused("fence wait");
                        self.notify_sessions(&mailbox);
                        std::thread::sleep(MEMORY_RETRY);
                    }
                    Err(e) => { eprintln!("[hal-vulkan] fence: {e:?}"); outcome = Outcome::Lost; }
                }
            }
            if stall_said { eprintln!("[hal-vulkan] GPU stall over: the batch finished {:.1} s after its submit", waited.elapsed().as_secs_f64()); }
            if outcome == Outcome::Lost { self.dev.lost.store(true, Ordering::Release); }
            self.complete(outcome != Outcome::Ran, mailbox);
        });
    }

    fn complete(mut self, failed: bool, mailbox: Mailbox) {
        crate::profile::since(crate::profile::Metric::SubmitToRendered, self.finished_ns);
        #[cfg(target_os = "linux")]
        if let Some((_, reply)) = &self.release_fence {
            reply.send(if failed { Err("Vulkan release submission failed".into()) } else { Ok(None) });
        }
        if let Some(prof) = self.prof.take() { prof.finish(&self.dev, self.query.0, failed); }
        let mut times = [0u64; 2];
        let timed = !failed && self.query.0 != vk::QueryPool::null()
            && unsafe { self.dev.raw.get_query_pool_results(self.query.0, 0, &mut times, vk::QueryResultFlags::TYPE_64) }.is_ok();
        let bits = self.dev.timestamp_bits;
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        let period = self.dev.properties.limits.timestamp_period as f64;
        let gpu_ns = if timed { ((times[1].wrapping_sub(times[0]) & mask) as f64 * period) as u64 } else { 0 };
        let gpu_span = if timed {
            let start = (times[0] & mask) as f64 * period * 1e-9;
            [start, start + gpu_ns as f64 * 1e-9]
        } else {
            [0.0; 2]
        };
        // Events are delivered only after every Vulkan object this batch kept alive is destroyed:
        // whoever observes an event (a test, a process about to exit) then cannot race the driver
        // calls of this worker, whose ICD exit handlers run concurrently with exit().
        let mut out: Vec<(Token, Event)> = Vec::new();
        let mut push = |token: Token, event: Event| out.push((token, event));
        for (token, name, query) in self.dispatch_times.drain(..) {
            let mut span = [0u64; 2];
            if timed && unsafe { self.dev.raw.get_query_pool_results(self.query.0, query, &mut span, vk::QueryResultFlags::TYPE_64) }.is_ok() {
                let gpu_ns = ((span[1].wrapping_sub(span[0]) & mask) as f64 * period) as u64;
                push(token, Event::DispatchTime { name, gpu_ns });
            }
        }
        for ticket in self.tickets.drain(..) {
            match ticket {
                Ticket::BufferLeases { token, req, ranges } => {
                    let status = if self.dev.lost.load(Ordering::Acquire) { readback::DEVICE_LOST }
                        else if failed { readback::FAILED }
                        else if ranges.iter().all(|(buffer, offset, len, _)| buffer.invalidate_range(*offset, *len).is_ok()) { readback::OK }
                        else { readback::FAILED };
                    let leases = if status == readback::OK { ranges.into_iter().map(|(_, _, _, lease)| lease).collect() } else { Vec::new() };
                    push(token, Event::BufferLeases { req, status, leases });
                }

                #[cfg(target_os = "linux")]
                Ticket::Capture { token, window, size, format, buffer } => {
                    if !failed {
                        match super::capture_gpu::completed(buffer, size, format) {
                            Ok(surface) => push(token, Event::Capture { window, surface }),
                            Err(e) => eprintln!("[hal-capture] {e}"),
                        }
                    }
                }
                Ticket::Frame { token, target, kind, frame, window } => {
                    // a frame that did not run (dropped, or its device was lost) is finished
                    // all the same, never presented: its app paints again (a dropped frame) or
                    // after DeviceReset (a lost device)
                    token.incarnation.done(frame);
                    push(token, Event::Frame { target, kind, frame, window: window && !failed, gpu_ns, gpu_span, ran: !failed });
                }
                Ticket::Copy { token, req, rect } => push(token, Event::Readback {
                    req, rect, status: if failed { readback::FAILED } else { readback::OK }, lease: None,
                }),
                Ticket::Shared { token, id, handle } => push(token, Event::Shared { id, handle: if failed { 0 } else { handle } }),
                Ticket::FrameCap { staging, len, done } => done(if failed { None } else { staging.read(len).ok() }),
                Ticket::Host { reply, image, staging } => {
                    let pixels = if failed { None } else { staging.read(image.desc.w as usize * image.desc.h as usize * 4).ok().map(|mut rgba| {
                        if image.desc.format == format::BGRA8 { for p in rgba.chunks_exact_mut(4) { p.swap(0, 2); } }
                        crate::backend::Pixels { w: image.desc.w, h: image.desc.h, rgba }
                    }) };
                    let _ = reply.try_send(pixels);
                }
                Ticket::Read { token, req, rect, format: fmt, staging, stencil } => {
                    let pixels = if failed { Err("GPU submission failed".into()) } else {
                        staging.read(format::level_bytes(fmt, rect[2], rect[3]) as usize)
                    };
                    if !failed { self.spares.readback_read(staging); }
                    match pixels {
                        Ok(mut pixels) => {
                            if let Some(stencil) = stencil {
                                match stencil.read(rect[2] as usize * rect[3] as usize) {
                                    Ok(values) => { for (p, s) in pixels.chunks_exact_mut(4).zip(values) { p[3] = s; } }
                                    Err(_) => { push(token, Event::Readback { req, rect, status: readback::FAILED, lease: None }); continue; }
                                }
                            }
                            if fmt == format::BGRA8 { for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); } }
                            let status = if matches!(fmt, format::RGBA8 | format::BGRA8) { readback::OK } else { readback::NATIVE };
                            push(token, Event::Readback { req, rect, status, lease: Some(crate::lease::Lease::from_vec(pixels, 0)) });
                        }
                        Err(e) => {
                            eprintln!("[hal-vulkan] readback: {e}");
                            push(token, Event::Readback { req, rect, status: readback::FAILED, lease: None });
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "android")]
        if !failed { if let Some((frame, _)) = self.native_frame.take() { frame.post(None); } }
        let scanout = self.scanout.take();
        let tap = self.tap.take().filter(|_| !failed);
        let self_prof_key = self.prof_key;
        // the fence signalled (or nothing was submitted): the objects may be reset and reused
        self.reusable = !failed;
        if self.reusable {
            self.buffers.clear();
            self.arena = None;
            let (released, wake) = {
                let mut arenas = lock(&self.spares.arenas);
                // A batch without a completion event can introduce the first idle deadline
                // after the pump went to sleep. Wake it only on that transition.
                let sleeping = out.is_empty() && arenas.trim_due().is_none();
                let released = arenas.give(self.arenas.drain(..).map(|(arena, _)| arena)
                    .filter(|arena| Arc::strong_count(arena) == 1), self.affinity, &self.keep);
                (released, sleeping && arenas.trim_due().is_some())
            };
            drop(released);
            if wake { (mailbox.notify)(self.arena_owner.as_ref().map_or(crate::session::HOST, |(t, _)| t.sid), false); }
        }
        drop(self);
        for (token, event) in out { mailbox.push(token, event); }
        if let Some((sid, window, image)) = tap { crate::frame_tap::offer(sid, window, &image); }
        if let Some(scanout) = scanout {
            if failed {
                scanout.0.dropped(scanout.6, scanout.5);
            } else {
                hand_over(scanout, self_prof_key, mailbox);
            }
        }
    }
}

/// A frame for a presenter: its image, and what reports it shown to its session.
fn hand_over(scanout: (Arc<dyn super::surface::FramePresenter>, Arc<Image>, Token, u32, Id, Id, u32, u64), prof_key: Option<crate::gpu_prof::FrameKey>, mailbox: Mailbox) {
    let (presenter, image, token, from, target, native, owner, frame) = scanout;
    presenter.submit(owner, native, from, image, Box::new(move |time| {
        // the presenter's time is when the frame reached the display
        if let Some(key) = prof_key { crate::gpu_prof::shown(key, time); }
        let sid = token.sid;
        lock(&mailbox.events).push(Done { token, time, event: Event::Presented { target, frame } });
        (mailbox.notify)(sid, true);
    }));
}

impl Drop for Batch {
    fn drop(&mut self) { unsafe {
        // made by `new`, whatever this one's end
        self.spares.in_flight.fetch_sub(1, Ordering::AcqRel);
        // a batch dropped before it completed (never submitted): its timed passes answer nothing
        if let Some(prof) = self.prof.take() { prof.finish(&self.dev, vk::QueryPool::null(), true); }
        for framebuffer in self.framebuffers.drain(..) { self.dev.raw.destroy_framebuffer(framebuffer, None); }
        let slot = Slot { dev: self.dev.clone(), pool: self.pool, cb: self.cb, fence: self.fence,
            query: if self.query.0 != vk::QueryPool::null() { self.query } else { self.idle_query } };
        if self.query.0 != vk::QueryPool::null() && self.idle_query.0 != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.idle_query.0, None); }
        if !self.reusable { crate::profile::count("vk batch slot destroyed (not reusable)"); return; } // dropping the slot destroys its objects
        let reset = self.dev.raw.reset_command_buffer(slot.cb, vk::CommandBufferResetFlags::default())
            .and_then(|_| self.dev.raw.reset_fences(&[slot.fence]));
        if reset.is_err() { crate::profile::count("vk batch slot destroyed (reset failed)"); return; }
        lock(&self.spares.slots).push_back((slot, None));
    } }
}
