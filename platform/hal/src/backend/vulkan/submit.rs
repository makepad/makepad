//! One command buffer per frame (plus independent upload/readback batches). The submission
//! worker owns queue access; a separate completion worker waits fences and reads timestamps.

use super::device::{error, lock, Device, Worker};
use super::pipeline::{Pipeline, Program, RenderPass};
use super::resources::{buffer_usage, Buffer, Image};
use super::surface::{Present, Shown};
use crate::font::FrameProgress;
use crate::types::*;
use makepad_vulkan::vk;
use std::collections::{HashMap, HashSet};
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
    Lost,
}
pub(super) struct Done { pub token: Token, pub time: u64, pub event: Event }

#[derive(Clone)]
pub(super) struct Mailbox { events: Arc<Mutex<Vec<Done>>>, pub wake: Arc<dyn Fn() + Send + Sync> }
impl Mailbox {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self { Self { events: Arc::new(Mutex::new(Vec::new())), wake } }
    pub fn push(&self, token: Token, event: Event) {
        lock(&self.events).push(Done { token, event, time: crate::clock_ns() }); (self.wake)();
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
}

/// How often a frame that found no free swapchain image tries again, and how many tries it gets
/// (about one second) before it completes unshown.
const SHOW_RETRY: std::time::Duration = std::time::Duration::from_millis(8);
const SHOW_TRIES: u32 = 125;

/// The Vulkan objects of a finished batch, reset for the next one. A command pool, fence,
/// timestamp pool, descriptor pools and a small arena made and destroyed per batch cost about a
/// millisecond each way (NVIDIA); a page of live effects submits dozens of target frames a beat.
pub(super) struct Slot {
    dev: Arc<Device>,
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    query: vk::QueryPool,
    /// Per layout: the pool and how many sets it holds.
    descriptors: HashMap<vk::DescriptorSetLayout, (vk::DescriptorPool, u32)>,
    arena: Option<Arc<Buffer>>,
}

impl Drop for Slot {
    fn drop(&mut self) { unsafe {
        for (_, (pool, _)) in self.descriptors.drain() { self.dev.raw.destroy_descriptor_pool(pool, None); }
        if self.query != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.query, None); }
        self.dev.raw.destroy_fence(self.fence, None);
        self.dev.raw.destroy_command_pool(self.pool, None);
    } }
}

/// The reset slots of one backend, shared with its batches in flight (each returns its own).
pub(super) type Spares = Arc<Mutex<Vec<Slot>>>;
/// More frames than this in flight at once are not kept for reuse.
const SPARE_SLOTS: usize = 256;
/// A larger arena (an upload, a frame of big inline instance data) is freed, not kept: the
/// spares stay a few megabytes at most.
const SPARE_ARENA_BYTES: u64 = 256 * 1024;

pub(super) struct Batch {
    pub dev: Arc<Device>,
    spares: Spares,
    /// Set once the GPU finished with this batch: its objects may be reset and reused.
    reusable: bool,
    pool: vk::CommandPool,
    pub cb: vk::CommandBuffer,
    fence: vk::Fence,
    query: vk::QueryPool,
    /// The slot's timestamp pool when this batch writes no timestamps (kept for the next one).
    idle_query: vk::QueryPool,
    pub images: Vec<Arc<Image>>,
    pub buffers: Vec<Arc<Buffer>>,
    pub pipelines: Vec<Arc<Pipeline>>,
    pub passes: Vec<Arc<RenderPass>>,
    pub framebuffers: Vec<vk::Framebuffer>,
    /// Per layout: the pool, how many sets it holds, and whether this batch sized it (a reused
    /// pool is checked against this batch's draws once).
    descriptors: HashMap<vk::DescriptorSetLayout, (vk::DescriptorPool, u32, bool)>,
    pub tickets: Vec<Ticket>,
    pub scanout: Option<(Arc<dyn super::surface::FramePresenter>, Arc<Image>, Token, Id, Id, u32, u64)>,
    pub present: Option<Present>,
    arena: Option<(Arc<Buffer>, u64)>,
    arenas: Vec<Arc<Buffer>>,
    reserve: u64,
    pub draw_counts: HashMap<usize, u32>,
    #[cfg(target_os = "linux")]
    pub external_waits: Vec<(Arc<super::share::sync::Semaphore>, u64)>,
    #[cfg(target_os = "linux")]
    pub external_signals: Vec<(Arc<super::share::sync::Semaphore>, u64)>,
    #[cfg(target_os = "linux")]
    pub release_fence: Option<(Arc<super::share::sync::Semaphore>, crate::dmabuf::FenceReply)>,
}

