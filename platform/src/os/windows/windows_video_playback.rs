//! Windows video playback using IMFMediaEngine (Media Foundation).
//!
//! IMFMediaEngine is the Windows equivalent of AVPlayer (macOS/iOS) or
//! GStreamer playbin (Linux) — a high-level platform video player that
//! handles audio, video decoding, and A/V sync natively.

use {
    crate::{
        event::video_playback::VideoSource,
        makepad_error_log::*,
        makepad_live_id::LiveId,
        texture::{
            CxTexturePool, TextureAlloc, TextureCategory, TextureFormat, TextureId, TexturePixel,
        },
        windows::{
            core::{IUnknown, Interface, GUID, HRESULT},
            Win32::Graphics::{
                Direct3D11::{
                    ID3D11Device, ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D,
                    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
                    D3D11_USAGE_DEFAULT,
                },
                Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            },
            Win32::Media::MediaFoundation::{IMFAttributes, MFCreateAttributes},
        },
    },
    std::{
        ffi::c_void,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
        sync::Mutex,
    },
};

// ── GUIDs ──────────────────────────────────────────────────────────────────────
// Values below come from Windows SDK `mfmediaengine.h` definitions.

const CLSID_MF_MEDIA_ENGINE_CLASS_FACTORY: GUID =
    GUID::from_u128(0xB44392DA_499B_446B_A4CB_005FEAD0E6D5);
const IID_IMF_MEDIA_ENGINE_CLASS_FACTORY: GUID =
    GUID::from_u128(0x4D645ACE_26AA_4688_9be1_df3516990b93);
const IID_IMF_MEDIA_ENGINE_NOTIFY: GUID = GUID::from_u128(0xFEE7C112_E776_42B5_9BBF_0048524E2BD5);

const MF_MEDIA_ENGINE_CALLBACK: GUID = GUID::from_u128(0xC60381B8_83A4_41F8_A3D0_DE05076849A9);
const MF_MEDIA_ENGINE_DXGI_MANAGER: GUID = GUID::from_u128(0x065702da_1094_486d_8617_ee7cc4ee4648);
const MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT: GUID =
    GUID::from_u128(0x5066893c_8cf9_42bc_8b8a_472212e52726);

// Media Engine event constants
const ME_EVENT_ERROR: u32 = 5;
const ME_EVENT_CANPLAY: u32 = 14;
const ME_EVENT_ENDED: u32 = 19;
const ME_EVENT_FORMATCHANGE: u32 = 1000;

// ── Raw FFI ────────────────────────────────────────────────────────────────────

