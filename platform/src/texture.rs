use {
    crate::{
        cx::Cx, id_pool::*, makepad_error_log::*, makepad_math::*, makepad_script::*,
        os::CxOsTexture, script::vm::*,
    },
    std::{
        rc::Rc,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
    },
};

/// Upload decoded images as mipmapped textures (`VecMipBGRAu8_32`) so minifying them on low-DPI
/// screens uses a mip chain instead of aliasing into a blocky look. Only helps when the source
/// has detail over the display size. Default on for OpenGL only; override with `MAKEPAD_IMAGE_MIPMAPS`.
pub fn image_cache_use_mipmaps() -> bool {
    if let Ok(v) = std::env::var("MAKEPAD_IMAGE_MIPMAPS") {
        return matches!(v.trim(), "1" | "true" | "on" | "yes");
    }
    // The platform crate can see the `use_vulkan` cfg the draw crate cannot, so the gate lives here.
    cfg!(all(target_os = "linux", not(use_vulkan)))
}

/// A shared, reusable texture handle. GPU storage is allocated lazily by rendering.
///
/// [`Texture::release`] retires the current allocation, including through every
/// clone of this handle. The handle and its format/CPU pixels remain valid; the
/// next upload or render allocates fresh storage. Render contents are lost and
/// `InitWith` clears again. Release after removing retained readers/producers:
/// a retained pass that still uses this handle may allocate it again.
///
/// Lifetime serials belong to one `Cx`, start at zero, and count submitted GPU
/// command buffers (Metal), render passes (GL/D3D11/WebGL), or software frames
/// (headless), not Draw events. Recording a pass is not submitting it. Sample
/// `Cx::frame_submission_serial` after rendering, then acknowledge only serials
/// at or below `Cx::frame_completion_serial`. Completion of N covers every
/// submission <= N on the renderer's ordered queue. These are completion, not
/// successful-rendering, guarantees; a device error can discard work.
///
/// Metal uses command-buffer completion handlers. GL uses zero-timeout sync
/// fences and D3D11 uses EVENT queries. No backend guesses a frame delay.
/// Poll completion while work is pending, including when no redraw is needed:
/// it inserts at most one outstanding fence and reclaims retired allocations.
/// Unavailable/failed fences leave the completed serial unchanged. Headless
/// completes synchronously. WebGL deletion safely delegates in-flight ownership
/// to the browser/driver, but this Rust bridge cannot report GPU completion or
/// actual allocation sizes: its completion serial stays zero and allocated
/// bytes return `None` (pool totals omit these unknown allocations). Do not use
/// WebGL pool totals for admission. Vulkan/OHOS/direct-DRM support is not implemented.
///
/// Byte counts describe allocated texture storage, including mip levels, cube
/// faces and known backend capacity, not CPU source pixels, upload staging,
/// driver metadata or total VRAM residency. Metal reports `allocatedSize`;
/// GL/D3D report texel storage (opaque driver padding is not queryable).
/// Headless reports its actual float raster/conversion buffers. Pool totals
/// include reusable free slots, previous resources and pending retirements.
/// Call completion polling before measuring to collect finished retirements.
/// GL measurements require this renderer's context to be current; otherwise
/// they return `None`. Release before changing dimensions to keep the old
/// allocation charged during reallocation. Command-buffer-only copies created
/// by ordinary implicit reallocations are not separately counted by the pool.
/// Native retirement conservatively covers the latest queue buffer, including
/// a Metal batch still being encoded, which can be later than the last reader.
/// Shared/video/external formats are managed by their owners and release is a
/// no-op for those formats.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture(Rc<PoolId>);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct TextureId(pub(crate) usize, u64);

/// A request accepted by one `Cx`. Tickets are never reused by that context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReadbackTicket(pub u64);

/// Capture the pending producer, or copy the last rendered allocation when
/// there is no dirty producer. `next_render` explicitly waits for the next
/// execution of the attachment's pass (it does not request a repaint).
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadbackRequest {
    pub next_render: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadbackError {
    UnsupportedBackend,
    UnsupportedFormat,
    InvalidSize,
    NotRendered,
    Backpressure,
    Cancelled,
    AllocationChanged,
    DeviceLost,
    Failed,
}

impl std::fmt::Display for ReadbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "texture readback: {self:?}")
    }
}
impl std::error::Error for ReadbackError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadbackChannelOrder { Bgra, Rgba }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadbackOrigin { TopLeft, BottomLeft }

/// Raw, unmodified UNORM8 attachment bytes, including the attachment's alpha
/// convention. An error is a terminal result for the ticket, with no payload.
#[derive(Debug)]
pub struct TextureReadback {
    pub ticket: ReadbackTicket,
    pub allocation_generation: u64,
    /// Actual producing submission, in this Cx's renderer serial domain.
    pub producer_serial: u64,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub channel_order: ReadbackChannelOrder,
    pub origin: ReadbackOrigin,
    pub data: Result<Arc<[u8]>, ReadbackError>,
}

/// Per-context limits. Admission reserves staging AND result capacity before
/// returning a ticket. Keep/retry a rejected request after draining results.
pub const TEXTURE_READBACK_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const TEXTURE_READBACK_MAX_REQUESTS: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
pub struct TextureReadbackUsage {
    pub requests: usize,
    /// Includes queued, in-flight and undelivered results. Each reservation
    /// covers a staging allocation and, independently, its CPU result.
    pub reserved_bytes: usize,
}

pub(crate) struct ReadbackSlot {
    pub legacy: bool,
    pub texture: Texture,
    pub pass: Option<crate::draw_pass::DrawPassId>,
    pub pass_generation: Option<u64>,
    pub next_render: bool,
    pub previous_serial: u64,
    pub pending: bool,
    pub cancelled: bool,
    pub reserved_bytes: usize,
    pub result: TextureReadback,
    pub receive: Option<std::sync::mpsc::Receiver<Result<Arc<[u8]>, ReadbackError>>>,
}

#[derive(Default)]
pub(crate) struct TextureReadbacks {
    pub next_ticket: u64,
    pub slots: Vec<ReadbackSlot>,
    pub reserved_bytes: usize,
}

/// A one-shot completion sender. Capacity is reserved per ticket; unrelated
/// completions can never fill it. A dropped sender becomes an explicit error
/// when the owning renderer polls the receiver.
pub(crate) struct ReadbackCompletion(
    pub std::sync::mpsc::SyncSender<Result<Arc<[u8]>, ReadbackError>>,
);

impl ReadbackCompletion {
    pub fn finish(self, result: Result<Arc<[u8]>, ReadbackError>) {
        // There is exactly one producer and one message in this channel. The
        // only possible send failure is teardown of the owning Cx.
        let _ = self.0.try_send(result);
        crate::thread::SignalToUI::set_ui_signal();
    }
}

pub(crate) struct ReadbackWork {
    pub ticket: ReadbackTicket,
    pub texture_id: TextureId,
    pub width: usize,
    pub height: usize,
    pub reserved_bytes: usize,
    pub completion: ReadbackCompletion,
}

