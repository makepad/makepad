//! The plain data that crosses the HAL boundary. Every type is `#[repr(C)]` and
//! `#[relax_api(plain)]`: Relax-compiled code reads, builds and drops them itself (Relax checks
//! rustc's size, align and field offsets against its own).

use makepad_relax_api::relax_api;

/// A resource id chosen by Cx: its pool slot and the slot's generation. The id space is per kind
/// (texture, buffer, pipeline, window).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Id {
    pub index: u32,
    pub gen: u32,
}

impl Id {
    /// "No id": never a live slot.
    pub const NONE: Id = Id { index: u32::MAX, gen: u32::MAX };
    pub const fn new(index: u32, gen: u32) -> Id {
        Id { index, gen }
    }
    pub const fn is_none(&self) -> bool {
        self.index == u32::MAX && self.gen == u32::MAX
    }
}

impl Default for Id {
    fn default() -> Id {
        Id::NONE
    }
}

/// A byte range into the CmdBuf / EventBuf arena. (Inside a FramePacket, `Pass::draws` and
/// `Draw::inst_ranges` are element ranges instead; the field says so.)
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub off: u32,
    pub len: u32,
}

/// A borrowed run of memory: `len` is in BYTES. The pointer is valid for the duration of the
/// call that receives it (`hal_submit`), never longer: the HAL copies whatever it keeps before
/// that call returns. Nothing may be stored from it.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    pub ptr: *const u8,
    pub len: usize,
}

impl Default for Slice {
    fn default() -> Slice {
        Slice::EMPTY
    }
}

/// Plain data a [`Slice`] may be read as: no padding-dependent invariants, every bit pattern
/// valid. Implemented for the repr(C) structs of this module only.
///
/// # Safety
/// `Self` must be `#[repr(C)]` plain data for which any bit pattern is a valid value.
pub unsafe trait Plain: Copy {}

unsafe impl Plain for u8 {}
unsafe impl Plain for u64 {}
unsafe impl Plain for u32 {}
unsafe impl Plain for f32 {}

impl Slice {
    pub const EMPTY: Slice = Slice { ptr: std::ptr::null(), len: 0 };

    /// Borrows `b`; the result is only meaningful while `b` is.
    pub fn from_bytes(b: &[u8]) -> Slice {
        Slice { ptr: b.as_ptr(), len: b.len() }
    }

    /// Borrows the elements of `v` (len in bytes).
    pub fn of<T: Plain>(v: &[T]) -> Slice {
        Slice { ptr: v.as_ptr() as *const u8, len: std::mem::size_of_val(v) }
    }

    /// The borrowed bytes.
    ///
    /// # Safety
    /// The memory behind `ptr` must still be alive: call only inside the call that received the
    /// Slice.
    pub unsafe fn as_bytes<'a>(&self) -> &'a [u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(self.ptr, self.len)
        }
    }

    /// The borrowed elements; `None` when the pointer is misaligned or the length is not a
    /// multiple of `size_of::<T>()`.
    ///
    /// # Safety
    /// As [`Slice::as_bytes`].
    pub unsafe fn as_slice<'a, T: Plain>(&self) -> Option<&'a [T]> {
        if self.ptr.is_null() || self.len == 0 {
            return Some(&[]);
        }
        let size = std::mem::size_of::<T>();
        if self.len % size != 0 || (self.ptr as usize) % std::mem::align_of::<T>() != 0 {
            return None;
        }
        Some(std::slice::from_raw_parts(self.ptr as *const T, self.len / size))
    }
}

/// An entry of the session string table (`StrDef`); 0 = none.
pub type StrId = u32;

/// Attribution of a resource, supplied by Cx on every create: who owns it (innermost gpu_owner
/// scope), where it was created (`#[track_caller]` location), an explicit label, and what it is.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attr {
    pub owner: StrId,
    pub site: StrId,
    pub label: StrId,
    pub kind: u32,
}

/// One batch of commands per loop turn. All slices are borrowed for the `hal_submit` call
/// only. `cmds` is `[Cmd]`, `frames` is `[FramePacket]`, `arena` holds every `Range` the commands
/// and packets name (8-aligned, so typed arrays in it can be read in place).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CmdBuf {
    pub session: u32,
    pub cmds: Slice,
    pub arena: Slice,
    pub frames: Slice,
    /// `[Pass]` and `[Draw]` of every frame in `frames` (element ranges, see [`FramePacket`])
    pub passes: Slice,
    pub draws: Slice,
    /// `[Layer]` of every frame, addressed by `FramePacket::layers`.
    pub layers: Slice,
}

/// One fire-and-forget command. `a` is decoded per `tag` by the generated constructors and
/// decoders in `cmds.rs`; `id` is the object the command is about, `data` its arena payload.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cmd {
    pub tag: u16,
    pub flags: u16,
    pub id: Id,
    pub a: [u64; 4],
    pub data: Range,
}

unsafe impl Plain for Cmd {}

/// The caller's buffer the HAL writes events into during `hal_pump`. `arena` receives the
/// payloads the events' `data` ranges name.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EventBuf {
    pub session: u32,
    pub events: *mut RawEvent,
    pub cap: u32,
    pub arena: *mut u8,
    pub arena_cap: u32,
    /// written by `hal_pump`: bytes of `arena` the delivered events' `data` ranges use
    pub arena_len: u32,
}

/// One event, decoded per `tag` by the generated helpers in `events.rs`.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawEvent {
    pub tag: u16,
    pub window: Id,
    pub time_ns: u64,
    pub a: [f64; 4],
    pub b: [u64; 3],
    pub data: Range,
}

unsafe impl Plain for RawEvent {}

/// `OpenReq::flags` bits.
pub mod open_flags {
    /// Windows are realised as offscreen render targets of the window's size and a presented
    /// frame is a finished frame (`FrameDone`). A permanent mode: GPU-real headless runs and
    /// tests on a machine with a display need no window system.
    pub const OFFSCREEN_WINDOWS: u32 = 1;
    /// Real windows keep a copy of their newest frame, so a `Snapshot` of an idle window can
    /// answer (automation, remote grabs). Costs one GPU copy per presented frame.
    pub const RETAIN_WINDOWS: u32 = 2;
    /// The process is an agent- or test-driven instance: its windows show but never take the
    /// user's focus or activate the app (`MAKEPAD_FOCUS` overrides).
    pub const NO_FOCUS: u32 = 4;
    /// Windows start transparent and click-through and show with the first presented frame
    /// (a launcher's child, so the launcher's spinner is all that is seen until then).
    pub const HIDDEN_UNTIL_PRESENT: u32 = 8;
    /// The process is a window manager: it hosts other apps' windows and never looks for
    /// another makepad WM to host its own (Linux backend discovery, `wm_socket`).
    pub const WINDOW_MANAGER: u32 = 16;
}