#[link(name = "mfplat")]
extern "system" {
    fn MFStartup(version: u32, flags: u32) -> HRESULT;
    fn MFShutdown() -> HRESULT;
    fn MFCreateDXGIDeviceManager(
        reset_token: *mut u32,
        pp_device_manager: *mut *mut c_void,
    ) -> HRESULT;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> HRESULT;
    fn CoCreateInstance(
        rclsid: *const GUID,
        punk_outer: *mut c_void,
        cls_context: u32,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT;
}

#[link(name = "oleaut32")]
extern "system" {
    fn SysAllocString(psz: *const u16) -> *mut u16;
    fn SysFreeString(bstr: *mut u16);
}

const MF_API_VERSION: u32 = 0x0070;
const MF_VERSION: u32 = (0x0002 << 16) | MF_API_VERSION;
const CLSCTX_INPROC_SERVER: u32 = 0x1;

// ── COM vtable definitions ─────────────────────────────────────────────────────

#[repr(C)]
#[allow(non_snake_case)]
struct IMFMediaEngineVtbl {
    // IUnknown
    QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    Release: unsafe extern "system" fn(*mut c_void) -> u32,
    // IMFMediaEngine
    GetError: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    SetErrorCode: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    SetSourceElements: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    SetSource: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    GetCurrentSource: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HRESULT,
    GetNetworkState: unsafe extern "system" fn(*mut c_void) -> u16,
    GetPreload: unsafe extern "system" fn(*mut c_void) -> u32,
    SetPreload: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    GetBuffered: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    Load: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    CanPlayType: unsafe extern "system" fn(*mut c_void, *const u16, *mut u32) -> HRESULT,
    GetReadyState: unsafe extern "system" fn(*mut c_void) -> u16,
    IsSeeking: unsafe extern "system" fn(*mut c_void) -> i32,
    GetCurrentTime: unsafe extern "system" fn(*mut c_void) -> f64,
    SetCurrentTime: unsafe extern "system" fn(*mut c_void, f64) -> HRESULT,
    GetStartTime: unsafe extern "system" fn(*mut c_void) -> f64,
    GetDuration: unsafe extern "system" fn(*mut c_void) -> f64,
    IsPaused: unsafe extern "system" fn(*mut c_void) -> i32,
    GetDefaultPlaybackRate: unsafe extern "system" fn(*mut c_void) -> f64,
    SetDefaultPlaybackRate: unsafe extern "system" fn(*mut c_void, f64) -> HRESULT,
    GetPlaybackRate: unsafe extern "system" fn(*mut c_void) -> f64,
    SetPlaybackRate: unsafe extern "system" fn(*mut c_void, f64) -> HRESULT,
    GetPlayed: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    GetSeekable: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    IsEnded: unsafe extern "system" fn(*mut c_void) -> i32,
    GetAutoPlay: unsafe extern "system" fn(*mut c_void) -> i32,
    SetAutoPlay: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    GetLoop: unsafe extern "system" fn(*mut c_void) -> i32,
    SetLoop: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    Play: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    Pause: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    GetMuted: unsafe extern "system" fn(*mut c_void) -> i32,
    SetMuted: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    GetVolume: unsafe extern "system" fn(*mut c_void) -> f64,
    SetVolume: unsafe extern "system" fn(*mut c_void, f64) -> HRESULT,
    HasVideo: unsafe extern "system" fn(*mut c_void) -> i32,
    HasAudio: unsafe extern "system" fn(*mut c_void) -> i32,
    GetNativeVideoSize: unsafe extern "system" fn(*mut c_void, *mut u32, *mut u32) -> HRESULT,
    GetVideoAspectRatio: unsafe extern "system" fn(*mut c_void, *mut u32, *mut u32) -> HRESULT,
    Shutdown: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    TransferVideoFrame: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const MFVideoNormalizedRect,
        *const RECT,
        *const MFARGB,
    ) -> HRESULT,
    OnVideoStreamTick: unsafe extern "system" fn(*mut c_void, *mut i64) -> HRESULT,
}

#[repr(C)]
#[allow(non_snake_case)]
struct IMFMediaEngineClassFactoryVtbl {
    QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    Release: unsafe extern "system" fn(*mut c_void) -> u32,
    CreateInstance:
        unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut *mut c_void) -> HRESULT,
    CreateTimeRange: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    CreateError: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
}

#[repr(C)]
#[allow(non_snake_case)]
struct IMFDXGIDeviceManagerVtbl {
    QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    Release: unsafe extern "system" fn(*mut c_void) -> u32,
    CloseDeviceHandle: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    GetVideoService: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT,
    LockDevice: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const GUID,
        *mut *mut c_void,
        i32,
    ) -> HRESULT,
    OpenDeviceHandle: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    ResetDevice: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> HRESULT,
    TestDevice: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    UnlockDevice: unsafe extern "system" fn(*mut c_void, *mut c_void, i32) -> HRESULT,
}

// ── TransferVideoFrame helper structs ──────────────────────────────────────────

#[repr(C)]
#[derive(Default)]
struct MFVideoNormalizedRect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[repr(C)]
#[derive(Default)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct MFARGB {
    blue: u8,
    green: u8,
    red: u8,
    alpha: u8,
}

// ── IMFMediaEngineNotify callback ──────────────────────────────────────────────

#[repr(C)]
#[allow(non_snake_case)]
struct MediaEngineNotifyVtbl {
    QueryInterface:
        unsafe extern "system" fn(*mut MediaEngineNotify, *const GUID, *mut *mut c_void) -> HRESULT,
    AddRef: unsafe extern "system" fn(*mut MediaEngineNotify) -> u32,
    Release: unsafe extern "system" fn(*mut MediaEngineNotify) -> u32,
    EventNotify: unsafe extern "system" fn(*mut MediaEngineNotify, u32, usize, u32) -> HRESULT,
}

