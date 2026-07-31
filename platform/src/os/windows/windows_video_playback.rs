//! Windows video playback using IMFMediaEngine (Media Foundation).
//!
//! All Media Foundation COM calls run on a long-lived MTA worker thread.
//! Makepad's UI thread is STA (`OleInitialize`); calling MF from STA while
//! buffering (especially HLS) hangs the message pump. The UI only posts
//! commands and drains results — never calls into `IMFMediaEngine` directly.

use {
    crate::{
        event::video_playback::VideoSource,
        makepad_error_log::*,
        makepad_live_id::LiveId,
        media_plugin::PlaybackPrepared,
        texture::{
            CxTexturePool, TextureAlloc, TextureCategory, TextureFormat, TextureId, TexturePixel,
        },
        thread::SignalToUI,
        windows::{
            core::{Interface, BSTR, GUID, HRESULT, IUnknown},
            Win32::{
                Foundation::RECT,
                Graphics::{
                    Direct3D11::{
                        ID3D11Device, ID3D11Multithread, ID3D11Resource, ID3D11ShaderResourceView,
                        ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                        D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                    },
                    Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                },
                Media::MediaFoundation::{
                    IMFDXGIDeviceManager, IMFMediaEngine, IMFMediaEngineClassFactory,
                    IMFMediaEngineNotify, MFARGB, MFCreateAttributes, MFCreateDXGIDeviceManager,
                    MFShutdown, MFStartup, CLSID_MFMediaEngineClassFactory, MFSTARTUP_FULL,
                    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER,
                    MF_MEDIA_ENGINE_EVENT_CANPLAY, MF_MEDIA_ENGINE_EVENT_ENDED,
                    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_FORMATCHANGE,
                    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
                },
                System::Com::{
                    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
                },
            },
        },
    },
    std::{
        collections::HashMap,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc::{self, Receiver, Sender},
            Mutex, OnceLock,
        },
    },
};

// ── IMFMediaEngineNotify (minimal COM object; events drained on the MTA worker) ─

#[repr(C)]
struct MediaEngineNotifyVtbl {
    query_interface: unsafe extern "system" fn(
        *mut MediaEngineNotify,
        *const GUID,
        *mut *mut std::ffi::c_void,
    ) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut MediaEngineNotify) -> u32,
    release: unsafe extern "system" fn(*mut MediaEngineNotify) -> u32,
    event_notify:
        unsafe extern "system" fn(*mut MediaEngineNotify, u32, usize, u32) -> HRESULT,
}

#[repr(C)]
struct MediaEngineNotify {
    vtbl: *const MediaEngineNotifyVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    events: Mutex<Vec<u32>>,
}

static NOTIFY_VTBL: MediaEngineNotifyVtbl = MediaEngineNotifyVtbl {
    query_interface: notify_query_interface,
    add_ref: notify_add_ref,
    release: notify_release,
    event_notify: notify_event_notify,
};

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);

unsafe extern "system" fn notify_query_interface(
    this: *mut MediaEngineNotify,
    riid: *const GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> HRESULT {
    if riid.is_null() || ppv.is_null() {
        return HRESULT(-2147467261); // E_POINTER
    }
    let iid = *riid;
    if iid == IID_IUNKNOWN || iid == IMFMediaEngineNotify::IID {
        (*this).ref_count.fetch_add(1, Ordering::SeqCst);
        *ppv = this as *mut std::ffi::c_void;
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
            ref_count: std::sync::atomic::AtomicU32::new(1),
            events: Mutex::new(Vec::new()),
        }))
    }

    unsafe fn drain_events(ptr: *mut Self) -> Vec<u32> {
        if ptr.is_null() {
            return Vec::new();
        }
        if let Ok(mut events) = (*ptr).events.lock() {
            std::mem::take(&mut *events)
        } else {
            Vec::new()
        }
    }
}

