//! GPU completion: each submitted batch (a frame, uploads, readbacks) ends with
//! `IDXGIDevice2::EnqueueSetEvent`, which flushes the context and signals an event when the GPU
//! has finished everything submitted so far. One completion thread waits for those events in
//! submission order (no polling), maps finished readbacks, releases what the batch kept alive
//! and posts the results to the mailbox the backend drains. Replaces the old backend's
//! per-frame event query poll (os/windows/d3d11.rs `is_gpu_done`, `d3d_poll_readback_leases`).

use super::device::{lock, Device};
use crate::font::FrameProgress;
use crate::types::*;
use makepad_windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use makepad_windows_sys::Win32::Graphics::Dxgi::IDXGIDevice2;
use makepad_windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// WaitForSingleObject's INFINITE timeout
const INFINITE: u32 = u32::MAX;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone)]
pub struct Token {
    pub sid: u32,
    pub incarnation: Arc<FrameProgress>,
}

pub enum Event {
    Frame { target: Id, kind: u32, frame: u64, window: bool, gpu_ns: u64 },
    Readback { req: u32, status: u32, rect: [u32; 4], pixels: Vec<u8> },
    Pick { req: u32, ids: Vec<u32> },
}

pub struct Done {
    pub token: Token,
    pub time: u64,
    pub event: Event,
}

#[derive(Clone)]
pub struct Mailbox {
    events: Arc<Mutex<Vec<Done>>>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl Mailbox {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { events: Arc::new(Mutex::new(Vec::new())), wake }
    }
    pub fn push(&self, token: Token, event: Event) {
        lock(&self.events).push(Done { token, event, time: crate::clock_ns() });
        (self.wake)();
    }
    pub fn take(&self) -> Vec<Done> {
        std::mem::take(&mut *lock(&self.events))
    }
}

/// A finished copy into a staging texture, mapped once the GPU is done.
pub struct Staged {
    pub tex: ID3D11Texture2D,
    pub format: u32,
    pub w: u32,
    pub h: u32,
}

/// A frame's GPU timing queries, reused through `Timers`.
pub struct Timer {
    pub disjoint: ID3D11Query,
    pub begin: ID3D11Query,
    pub end: ID3D11Query,
}

/// Timing query sets coming back from the completion thread for reuse.
#[derive(Clone, Default)]
pub struct Timers(Arc<Mutex<Vec<Timer>>>);

impl Timers {
    pub fn take(&self, dev: &Device) -> Option<Timer> {
        if let Some(t) = lock(&self.0).pop() {
            return Some(t);
        }
        let make = |q| {
            let mut out = None;
            unsafe { dev.device.CreateQuery(&D3D11_QUERY_DESC { Query: q, MiscFlags: 0 }, Some(&mut out)) }.ok()?;
            out
        };
        Some(Timer { disjoint: make(D3D11_QUERY_TIMESTAMP_DISJOINT)?, begin: make(D3D11_QUERY_TIMESTAMP)?, end: make(D3D11_QUERY_TIMESTAMP)? })
    }
    fn give(&self, t: Timer) {
        lock(&self.0).push(t);
    }
}

/// The GPU nanoseconds between a frame's two timestamps (0 when unknown or disjoint). The frame
/// is complete, so the data is there: read without flushing.
fn gpu_ns(dev: &Device, t: &Timer) -> u64 {
    unsafe {
        let get = |q: &ID3D11Query, data: *mut std::ffi::c_void, size: usize| {
            (dev.ctx.vtable().GetData)(dev.ctx.as_raw(), q.as_raw(), data, size as u32, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32).0 == 0
        };
        let mut dj = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
        let (mut b, mut e) = (0u64, 0u64);
        if !get(&t.disjoint, &mut dj as *mut _ as *mut _, std::mem::size_of_val(&dj)) || dj.Disjoint.as_bool() || dj.Frequency == 0 {
            return 0;
        }
        if !get(&t.begin, &mut b as *mut _ as *mut _, 8) || !get(&t.end, &mut e as *mut _ as *mut _, 8) || e < b {
            return 0;
        }
        ((e - b) as u128 * 1_000_000_000 / dj.Frequency as u128) as u64
    }
}