#[repr(C)]
struct MediaEngineNotify {
    vtbl: *const MediaEngineNotifyVtbl,
    ref_count: AtomicU32,
    events: Mutex<Vec<u32>>,
}

static NOTIFY_VTBL: MediaEngineNotifyVtbl = MediaEngineNotifyVtbl {
    QueryInterface: notify_query_interface,
    AddRef: notify_add_ref,
    Release: notify_release,
    EventNotify: notify_event_notify,
};

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);

unsafe extern "system" fn notify_query_interface(
    this: *mut MediaEngineNotify,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if riid.is_null() || ppv.is_null() {
        return HRESULT(-2147467261); // E_POINTER
    }
    let iid = *riid;
    if iid == IID_IUNKNOWN || iid == IID_IMF_MEDIA_ENGINE_NOTIFY {
        (*this).ref_count.fetch_add(1, Ordering::SeqCst);
        *ppv = this as *mut c_void;
        HRESULT(0)
    } else {
        *ppv = std::ptr::null_mut();
        HRESULT(-2147467262) // E_NOINTERFACE
    }
}

unsafe extern "system" fn notify_add_ref(this: *mut MediaEngineNotify) -> u32 {
    (*this).ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn notify_release(this: *mut MediaEngineNotify) -> u32 {
    let prev = (*this).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        drop(Box::from_raw(this));
    }
    prev - 1
}

unsafe extern "system" fn notify_event_notify(
    this: *mut MediaEngineNotify,
    event: u32,
    _param1: usize,
    _param2: u32,
) -> HRESULT {
    if let Ok(mut events) = (*this).events.lock() {
        events.push(event);
    }
    HRESULT(0)
}

impl MediaEngineNotify {
    fn create() -> *mut Self {
        Box::into_raw(Box::new(Self {
            vtbl: &NOTIFY_VTBL,
            ref_count: AtomicU32::new(1),
            events: Mutex::new(Vec::new()),
        }))
    }

    unsafe fn drain_events(ptr: *mut Self) -> Vec<u32> {
        if let Ok(mut events) = (*ptr).events.lock() {
            std::mem::take(&mut *events)
        } else {
            Vec::new()
        }
    }
}

// ── Helper: raw COM Release ────────────────────────────────────────────────────

unsafe fn com_release(ptr: *mut c_void) {
    if !ptr.is_null() {
        let vtbl = *(ptr as *const *const usize);
        let release: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));
        release(ptr);
    }
}

// ── Engine creation on MTA thread ──────────────────────────────────────────────
// IMFMediaEngine requires MTA. Makepad's UI thread is STA.
// We create the engine on a temporary MTA thread, then use it from the main
// thread via raw vtable pointers (no COM marshalling needed).