// ── MTA worker command / event protocol ───────────────────────────────────────

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
static CMD_TX: OnceLock<Sender<MfCmd>> = OnceLock::new();
static EVENTS: Mutex<Vec<MfEvent>> = Mutex::new(Vec::new());

enum CreateSource {
    Url(String),
    Memory { bytes: Vec<u8>, video_id: u64 },
}

enum MfCmd {
    Create {
        session: u64,
        device: ID3D11Device,
        source: CreateSource,
        is_looping: bool,
        autoplay: bool,
    },
    Play(u64),
    Pause(u64),
    Seek { session: u64, position_ms: u64 },
    Mute { session: u64, muted: bool },
    /// Process notify queue + optionally transfer a video frame.
    Tick(u64),
    Destroy(u64),
}

enum MfEvent {
    CreateFailed {
        session: u64,
        error: String,
    },
    Prepared {
        session: u64,
        width: u32,
        height: u32,
        duration_ms: u128,
    },
    Error {
        session: u64,
        error: String,
    },
    Eos {
        session: u64,
    },
    Playing {
        session: u64,
        playing: bool,
    },
    Frame {
        session: u64,
        texture: ID3D11Texture2D,
        srv: ID3D11ShaderResourceView,
        width: u32,
        height: u32,
        position_ms: u128,
    },
}

fn push_event(ev: MfEvent) {
    EVENTS.lock().unwrap().push(ev);
    SignalToUI::set_ui_signal();
}

fn drain_events_for(session: u64) -> Vec<MfEvent> {
    let mut all = EVENTS.lock().unwrap();
    let mut kept = Vec::new();
    let mut ours = Vec::new();
    for ev in all.drain(..) {
        let sid = match &ev {
            MfEvent::CreateFailed { session, .. }
            | MfEvent::Prepared { session, .. }
            | MfEvent::Error { session, .. }
            | MfEvent::Eos { session }
            | MfEvent::Playing { session, .. }
            | MfEvent::Frame { session, .. } => *session,
        };
        if sid == session {
            ours.push(ev);
        } else {
            kept.push(ev);
        }
    }
    *all = kept;
    ours
}

fn ensure_worker() -> Sender<MfCmd> {
    CMD_TX
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<MfCmd>();
            std::thread::Builder::new()
                .name("makepad-mf-mta".into())
                .spawn(move || mf_worker_main(rx))
                .expect("failed to spawn MF MTA worker");
            tx
        })
        .clone()
}

fn post(cmd: MfCmd) {
    let _ = ensure_worker().send(cmd);
}

struct WorkerSession {
    engine: IMFMediaEngine,
    notify: *mut MediaEngineNotify,
    _dxgi_manager: IMFDXGIDeviceManager,
    device: ID3D11Device,
    render_texture: Option<ID3D11Texture2D>,
    render_srv: Option<ID3D11ShaderResourceView>,
    width: u32,
    height: u32,
    autoplay: bool,
    prepared: bool,
    prepare_sent: bool,
    last_pts: Option<i64>,
    temp_file: Option<PathBuf>,
    want_play: bool,
}

// Safety: notify pointer is only touched on the MTA worker thread.
unsafe impl Send for WorkerSession {}