pub enum Ticket {
    Frame { token: Token, target: Id, kind: u32, frame: u64, window: bool, timer: Option<Timer> },
    Read { token: Token, req: u32, rect: [u32; 4], staged: Staged, pick: Option<[f32; 2]> },
    Copy { token: Token, req: u32, rect: [u32; 4] },
    Host { reply: std::sync::mpsc::SyncSender<Option<crate::backend::Pixels>>, staged: Staged },
}

/// What one submission produced: its tickets and the objects its commands use, held until the
/// GPU is done with them.
#[derive(Default)]
pub struct Batch {
    pub tickets: Vec<Ticket>,
    pub keep: Vec<Box<dyn std::any::Any + Send>>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.tickets.is_empty() && self.keep.is_empty()
    }
}

struct Pending {
    event: HANDLE,
    batch: Batch,
}

// SAFETY: an event HANDLE is a process-wide kernel object reference
unsafe impl Send for Pending {}

struct Queue {
    pending: VecDeque<Pending>,
    /// signalled events back for reuse (auto-reset: unsignalled again after the wait)
    free: Vec<usize>,
    stop: bool,
}

pub struct Completion {
    pub timers: Timers,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    dxgi: IDXGIDevice2,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Completion {
    pub fn new(dev: Arc<Device>, mailbox: Mailbox) -> Result<Self, String> {
        let dxgi = unsafe { IDXGIDevice2::query(dev.device.as_raw()) }.map_err(|e| format!("IDXGIDevice2 (DXGI 1.3): {e}"))?;
        let queue = Arc::new((Mutex::new(Queue { pending: VecDeque::new(), free: Vec::new(), stop: false }), Condvar::new()));
        let q = queue.clone();
        let timers = Timers::default();
        let t = timers.clone();
        let thread = std::thread::Builder::new()
            .name("makepad-d3d11-complete".into())
            .spawn(move || run(dev, q, mailbox, t))
            .map_err(|e| format!("completion thread: {e}"))?;
        Ok(Self { timers, queue, dxgi, thread: Some(thread) })
    }

    /// Ends a batch: flushes the context and queues its completion. Never waits.
    pub fn submit(&self, batch: Batch) {
        if batch.is_empty() {
            return;
        }
        let (m, cv) = &*self.queue;
        let event = lock(m).free.pop().map(|h| HANDLE(h as _));
        let event = match event {
            Some(e) => e,
            None => match unsafe { CreateEventW(None, false, false, None) } {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("[hal-d3d11] CreateEventW: {e}; completing the batch without a fence");
                    HANDLE(std::ptr::null_mut())
                }
            },
        };
        if !event.0.is_null() {
            if let Err(e) = unsafe { self.dxgi.EnqueueSetEvent(event) } {
                eprintln!("[hal-d3d11] EnqueueSetEvent: {e}");
            }
        }
        lock(m).pending.push_back(Pending { event, batch });
        cv.notify_one();
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        let (m, cv) = &*self.queue;
        lock(m).stop = true;
        cv.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for h in lock(m).free.drain(..) {
            unsafe {
                let _ = CloseHandle(HANDLE(h as _));
            }
        }
    }
}