impl Cx {
    pub(crate) fn validate_pending_readbacks(&mut self) {
        for index in 0..self.textures.1.readbacks.slots.len() {
            let slot = &self.textures.1.readbacks.slots[index];
            if !slot.pending { continue; }
            let id = slot.texture.texture_id();
            let texture = &self.textures[id];
            let error = if texture.allocation_generation > slot.result.allocation_generation
                || (texture.allocation_generation == slot.result.allocation_generation && texture.alloc.is_none()) {
                Some(ReadbackError::AllocationChanged)
            } else if let Some(pass) = slot.pass {
                let valid = self.passes.0.pool.get(pass.0).is_some_and(|entry| Some(entry.generation) == slot.pass_generation)
                    && !self.passes.0.is_free(pass.0)
                    && self.passes[pass].color_textures.iter().any(|attachment| attachment.texture.texture_id() == id);
                if !valid { Some(ReadbackError::Cancelled) }
                else if !slot.next_render && !self.passes[pass].paint_dirty && texture.producer_serial == slot.previous_serial {
                    Some(ReadbackError::NotRendered)
                } else { None }
            } else { None };
            if let Some(error) = error {
                let slot = &mut self.textures.1.readbacks.slots[index];
                slot.pending = false;
                slot.result.data = Err(error);
            }
        }
    }

    #[cfg(not(headless))]
    pub(crate) fn readback_pass_submitted(&mut self, pass: crate::draw_pass::DrawPassId, serial: u64) {
        for color in &self.passes[pass].color_textures {
            self.textures[color.texture.texture_id()].producer_serial = serial;
        }
    }

    pub(crate) fn take_readback_work(&mut self, pass: Option<crate::draw_pass::DrawPassId>, order: ReadbackChannelOrder, origin: ReadbackOrigin) -> Vec<ReadbackWork> {
        let mut work = Vec::new();
        for index in 0..self.textures.1.readbacks.slots.len() {
            let slot = &self.textures.1.readbacks.slots[index];
            if !slot.pending || slot.pass != pass { continue; }
            let id = slot.texture.texture_id();
            let texture = &self.textures[id];
            let error = if texture.allocation_generation != slot.result.allocation_generation {
                Some(ReadbackError::AllocationChanged)
            } else if texture.alloc.as_ref().is_none_or(|alloc| alloc.width != slot.result.width || alloc.height != slot.result.height)
                || texture.producer_serial == 0 || (pass.is_some() && texture.producer_serial <= slot.previous_serial) {
                Some(ReadbackError::NotRendered)
            } else { None };
            let serial = texture.producer_serial;
            let slot = &mut self.textures.1.readbacks.slots[index];
            slot.pending = false;
            if let Some(error) = error {
                slot.result.data = Err(error);
                crate::thread::SignalToUI::set_ui_signal();
                continue;
            }
            slot.result.producer_serial = serial;
            slot.result.channel_order = order;
            slot.result.origin = origin;
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            slot.receive = Some(receive);
            work.push(ReadbackWork { ticket: slot.result.ticket, texture_id: id, width: slot.result.width, height: slot.result.height,
                reserved_bytes: slot.reserved_bytes, completion: ReadbackCompletion(send) });
        }
        work
    }

    #[cfg(not(headless))]
    pub(crate) fn fail_pending_readbacks(&mut self, error: ReadbackError) {
        for slot in &mut self.textures.1.readbacks.slots {
            if slot.pending { slot.pending = false; slot.result.data = Err(error); }
        }
        crate::thread::SignalToUI::set_ui_signal();
    }
}

#[cfg(all(not(headless), any(use_vulkan, linux_direct, target_env = "ohos")))]
impl Cx {
    pub(crate) fn poll_texture_readbacks(&mut self) {
        self.fail_pending_readbacks(ReadbackError::UnsupportedBackend);
    }
}

#[cfg(all(not(headless), not(any(use_vulkan, linux_direct, target_env = "ohos")), any(target_os = "linux", target_os = "android", target_os = "windows")))]
pub(crate) type ReadbackCopyJob = Box<dyn FnOnce() + Send>;

/// One lazy, long-lived worker per renderer. It copies mapped leases and
/// wakes the renderer to poll GPU fences, but never calls a graphics API.
#[cfg(all(not(headless), not(any(use_vulkan, linux_direct, target_env = "ohos")), any(target_os = "linux", target_os = "android", target_os = "windows")))]
pub(crate) struct ReadbackWorker {
    send: std::sync::mpsc::SyncSender<Option<ReadbackCopyJob>>,
    active: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(all(not(headless), not(any(use_vulkan, linux_direct, target_env = "ohos")), any(target_os = "linux", target_os = "android", target_os = "windows")))]
impl ReadbackWorker {
    pub fn new(cx: &Cx) -> Result<Self, ReadbackError> {
        use std::sync::{atomic::AtomicBool, mpsc};
        let (send, receive) = mpsc::sync_channel::<Option<ReadbackCopyJob>>(TEXTURE_READBACK_MAX_REQUESTS + 1);
        let active = Arc::new(AtomicBool::new(false));
        let running = active.clone();
        cx.thread_spawner().spawn_worker(crate::thread::ThreadOptions::default(), move || {
            loop {
                let message = if running.load(Ordering::Acquire) {
                    receive.recv_timeout(std::time::Duration::from_millis(4))
                } else { receive.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) };
                match message {
                    Ok(Some(job)) => {
                        // A failed copy drops its one-shot sender, which the
                        // renderer converts to a terminal error. Keep the
                        // worker available for the remaining leases.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        crate::thread::SignalToUI::set_ui_signal();
                    }
                    Ok(None) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => crate::thread::SignalToUI::set_ui_signal(),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }).map_err(|_| ReadbackError::Failed)?.detach();
        Ok(Self { send, active })
    }

    pub fn set_active(&self, active: bool) {
        let was_active = self.active.swap(active, Ordering::AcqRel);
        if active && !was_active {
            // If full, queued jobs already wake the worker.
            let _ = self.send.try_send(None);
        }
    }

    /// Return the owned job on backpressure so the renderer retains its lease.
    pub fn try_copy(&self, job: ReadbackCopyJob) -> Result<(), (ReadbackCopyJob, bool)> {
        match self.send.try_send(Some(job)) {
            Ok(()) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(Some(job))) => Err((job, false)),
            Err(std::sync::mpsc::TrySendError::Disconnected(Some(job))) => Err((job, true)),
            _ => unreachable!(),
        }
    }
}

/// Only called by a copy worker while its backend retains the mapped resource.
#[cfg(all(not(headless), not(any(use_vulkan, linux_direct, target_env = "ohos")), any(target_os = "linux", target_os = "android", target_os = "windows")))]
pub(crate) unsafe fn copy_readback_rows(address: usize, pitch: usize, width: usize, height: usize) -> Arc<[u8]> {
    let mut bytes = Arc::<[u8]>::new_uninit_slice(width * height * 4);
    let dst = Arc::get_mut(&mut bytes).unwrap().as_mut_ptr().cast::<u8>();
    for y in 0..height {
        std::ptr::copy_nonoverlapping((address as *const u8).add(y * pitch), dst.add(y * width * 4), width * 4);
    }
    bytes.assume_init()
}

impl TextureId {
    pub(crate) fn from_pool_slot(index: usize, generation: u64) -> Self {
        Self(index, generation)
    }
}

impl Default for TextureId {
    /// Returns a sentinel `TextureId` that does not correspond to any allocated texture.
    /// Used for audio-only players that carry no video output.
    fn default() -> Self {
        TextureId(usize::MAX, 0)
    }
}