/// `HalInfo::flags` bits.
pub mod info_flags {
    /// The host granted domain control to this session.
    pub const PRIVILEGED: u32 = 8;
    /// This session renders embedded windows for a parent.
    pub const EMBEDDED: u32 = 16;
    /// Windows are on a display: a window announces a frame (`FrameReady`) on the display
    /// clock (or the shell's timer fallback) after a `FrameRequest`. Without it (headless, offscreen
    /// windows) the app paces its own beat.
    pub const DISPLAY_CLOCK: u32 = 1;
    /// The selected graphics queue can report GPU timestamp durations in `FrameDone`.
    pub const GPU_TIMESTAMPS: u32 = 2;
    /// The desktop shell may ask the app to draw window controls (per-window Chrome events).
    pub const CLIENT_CHROME: u32 = 4;
    /// One full desktop target owns scanout; widgets compose the direct-display cursor.
    pub const DIRECT_DISPLAY: u32 = 32;
    /// The backend composes session 0's window layer over a lent window (`window_overlay`
    /// with a layer texture).
    pub const WINDOW_LAYERS: u32 = 64;
    /// Session 0 may put badges on its windows (`window_kind::BADGE`, `target::BADGES`): the
    /// backend composes a window's badge layer into every frame it presents.
    pub const WINDOW_BADGES: u32 = 128;
    /// A makepad window manager hosts the windows (the Linux WM shell): they are its tiles,
    /// which draw their own caption in the WM's style; the WM's messages arrive as `AppMessage`.
    pub const WM_HOSTED: u32 = 256;
    /// A head-mounted display runs the session: `window_kind::XR` windows show in it, and the
    /// host window is a panel the person points at.
    pub const XR: u32 = 512;
    /// The backend renders multiview passes (`Pass::views` 2): an app draws a stereo view in one
    /// pass instead of one per eye.
    pub const MULTIVIEW: u32 = 1024;
    /// With `XR`: the XR window has no display of its own (a remote headset: its frames go to an
    /// encoder the app drives). XR passes render into the pass's own colour and depth textures,
    /// both eyes side by side, and the app hands the result on; the XR window only paces them
    /// (its `XrFrame`s and `FrameReady`s), it is never presented.
    pub const XR_OFFSCREEN: u32 = 2048;

    #[test]
    fn the_flags_are_distinct_bits() {
        let all = [PRIVILEGED, EMBEDDED, DISPLAY_CLOCK, GPU_TIMESTAMPS, CLIENT_CHROME, DIRECT_DISPLAY, WINDOW_LAYERS, WINDOW_BADGES, WM_HOSTED, XR, MULTIVIEW, XR_OFFSCREEN];
        for (i, a) in all.iter().enumerate() {
            assert!(a.is_power_of_two());
            assert!(all[i + 1..].iter().all(|b| a & b == 0), "flag {a} shares a bit");
        }
    }
}

/// `Cursor::cursor` values, in the order of Cx's `MouseCursor`.
pub mod cursor {
    pub const HIDDEN: u32 = 0;
    pub const DEFAULT: u32 = 1;
    pub const CROSSHAIR: u32 = 2;
    pub const HAND: u32 = 3;
    pub const ARROW: u32 = 4;
    pub const MOVE: u32 = 5;
    pub const TEXT: u32 = 6;
    pub const WAIT: u32 = 7;
    pub const HELP: u32 = 8;
    pub const NOT_ALLOWED: u32 = 9;
    pub const GRAB: u32 = 10;
    pub const GRABBING: u32 = 11;
    pub const N_RESIZE: u32 = 12;
    pub const NE_RESIZE: u32 = 13;
    pub const E_RESIZE: u32 = 14;
    pub const SE_RESIZE: u32 = 15;
    pub const S_RESIZE: u32 = 16;
    pub const SW_RESIZE: u32 = 17;
    pub const W_RESIZE: u32 = 18;
    pub const NW_RESIZE: u32 = 19;
    pub const NS_RESIZE: u32 = 20;
    pub const NESW_RESIZE: u32 = 21;
    pub const EW_RESIZE: u32 = 22;
    pub const NWSE_RESIZE: u32 = 23;
    pub const COL_RESIZE: u32 = 24;
    pub const ROW_RESIZE: u32 = 25;
}

/// `PointerLock::mode` values.
pub mod pointer_lock {
    pub const UNLOCK: u32 = 0;
    pub const LOCK: u32 = 1;
    pub const PIN: u32 = 2;
    pub const UNPIN: u32 = 3;
    pub const REPIN: u32 = 4;
}

/// `MenuItem::kind` bits: the node kind in the low byte, then flags.
pub mod menu_kind {
    pub const SUB: u32 = 0;
    pub const ITEM: u32 = 1;
    pub const LINE: u32 = 2;
    pub const SHIFT: u32 = 1 << 8;
    pub const DISABLED: u32 = 1 << 9;
    /// the application's Quit item: `terminate:`, not a command
    pub const QUIT: u32 = 1 << 10;
    /// the item shows a check mark state (`CHECKED` says which)
    pub const CHECKABLE: u32 = 1 << 11;
    pub const CHECKED: u32 = 1 << 12;
    /// the application's About item: the system's About panel (name, version, icon from the
    /// bundle), not a command
    pub const ABOUT: u32 = 1 << 13;
    /// the item is in the Dock icon's menu too (macOS; the system lists the app's windows there)
    pub const DOCK: u32 = 1 << 14;
}

/// `FileDialog::kind`: the dialog in the low byte, bit 8: choose several.
pub mod dialog_kind {
    pub const SELECT_FILE: u32 = 0;
    pub const SAVE_FILE: u32 = 1;
    pub const SELECT_FOLDER: u32 = 2;
    pub const SAVE_FOLDER: u32 = 3;
    pub const MULTIPLE: u32 = 1 << 8;
}

/// What `hal_open` is asked for.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenReq {
    /// [`backend`] id
    pub backend: u32,
    /// Identity of the app: opening a new session with the same non-zero key closes the old one.
    pub app_key: u64,
    pub flags: u32,
    pub dpi_hint: f32,
    /// The domain that started this one (0 = none): the session's windows are then embedded
    /// windows lent to the parent. Only valid for a session a privileged parent asked for with
    /// `DomainStart` (the HAL hands the host the `app_key` to open it with); any other claim is
    /// refused. `Hal::open_domain` (host code) may name a parent directly.
    pub parent: u32,
}

