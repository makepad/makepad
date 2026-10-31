//! Native CSD shadow tiles from opengl_wayland.rs:59-575. No EGL or Cx dependencies.
use super::super::io::memory;
use super::*;
use std::os::fd::FromRawFd;
use std::sync::Once;
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
    CsdShadowLayer {
        sigma: 7.0,
        spread: 5.0,
        alpha: 0.15,
    },
    CsdShadowLayer {
        sigma: 2.5,
        spread: 2.0,
        alpha: 0.10,
    },
    CsdShadowLayer {
        sigma: 0.0,
        spread: 1.0,
        alpha: 0.05,
    },
];

/// libadwaita 1.9 `window.csd:backdrop`. Its first term is `0 0 14px 5px transparent`,
/// which exists only to keep the shadow's extent identical to the focused profile, so
/// losing focus never changes any geometry. Being transparent it is omitted here.
const CSD_SHADOW_INACTIVE_LAYERS: &[CsdShadowLayer] = &[
    CsdShadowLayer {
        sigma: 5.0,
        spread: 5.0,
        alpha: 0.08,
    },
    CsdShadowLayer {
        sigma: 0.0,
        spread: 1.0,
        alpha: 0.05,
    },
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
    let rect = |x, y, width, height| CsdShadowRect {
        x,
        y,
        width,
        height,
    };
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

pub(super) struct WaylandCsdShadow {
    pieces: Vec<CsdShadowPiece>,
    state: Option<(i32, i32, bool, bool)>,
}

fn csd_shadow_visible_at_size(visible: bool, width: i32, height: i32) -> bool {
    visible && width > 2 * CSD_SHADOW_CORNER_INSET && height > 2 * CSD_SHADOW_CORNER_INSET
}

impl WaylandCsdShadow {
    pub(super) fn new(
        compositor: &wl_compositor::WlCompositor,
        subcompositor: Option<&wl_subcompositor::WlSubcompositor>,
        shm: Option<&wl_shm::WlShm>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        parent: &wl_surface::WlSurface,
        qhandle: &QueueHandle<State>,
    ) -> Option<Self> {
        let (Some(subcompositor), Some(shm), Some(viewporter)) = (subcompositor, shm, viewporter)
        else {
            CSD_SHADOW_UNAVAILABLE_LOGGED.call_once(|| {
                eprintln!(
                    "Wayland client-side shadow unavailable: wl_subcompositor, wl_shm, and \
                     wp_viewporter are required; continuing with client-side window controls"
                );
            });
            return None;
        };
        let Some(buffers) = Self::create_buffers(shm, qhandle) else {
            CSD_SHADOW_UNAVAILABLE_LOGGED.call_once(|| {
                eprintln!(
                    "Wayland client-side shadow unavailable: could not allocate shared-memory \
                     buffers; continuing with client-side window controls"
                );
            });
            return None;
        };
        let mut pieces = Vec::with_capacity(CSD_SHADOW_PIECES.len());
        for (kind, (active_buffer, inactive_buffer)) in CSD_SHADOW_PIECES.into_iter().zip(buffers) {
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
        qhandle: &QueueHandle<State>,
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
        let raw_fd = unsafe { memory::memfd_create(name.as_ptr(), memory::MFD_CLOEXEC) };
        if raw_fd < 0 {
            return None;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_fd) };
        if unsafe { memory::ftruncate(fd.as_raw_fd(), total_bytes as i64) } != 0 {
            return None;
        }
        let map = unsafe {
            memory::mmap(
                std::ptr::null_mut(),
                total_bytes,
                memory::PROT_READ | memory::PROT_WRITE,
                memory::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == memory::MAP_FAILED {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts_mut(map.cast::<u8>(), total_bytes) };
        for (style_index, active) in [true, false].into_iter().enumerate() {
            for &(kind, width, height, offset) in &layouts {
                let piece_offset = style_index * style_bytes + offset;
                for y in 0..height {
                    for x in 0..width {
                        let pixel_offset = piece_offset + ((y * width + x) * 4) as usize;
                        bytes[pixel_offset..pixel_offset + 4]
                            // wl_shm's ARGB8888 is defined as little endian regardless of
                            // the host, so the byte order cannot follow the CPU's.
                            .copy_from_slice(&csd_shadow_pixel(kind, x, y, active).to_le_bytes());
                    }
                }
            }
        }
        unsafe {
            memory::munmap(map, total_bytes);
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

    pub(super) fn update(&mut self, width: i32, height: i32, visible: bool, active: bool) -> bool {
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

    pub(super) fn destroy(self) {
        for piece in self.pieces {
            piece.viewport.destroy();
            piece.subsurface.destroy();
            piece.surface.destroy();
            piece.active_buffer.destroy();
            piece.inactive_buffer.destroy();
        }
    }
}

impl WaylandCsdShadow {
    pub(super) fn edge(&self, surface: &wl_surface::WlSurface) -> Option<xdg_toplevel::ResizeEdge> {
        if !self.is_visible() {
            return None;
        }
        self.piece_kind_for_surface(&surface.id())
            .map(csd_shadow_resize_edge)
    }
}

// A small file chip for a data-device drag. The compositor positions the icon at the
// pointer; its empty input region never steals the drop target's enter/motion events.
pub(super) struct DragIcon {
    pub surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
}
impl DragIcon {
    pub fn new(
        compositor: &wl_compositor::WlCompositor,
        shm: &wl_shm::WlShm,
        q: &QueueHandle<State>,
    ) -> Option<Self> {
        let width = 48;
        let height = 36;
        let size = width * height * 4;
        let fd = unsafe { memory::memfd_create(c"makepad-drag".as_ptr(), memory::MFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        if unsafe { memory::ftruncate(fd.as_raw_fd(), size as i64) } != 0 {
            return None;
        }
        let map = unsafe {
            memory::mmap(
                std::ptr::null_mut(),
                size as usize,
                memory::PROT_READ | memory::PROT_WRITE,
                memory::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == memory::MAP_FAILED {
            return None;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(map.cast::<u32>(), (width * height) as usize) };
        for y in 0..height {
            for x in 0..width {
                bytes[(y * width + x) as usize] = if (7..37).contains(&x) && (4..32).contains(&y) {
                    if x == 7 || x == 36 || y == 4 || y == 31 {
                        0xff406090
                    } else if y % 5 == 0 && y > 12 && (12..31).contains(&x) {
                        0xff90a0b0
                    } else {
                        0xfff4f6fa
                    }
                } else {
                    0
                };
            }
        }
        unsafe {
            memory::munmap(map, size as usize);
        }
        let pool = shm.create_pool(fd.as_fd(), size, q, ());
        let buffer =
            pool.create_buffer(0, width, height, width * 4, wl_shm::Format::Argb8888, q, ());
        pool.destroy();
        let surface = compositor.create_surface(q, ());
        let region = compositor.create_region(q, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, width, height);
        surface.commit();
        Some(Self { surface, buffer })
    }
}
impl Drop for DragIcon {
    fn drop(&mut self) {
        self.surface.destroy();
        self.buffer.destroy();
    }
}

/// xdg-toplevel-icon takes a square, premultiplied ARGB buffer. Pad a rectangular app icon
/// without scaling it; allocation and protocol integer limits are checked before marshalling.
pub(super) fn rgba_buffer(
    shm: &wl_shm::WlShm,
    q: &QueueHandle<State>,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Option<wl_buffer::WlBuffer> {
    let source_len = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || source_len != rgba.len() {
        return None;
    }
    let side = width.max(height) as usize;
    let bytes = side.checked_mul(side)?.checked_mul(4)?;
    let protocol_size = i32::try_from(bytes).ok()?;
    let raw = unsafe { memory::memfd_create(c"makepad-icon".as_ptr(), memory::MFD_CLOEXEC) };
    if raw < 0 {
        return None;
    }
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    if unsafe { memory::ftruncate(fd.as_raw_fd(), bytes as i64) } != 0 {
        return None;
    }
    let map = unsafe {
        memory::mmap(
            std::ptr::null_mut(),
            bytes,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if map == memory::MAP_FAILED {
        return None;
    }
    let dest = unsafe { std::slice::from_raw_parts_mut(map.cast::<u32>(), side * side) };
    let x0 = (side - width as usize) / 2;
    let y0 = (side - height as usize) / 2;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let pixel = &rgba[(y * width as usize + x) * 4..][..4];
            let alpha = pixel[3] as u32;
            dest[(y + y0) * side + x + x0] = alpha << 24
                | (pixel[0] as u32 * alpha / 255) << 16
                | (pixel[1] as u32 * alpha / 255) << 8
                | pixel[2] as u32 * alpha / 255;
        }
    }
    unsafe {
        memory::munmap(map, bytes);
    }
    let pool = shm.create_pool(fd.as_fd(), protocol_size, q, ());
    let buffer = pool.create_buffer(
        0,
        side as i32,
        side as i32,
        (side * 4) as i32,
        wl_shm::Format::Argb8888,
        q,
        (),
    );
    pool.destroy();
    Some(buffer)
}