impl Texture {
    pub fn texture_id(&self) -> TextureId {
        TextureId(self.0.id, self.0.generation)
    }

    /// Queue a full RenderBGRAu8 attachment without waiting for the GPU. The
    /// request follows its pending producer, or issues only an ordered copy
    /// when already rendered. Other formats return UnsupportedFormat.
    ///
    /// Admission reserves bounded staging/result bytes and pins this handle.
    /// Backpressure accepts no ticket: retain the request and retry after
    /// draining results. Every accepted ticket yields exactly one result,
    /// including cancellation, allocation changes and device errors. Poll
    /// `Cx::try_take_texture_readbacks` on Event::Signal. No redraw is needed
    /// for a copy of an existing allocation.
    pub fn read_back(&self, cx: &mut Cx, request: ReadbackRequest) -> Result<ReadbackTicket, ReadbackError> {
        let id = self.texture_id();
        let TextureFormat::RenderBGRAu8 { size, .. } = &cx.textures[id].format else {
            return Err(ReadbackError::UnsupportedFormat);
        };
        let producer = cx.passes.id_iter().find(|pass| {
            !cx.passes.0.is_free(pass.0) && cx.passes[*pass].color_textures.iter()
                .any(|attachment| attachment.texture.texture_id() == id)
        });
        let pending_pass = producer.filter(|pass| request.next_render || cx.passes[*pass].paint_dirty);
        if request.next_render && producer.is_none() {
            return Err(ReadbackError::NotRendered);
        }
        let (width, height) = match size {
            TextureSize::Fixed { width, height } => (*width, *height),
            TextureSize::Auto => {
                if let Some(pass) = pending_pass {
                    let dpi = cx.passes[pass].dpi_factor.unwrap_or(1.0);
                    let rect = cx.get_pass_rect(pass, dpi).ok_or(ReadbackError::InvalidSize)?;
                    ((rect.size.x * dpi) as usize, (rect.size.y * dpi) as usize)
                } else {
                    let alloc = cx.textures[id].alloc.as_ref().ok_or(ReadbackError::NotRendered)?;
                    (alloc.width, alloc.height)
                }
            }
        };
        if width == 0 || height == 0 || width > i32::MAX as usize || height > i32::MAX as usize {
            return Err(ReadbackError::InvalidSize);
        }
        let row = width.checked_mul(4).ok_or(ReadbackError::InvalidSize)?;
        // Covers row padding through 4 KiB on native staging textures. Drivers
        // with a larger mapped pitch fail explicitly before copying memory.
        let reserved_bytes = row.checked_add(4095).and_then(|row| (row & !4095).checked_mul(height))
            .ok_or(ReadbackError::InvalidSize)?;
        let texture = &cx.textures[id];
        let reallocate = texture.alloc != texture.format.as_render_alloc(width, height);
        if pending_pass.is_none() && (reallocate || texture.producer_serial == 0) {
            return Err(ReadbackError::NotRendered);
        }
        let generation = texture.allocation_generation.saturating_add(u64::from(reallocate));
        let producer_serial = if pending_pass.is_some() { 0 } else { texture.producer_serial };
        let previous_serial = texture.producer_serial;
        let pass_generation = pending_pass.map(|pass| cx.passes.0.pool[pass.0].generation);
        let state = &mut cx.textures.1.readbacks;
        if state.slots.len() >= TEXTURE_READBACK_MAX_REQUESTS || reserved_bytes > TEXTURE_READBACK_MAX_BYTES.saturating_sub(state.reserved_bytes) {
            return Err(ReadbackError::Backpressure);
        }
        let ticket = ReadbackTicket(state.next_ticket.checked_add(1).ok_or(ReadbackError::Backpressure)?);
        state.next_ticket = ticket.0;
        state.reserved_bytes += reserved_bytes;
        state.slots.push(ReadbackSlot {
            legacy: false,
            texture: self.clone(), pass: pending_pass, pending: true, cancelled: false,
            pass_generation, next_render: request.next_render, previous_serial,
            reserved_bytes, receive: None,
            result: TextureReadback {
                ticket, allocation_generation: generation, producer_serial, width, height,
                stride: row, channel_order: ReadbackChannelOrder::Bgra, origin: ReadbackOrigin::TopLeft,
                data: Err(ReadbackError::NotRendered),
            },
        });
        cx.poll_texture_readbacks();
        Ok(ticket)
    }
}

#[derive(Default)]
pub struct CxTexturePool(pub(crate) IdPool<CxTexture>, pub(crate) TextureLifetime);

#[derive(Default)]
pub(crate) struct FrameSerials {
    pub(crate) submitted: AtomicU64,
    pub(crate) completed: AtomicU64,
    #[cfg(all(
        not(headless),
        any(target_os = "macos", target_os = "ios", target_os = "tvos")
    ))]
    pub(crate) encoded: AtomicU64,
}

impl FrameSerials {
    #[cfg(any(
        test,
        headless,
        not(any(target_os = "macos", target_os = "ios", target_os = "tvos"))
    ))]
    pub(crate) fn submit(&self) -> u64 {
        // Submission has one writer (the renderer/UI thread); only completion
        // callbacks write concurrently, to the separate completed atomic.
        let serial = self.submitted.load(Ordering::Relaxed).saturating_add(1);
        self.submitted.store(serial, Ordering::Release);
        serial
    }

    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(crate) fn complete(&self, serial: u64) {
        self.completed.fetch_max(
            serial.min(self.submitted.load(Ordering::Acquire)),
            Ordering::Release,
        );
    }
}

#[derive(Default)]
pub(crate) struct TextureLifetime {
    pub(crate) serials: Arc<FrameSerials>,
    pub(crate) retired: Vec<RetiredTexture>,
    pub(crate) readbacks: TextureReadbacks,
    #[cfg(all(not(headless), any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
    pub(crate) metal_readbacks: crate::os::apple::metal::MetalReadbacks,
    #[cfg(all(not(headless), not(any(use_vulkan, linux_direct, target_env = "ohos")), any(target_os = "linux", target_os = "android")))]
    pub(crate) gl_readbacks: crate::os::linux::opengl::GlReadbacks,
    #[cfg(all(not(headless), target_os = "windows"))]
    pub(crate) d3d_readbacks: crate::os::windows::d3d11::D3dReadbacks,
    #[cfg(all(
        not(headless),
        not(any(use_vulkan, linux_direct, target_env = "ohos")),
        any(target_os = "linux", target_os = "android")
    ))]
    pub(crate) gl: crate::os::linux::opengl::TextureFence,
    #[cfg(all(not(headless), target_os = "windows"))]
    pub(crate) d3d: Option<(u64, windows::Win32::Graphics::Direct3D11::ID3D11Query)>,
}

pub(crate) struct RetiredTexture {
    pub(crate) serial: u64,
    pub(crate) bytes: u64,
    pub(crate) os: CxOsTexture,
}

impl CxTexturePool {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn id_at_index(&self, index: usize) -> Option<TextureId> {
        let slot = self.0.pool.get(index)?;
        (!self.0.is_free(index)).then_some(TextureId(index, slot.generation))
    }

