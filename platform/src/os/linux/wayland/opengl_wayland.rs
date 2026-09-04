#![allow(unused_imports)]
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::Once;

use crate::egl_sys::{EGLNativeWindowType, EGLSurface, NativeWindowType};
use crate::makepad_math::Vec2d;
use wayland_client::protocol::__interfaces::WL_OUTPUT_INTERFACE;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_region, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface,
    wl_surface,
};
use wayland_client::{Proxy, QueueHandle};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell;
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols::xdg::toplevel_icon::v1::client::{
    xdg_toplevel_icon_manager_v1, xdg_toplevel_icon_v1,
};

use crate::opengl_cx::OpenglCx;
use crate::screen::DEFAULT_WINDOW_SIZE;
use crate::wayland::wayland_state::WaylandState;
use crate::{
    egl_sys, event::WindowGeom, WaylandDecorationPreference, WindowId,
};

/// Wraps a `wl_egl_window` in an `EGLSurface`, or returns null when the driver refuses it —
/// which it does for an extent it cannot allocate a buffer for, however willingly
/// `wl_egl_window_create` accepted the same numbers.
fn create_egl_window_surface(opengl_cx: &OpenglCx, wl_egl_surface: &WlEglSurface) -> EGLSurface {
    unsafe {
        (opengl_cx.libegl.eglCreateWindowSurface.unwrap())(
            opengl_cx.egl_display,
            opengl_cx.egl_config,
            wl_egl_surface.ptr() as NativeWindowType,
            std::ptr::null(),
        )
    }
}

fn initially_uses_client_side_decorations(
    decoration_manager_available: bool,
    preference: WaylandDecorationPreference,
) -> bool {
    !decoration_manager_available || preference == WaylandDecorationPreference::ClientSide
}

// libadwaita 1.9's light-theme CSD profile. The 25 px margin is also what
// native GNOME applications include around their xdg window geometry.
const CSD_SHADOW_MARGIN: i32 = 25;
// How far a corner tile reaches along each edge before the straight strips take
// over. A square corner's influence dies out where the widest layer's coverage
// reaches 1 within half a quantization step: 0.15 * (1 - phi((5 + k) / 7)) < 1/510
// at k = 10.5, so 16 px leaves better than five px of margin on the seam.
const CSD_SHADOW_CORNER_INSET: i32 = 16;
const CSD_SHADOW_CORNER_SIZE: i32 = CSD_SHADOW_MARGIN + CSD_SHADOW_CORNER_INSET;
// How far the resize grab reaches outward from the window edge into the gutter.
// libadwaita sets its toplevel input region to the window rect grown by exactly
// this much; the rest of the gutter stays click-through so a window's shadow
// never steals a click from whatever is behind it.
const CSD_SHADOW_GRAB: i32 = 12;
static CSD_SHADOW_UNAVAILABLE_LOGGED: Once = Once::new();

/// One CSS `box-shadow` layer: the window rectangle grown by `spread`, blurred by a
/// Gaussian of standard deviation `sigma` (half the CSS blur radius), painted at
/// `alpha`. A zero `sigma` is an unblurred hard edge.
struct CsdShadowLayer {
    sigma: f64,
    spread: f64,
    alpha: f64,
}

/// libadwaita 1.9 `window.csd`, the profile every GNOME 50 app on a stock desktop casts:
/// `box-shadow: 0 0 14px 5px rgb(0 0 0/15%), 0 0 5px 2px rgb(0 0 0/10%), 0 0 0 1px rgb(0 0 0/5%)`
const CSD_SHADOW_ACTIVE_LAYERS: &[CsdShadowLayer] = &[
    CsdShadowLayer { sigma: 7.0, spread: 5.0, alpha: 0.15 },
    CsdShadowLayer { sigma: 2.5, spread: 2.0, alpha: 0.10 },
    CsdShadowLayer { sigma: 0.0, spread: 1.0, alpha: 0.05 },
];

/// libadwaita 1.9 `window.csd:backdrop`. Its first term is `0 0 14px 5px transparent`,
/// which exists only to keep the shadow's extent identical to the focused profile, so
/// losing focus never changes any geometry. Being transparent it is omitted here.
const CSD_SHADOW_INACTIVE_LAYERS: &[CsdShadowLayer] = &[
    CsdShadowLayer { sigma: 5.0, spread: 5.0, alpha: 0.08 },
    CsdShadowLayer { sigma: 0.0, spread: 1.0, alpha: 0.05 },
];

/// Abramowitz & Stegun 7.1.26, whose 1.5e-7 worst-case error is three orders of
/// magnitude below the 1/255 the result is quantized to.
fn csd_erf(x: f64) -> f64 {
    const P: f64 = 0.3275911;
    const A: [f64; 5] = [
        0.254829592,
        -0.284496736,
        1.421413741,
        -1.453152027,
        1.061405429,
    ];
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + P * x);
    let poly = A.iter().rev().fold(0.0, |acc, a| (acc + a) * t);
    sign * (1.0 - poly * (-x * x).exp())
}

/// The fraction of one blurred half-plane covering a point `t` px outside its edge.
/// `t` is signed, so a negative value is inside the shadow rectangle.
fn csd_shadow_coverage(layer: &CsdShadowLayer, t: f64) -> f64 {
    if layer.sigma <= 0.0 {
        return if t < layer.spread { 1.0 } else { 0.0 };
    }
    let z = (layer.spread - t) / (layer.sigma * std::f64::consts::SQRT_2);
    0.5 * (1.0 + csd_erf(z))
}

