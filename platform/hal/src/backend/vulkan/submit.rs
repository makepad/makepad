//! One command buffer per frame (plus independent upload/readback batches). The submission
//! worker owns queue access; a separate completion worker waits fences and reads timestamps.

use super::device::{error, lock, Device, Worker};
use super::pipeline::{Pipeline, Program, RenderPass};
use super::resources::{buffer_usage, Buffer, Image};
use super::surface::{Present, Shown};
use crate::font::FrameProgress;
use crate::types::*;
use makepad_vulkan::vk;
use crate::FastMap;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct Token { pub sid: u32, pub incarnation: Arc<FrameProgress> }
pub(super) enum Event {
    #[cfg(target_os = "linux")]
    Capture { window: Id, surface: Arc<crate::backend::linux::media::capture_surface::Surface> },
    Presented { target: Id, frame: u64 },
    Frame { target: Id, kind: u32, frame: u64, window: bool, gpu_ns: u64 },
    Readback { req: u32, status: u32, rect: [u32; 4], pixels: Vec<u8> },
    Pick { req: u32, ids: Vec<u32> },
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
    pub fn take(&self) -> Vec<Done> { std::mem::take(&mut *lock(&self.events)) }
}

pub(super) enum Ticket {
    #[cfg(target_os = "linux")]
    Capture { token: Token, window: Id, size: [u32;2], format: u32, buffer: Arc<Buffer> },
    Frame { token: Token, target: Id, kind: u32, frame: u64, window: bool },
    Read { token: Token, req: u32, rect: [u32; 4], format: u32, staging: Arc<Buffer>, stencil: Option<Arc<Buffer>>, pick: Option<[f32; 2]> },
    Shared { token: Token, id: Id, handle: u64 },
    Copy { token: Token, req: u32, rect: [u32; 4] },
    Host { reply: std::sync::mpsc::SyncSender<Option<crate::backend::Pixels>>, image: Arc<Image>, staging: Arc<Buffer> },
    /// Frame capture: a texture's storage, every level and layer (`Gpu::capture_texture`).
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

/// How often a frame that found no free swapchain image tries again, and how many tries it gets
/// (about one second) before it completes unshown.
const SHOW_RETRY: std::time::Duration = std::time::Duration::from_millis(8);
const SHOW_TRIES: u32 = 125;
/// The completion worker waits for a batch's fence in slices of this long, and says once what a
/// batch still unfinished after FENCE_STALL_SAY waits on (the wait itself never gives up).
const FENCE_SLICE_NS: u64 = 250_000_000;
const FENCE_STALL_SAY: std::time::Duration = std::time::Duration::from_secs(2);

/// The Vulkan objects of a finished batch, reset for the next one. A command pool, fence,
/// timestamp pool and descriptor pools made and destroyed per batch cost about a millisecond
/// each way (NVIDIA); a page of live effects submits dozens of target frames a beat.
pub(super) struct Slot {
    dev: Arc<Device>,
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    /// the timestamp pool and how many queries it holds (a profiled frame's batch grew it)
    query: (vk::QueryPool, u32),
    /// Per layout: the pool and how many sets it holds.
    descriptors: FastMap<vk::DescriptorSetLayout, (vk::DescriptorPool, u32)>,
}

impl Drop for Slot {
    fn drop(&mut self) { unsafe {
        for (_, (pool, _)) in self.descriptors.drain() { self.dev.raw.destroy_descriptor_pool(pool, None); }
        if self.query.0 != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.query.0, None); }
        self.dev.raw.destroy_fence(self.fence, None);
        self.dev.raw.destroy_command_pool(self.pool, None);
    } }
}