    // Allocates a new texture in the pool, potentially reusing an existing texture slot.
    ///
    /// This method attempts to find a compatible texture slot for reuse. If found, it preserves
    /// the old os-specific resources for proper cleanup. If not, it allocates a new slot.
    ///
    /// # Arguments
    /// * `requested_format` - The format of the texture to be allocated.
    ///
    /// # Returns
    /// A `Texture` instance representing the allocated or reused texture.
    ///
    /// # Note
    /// When a texture slot is reused, the old platofrm-specific resources are stored in the `previous_platform_resource` field
    /// of the new `CxTexture`. This allows for proper resource management and cleanup in the corresponding platform.
    pub fn alloc(&mut self, requested_format: TextureFormat) -> Texture {
        let is_video = requested_format.is_video();
        let cx_texture = CxTexture {
            format: requested_format,
            alloc: None,
            ..Default::default()
        };

        let (new_id, previous_item) = self.0.alloc_with_reuse_filter(
            |item| {
                // Check for compatibility, intentionally not using `is_compatible_with` to avoid passing the whole format and cloning vec contents
                is_video == item.item.format.is_video()
            },
            cx_texture,
        );

        if let Some(previous_item) = previous_item {
            // We know this index is valid because it was just reused
            self.0.pool[new_id.id].item.previous_platform_resource = Some(previous_item.os);
        }

        Texture(Rc::new(new_id))
    }
}

impl std::ops::Index<TextureId> for CxTexturePool {
    type Output = CxTexture;
    fn index(&self, index: TextureId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Texture id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &d.item
    }
}

impl std::ops::IndexMut<TextureId> for CxTexturePool {
    fn index_mut(&mut self, index: TextureId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "Texture id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &mut d.item
    }
}

#[derive(Clone, Debug)]
pub enum TextureSize {
    Auto,
    Fixed { width: usize, height: usize },
}

impl TextureSize {
    fn width_height(&self, w: usize, h: usize) -> (usize, usize) {
        match self {
            TextureSize::Auto => (w, h),
            TextureSize::Fixed { width, height } => (*width, *height),
        }
    }
}

/// Wrap mode stored on vec textures. Metal/Vulkan/D3D take address from the
/// shader sampler; OpenGL (and other per-texture wrap backends) read this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextureWrap {
    #[default]
    ClampToEdge,
    Repeat,
}

#[derive(Clone)]
pub enum TextureFormat {
    Unknown,
    VecBGRAu8_32 {
        width: usize,
        height: usize,
        data: Option<Vec<u32>>,
        updated: TextureUpdated,
    },
    /// Cubemap faces are packed in this order: +X, -X, +Y, -Y, +Z, -Z.
    VecCubeBGRAu8_32 {
        width: usize,
        height: usize,
        data: Option<Vec<u32>>,
        updated: TextureUpdated,
    },
    VecMipBGRAu8_32 {
        width: usize,
        height: usize,
        data: Option<Vec<u32>>,
        max_level: Option<usize>,
        wrap: TextureWrap,
        updated: TextureUpdated,
    },
    VecMipRGBAf32 {
        width: usize,
        height: usize,
        data: Option<Vec<f32>>,
        max_level: Option<usize>,
        updated: TextureUpdated,
    },
    VecRGBAf32 {
        width: usize,
        height: usize,
        data: Option<Vec<f32>>,
        updated: TextureUpdated,
    },
    VecRu8 {
        width: usize,
        height: usize,
        data: Option<Vec<u8>>,
        unpack_row_length: Option<usize>,
        updated: TextureUpdated,
    },
    VecRGu8 {
        width: usize,
        height: usize,
        data: Option<Vec<u8>>,
        unpack_row_length: Option<usize>,
        updated: TextureUpdated,
    },
    VecRf32 {
        width: usize,
        height: usize,
        data: Option<Vec<f32>>,
        updated: TextureUpdated,
    },
    DepthD32 {
        size: TextureSize,
        initial: bool,
    },
    /// Depth attachment retained for later comparison sampling. Ordinary UI
    /// depth stays attachment-only; use texture_depth().sample_compare in Splash.
    DepthD32Sampled {
        size: TextureSize,
        initial: bool,
    },
    RenderBGRAu8 {
        size: TextureSize,
        initial: bool,
    },
    RenderCubeBGRAu8 {
        size: TextureSize,
        initial: bool,
    },
    RenderRGBAf16 {
        size: TextureSize,
        initial: bool,
    },
    RenderRGBAf32 {
        size: TextureSize,
        initial: bool,
    },
    /// Single-channel float render target (R32F). Added for GPU baking
    /// passes that need real float precision (depth scratch maps) — sampled
    /// with `sample_nearest` (32-bit float filtering is not universal).
    /// Draw shaders rendering into it must declare `color_format: @Rf32`
    /// so their pipeline state matches the attachment format.
    RenderRf32 {
        size: TextureSize,
        initial: bool,
    },

    SharedBGRAu8 {
        width: usize,
        height: usize,
        id: crate::shared_framebuf::PresentableImageId,
        initial: bool,
    },
    /// A single YUV plane texture (Y, U, or V). Backend-managed — the render
    /// backend or platform layer creates/uploads the GPU texture directly.
    /// Used for both I420 (three R8 planes) and NV12 (R8 luma + RG8 chroma).
    VideoYuvPlane,
    /// An opaque external video texture whose contents are managed outside the
    /// normal texture upload path (e.g. Android SurfaceTexture/OES, or
    /// platform-native composited video output).
    VideoExternal,
    /// Linux GStreamer `GLMemory` RGBA texture (`TEXTURE_2D`) shared into
    /// Makepad's GL context via a sibling EGL share group. Not OES.
    VideoGlMemoryRgba,
    /// Android/Vulkan camera texture backed by an imported RGBA
    /// `AHardwareBuffer`.
    VideoRgbaHardwareBuffer,
}