unsafe fn create_engine_on_mta(
    device_raw: usize,
    notify_raw: usize,
    is_looping: bool,
    wide_url: Vec<u16>,
) -> Option<(usize, usize)> {
    let hr = CoInitializeEx(std::ptr::null_mut(), 0x0); // COINIT_MULTITHREADED
    if hr.0 < 0 {
        error!("VIDEO: CoInitializeEx(MTA) failed: {:?}", hr);
        return None;
    }

    let hr = MFStartup(MF_VERSION, 0);
    if hr.is_err() {
        error!("VIDEO: MFStartup failed: {:?}", hr);
        return None;
    }

    // Create DXGI device manager for hardware-accelerated decode
    let mut reset_token: u32 = 0;
    let mut dxgi_manager: *mut c_void = std::ptr::null_mut();
    let hr = MFCreateDXGIDeviceManager(&mut reset_token, &mut dxgi_manager);
    if hr.is_err() || dxgi_manager.is_null() {
        error!("VIDEO: MFCreateDXGIDeviceManager failed: {:?}", hr);
        let _ = MFShutdown();
        return None;
    }

    // Enable multithread protection on the D3D11 device (required when sharing
    // a device between the app's render thread and MF's internal worker threads)
    let device = device_raw as *mut c_void;
    let iid_multithread = GUID::from_u128(0x9B7E4E00_342C_4106_A19F_4F2704F689F0u128);
    let mut mt: *mut c_void = std::ptr::null_mut();
    let vtbl = *(device as *const *const usize);
    let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(*vtbl);
    let hr = qi(device, &iid_multithread, &mut mt);
    if hr.is_ok() && !mt.is_null() {
        // ID3D10Multithread::SetMultithreadProtected is at vtable index 4
        let mt_vtbl = *(mt as *const *const usize);
        let set_protected: unsafe extern "system" fn(*mut c_void, i32) -> i32 =
            std::mem::transmute(*mt_vtbl.add(4));
        set_protected(mt, 1);
        com_release(mt);
    }

    // Bind D3D11 device to DXGI manager
    let mgr_vtbl = *(dxgi_manager as *const *const IMFDXGIDeviceManagerVtbl);
    let hr = ((*mgr_vtbl).ResetDevice)(dxgi_manager, device, reset_token);
    if hr.is_err() {
        error!("VIDEO: ResetDevice failed: {:?}", hr);
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }

    // Set up attributes for engine creation
    let mut attrs: Option<IMFAttributes> = None;
    if let Err(e) = MFCreateAttributes(&mut attrs, 4) {
        error!("VIDEO: MFCreateAttributes failed: {:?}", e);
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }
    let attributes = attrs.unwrap();

    // Set notification callback
    let notify = notify_raw as *mut MediaEngineNotify;
    notify_add_ref(notify);
    let notify_unk: IUnknown = IUnknown::from_raw(notify as *mut c_void);
    if let Err(e) = attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify_unk) {
        error!("VIDEO: SetUnknown(CALLBACK) failed: {:?}", e);
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }

    // Set DXGI device manager
    ((*mgr_vtbl).AddRef)(dxgi_manager);
    let mgr_unk: IUnknown = IUnknown::from_raw(dxgi_manager);
    if let Err(e) = attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &mgr_unk) {
        error!("VIDEO: SetUnknown(DXGI_MANAGER) failed: {:?}", e);
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }

    // Set output format
    let _ = attributes.SetUINT32(
        &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
        DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
    );

    // Media Foundation class factory is provided as an in-proc COM server.
    let mut factory: *mut c_void = std::ptr::null_mut();
    let hr = CoCreateInstance(
        &CLSID_MF_MEDIA_ENGINE_CLASS_FACTORY,
        std::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        &IID_IMF_MEDIA_ENGINE_CLASS_FACTORY,
        &mut factory,
    );
    if hr.is_err() || factory.is_null() {
        let hr_code = hr.0 as u32;
        if hr_code == 0x80040111 {
            error!("VIDEO: CoCreateInstance(MFMediaEngineClassFactory) failed: {:?} (0x{:08X}, CLASS_E_CLASSNOTAVAILABLE). \
                Media Foundation class factory is unavailable. \
                Check: Settings > Apps > Optional features > Media Feature Pack", hr, hr_code);
        } else {
            error!(
                "VIDEO: CoCreateInstance(MFMediaEngineClassFactory) failed: {:?} (0x{:08X}). \
                This typically means Media Foundation is not properly installed. \
                Check: Settings > Apps > Optional features > Media Feature Pack",
                hr, hr_code
            );
        }
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }

    // Create engine instance
    let factory_vtbl = *(factory as *const *const IMFMediaEngineClassFactoryVtbl);
    let mut engine: *mut c_void = std::ptr::null_mut();
    let hr = ((*factory_vtbl).CreateInstance)(
        factory,
        0,
        Interface::as_raw(&attributes) as *mut c_void,
        &mut engine,
    );
    com_release(factory);

    if hr.is_err() || engine.is_null() {
        error!("VIDEO: CreateInstance(engine) failed: {:?}", hr);
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }

    // Configure looping and set source
    let engine_vtbl = *(engine as *const *const IMFMediaEngineVtbl);
    let _ = ((*engine_vtbl).SetLoop)(engine, if is_looping { 1 } else { 0 });

    let bstr = SysAllocString(wide_url.as_ptr());
    let hr = ((*engine_vtbl).SetSource)(engine, bstr);
    SysFreeString(bstr);

    if hr.is_err() {
        error!("VIDEO: SetSource failed: {:?}", hr);
        let _ = ((*engine_vtbl).Shutdown)(engine);
        com_release(engine);
        com_release(dxgi_manager);
        let _ = MFShutdown();
        return None;
    }

    Some((engine as usize, dxgi_manager as usize))
}

