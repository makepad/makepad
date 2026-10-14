//! Compositor state machine: surfaces, subsurfaces, shm buffers, xdg_shell windows and popups,
//! seat input. It implements `Handler` and talks to the host through `UiEvent`s (out) and
//! methods taking `&mut Core` (in). No rendering and no threads here.
pub mod geom;
mod data;
pub use data::{Board, SelectionReader, TEXT_MIMES};
mod dmabuf;
mod syncobj;
pub use dmabuf::{DmabufBuffer, DmabufConfig, DmabufFrame, DmabufPlane};
mod ext;
mod input;
mod surface;
pub mod xdg;

use crate::core::{ClientId, Core, Fatal, Handler, ObjRef};
use crate::proto::*;
use crate::schema::Interface;
use crate::shm::{self, BufferSpec, Pool};
use crate::wire::Value;
pub use geom::{Positioner, Rect};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type SurfKey = (ClientId, u32);

/// Pixels handed to the host for one commit: rows `y0..y1` of the buffer, tightly packed.
#[derive(Debug)]
pub struct Frame { pub width: u32, pub height: u32, pub format: u32, pub y0: u32, pub y1: u32, pub data: Vec<u8> }
impl Frame { pub fn is_full(&self) -> bool { self.y0 == 0 && self.y1 == self.height } }

#[derive(Debug)]
pub enum BufferUpdate { Keep, Detach, Attach(Frame), Dmabuf(DmabufFrame) }