impl std::fmt::Debug for TextureFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        match self {
            TextureFormat::Unknown => write!(f, "TextureFormat::Unknown"),
            TextureFormat::VecBGRAu8_32 { width, height, .. } => write!(
                f,
                "TextureFormat::VecBGRAu8_32(width:{width},height:{height})"
            ),
            TextureFormat::VecCubeBGRAu8_32 { width, height, .. } => write!(
                f,
                "TextureFormat::VecCubeBGRAu8_32(width:{width},height:{height})"
            ),
            TextureFormat::VecMipBGRAu8_32 { width, height, .. } => write!(
                f,
                "TextureFormat::VecMipBGRAu8_32(width:{width},height:{height})"
            ),
            TextureFormat::VecMipRGBAf32 { width, height, .. } => write!(
                f,
                "TextureFormat::VecMipRGBAf32(width:{width},height:{height})"
            ),
            TextureFormat::VecRGBAf32 { width, height, .. } => write!(
                f,
                "TextureFormat::VecRGBAf32(width:{width},height:{height})"
            ),
            TextureFormat::VecRu8 { width, height, .. } => {
                write!(f, "TextureFormat::VecRu8(width:{width},height:{height})")
            }
            TextureFormat::VecRGu8 { width, height, .. } => {
                write!(f, "TextureFormat::VecRGu8(width:{width},height:{height})")
            }
            TextureFormat::VecRf32 { width, height, .. } => {
                write!(f, "TextureFormat::VecRf32(width:{width},height:{height})")
            }
            TextureFormat::DepthD32 { size, .. } => {
                write!(f, "TextureFormat::DepthD32(size:{:?})", size)
            }
            TextureFormat::DepthD32Sampled { size, .. } => {
                write!(f, "TextureFormat::DepthD32Sampled(size:{:?})", size)
            }
            TextureFormat::RenderBGRAu8 { size, .. } => {
                write!(f, "TextureFormat::RenderBGRAu8(size:{:?})", size)
            }
            TextureFormat::RenderCubeBGRAu8 { size, .. } => {
                write!(f, "TextureFormat::RenderCubeBGRAu8(size:{:?})", size)
            }
            TextureFormat::RenderRGBAf16 { size, .. } => {
                write!(f, "TextureFormat::RenderRGBAf16(size:{:?})", size)
            }
            TextureFormat::RenderRGBAf32 { size, .. } => {
                write!(f, "TextureFormat::RenderRGBAf32(size:{:?})", size)
            }
            TextureFormat::RenderRf32 { size, .. } => {
                write!(f, "TextureFormat::RenderRf32(size:{:?})", size)
            }
            TextureFormat::SharedBGRAu8 { width, height, .. } => write!(
                f,
                "TextureFormat::SharedBGRAu8(width:{width},height:{height})"
            ),
            TextureFormat::VideoYuvPlane => write!(f, "TextureFormat::VideoYuvPlane"),
            TextureFormat::VideoExternal => write!(f, "TextureFormat::VideoExternal"),
            TextureFormat::VideoGlMemoryRgba => write!(f, "TextureFormat::VideoGlMemoryRgba"),
            TextureFormat::VideoRgbaHardwareBuffer => {
                write!(f, "TextureFormat::VideoRgbaHardwareBuffer")
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TextureAnimation {
    pub width: usize,
    pub height: usize,
    pub num_frames: usize,
    pub frame_delays: Vec<f64>,
}

#[cfg(test)]
mod texture_animation_tests {
    use super::*;

    #[test]
    fn test_texture_animation_default_frame_delays_is_empty() {
        let animation = TextureAnimation::default();
        assert!(animation.frame_delays.is_empty());
        assert_eq!(animation.frame_delays.len(), 0);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextureAlloc {
    pub category: TextureCategory,
    pub pixel: TexturePixel,
    pub width: usize,
    pub height: usize,
}

#[allow(unused)]
#[derive(Clone, Debug)]
pub enum TextureCategory {
    Vec,
    VecMip,
    VecCube,
    Render,
    RenderCube,
    DepthBuffer,
    DepthBufferSampled,
    Shared,
    Video,
}

impl PartialEq for TextureCategory {
    fn eq(&self, other: &TextureCategory) -> bool {
        match self {
            Self::DepthBufferSampled => matches!(other, Self::DepthBufferSampled),
            Self::Vec { .. } => {
                if let Self::Vec { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::VecMip { .. } => {
                if let Self::VecMip { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::VecCube { .. } => {
                if let Self::VecCube { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::Render { .. } => {
                if let Self::Render { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::RenderCube { .. } => {
                if let Self::RenderCube { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::Shared { .. } => {
                if let Self::Shared { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::DepthBuffer { .. } => {
                if let Self::DepthBuffer { .. } = other {
                    true
                } else {
                    false
                }
            }
            Self::Video { .. } => {
                if let Self::Video { .. } = other {
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// What of a Vec texture's CPU image still has to reach the GPU.
///
/// `Partial` describes the change RELATIVE TO WHAT THE GPU ALREADY HOLDS: a
/// backend that updates its GPU storage in place may upload just that rect,
/// but one that (re)allocates the storage — first sight, a size change (the
/// slug glyph atlas grows by appending rows and marks only those dirty) —
/// has nothing to keep and must upload the whole image regardless of the
/// rect. Every backend honors that (GL `glTexImage2D`, D3D11's realloc path,
/// the headless mirror rebuild, Metal's `vec_fresh`).
#[derive(Clone, Copy, Debug)]
pub enum TextureUpdated {
    Empty,
    Partial(RectUsize),
    Full,
}

impl TextureUpdated {
    pub fn is_empty(&self) -> bool {
        match self {
            TextureUpdated::Empty => true,
            _ => false,
        }
    }

    pub fn update(self, dirty_rect: Option<RectUsize>) -> Self {
        let new = match dirty_rect {
            Some(dirty_rect) => match self {
                TextureUpdated::Empty => TextureUpdated::Partial(dirty_rect),
                TextureUpdated::Partial(rect) => TextureUpdated::Partial(rect.union(dirty_rect)),
                TextureUpdated::Full => TextureUpdated::Full,
            },
            None => TextureUpdated::Full,
        };
        if let TextureUpdated::Partial(p) = new {
            if p.size == SizeUsize::new(0, 0) {
                return TextureUpdated::Empty;
            }
        }
        new
    }
}

#[allow(unused)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TexturePixel {
    BGRAu8,
    RGBAf16,
    RGBAf32,
    Ru8,
    RGu8,
    Rf32,
    D32,
    /// YUV plane pixel type. Individual planes are R8 (luma, I420 chroma) or
    /// RG8 (NV12 chroma); the actual GPU format is set at upload/wrap time.
    VideoYuvPlane,
    /// Opaque external video pixel type (e.g. Android OES, composited RGBA).
    VideoExternal,
    /// Linux GStreamer GLMemory RGBA (`TEXTURE_2D`).
    VideoGlMemoryRgba,
    /// Android/Vulkan imported RGBA hardware buffer.
    VideoRgbaHardwareBuffer,
}

impl CxTexture {
    pub(crate) fn reset_allocation(&mut self) {
        self.allocation_generation = self.allocation_generation.saturating_add(1);
        self.producer_serial = 0;
        self.alloc = None;
        if self.format.is_vec() {
            self.set_updated(TextureUpdated::Full);
        } else if self.format.is_render() || self.format.is_depth() {
            self.set_initial(true);
        }
    }

    #[allow(unused)]
    pub(crate) fn updated(&self) -> TextureUpdated {
        match self.format {
            TextureFormat::VecBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecCubeBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecMipBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecMipRGBAf32 { updated, .. } => updated,
            TextureFormat::VecRGBAf32 { updated, .. } => updated,
            TextureFormat::VecRu8 { updated, .. } => updated,
            TextureFormat::VecRGu8 { updated, .. } => updated,
            TextureFormat::VecRf32 { updated, .. } => updated,
            _ => panic!(),
        }
    }

    #[allow(unused)]
    pub(crate) fn initial(&mut self) -> bool {
        match self.format {
            TextureFormat::DepthD32 { initial, .. } | TextureFormat::DepthD32Sampled { initial, .. } => initial,
            TextureFormat::RenderBGRAu8 { initial, .. } => initial,
            TextureFormat::RenderCubeBGRAu8 { initial, .. } => initial,
            TextureFormat::RenderRGBAf16 { initial, .. } => initial,
            TextureFormat::RenderRGBAf32 { initial, .. } => initial,
            TextureFormat::RenderRf32 { initial, .. } => initial,
            TextureFormat::SharedBGRAu8 { initial, .. } => initial,
            _ => panic!(),
        }
    }

    #[allow(unused)]
    pub(crate) fn set_updated(&mut self, updated: TextureUpdated) {
        *match &mut self.format {
            TextureFormat::VecBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecCubeBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecMipBGRAu8_32 { updated, .. } => updated,
            TextureFormat::VecMipRGBAf32 { updated, .. } => updated,
            TextureFormat::VecRGBAf32 { updated, .. } => updated,
            TextureFormat::VecRu8 { updated, .. } => updated,
            TextureFormat::VecRGu8 { updated, .. } => updated,
            TextureFormat::VecRf32 { updated, .. } => updated,
            _ => panic!(),
        } = updated;
    }

    pub fn set_initial(&mut self, initial: bool) {
        *match &mut self.format {
            TextureFormat::DepthD32 { initial, .. } | TextureFormat::DepthD32Sampled { initial, .. } => initial,
            TextureFormat::RenderBGRAu8 { initial, .. } => initial,
            TextureFormat::RenderCubeBGRAu8 { initial, .. } => initial,
            TextureFormat::RenderRGBAf16 { initial, .. } => initial,
            TextureFormat::RenderRGBAf32 { initial, .. } => initial,
            TextureFormat::RenderRf32 { initial, .. } => initial,
            TextureFormat::SharedBGRAu8 { initial, .. } => initial,
            _ => panic!(),
        } = initial;
    }

    #[allow(unused)]
    pub(crate) fn take_updated(&mut self) -> TextureUpdated {
        let updated = self.updated();
        self.set_updated(TextureUpdated::Empty);
        updated
    }
    #[allow(unused)]
    pub(crate) fn take_initial(&mut self) -> bool {
        let initial = self.initial();
        self.set_initial(false);
        initial
    }

    #[allow(unused)]
    pub(crate) fn alloc_vec(&mut self) -> bool {
        if let Some(alloc) = self.format.as_vec_alloc() {
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc {
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }

    #[allow(unused)]
    pub(crate) fn alloc_shared(&mut self) -> bool {
        if let Some(alloc) = self.format.as_shared_alloc() {
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc {
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }

    #[allow(unused)]
    pub(crate) fn alloc_render(&mut self, width: usize, height: usize) -> bool {
        if let Some(alloc) = self.format.as_render_alloc(width, height) {
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc {
                self.allocation_generation = self.allocation_generation.saturating_add(1);
                self.producer_serial = 0;
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }

    #[allow(unused)]
    pub(crate) fn alloc_depth(&mut self, width: usize, height: usize) -> bool {
        if let Some(alloc) = self.format.as_depth_alloc(width, height) {
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc {
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }

    #[allow(unused)]
    pub(crate) fn alloc_video(&mut self) -> bool {
        if let Some(alloc) = self.format.as_video_alloc() {
            // Video textures are sized/filled by the present path (DMA-Buf / MediaCodec).
            // Once the pixel type matches, do not treat width/height updates as a re-alloc —
            // that would re-init the GL texture every frame and detach EGLImages (NVIDIA
            // black screen / corner garbage).
            if let Some(existing) = self.alloc.as_ref() {
                if existing.pixel == alloc.pixel
                    && matches!(
                        existing.pixel,
                        TexturePixel::VideoExternal
                            | TexturePixel::VideoYuvPlane
                            | TexturePixel::VideoGlMemoryRgba
                            | TexturePixel::VideoRgbaHardwareBuffer
                    )
                {
                    return false;
                }
            }
            if self.alloc.is_none() || self.alloc.as_ref().unwrap() != &alloc {
                self.alloc = Some(alloc);
                return true;
            }
        }
        false
    }
}

impl TextureFormat {
    /// CPU pixel bytes this format currently holds (capacity, not length).
    pub(crate) fn cpu_data_bytes(&self) -> usize {
        match self {
            TextureFormat::VecBGRAu8_32 { data, .. }
            | TextureFormat::VecCubeBGRAu8_32 { data, .. }
            | TextureFormat::VecMipBGRAu8_32 { data, .. } => data
                .as_ref()
                .map_or(0, |data| data.capacity().saturating_mul(4)),
            TextureFormat::VecMipRGBAf32 { data, .. }
            | TextureFormat::VecRGBAf32 { data, .. }
            | TextureFormat::VecRf32 { data, .. } => data
                .as_ref()
                .map_or(0, |data| data.capacity().saturating_mul(4)),
            TextureFormat::VecRu8 { data, .. } | TextureFormat::VecRGu8 { data, .. } => {
                data.as_ref().map_or(0, |data| data.capacity())
            }
            _ => 0,
        }
    }

    pub fn is_shared(&self) -> bool {
        match self {
            Self::SharedBGRAu8 { .. } => true,
            _ => false,
        }
    }
    pub fn is_vec(&self) -> bool {
        match self {
            Self::VecBGRAu8_32 { .. } => true,
            Self::VecCubeBGRAu8_32 { .. } => true,
            Self::VecMipBGRAu8_32 { .. } => true,
            Self::VecMipRGBAf32 { .. } => true,
            Self::VecRGBAf32 { .. } => true,
            Self::VecRu8 { .. } => true,
            Self::VecRGu8 { .. } => true,
            Self::VecRf32 { .. } => true,
            _ => false,
        }
    }

    pub fn is_render(&self) -> bool {
        match self {
            Self::RenderBGRAu8 { .. } => true,
            Self::RenderCubeBGRAu8 { .. } => true,
            Self::RenderRGBAf16 { .. } => true,
            Self::RenderRGBAf32 { .. } => true,
            Self::RenderRf32 { .. } => true,
            _ => false,
        }
    }

    pub fn is_depth(&self) -> bool {
        match self {
            Self::DepthD32 { .. } | Self::DepthD32Sampled { .. } => true,
            _ => false,
        }
    }

    pub fn is_sampled_depth(&self) -> bool {
        matches!(self, Self::DepthD32Sampled { .. })
    }

    pub fn is_video(&self) -> bool {
        matches!(
            self,
            Self::VideoYuvPlane
                | Self::VideoExternal
                | Self::VideoGlMemoryRgba
                | Self::VideoRgbaHardwareBuffer
        )
    }

    pub fn is_video_external(&self) -> bool {
        matches!(self, Self::VideoExternal)
    }

    pub fn is_video_rgba_hardware_buffer(&self) -> bool {
        matches!(self, Self::VideoRgbaHardwareBuffer)
    }

    /// Per-texture wrap. Defaults to clamp; world/albedo mip textures set Repeat
    /// so OpenGL matches the shader's `sample_*_repeat` sampler.
    pub fn wrap(&self) -> TextureWrap {
        match self {
            Self::VecMipBGRAu8_32 { wrap, .. } => *wrap,
            _ => TextureWrap::ClampToEdge,
        }
    }

    pub fn vec_width_height(&self) -> Option<(usize, usize)> {
        match self {
            Self::VecBGRAu8_32 { width, height, .. } => Some((*width, *height)),
            Self::VecCubeBGRAu8_32 { width, height, .. } => Some((*width, *height)),
            Self::VecMipBGRAu8_32 { width, height, .. } => Some((*width, *height)),
            Self::VecMipRGBAf32 { width, height, .. } => Some((*width, *height)),
            Self::VecRGBAf32 { width, height, .. } => Some((*width, *height)),
            Self::VecRu8 { width, height, .. } => Some((*width, *height)),
            Self::VecRGu8 { width, height, .. } => Some((*width, *height)),
            Self::VecRf32 { width, height, .. } => Some((*width, *height)),
            _ => None,
        }
    }

    /// Fixed dimensions of a render-target texture, when declared. Auto
    /// targets take their pass's size at draw time and report None here —
    /// a consumer that needs dims for layout (e.g. Image) can only size a
    /// Fixed target.
    pub fn render_fixed_width_height(&self) -> Option<(usize, usize)> {
        match self {
            Self::RenderBGRAu8 {
                size: TextureSize::Fixed { width, height },
                ..
            } => Some((*width, *height)),
            _ => None,
        }
    }

    #[allow(unused)]
    pub(crate) fn as_vec_alloc(&self) -> Option<TextureAlloc> {
        match self {
            Self::VecBGRAu8_32 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::BGRAu8,
                category: TextureCategory::Vec,
            }),
            Self::VecCubeBGRAu8_32 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::BGRAu8,
                category: TextureCategory::VecCube,
            }),
            Self::VecMipBGRAu8_32 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::BGRAu8,
                category: TextureCategory::VecMip,
            }),
            Self::VecMipRGBAf32 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::RGBAf32,
                category: TextureCategory::VecMip,
            }),
            Self::VecRGBAf32 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::RGBAf32,
                category: TextureCategory::Vec,
            }),
            Self::VecRu8 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::Ru8,
                category: TextureCategory::Vec,
            }),
            Self::VecRGu8 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::RGu8,
                category: TextureCategory::Vec,
            }),
            Self::VecRf32 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::Rf32,
                category: TextureCategory::Vec,
            }),
            _ => None,
        }
    }
    #[allow(unused)]
    pub(crate) fn as_render_alloc(&self, width: usize, height: usize) -> Option<TextureAlloc> {
        match self {
            Self::RenderBGRAu8 { size, .. } => {
                let (width, height) = size.width_height(width, height);
                Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::BGRAu8,
                    category: TextureCategory::Render,
                })
            }
            Self::RenderCubeBGRAu8 { size, .. } => {
                let (width, height) = size.width_height(width, height);
                Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::BGRAu8,
                    category: TextureCategory::RenderCube,
                })
            }
            Self::RenderRGBAf16 { size, .. } => {
                let (width, height) = size.width_height(width, height);
                Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::RGBAf16,
                    category: TextureCategory::Render,
                })
            }
            Self::RenderRGBAf32 { size, .. } => {
                let (width, height) = size.width_height(width, height);
                Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::RGBAf32,
                    category: TextureCategory::Render,
                })
            }
            Self::RenderRf32 { size, .. } => {
                let (width, height) = size.width_height(width, height);
                Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::Rf32,
                    category: TextureCategory::Render,
                })
            }
            _ => None,
        }
    }

    #[allow(unused)]
    pub(crate) fn as_depth_alloc(&self, width: usize, height: usize) -> Option<TextureAlloc> {
        match self {
            Self::DepthD32 { size, .. } | Self::DepthD32Sampled { size, .. } => {
                let (width, height) = size.width_height(width, height);
                Some(TextureAlloc {
                    width,
                    height,
                    pixel: TexturePixel::D32,
                    category: if self.is_sampled_depth() { TextureCategory::DepthBufferSampled } else { TextureCategory::DepthBuffer },
                })
            }
            _ => None,
        }
    }

    #[allow(unused)]
    pub(crate) fn as_video_alloc(&self) -> Option<TextureAlloc> {
        match self {
            Self::VideoYuvPlane => Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoYuvPlane,
                category: TextureCategory::Video,
            }),
            Self::VideoExternal => Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            }),
            Self::VideoGlMemoryRgba => Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoGlMemoryRgba,
                category: TextureCategory::Video,
            }),
            Self::VideoRgbaHardwareBuffer => Some(TextureAlloc {
                width: 0,
                height: 0,
                pixel: TexturePixel::VideoRgbaHardwareBuffer,
                category: TextureCategory::Video,
            }),
            _ => None,
        }
    }

    #[allow(unused)]
    pub(crate) fn as_shared_alloc(&self) -> Option<TextureAlloc> {
        match self {
            Self::SharedBGRAu8 { width, height, .. } => Some(TextureAlloc {
                width: *width,
                height: *height,
                pixel: TexturePixel::BGRAu8,
                category: TextureCategory::Shared,
            }),
            _ => None,
        }
    }

    #[allow(unused)]
    fn is_compatible_with(&self, other: &Self) -> bool {
        !(self.is_video() ^ other.is_video())
    }
}

