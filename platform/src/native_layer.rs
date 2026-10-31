//! Native content recorded into a draw list; placement belongs to its window frame.
use crate::{
    cx::Cx,
    cx_api::CxOsOp,
    draw_list::DrawListId,
    draw_pass::{CxDrawPassParent, DrawPassId},
    event::Event,
    id_pool::{IdPool, PoolId},
    makepad_live_id::LiveId,
    makepad_math::Rect,
    texture::Texture,
    video::{VideoFormatId, VideoInputId},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeLayerId {
    pub(crate) index: usize,
    pub(crate) generation: u64,
}

#[derive(Debug)]
pub struct NativeLayer(PoolId);

#[derive(Clone, Debug, PartialEq)]
pub enum NativeContent {
    Web { url: String },
    /// A Wayland app in the window manager's opt-in launcher registry.
    WaylandApp { app: String },
    CameraPreview { video_id: LiveId, input: VideoInputId, format: VideoFormatId },
}

#[derive(Clone, Copy, Debug)]
pub struct NativeLayerStyle {
    /// Top-left, top-right, bottom-right, bottom-left, in layout points.
    pub radius: [f64; 4],
    pub opacity: f64,
    pub input_passthrough: bool,
}

impl Default for NativeLayerStyle {
    fn default() -> Self {
        Self { radius: [0.0; 4], opacity: 1.0, input_passthrough: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum NativeLayerMode {
    #[default]
    Pending,
    Composited,
    Textured(Texture),
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct NativeLayerEvent {
    pub layer: NativeLayerId,
    pub kind: NativeLayerEventKind,
}

#[derive(Clone, Debug)]
pub enum NativeLayerEventKind {
    Realized(NativeLayerMode),
    Title(String),
    Url(String),
    Loading(bool),
    Nav { can_back: bool, can_forward: bool },
    FocusIn,
    FocusOut,
    Cursor(u32),
    Crashed(String),
    Closed,
}

#[derive(Debug, PartialEq)]
pub enum NativeLayerOperation {
    Navigate { url: String, replace: bool },
    History(i32),
    Reload,
    Stop,
    Appearance { dark: bool, background: [f32; 4] },
    Focus,
    Blur,
    /// The same packed event and payload as HAL `buffers::pack_forward`.
    Input(Vec<u8>),
    Size { width: u32, height: u32 },
    CameraSource { input: VideoInputId, format: VideoFormatId },
}

#[derive(Default)]
pub(crate) struct CxNativeLayer {
    mode: NativeLayerMode,
    closed: bool,
    logged_non_window: bool,
}

#[derive(Default)]
pub(crate) struct CxNativeLayerPool(IdPool<CxNativeLayer>);

impl CxNativeLayerPool {
    fn get(&self, id: NativeLayerId) -> Option<&CxNativeLayer> {
        self.0.is_live_generation(id.index, id.generation).then(|| &self.0.pool[id.index].item)
    }
}

impl NativeLayerId {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn hal(self) -> makepad_hal::Id {
        makepad_hal::Id::new(self.index as u32, self.generation as u32)
    }
}

impl NativeLayer {
    pub fn id(&self) -> NativeLayerId {
        NativeLayerId { index: self.0.id, generation: self.0.generation }
    }

    pub fn mode(&self, cx: &Cx) -> NativeLayerMode {
        cx.native_layer_mode(self.id())
    }

    /// The draw crate's `NativeLayerDraw` supplies the current Cx2d list/pass.
    pub fn draw_in_list(&self, cx: &mut Cx, list: DrawListId, pass: DrawPassId, rect: Rect, style: NativeLayerStyle) -> NativeLayerMode {
        let mode = self.mode(cx);
        if !cx.native_layer_window_pass(pass) {
            if let Some(layer) = cx.native_layers.0.pool.get_mut(self.0.id) {
                if !layer.logged_non_window {
                    layer.logged_non_window = true;
                    crate::log!("Native layer {:?} drawn outside a window main pass", self.id());
                }
            }
            return match mode {
                NativeLayerMode::Textured(_) => mode,
                _ => NativeLayerMode::Unsupported,
            };
        }
        if matches!(mode, NativeLayerMode::Pending | NativeLayerMode::Composited) {
            cx.draw_lists[list].append_native_layer(cx.redraw_id, self.id(), rect, style);
        }
        mode
    }
}

impl Cx {
    pub fn native_layer_new(&mut self, content: NativeContent) -> NativeLayer {
        self.native_layer_retire();
        let handle = NativeLayer(self.native_layers.0.alloc());
        self.native_layers.0.pool[handle.0.id].item = CxNativeLayer::default();
        if self.script_data.std.host_io_only() {
            self.native_layers.0.pool[handle.0.id].mode = NativeLayerMode::Unsupported;
            self.native_layers.0.pool[handle.0.id].closed = true;
        } else {
            self.platform_ops.push_back(CxOsOp::NativeLayerCreate { id: handle.id(), content });
        }
        handle
    }

    pub fn native_layer(&mut self, id: NativeLayerId) -> NativeLayerHandle<'_> {
        NativeLayerHandle { cx: self, id }
    }

    pub(crate) fn native_layer_mode(&self, id: NativeLayerId) -> NativeLayerMode {
        self.native_layers.get(id).filter(|l| !l.closed).map_or(NativeLayerMode::Unsupported, |l| l.mode.clone())
    }

    pub(crate) fn native_layer_window_pass(&self, pass: DrawPassId) -> bool {
        matches!(self.passes[pass].parent, CxDrawPassParent::Window(w) if self.windows[w].main_pass_id == Some(pass))
    }

    pub(crate) fn native_layer_retire(&mut self) {
        while self.native_layers.0.has_pending_retirements() {
            let Some(index) = self.native_layers.0.take_free_retirement() else { continue };
            let slot = &mut self.native_layers.0.pool[index];
            if !slot.closed {
                slot.closed = true;
                slot.mode = NativeLayerMode::Unsupported;
                self.platform_ops.push_back(CxOsOp::NativeLayerDestroy {
                    id: NativeLayerId { index, generation: slot.generation },
                });
            }
        }
    }

    pub(crate) fn native_layer_event(&mut self, layer: NativeLayerId, kind: NativeLayerEventKind) {
        if self.native_layers.get(layer).is_none() { return; }
        match &kind {
            NativeLayerEventKind::Realized(mode) if !self.native_layers.0.pool[layer.index].closed => {
                self.native_layers.0.pool[layer.index].mode = mode.clone();
                self.redraw_all();
            }
            NativeLayerEventKind::Closed => {
                self.native_layers.0.pool[layer.index].closed = true;
                self.native_layers.0.pool[layer.index].mode = NativeLayerMode::Unsupported;
                self.redraw_all();
            }
            _ => {}
        }
        self.call_event_handler(&Event::NativeLayer(NativeLayerEvent { layer, kind }));
    }
}

pub struct NativeLayerHandle<'a> {
    cx: &'a mut Cx,
    id: NativeLayerId,
}

impl NativeLayerHandle<'_> {
    fn op(&mut self, op: NativeLayerOperation) {
        if self.cx.script_data.std.host_io_only() { return; }
        if self.cx.native_layers.get(self.id).is_some_and(|l| !l.closed) {
            self.cx.platform_ops.push_back(CxOsOp::NativeLayerOp { id: self.id, op });
        }
    }
    pub fn navigate(&mut self, url: &str, replace: bool) {
        self.op(NativeLayerOperation::Navigate { url: url.to_owned(), replace });
    }
    pub fn history(&mut self, delta: i32) { self.op(NativeLayerOperation::History(delta)); }
    pub fn reload(&mut self) { self.op(NativeLayerOperation::Reload); }
    pub fn stop(&mut self) { self.op(NativeLayerOperation::Stop); }
    pub fn appearance(&mut self, dark: bool, background: [f32; 4]) {
        self.op(NativeLayerOperation::Appearance { dark, background });
    }
    pub fn focus(&mut self) { self.op(NativeLayerOperation::Focus); }
    pub fn blur(&mut self) { self.op(NativeLayerOperation::Blur); }
    pub fn input(&mut self, packed: &[u8]) { self.op(NativeLayerOperation::Input(packed.to_vec())); }
    pub fn size(&mut self, width: u32, height: u32) { self.op(NativeLayerOperation::Size { width, height }); }
    pub fn camera_source(&mut self, input: VideoInputId, format: VideoFormatId) {
        self.op(NativeLayerOperation::CameraSource { input, format });
    }
    pub fn close(&mut self) {
        if self.cx.native_layers.get(self.id).is_some_and(|l| !l.closed) {
            self.cx.native_layers.0.pool[self.id.index].closed = true;
            self.cx.native_layers.0.pool[self.id.index].mode = NativeLayerMode::Unsupported;
            self.cx.platform_ops.push_back(CxOsOp::NativeLayerDestroy { id: self.id });
            self.cx.redraw_all();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Cx {
    pub(crate) fn hal_native_layer_create(&mut self, id: NativeLayerId, content: NativeContent) {
        use makepad_hal::native_layer::kind;
        match content {
            NativeContent::WaylandApp { app } => self.hal.frames.w.native_layer_create(id.hal(), kind::FOREIGN_WAYLAND, app.as_bytes()),
            NativeContent::Web { url } => self.hal.frames.w.native_layer_create(id.hal(), kind::WEB, url.as_bytes()),
            NativeContent::CameraPreview { input, format, .. } => {
                let mut params = [0; 16];
                params[..8].copy_from_slice(&input.0.0.to_le_bytes());
                params[8..].copy_from_slice(&format.0.0.to_le_bytes());
                self.hal.frames.w.native_layer_create(id.hal(), kind::CAMERA_PREVIEW, &params);
            }
        }
    }

    pub(crate) fn hal_native_layer_op(&mut self, id: NativeLayerId, operation: NativeLayerOperation) {
        use makepad_hal::native_layer::op;
        let w = &mut self.hal.frames.w;
        let id = id.hal();
        match operation {
            NativeLayerOperation::Navigate { url, replace } => w.native_layer_op(id, op::NAVIGATE, replace as u64, url.as_bytes()),
            NativeLayerOperation::History(delta) => w.native_layer_op(id, op::HISTORY, delta as i64 as u64, &[]),
            NativeLayerOperation::Reload => w.native_layer_op(id, op::RELOAD, 0, &[]),
            NativeLayerOperation::Stop => w.native_layer_op(id, op::STOP, 0, &[]),
            NativeLayerOperation::Appearance { dark, background } => {
                let mut data = [0; 16];
                for (dst, value) in data.chunks_exact_mut(4).zip(background) {
                    dst.copy_from_slice(&value.to_le_bytes());
                }
                w.native_layer_op(id, op::APPEARANCE, dark as u64, &data);
            }
            NativeLayerOperation::Focus => w.native_layer_op(id, op::FOCUS, 0, &[]),
            NativeLayerOperation::Blur => w.native_layer_op(id, op::BLUR, 0, &[]),
            NativeLayerOperation::Input(data) => w.native_layer_op(id, op::INPUT, 0, &data),
            NativeLayerOperation::Size { width, height } => {
                let mut data = [0u8; 8];
                data[..4].copy_from_slice(&width.to_le_bytes());
                data[4..].copy_from_slice(&height.to_le_bytes());
                w.native_layer_op(id, op::SIZE, 0, &data);
            }
            NativeLayerOperation::CameraSource { input, format } => w.native_layer_op(id, op::CAMERA_SOURCE, input.0.0, &format.0.0.to_le_bytes()),
        }
    }

    pub(crate) fn hal_native_layer_state(&mut self, id: makepad_hal::Id, what: u32, a: u64, b: u64, data: &[u8]) {
        use makepad_hal::native_layer::{mode, state};
        let layer = NativeLayerId { index: id.index as usize, generation: id.gen as u64 };
        if self.native_layers.get(layer).is_none_or(|slot| slot.closed && what != state::CLOSED) { return; }
        let kind = match what {
            state::REALIZED => NativeLayerEventKind::Realized(match a {
                mode::COMPOSITED => NativeLayerMode::Composited,
                mode::TEXTURED => NativeLayerMode::Textured(Texture::from_hal_lent(self, crate::event::LentId { index: b as u32, gen: (b >> 32) as u32 })),
                _ => NativeLayerMode::Unsupported,
            }),
            state::FRAME => { self.redraw_all(); return; }
            state::TITLE => NativeLayerEventKind::Title(String::from_utf8_lossy(data).into_owned()),
            state::URL => NativeLayerEventKind::Url(String::from_utf8_lossy(data).into_owned()),
            state::LOADING => NativeLayerEventKind::Loading(a != 0),
            state::NAV => NativeLayerEventKind::Nav { can_back: a != 0, can_forward: b != 0 },
            state::FOCUS_IN => NativeLayerEventKind::FocusIn,
            state::FOCUS_OUT => NativeLayerEventKind::FocusOut,
            state::CURSOR => NativeLayerEventKind::Cursor(a as u32),
            state::CRASHED => NativeLayerEventKind::Crashed(String::from_utf8_lossy(data).into_owned()),
            state::CLOSED => NativeLayerEventKind::Closed,
            _ => return,
        };
        self.native_layer_event(layer, kind);
    }
}