// ── WindowsVideoPlayer ────────────────────────────────────────────────────────

pub struct WindowsVideoPlayer {
    engine: *mut c_void,
    notify: *mut MediaEngineNotify,
    dxgi_manager: *mut c_void,
    d3d11_device: ID3D11Device,
    render_texture: Option<ID3D11Texture2D>,
    render_srv: Option<ID3D11ShaderResourceView>,
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    is_prepared: bool,
    prepare_notified: bool,
    prepare_error: Option<String>,
    is_eos: bool,
    eos_notified: bool,
    autoplay: bool,
    video_width: u32,
    video_height: u32,
    temp_file_path: Option<PathBuf>,
}

impl WindowsVideoPlayer {
    fn path_to_file_url(path: &str) -> String {
        if path.starts_with("file://") {
            return path.to_string();
        }
        // IMFMediaEngine::SetSource expects a URL. Normalize Windows paths.
        let normalized = path.replace('\\', "/");
        if normalized.starts_with('/') {
            format!("file://{}", normalized)
        } else {
            format!("file:///{}", normalized)
        }
    }

    pub fn new(
        d3d11_device: &ID3D11Device,
        video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Option<Self> {
        let (wide_url, temp_file_path) = Self::source_to_wide_url(video_id, &source);

        let notify = MediaEngineNotify::create();
        let device_raw = Interface::as_raw(d3d11_device) as usize;
        let notify_raw = notify as usize;

        // Create engine on MTA thread (IMFMediaEngine requires MTA, Makepad UI is STA)
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result =
                unsafe { create_engine_on_mta(device_raw, notify_raw, is_looping, wide_url) };
            let _ = tx.send(result);
        });

        let ptrs = match rx.recv() {
            Ok(Some(ptrs)) => ptrs,
            _ => {
                error!("VIDEO: failed to create IMFMediaEngine for {:?}", video_id);
                unsafe { notify_release(notify) };
                return None;
            }
        };