/// What one backend's finished batches leave for the next ones: the reset slots and the upload
/// arenas no batch holds. Shared with its batches in flight (each returns its own).
#[derive(Default)]
pub(super) struct Spare {
    slots: Mutex<Vec<Slot>>,
    arenas: Mutex<Arenas>,
    /// batches made and not yet dropped (completed or abandoned): at `IN_FLIGHT_MAX` the
    /// renderer waits for room (`Batch::new`) instead of recording ahead of the submit worker
    in_flight: Mutex<usize>,
    room: std::sync::Condvar,
}
pub(super) type Spares = Arc<Spare>;
/// More frames than this in flight at once are not kept for reuse.
const SPARE_SLOTS: usize = 256;
/// Batches recorded and not yet completed at once; the renderer waits for room at this many
/// (every target frame of a beat is a batch: a window frame with its effect passes is a few).
/// Without the bound a submit worker slower than the renderer queued seconds of frames, and the
/// backlog fed itself: every batch took a fresh slot, descriptor pools and arena (a Snapdragon
/// 685 swipe: 1,439 slots made in 27 s, none reused, each frame shown 21 s after its recording).
const IN_FLIGHT_MAX: usize = 8;
/// The longest one wait for room: the worker may itself wait on the host freeing GPU memory
/// (`submit_until_room`), which needs the renderer running.
const IN_FLIGHT_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

/// The smallest upload arena; arenas grow by powers of two up to `ARENA_MAX`.
const ARENA_MIN: u64 = 256 * 1024;
/// The largest arena a batch asks for: a frame with more upload bytes spreads them over several
/// arenas. Only a single upload larger than this gets an arena of its own size.
const ARENA_MAX: u64 = 8 << 20;
/// Free arena bytes kept for the next batches; beyond it the largest free arenas are released.
const ARENA_KEEP: u64 = 64 << 20;
/// A free arena no batch took for this long is released (the upload volume went down).
const ARENA_IDLE: std::time::Duration = std::time::Duration::from_secs(5);

/// The size of a new arena for `need` bytes: a power of two in [ARENA_MIN, ARENA_MAX], or a
/// single upload larger than ARENA_MAX rounded up to whole ARENA_MIN.
fn arena_size(need: u64) -> u64 {
    if need > ARENA_MAX { need.div_ceil(ARENA_MIN) * ARENA_MIN } else { need.next_power_of_two().clamp(ARENA_MIN, ARENA_MAX) }
}

/// Upload arenas (host-visible buffers for uniforms, inline instances and upload staging) whose
/// batch's fence signalled. A batch takes them back instead of allocating: making one costs a
/// vkAllocateMemory + map (NVIDIA: milliseconds for megabytes), and frames with more upload bytes
/// than the old one-small-spare-per-slot made one every frame.
#[derive(Default)]
struct Arenas {
    free: Vec<(Arc<Buffer>, std::time::Instant)>,
    bytes: u64,
}

impl Arenas {
    /// The smallest free arena of at least `want` bytes, else the largest that holds `len`.
    fn take(&mut self, want: u64, len: u64) -> Option<Arc<Buffer>> {
        let fits = |min: u64| self.free.iter().enumerate().filter(move |(_, (a, _))| a.size >= min);
        let i = fits(want).min_by_key(|(_, (a, _))| a.size).or_else(|| fits(len).max_by_key(|(_, (a, _))| a.size)).map(|(i, _)| i)?;
        let (arena, _) = self.free.swap_remove(i);
        self.bytes -= arena.bytes;
        Some(arena)
    }

    /// Every free arena, to release (the device ran out of memory for a new one).
    fn release_all(&mut self) -> Vec<Arc<Buffer>> {
        self.bytes = 0;
        self.free.drain(..).map(|(arena, _)| arena).collect()
    }