impl Default for TextureFormat {
    fn default() -> Self {
        TextureFormat::Unknown
    }
}

impl ScriptHook for Texture {}
impl ScriptApply for Texture {}
impl ScriptNew for Texture {
    fn script_new(vm: &mut ScriptVm) -> Self {
        Self::new(vm.cx_mut())
    }
}

impl Texture {
    pub fn new(cx: &mut Cx) -> Self {
        cx.null_texture()
    }

    /// Retire GPU storage without invalidating this handle. See [`Texture`].
    /// This does not submit recorded passes or request a redraw. Reclamation is
    /// nonblocking and is collected by `Cx::frame_completion_serial` polling.
    pub fn release(&self, cx: &mut Cx) {
        cx.release_texture_allocation(self.texture_id());
    }

    /// Bytes in the current backend allocation, or `None` when unallocated or
    /// externally managed/unsupported. Pending retirements are charged only to
    /// `Cx::texture_pool_bytes`, not to this handle's new allocation.
    pub fn allocated_bytes(&self, cx: &Cx) -> Option<u64> {
        cx.texture_allocation_bytes(self.texture_id())
    }

    pub fn new_with_format(cx: &mut Cx, format: TextureFormat) -> Self {
        let texture = cx.textures.alloc(format);
        texture
    }

