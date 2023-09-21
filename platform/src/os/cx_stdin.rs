#![allow(dead_code)]
use {
    std::cell::Cell,
    crate::{
        cx::Cx,
        cursor::MouseCursor,
        makepad_micro_serde::*,
        makepad_math::dvec2,
        window::CxWindowPool,
        area::Area,
        event::{
            KeyEvent,
            ScrollEvent,
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
        }
    }
};

// HACK(eddyb) more or less `<[T; N]>::each_ref`, which is still unstable.
fn ref_array_to_array_of_refs<T, const N: usize>(ref_array: &[T; N]) -> [&T; N] {
    let mut out_refs = std::mem::MaybeUninit::<[&T; N]>::uninit();
    for (i, ref_elem) in ref_array.iter().enumerate() {
        unsafe { *out_refs.as_mut_ptr().cast::<&T>().add(i) = ref_elem; }
    }
    unsafe { out_refs.assume_init() }
}

pub const SWAPCHAIN_IMAGE_COUNT: usize = match () {
    // HACK(eddyb) done like this so that we can override each target easily.
    _ if cfg!(target_os = "linux")   => 2,
    _ if cfg!(target_os = "macos")   => 1,
    _ if cfg!(target_os = "windows") => 2,
    _ => 2,
};

/// "Swapchains" group together some number (i.e. `SWAPCHAIN_IMAGE_COUNT` here)
/// of "presentable images", to form a queue of render targets which can be
/// "presented" (to a surface, like a display, window, etc.) independently of
/// rendering being done onto *other* "presentable images" in the "swapchain".
///
/// Certain configurations of swapchains often have older/more specific names,
/// e.g. "double buffering" for `SWAPCHAIN_IMAGE_COUNT == 2` (or "triple" etc.).
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct Swapchain<I>
    // HACK(eddyb) hint `{Ser,De}{Bin,Json}` derivers to add their own bounds.
    where I: Sized
{
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [PresentableImage<I>; SWAPCHAIN_IMAGE_COUNT],
}

impl Swapchain<()> {
    pub fn new(alloc_width: u32, alloc_height: u32) -> Self {
        let presentable_images = [(); SWAPCHAIN_IMAGE_COUNT].map(|()| PresentableImage {
            id: PresentableImageId::alloc(),
            image: (),
        });
        Self { alloc_width, alloc_height, presentable_images }
    }
}

impl<I> Swapchain<I> {
    pub fn get_image(&self, id: PresentableImageId) -> Option<&PresentableImage<I>> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }
    pub fn images_as_ref(&self) -> Swapchain<&I> {
        let Swapchain { alloc_width, alloc_height, ref presentable_images } = *self;
        let presentable_images = ref_array_to_array_of_refs(presentable_images)
            .map(|&PresentableImage { id, ref image }| PresentableImage { id, image });
        Swapchain { alloc_width, alloc_height, presentable_images }
    }
    pub fn images_map<I2>(self, mut f: impl FnMut(PresentableImageId, I) -> I2) -> Swapchain<I2> {
        let Swapchain { alloc_width, alloc_height, presentable_images } = self;
        let presentable_images = presentable_images
            .map(|PresentableImage { id, image }| PresentableImage { id, image: f(id, image) });
        Swapchain { alloc_width, alloc_height, presentable_images }
    }
}

/// One of the "presentable images" of a [`SharedSwapchain`].
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct PresentableImage<I>
    // HACK(eddyb) hint `{Ser,De}{Bin,Json}` derivers to add their own bounds.
    where I: Sized
{
    pub id: PresentableImageId,
    pub image: I,
}

/// Cross-process-unique (on best-effort) ID of a [`SharedPresentableImage`],
/// such that multiple processes on the same system should be able to share
/// swapchains with each-other and (effectively) never observe collisions.
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct PresentableImageId {
    /// PID of the originating process (which allocated this ID).
    origin_pid: u32,

    /// The atomically-acquired value of a (private) counter, during allocation,
    /// in the originating process, which will guarantee that the same process
    /// continuously generating new swapchains will not overlap with itself,
    /// unless it generates billions of swapchains, mixing old and new ones.
    per_origin_counter: u32,
}

