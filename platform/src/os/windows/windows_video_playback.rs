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
        gpu_texture::with_media_d3d11_lock,
        thread::SignalToUI,
        windows::{
            core::{Interface, BSTR, IUnknown},
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
                    MFARGB, MFCreateAttributes, MFCreateDXGIDeviceManager,
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

use super::windows_media_engine_notify::{
    drain_notify_events, new_media_engine_notify, MediaEngineNotifyState,
};

// ── MTA worker command / event protocol ───────────────────────────────────────

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
static CMD_TX: OnceLock<Sender<MfCmd>> = OnceLock::new();
static EVENTS: Mutex<Vec<MfEvent>> = Mutex::new(Vec::new());
static SESSION_BOOTSTRAP: OnceLock<Mutex<HashMap<u64, SessionBootstrap>>> = OnceLock::new();

fn session_bootstrap_map() -> &'static Mutex<HashMap<u64, SessionBootstrap>> {
    SESSION_BOOTSTRAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Commands that arrive before `Create` finishes on the worker.
#[derive(Default)]
struct SessionBootstrap {
    /// `None` = keep Create's `autoplay`; `Some` = last Play/Pause before ready.
    want_play: Option<bool>,
    pending_seek_ms: Option<u64>,
    pending_mute: Option<bool>,
    pending_volume: Option<f64>,
    pending_playback_rate: Option<f64>,
}

fn take_bootstrap(session: u64) -> SessionBootstrap {
    session_bootstrap_map()
        .lock()
        .unwrap()
        .remove(&session)
        .unwrap_or_default()
}

fn with_bootstrap<F: FnOnce(&mut SessionBootstrap)>(session: u64, f: F) {
    let mut map = session_bootstrap_map().lock().unwrap();
    f(map.entry(session).or_default());
}

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
    SetVolume { session: u64, volume: f64 },
    SetPlaybackRate { session: u64, rate: f64 },
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
        has_audio: bool,
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
    if let Err(send_err) = ensure_worker().send(cmd) {
        if let MfCmd::Create { session, .. } = send_err.0 {
            push_event(MfEvent::CreateFailed {
                session,
                error: "MF video worker unavailable".to_string(),
            });
        }
    }
}

struct WorkerSession {
    engine: IMFMediaEngine,
    _notify: windows::core::ComObject<MediaEngineNotifyState>,
    _dxgi_manager: IMFDXGIDeviceManager,
    device: ID3D11Device,
    /// Triple-buffer BGRA present targets so Transfer cannot overwrite a texture
    /// still sampled by an in-flight GPU frame (double-buffer is not enough).
    render_textures: [Option<(ID3D11Texture2D, ID3D11ShaderResourceView)>; 3],
    write_index: usize,
    width: u32,
    height: u32,
    is_looping: bool,
    autoplay: bool,
    prepared: bool,
    prepare_sent: bool,
    last_pts: Option<i64>,
    temp_file: Option<PathBuf>,
    want_play: bool,
    pending_seek_ms: Option<u64>,
    pending_mute: Option<bool>,
    pending_volume: Option<f64>,
    pending_playback_rate: Option<f64>,
}

// Safety: notify pointer is only touched on the MTA worker thread.
unsafe impl Send for WorkerSession {}

fn init_media_foundation_on_worker() -> Result<(), String> {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        return Err(format!("CoInitializeEx(MTA) failed: {:?}", hr));
    }
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
        .map_err(|e| format!("MFStartup failed: {e:?}"))
}

fn drain_worker_after_init_failure(rx: Receiver<MfCmd>, error: String) {
    while let Ok(cmd) = rx.recv() {
        if let MfCmd::Create { session, .. } = cmd {
            push_event(MfEvent::CreateFailed {
                session,
                error: error.clone(),
            });
        }
    }
}

fn apply_session_bootstrap(s: &mut WorkerSession, boot: SessionBootstrap) {
    if let Some(want) = boot.want_play {
        s.want_play = want;
        s.autoplay = want;
    }
    if boot.pending_seek_ms.is_some() {
        s.pending_seek_ms = boot.pending_seek_ms;
    }
    if boot.pending_mute.is_some() {
        s.pending_mute = boot.pending_mute;
    }
    if boot.pending_volume.is_some() {
        s.pending_volume = boot.pending_volume;
    }
    if boot.pending_playback_rate.is_some() {
        s.pending_playback_rate = boot.pending_playback_rate;
    }
}

fn stop_playback_intent(s: &mut WorkerSession) {
    s.want_play = false;
    s.autoplay = false;
    let _ = unsafe { s.engine.Pause() };
}

fn apply_pending_controls(s: &mut WorkerSession) {
    if let Some(ms) = s.pending_seek_ms.take() {
        s.last_pts = None;
        let _ = unsafe { s.engine.SetCurrentTime(ms as f64 / 1000.0) };
    }
    if let Some(muted) = s.pending_mute.take() {
        let _ = unsafe { s.engine.SetMuted(muted) };
    }
    if let Some(volume) = s.pending_volume.take() {
        let _ = unsafe { s.engine.SetVolume(volume) };
    }
    if let Some(rate) = s.pending_playback_rate.take() {
        let _ = unsafe { s.engine.SetPlaybackRate(rate) };
    }
}