/// Composited shadow alpha at a point `tx` px outside the window's nearer vertical
/// edge and `ty` px outside its nearer horizontal edge, both signed. A rectangle's
/// Gaussian shadow is separable, so each layer's 2-D coverage is the product of its
/// two 1-D coverages; `f64::NEG_INFINITY` means "far enough inside that this axis
/// contributes full coverage", which is what an edge strip passes for the axis it
/// is constant along.
fn csd_shadow_alpha(layers: &[CsdShadowLayer], tx: f64, ty: f64) -> u32 {
    let transmission = layers.iter().fold(1.0, |acc, layer| {
        let coverage = csd_shadow_coverage(layer, tx) * csd_shadow_coverage(layer, ty);
        acc * (1.0 - layer.alpha * coverage)
    });
    ((1.0 - transmission) * 255.0).round() as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CsdShadowPieceKind {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

const CSD_SHADOW_PIECES: [CsdShadowPieceKind; 8] = [
    CsdShadowPieceKind::TopLeft,
    CsdShadowPieceKind::Top,
    CsdShadowPieceKind::TopRight,
    CsdShadowPieceKind::Left,
    CsdShadowPieceKind::Right,
    CsdShadowPieceKind::BottomLeft,
    CsdShadowPieceKind::Bottom,
    CsdShadowPieceKind::BottomRight,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CsdShadowRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn csd_shadow_rect(kind: CsdShadowPieceKind, width: i32, height: i32) -> CsdShadowRect {
    let m = CSD_SHADOW_MARGIN;
    let i = CSD_SHADOW_CORNER_INSET;
    let c = CSD_SHADOW_CORNER_SIZE;
    match kind {
        CsdShadowPieceKind::TopLeft => CsdShadowRect {
            x: -m,
            y: -m,
            width: c,
            height: c,
        },
        CsdShadowPieceKind::Top => CsdShadowRect {
            x: i,
            y: -m,
            width: width - 2 * i,
            height: m,
        },
        CsdShadowPieceKind::TopRight => CsdShadowRect {
            x: width - i,
            y: -m,
            width: c,
            height: c,
        },
        CsdShadowPieceKind::Left => CsdShadowRect {
            x: -m,
            y: i,
            width: m,
            height: height - 2 * i,
        },
        CsdShadowPieceKind::Right => CsdShadowRect {
            x: width,
            y: i,
            width: m,
            height: height - 2 * i,
        },
        CsdShadowPieceKind::BottomLeft => CsdShadowRect {
            x: -m,
            y: height - i,
            width: c,
            height: c,
        },
        CsdShadowPieceKind::Bottom => CsdShadowRect {
            x: i,
            y: height,
            width: width - 2 * i,
            height: m,
        },
        CsdShadowPieceKind::BottomRight => CsdShadowRect {
            x: width - i,
            y: height - i,
            width: c,
            height: c,
        },
    }
}

fn csd_shadow_buffer_size(kind: CsdShadowPieceKind) -> (i32, i32) {
    match kind {
        CsdShadowPieceKind::TopLeft
        | CsdShadowPieceKind::TopRight
        | CsdShadowPieceKind::BottomLeft
        | CsdShadowPieceKind::BottomRight => (CSD_SHADOW_CORNER_SIZE, CSD_SHADOW_CORNER_SIZE),
        CsdShadowPieceKind::Top | CsdShadowPieceKind::Bottom => (1, CSD_SHADOW_MARGIN),
        CsdShadowPieceKind::Left | CsdShadowPieceKind::Right => (CSD_SHADOW_MARGIN, 1),
    }
}

/// Where buffer pixel `(x, y)` of a piece sits relative to the window, as the signed
/// distance outside the nearer vertical edge and the nearer horizontal edge. Sampling
/// at pixel centres keeps the tiles seam-free against the strips they abut.
///
/// Makepad's window is a plain rectangle, so both distances are measured from a square
/// corner. Sampling a rounded window's shadow here instead would leave the corners
/// visibly washed out, because a rounded corner's geometry recedes from the square
/// corner the shadow has to hug.
fn csd_shadow_offsets(kind: CsdShadowPieceKind, x: i32, y: i32) -> (f64, f64) {
    const INSIDE: f64 = f64::NEG_INFINITY;
    let margin = CSD_SHADOW_MARGIN as f64;
    let inset = CSD_SHADOW_CORNER_INSET as f64;
    let x = x as f64 + 0.5;
    let y = y as f64 + 0.5;
    // Left-hand and top pieces start `margin` px before the window; right-hand and
    // bottom corner pieces start `inset` px inside it, and their strips start on it.
    let past_left = margin - x;
    let past_top = margin - y;
    let past_right_corner = x - inset;
    let past_bottom_corner = y - inset;
    match kind {
        CsdShadowPieceKind::TopLeft => (past_left, past_top),
        CsdShadowPieceKind::Top => (INSIDE, past_top),
        CsdShadowPieceKind::TopRight => (past_right_corner, past_top),
        CsdShadowPieceKind::Left => (past_left, INSIDE),
        CsdShadowPieceKind::Right => (x, INSIDE),
        CsdShadowPieceKind::BottomLeft => (past_left, past_bottom_corner),
        CsdShadowPieceKind::Bottom => (INSIDE, y),
        CsdShadowPieceKind::BottomRight => (past_right_corner, past_bottom_corner),
    }
}

fn csd_shadow_pixel(kind: CsdShadowPieceKind, x: i32, y: i32, active: bool) -> u32 {
    let layers = if active {
        CSD_SHADOW_ACTIVE_LAYERS
    } else {
        CSD_SHADOW_INACTIVE_LAYERS
    };
    let (tx, ty) = csd_shadow_offsets(kind, x, y);
    // Premultiplied ARGB8888 black: with all three colour channels at zero the alpha
    // is already the premultiplied value wl_shm requires.
    csd_shadow_alpha(layers, tx, ty) << 24
}

/// The part of a piece that grabs the pointer for a resize, in its own surface-local
/// coordinates. The union of these across the eight pieces is the window rectangle
/// grown by [`CSD_SHADOW_GRAB`], which is exactly the input region libadwaita gives
/// its toplevels, minus the window itself (which the parent surface covers anyway).
///
/// Each piece maps to one resize edge, so where the pointer landed is enough to know
/// which edge it grabbed and no coordinate hit-testing is needed.
fn csd_shadow_grab_rect(kind: CsdShadowPieceKind) -> CsdShadowRect {
    // `wl_surface.set_input_region` ignores whatever falls outside the surface, so a
    // span longer than any window keeps the stretched axis correct without ever
    // needing to be re-sent on resize.
    const SPAN: i32 = 1 << 20;
    let outer = CSD_SHADOW_MARGIN - CSD_SHADOW_GRAB;
    let inner = CSD_SHADOW_CORNER_INSET + CSD_SHADOW_GRAB;
    let grab = CSD_SHADOW_GRAB;
    let rect = |x, y, width, height| CsdShadowRect { x, y, width, height };
    match kind {
        CsdShadowPieceKind::TopLeft => rect(outer, outer, SPAN, SPAN),
        CsdShadowPieceKind::Top => rect(0, outer, SPAN, grab),
        CsdShadowPieceKind::TopRight => rect(0, outer, inner, SPAN),
        CsdShadowPieceKind::Left => rect(outer, 0, grab, SPAN),
        CsdShadowPieceKind::Right => rect(0, 0, grab, SPAN),
        CsdShadowPieceKind::BottomLeft => rect(outer, 0, SPAN, inner),
        CsdShadowPieceKind::Bottom => rect(0, 0, SPAN, grab),
        CsdShadowPieceKind::BottomRight => rect(0, 0, inner, inner),
    }
}

/// The edge a grab on this piece resizes.
fn csd_shadow_resize_edge(kind: CsdShadowPieceKind) -> xdg_toplevel::ResizeEdge {
    use xdg_toplevel::ResizeEdge;
    match kind {
        CsdShadowPieceKind::TopLeft => ResizeEdge::TopLeft,
        CsdShadowPieceKind::Top => ResizeEdge::Top,
        CsdShadowPieceKind::TopRight => ResizeEdge::TopRight,
        CsdShadowPieceKind::Left => ResizeEdge::Left,
        CsdShadowPieceKind::Right => ResizeEdge::Right,
        CsdShadowPieceKind::BottomLeft => ResizeEdge::BottomLeft,
        CsdShadowPieceKind::Bottom => ResizeEdge::Bottom,
        CsdShadowPieceKind::BottomRight => ResizeEdge::BottomRight,
    }
}

struct CsdShadowPiece {
    kind: CsdShadowPieceKind,
    surface: wl_surface::WlSurface,
    subsurface: wl_subsurface::WlSubsurface,
    viewport: wp_viewport::WpViewport,
    active_buffer: wl_buffer::WlBuffer,
    inactive_buffer: wl_buffer::WlBuffer,
}

struct WaylandCsdShadow {
    pieces: Vec<CsdShadowPiece>,
    state: Option<(i32, i32, bool, bool)>,
}

fn csd_shadow_visible_at_size(visible: bool, width: i32, height: i32) -> bool {
    visible
        && width > 2 * CSD_SHADOW_CORNER_INSET
        && height > 2 * CSD_SHADOW_CORNER_INSET
}

impl WaylandCsdShadow {
    fn new(
        compositor: &wl_compositor::WlCompositor,
        subcompositor: Option<&wl_subcompositor::WlSubcompositor>,
        shm: Option<&wl_shm::WlShm>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        parent: &wl_surface::WlSurface,
        qhandle: &QueueHandle<WaylandState>,
    ) -> Option<Self> {
        let (Some(subcompositor), Some(shm), Some(viewporter)) =
            (subcompositor, shm, viewporter)
        else {
            CSD_SHADOW_UNAVAILABLE_LOGGED.call_once(|| {
                crate::warning!(
                    "Wayland client-side shadow unavailable: wl_subcompositor, wl_shm, and \
                     wp_viewporter are required; continuing with client-side window controls"
                );
            });
            return None;
        };
        let Some(buffers) = Self::create_buffers(shm, qhandle) else {
            CSD_SHADOW_UNAVAILABLE_LOGGED.call_once(|| {
                crate::warning!(
                    "Wayland client-side shadow unavailable: could not allocate shared-memory \
                     buffers; continuing with client-side window controls"
                );
            });
            return None;
        };
        let mut pieces = Vec::with_capacity(CSD_SHADOW_PIECES.len());
        for (kind, (active_buffer, inactive_buffer)) in
            CSD_SHADOW_PIECES.into_iter().zip(buffers)
        {
            let surface = compositor.create_surface(qhandle, ());
            // The gutter is where this window is resized from, the way it is for every
            // native app on the desktop: the pointer never has to compete with a widget
            // for the edge, so the close button no longer swallows the top-right corner.
            // Input regions are copied by the compositor at request time, and these are
            // expressed so they survive any resize, so this is the only time they are set.
            let grab = csd_shadow_grab_rect(kind);
            let region = compositor.create_region(qhandle, ());
            region.add(grab.x, grab.y, grab.width, grab.height);
            surface.set_input_region(Some(&region));
            region.destroy();
            let subsurface = subcompositor.get_subsurface(&surface, parent, qhandle, ());
            subsurface.set_sync();
            subsurface.place_below(parent);
            let viewport = viewporter.get_viewport(&surface, qhandle, ());
            pieces.push(CsdShadowPiece {
                kind,
                surface,
                subsurface,
                viewport,
                active_buffer,
                inactive_buffer,
            });
        }
        Some(Self {
            pieces,
            state: None,
        })
    }

    /// The piece owning `surface`, if any. The pointer entering one means the pointer is
    /// in this window's gutter rather than in the window.
    fn piece_kind_for_surface(
        &self,
        surface_id: &wayland_client::backend::ObjectId,
    ) -> Option<CsdShadowPieceKind> {
        self.pieces
            .iter()
            .find(|piece| piece.surface.id() == *surface_id)
            .map(|piece| piece.kind)
    }

    fn create_buffers(
        shm: &wl_shm::WlShm,
        qhandle: &QueueHandle<WaylandState>,
    ) -> Option<Vec<(wl_buffer::WlBuffer, wl_buffer::WlBuffer)>> {
        let mut offset = 0;
        let layouts: Vec<_> = CSD_SHADOW_PIECES
            .into_iter()
            .map(|kind| {
                let (width, height) = csd_shadow_buffer_size(kind);
                let layout = (kind, width, height, offset);
                offset += (width * height * 4) as usize;
                layout
            })
            .collect();
        let style_bytes = offset;
        let total_bytes = style_bytes * 2;
        let name = std::ffi::CString::new("makepad-csd-shadow").ok()?;
        let raw_fd =
            unsafe { crate::libc_sys::memfd_create(name.as_ptr(), crate::libc_sys::MFD_CLOEXEC) };
        if raw_fd < 0 {
            return None;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_fd) };
        if unsafe { crate::libc_sys::ftruncate(fd.as_raw_fd(), total_bytes as i64) } != 0 {
            return None;
        }
        let map = unsafe {
            crate::libc_sys::mmap(
                std::ptr::null_mut(),
                total_bytes,
                crate::libc_sys::PROT_READ | crate::libc_sys::PROT_WRITE,
                crate::libc_sys::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == crate::libc_sys::MAP_FAILED {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), total_bytes) };
        for (style_index, active) in [true, false].into_iter().enumerate() {
            for &(kind, width, height, offset) in &layouts {
                let piece_offset = style_index * style_bytes + offset;
                for y in 0..height {
                    for x in 0..width {
                        let pixel_offset = piece_offset
                            + ((y * width + x) * 4) as usize;
                        bytes[pixel_offset..pixel_offset + 4]
                            // wl_shm's ARGB8888 is defined as little endian regardless of
                            // the host, so the byte order cannot follow the CPU's.
                            .copy_from_slice(&csd_shadow_pixel(kind, x, y, active).to_le_bytes());
                    }
                }
            }
        }
        unsafe {
            crate::libc_sys::munmap(map, total_bytes);
        }

        let pool = shm.create_pool(fd.as_fd(), total_bytes as i32, qhandle, ());
        let create_buffer = |offset: usize, width: i32, height: i32| {
                pool.create_buffer(
                    offset as i32,
                    width,
                    height,
                    width * 4,
                    wl_shm::Format::Argb8888,
                    qhandle,
                    (),
                )
        };
        let buffers = layouts
            .into_iter()
            .map(|(_, width, height, offset)| {
                (
                    create_buffer(offset, width, height),
                    create_buffer(style_bytes + offset, width, height),
                )
            })
            .collect();
        pool.destroy();
        Some(buffers)
    }

    fn is_visible(&self) -> bool {
        self.state.is_some_and(|state| state.2)
    }

    fn needs_update(&self, width: i32, height: i32, visible: bool, active: bool) -> bool {
        let visible = csd_shadow_visible_at_size(visible, width, height);
        let active = visible && active;
        self.state != Some((width, height, visible, active))
    }

    fn update(&mut self, width: i32, height: i32, visible: bool, active: bool) -> bool {
        // Nine-patch edge destinations must be positive. Makepad's 200x120 minimum is
        // well above this; suppress the visual-only shadow for pathological sizes.
        let visible = csd_shadow_visible_at_size(visible, width, height);
        // Focus has no visual effect while detached. Ignoring it here also avoids
        // repainting maximized, tiled, fullscreen, and server-decorated windows.
        let active = visible && active;
        let size_changed = self.state.map(|state| (state.0, state.1)) != Some((width, height));
        if !self.needs_update(width, height, visible, active) {
            return false;
        }
        let was_visible = self.state.is_some_and(|state| state.2);
        let style_changed = self.state.map_or(true, |state| state.3 != active);
        for (kind, piece) in CSD_SHADOW_PIECES.into_iter().zip(&self.pieces) {
            if visible {
                let rect = csd_shadow_rect(kind, width, height);
                // Both are sticky surface state, so a focus change — which swaps buffers
                // at an unchanged size — has no reason to re-send them.
                if !was_visible || size_changed {
                    piece.subsurface.set_position(rect.x, rect.y);
                    piece.viewport.set_destination(rect.width, rect.height);
                }
                if !was_visible || style_changed {
                    let buffer = if active {
                        &piece.active_buffer
                    } else {
                        &piece.inactive_buffer
                    };
                    piece.surface.attach(Some(buffer), 0, 0);
                }
                if !was_visible || style_changed || size_changed {
                    piece.surface.damage(0, 0, rect.width, rect.height);
                }
                piece.surface.commit();
            } else if was_visible {
                piece.surface.attach(None, 0, 0);
                piece.surface.commit();
            }
        }
        self.state = Some((width, height, visible, active));
        size_changed
    }

    fn destroy(self) {
        for piece in self.pieces {
            piece.viewport.destroy();
            piece.subsurface.destroy();
            piece.surface.destroy();
            piece.active_buffer.destroy();
            piece.inactive_buffer.destroy();
        }
    }
}

fn should_show_csd_shadow(uses_csd: bool, maximized: bool, fullscreen: bool, tiled: bool) -> bool {
    uses_csd && !maximized && !fullscreen && !tiled
}

pub(crate) struct WaylandWindow {
    pub window_id: WindowId,
    pub base_surface: wl_surface::WlSurface,
    pub toplevel: xdg_toplevel::XdgToplevel,
    pub decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    pub uses_client_side_decorations: bool,
    pub pending_client_side_decorations: Option<bool>,
    pub is_maximized: bool,
    pub is_fullscreen: bool,
    pub is_tiled: bool,
    pub is_active: bool,
    pub unavailable_resize_edges: u8,
    pub xdg_surface: xdg_surface::XdgSurface,
    pub viewport: Option<wp_viewport::WpViewport>,
    pub fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    pub configured: bool,
    pub window_geom: WindowGeom,
    pub cal_size: Vec2d,
    pub wl_egl_surface: WlEglSurface,
    pub egl_surface: EGLSurface,
    csd_shadow: Option<WaylandCsdShadow>,
    /// The `(width, height, opaque)` the surface's opaque region was last set from, so a
    /// steady-state frame re-sends nothing.
    opaque_region_state: Option<(i32, i32, bool)>,
}

impl WaylandWindow {
    pub fn new(
        window_id: WindowId,
        compositer: &wl_compositor::WlCompositor,
        subcompositor: Option<&wl_subcompositor::WlSubcompositor>,
        wm_base: &xdg_wm_base::XdgWmBase,
        decoration_manager: Option<&zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
        scale_manager: Option<&wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        icon_manager: Option<&xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
        shm: Option<&wl_shm::WlShm>,
        qhandle: &QueueHandle<WaylandState>,
        opengl_cx: &OpenglCx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
        app_id: &str,
        is_fullscreen: bool,
        decoration_preference: WaylandDecorationPreference,
    ) -> WaylandWindow {
        // Checked "downcast" of the EGL platform display to a X11 display.
        assert_eq!(opengl_cx.egl_platform, egl_sys::EGL_PLATFORM_WAYLAND_KHR);

        let base_surface = compositer.create_surface(qhandle, ());
        let fractional_scale = scale_manager
            .map(|manager| manager.get_fractional_scale(&base_surface, qhandle, window_id));
        let viewport = viewporter.map(|vp| vp.get_viewport(&base_surface, qhandle, ()));

        let shell_surface = wm_base.get_xdg_surface(&base_surface, qhandle, window_id);
        let toplevel = shell_surface.get_toplevel(qhandle, window_id);
        toplevel.set_title(String::from(title));
        toplevel.set_app_id(app_id.to_owned());

        // Set window icon via xdg-toplevel-icon-v1 if compositor supports it
        Self::set_wayland_icon(icon_manager, shm, &toplevel, qhandle);

        let uses_client_side_decorations = initially_uses_client_side_decorations(
            decoration_manager.is_some(),
            decoration_preference,
        );
        let decoration = decoration_manager.and_then(|manager| {
            if decoration_preference == WaylandDecorationPreference::ClientSide {
                // Without negotiation the protocol requires clients to self-decorate;
                // set_mode(ClientSide) would only be a preference the compositor may reject.
                return None;
            }
            let decoration = manager.get_toplevel_decoration(&toplevel, qhandle, window_id);
            decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            Some(decoration)
        });

        let surface_width = (inner_size.x as i32).max(1);
        let surface_height = (inner_size.y as i32).max(1);
        // Do not allocate eight shadow buffers and subsurfaces for a window that
        // the compositor decorates. If negotiation later selects CSD, the
        // configure handler creates them before the first client-decorated frame.
        let mut csd_shadow = uses_client_side_decorations
            .then(|| {
                WaylandCsdShadow::new(
                    compositer,
                    subcompositor,
                    shm,
                    viewporter,
                    &base_surface,
                    qhandle,
                )
            })
            .flatten();
        if let Some(shadow) = csd_shadow.as_mut() {
            shell_surface.set_window_geometry(0, 0, surface_width, surface_height);
            shadow.update(
                surface_width,
                surface_height,
                should_show_csd_shadow(
                    uses_client_side_decorations,
                    false,
                    is_fullscreen,
                    false,
                ),
                false,
            );
        }

        if is_fullscreen {
            toplevel.set_fullscreen(None);
        }
        base_surface.commit();

        // `wl_egl_window_create` rejects a non-positive extent, and a float-to-int cast turns
        // both a negative and a NaN into zero, so the requested size is floored before the
        // call rather than allowed to panic an app at startup over a bad saved size.
        let egl_w = surface_width;
        let egl_h = surface_height;
        let mut wl_egl_surface = match WlEglSurface::new(base_surface.id(), egl_w, egl_h) {
            Ok(surface) => surface,
            Err(e) => {
                crate::error!("wl_egl_window_create failed at {egl_w}x{egl_h}: {e:?}");
                WlEglSurface::new(base_surface.id(), 800, 600)
                    .expect("wl_egl_window_create failed at the fallback size too")
            }
        };
        let mut egl_surface = create_egl_window_surface(opengl_cx, &wl_egl_surface);
        // `wl_egl_window_create` accepts an extent it never allocates for, so a size too large to
        // back is not refused until here. Falling back to a window that works beats taking the
        // process down: the size came from a state file, and an app that panics on startup cannot
        // rewrite the file that is panicking it, so every later launch would die the same way.
        if egl_surface.is_null() {
            crate::error!(
                "eglCreateWindowSurface failed at {egl_w}x{egl_h}; retrying at the default size"
            );
            wl_egl_surface = WlEglSurface::new(
                base_surface.id(),
                DEFAULT_WINDOW_SIZE.x as i32,
                DEFAULT_WINDOW_SIZE.y as i32,
            )
            .expect("wl_egl_window_create failed at the fallback size too");
            egl_surface = create_egl_window_surface(opengl_cx, &wl_egl_surface);
        }
        assert!(
            !egl_surface.is_null(),
            "eglCreateWindowSurface failed at the fallback size too"
        );

        // let positioner = wm_base.create_positioner(qhandle, ());
        let position = position.unwrap_or_default();

        let geom = WindowGeom {
            xr_is_presenting: false,
            can_fullscreen: false,
            is_topmost: false,
            is_fullscreen: false,
            inner_size: inner_size,
            outer_size: inner_size,
            dpi_factor: 1.0,
            position: position,
            ..Default::default()
        };
        Self {
            base_surface,
            toplevel,
            decoration,
            uses_client_side_decorations,
            pending_client_side_decorations: None,
            is_maximized: false,
            is_fullscreen,
            is_tiled: false,
            is_active: false,
            unavailable_resize_edges: 0,
            viewport,
            fractional_scale,
            configured: false,
            xdg_surface: shell_surface,
            window_id,
            cal_size: Vec2d::default(),
            window_geom: geom,
            wl_egl_surface,
            egl_surface,
            csd_shadow,
            opaque_region_state: None,
        }
    }
    /// Set the toplevel icon via xdg-toplevel-icon-v1 protocol using shm pixel data.
    /// Silently skips if the compositor does not support the protocol.
    fn set_wayland_icon(
        icon_manager: Option<&xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
        shm: Option<&wl_shm::WlShm>,
        toplevel: &xdg_toplevel::XdgToplevel,
        qhandle: &QueueHandle<WaylandState>,
    ) {
        let (icon_manager, shm) = match (icon_manager, shm) {
            (Some(im), Some(s)) => (im, s),
            _ => return, // compositor doesn't support the protocol
        };

        let icon_data = crate::app_icon::window_icon();
        let buf = match icon_data.buffers.first() {
            Some(b) => b,
            None => return,
        };

        let width = buf.width as usize;
        let height = buf.height as usize;
        // Convert RGBA8 to ARGB8888 (Wayland native byte order)
        let pixel_count = width * height;
        let shm_size = pixel_count * 4;

        // Create anonymous shm file
        let name = std::ffi::CString::new("makepad-icon").unwrap();
        let fd =
            unsafe { crate::libc_sys::memfd_create(name.as_ptr(), crate::libc_sys::MFD_CLOEXEC) };
        if fd < 0 {
            return;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        if unsafe { crate::libc_sys::ftruncate(fd.as_raw_fd(), shm_size as i64) } != 0 {
            return;
        }

        // mmap and write ARGB data
        let map = unsafe {
            crate::libc_sys::mmap(
                std::ptr::null_mut(),
                shm_size as crate::libc_sys::size_t,
                crate::libc_sys::PROT_READ | crate::libc_sys::PROT_WRITE,
                crate::libc_sys::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == crate::libc_sys::MAP_FAILED {
            return;
        }
        let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, shm_size) };
        for i in 0..pixel_count {
            let r = buf.data[i * 4];
            let g = buf.data[i * 4 + 1];
            let b = buf.data[i * 4 + 2];
            let a = buf.data[i * 4 + 3];
            // ARGB8888 in native byte order
            let argb = u32::from_ne_bytes([b, g, r, a]);
            dst[i * 4..i * 4 + 4].copy_from_slice(&argb.to_ne_bytes());
        }
        unsafe {
            crate::libc_sys::munmap(map, shm_size as crate::libc_sys::size_t);
        }

        // Create wl_shm_pool and wl_buffer
        let pool = shm.create_pool(fd.as_fd(), shm_size as i32, qhandle, ());
        let wl_buf = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            qhandle,
            (),
        );

        // Create icon, add buffer, set on toplevel
        let icon = icon_manager.create_icon(qhandle, ());
        icon.add_buffer(&wl_buf, buf.scale);
        icon_manager.set_icon(toplevel, Some(&icon));

        // The icon object and buffer can be destroyed after set_icon
        icon.destroy();
        pool.destroy();
        // wl_buf kept alive until compositor reads it (destroyed on drop)
    }

    pub fn prepare_buffer_size(&mut self, opengl_cx: &OpenglCx) -> bool {
        let cal_size = Vec2d {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor,
        };
        if self.cal_size != cal_size {
            // NVIDIA's Wayland EGL platform may defer resizing a non-current
            // EGLSurface until its next swap. Bind this exact surface first so
            // the next frame cannot mix the old buffer with the new viewport.
            if !opengl_cx.make_current_with_surface(self.egl_surface) {
                return false;
            }
            let pix_width = cal_size.x.max(1.0) as i32;
            let pix_height = cal_size.y.max(1.0) as i32;
            self.wl_egl_surface.resize(pix_width, pix_height, 0, 0);
            // Cache only a resize that was actually issued. A failed bind is
            // retried on the next paint rather than leaving stale buffers.
            self.cal_size = cal_size;
        }
        true
    }

    /// Promises the compositor that the whole window rectangle is solid, so it can skip
    /// blending this surface and cull everything the window covers, and can hand a
    /// fullscreen buffer straight to the display controller instead of compositing it.
    ///
    /// The buffer is ARGB8888 — every EGL config Makepad accepts asks for 8 alpha bits —
    /// so without this promise the compositor has no way to learn that the alpha channel
    /// is uniformly opaque short of reading every pixel, and must assume it is not.
    ///
    /// `opaque` must be false for a window that actually wants translucency, or the
    /// compositor will happily leave whatever was behind it on screen. The region covers
    /// the base surface only: the shadow subsurfaces are genuinely translucent, and are
    /// separate surfaces that keep their own (empty) opaque regions.
    pub fn sync_opaque_region(
        &mut self,
        compositor: &wl_compositor::WlCompositor,
        qhandle: &QueueHandle<WaylandState>,
        opaque: bool,
    ) {
        let width = (self.window_geom.inner_size.x as i32).max(1);
        let height = (self.window_geom.inner_size.y as i32).max(1);
        if self.opaque_region_state == Some((width, height, opaque)) {
            return;
        }
        self.opaque_region_state = Some((width, height, opaque));
        if opaque {
            let region = compositor.create_region(qhandle, ());
            region.add(0, 0, width, height);
            self.base_surface.set_opaque_region(Some(&region));
            region.destroy();
        } else {
            self.base_surface.set_opaque_region(None);
        }
    }

    /// Whether the shadow gutter is currently mapped, and therefore carrying this window's
    /// resize grabs. While it is, the interior bands are redundant: they would only compete
    /// with the app's own widgets for the pointer, which is what let the close button
    /// swallow the top-right corner. A tiled window has no gutter and falls back to them.
    pub fn csd_shadow_gutter_active(&self) -> bool {
        self.csd_shadow
            .as_ref()
            .is_some_and(|shadow| shadow.is_visible())
    }

    /// The resize edge and cursor for a pointer that has entered one of this window's
    /// shadow surfaces, or `None` if `surface` is not part of this window's shadow.
    /// Edges the compositor has declared unavailable — a tiled window's shared borders —
    /// are narrowed to the components that remain resizable, so a half-tiled window
    /// keeps the corner grabs on its free axis.
    pub fn csd_shadow_resize_for_surface(
        &self,
        surface_id: &wayland_client::backend::ObjectId,
    ) -> Option<(xdg_toplevel::ResizeEdge, wp_cursor_shape_device_v1::Shape)> {
        let kind = self
            .csd_shadow
            .as_ref()?
            .piece_kind_for_surface(surface_id)?;
        let edge = crate::wayland::wayland_state::available_resize_edge(
            csd_shadow_resize_edge(kind),
            self.unavailable_resize_edges,
        )?;
        Some((edge, crate::wayland::wayland_state::resize_edge_cursor(edge)))
    }

    pub fn csd_shadow_needs_update(&self) -> bool {
        let width = (self.window_geom.inner_size.x as i32).max(1);
        let height = (self.window_geom.inner_size.y as i32).max(1);
        let visible = should_show_csd_shadow(
            self.uses_client_side_decorations,
            self.is_maximized,
            self.is_fullscreen,
            self.is_tiled,
        );
        self.csd_shadow
            .as_ref()
            .is_some_and(|shadow| shadow.needs_update(width, height, visible, self.is_active))
    }

    pub(crate) fn ensure_csd_shadow(
        &mut self,
        compositor: &wl_compositor::WlCompositor,
        subcompositor: Option<&wl_subcompositor::WlSubcompositor>,
        shm: Option<&wl_shm::WlShm>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        qhandle: &QueueHandle<WaylandState>,
    ) {
        if self.csd_shadow.is_none() {
            self.csd_shadow = WaylandCsdShadow::new(
                compositor,
                subcompositor,
                shm,
                viewporter,
                &self.base_surface,
                qhandle,
            );
        }
    }

    pub fn prepare_csd_shadow(&mut self) {
        let width = (self.window_geom.inner_size.x as i32).max(1);
        let height = (self.window_geom.inner_size.y as i32).max(1);
        let visible = should_show_csd_shadow(
            self.uses_client_side_decorations,
            self.is_maximized,
            self.is_fullscreen,
            self.is_tiled,
        );
        if let Some(shadow) = self.csd_shadow.as_mut() {
            if shadow.update(width, height, visible, self.is_active) {
                self.xdg_surface
                    .set_window_geometry(0, 0, width, height);
            }
        }
    }

    pub fn close_window(&mut self) {
        // Destroy in protocol order: role-specific objects first, base
        // surface last.
        if let Some(decoration) = self.decoration.take() {
            decoration.destroy();
        }
        if let Some(shadow) = self.csd_shadow.take() {
            shadow.destroy();
        }
        self.toplevel.destroy();
        self.xdg_surface.destroy();
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional_scale) = self.fractional_scale.take() {
            fractional_scale.destroy();
        }
        self.base_surface.destroy();
    }
}

pub(crate) struct WaylandPopupWindow {
    pub window_id: WindowId,
    pub parent_window_id: WindowId,
    pub base_surface: wl_surface::WlSurface,
    pub xdg_surface: xdg_surface::XdgSurface,
    pub xdg_popup: xdg_popup::XdgPopup,
    pub viewport: Option<wp_viewport::WpViewport>,
    pub fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    pub wl_egl_surface: Option<WlEglSurface>,
    pub egl_surface: EGLSurface,
    pub egl_display: egl_sys::EGLDisplay,
    egl_destroy_surface_fn:
        unsafe extern "C" fn(egl_sys::EGLDisplay, EGLSurface) -> egl_sys::EGLBoolean,
    pub window_geom: WindowGeom,
    pub configured: bool,
    pub cal_size: Vec2d,
}

impl WaylandPopupWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window_id: WindowId,
        parent_window_id: WindowId,
        compositer: &wl_compositor::WlCompositor,
        wm_base: &xdg_wm_base::XdgWmBase,
        parent_xdg_surface: &xdg_surface::XdgSurface,
        _seat: Option<&wayland_client::protocol::wl_seat::WlSeat>,
        _pointer_serial: Option<u32>,
        _keyboard_serial: Option<u32>,
        scale_manager: Option<&wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        qhandle: &QueueHandle<WaylandState>,
        opengl_cx: &OpenglCx,
        size: Vec2d,
        position: Vec2d,
        _grab_keyboard: bool,
    ) -> WaylandPopupWindow {
        assert_eq!(opengl_cx.egl_platform, egl_sys::EGL_PLATFORM_WAYLAND_KHR);

        let base_surface = compositer.create_surface(qhandle, ());
        let fractional_scale = scale_manager
            .map(|manager| manager.get_fractional_scale(&base_surface, qhandle, window_id));
        let viewport = viewporter.map(|vp| vp.get_viewport(&base_surface, qhandle, ()));

        let xdg_surface = wm_base.get_xdg_surface(&base_surface, qhandle, window_id);
        let positioner = wm_base.create_positioner(qhandle, ());
        positioner.set_size(size.x.max(1.0) as i32, size.y.max(1.0) as i32);
        positioner.set_anchor_rect(position.x as i32, position.y as i32, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        positioner.set_constraint_adjustment(
            xdg_positioner::ConstraintAdjustment::FlipX
                | xdg_positioner::ConstraintAdjustment::FlipY
                | xdg_positioner::ConstraintAdjustment::SlideX
                | xdg_positioner::ConstraintAdjustment::SlideY,
        );

        let xdg_popup =
            xdg_surface.get_popup(Some(parent_xdg_surface), &positioner, qhandle, window_id);
        // Do NOT grab the popup. Without a grab the compositor will not
        // auto-dismiss the popup on outside clicks, giving the app full
        // control over popup lifetime via explicit close.
        base_surface.commit();
        positioner.destroy();

        let popup_w = size.x.max(1.0) as i32;
        let popup_h = size.y.max(1.0) as i32;
        let mut wl_egl_surface = WlEglSurface::new(base_surface.id(), popup_w, popup_h).unwrap();
        // Same fallback as `WaylandWindow::new`: a popup extent the driver cannot back should
        // cost the popup its requested size, not the process.
        let mut egl_surface = create_egl_window_surface(opengl_cx, &wl_egl_surface);
        if egl_surface.is_null() {
            crate::error!(
                "eglCreateWindowSurface failed for a popup at {popup_w}x{popup_h}; retrying at the default size"
            );
            wl_egl_surface = WlEglSurface::new(
                base_surface.id(),
                DEFAULT_WINDOW_SIZE.x as i32,
                DEFAULT_WINDOW_SIZE.y as i32,
            )
            .expect("wl_egl_window_create failed at the fallback size too");
            egl_surface = create_egl_window_surface(opengl_cx, &wl_egl_surface);
        }
        assert!(
            !egl_surface.is_null(),
            "eglCreateWindowSurface failed at the fallback size too"
        );

        let geom = WindowGeom {
            xr_is_presenting: false,
            can_fullscreen: false,
            is_topmost: false,
            is_fullscreen: false,
            inner_size: size,
            outer_size: size,
            dpi_factor: 1.0,
            position,
            ..Default::default()
        };

        Self {
            window_id,
            parent_window_id,
            base_surface,
            xdg_surface,
            xdg_popup,
            viewport,
            fractional_scale,
            wl_egl_surface: Some(wl_egl_surface),
            egl_surface,
            egl_display: opengl_cx.egl_display,
            egl_destroy_surface_fn: opengl_cx.libegl.eglDestroySurface.unwrap(),
            window_geom: geom,
            configured: false,
            cal_size: Vec2d::default(),
        }
    }

    pub fn prepare_buffer_size(&mut self, opengl_cx: &OpenglCx) -> bool {
        let cal_size = Vec2d {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor,
        };
        if self.cal_size != cal_size {
            if !opengl_cx.make_current_with_surface(self.egl_surface) {
                return false;
            }
            if let Some(ref wl_egl_surface) = self.wl_egl_surface {
                wl_egl_surface.resize(cal_size.x.max(1.0) as i32, cal_size.y.max(1.0) as i32, 0, 0);
            }
            self.cal_size = cal_size;
        }
        true
    }

    pub fn close_window(&mut self) {
        // Destroy EGL surface first — it holds a reference to the wl_egl_surface.
        if !self.egl_surface.is_null() {
            unsafe {
                (self.egl_destroy_surface_fn)(self.egl_display, self.egl_surface);
            }
            self.egl_surface = std::ptr::null_mut();
        }
        // Drop wl_egl_surface before Wayland objects — wl_egl_window_destroy
        // accesses the underlying wl_surface.
        self.wl_egl_surface.take();
        // Destroy Wayland objects in protocol order: role-specific first,
        // then the base surface last.
        self.xdg_popup.destroy();
        self.xdg_surface.destroy();
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional_scale) = self.fractional_scale.take() {
            fractional_scale.destroy();
        }
        self.base_surface.destroy();
    }
}