impl OpenReq {
    /// Native apps and their host choose the same device and window mode.
    pub fn from_env(app_key: u64, window_flags: u32, dpi_hint: f32) -> Self {
        let hidden = std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some();
        let (backend, flags) = match std::env::var("MAKEPAD_HAL_BACKEND").as_deref() {
            Ok("headless") => (backend_id::HEADLESS, 0),
            Ok("offscreen") if cfg!(target_vendor = "apple") => (backend_id::METAL, open_flags::OFFSCREEN_WINDOWS),
            Ok("offscreen") if cfg!(target_os = "linux") => (backend_id::AUTO, open_flags::OFFSCREEN_WINDOWS),
            Ok("offscreen") if cfg!(windows) => (backend_id::D3D11, open_flags::OFFSCREEN_WINDOWS),
            // An explicit native backend keeps OS windows for accessibility/automation.
            // MAKEPAD_HIDE_WINDOWS hides the Win32 HWNDs; "offscreen" creates none.
            Ok("d3d11") if cfg!(windows) => (backend_id::D3D11, window_flags),
            Ok("offscreen") => (backend_id::HEADLESS, 0),
            Ok("metal") => (backend_id::METAL, window_flags),
            Ok("gl") if cfg!(target_os = "linux") => (backend_id::GL, window_flags | if hidden { open_flags::OFFSCREEN_WINDOWS } else { 0 }),
            Ok("vulkan") if cfg!(target_os = "linux") && !hidden => (backend_id::VULKAN, window_flags),
            Ok("vulkan") => (backend_id::VULKAN, window_flags | open_flags::OFFSCREEN_WINDOWS),
            _ if cfg!(target_vendor = "apple") && hidden => (backend_id::METAL, open_flags::OFFSCREEN_WINDOWS),
            _ if cfg!(target_vendor = "apple") => (backend_id::METAL, window_flags),
            _ if cfg!(target_os = "linux") => (backend_id::AUTO, if hidden { window_flags | open_flags::OFFSCREEN_WINDOWS } else { window_flags }),
            _ if cfg!(target_os = "android") => (backend_id::VULKAN, window_flags),
            _ if cfg!(windows) => (backend_id::D3D11, if hidden { window_flags | open_flags::OFFSCREEN_WINDOWS } else { window_flags }),
            _ => (backend_id::HEADLESS, 0),
        };
        Self { backend, flags, app_key, dpi_hint, parent: 0 }
    }
}

/// What `hal_open` answers: the session id (0 = refused) and the capabilities that select the
/// shader codegen.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HalInfo {
    pub session: u32,
    pub backend: u32,
    pub dialect: u32,
    pub flags: u32,
    /// bit per [`format`]
    pub formats: u64,
    /// The sample counts a pass may ask for ([`Pass::samples`]): bit n set = 2^n samples per
    /// pixel (1: single sampling only, no MSAA).
    pub msaa: u32,
    pub dpi: f32,
    pub max_texture: u32,
    pub max_window: [u32; 2],
    /// the largest single GPU buffer the device allocates, in bytes; 0 = unknown
    pub max_buffer: u64,
    /// the memory the device works in, in bytes; 0 = unknown
    pub working_set: u64,
    /// The GPU's PCI vendor id (0 = unknown; Apple GPUs report 0x106b).
    pub vendor_id: u32,
    /// [`device_kind`]: integrated, discrete, virtual or a software rasterizer.
    pub device_kind: u32,
    /// The device's name as the driver reports it, UTF-8, zero-padded (a fixed
    /// array: this struct crosses the HAL pipe as plain data). An app picks its
    /// graphics quality from it ([`HalInfo::device_name`]).
    pub device_name: [u64; 8],
}

/// What kind of GPU a backend runs on (`HalInfo::device_kind`).
pub mod device_kind {
    pub const UNKNOWN: u32 = 0;
    pub const INTEGRATED: u32 = 1;
    pub const DISCRETE: u32 = 2;
    pub const VIRTUAL: u32 = 3;
    pub const SOFTWARE: u32 = 4;
}

impl HalInfo {
    /// The device name (empty when the backend does not know it).
    pub fn device_name(&self) -> String {
        let bytes: Vec<u8> = self.device_name.iter().flat_map(|w| w.to_le_bytes()).take_while(|b| *b != 0).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Store `name`, cut at a character boundary to fit.
    pub fn set_device_name(&mut self, name: &str) {
        let mut end = name.len().min(63);
        while !name.is_char_boundary(end) { end -= 1; }
        let mut bytes = [0u8; 64];
        bytes[..end].copy_from_slice(&name.as_bytes()[..end]);
        for (i, w) in self.device_name.iter_mut().enumerate() {
            *w = u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
        }
    }
}

/// Backend ids (`OpenReq::backend`, `HalInfo::backend`).
pub mod backend_id {
    pub const HEADLESS: u32 = 0;
    /// Metal (macOS)
    pub const METAL: u32 = 1;
    /// Vulkan (Linux desktop, or explicit offscreen/externally supplied surfaces).
    pub const VULKAN: u32 = 2;
    /// Direct3D 11 (Windows)
    pub const D3D11: u32 = 3;
    /// OpenGL ES 3 through EGL (Linux X11, Wayland and offscreen).
    pub const GL: u32 = 4;
    /// Request Vulkan first, then GLES. HalInfo always returns the selected backend.
    pub const AUTO: u32 = u32::MAX - 1;
    pub const NONE: u32 = u32::MAX;
}

/// Shader dialects (`HalInfo::dialect`, `PipelineCreate`).
pub mod dialect {
    pub const NONE: u32 = 0;
    pub const MSL: u32 = 1;
    pub const SPIRV: u32 = 2;
    pub const HLSL: u32 = 3;
    pub const GLSL: u32 = 4;
}

/// Texture formats and their size.
pub mod format {
    pub const RGBA8: u32 = 0;
    pub const BGRA8: u32 = 1;
    pub const R8: u32 = 2;
    pub const RG8: u32 = 3;
    pub const RGBA16F: u32 = 4;
    pub const RGBA32F: u32 = 5;
    pub const DEPTH32F: u32 = 6;
    pub const DEPTH24_STENCIL8: u32 = 7;
    /// one-channel float render target / data texture
    pub const R32F: u32 = 8;
    /// two-channel half-float render target
    pub const RG16F: u32 = 9;
    /// one-channel unsigned-integer id buffer
    pub const R32UINT: u32 = 10;
    /// 4x4-texel blocks of 16 bytes (BC7 / BC5 UNORM, ASTC 4x4 LDR)
    pub const BC7: u32 = 11;
    pub const BC5: u32 = 12;
    pub const ASTC4X4: u32 = 13;
    pub const COUNT: u32 = 14;
    /// RGBA8 with sRGB encoding on write (the shader outputs linear colour): a render target
    /// of the Android XR projection chains only (`hal_xr_linear_frames`), outside the texture
    /// formats every backend lists (`COUNT`); Vulkan alone maps it.
    pub const RGBA8_SRGB: u32 = 32;
    /// A two-plane 4:2:0 picture for a video encoder: luma (one byte per pixel) then chroma (U V
    /// at half resolution both ways). A render pass draws into one plane, its layer: 0 luma (an
    /// R8 target of the texture's size), 1 chroma (RG8, half the size). Outside `COUNT`: D3D11
    /// alone maps it (a shared texture an encoder reads where it lies).
    pub const NV12: u32 = 33;