    pub fn set_animation(&self, cx: &mut Cx, animation: Option<TextureAnimation>) {
        cx.textures[self.texture_id()].animation = animation;
    }

    pub fn animation<'a>(&self, cx: &'a mut Cx) -> &'a Option<TextureAnimation> {
        &cx.textures[self.texture_id()].animation
    }

    pub fn get_format<'a>(&self, cx: &'a mut Cx) -> &'a mut TextureFormat {
        &mut cx.textures[self.texture_id()].format
    }

    pub fn take_vec_u32(&self, cx: &mut Cx) -> Vec<u32> {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let data = match &mut cx_texture.format {
            TextureFormat::VecBGRAu8_32 { data, .. } => data,
            _ => panic!("incorrect texture format for u32 image data"),
        };
        data.take().expect("image data already taken")
    }

    pub fn swap_vec_u32(&self, cx: &mut Cx, vec: &mut Vec<u32>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecBGRAu8_32 { data, updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for u32 image data"),
        };
        if data.is_none() {
            *data = Some(vec![]);
        }
        if let Some(data) = data {
            std::mem::swap(data, vec)
        }
        *updated = updated.update(None);
    }

    /// Replace the pixel data and dimensions of a BGRA u32 texture.
    /// Unlike `put_back_vec_u32`, this also updates width and height,
    /// making it safe for image sources that change resolution (e.g.
    /// animated images with varying frame sizes, or lazily-loaded images
    /// replacing a placeholder).
    pub fn set_data_u32(
        &self,
        cx: &mut Cx,
        new_width: usize,
        new_height: usize,
        new_data: Vec<u32>,
    ) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (width, height, data, updated) = match &mut cx_texture.format {
            TextureFormat::VecBGRAu8_32 {
                width,
                height,
                data,
                updated,
            } => (width, height, data, updated),
            _ => panic!("incorrect texture format for u32 image data"),
        };
        *width = new_width;
        *height = new_height;
        *data = Some(new_data);
        *updated = updated.update(None);
    }

    pub fn put_back_vec_u32(&self, cx: &mut Cx, new_data: Vec<u32>, dirty_rect: Option<RectUsize>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecBGRAu8_32 { data, updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for u32 image data"),
        };
        //assert!(data.is_none(), "image data not taken or already put back");
        *data = Some(new_data);
        *updated = updated.update(dirty_rect);
    }

    pub fn take_vec_u8(&self, cx: &mut Cx) -> Vec<u8> {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let data = match &mut cx_texture.format {
            TextureFormat::VecRu8 { data, .. } => data,
            TextureFormat::VecRGu8 { data, .. } => data,
            _ => panic!("incorrect texture format for u32 image data"),
        };
        data.take().expect("image data already taken")
    }

    pub fn put_back_vec_u8(&self, cx: &mut Cx, new_data: Vec<u8>, dirty_rect: Option<RectUsize>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecRu8 { data, updated, .. } => (data, updated),
            TextureFormat::VecRGu8 { data, updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for u8 image data"),
        };
        assert!(data.is_none(), "image data not taken or already put back");
        *data = Some(new_data);
        *updated = updated.update(dirty_rect);
    }

    pub fn take_vec_f32(&self, cx: &mut Cx) -> Vec<f32> {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let data = match &mut cx_texture.format {
            TextureFormat::VecRf32 { data, .. } => data,
            TextureFormat::VecMipRGBAf32 { data, .. } => data,
            TextureFormat::VecRGBAf32 { data, .. } => data,
            _ => panic!("Not the correct texture desc for f32 image data"),
        };
        data.take().expect("image data already taken")
    }

    pub fn put_back_vec_f32(&self, cx: &mut Cx, new_data: Vec<f32>, dirty_rect: Option<RectUsize>) {
        let cx_texture = &mut cx.textures[self.texture_id()];
        let (data, updated) = match &mut cx_texture.format {
            TextureFormat::VecRf32 { data, updated, .. } => (data, updated),
            TextureFormat::VecMipRGBAf32 { data, updated, .. } => (data, updated),
            TextureFormat::VecRGBAf32 { data, updated, .. } => (data, updated),
            _ => panic!("incorrect texture format for f32 image data"),
        };
        assert!(data.is_none(), "image data not taken or already put back");
        *data = Some(new_data);
        *updated = updated.update(dirty_rect);
    }
}