impl Drop for WaylandWindow {
    fn drop(&mut self) {
        self.close_window();
    }
}

impl Drop for WaylandPopupWindow {
    fn drop(&mut self) {
        self.close_window();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixel: u32) -> u8 {
        (pixel >> 24) as u8
    }

    #[test]
    fn decoration_initial_state_prefers_server_and_falls_back_without_protocol() {
        assert!(!initially_uses_client_side_decorations(
            true,
            WaylandDecorationPreference::ServerSide
        ));
        assert!(initially_uses_client_side_decorations(
            false,
            WaylandDecorationPreference::ServerSide
        ));
        assert!(initially_uses_client_side_decorations(
            true,
            WaylandDecorationPreference::ClientSide
        ));
    }

    #[test]
    fn shadow_layout_tiles_the_gutter_without_gaps_or_overlap() {
        let expected = [
            (-25, -25, 41, 41),
            (16, -25, 608, 25),
            (624, -25, 41, 41),
            (-25, 16, 25, 448),
            (640, 16, 25, 448),
            (-25, 464, 41, 41),
            (16, 480, 608, 25),
            (624, 464, 41, 41),
        ];
        for (kind, expected) in CSD_SHADOW_PIECES.into_iter().zip(expected) {
            let rect = csd_shadow_rect(kind, 640, 480);
            assert_eq!((rect.x, rect.y, rect.width, rect.height), expected);
        }

        // Every pixel of the ring around the window is covered exactly once. The pieces
        // are translucent, so an overlap would double-blend into a visible seam.
        // The smallest size `csd_shadow_visible_at_size` admits is checked too, since
        // that is where the corner tiles come closest to colliding.
        let smallest = 2 * CSD_SHADOW_CORNER_INSET + 1;
        for (width, height) in [(640, 480), (smallest, smallest), (smallest, 480)] {
            let rects: Vec<_> = CSD_SHADOW_PIECES
                .into_iter()
                .map(|kind| csd_shadow_rect(kind, width, height))
                .collect();
            for rect in &rects {
                assert!(
                    rect.width > 0 && rect.height > 0,
                    "empty destination at {width}x{height}"
                );
            }
            for y in -CSD_SHADOW_MARGIN..height + CSD_SHADOW_MARGIN {
                for x in -CSD_SHADOW_MARGIN..width + CSD_SHADOW_MARGIN {
                    let covers = rects
                        .iter()
                        .filter(|rect| {
                            x >= rect.x
                                && x < rect.x + rect.width
                                && y >= rect.y
                                && y < rect.y + rect.height
                        })
                        .count();
                    let inside_window = (0..width).contains(&x) && (0..height).contains(&y);
                    // The corner tiles reach `CSD_SHADOW_CORNER_INSET` px into the window,
                    // where the parent surface covers them; everywhere else in the ring is
                    // covered exactly once and nothing spills past the margin.
                    if inside_window {
                        assert!(
                            covers <= 1,
                            "overlap inside the {width}x{height} window at ({x}, {y})"
                        );
                    } else {
                        assert_eq!(
                            covers, 1,
                            "gutter of {width}x{height} not covered once at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn shadow_raster_is_monotonic_symmetric_and_seam_free() {
        for i in 1..CSD_SHADOW_MARGIN {
            let previous = alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, i - 1, true));
            let current = alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, i, true));
            assert!(current >= previous);
        }
        for i in 0..CSD_SHADOW_MARGIN {
            assert_eq!(
                alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, i, true)),
                alpha(csd_shadow_pixel(
                    CsdShadowPieceKind::Bottom,
                    0,
                    CSD_SHADOW_MARGIN - 1 - i,
                    true,
                ))
            );
        }
        // The outermost row of the margin has faded to nothing, so the shadow does not
        // end in a visible step.
        assert_eq!(
            alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, 0, true)),
            0
        );

        // A square corner is where two half-covered edges meet, so it is lighter than a
        // straight edge at the same distance but nowhere near the washed-out 14/255 that
        // sampling a 15 px-rounded window's shadow here would give.
        let corner = alpha(csd_shadow_pixel(CsdShadowPieceKind::TopLeft, 24, 24, true));
        let straight = alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, 24, true));
        assert_eq!((corner, straight), (44, 55));

        let inner = csd_shadow_pixel(CsdShadowPieceKind::Top, 0, 24, true);
        assert_eq!(inner & 0x00ff_ffff, 0, "must stay premultiplied black");
        assert_eq!(alpha(inner), straight);
    }

    #[test]
    fn shadow_corner_tiles_join_the_straight_edges_exactly() {
        let last = CSD_SHADOW_CORNER_SIZE - 1;
        for active in [false, true] {
            // CSD_SHADOW_CORNER_INSET is chosen so the corner's second axis has reached
            // full coverage by the time the strips take over: the join is not merely
            // close, it is bit-identical, so no seam can appear at any scale.
            for y in 0..CSD_SHADOW_MARGIN {
                assert_eq!(
                    csd_shadow_pixel(CsdShadowPieceKind::TopLeft, last, y, active),
                    csd_shadow_pixel(CsdShadowPieceKind::Top, 0, y, active),
                    "top join differs at y={y}"
                );
            }
            for x in 0..CSD_SHADOW_MARGIN {
                assert_eq!(
                    csd_shadow_pixel(CsdShadowPieceKind::TopLeft, x, last, active),
                    csd_shadow_pixel(CsdShadowPieceKind::Left, x, 0, active),
                    "left join differs at x={x}"
                );
            }

            for y in 0..CSD_SHADOW_CORNER_SIZE {
                for x in 0..CSD_SHADOW_CORNER_SIZE {
                    let mirror_x = last - x;
                    let mirror_y = last - y;
                    assert_eq!(
                        csd_shadow_pixel(CsdShadowPieceKind::TopLeft, x, y, active),
                        csd_shadow_pixel(CsdShadowPieceKind::TopRight, mirror_x, y, active)
                    );
                    assert_eq!(
                        csd_shadow_pixel(CsdShadowPieceKind::TopLeft, x, y, active),
                        csd_shadow_pixel(CsdShadowPieceKind::BottomLeft, x, mirror_y, active)
                    );
                    assert_eq!(
                        csd_shadow_pixel(CsdShadowPieceKind::TopLeft, x, y, active),
                        csd_shadow_pixel(
                            CsdShadowPieceKind::BottomRight,
                            mirror_x,
                            mirror_y,
                            active,
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn shadow_grab_rects_reach_exactly_the_libadwaita_input_region() {
        // libadwaita grows its toplevel input region by 12 px on every side. The union of
        // the eight grab rects must be the same ring, with each piece owning the side it
        // resizes and none of them claiming the outer half of the gutter.
        let (width, height) = (640, 480);
        for kind in CSD_SHADOW_PIECES {
            let rect = csd_shadow_rect(kind, width, height);
            let grab = csd_shadow_grab_rect(kind);
            // Clip the grab rect to the piece the way the compositor does, then place it
            // in window coordinates.
            let x0 = rect.x + grab.x.max(0);
            let y0 = rect.y + grab.y.max(0);
            let x1 = rect.x + (grab.x + grab.width).min(rect.width);
            let y1 = rect.y + (grab.y + grab.height).min(rect.height);
            assert!(x0 < x1 && y0 < y1, "{kind:?} has an empty grab region");
            assert!(
                x0 >= -CSD_SHADOW_GRAB
                    && y0 >= -CSD_SHADOW_GRAB
                    && x1 <= width + CSD_SHADOW_GRAB
                    && y1 <= height + CSD_SHADOW_GRAB,
                "{kind:?} grabs outside the 12 px halo: ({x0},{y0})..({x1},{y1})"
            );
        }

        // The top-right corner is grabbable strictly outside the window, which is what
        // keeps the close button from swallowing it.
        let rect = csd_shadow_rect(CsdShadowPieceKind::TopRight, width, height);
        let grab = csd_shadow_grab_rect(CsdShadowPieceKind::TopRight);
        assert!(rect.x + grab.x + grab.width > width);
        assert!(rect.y + grab.y < 0);
    }

    #[test]
    fn shadow_pieces_map_to_the_edge_they_sit_on() {
        use xdg_toplevel::ResizeEdge;
        for (kind, edge) in [
            (CsdShadowPieceKind::TopLeft, ResizeEdge::TopLeft),
            (CsdShadowPieceKind::Top, ResizeEdge::Top),
            (CsdShadowPieceKind::TopRight, ResizeEdge::TopRight),
            (CsdShadowPieceKind::Left, ResizeEdge::Left),
            (CsdShadowPieceKind::Right, ResizeEdge::Right),
            (CsdShadowPieceKind::BottomLeft, ResizeEdge::BottomLeft),
            (CsdShadowPieceKind::Bottom, ResizeEdge::Bottom),
            (CsdShadowPieceKind::BottomRight, ResizeEdge::BottomRight),
        ] {
            assert_eq!(csd_shadow_resize_edge(kind), edge);
        }
    }

    #[test]
    fn shadow_alpha_matches_the_installed_libadwaita_profile() {
        let active = [54, 39, 34, 28, 24, 20, 17, 14, 12, 10, 8, 6, 5, 4, 3, 2, 1, 1, 1, 0];
        let inactive = [28, 15, 14, 12, 11, 9, 8, 6, 5, 3, 2, 1, 1, 1, 0];
        for (distance, expected) in active.into_iter().enumerate() {
            let actual = alpha(csd_shadow_pixel(
                CsdShadowPieceKind::Top,
                0,
                CSD_SHADOW_MARGIN - 1 - distance as i32,
                true,
            ));
            assert!(actual.abs_diff(expected) <= 1);
        }
        for (distance, expected) in inactive.into_iter().enumerate() {
            let actual = alpha(csd_shadow_pixel(
                CsdShadowPieceKind::Top,
                0,
                CSD_SHADOW_MARGIN - 1 - distance as i32,
                false,
            ));
            assert!(actual.abs_diff(expected) <= 1);
        }
        assert!(
            alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, 24, true))
                > alpha(csd_shadow_pixel(CsdShadowPieceKind::Top, 0, 24, false))
        );
    }

    #[test]
    fn shadow_avoids_invalid_nine_patch_destinations_for_tiny_windows() {
        assert!(!csd_shadow_visible_at_size(true, 32, 480));
        assert!(!csd_shadow_visible_at_size(true, 640, 32));
        assert!(csd_shadow_visible_at_size(true, 33, 33));
        assert!(!csd_shadow_visible_at_size(false, 640, 480));
    }

    #[test]
    fn shadow_only_appears_on_floating_client_decorated_windows() {
        assert!(should_show_csd_shadow(true, false, false, false));
        assert!(!should_show_csd_shadow(false, false, false, false));
        assert!(!should_show_csd_shadow(true, true, false, false));
        assert!(!should_show_csd_shadow(true, false, true, false));
        assert!(!should_show_csd_shadow(true, false, false, true));
    }
}
