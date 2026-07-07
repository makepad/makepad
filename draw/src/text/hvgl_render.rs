//! Render Apple `hvgl` (Hierarchical Variation Font) glyph outlines via Apple's
//! `libhvf` on iOS / macOS.
//!
//! iOS/macOS system fonts (the CJK / Devanagari / Arabic / Thai super-font, SF,
//! PingFang, …) store their outlines in Apple's proprietary `hvgl` format. Every
//! glyph is a *composite part* — `ttf_parser` cannot decode them. Apple ships
//! `/usr/lib/libhvf.dylib` (public API since iOS 18.4 / macOS 15.4) which renders
//! `hvgl` outlines through a callback. We `dlopen` it at runtime so the app still
//! launches on older OSes (where these `hvgl` fonts don't exist anyway) — there we
//! simply return `None` and the caller renders a blank glyph, same as before.
//!
//! Headers: `$SDK/usr/include/hvf/{Scaler,RenderContext,Types}.h`.
//! Docs: <https://developer.apple.com/documentation/hvf>

#![cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]

use {
    super::geom::{Point, Rect},
    rustybuzz::ttf_parser::{self, OutlineBuilder},
    std::{
        ffi::c_void,
        os::raw::c_int,
        sync::OnceLock,
    },
};

// ---- libhvf C ABI (see hvf/RenderContext.h) --------------------------------

/// `enum HVFPartRenderInstruction` (RenderContext.h). C enums are `int`-sized.
#[repr(C)]
#[allow(dead_code)]
enum Instruction {
    BeginPart = 0, // param beginPart
    BeginPath = 1,
    AddPoint = 2,  // param addPoint (moveto)
    AddLine = 3,   // param addPoint (not implemented by libhvf)
    AddQuad = 4,   // param addQuad
    AddCubic = 5,  // param addCubic (not used by libhvf)
    ClosePath = 6,
    EndPath = 7,
    EndPart = 8,   // param partInfo
    Stop = 9,
}

/// `enum HVFPartRenderAction`.
const ACTION_CONTINUE: c_int = 0;
#[allow(dead_code)]
const ACTION_SKIP: c_int = 1;
#[allow(dead_code)]
const ACTION_STOP: c_int = 2;

/// `HVFXYCoord = double`, `struct HVFPoint { HVFXYCoord x, y; }`.
#[repr(C)]
#[derive(Clone, Copy)]
struct HvfPoint {
    x: f64,
    y: f64,
}