    /// Bytes per texel; 0 for an unknown format.
    pub const fn texel_bytes(f: u32) -> u64 {
        match f {
            RGBA8 | BGRA8 | DEPTH32F | DEPTH24_STENCIL8 | R32F | RG16F | R32UINT | RGBA8_SRGB => 4,
            // 16 bytes per 4x4 texels; NV12: its luma plane's (the chroma adds half: `level_bytes`)
            R8 | BC7 | BC5 | ASTC4X4 | NV12 => 1,
            RG8 => 2,
            RGBA16F => 8,
            RGBA32F => 16,
            _ => 0,
        }
    }

    pub const fn name(f: u32) -> &'static str {
        match f {
            RGBA8 => "Rgba8",
            RGBA8_SRGB => "Rgba8Srgb",
            NV12 => "Nv12",
            BGRA8 => "Bgra8",
            R8 => "R8",
            RG8 => "Rg8",
            RGBA16F => "Rgba16F",
            RGBA32F => "Rgba32F",
            DEPTH32F => "Depth32F",
            DEPTH24_STENCIL8 => "Depth24Stencil8",
            R32F => "R32F",
            RG16F => "Rg16F",
            R32UINT => "R32Uint",
            BC7 => "Bc7",
            BC5 => "Bc5",
            ASTC4X4 => "Astc4x4",
            _ => "unknown",
        }
    }

    pub const fn is_depth(f: u32) -> bool {
        f == DEPTH32F || f == DEPTH24_STENCIL8
    }

    /// The render-target planes of `f` (a pass's layer picks one): 2 for NV12, else 1.
    pub const fn planes(f: u32) -> u32 {
        if f == NV12 { 2 } else { 1 }
    }

    pub const fn is_block(f: u32) -> bool {
        matches!(f, BC7 | BC5 | ASTC4X4)
    }

    /// Bytes of one mip level (one layer) of `w` x `h` texels; whole 4x4 blocks for the block
    /// formats. 0 for an unknown format.
    pub fn level_bytes(f: u32, w: u32, h: u32) -> u64 {
        if is_block(f) {
            (w.max(1) as u64).div_ceil(4) * (h.max(1) as u64).div_ceil(4) * 16
        } else if f == NV12 {
            w.max(1) as u64 * h.max(1) as u64 * 3 / 2
        } else {
            w.max(1) as u64 * h.max(1) as u64 * texel_bytes(f)
        }
    }
}

/// Texture usage bits (`TextureCreate` flags).
pub mod usage {
    pub const RENDER_TARGET: u16 = 1;
    pub const SAMPLED: u16 = 2;
    pub const SHARED: u16 = 4;
    pub const TRANSIENT: u16 = 8;
    /// a layered texture that is a 2D array even with six square colour layers (otherwise those
    /// are a cube: [`crate::session::TextureObj::is_cube`])
    pub const ARRAY: u16 = 16;
}

/// Buffer kinds (`BufferCreate` flags).
pub mod buffer_kind {
    pub const INSTANCE: u16 = 0;
    pub const VERTEX: u16 = 1;
    pub const INDEX: u16 = 2;
    pub const UNIFORM: u16 = 3;
}

/// `Pass::load` values.
pub mod pass_load {
    pub const LOAD: u8 = 0;
    pub const CLEAR: u8 = 1;
    pub const DONT_CARE: u8 = 2;
}

/// How input events encode their small enums. Shared by the backends that produce the events
/// and the Cx intake that reads them.
pub mod input {
    /// `mods` bits of the input events.
    pub const MOD_SHIFT: u32 = 1;
    pub const MOD_CONTROL: u32 = 2;
    pub const MOD_ALT: u32 = 4;
    pub const MOD_LOGO: u32 = 8;

    // `MouseDown` / `MouseUp` `button` is the bit mask Cx's `MouseButton` uses (primary 1,
    // secondary 2, middle 4, back 8, forward 16). A key event's `key` is the index of the key in
    // the key-code table both sides share (`makepad_key_code::KEYCODE_VARIANTS`).

    /// `Scroll::phase`: the gesture phase of a trackpad scroll (`NONE` for a wheel).
    pub mod scroll {
        pub const NONE: u32 = 0;
        pub const BEGAN: u32 = 1;
        /// a raw finger contact with a zero delta
        pub const TOUCHED: u32 = 2;
        pub const CHANGED: u32 = 3;
        pub const ENDED: u32 = 4;
        pub const MOMENTUM: u32 = 5;
        pub const MOMENTUM_ENDED: u32 = 6;
    }

    /// `DragReply::to`
    pub mod reply {
        /// answers a `DragQuery`: `answer` is a [`drag_query`] value
        pub const QUERY: u32 = 0;
        /// answers a `DragOver`: `answer` is a [`drag_response`] value
        pub const OVER: u32 = 1;
    }

    /// What a window drag query found at the point.
    pub mod drag_query {
        pub const NO_ANSWER: u32 = 0;
        pub const CLIENT: u32 = 1;
        pub const CAPTION: u32 = 2;
        pub const SYS_MENU: u32 = 3;
    }

    /// What the app would do with the dragged items.
    pub mod drag_response {
        pub const NONE: u32 = 0;
        pub const COPY: u32 = 1;
        pub const LINK: u32 = 2;
        pub const MOVE: u32 = 3;
    }

    /// `Text::kind`
    pub mod text {
        pub const TYPED: u32 = 0;
        /// the text of the pasteboard, made by the HAL on the paste chord
        pub const PASTE: u32 = 1;
    }

    /// `Pinch::phase`
    pub mod pinch {
        pub const BEGIN: u32 = 0;
        pub const UPDATE: u32 = 1;
        pub const END: u32 = 2;
    }

    /// `TouchPoint::phase`
    pub mod touch {
        pub const START: u32 = 0;
        pub const MOVE: u32 = 1;
        pub const STOP: u32 = 2;
        pub const STABLE: u32 = 3;
    }

    /// `Ime::kind`: `start` / `end` of the event are the replaced range of the committed text.
    pub mod ime {
        pub const START: u32 = 0;
        pub const UPDATE: u32 = 1;
        pub const COMMIT: u32 = 2;
        pub const CANCEL: u32 = 3;
    }

    /// `BackGesture::state`: the step of the system back gesture.
    pub mod back_state {
        pub const START: u32 = 0;
        pub const MOVE: u32 = 1;
        pub const CANCEL: u32 = 2;
    }

    /// `BackGesture::edge`: the screen edge the back gesture started from.
    pub mod back_edge {
        pub const LEFT: u32 = 0;
        pub const RIGHT: u32 = 1;
        pub const NONE: u32 = 2;
    }

    /// `ImeAnimate::curve`: the soft keyboard's timing curve.
    pub mod ime_curve {
        pub const EASE_IN_OUT: u32 = 0;
        pub const EASE_IN: u32 = 1;
        pub const EASE_OUT: u32 = 2;
        pub const LINEAR: u32 = 3;
    }