#[derive(Default)]
pub struct CxTexture {
    pub(crate) format: TextureFormat,
    pub(crate) alloc: Option<TextureAlloc>,
    pub(crate) allocation_generation: u64,
    pub(crate) producer_serial: u64,
    pub(crate) animation: Option<TextureAnimation>,
    pub os: CxOsTexture,
    pub previous_platform_resource: Option<CxOsTexture>,
}

#[cfg(test)]
mod tests {
    use super::{TextureFormat, TextureUpdated, TextureWrap};

    #[test]
    fn frame_serials_are_monotonic_and_completion_is_bounded() {
        use super::FrameSerials;
        use std::sync::atomic::Ordering;
        let serials = FrameSerials::default();
        assert_eq!(serials.submitted.load(Ordering::Acquire), 0);
        serials.complete(100);
        assert_eq!(serials.completed.load(Ordering::Acquire), 0);
        assert_eq!(serials.submit(), 1);
        assert_eq!(serials.submit(), 2);
        serials.complete(2);
        serials.complete(1); // A late callback cannot move the frontier back.
        assert_eq!(serials.completed.load(Ordering::Acquire), 2);
        assert_eq!(serials.submit(), 3);
        serials.complete(u64::MAX);
        assert_eq!(serials.completed.load(Ordering::Acquire), 3);
        serials.submitted.store(u64::MAX, Ordering::Release);
        assert_eq!(serials.submit(), u64::MAX);
        serials.complete(u64::MAX);
        assert_eq!(serials.completed.load(Ordering::Acquire), u64::MAX);
    }

    #[test]
    fn mip_format_reports_wrap() {
        let clamp = TextureFormat::VecMipBGRAu8_32 {
            width: 4,
            height: 4,
            data: None,
            max_level: Some(2),
            wrap: TextureWrap::ClampToEdge,
            updated: TextureUpdated::Full,
        };
        let repeat = TextureFormat::VecMipBGRAu8_32 {
            width: 4,
            height: 4,
            data: None,
            max_level: Some(2),
            wrap: TextureWrap::Repeat,
            updated: TextureUpdated::Full,
        };
        assert_eq!(clamp.wrap(), TextureWrap::ClampToEdge);
        assert_eq!(repeat.wrap(), TextureWrap::Repeat);
        assert_eq!(
            TextureFormat::VecBGRAu8_32 {
                width: 1,
                height: 1,
                data: None,
                updated: TextureUpdated::Full,
            }
            .wrap(),
            TextureWrap::ClampToEdge
        );
    }
}

#[cfg(all(
    not(headless),
    not(target_arch = "wasm32"),
    not(any(use_vulkan, linux_direct, target_env = "ohos")),
    any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "linux",
        target_os = "android",
        target_os = "windows"
    )
))]
impl Cx {
    pub(crate) fn texture_allocation_bytes(&self, id: TextureId) -> Option<u64> {
        let texture = &self.textures[id];
        if !(texture.format.is_render() || texture.format.is_vec() || texture.format.is_depth()) {
            return None;
        }
        texture.os.allocated_bytes(self)
    }

    pub(crate) fn release_texture_allocation(&mut self, id: TextureId) {
        if !(self.textures[id].format.is_render()
            || self.textures[id].format.is_vec()
            || self.textures[id].format.is_depth())
        {
            return;
        }
        if self.textures[id].alloc.is_none()
            && self.textures[id].previous_platform_resource.is_none()
        {
            self.textures[id].reset_allocation();
            return;
        }
        self.poll_texture_lifetimes();
        self.detach_released_texture(id);
        let serial = self.frame_submission_serial();
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        let serial = serial.max(self.textures.1.serials.encoded.load(Ordering::Acquire));
        let texture = &mut self.textures[id];
        let os = std::mem::take(&mut texture.os);
        let previous = texture.previous_platform_resource.take();
        texture.reset_allocation();
        for os in Some(os).into_iter().chain(previous) {
            let bytes = os.allocated_bytes(self).unwrap_or(0);
            self.textures
                .1
                .retired
                .push(RetiredTexture { serial, bytes, os });
        }
        self.poll_texture_lifetimes();
    }
}

// Renderer backends outside the supported lifetime matrix fail closed.
#[cfg(all(not(headless), any(use_vulkan, linux_direct, target_env = "ohos")))]
impl Cx {
    pub(crate) fn poll_texture_lifetimes(&mut self) {}
    pub(crate) fn release_texture_allocation(&mut self, _id: TextureId) {}
    pub(crate) fn texture_allocation_bytes(&self, _id: TextureId) -> Option<u64> {
        None
    }
}
#[cfg(all(not(headless), any(use_vulkan, linux_direct, target_env = "ohos")))]
impl CxOsTexture {
    pub(crate) fn allocated_bytes(&self, _cx: &Cx) -> Option<u64> {
        None
    }
}