/// `union HVFPartRenderParameters` (RenderContext.h). The largest member is
/// `addCubic { HVFPoint cp1, cp2, onpt; }` = 6 doubles; `partId` is a `size_t`.
/// We read it by casting to the specific member for each instruction.
#[repr(C)]
#[derive(Clone, Copy)]
union RenderParameters {
    part_info: HvfPartInfo,
    add_point: HvfAddPoint,
    add_quad: HvfAddQuad,
    _add_cubic: HvfAddCubic,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HvfPartInfo {
    part_id: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct HvfAddPoint {
    pt: HvfPoint,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct HvfAddQuad {
    offpt: HvfPoint,
    onpt: HvfPoint,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct HvfAddCubic {
    cp1: HvfPoint,
    cp2: HvfPoint,
    onpt: HvfPoint,
}

/// `typedef HVFPartRenderAction (*HVFRenderContext)(HVFPartRenderInstruction,
///     const HVFPartRenderParameters*, void*);`
type RenderContext =
    unsafe extern "C" fn(c_int, *const RenderParameters, *mut c_void) -> c_int;

// libhvf function pointer types (Scaler.h).
type FnStorageSize = unsafe extern "C" fn() -> usize;
type FnOpen = unsafe extern "C" fn(
    hvgl: *const c_void,
    hvgl_size: usize,
    hvpm: *const c_void,
    hvpm_size: usize,
    storage: *mut c_void,
    storage_size: usize,
) -> c_int;
type FnSetRenderPart = unsafe extern "C" fn(renderer: *mut c_void, part: usize) -> c_int;
type FnRenderCurrent =
    unsafe extern "C" fn(renderer: *mut c_void, ctx: RenderContext, caller: *mut c_void) -> c_int;
type FnClose = unsafe extern "C" fn(renderer: *mut c_void) -> c_int;
type FnClearCache = unsafe extern "C" fn(renderer: *mut c_void) -> c_int;

struct Lib {
    storage_size: FnStorageSize,
    open: FnOpen,
    set_render_part: FnSetRenderPart,
    render_current: FnRenderCurrent,
    close: FnClose,
    clear_cache: FnClearCache,
}

// SAFETY: the resolved function pointers are immutable code addresses in a
// dylib that stays loaded for the process lifetime.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        unsafe extern "C" {
            fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
            fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
        }
        let handle = dlopen(b"/usr/lib/libhvf.dylib\0".as_ptr() as *const i8, 1); // RTLD_LAZY
        if handle.is_null() {
            crate::log!("[hvgl] dlopen /usr/lib/libhvf.dylib FAILED (nil handle) — hvgl fonts will tofu");
            return None;
        }
        let sym = |name: &[u8]| dlsym(handle, name.as_ptr() as *const i8);
        let storage_size = sym(b"HVF_part_renderer_storage_size\0");
        let open = sym(b"HVF_open_part_renderer\0");
        let set_render_part = sym(b"HVF_set_render_part\0");
        let render_current = sym(b"HVF_render_current_part\0");
        let close = sym(b"HVF_close_part_renderer\0");
        let clear_cache = sym(b"HVF_clear_part_cache\0");
        if storage_size.is_null()
            || open.is_null()
            || set_render_part.is_null()
            || render_current.is_null()
            || close.is_null()
            || clear_cache.is_null()
        {
            crate::log!(
                "[hvgl] libhvf missing symbols: storage_size={} open={} set_render_part={} render_current={} close={} clear_cache={}",
                !storage_size.is_null(), !open.is_null(), !set_render_part.is_null(),
                !render_current.is_null(), !close.is_null(), !clear_cache.is_null(),
            );
            return None;
        }
        Some(Lib {
            storage_size: std::mem::transmute::<*mut c_void, FnStorageSize>(storage_size),
            open: std::mem::transmute::<*mut c_void, FnOpen>(open),
            set_render_part: std::mem::transmute::<*mut c_void, FnSetRenderPart>(set_render_part),
            render_current: std::mem::transmute::<*mut c_void, FnRenderCurrent>(render_current),
            close: std::mem::transmute::<*mut c_void, FnClose>(close),
            clear_cache: std::mem::transmute::<*mut c_void, FnClearCache>(clear_cache),
        })
    })
    .as_ref()
}

/// A `libhvf` part renderer bound to a single font's `hvgl` table.
///
/// Owns an 8-byte-aligned copy of the `hvgl` bytes and the renderer's backing
/// storage (both required by `HVF_open_part_renderer` to be double-aligned), so
/// this must not be moved after `open` (it is boxed / kept behind a `RefCell` in
/// `Font`, and the internal pointers reference the heap allocations, not `self`).
pub struct HvglRenderer {
    // 8-byte aligned via u64 backing storage.
    _hvgl: Box<[u64]>,
    _storage: Box<[u64]>,
    renderer: *mut c_void,
    /// Glyphs rendered since the last cache clear (libhvf recommends clearing
    /// every dozen or so parts to bound cache growth over a 58MB table).
    since_clear: std::cell::Cell<u32>,
}

impl std::fmt::Debug for HvglRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HvglRenderer").finish_non_exhaustive()
    }
}

impl Drop for HvglRenderer {
    fn drop(&mut self) {
        if let Some(lib) = lib() {
            if !self.renderer.is_null() {
                unsafe { (lib.close)(self.renderer) };
            }
        }
    }
}

/// Copy `bytes` into an 8-byte-aligned `Box<[u64]>` (last word zero-padded).
fn aligned_copy(bytes: &[u8]) -> Box<[u64]> {
    let words = (bytes.len() + 7) / 8;
    let mut buf = vec![0u64; words].into_boxed_slice();
    // SAFETY: buf has words*8 >= bytes.len() bytes; u64 has no invalid bit patterns.
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            buf.as_mut_ptr() as *mut u8,
            bytes.len(),
        );
    }
    buf
}