    /// `ImeShow::config`: the text field's flags (bits 0..3) and its soft keyboard (the fields
    /// below, each the index of the Cx enum's variant in declaration order).
    pub mod ime_config {
        pub const MULTILINE: u32 = 1;
        pub const SECURE: u32 = 1 << 1;
        pub const SUBMIT_ON_ENTER: u32 = 1 << 2;
        pub const READ_ONLY: u32 = 1 << 3;
        /// `InputMode` (4 bits)
        pub const INPUT_MODE_SHIFT: u32 = 8;
        /// `AutoCapitalize` (2 bits)
        pub const AUTOCAPITALIZE_SHIFT: u32 = 12;
        /// `AutoCorrect` (2 bits)
        pub const AUTOCORRECT_SHIFT: u32 = 14;
        /// `ReturnKeyType` (4 bits)
        pub const RETURN_KEY_SHIFT: u32 = 16;
        /// `TextInputContentType` (4 bits)
        pub const CONTENT_TYPE_SHIFT: u32 = 20;
        /// The field of `config` at `shift`, `bits` wide.
        pub fn field(config: u32, shift: u32, bits: u32) -> u32 {
            (config >> shift) & ((1 << bits) - 1)
        }
    }

    /// `DragItem::kind`
    pub mod drag_item {
        pub const FILE: u32 = 0;
        pub const STRING: u32 = 1;
        /// A file the app is still making for a drag out (`crate::drag_promise`): the string is
        /// `<final path>\n<mime type>`. Only in `DragStart`.
        pub const PROMISE: u32 = 2;
    }
}

/// Window kinds (`WindowCreate`).
pub mod window_kind {
    pub const NORMAL: u32 = 0;
    pub const POPUP: u32 = 1;
    pub const CHROMELESS: u32 = 2;
    /// A window in another process's compositor. No OS window or drawable is created.
    /// The host supplies geometry and ticks; each frame names its imported image in
    /// `Pass::color[0]`. Completion is reported against the window as usual.
    pub const HOSTED: u32 = 3;
    /// Session 0 only (`info_flags::WINDOW_BADGES`): a badge's place over a window of session 0
    /// (`parent`) at `geom`, in the parent's points (top left). It has no surface and no frames
    /// (session 0 draws badges in the window's badge layer, `target::BADGES`); it is never
    /// adopted and is the HAL's own object, not the window system's. Made with
    /// `window_style::BADGE_HOVER`, session 0 sees the pointer move over it (copies of the moves as
    /// events of the badge, and a `MouseLeave` of it); input itself always stays the window's.
    pub const BADGE: u32 = 4;
    /// A tray app's dropdown: borderless, not resizable, above other windows, no Dock / taskbar
    /// entry of its own, made hidden. `WindowAtTray` places and shows it under its tray icon; its
    /// focus loss answers `PopupDismissed` (the app hides it). No parent.
    pub const DROPDOWN: u32 = 5;
    /// A head-mounted display's stereo view (`info_flags::XR`): the shell sizes it (its geom is
    /// both eyes side by side, the left eye in the left half) and paces it with `XrFrame` events,
    /// each followed by the window's FrameReady. A frame must be drawn with the eyes of the
    /// newest `XrFrame`; the shell shows it as the runtime's projection layer with those eyes.
    pub const XR: u32 = 6;
}

/// `TraySet::flags`.
pub mod tray_flags {
    /// a primary click answers `TrayClick`, the menu opens on a secondary click
    pub const DROPDOWN: u32 = 1;
}

/// `TextureImport::kind`.
pub mod texture_handle {
    /// The backend's native shared image handle (Metal: a global IOSurface ID).
    pub const NATIVE: u32 = 0;
    /// The luma plane of a native biplanar 4:2:0 picture (Metal: plane 0 of a `420v`/`420f`
    /// IOSurface, by global id), as an R8 texture.
    pub const LUMA: u32 = 1;
    /// Its interleaved chroma plane (Metal: plane 1), as an RG8 texture of half the size.
    pub const CHROMA: u32 = 2;
    /// A decoder's whole picture (Vulkan on Android: a stream decoder's picture ticket, a Y'CbCr
    /// hardware buffer) as one video texture: sampled from a `texture_video` slot it gives the
    /// picture's raw codes (.g = Y', .b = Cb, .r = Cr, 0..1 of the 8-bit range).
    pub const VIDEO: u32 = 3;
    /// An XR runtime's environment depth image (Vulkan on Android: a ticket of the XR shell's,
    /// `XrDepth::image`): both eyes' layers as a 2D array texture, sampled from a
    /// `texture_2d_array` slot it gives the depth (0..1, the image's projection) in .r.
    pub const XR_DEPTH: u32 = 4;
}

/// The bits above the kind byte of `WindowCreate::kind`: how a macOS window is made (Cx's
/// `MacosWindowConfig`). Without any a normal window is titled, closable, miniaturizable and
/// resizable.
pub mod window_style {
    /// an `NSPanel` (a floating panel)
    pub const PANEL: u32 = 1 << 8;
    /// no title bar
    pub const BORDERLESS: u32 = 1 << 9;
    pub const NOT_CLOSABLE: u32 = 1 << 10;
    pub const NOT_MINIATURIZABLE: u32 = 1 << 11;
    pub const NOT_RESIZABLE: u32 = 1 << 12;
    /// window level above normal windows / at the status bar
    pub const LEVEL_FLOATING: u32 = 1 << 13;
    pub const LEVEL_STATUS: u32 = 1 << 14;
    /// a panel that does not activate the app
    pub const NON_ACTIVATING: u32 = 1 << 15;
    pub const KEY_ONLY_IF_NEEDED: u32 = 1 << 16;
    pub const JOIN_ALL_SPACES: u32 = 1 << 17;
    pub const FULLSCREEN_AUXILIARY: u32 = 1 << 18;
    /// the size is exact: not fitted to the attached displays (a capture size may exceed any)
    pub const EXACT_SIZE: u32 = 1 << 19;
    /// a badge (`window_kind::BADGE`) that sees the pointer hover over its rect (never input)
    pub const BADGE_HOVER: u32 = 1 << 20;
}

/// Window states (`WindowState`).
pub mod window_state {
    pub const RESTORE: u32 = 0;
    pub const MINIMIZE: u32 = 1;
    pub const MAXIMIZE: u32 = 2;
    pub const FULLSCREEN: u32 = 3;
    pub const HIDE: u32 = 4;
    pub const SHOW: u32 = 5;
    /// The window's own close button (an app-drawn caption's): the window system answers
    /// `CloseRequested`, as for its native close, and the app decides; an embedded window asks
    /// its parent to close it.
    pub const CLOSE: u32 = 6;
}