fn mf_worker_main(rx: Receiver<MfCmd>) {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // S_OK (0) / S_FALSE (1) are success; anything else is fatal for this worker.
    if hr.is_err() {
        error!("VIDEO: CoInitializeEx(MTA) failed: {:?}", hr);
        return;
    }
    if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
        error!("VIDEO: MFStartup failed: {:?}", e);
        return;
    }

    let mut sessions: HashMap<u64, WorkerSession> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            MfCmd::Create {
                session,
                device,
                source,
                is_looping,
                autoplay,
            } => match create_session(device, source, is_looping, autoplay) {
                Ok(sess) => {
                    sessions.insert(session, sess);
                }
                Err(error) => {
                    push_event(MfEvent::CreateFailed { session, error });
                }
            },
            MfCmd::Play(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.want_play = true;
                    if s.prepared {
                        let _ = unsafe { s.engine.Play() };
                        push_event(MfEvent::Playing {
                            session,
                            playing: true,
                        });
                    } else {
                        s.autoplay = true;
                    }
                }
            }
            MfCmd::Pause(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.want_play = false;
                    s.autoplay = false;
                    if s.prepared {
                        let _ = unsafe { s.engine.Pause() };
                    }
                    push_event(MfEvent::Playing {
                        session,
                        playing: false,
                    });
                }
            }
            MfCmd::Seek {
                session,
                position_ms,
            } => {
                if let Some(s) = sessions.get_mut(&session) {
                    if s.prepared {
                        s.last_pts = None;
                        let _ = unsafe { s.engine.SetCurrentTime(position_ms as f64 / 1000.0) };
                    }
                }
            }
            MfCmd::Mute { session, muted } => {
                if let Some(s) = sessions.get_mut(&session) {
                    if s.prepared {
                        let _ = unsafe { s.engine.SetMuted(muted) };
                    }
                }
            }
            MfCmd::Tick(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    tick_session(session, s);
                }
            }
            MfCmd::Destroy(session) => {
                if let Some(mut s) = sessions.remove(&session) {
                    destroy_session(&mut s);
                }
            }
        }
    }

    for (_, mut s) in sessions.drain() {
        destroy_session(&mut s);
    }
    let _ = unsafe { MFShutdown() };
}

fn enable_d3d11_multithread(device: &ID3D11Device) {
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        let _ = unsafe { mt.SetMultithreadProtected(true) };
    }
}