impl PresentableImageId {
    pub fn alloc() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        Self {
            origin_pid: std::process::id(),
            per_origin_counter: COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn as_u64(self) -> u64 {
        let Self { origin_pid, per_origin_counter } = self;
        (u64::from(origin_pid) << 32) | u64::from(per_origin_counter)
    }
}

pub type SharedSwapchain = Swapchain<SharedPresentableImageOsHandle>;

// FIXME(eddyb) move these type aliases into `os::{linux,apple,windows}`.

/// [DMA-BUF](crate::os::linux::dma_buf)-backed image from `eglExportDMABUFImageMESA`.
#[cfg(target_os = "linux")]
pub type SharedPresentableImageOsHandle =
    crate::os::linux::dma_buf::Image<crate::os::linux::dma_buf::RemoteFd>;

// HACK(eddyb) the macOS helper XPC service (in `os/apple/metal_xpc.{m,rs}`)
// doesn't need/want any form of "handle passing", as the `id` field contains
// all the disambiguating information it may need (however, long-term it'd
// probably be better to use something like `IOSurface` + mach ports).
#[cfg(target_os = "macos")]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImageOsHandle {
    // HACK(eddyb) working around deriving limitations.
    pub _dummy_for_macos: Option<u32>,
}

/// DirectX 11 `HANDLE` from `IDXGIResource::GetSharedHandle`.
#[cfg(target_os = "windows")]
// FIXME(eddyb) actually use a newtype of `HANDLE` with manual trait impls.
pub type SharedPresentableImageOsHandle = u64;

// FIXME(eddyb) use `enum Foo {}` here ideally, when the derives are fixed.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImageOsHandle {
    // HACK(eddyb) working around deriving limitations.
    pub _dummy_for_unsupported: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseDown{
   pub button: usize,
   pub x: f64,
   pub y: f64,
   pub time: f64,
}

impl From<StdinMouseDown> for MouseDownEvent {
    fn from(v: StdinMouseDown) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            button: v.button,
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseMove{
   pub time: f64,
   pub x: f64,
   pub y: f64
}

impl From<StdinMouseMove> for MouseMoveEvent {
    fn from(v: StdinMouseMove) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseUp{
   pub time: f64,
   pub button: usize,
   pub x: f64,
   pub y: f64
}

impl From<StdinMouseUp> for MouseUpEvent {
    fn from(v: StdinMouseUp) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            button: v.button,
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
        }
    }
}


#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinScroll{
   pub time: f64,
   pub sx: f64,
   pub sy: f64,
   pub x: f64,
   pub y: f64,
   pub is_mouse: bool,
}

impl From<StdinScroll> for ScrollEvent {
    fn from(v: StdinScroll) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            scroll: dvec2(v.sx, v.sy),
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: v.is_mouse,
            time: v.time,
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum HostToStdin{
    Swapchain(SharedSwapchain),
    WindowGeomChange {
        dpi_factor: f64,
        // HACK(eddyb) `DVec` (like `WindowGeom`'s `inner_size` field) can't
        // be used here due to it not implementing (de)serialization traits.
        inner_width: f64,
        inner_height: f64,
    },
    Tick{
        buffer_id: u64,
        frame: u64,
        time: f64,
    },
    MouseDown(StdinMouseDown),
    MouseUp(StdinMouseUp),
    MouseMove(StdinMouseMove),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    Scroll(StdinScroll),
    ReloadFile{
        file:String,
        contents:String
    },
}

/// After a successful client-side draw, all the host needs to know, so it can
/// present the result, is the swapchain image used, and the sub-area within
/// that image that was being used to draw the entire client window (with the
/// whole allocated area rarely used, except just before needing a new swapchain).
#[derive(Copy, Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct PresentableDraw {
    pub target_id: PresentableImageId,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum StdinToHost {
    ReadyToStart,
    SetCursor(MouseCursor),
    // the client is done drawing, and the texture is completely updated
    DrawCompleteAndFlip(PresentableDraw)
}

impl StdinToHost{
    pub fn to_json(&self)->String{
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl HostToStdin{
    pub fn to_json(&self)->String{
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl Cx {
    
}