/// `MobileUi::op`.
pub mod mobile_ui {
    /// `value` 1: dark status-bar icons (a light background), 0: light icons.
    pub const STATUS_BAR_DARK_ICONS: u32 = 0;
    /// `value`: `orientation::*`.
    pub const ORIENTATION: u32 = 1;
    /// `value` 1: the screen stays on (no idle dimming / lock).
    pub const KEEP_SCREEN_ON: u32 = 2;
    /// `value`: the screen edges (`edge::*` bits) whose system gestures wait for the app.
    pub const DEFER_SYSTEM_GESTURES: u32 = 3;

    pub mod orientation {
        pub const AUTO: u32 = 0;
        pub const PORTRAIT: u32 = 1;
        pub const LANDSCAPE: u32 = 2;
    }

    pub mod edge {
        pub const TOP: u32 = 1;
        pub const LEFT: u32 = 2;
        pub const BOTTOM: u32 = 4;
        pub const RIGHT: u32 = 8;
    }
}

/// `SelectionHandles::op`.
pub mod selection_handles {
    pub const SHOW: u32 = 0;
    pub const MOVE: u32 = 1;
    pub const HIDE: u32 = 2;
}

/// `Frame` target kinds.
pub mod target {
    pub const WINDOW: u32 = 0;
    pub const TEXTURE: u32 = 1;
    /// Or-ed into `kind`: the frame is a pick frame. Its draws name pick-variant pipelines
    /// (ordinary `PipelineCreate`s of the shaders' pick code), its passes draw into id textures
    /// (8-bit), each draw writes `Draw::pick` and binds, after its `ntex` sampled textures
    /// (`pipeline_binding::TEXTURES`), the id twin of each sampled texture (element `i` of `pick_tex`).
    pub const PICK: u32 = 0x100;
    /// Or-ed into `kind` of a window frame: a recorder is due this frame. The HAL copies the
    /// presented window on the GPU into a capture buffer and answers with `ScreenSurface`.
    pub const CAPTURE: u32 = 0x200;
    /// Keep this window frame for a following `Snapshot`, without retaining every frame.
    pub const SNAPSHOT: u32 = 0x400;
    /// Executor-set flag: a session-0 overlay frame, never a transfer of ownership.
    pub const OVERLAY: u32 = 0x800;
    /// Or-ed into `kind` of a window frame of session 0 (`info_flags::WINDOW_BADGES`): the
    /// window's badge layer (the JIT ring, its tooltip), in window pixels, its passes loading
    /// what is under them. The HAL keeps it and draws it over every frame the window presents,
    /// whoever drew that frame (a borrower's too), until the next one; it presents the window
    /// again at once (the app may be idle). One with no draws ends the layer.
    pub const BADGES: u32 = 0x1000;
    /// Executor-set flag: the kept badge layer drawn again over a new frame of its window (no
    /// `FrameDone`: session 0 heard of that frame when it sent it).
    pub const REPLAY: u32 = 0x2000;
    /// The bits of `kind` that are flags; the rest is the target.
    pub const FLAGS: u32 = PICK | CAPTURE | SNAPSHOT | OVERLAY | BADGES | REPLAY;
}

/// Layout of `PipelineDesc::bindings` (the blob's `[u32]` binding table): the buffer slot of each
/// uniform block (`NONE`: the shader has none), then the texture count and the custom uniform
/// buffer slots.
pub mod pipeline_binding {
    pub const DRAW_CALL: usize = 0;
    pub const PASS: usize = 1;
    pub const DRAW_LIST: usize = 2;
    pub const DYN: usize = 3;
    pub const SCOPE: usize = 4;
    pub const TEXTURES: usize = 5;
    /// the custom uniform buffer slots follow
    pub const BUFFERS: usize = 6;
    pub const NONE: u32 = u32::MAX;
}

/// `Lifecycle` event kinds.
pub mod lifecycle {
    pub const FOREGROUND: u32 = 0;
    pub const BACKGROUND: u32 = 1;
    pub const PAUSE: u32 = 2;
    pub const RESUME: u32 = 3;
    pub const QUIT_REQUESTED: u32 = 4;
    pub const DESTROYED: u32 = 5;
    pub const FOCUS_GAINED: u32 = 6;
    pub const FOCUS_LOST: u32 = 7;
    pub const SURFACE_LOST: u32 = 8;
    pub const SURFACE_AVAILABLE: u32 = 9;
}

/// `Readback` event status.
pub mod readback {
    pub const OK: u32 = 0;
    /// The id was not a live texture / window.
    pub const STALE: u32 = 1;
    /// headless: this backend has no raster, `pixels` is empty
    pub const NO_RASTER: u32 = 2;
    /// A format that is not 8-bit colour: `pixels` are the texels in the texture's own layout
    /// (rows of the rect, tightly packed). 8-bit colour comes back as RGBA8 with `OK`.
    pub const NATIVE: u32 = 3;
    /// The GPU could not deliver the pixels (device lost, allocation refused).
    pub const FAILED: u32 = 4;
    /// A pick replay skipped a pending pipeline or sampled an incomplete pick target. Retry it.
    pub const INCOMPLETE: u32 = 5;
}

/// One element of `DragStart` / `DragMove` item arrays.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DragItem {
    pub kind: u32,
    /// In a command: a [`StrId`] of the session string table. In an event the table only runs
    /// Cx -> HAL, so the strings ride in the payload: `str` is the byte offset, from the start of
    /// the payload, of a (u32 length + utf8 bytes) record that follows the `count` items (see
    /// `buffers::pack_drag_items` / `read_drag_items`).
    pub str: StrId,
}

/// One element of the `MenuSet` array.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuItem {
    pub parent: u32,
    pub kind: u32,
    pub cmd: u64,
    pub key: u32,
    pub str: StrId,
}

/// One element of the `Touch` event array.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchPoint {
    pub id: u32,
    pub phase: u32,
    pub x: f32,
    pub y: f32,
    pub force: f32,
}

/// A tracked pose in the XR session's reference space: metres, a unit quaternion (x, y, z, w);
/// y up, -z forward at the start of the session, the floor at y = 0 when the runtime knows it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrPose {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
}

/// One eye of an `XrFrame`: where it is and its field of view (radians: left, right, up, down;
/// left and down negative).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrEye {
    pub pose: XrPose,
    pub fov: [f32; 4],
}

/// One controller (the old Android backend's `XrController`, field for field): its grip and
/// aim poses, the analog trigger and grip (0..1), [`xr_controller`] bits and the thumbstick
/// (-1..1, up positive, as the runtime reports it).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrController {
    pub grip_pose: XrPose,
    pub aim_pose: XrPose,
    pub trigger: f32,
    pub grip: f32,
    pub buttons: u32,
    pub stick: [f32; 2],
}