        Some(Self {
            engine: ptrs.0 as *mut c_void,
            dxgi_manager: ptrs.1 as *mut c_void,
            notify,
            d3d11_device: d3d11_device.clone(),
            render_texture: None,
            render_srv: None,
            video_id,
            texture_id,
            is_prepared: false,
            prepare_notified: false,
            prepare_error: None,
            is_eos: false,
            eos_notified: false,
            autoplay,
            video_width: 0,
            video_height: 0,
            temp_file_path,
        })
    }

    fn source_to_wide_url(video_id: LiveId, source: &VideoSource) -> (Vec<u16>, Option<PathBuf>) {
        match source {
            VideoSource::Network(url) => {
                let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
                (wide, None)
            }
            VideoSource::Filesystem(path) => {
                let file_url = Self::path_to_file_url(path);
                let wide: Vec<u16> = file_url.encode_utf16().chain(std::iter::once(0)).collect();
                (wide, None)
            }
            VideoSource::InMemory(data) => {
                let tmp_path =
                    std::env::temp_dir().join(format!("makepad_video_{}.mp4", video_id.0));
                if let Err(e) = std::fs::write(&tmp_path, data.as_ref()) {
                    error!("VIDEO: failed to write temp file: {}", e);
                }
                let path_str = tmp_path.to_string_lossy().to_string();
                let file_url = Self::path_to_file_url(&path_str);
                let wide: Vec<u16> = file_url.encode_utf16().chain(std::iter::once(0)).collect();
                (wide, Some(tmp_path))
            }
            VideoSource::Camera(..) => {
                error!("VIDEO: Camera source not supported on Windows");
                (vec![0], None)
            }
        }
    }

    #[inline]
    unsafe fn vtbl(&self) -> &'static IMFMediaEngineVtbl {
        &**(self.engine as *const *const IMFMediaEngineVtbl)
    }

    fn process_events(&mut self) {
        let events = unsafe { MediaEngineNotify::drain_events(self.notify) };
        for event in events {
            match event {
                ME_EVENT_CANPLAY => {
                    if !self.is_prepared {
                        self.is_prepared = true;
                    }
                }
                ME_EVENT_ENDED => {
                    self.is_eos = true;
                }
                ME_EVENT_FORMATCHANGE => unsafe {
                    let vtbl = self.vtbl();
                    let mut w: u32 = 0;
                    let mut h: u32 = 0;
                    let hr = (vtbl.GetNativeVideoSize)(self.engine, &mut w, &mut h);
                    if hr.is_ok()
                        && w > 0
                        && h > 0
                        && (w != self.video_width || h != self.video_height)
                    {
                        self.video_width = w;
                        self.video_height = h;
                        self.render_texture = None;
                        self.render_srv = None;
                    }
                },
                ME_EVENT_ERROR => {
                    let message = "MediaEngine error event".to_string();
                    self.prepare_error = Some(message.clone());
                    error!("VIDEO: {}", message);
                }
                _ => {}
            }
        }
    }

    fn ensure_render_texture(&mut self) {
        if self.render_texture.is_some() || self.video_width == 0 || self.video_height == 0 {
            return;
        }
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.video_width,
                Height: self.video_height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture: Option<ID3D11Texture2D> = None;
            if let Err(e) = self
                .d3d11_device
                .CreateTexture2D(&desc, None, Some(&mut texture))
            {
                error!("VIDEO: CreateTexture2D failed: {:?}", e);
                return;
            }
            let texture = texture.unwrap();
            let resource: ID3D11Resource = texture.cast().unwrap();
            let mut srv: Option<ID3D11ShaderResourceView> = None;
            if let Err(e) =
                self.d3d11_device
                    .CreateShaderResourceView(&resource, None, Some(&mut srv))
            {
                error!("VIDEO: CreateShaderResourceView failed: {:?}", e);
                return;
            }
            self.render_texture = Some(texture);
            self.render_srv = srv;
        }
    }

    // ── Public API ─────────────────────────────────────────────────────────────

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        if self.prepare_notified {
            return None;
        }
        self.process_events();
        if let Some(err) = self.prepare_error.take() {
            self.prepare_notified = true;
            return Some(Err(err));
        }
        if !self.is_prepared {
            return None;
        }
        unsafe {
            let vtbl = self.vtbl();
            let mut w: u32 = 0;
            let mut h: u32 = 0;
            let hr = (vtbl.GetNativeVideoSize)(self.engine, &mut w, &mut h);
            if hr.is_err() || w == 0 || h == 0 {
                self.is_prepared = false;
                return None;
            }
            self.video_width = w;
            self.video_height = h;
            let dur = (vtbl.GetDuration)(self.engine);
            let duration_ms = if dur.is_finite() && dur > 0.0 {
                (dur * 1000.0) as u128
            } else {
                0
            };
            self.prepare_notified = true;
            if self.autoplay {
                let _ = (vtbl.Play)(self.engine);
            }
            let is_seekable = duration_ms > 0;
            let video_tracks = if w > 0 && h > 0 {
                vec!["video".to_string()]
            } else {
                vec![]
            };
            let audio_tracks = vec!["audio".to_string()];
            Some(Ok((
                w,
                h,
                duration_ms,
                is_seekable,
                video_tracks,
                audio_tracks,
            )))
        }
    }

    pub fn set_volume(&self, _volume: f64) {
        // TODO: implement via IMFMediaEngine::SetVolume
    }

    pub fn set_playback_rate(&self, _rate: f64) {
        // TODO: implement via IMFMediaEngine::SetPlaybackRate
    }

    /// Returns the canPlayType string for the given MIME type on Windows (Media Foundation).
    pub fn can_play_type(mime: &str) -> &'static str {
        let base = mime.split(';').next().unwrap_or("").trim();
        match base {
            "video/mp4" | "video/x-m4v" => "probably",
            "audio/mp4" | "audio/x-m4a" | "audio/mpeg" | "audio/wav" | "audio/x-wav" => "probably",
            "video/webm" | "audio/webm" => "maybe",
            _ if base.starts_with("video/") || base.starts_with("audio/") => "maybe",
            _ => "",
        }
    }

    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        if !self.is_prepared {
            return false;
        }
        self.process_events();

        unsafe {
            let vtbl = self.vtbl();
            if (vtbl.IsPaused)(self.engine) != 0 {
                return false;
            }
            let mut pts: i64 = 0;
            let hr = (vtbl.OnVideoStreamTick)(self.engine, &mut pts);
            if hr.0 != 0 {
                return false;
            }

            self.ensure_render_texture();
            let texture = match &self.render_texture {
                Some(t) => t,
                None => return false,
            };

            let dst_rect = RECT {
                left: 0,
                top: 0,
                right: self.video_width as i32,
                bottom: self.video_height as i32,
            };
            let border = MFARGB::default();
            let hr = (vtbl.TransferVideoFrame)(
                self.engine,
                Interface::as_raw(texture) as *mut c_void,
                std::ptr::null(),
                &dst_rect,
                &border,
            );
            if hr.is_err() {
                return false;
            }

            // Swap texture into Makepad's texture pool
            let cxtexture = &mut textures[self.texture_id];
            cxtexture.os.texture = self.render_texture.clone();
            cxtexture.os.shader_resource_view = self.render_srv.clone();
            cxtexture.format = TextureFormat::VideoExternal;
            cxtexture.alloc = Some(TextureAlloc {
                width: self.video_width as usize,
                height: self.video_height as usize,
                pixel: TexturePixel::VideoExternal,
                category: TextureCategory::Video,
            });
            true
        }
    }

    pub fn check_eos(&mut self) -> bool {
        if self.eos_notified {
            return false;
        }
        self.process_events();
        if self.is_eos {
            self.eos_notified = true;
            return true;
        }
        false
    }

    pub fn play(&mut self) {
        self.is_eos = false;
        self.eos_notified = false;
        unsafe {
            let _ = (self.vtbl().Play)(self.engine);
        }
    }

    pub fn is_playing(&self) -> bool {
        if !self.is_prepared {
            return false;
        }
        unsafe {
            let vtbl = self.vtbl();
            (vtbl.IsPaused)(self.engine) == 0 && (vtbl.IsEnded)(self.engine) == 0
        }
    }

    pub fn pause(&mut self) {
        unsafe {
            let _ = (self.vtbl().Pause)(self.engine);
        }
    }

    pub fn resume(&mut self) {
        unsafe {
            let _ = (self.vtbl().Play)(self.engine);
        }
    }

    pub fn mute(&mut self) {
        unsafe {
            let _ = (self.vtbl().SetMuted)(self.engine, 1);
        }
    }

    pub fn unmute(&mut self) {
        unsafe {
            let _ = (self.vtbl().SetMuted)(self.engine, 0);
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        self.is_eos = false;
        self.eos_notified = false;
        let seconds = position_ms as f64 / 1000.0;
        unsafe {
            let _ = (self.vtbl().SetCurrentTime)(self.engine, seconds);
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        unsafe {
            let secs = (self.vtbl().GetCurrentTime)(self.engine);
            if secs.is_finite() && secs >= 0.0 {
                (secs * 1000.0) as u128
            } else {
                0
            }
        }
    }

    pub fn cleanup(&mut self) {
        if !self.engine.is_null() {
            unsafe {
                let _ = (self.vtbl().Shutdown)(self.engine);
                com_release(self.engine);
            }
            self.engine = std::ptr::null_mut();
        }
        if !self.notify.is_null() {
            unsafe { notify_release(self.notify) };
            self.notify = std::ptr::null_mut();
        }
        if !self.dxgi_manager.is_null() {
            unsafe { com_release(self.dxgi_manager) };
            self.dxgi_manager = std::ptr::null_mut();
        }
        self.render_texture = None;
        self.render_srv = None;
        if let Some(path) = self.temp_file_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for WindowsVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
