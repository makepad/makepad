//! Cursor-theme fallback for compositors without cursor-shape-v1. ABI declarations follow
//! libs/linux/wayland-sys/src/cursor.rs; the optional system library owns its wl_buffer objects.
use super::*;
use std::ffi::{c_char, c_int, c_void};
use std::ptr;

#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(lib: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(lib: *mut c_void) -> c_int;
}
#[repr(C)]
struct Image {
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    delay: u32,
}
#[repr(C)]
struct Cursor {
    image_count: u32,
    images: *mut *mut Image,
    name: *mut c_char,
}
type Load = unsafe extern "C" fn(*const c_char, c_int, *mut c_void) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Get = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut Cursor;
type Buffer = unsafe extern "C" fn(*mut Image) -> *mut c_void;

pub(super) struct Theme {
    library: *mut c_void,
    load: Load,
    destroy: Destroy,
    get: Get,
    buffer: Buffer,
    themes: HashMap<i32, *mut c_void>,
    surface: wl_surface::WlSurface,
}
impl Theme {
    pub fn new(compositor: &wl_compositor::WlCompositor, q: &QueueHandle<State>) -> Option<Self> {
        unsafe {
            let library = dlopen(c"libwayland-cursor.so.0".as_ptr(), 1);
            if library.is_null() {
                return None;
            }
            let load = dlsym(library, c"wl_cursor_theme_load".as_ptr());
            let destroy = dlsym(library, c"wl_cursor_theme_destroy".as_ptr());
            let get = dlsym(library, c"wl_cursor_theme_get_cursor".as_ptr());
            let buffer = dlsym(library, c"wl_cursor_image_get_buffer".as_ptr());
            if [load, destroy, get, buffer].iter().any(|p| p.is_null()) {
                dlclose(library);
                return None;
            }
            Some(Self {
                library,
                load: std::mem::transmute::<*mut c_void, Load>(load),
                destroy: std::mem::transmute::<*mut c_void, Destroy>(destroy),
                get: std::mem::transmute::<*mut c_void, Get>(get),
                buffer: std::mem::transmute::<*mut c_void, Buffer>(buffer),
                themes: HashMap::new(),
                surface: compositor.create_surface(q, ()),
            })
        }
    }
    pub fn show(
        &mut self,
        conn: &Connection,
        shm: &wl_shm::WlShm,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        cursor: u32,
        dpi: f32,
    ) {
        unsafe {
            let scale = if self.surface.version() >= 3 {
                dpi.ceil().max(1.0) as i32
            } else {
                1
            };
            let base = std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|s| *s > 0)
                .unwrap_or(24);
            let size = base.saturating_mul(scale);
            let theme = *self.themes.entry(size).or_insert_with(|| {
                let name = std::env::var("XCURSOR_THEME")
                    .ok()
                    .map(|s| common::cstring(&s));
                (self.load)(
                    name.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
                    size,
                    shm.id().as_ptr().cast(),
                )
            });
            if theme.is_null() {
                eprintln!("[hal-wayland] cursor theme unavailable");
                return;
            }
            let mut shape = (self.get)(theme, common::cursor_name(cursor).as_ptr());
            if shape.is_null() {
                shape = (self.get)(theme, c"left_ptr".as_ptr());
            }
            let Some(shape) = shape
                .as_ref()
                .filter(|s| s.image_count > 0 && !s.images.is_null())
            else {
                return;
            };
            let image = *shape.images;
            let Some(info) = image.as_ref() else {
                return;
            };
            let raw = (self.buffer)(image);
            if raw.is_null() {
                return;
            }
            let Ok(id) = wayland_client::backend::ObjectId::from_ptr(
                wl_buffer::WlBuffer::interface(),
                raw.cast(),
            ) else {
                return;
            };
            let Ok(buffer) = wl_buffer::WlBuffer::from_id(conn, id) else {
                return;
            };
            if self.surface.version() >= 3 {
                self.surface.set_buffer_scale(scale);
            }
            pointer.set_cursor(
                serial,
                Some(&self.surface),
                info.hotspot_x as i32 / scale,
                info.hotspot_y as i32 / scale,
            );
            self.surface.attach(Some(&buffer), 0, 0);
            self.surface.damage(
                0,
                0,
                (info.width as i32 + scale - 1) / scale,
                (info.height as i32 + scale - 1) / scale,
            );
            self.surface.commit();
        }
    }
}
impl Drop for Theme {
    fn drop(&mut self) {
        self.surface.destroy();
        unsafe {
            for (_, theme) in self.themes.drain() {
                if !theme.is_null() {
                    (self.destroy)(theme);
                }
            }
            dlclose(self.library);
        }
    }
}