/// `XrController::buttons` (the bits of Cx `XrController`).
pub mod xr_controller {
    pub const CLICK_X: u32 = 1 << 0;
    pub const CLICK_Y: u32 = 1 << 1;
    pub const CLICK_A: u32 = 1 << 2;
    pub const CLICK_B: u32 = 1 << 3;
    pub const CLICK_MENU: u32 = 1 << 4;
    /// the controller is tracked (its poses are valid)
    pub const ACTIVE: u32 = 1 << 5;
    pub const CLICK_THUMBSTICK: u32 = 1 << 6;
    pub const TOUCH_X: u32 = 1 << 7;
    pub const TOUCH_Y: u32 = 1 << 8;
    pub const TOUCH_A: u32 = 1 << 9;
    pub const TOUCH_B: u32 = 1 << 10;
    pub const TOUCH_THUMBSTICK: u32 = 1 << 11;
    pub const TOUCH_TRIGGER: u32 = 1 << 12;
    pub const TOUCH_THUMBREST: u32 = 1 << 13;
}

/// One tracked hand (XR_EXT_hand_tracking + XR_FB_hand_tracking_aim; the old backend's
/// `XrHand`, field for field): the 21 joints that are not finger tips (the 26 of the
/// extension without the tips), each tip as its distance from the joint before it, the aim
/// pose, [`xr_hand`] flags and the pinch strengths (index, middle, ring, little; 0..255).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrHand {
    pub flags: u32,
    pub joints: [XrPose; xr_hand::JOINT_COUNT],
    pub tips: [f32; 5],
    /// a bit per tip (1 << tip) whose distance is valid, and [`xr_hand::GRAB_ACTIVE`]
    pub tips_active: u32,
    pub aim_pose: XrPose,
    pub pinch: [u8; 4],
}

impl Default for XrHand {
    fn default() -> Self { XrHand { flags: 0, joints: [XrPose::default(); xr_hand::JOINT_COUNT], tips: [0.; 5], tips_active: 0, aim_pose: XrPose::default(), pinch: [0; 4] } }
}

/// `XrHand` flags, joint slots and pinch slots (the values of Cx `XrHand`).
pub mod xr_hand {
    pub const IN_VIEW: u32 = 1 << 0;
    pub const AIM_VALID: u32 = 1 << 1;
    pub const PINCH_INDEX: u32 = 1 << 2;
    pub const PINCH_MIDDLE: u32 = 1 << 3;
    pub const PINCH_RING: u32 = 1 << 4;
    pub const PINCH_LITTLE: u32 = 1 << 5;
    pub const DOMINANT_HAND: u32 = 1 << 6;
    pub const MENU_PRESSED: u32 = 1 << 7;
    /// in `tips_active`: the grasp (hand_interaction_ext) is held, with hysteresis
    pub const GRAB_ACTIVE: u32 = 1 << 5;
    pub const PINCH_STRENGTH_INDEX: usize = 0;
    pub const PINCH_STRENGTH_MIDDLE: usize = 1;
    pub const PINCH_STRENGTH_RING: usize = 2;
    pub const PINCH_STRENGTH_LITTLE: usize = 3;
    pub const JOINT_COUNT: usize = 21;
    pub const CENTER: usize = 0;
    pub const WRIST: usize = 1;
}

/// The payload of `XrFrame`: the head, both eyes (left, right), both controllers and both hands
/// (left, right) at the frame's predicted display time.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrFrame {
    pub head: XrPose,
    pub eyes: [XrEye; 2],
    pub controllers: [XrController; 2],
    pub hands: [XrHand; 2],
}

/// The header of an `XrDepth` event (`buffers::Wire`): the environment depth images of the
/// first `layers` eyes (XR_META_environment_depth), each `width` x `height` D16 texels
/// (row-major, little-endian u16, nonlinear: the depth of that eye's projection with
/// `near`/`far`, `far` 0 = infinite), eye 0's first, follow it in the payload. `views` are the depth camera of each eye in the local space (y
/// up, the floor at y = 0), at the time the depth was captured. `image`: the image itself for
/// the GPU (`texture_handle::XR_DEPTH` ticket; 0 none). A payload without texels announces a
/// newly acquired image (the GPU reads it from now on); the texels follow in a later event.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrDepth {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub near: f32,
    pub far: f32,
    pub views: [XrEye; 2],
    pub image: u64,
}

// ------------------------------------------------------------------ frames

/// One render pass of a frame. `draws` is an element range into the packet's draw array.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Pass {
    pub color: [Id; 4],
    pub depth: Id,
    /// per colour attachment, then depth: [`pass_load`]
    pub load: [u8; 5],
    /// per colour attachment: the clear colour (used when its `load` is CLEAR)
    pub clear: [[f32; 4]; 4],
    pub clear_depth: f32,
    /// per colour attachment: the cube face / array slice it renders into
    pub layer: [u32; 4],
    /// x, y, w, h in target pixels. Draw scissors are relative to its origin; with an origin
    /// (an eye of a side-by-side stereo target) the pass clears and stores from the origin on.
    pub viewport: [f32; 4],
    pub dpi: f32,
    /// 0 or 1: one view. 2 (`info_flags::MULTIVIEW` only): multiview, both views in one pass --
    /// a window target is a two-layer image (one layer per view, each the viewport's size), the
    /// draws run their pipelines' multiview stages once and the GPU renders every layer, the
    /// pass uniforms' `_r` matrices being the second view's.
    pub views: u32,
    /// Samples per pixel: 0 or 1 single sampling; 2, 4 or 8 (a count in [`HalInfo::msaa`],
    /// else single sampled) an MSAA pass: it renders into multisampled colour and depth images
    /// of its attachments' formats and size the backend keeps per attachment, resolved into the
    /// attachments at its end (colour: the samples' average; depth: sample 0). A pass that loads
    /// continues the multisampled images (the second eye of a side-by-side target), not what
    /// another pass wrote into the attachments.
    pub samples: u32,
    pub uniforms: Slice,
    pub draws: Range,
    pub name: StrId,
    pub owner: StrId,
}

unsafe impl Plain for Pass {}

/// Where a draw's instances come from: inline floats (a borrowed [`Slice`]) or a retained buffer.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Inst {
    /// [`Inst::INLINE`] or [`Inst::BUFFER`]
    pub kind: u32,
    pub id: Id,
    pub off: u32,
    pub count: u32,
    pub data: Slice,
}

impl Inst {
    pub const INLINE: u32 = 0;
    pub const BUFFER: u32 = 1;

    /// Instances read from `data` (borrowed for the submit call).
    pub fn inline(data: Slice, count: u32) -> Inst {
        Inst { kind: Inst::INLINE, id: Id::NONE, off: 0, count, data }
    }

    /// Instances read from a retained instance buffer.
    pub fn buffer(id: Id, off: u32, count: u32) -> Inst {
        Inst { kind: Inst::BUFFER, id, off, count, data: Slice::EMPTY }
    }
}