impl Batch {
    pub fn new(dev: Arc<Device>, spares: &Spares, timestamp: bool, reserve: u64) -> Result<Self, String> {
        let spare = lock(spares).pop();
        let mut b = Self { dev, spares: spares.clone(), reusable: false, pool: vk::CommandPool::null(), cb: vk::CommandBuffer::null(), fence: vk::Fence::null(),
            query: vk::QueryPool::null(), idle_query: vk::QueryPool::null(),
            images: Vec::new(), buffers: Vec::new(), pipelines: Vec::new(), passes: Vec::new(), framebuffers: Vec::new(),
            descriptors: HashMap::new(), tickets: Vec::new(), scanout: None, present: None, arena: None, arenas: Vec::new(), reserve, draw_counts: HashMap::new(),
            #[cfg(target_os = "linux")] external_waits: Vec::new(),
            #[cfg(target_os = "linux")] external_signals: Vec::new(),
            #[cfg(target_os = "linux")] release_fence: None };
        if let Some(mut slot) = spare {
            // the slot's objects move into this batch; the emptied slot destroys nothing
            b.pool = std::mem::replace(&mut slot.pool, vk::CommandPool::null());
            b.cb = slot.cb;
            b.fence = std::mem::replace(&mut slot.fence, vk::Fence::null());
            b.idle_query = std::mem::replace(&mut slot.query, vk::QueryPool::null());
            b.descriptors = slot.descriptors.drain().map(|(layout, (pool, sets))| (layout, (pool, sets, false))).collect();
            if let Some(arena) = slot.arena.take().filter(|arena| arena.size >= reserve.max(16)) {
                b.arena = Some((arena.clone(), 0));
                b.arenas.push(arena);
            }
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
            if timestamp && b.dev.timestamp_bits > 0 {
                b.query = std::mem::replace(&mut b.idle_query, vk::QueryPool::null());
                if b.query == vk::QueryPool::null() {
                    b.query = b.dev.raw.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2), None).map_err(error)?;
                }
                b.dev.raw.cmd_reset_query_pool(b.cb, b.query, 0, 2);
                b.dev.raw.cmd_write_timestamp(b.cb, vk::PipelineStageFlags::TOP_OF_PIPE, b.query, 0);
            }
        }
        Ok(b)
    }

    pub fn put(&mut self, bytes: &[u8]) -> Result<(Arc<Buffer>, u64), String> {
        // Sixteen also satisfies the byte alignment of HAL's largest texel/compression block.
        let align = self.dev.properties.limits.min_uniform_buffer_offset_alignment.max(self.dev.properties.limits.min_storage_buffer_offset_alignment).max(16);
        let len = bytes.len().max(16) as u64;
        let available = self.arena.as_ref().map(|(b, used)| (b.clone(), used.div_ceil(align) * align));
        let (buffer, at) = match available.filter(|(b, at)| at.checked_add(len).is_some_and(|end| end <= b.size)) {
            Some(loc) => loc,
            None => {
                let b = Buffer::new(self.dev.clone(), self.reserve.max(len), buffer_usage())?;
                self.arenas.push(b.clone()); (b, 0)
            }
        };
        buffer.write_unflushed(at, bytes)?;
        self.arena = Some((buffer.clone(), at + len));
        Ok((buffer, at))
    }

    pub fn descriptors(&mut self, p: &Program, state: usize) -> Result<vk::DescriptorSet, String> {
        // a reused pool that holds fewer sets than this batch draws with the layout is replaced
        if let Some((pool, sets, sized)) = self.descriptors.get_mut(&p.descriptors).filter(|(_, _, sized)| !*sized) {
            let draws = *self.draw_counts.get(&state).ok_or("pipeline missing from frame descriptor counts")?;
            *sized = true;
            if *sets < draws {
                unsafe { self.dev.raw.destroy_descriptor_pool(*pool, None); }
                self.descriptors.remove(&p.descriptors);
            }
        }
        let pool = if let Some((pool, _, _)) = self.descriptors.get(&p.descriptors) { *pool } else {
            let draws = *self.draw_counts.get(&state).ok_or("pipeline missing from frame descriptor counts")?;
            let mut counts: HashMap<vk::DescriptorType, u32> = HashMap::new();
            for (_, ty) in &p.descriptor_types { let count = counts.entry(*ty).or_default(); *count = count.saturating_add(draws); }
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
        let later = submit.later();
        let ended = (|| unsafe {
            for arena in &self.arenas { arena.flush()?; }
            // A shared target or imported producer image is handed back after every use.
            // FrameDone/Readback/TextureShared are delivered only after this release finishes.
            let mut external = HashSet::new();
            for image in &self.images {
                if image.external.is_some() && external.insert(image.image) {
                    #[cfg(target_os = "linux")]
                    if let Some(timeline) = image.timeline.as_ref().filter(|t| t.producer) {
                        let (wait, signal) = timeline.submitted()?;
                        self.external_waits.push((timeline.semaphore.clone(), wait));
                        self.external_signals.push((timeline.semaphore.clone(), signal));
                    }
                    image.release_external(self.cb);
                }
            }
            if self.query != vk::QueryPool::null() { self.dev.raw.cmd_write_timestamp(self.cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, self.query, 1); }
            self.dev.raw.end_command_buffer(self.cb).map_err(error)
        })();
        submit.push(move || {
            let cb = [self.cb];
            let waits: Vec<vk::Semaphore> = Vec::new();
            let signals: Vec<vk::Semaphore> = Vec::new();
            let stages: Vec<vk::PipelineStageFlags> = Vec::new();
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
            let submitted = ended.and_then(|_| { let _queue = lock(&self.dev.queue_lock); unsafe { self.dev.raw.queue_submit(self.dev.queue, &[info], self.fence) }.map_err(error) });
            #[cfg(target_os = "linux")]
            if submitted.is_ok() {
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
            let failed = submitted.is_err();
            if let Err(e) = &submitted { eprintln!("[hal-vulkan] submit: {e}"); }
            let submitted = submitted.is_ok();
            match self.present.take() {
                Some(present) if !failed => {
                    // A frame of this window still waiting for an image is superseded by this one.
                    let waiting = lock(&present.chain.pending).take();
                    if let Some(waiting) = waiting { waiting(true); }
                    self.show(present, SHOW_TRIES, later, complete, mailbox);
                }
                _ => self.deliver(submitted, failed, true, complete, mailbox),
            }
        });
    }

    /// Queue worker: show the frame in its window (`Swapchain::show`). With no free swapchain
    /// image it waits on this worker, retried every SHOW_RETRY, without reporting the frame:
    /// its FrameDone follows the present (or the frame's end: superseded by a newer frame,
    /// swapchain replaced, worker closing, SHOW_TRIES exhausted -- then it is not presented).
    fn show(self, present: Present, tries: u32, later: super::device::Later, complete: Arc<Worker>, mailbox: Mailbox) {
        match present.chain.show(&present.image) {
            Ok(Shown::Presented) => self.deliver(true, false, true, complete, mailbox),
            Ok(Shown::Dropped) => self.deliver(true, false, false, complete, mailbox),
            Err(e) => { eprintln!("[hal-vulkan] present: {e}"); self.deliver(true, true, false, complete, mailbox) }
            Ok(Shown::NotReady) if tries <= 1 => self.deliver(true, false, false, complete, mailbox),
            Ok(Shown::NotReady) => {
                let chain = present.chain.clone();
                let frame = present.frame;
                let retry_list = later.clone();
                *lock(&chain.pending) = Some(Box::new(move |give_up: bool| {
                    if give_up { self.deliver(true, false, false, complete, mailbox); }
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
    /// clears the window flag of its frame tickets (no presentation is reported).
    fn deliver(mut self, submitted: bool, mut failed: bool, shown: bool, complete: Arc<Worker>, mailbox: Mailbox) {
        if !shown {
            for t in &mut self.tickets { if let Ticket::Frame { window, .. } = t { *window = false; } }
        }
        complete.push(move || {
            if submitted {
                if let Err(e) = unsafe { self.dev.raw.wait_for_fences(&[self.fence], true, u64::MAX) } {
                    eprintln!("[hal-vulkan] fence: {e:?}"); failed = true;
                }
            }
            if failed { self.dev.lost.store(true, Ordering::Release); }
            self.complete(failed, mailbox);
        });
    }

    fn complete(mut self, failed: bool, mailbox: Mailbox) {
        #[cfg(target_os = "linux")]
        if let Some((_, reply)) = &self.release_fence {
            reply.send(if failed { Err("Vulkan release submission failed".into()) } else { Ok(None) });
        }
        let mut times = [0u64; 2];
        let timed = !failed && self.query != vk::QueryPool::null()
            && unsafe { self.dev.raw.get_query_pool_results(self.query, 0, &mut times, vk::QueryResultFlags::TYPE_64) }.is_ok();
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
                    token.incarnation.done(frame);
                    if failed { push(token, Event::Lost); }
                    else { push(token, Event::Frame { target, kind, frame, window, gpu_ns }); }
                }
                Ticket::Copy { token, req, rect } => push(token, Event::Readback {
                    req, rect, status: if failed { readback::FAILED } else { readback::OK }, pixels: Vec::new(),
                }),
                Ticket::Shared { token, id, handle } => push(token, Event::Shared { id, handle: if failed { 0 } else { handle } }),
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
        let scanout = self.scanout.take();
        // the fence signalled (or nothing was submitted): the objects may be reset and reused
        self.reusable = !failed;
        drop(self);
        for (token, event) in out { mailbox.push(token, event); }
        if !failed {
            if let Some((presenter, image, token, target, native, owner, frame)) = scanout {
                presenter.submit(owner, native, image, Box::new(move |time| {
                    lock(&mailbox.events).push(Done { token, time, event: Event::Presented { target, frame } });
                    (mailbox.wake)();
                }));
            }
        }
    }
}

impl Drop for Batch {
    fn drop(&mut self) { unsafe {
        for framebuffer in self.framebuffers.drain(..) { self.dev.raw.destroy_framebuffer(framebuffer, None); }
        let mut slot = Slot { dev: self.dev.clone(), pool: self.pool, cb: self.cb, fence: self.fence,
            query: if self.query != vk::QueryPool::null() { self.query } else { self.idle_query },
            descriptors: self.descriptors.drain().map(|(layout, (pool, sets, _))| (layout, (pool, sets))).collect(), arena: None };
        if self.query != vk::QueryPool::null() && self.idle_query != vk::QueryPool::null() { self.dev.raw.destroy_query_pool(self.idle_query, None); }
        if !self.reusable { return; } // dropping the slot destroys its objects
        // the arena goes back once nothing else of this batch holds it
        self.buffers.clear();
        self.arena = None;
        slot.arena = self.arenas.drain(..).filter(|arena| Arc::strong_count(arena) == 1 && arena.bytes <= SPARE_ARENA_BYTES).max_by_key(|arena| arena.size);
        let reset = self.dev.raw.reset_command_buffer(slot.cb, vk::CommandBufferResetFlags::default())
            .and_then(|_| self.dev.raw.reset_fences(&[slot.fence]))
            .and_then(|_| slot.descriptors.values().try_for_each(|(pool, _)| self.dev.raw.reset_descriptor_pool(*pool, vk::DescriptorPoolResetFlags::default())));
        if reset.is_err() { return; }
        let mut spares = lock(&self.spares);
        if spares.len() < SPARE_SLOTS { spares.push(slot); }
    } }
}