fn mf_worker_main(rx: Receiver<MfCmd>) {
    if let Err(error) = init_media_foundation_on_worker() {
        error!("VIDEO: {error}");
        drain_worker_after_init_failure(rx, error);
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
            } => {
                let boot = take_bootstrap(session);
                // Last Play/Pause before Create wins over the Create autoplay flag.
                let want_play = boot.want_play.unwrap_or(autoplay);
                match create_session(device, source, is_looping, want_play) {
                    Ok(mut sess) => {
                        apply_session_bootstrap(&mut sess, boot);
                        sessions.insert(session, sess);
                    }
                    Err(error) => {
                        push_event(MfEvent::CreateFailed { session, error });
                    }
                }
            }
            MfCmd::Play(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.want_play = true;
                    s.autoplay = true;
                    if s.prepared {
                        let _ = unsafe { s.engine.Play() };
                        push_event(MfEvent::Playing {
                            session,
                            playing: true,
                        });
                    }
                } else {
                    with_bootstrap(session, |b| b.want_play = Some(true));
                }
            }
            MfCmd::Pause(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    stop_playback_intent(s);
                    push_event(MfEvent::Playing {
                        session,
                        playing: false,
                    });
                } else {
                    with_bootstrap(session, |b| b.want_play = Some(false));
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
                    } else {
                        s.pending_seek_ms = Some(position_ms);
                    }
                } else {
                    with_bootstrap(session, |b| b.pending_seek_ms = Some(position_ms));
                }
            }
            MfCmd::Mute { session, muted } => {
                if let Some(s) = sessions.get_mut(&session) {
                    if s.prepared {
                        let _ = unsafe { s.engine.SetMuted(muted) };
                    } else {
                        s.pending_mute = Some(muted);
                    }
                } else {
                    with_bootstrap(session, |b| b.pending_mute = Some(muted));
                }
            }
            MfCmd::SetVolume { session, volume } => {
                if let Some(s) = sessions.get_mut(&session) {
                    if s.prepared {
                        let _ = unsafe { s.engine.SetVolume(volume) };
                    } else {
                        s.pending_volume = Some(volume);
                    }
                } else {
                    with_bootstrap(session, |b| b.pending_volume = Some(volume));
                }
            }
            MfCmd::SetPlaybackRate { session, rate } => {
                if let Some(s) = sessions.get_mut(&session) {
                    if s.prepared {
                        let _ = unsafe { s.engine.SetPlaybackRate(rate) };
                    } else {
                        s.pending_playback_rate = Some(rate);
                    }
                } else {
                    with_bootstrap(session, |b| b.pending_playback_rate = Some(rate));
                }
            }
            MfCmd::Tick(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    tick_session(session, s);
                }
            }
            MfCmd::Destroy(session) => {
                let _ = take_bootstrap(session);
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

    let mut temp_file: Option<PathBuf> = None;
    let result = (|| {
        let url = match source {
            CreateSource::Url(url) => url,
            CreateSource::Memory { bytes, video_id } => {
                let tmp_path =
                    std::env::temp_dir().join(format!("makepad_video_{video_id}.mp4"));
                std::fs::write(&tmp_path, &bytes)
                    .map_err(|e| format!("temp write failed: {e}"))?;
                temp_file = Some(tmp_path.clone());
                path_to_file_url(&tmp_path.to_string_lossy())
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
        unsafe { MFCreateAttributes(&mut attrs, 4) }
            .map_err(|e| format!("MFCreateAttributes: {e:?}"))?;
        let attributes = attrs.ok_or_else(|| "null attributes".to_string())?;

        let notify_com = new_media_engine_notify();
        let notify_unk: IUnknown = notify_com.clone().into_interface();
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
            _notify: notify_com,
            _dxgi_manager: dxgi_manager,
            device,
            render_textures: [None, None, None],
            write_index: 0,
            width: 0,
            height: 0,
            is_looping,
            prepared: false,
            prepare_sent: false,
            last_pts: None,
            temp_file: temp_file.take(),
            want_play: autoplay,
            autoplay,
            pending_seek_ms: None,
            pending_mute: None,
            pending_volume: None,
            pending_playback_rate: None,
        })
    })();

    if result.is_err() {
        if let Some(path) = temp_file.take() {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

fn push_file_url_char(out: &mut String, ch: char) {
    match ch {
        '/' | '-' | '_' | '.' | '~' | '(' | ')' | '!' | '*' | '\'' => out.push(ch),
        'A'..='Z' | 'a'..='z' | '0'..='9' => out.push(ch),
        ' ' => out.push_str("%20"),
        _ if ch.is_ascii() => {
            for byte in ch.to_string().as_bytes() {
                use std::fmt::Write;
                let _ = write!(out, "%{:02X}", byte);
            }
        }
        // Media Foundation expects Unicode file URLs as UTF-16 in the BSTR, not
        // percent-encoded UTF-8 byte sequences.
        _ => out.push(ch),
    }
}

fn encode_windows_file_url_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    let mut rest = path;
    if let Some(stripped) = rest.strip_prefix('/') {
        out.push('/');
        rest = stripped;
    }

    let path_tail: String = if rest.len() >= 2 {
        let mut chars = rest.chars();
        let a = chars.next().unwrap();
        let b = chars.next().unwrap();
        if a.is_ascii_alphabetic() && b == ':' {
            out.push(a);
            out.push(':');
            let tail: String = chars.collect();
            if let Some(tail) = tail.strip_prefix('/') {
                out.push('/');
                tail.to_string()
            } else {
                tail
            }
        } else {
            rest.to_string()
        }
    } else {
        rest.to_string()
    };

    for ch in path_tail.chars() {
        push_file_url_char(&mut out, ch);
    }
    out
}

fn path_to_file_url(path: &str) -> String {
    if path.starts_with("file://") {
        return path.to_string();
    }
    let normalized = path.replace('\\', "/");
    let encoded = encode_windows_file_url_path(&normalized);
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

fn destroy_session(s: &mut WorkerSession) {
    let _ = unsafe { s.engine.Shutdown() };
    s.render_textures = [None, None, None];
    s.write_index = 0;
    if let Some(path) = s.temp_file.take() {
        let _ = std::fs::remove_file(path);
    }
}

fn create_render_target(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Option<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
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
    if unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.is_err() {
        return None;
    }
    let texture = texture?;
    let resource = texture.cast::<ID3D11Resource>().ok()?;
    let mut srv = None;
    if unsafe {
        device.CreateShaderResourceView(&resource, None, Some(&mut srv))
    }
    .is_err()
    {
        return None;
    }
    Some((texture, srv?))
}

fn ensure_render_textures(s: &mut WorkerSession) {
    if s.width == 0 || s.height == 0 {
        return;
    }
    for slot in &mut s.render_textures {
        if slot.is_none() {
            *slot = create_render_target(&s.device, s.width, s.height);
        }
    }
}

fn media_engine_error_message(engine: &IMFMediaEngine) -> String {
    match unsafe { engine.GetError() } {
        Ok(err) => {
            let code = unsafe { err.GetErrorCode() };
            let label = match code {
                1 => "aborted",
                2 => "network",
                3 => "decode",
                4 => "src_not_supported",
                5 => "encrypted",
                _ => "unknown",
            };
            format!("MediaEngine error: {label} (code {code})")
        }
        Err(e) => format!("MediaEngine error event ({e:?})"),
    }
}

fn tick_session(session: u64, s: &mut WorkerSession) {
    let events = drain_notify_events(s._notify.get());
    for event in events {
        if event == MF_MEDIA_ENGINE_EVENT_CANPLAY.0 as u32 {
            s.prepared = true;
        } else if event == MF_MEDIA_ENGINE_EVENT_ENDED.0 as u32 {
            if !s.is_looping {
                stop_playback_intent(s);
                push_event(MfEvent::Eos { session });
                push_event(MfEvent::Playing {
                    session,
                    playing: false,
                });
            }
        } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            let message = media_engine_error_message(&s.engine);
            error!("VIDEO: {message}");
            stop_playback_intent(s);
            push_event(MfEvent::Playing {
                session,
                playing: false,
            });
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
                s.render_textures = [None, None, None];
                s.write_index = 0;
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
            apply_pending_controls(s);
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
                has_audio: unsafe {
                    (Interface::vtable(&s.engine).HasAudio)(Interface::as_raw(&s.engine))
                }
                .as_bool(),
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

    with_media_d3d11_lock(|| {
        ensure_render_textures(s);
        let write_index = s.write_index;
        let Some((texture, srv)) = s.render_textures[write_index].clone() else {
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

        s.write_index = (write_index + 1) % s.render_textures.len();

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
                    has_audio,
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
                        let audio_tracks = if has_audio {
                            vec!["audio".to_string()]
                        } else {
                            vec![]
                        };
                        self.prepare_result = Some(Ok(PlaybackPrepared::new(
                            width,
                            height,
                            duration_ms,
                            is_seekable,
                            video_tracks,
                            audio_tracks,
                        )));
                    }
                }
                MfEvent::Error { error, .. } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    self.playing.store(false, Ordering::Relaxed);
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

    pub fn set_volume(&self, volume: f64) {
        post(MfCmd::SetVolume {
            session: self.session,
            volume,
        });
    }

    pub fn set_playback_rate(&self, rate: f64) {
        post(MfCmd::SetPlaybackRate {
            session: self.session,
            rate,
        });
    }

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
        post(MfCmd::Tick(self.session));
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