/// One draw call. `inst_ranges` is an element range into the arena's `[u32; 2]` (first, count)
/// array for recorded instances.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Draw {
    pub pipeline: Id,
    pub vb: Id,
    pub ib: Id,
    /// first, count
    pub index: [u32; 2],
    pub inst: Inst,
    pub inst_ranges: Range,
    /// draw, list, pass-dynamic, scope
    pub uniforms: [Slice; 4],
    pub ubufs: [Id; 4],
    /// The sampled textures, one per texture the pipeline samples (`pipeline_binding::TEXTURES`),
    /// in binding order: an ELEMENT range into the arena's `[u32; 2]` (index, gen) array, as
    /// [`CmdWriter::put_ids`](crate::buffers::CmdWriter::put_ids) writes it and
    /// [`id_at`](crate::buffers::id_at) reads it. No fixed slot count: a slot past the range is
    /// [`Id::NONE`].
    pub tex: Range,
    /// Id targets sampled by a PICK replay, parallel to `tex` (the same encoding).
    pub pick_tex: Range,
    pub scissor: [f32; 4],
    pub zbias: f32,
    pub pick: u32,
    /// [`Draw::DEPTH_WRITE`] | [`Draw::CULL`] | [`Draw::BLEND`] and the other `Draw` flag bits.
    pub flags: u32,
    pub owner: StrId,
    /// Cx draw-list id, draw-item id and the list's debug name
    pub list: u32,
    pub item: u32,
    pub list_name: StrId,
    /// instance AABB in target px; NaN = unknown
    pub bounds: [f32; 4],
}

impl Draw {
    pub const DEPTH_WRITE: u32 = 1;
    pub const CULL: u32 = 2;
    pub const BLEND: u32 = 4;
    /// the index buffer holds u32 indices (otherwise u16)
    pub const INDEX_U32: u32 = 8;
    /// triangles drawn as their edges (frame capture's wireframe view; backends without a line
    /// fill mode draw them filled)
    pub const WIREFRAME: u32 = 16;
    /// This draw never occludes input to native content below it.
    pub const INPUT_TRANSPARENT: u32 = 32;
}

unsafe impl Plain for Draw {}

/// Geometry resolved by the frame builder in window pixels. Rectangles are x, y, width,
/// height; the transform is column-major. Radius order is top-left, top-right, bottom-right,
/// bottom-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeLayerGeom {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub radius: [f32; 4],
    pub transform: [f32; 16],
    pub opacity: f32,
    pub flags: u32,
}

impl Default for NativeLayerGeom {
    fn default() -> Self {
        Self {
            rect: [0.0; 4], clip: [0.0; 4], radius: [0.0; 4],
            transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            opacity: 1.0, flags: crate::native_layer::layer_flags::VISIBLE,
        }
    }
}

/// One segment of a window pass. DRAWS names elements of the batch's draw array, all from the
/// same pass. NATIVE carries a layer's unclipped rect and clip in window pixels (x, y, width,
/// height), corner radii, and a column-major transform. `occluders` is an ELEMENT range of
/// `[f32; 4]` x, y, width, height rectangles in the byte arena (`buffers::read_rects`).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub kind: u32,
    pub draws: Range,
    pub native: Id,
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub radius: [f32; 4],
    pub transform: [f32; 16],
    pub opacity: f32,
    pub flags: u32,
    pub occluders: Range,
}

impl Layer {
    pub const DRAWS: u32 = 0;
    pub const NATIVE: u32 = 1;
}

impl Default for Layer {
    fn default() -> Self {
        let g = NativeLayerGeom::default();
        Self { kind: Self::DRAWS, draws: Range::default(), native: Id::NONE,
            rect: g.rect, clip: g.clip, radius: g.radius, transform: g.transform,
            opacity: g.opacity, flags: g.flags, occluders: Range::default() }
    }
}

unsafe impl Plain for Layer {}

/// Timing Cx measured for the frame, in absolute raw monotonic ns (see [`clock_ns`]).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameMeta {
    pub build_start_ns: u64,
    pub build_end_ns: u64,
    /// the causing (oldest consumed) input event
    pub input_ns: u64,
    /// start, end per CPU phase of the app's beat (0, 0: none): dispatch (services, timers),
    /// next-frame callbacks, draw (the draw event walk, shader compiles), then build / submit /
    /// spare (unused; the pass's build is `build_start_ns..build_end_ns`)
    pub phases: [[u64; 2]; 6],
}

/// The one clock of the HAL (events, `FrameMeta`, `FrameDone`, ledger, traces): raw monotonic
/// nanoseconds, absolute (macOS CLOCK_UPTIME_RAW, Linux CLOCK_MONOTONIC; elsewhere the
/// process-start-relative monotonic clock). The headless backend's automation clock replaces it.
pub fn clock_ns() -> u64 {
    crate::clock::raw_ns()
}

/// One window's / target's frame. `passes` and `draws` are ELEMENT ranges into the CmdBuf's
/// `[Pass]` / `[Draw]` arrays (typed arrays of their own, not the byte arena: they hold padding
/// and need alignment); `Pass::draws` is an element range into the same draw array.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FramePacket {
    pub frame: u64,
    pub passes: Range,
    pub draws: Range,
    /// Element range into `CmdBuf::layers`. Empty preserves the unsplit rendering path.
    pub layers: Range,
    pub meta: FrameMeta,
}

unsafe impl Plain for FramePacket {}

/// What a pipeline blob (`PipelineCreate::desc`) starts with; the ranges are relative to the
/// start of the blob.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineDesc {
    pub code: Range,
    /// `[u32]` binding table
    pub bindings: Range,
    /// `[u32]` vertex layout
    pub vertex_layout: Range,
    pub color_formats: [u32; 4],
    pub blend: u32,
    pub depth: u32,
    /// Cx draw_shader index (stable per session; names collide), its debug name and its script
    /// source location (file:line:col)
    pub shader: u32,
    pub name: StrId,
    pub source: StrId,
    /// instance stride in floats and the varying count
    pub instance_stride: u32,
    pub varyings: u32,
    /// bit `i` set when the fragment stage writes colour output `i`
    pub outputs: u32,
}

/// `PipelineDesc::blend` bits.
pub mod pipeline_blend {
    /// per-channel max instead of the premultiplied-over blend
    pub const MAX: u32 = 1;
    /// a raw-write data pass: never blends, alpha is payload
    pub const RAW: u32 = 2;
}

/// Texture and buffer indices from here up are the HAL's: the render target of an embedded window
/// (in the child's table) and the alias a parent samples it by (in the parent's table). Cx's pool
/// ids stay below.
pub const LENT_BASE: u32 = 0x8000_0000;

/// `DomainState::state`.
pub mod domain_state {
    /// the host is compiling / loading the app (`phase`, `pct` say how far)
    pub const COMPILING: u32 = 0;
    pub const RUNNING: u32 = 1;
    pub const CRASHED: u32 = 2;
    pub const EXITED: u32 = 3;
}

/// `DomainStop::mode`.
pub mod domain_stop {
    /// ask the app to quit (a `Lifecycle` quit request), the session closes when it does
    pub const QUIT: u32 = 0;
    /// free the session now
    pub const KILL: u32 = 1;
}