fn create_session(
    device: ID3D11Device,
    source: CreateSource,
    is_looping: bool,
    autoplay: bool,
) -> Result<WorkerSession, String> {
    enable_d3d11_multithread(&device);

    let (url, temp_file) = match source {
        CreateSource::Url(url) => (url, None),
        CreateSource::Memory { bytes, video_id } => {
            let tmp_path = std::env::temp_dir().join(format!("makepad_video_{video_id}.mp4"));
            std::fs::write(&tmp_path, &bytes).map_err(|e| format!("temp write failed: {e}"))?;
            let file_url = path_to_file_url(&tmp_path.to_string_lossy());
            (file_url, Some(tmp_path))
        }
    };

    let mut reset_token = 0u32;
    let mut dxgi_manager = None;
    unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut dxgi_manager) }
        .map_err(|e| format!("MFCreateDXGIDeviceManager: {e:?}"))?;
    let dxgi_manager = dxgi_manager.ok_or_else(|| "null DXGI manager".to_string())?;
    unsafe { dxgi_manager.ResetDevice(&device, reset_token) }
        .map_err(|e| format!("ResetDevice: {e:?}"))?;

    let mut attrs = None;
    unsafe { MFCreateAttributes(&mut attrs, 4) }.map_err(|e| format!("MFCreateAttributes: {e:?}"))?;
    let attributes = attrs.ok_or_else(|| "null attributes".to_string())?;

    let notify = MediaEngineNotify::create();
    // IMFAttributes::SetUnknown takes IUnknown; transfer one ref to attributes.
    let notify_unk = unsafe {
        notify_add_ref(notify);
        IUnknown::from_raw(notify as *mut std::ffi::c_void)
    };
    unsafe { attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify_unk) }
        .map_err(|e| format!("SetUnknown(CALLBACK): {e:?}"))?;
    unsafe { attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &dxgi_manager) }
        .map_err(|e| format!("SetUnknown(DXGI_MANAGER): {e:?}"))?;
    let _ = unsafe {
        attributes.SetUINT32(
            &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
            DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
        )
    };

    let factory: IMFMediaEngineClassFactory = unsafe {
        CoCreateInstance(
            &CLSID_MFMediaEngineClassFactory,
            None,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(|e| {
        format!(
            "CoCreateInstance(MFMediaEngineClassFactory): {e:?}. \
             Check Optional features > Media Feature Pack"
        )
    })?;

    let engine = unsafe { factory.CreateInstance(0, &attributes) }
        .map_err(|e| format!("CreateInstance(engine): {e:?}"))?;
    let _ = unsafe { engine.SetLoop(is_looping) };

    let bstr = BSTR::from(url.as_str());
    unsafe { engine.SetSource(&bstr) }.map_err(|e| format!("SetSource: {e:?}"))?;

    Ok(WorkerSession {
        engine,
        notify,
        _dxgi_manager: dxgi_manager,
        device,
        render_texture: None,
        render_srv: None,
        width: 0,
        height: 0,
        autoplay,
        prepared: false,
        prepare_sent: false,
        last_pts: None,
        temp_file,
        want_play: autoplay,
    })
}

fn path_to_file_url(path: &str) -> String {
    if path.starts_with("file://") {
        return path.to_string();
    }
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

fn destroy_session(s: &mut WorkerSession) {
    let _ = unsafe { s.engine.Shutdown() };
    if !s.notify.is_null() {
        unsafe { notify_release(s.notify) };
        s.notify = std::ptr::null_mut();
    }
    s.render_texture = None;
    s.render_srv = None;
    if let Some(path) = s.temp_file.take() {
        let _ = std::fs::remove_file(path);
    }
}

fn ensure_render_texture(s: &mut WorkerSession) {
    if s.render_texture.is_some() || s.width == 0 || s.height == 0 {
        return;
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: s.width,
        Height: s.height,
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
    let mut texture = None;
    if unsafe { s.device.CreateTexture2D(&desc, None, Some(&mut texture)) }.is_err() {
        return;
    }
    let Some(texture) = texture else {
        return;
    };
    let Ok(resource) = texture.cast::<ID3D11Resource>() else {
        return;
    };
    let mut srv = None;
    if unsafe {
        s.device
            .CreateShaderResourceView(&resource, None, Some(&mut srv))
    }
    .is_err()
    {
        return;
    }
    s.render_texture = Some(texture);
    s.render_srv = srv;
}

fn tick_session(session: u64, s: &mut WorkerSession) {
    let events = unsafe { MediaEngineNotify::drain_events(s.notify) };
    for event in events {
        if event == MF_MEDIA_ENGINE_EVENT_CANPLAY.0 as u32 {
            s.prepared = true;
        } else if event == MF_MEDIA_ENGINE_EVENT_ENDED.0 as u32 {
            push_event(MfEvent::Eos { session });
            push_event(MfEvent::Playing {
                session,
                playing: false,
            });
        } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            let message = "MediaEngine error event".to_string();
            error!("VIDEO: {message}");
            push_event(MfEvent::Error {
                session,
                error: message,
            });
        } else if event == MF_MEDIA_ENGINE_EVENT_FORMATCHANGE.0 as u32 {
            let mut w = 0u32;
            let mut h = 0u32;
            if unsafe { s.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h)) }.is_ok()
                && w > 0
                && h > 0
                && (w != s.width || h != s.height)
            {
                s.width = w;
                s.height = h;
                s.render_texture = None;
                s.render_srv = None;
            }
        }
    }

    if s.prepared && !s.prepare_sent {
        let mut w = 0u32;
        let mut h = 0u32;
        let size_ok = unsafe { s.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h)) }.is_ok();
        if size_ok && w > 0 && h > 0 {
            s.width = w;
            s.height = h;
            let dur = unsafe { s.engine.GetDuration() };
            let duration_ms = if dur.is_finite() && dur > 0.0 {
                (dur * 1000.0) as u128
            } else {
                0
            };
            s.prepare_sent = true;
            if s.autoplay || s.want_play {
                let _ = unsafe { s.engine.Play() };
                push_event(MfEvent::Playing {
                    session,
                    playing: true,
                });
            }
            push_event(MfEvent::Prepared {
                session,
                width: w,
                height: h,
                duration_ms,
            });
        } else {
            // CANPLAY without size yet — keep waiting.
            s.prepared = false;
        }
    }

    if !s.prepared || !s.want_play {
        return;
    }
    if unsafe { s.engine.IsPaused() }.as_bool() {
        push_event(MfEvent::Playing {
            session,
            playing: false,
        });
        return;
    }

    let pts = match unsafe { s.engine.OnVideoStreamTick() } {
        Ok(pts) => pts,
        Err(_) => return,
    };
    if s.last_pts == Some(pts) {
        return;
    }
    s.last_pts = Some(pts);

    ensure_render_texture(s);
    let (Some(texture), Some(srv)) = (s.render_texture.clone(), s.render_srv.clone()) else {
        return;
    };

    let dst = RECT {
        left: 0,
        top: 0,
        right: s.width as i32,
        bottom: s.height as i32,
    };
    let border = MFARGB {
        rgbBlue: 0,
        rgbGreen: 0,
        rgbRed: 0,
        rgbAlpha: 0,
    };
    let unk: IUnknown = texture.cast().unwrap();
    if unsafe {
        s.engine
            .TransferVideoFrame(&unk, None, &dst, Some(&border))
    }
    .is_err()
    {
        return;
    }

    let position_ms = {
        let secs = unsafe { s.engine.GetCurrentTime() };
        if secs.is_finite() && secs >= 0.0 {
            (secs * 1000.0) as u128
        } else {
            0
        }
    };

    push_event(MfEvent::Frame {
        session,
        texture,
        srv,
        width: s.width,
        height: s.height,
        position_ms,
    });
    push_event(MfEvent::Playing {
        session,
        playing: true,
    });
}