impl HvglRenderer {
    /// Open a renderer for a font's raw `hvgl` table bytes. Returns `None` if
    /// `libhvf` is unavailable (iOS < 18.4 / macOS < 15.4) or open fails.
    pub fn open(hvgl_bytes: &[u8]) -> Option<HvglRenderer> {
        let lib = lib()?;
        let hvgl_len = hvgl_bytes.len();
        let hvgl = aligned_copy(hvgl_bytes);
        let storage_bytes = unsafe { (lib.storage_size)() };
        let storage_words = (storage_bytes + 7) / 8;
        let mut storage = vec![0u64; storage_words.max(1)].into_boxed_slice();

        let rc = unsafe {
            (lib.open)(
                hvgl.as_ptr() as *const c_void,
                hvgl_len,
                std::ptr::null(),
                0,
                storage.as_mut_ptr() as *mut c_void,
                storage_bytes,
            )
        };
        if rc != 0 {
            crate::log!("[hvgl] open FAILED (rc={}) — hvpm=null; hvgl glyphs will tofu", rc);
            return None;
        }
        // The storage buffer *is* the renderer handle.
        let renderer = storage.as_mut_ptr() as *mut c_void;
        Some(HvglRenderer {
            _hvgl: hvgl,
            _storage: storage,
            renderer,
            since_clear: std::cell::Cell::new(0),
        })
    }

    /// Render `glyph_id` into `builder`, returning the outline's bounding box in
    /// font units. `None` if the glyph could not be rendered.
    pub fn outline(
        &self,
        glyph_id: ttf_parser::GlyphId,
        builder: &mut dyn OutlineBuilder,
    ) -> Option<Rect<f32>> {
        let lib = lib()?;

        let set_rc = unsafe { (lib.set_render_part)(self.renderer, glyph_id.0 as usize) };
        if set_rc != 0 {
            return None;
        }

        let mut cb = CallbackData {
            builder,
            open_contour: false,
            last: Point::new(0.0, 0.0),
            min: Point::new(f32::MAX, f32::MAX),
            max: Point::new(f32::MIN, f32::MIN),
            any: false,
        };
        let rc = unsafe {
            (lib.render_current)(
                self.renderer,
                render_callback,
                &mut cb as *mut CallbackData as *mut c_void,
            )
        };
        // Close any dangling contour (libhvf normally emits ClosePath itself).
        if cb.open_contour {
            cb.builder.close();
        }

        // Periodically clear the part cache to bound growth (libhvf guidance).
        let n = self.since_clear.get() + 1;
        if n >= 16 {
            unsafe { (lib.clear_cache)(self.renderer) };
            self.since_clear.set(0);
        } else {
            self.since_clear.set(n);
        }

        if rc != 0 || !cb.any {
            return None;
        }
        Some(Rect::new(cb.min, cb.max - cb.min))
    }
}

struct CallbackData<'a> {
    builder: &'a mut dyn OutlineBuilder,
    open_contour: bool,
    last: Point<f32>,
    min: Point<f32>,
    max: Point<f32>,
    any: bool,
}

impl<'a> CallbackData<'a> {
    #[inline]
    fn track(&mut self, x: f32, y: f32) {
        self.min.x = self.min.x.min(x);
        self.min.y = self.min.y.min(y);
        self.max.x = self.max.x.max(x);
        self.max.y = self.max.y.max(y);
        self.any = true;
    }
}

/// `HVFRenderContext` callback. Translates libhvf path instructions into
/// `ttf_parser::OutlineBuilder` calls. `caller` is a `*mut CallbackData`.
unsafe extern "C" fn render_callback(
    instruction: c_int,
    params: *const RenderParameters,
    caller: *mut c_void,
) -> c_int {
    let cb = unsafe { &mut *(caller as *mut CallbackData) };
    match instruction {
        // AddPoint == moveto (start of a contour).
        x if x == Instruction::AddPoint as c_int => {
            let p = unsafe { (*params).add_point.pt };
            let (px, py) = (p.x as f32, p.y as f32);
            if cb.open_contour {
                cb.builder.close();
            }
            cb.builder.move_to(px, py);
            cb.open_contour = true;
            cb.last = Point::new(px, py);
            cb.track(px, py);
        }
        // AddQuad == quadratic bezier to onpt with control offpt.
        x if x == Instruction::AddQuad as c_int => {
            let q = unsafe { (*params).add_quad };
            let (cx, cy) = (q.offpt.x as f32, q.offpt.y as f32);
            let (nx, ny) = (q.onpt.x as f32, q.onpt.y as f32);
            cb.builder.quad_to(cx, cy, nx, ny);
            cb.last = Point::new(nx, ny);
            cb.track(cx, cy);
            cb.track(nx, ny);
        }
        x if x == Instruction::ClosePath as c_int => {
            if cb.open_contour {
                cb.builder.close();
                cb.open_contour = false;
            }
        }
        // BeginPart / BeginPath / EndPath / EndPart / Stop: nothing to emit.
        _ => {}
    }
    ACTION_CONTINUE
}