#[derive(Debug)]
pub enum UiEvent {
    /// `size` is the surface's logical size (0x0 without a buffer); `src` the viewport crop in buffer pixels.
    Commit { surface: SurfKey, buffer: BufferUpdate, scale: i32, transform: i32, input_region: Option<Vec<(Rect, bool)>>, size: (i32, i32), src: Option<(f64, f64, f64, f64)> },
    /// Child surfaces of `parent` bottom to top (`None` marks the parent's own layer) with offsets.
    SubsurfaceTree { parent: SurfKey, stack: Vec<Option<(SurfKey, (i32, i32))>> },
    ToplevelNew { surface: SurfKey },
    ToplevelTitle { surface: SurfKey, title: String },
    ToplevelAppId { surface: SurfKey, app_id: String },
    ToplevelLimits { surface: SurfKey, min: (i32, i32), max: (i32, i32) },
    ToplevelRequest { surface: SurfKey, what: ToplevelRequest },
    WindowGeometry { surface: SurfKey, rect: Rect },
    Mapped { surface: SurfKey },
    Unmapped { surface: SurfKey },
    PopupNew { surface: SurfKey, parent: SurfKey, rect: Rect, grab: bool },
    PopupMoved { surface: SurfKey, rect: Rect },
    /// The surface's role object or the surface is gone.
    SurfaceGone { surface: SurfKey },
    SetCursor { client: ClientId, surface: Option<SurfKey>, hotspot: (i32, i32) },
    /// A named cursor (wp_cursor_shape_v1 shape number).
    SetCursorShape { client: ClientId, shape: u32 },
    /// The client destroyed a dmabuf wl_buffer: the host drops its import of it.
    BufferGone { client: ClientId, buffer: u32 },
    ClientGone(ClientId),
    /// A client's selection text, read once when it set the selection (only with a selection
    /// reader installed; the host hands the read back through `Compositor::selection_read`).
    Selection { board: data::Board, text: String },
    /// The server cut the client off: a protocol error or an output queue it did not drain.
    ClientKilled { client: ClientId, why: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToplevelRequest { Move, Resize(u32), Maximize(bool), Fullscreen(bool), Minimize, WindowMenu(i32, i32) }

#[derive(Clone, Debug)]
pub struct OutputInfo { pub x: i32, pub y: i32, pub width: i32, pub height: i32, pub refresh_mhz: i32, pub scale: i32, pub name: String, pub phys_mm: (i32, i32) }
impl Default for OutputInfo {
    fn default() -> Self { Self { x: 0, y: 0, width: 1920, height: 1080, refresh_mhz: 60_000, scale: 1, name: "WM-1".into(), phys_mm: (0, 0) } }
}

pub(crate) enum Obj {
    Plain,
    Surface(surface::Surface),
    Region(Vec<(Rect, bool)>),
    Pool(Arc<Mutex<Pool>>),
    Buffer { spec: BufferSpec, pool: Arc<Mutex<Pool>> },
    Positioner(Positioner),
    XdgSurface(xdg::XdgSurface),
    Toplevel { xdg: u32 },
    Popup { xdg: u32, parent: SurfKey, positioner: Positioner, grab: bool },
    Subsurface { surface: u32 },
    Pointer,
    Keyboard,
    Viewport { surface: u32 },
    FractionalScale,
    CursorDevice,
    Decoration,
    SyncSurface { surface: u32 },
    Timeline { handle: u32 },
    DmaParams(dmabuf::Params),
    DmaBuffer(Arc<DmabufBuffer>),
    DataSource { board: data::Board, mimes: Vec<String> },
    DataDevice { board: data::Board },
    DataOffer { generation: u64 },
}

pub struct Compositor {
    pub(crate) objs: HashMap<SurfKey, Obj>,
    pub(crate) events: Vec<UiEvent>,
    pub output: OutputInfo,
    pub popup_bounds: Rect,
    pub(crate) input: input::InputState,
    /// The clipboard and the primary selection (`data::Board` order).
    pub(crate) boards: [data::BoardState; 2],
    pub(crate) selection_reader: Option<data::SelectionReader>,
    /// Output scale as clients are told it (fractional).
    pub(crate) scale: f64,
    pub(crate) fractional: Vec<(ClientId, u32)>,
    pub(crate) dmabuf: Option<dmabuf::DmabufState>,
    pub(crate) drm: Option<syncobj::Drm>,
    /// Release points of explicit-sync commits, per client buffer, oldest first.
    pub(crate) release_points: HashMap<(ClientId, u32), Vec<syncobj::Point>>,
    /// Timeline handles whose protocol object is gone (destroyed with the client).
    pub(crate) timelines: Vec<(ClientId, u32)>,
    pub(crate) outputs: Vec<(ClientId, u32)>,
    globals: Vec<u32>,
}

impl Compositor {
    /// Registers the globals. Call once after creating the server.
    pub fn new(core: &mut Core, output: OutputInfo, keymap: &'static xkb_keymap::Layout) -> std::io::Result<Self> {
        let mut globals = Vec::new();
        globals.push(core.add_global(&wl_compositor::INTERFACE, 6));
        globals.push(core.add_global(&wl_subcompositor::INTERFACE, 1));
        globals.push(core.add_global(&wl_shm::INTERFACE, 1));
        globals.push(core.add_global(&xdg_wm_base::INTERFACE, 6));
        globals.push(core.add_global(&wl_seat::INTERFACE, 8));
        globals.push(core.add_global(&wl_output::INTERFACE, 4));
        globals.push(core.add_global(&wl_data_device_manager::INTERFACE, 3));
        globals.push(core.add_global(&zwp_primary_selection_device_manager_v1::INTERFACE, 1));
        globals.push(core.add_global(&wp_viewporter::INTERFACE, 1));
        globals.push(core.add_global(&wp_fractional_scale_manager_v1::INTERFACE, 1));
        globals.push(core.add_global(&wp_cursor_shape_manager_v1::INTERFACE, 1));
        globals.push(core.add_global(&zxdg_decoration_manager_v1::INTERFACE, 1));
        Ok(Self {
            objs: HashMap::new(), events: Vec::new(), output,
            popup_bounds: Rect::new(-100_000, -100_000, 200_000, 200_000),
            input: input::InputState::new(keymap)?, boards: Default::default(), selection_reader: None, scale: 1.0, fractional: Vec::new(), dmabuf: None, drm: None, release_points: HashMap::new(), timelines: Vec::new(), outputs: Vec::new(), globals,
        })
    }
        pub(crate) fn surface(&mut self, k: SurfKey) -> Option<&mut surface::Surface> {
        match self.objs.get_mut(&k) { Some(Obj::Surface(s)) => Some(s), _ => None }
    }
    pub fn output_global_names(&self) -> &[u32] { &self.globals }
}

fn uint(v: &Value) -> u32 { v.as_uint() }

impl Handler for Compositor {
    fn bind(&mut self, core: &mut Core, c: ClientId, _g: u32, iface: &'static Interface, version: u32, id: u32) -> Result<(), Fatal> {
        self.objs.insert((c, id), Obj::Plain);
        match iface.name {
            "wl_shm" => {
                for f in [shm::ARGB8888, shm::XRGB8888, shm::ABGR8888, shm::XBGR8888] {
                    let _ = core.send(c, id, wl_shm::EVT_FORMAT, vec![Value::Uint(f)]);
                }
            }
            "wl_seat" => {
                let _ = core.send(c, id, wl_seat::EVT_CAPABILITIES, vec![Value::Uint(3)]);
                let _ = core.send(c, id, wl_seat::EVT_NAME, vec![Value::Str(Some("seat0".into()))]);
            }
            "zwp_linux_dmabuf_v1" => self.dmabuf_bind(core, c, id, version),
            "wl_output" => { self.outputs.push((c, id)); self.send_output(core, c, id, version) }
            _ => {}
        }
        Ok(())
    }

    fn request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, mut args: Vec<Value>) -> Result<(), Fatal> {
        match o.iface.name {
            "wp_viewporter" | "wp_viewport" | "wp_fractional_scale_manager_v1" | "wp_cursor_shape_manager_v1" | "wp_cursor_shape_device_v1" | "zxdg_decoration_manager_v1" | "zxdg_toplevel_decoration_v1" => self.ext_request(core, c, o, op, &args)?,
            "wp_linux_drm_syncobj_manager_v1" | "wp_linux_drm_syncobj_surface_v1" | "wp_linux_drm_syncobj_timeline_v1" => self.syncobj_request(core, c, o, op, &mut args)?,
            "zwp_linux_dmabuf_v1" | "zwp_linux_buffer_params_v1" => self.dmabuf_request(core, c, o, op, &mut args)?,
            "wl_data_device_manager" | "wl_data_source" | "wl_data_device" | "wl_data_offer"
            | "zwp_primary_selection_device_manager_v1" | "zwp_primary_selection_source_v1" | "zwp_primary_selection_device_v1" | "zwp_primary_selection_offer_v1" => self.data_request(core, c, o, op, &mut args)?,
            "wl_compositor" => match op {
                wl_compositor::REQ_CREATE_SURFACE => {
                    self.objs.insert((c, uint(&args[0])), Obj::Surface(surface::Surface::new()));
                }
                wl_compositor::REQ_CREATE_REGION => { self.objs.insert((c, uint(&args[0])), Obj::Region(Vec::new())); }
                _ => {}
            },
            "wl_region" => if let (true, Some(Obj::Region(r))) = (op != wl_region::REQ_DESTROY, self.objs.get_mut(&(c, o.id))) {
                let rect = Rect::new(args[0].as_int(), args[1].as_int(), args[2].as_int(), args[3].as_int());
                match op { wl_region::REQ_ADD => r.push((rect, true)), wl_region::REQ_SUBTRACT => r.push((rect, false)), _ => {} }
            },
            "wl_shm" => if op == wl_shm::REQ_CREATE_POOL {
                let mut it = args.into_iter();
                let id = it.next().map_or(0, |v| v.as_uint());
                let Some(Value::Fd(fd)) = it.next() else { return Ok(()) };
                let size = it.next().map_or(0, |v| v.as_int());
                self.create_pool(c, o.id, id, fd, size)?;
            },
            "wl_shm_pool" => self.pool_request(core, c, o, op, &args)?,
            "wl_buffer" => {}
            "wl_surface" => self.surface_request(core, c, o, op, &args)?,
            "wl_subcompositor" => if op == wl_subcompositor::REQ_GET_SUBSURFACE { self.get_subsurface(core, c, o.id, &args)?; },
            "wl_subsurface" => self.subsurface_request(core, c, o, op, &args)?,
            "xdg_wm_base" | "xdg_positioner" | "xdg_surface" | "xdg_toplevel" | "xdg_popup" => self.xdg_request(core, c, o, op, &args)?,
            "wl_seat" => self.seat_request(core, c, o, op, &args),
            "wl_pointer" => if op == wl_pointer::REQ_SET_CURSOR {
                let surf = args[1].as_uint();
                self.events.push(UiEvent::SetCursor { client: c, surface: (surf != 0).then_some((c, surf)), hotspot: (args[2].as_int(), args[3].as_int()) });
            },
            _ => {}
        }
        Ok(())
    }

    fn destroyed(&mut self, core: &mut Core, c: ClientId, o: ObjRef) {
        self.object_destroyed(core, c, o);
    }
    fn client_gone(&mut self, _core: &mut Core, c: ClientId) {
        if let Some(d) = &self.drm { for (_, h) in self.timelines.iter().filter(|(k, _)| *k == c) { d.destroy(*h); } }
        self.timelines.retain(|(k, _)| *k != c);
        self.input.client_gone(c);
        self.data_client_gone(c);
        self.events.push(UiEvent::ClientGone(c));
    }
}