// ── UI-side player (no MF calls) ──────────────────────────────────────────────

struct PendingFrame {
    texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
    position_ms: u128,
}

pub struct WindowsVideoPlayer {
    session: u64,
    #[allow(unused)]
    pub(crate) video_id: LiveId,
    texture_id: TextureId,
    prepare_notified: bool,
    prepare_result: Option<Result<PlaybackPrepared, String>>,
    pending_eos: bool,
    eos_notified: bool,
    pending_frame: Option<PendingFrame>,
    position_ms: u128,
    playing: AtomicBool,
    preparing: AtomicBool,
    alive: bool,
}

impl WindowsVideoPlayer {
    pub fn new(
        d3d11_device: &ID3D11Device,
        video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Option<Self> {
        let create_source = match &source {
            VideoSource::Network(url) => CreateSource::Url(url.clone()),
            VideoSource::Filesystem(path) => CreateSource::Url(path_to_file_url(path)),
            VideoSource::InMemory(data) => CreateSource::Memory {
                bytes: data.as_ref().clone(),
                video_id: video_id.0,
            },
            VideoSource::Camera(..) => {
                error!("VIDEO: Camera source not supported on Windows");
                return None;
            }
            VideoSource::PlaybackSession(..) | VideoSource::Session(..) => {
                error!("VIDEO: session sources are handled by the software video player");
                return None;
            }
        };

        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        post(MfCmd::Create {
            session,
            device: d3d11_device.clone(),
            source: create_source,
            is_looping,
            autoplay,
        });

        Some(Self {
            session,
            video_id,
            texture_id,
            prepare_notified: false,
            prepare_result: None,
            pending_eos: false,
            eos_notified: false,
            pending_frame: None,
            position_ms: 0,
            playing: AtomicBool::new(false),
            preparing: AtomicBool::new(true),
            alive: true,
        })
    }

    fn drain_worker_events(&mut self) {
        for ev in drain_events_for(self.session) {
            match ev {
                MfEvent::CreateFailed { error, .. } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    if !self.prepare_notified {
                        self.prepare_result = Some(Err(error));
                    }
                }
                MfEvent::Prepared {
                    width,
                    height,
                    duration_ms,
                    ..
                } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    if !self.prepare_notified {
                        let is_seekable = duration_ms > 0;
                        let video_tracks = if width > 0 && height > 0 {
                            vec!["video".to_string()]
                        } else {
                            vec![]
                        };
                        self.prepare_result = Some(Ok(PlaybackPrepared::new(
                            width,
                            height,
                            duration_ms,
                            is_seekable,
                            video_tracks,
                            vec!["audio".to_string()],
                        )));
                    }
                }
                MfEvent::Error { error, .. } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    if !self.prepare_notified {
                        self.prepare_result = Some(Err(error));
                    } else {
                        error!("VIDEO: {}", error);
                    }
                }
                MfEvent::Eos { .. } => {
                    self.pending_eos = true;
                    self.playing.store(false, Ordering::Relaxed);
                }
                MfEvent::Playing { playing, .. } => {
                    self.playing.store(playing, Ordering::Relaxed);
                }
                MfEvent::Frame {
                    texture,
                    srv,
                    width,
                    height,
                    position_ms,
                    ..
                } => {
                    self.position_ms = position_ms;
                    self.pending_frame = Some(PendingFrame {
                        texture,
                        srv,
                        width,
                        height,
                        position_ms,
                    });
                }
            }
        }
    }

    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        // Ask the worker to process CANPLAY / errors even before the first frame.
        post(MfCmd::Tick(self.session));
        self.drain_worker_events();
        if self.prepare_notified {
            return None;
        }
        if let Some(result) = self.prepare_result.take() {
            self.prepare_notified = true;
            self.preparing.store(false, Ordering::Relaxed);
            Some(result)
        } else {
            None
        }
    }

    pub fn is_preparing(&self) -> bool {
        self.preparing.load(Ordering::Relaxed)
    }

    pub fn keep_polling(&self) -> bool {
        self.is_preparing() || self.is_playing()
    }

    pub fn set_volume(&self, _volume: f64) {}

    pub fn set_playback_rate(&self, _rate: f64) {}

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
        post(MfCmd::Tick(self.session));
        self.drain_worker_events();
        let Some(frame) = self.pending_frame.take() else {
            return false;
        };
        let cxtexture = &mut textures[self.texture_id];
        cxtexture.os.texture = Some(frame.texture);
        cxtexture.os.shader_resource_view = Some(frame.srv);
        cxtexture.format = TextureFormat::VideoExternal;
        cxtexture.alloc = Some(TextureAlloc {
            width: frame.width as usize,
            height: frame.height as usize,
            pixel: TexturePixel::VideoExternal,
            category: TextureCategory::Video,
        });
        self.position_ms = frame.position_ms;
        true
    }

    pub fn check_eos(&mut self) -> bool {
        self.drain_worker_events();
        if self.eos_notified {
            return false;
        }
        if self.pending_eos {
            self.eos_notified = true;
            self.pending_eos = false;
            return true;
        }
        false
    }

    pub fn play(&mut self) {
        self.eos_notified = false;
        self.pending_eos = false;
        self.playing.store(true, Ordering::Relaxed);
        post(MfCmd::Play(self.session));
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn pause(&mut self) {
        self.playing.store(false, Ordering::Relaxed);
        post(MfCmd::Pause(self.session));
    }

    pub fn resume(&mut self) {
        self.play();
    }

    pub fn mute(&mut self) {
        post(MfCmd::Mute {
            session: self.session,
            muted: true,
        });
    }

    pub fn unmute(&mut self) {
        post(MfCmd::Mute {
            session: self.session,
            muted: false,
        });
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        self.eos_notified = false;
        self.pending_eos = false;
        post(MfCmd::Seek {
            session: self.session,
            position_ms,
        });
    }

    pub fn current_position_ms(&self) -> u128 {
        self.position_ms
    }

    pub fn cleanup(&mut self) {
        if self.alive {
            post(MfCmd::Destroy(self.session));
            self.alive = false;
            self.playing.store(false, Ordering::Relaxed);
            self.preparing.store(false, Ordering::Relaxed);
        }
    }
}

impl Drop for WindowsVideoPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