fn run(dev: Arc<Device>, queue: Arc<(Mutex<Queue>, Condvar)>, mailbox: Mailbox, timers: Timers) {
    let (m, cv) = &*queue;
    loop {
        let front = {
            let mut q = lock(m);
            loop {
                if let Some(p) = q.pending.pop_front() {
                    break Some(p);
                }
                if q.stop {
                    break None;
                }
                q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(Pending { event, batch }) = front else { return };
        if !event.0.is_null() {
            // GPU work is finite; a removed device signals too (its work is discarded)
            let wait = unsafe { WaitForSingleObject(event, INFINITE) };
            if wait != WAIT_OBJECT_0 {
                eprintln!("[hal-d3d11] completion wait: {wait:?}");
            }
            lock(m).free.push(event.0 as usize);
        }
        let lost = dev.check_lost();
        complete(&dev, batch, lost, &mailbox, &timers);
    }
}

fn read_staged(dev: &Device, s: &Staged) -> Option<Vec<u8>> {
    let row = crate::types::format::level_bytes(s.format, s.w, 1) as usize;
    let rows = if crate::types::format::is_block(s.format) { s.h.div_ceil(4) } else { s.h } as usize;
    let row = if crate::types::format::is_block(s.format) { crate::types::format::level_bytes(s.format, s.w, 4) as usize } else { row };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        // the batch is complete (its event fired); some drivers still answer WAS_STILL_DRAWING to a
        // non-waiting map, so this thread maps waiting (it never holds anything else)
        dev.ctx.Map(s.tex.as_ID3D11Resource(), 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
        let mut out = vec![0u8; row * rows];
        for y in 0..rows {
            let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
            std::ptr::copy_nonoverlapping(src, out.as_mut_ptr().add(y * row), row);
        }
        dev.ctx.Unmap(s.tex.as_ID3D11Resource(), 0);
        Some(out)
    }
}

/// BGRA texels as the RGBA every 8-bit readback answers in.
fn to_rgba(format: u32, pixels: &mut [u8]) {
    if format == crate::types::format::BGRA8 {
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
    }
}

fn complete(dev: &Device, mut batch: Batch, lost: bool, mailbox: &Mailbox, timers: &Timers) {
    let mut out: Vec<(Token, Event)> = Vec::new();
    for ticket in batch.tickets.drain(..) {
        match ticket {
            Ticket::Frame { token, target, kind, frame, window, timer } => {
                token.incarnation.done(frame);
                let gpu_ns = match timer {
                    Some(t) => {
                        let ns = if lost { 0 } else { gpu_ns(dev, &t) };
                        timers.give(t);
                        ns
                    }
                    None => 0,
                };
                // a frame of a lost device still finishes (nothing reached the glass): the app's
                // frame accounting closes, the reset that follows makes it paint again
                out.push((token, Event::Frame { target, kind, frame, window: window && !lost, gpu_ns }));
            }
            Ticket::Copy { token, req, rect } => {
                out.push((token, Event::Readback { req, rect, status: if lost { readback::FAILED } else { readback::OK }, pixels: Vec::new() }));
            }
            Ticket::Host { reply, staged } => {
                let pixels = if lost { None } else {
                    read_staged(dev, &staged).map(|mut rgba| {
                        to_rgba(staged.format, &mut rgba);
                        crate::backend::Pixels { w: staged.w, h: staged.h, rgba }
                    })
                };
                let _ = reply.try_send(pixels);
            }
            Ticket::Read { token, req, rect, staged, pick } => {
                let pixels = if lost { None } else { read_staged(dev, &staged) };
                match (pixels, pick) {
                    (Some(mut pixels), Some(pos)) => {
                        to_rgba(staged.format, &mut pixels);
                        // ids nearest the point first (the Metal/Vulkan pick neighbourhood order)
                        let mut ordered: Vec<(f32, u32)> = pixels
                            .chunks_exact(4)
                            .enumerate()
                            .map(|(i, px)| {
                                let x = rect[0] as f32 + (i as u32 % rect[2]) as f32;
                                let y = rect[1] as f32 + (i as u32 / rect[2]) as f32;
                                ((x - pos[0]).powi(2) + (y - pos[1]).powi(2), u32::from_le_bytes(px.try_into().unwrap()) & 0x00ff_ffff)
                            })
                            .collect();
                        ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                        let mut ids = Vec::new();
                        for (_, id) in ordered {
                            if id != 0 && !ids.contains(&id) {
                                ids.push(id);
                            }
                        }
                        out.push((token, Event::Pick { req, ids }));
                    }
                    (Some(mut pixels), None) => {
                        to_rgba(staged.format, &mut pixels);
                        let status = if matches!(staged.format, format::RGBA8 | format::BGRA8) { readback::OK } else { readback::NATIVE };
                        out.push((token, Event::Readback { req, rect, status, pixels }));
                    }
                    (None, Some(_)) => out.push((token, Event::Pick { req, ids: Vec::new() })),
                    (None, None) => out.push((token, Event::Readback { req, rect, status: readback::FAILED, pixels: Vec::new() })),
                }
            }
        }
    }
    // what the batch used goes before anyone hears it finished
    drop(batch);
    if lost {
        dev.lost.store(true, Ordering::Release);
    }
    for (token, event) in out {
        mailbox.push(token, event);
    }
}