    /// Takes back a finished batch's arenas; returns those to release (idle ones, then the
    /// largest while over ARENA_KEEP) for the caller to free outside the lock.
    fn give(&mut self, arenas: impl Iterator<Item = Arc<Buffer>>) -> Vec<Arc<Buffer>> {
        let now = std::time::Instant::now();
        for arena in arenas { self.bytes += arena.bytes; self.free.push((arena, now)); }
        let mut released = Vec::new();
        let mut i = 0;
        while i < self.free.len() {
            if now.duration_since(self.free[i].1) > ARENA_IDLE {
                let (arena, _) = self.free.swap_remove(i);
                self.bytes -= arena.bytes; released.push(arena);
            } else { i += 1; }
        }
        while self.bytes > ARENA_KEEP {
            let Some(i) = self.free.iter().enumerate().max_by_key(|(_, (a, _))| a.size).map(|(i, _)| i) else { break };
            let (arena, _) = self.free.swap_remove(i);
            self.bytes -= arena.bytes; released.push(arena);
        }
        released
    }
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
    pub images: Vec<Arc<Image>>,
    pub buffers: Vec<Arc<Buffer>>,
    pub pipelines: Vec<Arc<Pipeline>>,
    pub passes: Vec<Arc<RenderPass>>,
    pub framebuffers: Vec<vk::Framebuffer>,
    /// Per layout: the pool, how many sets it holds, and whether this batch sized it (a reused
    /// pool is checked against this batch's draws once).
    descriptors: FastMap<vk::DescriptorSetLayout, (vk::DescriptorPool, u32, bool)>,
    pub tickets: Vec<Ticket>,
    pub scanout: Option<(Arc<dyn super::surface::FramePresenter>, Arc<Image>, Token, Id, Id, u32, u64)>,
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
    arenas: Vec<Arc<Buffer>>,
    /// the upload bytes this batch still expects (the frame's uniforms and inline instances):
    /// sizes the next arena it needs
    reserve: u64,
    /// arenas this batch had to allocate (none in steady frames): the frame stats' `arena_allocs`
    pub allocs: u64,
    pub draw_counts: FastMap<usize, u32>,
    /// The sets this batch wrote, by what they hold (`descriptor_set`): a draw with the same
    /// buffers, images and samplers binds one again with its own dynamic offsets.
    sets: FastMap<u64, (vk::DescriptorSet, Vec<u64>)>,
    set_key: Vec<u64>,
    /// The uniform blocks this pass already uploaded, per binding role (`put_block`).
    blocks: [(usize, usize, vk::Buffer, u64); 8],
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
                Ticket::Frame { token, .. } | Ticket::Read { token, .. } | Ticket::Shared { token, .. } | Ticket::Copy { token, .. } => token.sid,
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
        {
            let mut n = lock(&spares.in_flight);
            if *n >= IN_FLIGHT_MAX {
                crate::profile::count("vk batch waited for room");
                let waited = std::time::Instant::now();
                while *n >= IN_FLIGHT_MAX {
                    let (guard, timeout) = spares.room.wait_timeout(n, IN_FLIGHT_WAIT.saturating_sub(waited.elapsed())).unwrap_or_else(|e| e.into_inner());
                    n = guard;
                    if timeout.timed_out() && *n >= IN_FLIGHT_MAX { crate::profile::count("vk batch waited for room: timed out"); break; }
                }
            }
            *n += 1;
        }
        let spare = lock(&spares.slots).pop();
        if spare.is_none() { crate::profile::count("vk batch slot made"); }
        let mut b = Self { dev, spares: spares.clone(), reusable: false, pool: vk::CommandPool::null(), cb: vk::CommandBuffer::null(), fence: vk::Fence::null(),
            query: (vk::QueryPool::null(), 0), idle_query: (vk::QueryPool::null(), 0), prof: None, prof_key: None,
            images: Vec::new(), buffers: Vec::new(), pipelines: Vec::new(), passes: Vec::new(), framebuffers: Vec::new(),
            descriptors: FastMap::default(), tickets: Vec::new(), scanout: None, present: None, tap: None, arena: None, arenas: Vec::new(), reserve, allocs: 0, draw_counts: FastMap::default(),
            #[cfg(target_os = "android")] native_frame: None,
            #[cfg(target_os = "android")] native_signal: None,
            sets: FastMap::default(), set_key: Vec::new(), blocks: [(0, 0, vk::Buffer::null(), 0); 8], finished_ns: 0,
            #[cfg(target_os = "linux")] external_waits: Vec::new(),
            #[cfg(target_os = "linux")] external_signals: Vec::new(),
            #[cfg(target_os = "linux")] release_fence: None,
            #[cfg(target_os = "linux")] hosted_frames: Vec::new() };
        if let Some(mut slot) = spare {
            // the slot's objects move into this batch; the emptied slot destroys nothing
            b.pool = std::mem::replace(&mut slot.pool, vk::CommandPool::null());
            b.cb = slot.cb;
            b.fence = std::mem::replace(&mut slot.fence, vk::Fence::null());
            b.idle_query = std::mem::replace(&mut slot.query, (vk::QueryPool::null(), 0));
            b.descriptors = slot.descriptors.drain().map(|(layout, (pool, sets))| (layout, (pool, sets, false))).collect();
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
            if queries > 0 && b.dev.timestamp_bits > 0 {
                let queries = queries.max(2);
                b.query = std::mem::replace(&mut b.idle_query, (vk::QueryPool::null(), 0));
                if b.query.1 < queries {
                    // a profiled frame needs more than the slot's pool holds: a larger one, kept
                    // with the slot after (sized in powers of two, so steady frames reuse it)
                    if b.query.0 != vk::QueryPool::null() { b.dev.raw.destroy_query_pool(b.query.0, None); }
                    let count = queries.next_power_of_two();
                    b.query = (vk::QueryPool::null(), 0);
                    b.query = (b.dev.raw.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(count), None).map_err(error)?, count);
                }
                b.dev.raw.cmd_reset_query_pool(b.cb, b.query.0, 0, queries);
                b.dev.raw.cmd_write_timestamp(b.cb, vk::PipelineStageFlags::TOP_OF_PIPE, b.query.0, 0);
            }
        }
        Ok(b)
    }

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

    pub fn put(&mut self, bytes: &[u8]) -> Result<(Arc<Buffer>, u64), String> {
        // Sixteen also satisfies the byte alignment of HAL's largest texel/compression block.
        let align = self.dev.properties.limits.min_uniform_buffer_offset_alignment.max(self.dev.properties.limits.min_storage_buffer_offset_alignment).max(16);
        let len = bytes.len().max(16) as u64;
        let available = self.arena.as_ref().map(|(b, used)| (b.clone(), used.div_ceil(align) * align));
        let (buffer, at) = match available.filter(|(b, at)| at.checked_add(len).is_some_and(|end| end <= b.size)) {
            Some(loc) => loc,
            None => {
                // the rest of the frame's uploads in one arena when it fits under ARENA_MAX
                let want = arena_size(self.reserve.min(ARENA_MAX).max(len));
                let reused = lock(&self.spares.arenas).take(want, len);
                let b = match reused {
                    Some(b) => b,
                    None => {
                        self.allocs += 1;
                        crate::profile::count("upload arena allocs");
                        match Buffer::scratch(self.dev.clone(), want, buffer_usage()) {
                            Ok(b) => b,
                            // out of memory: the pool's free arenas go first (and the host is
                            // asked for room), then one more try
                            Err(e) if super::pipeline::out_of_memory(&e) => {
                                let released = lock(&self.spares.arenas).release_all();
                                drop(released);
                                self.dev.refused("upload arena");
                                Buffer::scratch(self.dev.clone(), want, buffer_usage())?
                            }
                            Err(e) => return Err(e),
                        }
                    }
                };
                self.arenas.push(b.clone()); (b, 0)
            }
        };
        buffer.write_unflushed(at, bytes)?;
        self.arena = Some((buffer.clone(), at + len));
        self.reserve = self.reserve.saturating_sub(len.div_ceil(align) * align);
        Ok((buffer, at))
    }

    /// Upload a uniform block for the binding role `which`, unless this pass's previous upload
    /// for the role was the same bytes (the pass block, a draw list's block): those are shared.
    pub fn put_block(&mut self, which: usize, bytes: &[u8]) -> Result<(vk::Buffer, u64), String> {
        let memo = (bytes.as_ptr() as usize, bytes.len());
        if let Some((ptr, len, buffer, at)) = self.blocks.get(which).copied() {
            if (ptr, len) == memo && buffer != vk::Buffer::null() { return Ok((buffer, at)); }
        }
        let (buffer, at) = self.put(bytes)?;
        if let Some(slot) = self.blocks.get_mut(which) { *slot = (memo.0, memo.1, buffer.buffer, at); }
        Ok((buffer.buffer, at))
    }

    /// A new pass's command data: no uploaded block is known to match it.
    pub fn forget_blocks(&mut self) { self.blocks = [(0, 0, vk::Buffer::null(), 0); 8]; }

    /// The set for a draw's bindings: one this batch already wrote with the same buffers,
    /// images and samplers (`false`: nothing to write), or a new one to write (`true`).
    pub fn descriptor_set(&mut self, p: &Program, state: usize, bins: &[(u32, vk::DescriptorType, vk::DescriptorBufferInfo)],
        images: &[(u32, vk::DescriptorType, vk::DescriptorImageInfo)]) -> Result<(vk::DescriptorSet, bool), String> {
        let mut key = std::mem::take(&mut self.set_key);
        key.clear();
        key.push(p.descriptors.0);
        for (slot, ty, info) in bins { key.extend([*slot as u64 | (ty.0 as u64) << 32, info.buffer.0, info.offset, info.range]); }
        for (slot, ty, info) in images { key.extend([*slot as u64 | (ty.0 as u64) << 32, info.sampler.0, info.image_view.0, info.image_layout.0 as u64]); }
        let hash = key.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ *v).wrapping_mul(0x0100_0000_01b3).rotate_left(23));
        let found = self.sets.get(&hash).filter(|(_, k)| *k == key).map(|(set, _)| *set);
        let out = match found {
            Some(set) => Ok((set, false)),
            None => self.descriptors(p, state).map(|set| {
                // a hash two different keys share keeps its first set; the second is not reused
                if !self.sets.contains_key(&hash) { self.sets.insert(hash, (set, key.clone())); }
                (set, true)
            }),
        };
        self.set_key = key;
        out
    }

    fn descriptors(&mut self, p: &Program, state: usize) -> Result<vk::DescriptorSet, String> {
        // a reused pool that holds fewer sets than this batch draws with the layout is replaced
        if let Some((pool, sets, sized)) = self.descriptors.get_mut(&p.descriptors).filter(|(_, _, sized)| !*sized) {
            let draws = *self.draw_counts.get(&state).ok_or("pipeline missing from frame descriptor counts")?;
            *sized = true;
            if *sets < draws {
                crate::profile::count("vk descriptor pool too small: remade");
                unsafe { self.dev.raw.destroy_descriptor_pool(*pool, None); }
                self.descriptors.remove(&p.descriptors);
            }
        }
        let pool = if let Some((pool, _, _)) = self.descriptors.get(&p.descriptors) { *pool } else {
            // Sized to the next power of two: a slot's pool serves the frames after this one, and a
            // frame drawing one more than its pool holds remade it (a destroy and a create, both
            // driver round trips) every few frames as a view's draw count wandered.
            let draws = self.draw_counts.get(&state).ok_or("pipeline missing from frame descriptor counts")?.next_power_of_two();
            let mut counts: FastMap<vk::DescriptorType, u32> = FastMap::default();
            // (a combined image sampler through a Y'CbCr conversion may take up to three
            // descriptors: combinedImageSamplerDescriptorCount)
            for (_, ty) in &p.descriptor_types {
                let per = if *ty == vk::DescriptorType::COMBINED_IMAGE_SAMPLER { 3 } else { 1 };
                let count = counts.entry(*ty).or_default(); *count = count.saturating_add(draws.saturating_mul(per));
            }
            let sizes: Vec<_> = counts.into_iter().map(|(ty, count)| vk::DescriptorPoolSize::default().ty(ty).descriptor_count(count)).collect();
            let pool = unsafe { self.dev.raw.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(draws).pool_sizes(&sizes), None) }.map_err(error)?;
            self.descriptors.insert(p.descriptors, (pool, draws, true)); pool
        };
        let layouts = [p.descriptors];
        // one set, written in place (the wrapper returns a Vec per call)
        let allocate = self.dev.raw.table.allocate_descriptor_sets.ok_or("vkAllocateDescriptorSets is unavailable")?;
        let mut set = vk::DescriptorSet::null();
        unsafe { allocate(self.dev.raw.handle(), &vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts), &mut set) }.result().map_err(error)?;
        Ok(set)
    }

    pub fn finish(mut self, submit: &Worker, complete: Arc<Worker>, mailbox: Mailbox) {
        if crate::profile::on() { self.finished_ns = crate::clock_ns(); }
        let later = submit.later();
        #[cfg(target_os = "linux")]
        let mut published: Vec<(Arc<super::share::sync::Timeline>, u64)> = Vec::new();
        let ended = (|| unsafe {
            for arena in &self.arenas { arena.flush()?; }
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
    fn show(self, present: Present, tries: u32, later: super::device::Later, complete: Arc<Worker>, mailbox: Mailbox) {
        // (profiled: the acquire, copy and present of one frame on the submit worker)
        let at = if crate::profile::on() { crate::clock_ns() } else { 0 };
        let shown = present.chain.show(&present.image);
        if at != 0 { crate::profile::since(crate::profile::Metric::ToPresent, at); }
        crate::profile::count(match &shown {
            Ok(Shown::Presented) => "vk show: presented",
            Ok(Shown::Dropped) => "vk show: dropped",
            Ok(Shown::NotReady) => "vk show: no free image (retried)",
            Err(_) => "vk show: error",
        });
        match shown {
            Ok(Shown::Presented) => self.deliver(Outcome::Ran, true, complete, mailbox),
            Ok(Shown::Dropped) => self.deliver(Outcome::Ran, false, complete, mailbox),
            // the frame ran; only its presentation failed (lost only if the driver says so)
            Err(e) => {
                eprintln!("[hal-vulkan] present: {e}");
                self.deliver(if lost_error(&e) { Outcome::Lost } else { Outcome::Ran }, false, complete, mailbox)
            }
            Ok(Shown::NotReady) if tries <= 1 => self.deliver(Outcome::Ran, false, complete, mailbox),
            Ok(Shown::NotReady) => {
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
        let gpu_ns = if timed { ((times[1].wrapping_sub(times[0]) & mask) as f64 * self.dev.properties.limits.timestamp_period as f64) as u64 } else { 0 };
        // Events are delivered only after every Vulkan object this batch kept alive is destroyed:
        // whoever observes an event (a test, a process about to exit) then cannot race the driver
        // calls of this worker, whose ICD exit handlers run concurrently with exit().
        let mut out: Vec<(Token, Event)> = Vec::new();
        let mut push = |token: Token, event: Event| out.push((token, event));
        for ticket in self.tickets.drain(..) {
            match ticket {
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
                    push(token, Event::Frame { target, kind, frame, window: window && !failed, gpu_ns });
                }
                Ticket::Copy { token, req, rect } => push(token, Event::Readback {
                    req, rect, status: if failed { readback::FAILED } else { readback::OK }, pixels: Vec::new(),
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
                Ticket::Read { token, req, rect, format: fmt, staging, stencil, pick } => {
                    let pixels = if failed { Err("GPU submission failed".into()) } else {
                        staging.read(format::level_bytes(fmt, rect[2], rect[3]) as usize)
                    };
                    match pixels {
                        Ok(mut pixels) => {
                            if let Some(stencil) = stencil {
                                match stencil.read(rect[2] as usize * rect[3] as usize) {
                                    Ok(values) => { for (p, s) in pixels.chunks_exact_mut(4).zip(values) { p[3] = s; } }
                                    Err(_) => { push(token, Event::Readback { req, rect, status: readback::FAILED, pixels: Vec::new() }); continue; }
                                }
                            }
                            if fmt == format::BGRA8 { for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); } }
                            if let Some(pos) = pick {
                                let mut ids = Vec::new();
                                let mut ordered: Vec<_> = pixels.chunks_exact(4).enumerate().map(|(i, px)| {
                                    let x = rect[0] as f32 + (i as u32 % rect[2]) as f32;
                                    let y = rect[1] as f32 + (i as u32 / rect[2]) as f32;
                                    ((x - pos[0]).powi(2) + (y - pos[1]).powi(2), u32::from_le_bytes(px.try_into().unwrap()) & 0x00ff_ffff)
                                }).collect();
                                ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                                for (_, id) in ordered {
                                    if id != 0 && !ids.contains(&id) { ids.push(id); }
                                }
                                push(token, Event::Pick { req, ids });
                            } else {
                                let status = if matches!(fmt, format::RGBA8 | format::BGRA8) { readback::OK } else { readback::NATIVE };
                                push(token, Event::Readback { req, rect, status, pixels });
                            }
                        }
                        Err(e) => {
                            eprintln!("[hal-vulkan] readback: {e}");
                            push(token, if pick.is_some() { Event::Pick { req, ids: Vec::new() } }
                                else { Event::Readback { req, rect, status: readback::FAILED, pixels: Vec::new() } });
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
        drop(self);
        for (token, event) in out { mailbox.push(token, event); }
        if let Some((sid, window, image)) = tap { crate::frame_tap::offer(sid, window, &image); }
        if let Some(scanout) = scanout {
            if failed {
                scanout.0.dropped(scanout.5, scanout.4);
            } else {
                hand_over(scanout, self_prof_key, mailbox);
            }
        }
    }
}

/// A frame for a presenter: its image, and what reports it shown to its session.
fn hand_over(scanout: (Arc<dyn super::surface::FramePresenter>, Arc<Image>, Token, Id, Id, u32, u64), prof_key: Option<crate::gpu_prof::FrameKey>, mailbox: Mailbox) {
    let (presenter, image, token, target, native, owner, frame) = scanout;
    presenter.submit(owner, native, image, Box::new(move |time| {
        // the presenter's time is when the frame reached the display
        if let Some(key) = prof_key { crate::gpu_prof::shown(key, time); }
        let sid = token.sid;
        lock(&mailbox.events).push(Done { token, time, event: Event::Presented { target, frame } });
        (mailbox.notify)(sid, true);
    }));
}

impl Drop for Batch {
    fn drop(&mut self) { unsafe {
        // room for the renderer's next batch, whatever this one's end
        { let mut n = lock(&self.spares.in_flight); *n = n.saturating_sub(1); }
        self.spares.room.notify_all();
        // a batch dropped before it completed (never submitted): its timed passes answer nothing
        if let Some(prof) = self.prof.take() { prof.finish(&self.dev, vk::QueryPool::null(), true); }
        for framebuffer in self.framebuffers.drain(..) { self.dev.raw.destroy_framebuffer(framebuffer, None); }
        let slot = Slot { dev: self.dev.clone(), pool: self.pool, cb: self.cb, fence: self.fence,
            query: if self.query.0 != vk::QueryPool::null() { self.query } else { self.idle_query },
            descriptors: self.descriptors.drain().map(|(layout, (pool, sets, _))| (layout, (pool, sets))).collect() };
        if self.query.0 != vk::QueryPool::null() && self.idle_query.0 != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.idle_query.0, None); }
        if !self.reusable { crate::profile::count("vk batch slot destroyed (not reusable)"); return; } // dropping the slot destroys its objects
        // the arenas go back once nothing else holds them (freed here, outside the lock, if the
        // pool is full)
        self.buffers.clear();
        self.arena = None;
        let released = lock(&self.spares.arenas).give(self.arenas.drain(..).filter(|arena| Arc::strong_count(arena) == 1));
        drop(released);
        let reset = self.dev.raw.reset_command_buffer(slot.cb, vk::CommandBufferResetFlags::default())
            .and_then(|_| self.dev.raw.reset_fences(&[slot.fence]))
            .and_then(|_| slot.descriptors.values().try_for_each(|(pool, _)| self.dev.raw.reset_descriptor_pool(*pool, vk::DescriptorPoolResetFlags::default())));
        if reset.is_err() { crate::profile::count("vk batch slot destroyed (reset failed)"); return; }
        let mut slots = lock(&self.spares.slots);
        if slots.len() < SPARE_SLOTS { slots.push(slot); } else { crate::profile::count("vk batch slot destroyed (spares full)"); }
    } }
}
