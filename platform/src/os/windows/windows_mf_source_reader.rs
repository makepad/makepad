//! Media Foundation Source Reader + DXGI NV12 true zero-copy present.
//!
//! Progressive files use this path: DXVA decoder writes into a shared D3D11
//! Texture2DArray, we pull `IMFDXGIBuffer` from each sample, and adopt Y/UV
//! plane SRVs via [`crate::gpu_texture::adopt_d3d11_nv12_biplanar`].
//!
//! Adaptive streams (HLS/DASH) stay on `IMFMediaEngine`. Force MediaEngine
//! for progressive with `MAKEPAD_MF_ENGINE=1`.
//!
//! Playback controls (seek / mute / volume / rate / loop) and PCM audio via
//! WASAPI are handled on this worker thread.

use {
    crate::{
        event::video_playback::VideoSource,
        gpu_texture::D3d11Nv12Frame,
        makepad_error_log::*,
        makepad_live_id::LiveId,
        media_plugin::PlaybackPrepared,
        texture::{CxTexturePool, TextureId},
        thread::SignalToUI,
        video_decode::yuv::YuvColorMatrix,
        windows::{
            core::{Interface, GUID, PCWSTR},
            Win32::{
                Graphics::Direct3D11::{
                    ID3D11Device, ID3D11Multithread, ID3D11Texture2D, D3D11_BIND_DECODER,
                    D3D11_BIND_SHADER_RESOURCE,
                },
                Media::{
                    Audio::{
                        eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
                        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, MMDeviceEnumerator, WAVEFORMATEX,
                        WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
                    },
                    KernelStreaming::WAVE_FORMAT_EXTENSIBLE,
                    MediaFoundation::{
                        IMFAttributes, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaType, IMFSample,
                        IMFSourceReader, MFAudioFormat_Float, MFCreateAttributes,
                        MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateSourceReaderFromURL,
                        MFMediaType_Audio, MFMediaType_Video, MFNominalRange_0_255,
                        MFNominalRange_16_235, MFShutdown, MFStartup, MFVideoFormat_NV12,
                        MFVideoTransferMatrix_BT2020_10, MFVideoTransferMatrix_BT2020_12,
                        MFVideoTransferMatrix_BT601, MFVideoTransferMatrix_BT709,
                        MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
                        MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
                        MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
                        MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
                        MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX, MF_PD_DURATION,
                        MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SA_D3D11_BINDFLAGS,
                        MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_STREAMTICK,
                        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_D3D11_BIND_FLAGS,
                        MF_SOURCE_READER_D3D_MANAGER, MF_SOURCE_READER_DISABLE_DXVA,
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                        MF_SOURCE_READER_MEDIASOURCE, MF_VERSION, MFSTARTUP_FULL,
                    },
                    Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
                },
                System::{
                    Com::{
                        StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0},
                        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
                    },
                    Variant::VARENUM,
                },
            },
        },
    },
    std::{
        collections::{HashMap, VecDeque},
        ffi::OsStr,
        os::windows::ffi::OsStrExt,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc::{self, Receiver, Sender},
            Arc, Mutex, OnceLock,
        },
        time::Instant,
    },
};

use super::windows_video_playback::{detect_container_extension, path_to_file_url};

fn source_prefers_media_engine(source: &VideoSource) -> bool {
    let path = match source {
        VideoSource::Filesystem(p) => p.as_str(),
        VideoSource::Network(u) => u.as_str(),
        _ => return false,
    };
    let lower = path.to_ascii_lowercase();
    let ext = lower
        .rsplit(['/', '\\', '?'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, e)| e)
        .unwrap_or("");
    matches!(ext, "wmv" | "asf" | "wma")
}

/// `IMFSample` is apartment-threaded COM; we only move it MTA→UI via our queue
/// and drop it after adopt, so marking Send/Sync is sound for this use.
#[allow(dead_code)]
struct SendSample(IMFSample);
unsafe impl Send for SendSample {}
unsafe impl Sync for SendSample {}

fn create_source_reader_from_url(
    url: &str,
    attributes: &IMFAttributes,
) -> Result<IMFSourceReader, String> {
    let wide: Vec<u16> = OsStr::new(url)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe { MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), attributes) }
        .map_err(|e| format!("MFCreateSourceReaderFromURL: {e:?}"))
}

fn configure_nv12_output(reader: &IMFSourceReader, stream: u32) -> Result<(), String> {
    // Native types from the container are usually compressed (e.g. H264). Build an
    // uncompressed NV12 decoder output type, copying geometry so hardware MFTs
    // accept the type (bare NV12 → MF_E_INVALIDMEDIATYPE with HW transforms).
    let native0 = unsafe { reader.GetNativeMediaType(stream, 0) }.map_err(|e| {
        format!("GetNativeMediaType(0): {e:?}")
    })?;

    let mut attempts: Vec<(&str, IMFMediaType)> = Vec::new();
    if let Ok(mt) = nv12_type_from_native(&native0) {
        attempts.push(("NV12+geometry", mt));
    }
    if let Ok(mt) = bare_nv12_type() {
        attempts.push(("NV12 bare", mt));
    }

    let mut last_err = String::from("no NV12 media type candidates");
    for (label, mt) in attempts {
        match unsafe { reader.SetCurrentMediaType(stream, None, &mt) } {
            Ok(()) => {
                log!("VIDEO: SourceReader SetCurrentMediaType ok ({label})");
                return Ok(());
            }
            Err(e) => {
                last_err = format!("SetCurrentMediaType({label}): {e:?}");
                log!("VIDEO: {last_err}");
            }
        }
    }
    Err(last_err)
}

fn bare_nv12_type() -> Result<IMFMediaType, String> {
    let media_type =
        unsafe { MFCreateMediaType() }.map_err(|e| format!("MFCreateMediaType: {e:?}"))?;
    unsafe {
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| format!("SetGUID(MAJOR_TYPE): {e:?}"))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| format!("SetGUID(NV12): {e:?}"))?;
        let _ = media_type.SetUINT32(&MF_SA_D3D11_BINDFLAGS, nv12_decoder_bind_flags());
    }
    Ok(media_type)
}

fn nv12_type_from_native(native: &IMFMediaType) -> Result<IMFMediaType, String> {
    let media_type = bare_nv12_type()?;
    unsafe {
        if let Ok(v) = native.GetUINT64(&MF_MT_FRAME_SIZE) {
            let _ = media_type.SetUINT64(&MF_MT_FRAME_SIZE, v);
        }
        if let Ok(v) = native.GetUINT64(&MF_MT_FRAME_RATE) {
            let _ = media_type.SetUINT64(&MF_MT_FRAME_RATE, v);
        }
        if let Ok(v) = native.GetUINT64(&MF_MT_PIXEL_ASPECT_RATIO) {
            let _ = media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, v);
        }
        if let Ok(v) = native.GetUINT32(&MF_MT_INTERLACE_MODE) {
            let _ = media_type.SetUINT32(&MF_MT_INTERLACE_MODE, v);
        }
    }
    Ok(media_type)
}

fn sample_to_dxgi_nv12(
    sample: IMFSample,
) -> Result<(ID3D11Texture2D, u32, Arc<dyn std::any::Any + Send + Sync>), String> {
    let buffer = unsafe { sample.GetBufferByIndex(0) }
        .map_err(|e| format!("GetBufferByIndex: {e:?}"))?;
    let dxgi: IMFDXGIBuffer = buffer.cast().map_err(|e| {
        format!("QI IMFDXGIBuffer failed ({e:?}) — decoder did not return a DXGI surface")
    })?;

    let mut tex_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe { dxgi.GetResource(&ID3D11Texture2D::IID, &mut tex_ptr) }
        .map_err(|e| format!("IMFDXGIBuffer::GetResource: {e:?}"))?;
    if tex_ptr.is_null() {
        return Err("IMFDXGIBuffer::GetResource returned null".into());
    }
    let texture: ID3D11Texture2D = unsafe { windows::core::Type::from_abi(tex_ptr) }
        .map_err(|e| format!("ID3D11Texture2D abi: {e:?}"))?;
    let slice = unsafe { dxgi.GetSubresourceIndex() }.unwrap_or(0);
    let keep_alive: Arc<dyn std::any::Any + Send + Sync> = Arc::new(SendSample(sample));
    Ok((texture, slice, keep_alive))
}

fn propvariant_i8(value: i64) -> PROPVARIANT {
    // windows_strip drops the PROPVARIANT extension helpers; build VT_I8 by hand.
    const VT_I8: VARENUM = VARENUM(20);
    PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_I8,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 { hVal: value },
            }),
        },
    }
}

fn propvariant_hns(var: &PROPVARIANT) -> i64 {
    // MF_PD_DURATION is VT_UI8 (100-ns units). Also accept VT_I8.
    unsafe {
        let inner = &*var.Anonymous.Anonymous;
        match inner.vt.0 {
            20 => inner.Anonymous.hVal,          // VT_I8
            21 => inner.Anonymous.uhVal as i64,  // VT_UI8
            _ => 0,
        }
    }
}

fn sample_pcm_f32(sample: &IMFSample) -> Result<Vec<f32>, String> {
    let buffer = unsafe { sample.GetBufferByIndex(0) }
        .map_err(|e| format!("audio GetBufferByIndex: {e:?}"))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut len = 0u32;
    unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }
        .map_err(|e| format!("audio Lock: {e:?}"))?;
    if ptr.is_null() || len == 0 {
        let _ = unsafe { buffer.Unlock() };
        return Ok(Vec::new());
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    let _ = unsafe { buffer.Unlock() };
    Ok(out)
}

fn query_duration_ms(reader: &IMFSourceReader) -> u128 {
    let media_source = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
    match unsafe { reader.GetPresentationAttribute(media_source, &MF_PD_DURATION) } {
        Ok(var) => {
            let hns = propvariant_hns(&var);
            if hns > 0 {
                (hns as u128) / 10_000
            } else {
                0
            }
        }
        Err(_) => 0,
    }
}

fn seek_reader(reader: &IMFSourceReader, position_ms: u64) -> Result<(), String> {
    let hns = (position_ms as i64).saturating_mul(10_000);
    let var = propvariant_i8(hns);
    let time_format = GUID::zeroed();
    unsafe { reader.SetCurrentPosition(&time_format, &var) }
        .map_err(|e| format!("SetCurrentPosition: {e:?}"))?;
    let all = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
    let _ = unsafe { reader.Flush(all) };
    Ok(())
}

fn configure_float_audio(
    reader: &IMFSourceReader,
    stream: u32,
) -> Result<(u32, u32), String> {
    let native = unsafe { reader.GetNativeMediaType(stream, 0) }
        .map_err(|e| format!("audio GetNativeMediaType: {e:?}"))?;
    let native_ch = unsafe { native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }
        .unwrap_or(2)
        .clamp(1, 8);
    // WASAPI path is mono/stereo only — ask MF to downmix wider layouts.
    let channels = if native_ch > 2 { 2 } else { native_ch };
    let rate = unsafe { native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }
        .unwrap_or(48_000)
        .max(8_000);
    let mt = unsafe { MFCreateMediaType() }.map_err(|e| format!("audio MFCreateMediaType: {e:?}"))?;
    unsafe {
        mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| format!("audio MAJOR_TYPE: {e:?}"))?;
        mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)
            .map_err(|e| format!("audio SUBTYPE Float: {e:?}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
            .map_err(|e| format!("audio NUM_CHANNELS: {e:?}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)
            .map_err(|e| format!("audio SAMPLES_PER_SECOND: {e:?}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)
            .map_err(|e| format!("audio BITS_PER_SAMPLE: {e:?}"))?;
        let block = channels.saturating_mul(4);
        mt.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)
            .map_err(|e| format!("audio BLOCK_ALIGNMENT: {e:?}"))?;
        mt.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, block.saturating_mul(rate))
            .map_err(|e| format!("audio AVG_BYTES: {e:?}"))?;
        reader
            .SetCurrentMediaType(stream, None, &mt)
            .map_err(|e| format!("audio SetCurrentMediaType: {e:?}"))?;
    }
    Ok((channels, rate))
}

struct SrAudioOut {
    client: IAudioClient,
    render: IAudioRenderClient,
    channels: usize,
    sample_rate: u32,
    pending: VecDeque<f32>,
    /// Media-domain PCM frames submitted to WASAPI since the last clock reset.
    /// This is the audio master clock (seconds = frames / sample_rate).
    media_frames_consumed: u64,
    muted: bool,
    volume: f32,
    started: bool,
}

unsafe impl Send for SrAudioOut {}

impl SrAudioOut {
    fn open_default(channels: u32, sample_rate: u32) -> Result<Self, String> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|e| format!("MMDeviceEnumerator: {e:?}"))?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|e| format!("GetDefaultAudioEndpoint: {e:?}"))?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|e| format!("Activate IAudioClient: {e:?}"))?;

        let channels = channels.clamp(1, 2) as usize;
        let mut format = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: channels as u16,
                nSamplesPerSec: sample_rate,
                nAvgBytesPerSec: sample_rate * channels as u32 * 4,
                nBlockAlign: (channels * 4) as u16,
                wBitsPerSample: 32,
                cbSize: 22,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 32,
            },
            dwChannelMask: if channels == 1 { 0x4 } else { 0x3 },
            SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        };
        let wave_ptr = &mut format as *mut _ as *const WAVEFORMATEX;
        // ~100ms shared buffer; autoconvert toward engine mix format.
        let buffer_hns = 1_000_000i64;
        unsafe {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    buffer_hns,
                    0,
                    wave_ptr,
                    None,
                )
                .map_err(|e| format!("IAudioClient::Initialize: {e:?}"))?;
        }
        let render: IAudioRenderClient = unsafe { client.GetService() }
            .map_err(|e| format!("GetService IAudioRenderClient: {e:?}"))?;
        Ok(Self {
            client,
            render,
            channels,
            sample_rate,
            pending: VecDeque::new(),
            media_frames_consumed: 0,
            muted: false,
            volume: 1.0,
            started: false,
        })
    }

    fn set_playing(&mut self, playing: bool) {
        if playing {
            if !self.started {
                let _ = unsafe { self.client.Start() };
                self.started = true;
            }
        } else if self.started {
            // Silence anything still queued in the device buffer, then stop.
            // (IAudioClient::Reset is not in the stripped windows bindings.)
            let padding = unsafe { self.client.GetCurrentPadding() }.unwrap_or(0);
            if let Ok(buf_frames) = unsafe { self.client.GetBufferSize() } {
                let avail = buf_frames.saturating_sub(padding);
                if avail > 0 {
                    if let Ok(ptr) = unsafe { self.render.GetBuffer(avail) } {
                        let out = unsafe {
                            std::slice::from_raw_parts_mut(
                                ptr as *mut f32,
                                avail as usize * self.channels,
                            )
                        };
                        out.fill(0.0);
                        let _ = unsafe { self.render.ReleaseBuffer(avail, 0) };
                    }
                }
            }
            let _ = unsafe { self.client.Stop() };
            self.started = false;
            self.pending.clear();
            // Drop unplayed device-buffer frames from the media clock.
            self.media_frames_consumed =
                self.media_frames_consumed.saturating_sub(padding as u64);
        }
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.media_frames_consumed = 0;
    }

    fn reset_clock(&mut self) {
        self.media_frames_consumed = 0;
    }

    /// Media time of audio currently audible (submitted minus WASAPI padding).
    fn media_clock_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        let padding = if self.started {
            unsafe { self.client.GetCurrentPadding() }.unwrap_or(0) as f64
        } else {
            0.0
        };
        let played = (self.media_frames_consumed as f64 - padding).max(0.0);
        played / self.sample_rate as f64
    }

    fn push_pcm(&mut self, samples: &[f32]) {
        self.pending.extend(samples.iter().copied());
        // Cap backlog to ~1s to avoid unbounded growth after seeks/stalls.
        let cap = self.channels.saturating_mul(self.sample_rate.max(1) as usize);
        while self.pending.len() > cap {
            self.pending.pop_front();
        }
    }

    fn write_available(&mut self, rate: f64) {
        if !self.started {
            return;
        }
        let Ok(padding) = (unsafe { self.client.GetCurrentPadding() }) else {
            return;
        };
        let Ok(buf_frames) = (unsafe { self.client.GetBufferSize() }) else {
            return;
        };
        let avail = buf_frames.saturating_sub(padding);
        if avail == 0 {
            return;
        }

        let rate = rate.clamp(0.05, 8.0);
        let src_frames_needed = ((avail as f64) * rate).ceil() as usize;
        let ch = self.channels;
        let have_frames = self.pending.len() / ch;
        let take_frames = src_frames_needed.min(have_frames);
        if take_frames == 0 {
            return;
        }

        let mut src = Vec::with_capacity(take_frames * ch);
        for _ in 0..(take_frames * ch) {
            src.push(self.pending.pop_front().unwrap_or(0.0));
        }

        let out_frames = if rate <= 1.01 && rate >= 0.99 {
            take_frames.min(avail as usize)
        } else {
            ((take_frames as f64) / rate).round() as usize
        }
        .min(avail as usize)
        .max(1);

        let Ok(ptr) = (unsafe { self.render.GetBuffer(out_frames as u32) }) else {
            // Put samples back on failure.
            for s in src.into_iter().rev() {
                self.pending.push_front(s);
            }
            return;
        };
        let out = unsafe { std::slice::from_raw_parts_mut(ptr as *mut f32, out_frames * ch) };
        let gain = if self.muted {
            0.0
        } else {
            self.volume.clamp(0.0, 1.0)
        };
        for frame in 0..out_frames {
            let src_frame = if take_frames <= 1 {
                0
            } else {
                ((frame as f64) * (take_frames.saturating_sub(1) as f64)
                    / ((out_frames.saturating_sub(1)).max(1) as f64))
                    .round() as usize
            }
            .min(take_frames.saturating_sub(1));
            for c in 0..ch {
                out[frame * ch + c] = src[src_frame * ch + c] * gain;
            }
        }
        let _ = unsafe { self.render.ReleaseBuffer(out_frames as u32, 0) };
        // Advance master clock by media frames consumed (not device frames).
        self.media_frames_consumed = self
            .media_frames_consumed
            .saturating_add(take_frames as u64);
    }
}

// ── Worker protocol ──────────────────────────────────────────────────────────

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
static CMD_TX: OnceLock<Sender<SrCmd>> = OnceLock::new();
static EVENTS: Mutex<Vec<SrEvent>> = Mutex::new(Vec::new());
static SESSION_BOOTSTRAP: OnceLock<Mutex<HashMap<u64, SessionBootstrap>>> = OnceLock::new();

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

fn session_bootstrap_map() -> &'static Mutex<HashMap<u64, SessionBootstrap>> {
    SESSION_BOOTSTRAP.get_or_init(|| Mutex::new(HashMap::new()))
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

enum SrCmd {
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
    Tick(u64),
    Destroy(u64),
}

enum SrEvent {
    CreateFailed { session: u64, error: String },
    Prepared {
        session: u64,
        width: u32,
        height: u32,
        duration_ms: u128,
        has_audio: bool,
        matrix: YuvColorMatrix,
        full_range: bool,
    },
    Error { session: u64, error: String },
    Eos { session: u64 },
    Playing { session: u64, playing: bool },
    Frame {
        session: u64,
        frame: D3d11Nv12Frame,
        position_ms: u128,
        full_range: bool,
    },
}

fn push_event(ev: SrEvent) {
    let mut q = EVENTS.lock().unwrap();
    // Keep only the latest Frame per session to avoid UI backlog / keep_alive growth.
    if matches!(&ev, SrEvent::Frame { .. }) {
        let sid = match &ev {
            SrEvent::Frame { session, .. } => *session,
            _ => unreachable!(),
        };
        q.retain(|e| !matches!(e, SrEvent::Frame { session, .. } if *session == sid));
    }
    q.push(ev);
    SignalToUI::set_ui_signal();
}

fn drain_events_for(session: u64) -> Vec<SrEvent> {
    let mut all = EVENTS.lock().unwrap();
    let mut kept = Vec::new();
    let mut ours = Vec::new();
    for ev in all.drain(..) {
        let sid = match &ev {
            SrEvent::CreateFailed { session, .. }
            | SrEvent::Prepared { session, .. }
            | SrEvent::Error { session, .. }
            | SrEvent::Eos { session }
            | SrEvent::Playing { session, .. }
            | SrEvent::Frame { session, .. } => *session,
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

fn ensure_worker() -> Sender<SrCmd> {
    CMD_TX
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<SrCmd>();
            std::thread::Builder::new()
                .name("makepad-mf-sr-mta".into())
                .spawn(move || sr_worker_main(rx))
                .expect("failed to spawn MF SourceReader MTA worker");
            tx
        })
        .clone()
}

fn post(cmd: SrCmd) {
    if let Err(send_err) = ensure_worker().send(cmd) {
        if let SrCmd::Create { session, .. } = send_err.0 {
            push_event(SrEvent::CreateFailed {
                session,
                error: "MF SourceReader worker unavailable".to_string(),
            });
        }
    }
}

fn enable_d3d11_multithread(device: &ID3D11Device) {
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        let _ = unsafe { mt.SetMultithreadProtected(true) };
    }
}

struct WorkerSession {
    reader: IMFSourceReader,
    _dxgi_manager: IMFDXGIDeviceManager,
    width: u32,
    height: u32,
    duration_ms: u128,
    is_looping: bool,
    want_play: bool,
    prepared: bool,
    prepare_sent: bool,
    temp_file: Option<PathBuf>,
    has_audio: bool,
    audio: Option<SrAudioOut>,
    muted: bool,
    volume: f64,
    rate: f64,
    color_matrix: YuvColorMatrix,
    full_range: bool,
    wall_start: Option<Instant>,
    pts_origin_hns: i64,
    /// After seek / start, wait for the first decoded PTS before starting the clock.
    await_clock_anchor: bool,
    /// Accurate seek: discard decoded frames/audio until PTS >= this (100-ns units).
    seek_target_hns: Option<i64>,
    pending_video: Option<(IMFSample, i64)>,
    video_eos: bool,
    /// Hard decode failure — stop ticking this session.
    fatal: bool,
}

unsafe impl Send for WorkerSession {}

fn apply_session_bootstrap(s: &mut WorkerSession, boot: SessionBootstrap) {
    if let Some(want) = boot.want_play {
        s.want_play = want;
    }
    if let Some(muted) = boot.pending_mute {
        s.muted = muted;
        if let Some(a) = s.audio.as_mut() {
            a.muted = muted;
        }
    }
    if let Some(volume) = boot.pending_volume {
        s.volume = volume.clamp(0.0, 1.0);
        if let Some(a) = s.audio.as_mut() {
            a.volume = s.volume as f32;
        }
    }
    if let Some(rate) = boot.pending_playback_rate {
        s.rate = if rate.is_finite() && rate > 0.0 {
            rate.clamp(0.05, 8.0)
        } else {
            1.0
        };
    }
    if let Some(ms) = boot.pending_seek_ms {
        apply_seek(s, ms);
    }
}

fn sr_worker_main(rx: Receiver<SrCmd>) {
    if let Err(e) = init_mf() {
        error!("VIDEO: SourceReader MF init failed: {e}");
        while let Ok(cmd) = rx.recv() {
            if let SrCmd::Create { session, .. } = cmd {
                let _ = take_bootstrap(session);
                push_event(SrEvent::CreateFailed {
                    session,
                    error: e.clone(),
                });
            }
        }
        return;
    }

    let mut sessions: HashMap<u64, WorkerSession> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            SrCmd::Create {
                session,
                device,
                source,
                is_looping,
                autoplay,
            } => {
                let boot = take_bootstrap(session);
                let want_play = boot.want_play.unwrap_or(autoplay);
                match create_session(device, source, is_looping, want_play) {
                    Ok(mut s) => {
                        s.want_play = want_play;
                        if let Err(err) = prepare_session(&mut s) {
                            push_event(SrEvent::CreateFailed { session, error: err });
                            destroy_session(&mut s);
                        } else {
                            apply_session_bootstrap(&mut s, boot);
                            if s.want_play {
                                if let Some(a) = s.audio.as_mut() {
                                    a.set_playing(true);
                                }
                            }
                            sessions.insert(session, s);
                        }
                    }
                    Err(error) => push_event(SrEvent::CreateFailed { session, error }),
                }
            }
            SrCmd::Play(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.want_play = true;
                    s.video_eos = false;
                    // Resume from a frozen clock: restart wall without changing media origin.
                    // Fresh start / post-seek still waits for the first sample via await_clock_anchor.
                    if s.wall_start.is_none() && !s.await_clock_anchor {
                        s.wall_start = Some(Instant::now());
                    }
                    if let Some(a) = s.audio.as_mut() {
                        a.set_playing(true);
                    }
                    push_event(SrEvent::Playing {
                        session,
                        playing: true,
                    });
                } else {
                    with_bootstrap(session, |b| b.want_play = Some(true));
                }
            }
            SrCmd::Pause(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.want_play = false;
                    // Freeze A/V clock across pause so resume does not catch up.
                    if let Some(start) = s.wall_start.take() {
                        let media_elapsed = start.elapsed().as_secs_f64() * s.rate.max(0.05);
                        s.pts_origin_hns += (media_elapsed * 10_000_000.0) as i64;
                    }
                    if let Some(a) = s.audio.as_mut() {
                        a.set_playing(false);
                    }
                    push_event(SrEvent::Playing {
                        session,
                        playing: false,
                    });
                } else {
                    with_bootstrap(session, |b| b.want_play = Some(false));
                }
            }
            SrCmd::Seek {
                session,
                position_ms,
            } => {
                if let Some(s) = sessions.get_mut(&session) {
                    apply_seek(s, position_ms);
                } else {
                    with_bootstrap(session, |b| b.pending_seek_ms = Some(position_ms));
                }
            }
            SrCmd::Mute { session, muted } => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.muted = muted;
                    if let Some(a) = s.audio.as_mut() {
                        a.muted = muted;
                    }
                } else {
                    with_bootstrap(session, |b| b.pending_mute = Some(muted));
                }
            }
            SrCmd::SetVolume { session, volume } => {
                if let Some(s) = sessions.get_mut(&session) {
                    s.volume = volume.clamp(0.0, 1.0);
                    if let Some(a) = s.audio.as_mut() {
                        a.volume = s.volume as f32;
                    }
                } else {
                    with_bootstrap(session, |b| b.pending_volume = Some(volume));
                }
            }
            SrCmd::SetPlaybackRate { session, rate } => {
                if let Some(s) = sessions.get_mut(&session) {
                    let rate = if rate.is_finite() && rate > 0.0 {
                        rate.clamp(0.05, 8.0)
                    } else {
                        1.0
                    };
                    // Audio master clock is in media-frame domain — no re-anchor.
                    // Video-only falls back to wall clock and needs a re-anchor.
                    if s.audio.is_none() {
                        if let Some(start) = s.wall_start {
                            let media_elapsed =
                                start.elapsed().as_secs_f64() * s.rate.max(0.05);
                            s.pts_origin_hns += (media_elapsed * 10_000_000.0) as i64;
                            s.wall_start = Some(Instant::now());
                        }
                    }
                    s.rate = rate;
                } else {
                    with_bootstrap(session, |b| b.pending_playback_rate = Some(rate));
                }
            }
            SrCmd::Tick(session) => {
                if let Some(s) = sessions.get_mut(&session) {
                    tick_session(session, s);
                }
            }
            SrCmd::Destroy(session) => {
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

fn init_mf() -> Result<(), String> {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        return Err(format!("CoInitializeEx(MTA) failed: {hr:?}"));
    }
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
        .map_err(|e| format!("MFStartup failed: {e:?}"))
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
                let ext = detect_container_extension(&bytes);
                let tmp_path =
                    std::env::temp_dir().join(format!("makepad_sr_video_{video_id}.{ext}"));
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

        let unk: windows::core::IUnknown = dxgi_manager
            .cast()
            .map_err(|e| format!("DXGI manager cast: {e}"))?;

        // Prefer hardware MFTs (DXGI surfaces). If NV12 negotiation fails, retry
        // with D3D manager only — DXVA can still attach to the software decoder.
        let (reader, has_audio, audio_fmt) = match open_source_reader(&url, &unk, true) {
            Ok(r) => {
                log!("VIDEO: SourceReader opened with hardware MFTs");
                r
            }
            Err(hw_err) => {
                log!("VIDEO: SourceReader HW MFT path failed ({hw_err}); retrying DXVA-only");
                open_source_reader(&url, &unk, false).map_err(|e| {
                    format!("SourceReader open failed (HW: {hw_err}; DXVA-only: {e})")
                })?
            }
        };

        let mut has_audio = has_audio;
        let audio = if let Some((channels, rate)) = audio_fmt {
            match SrAudioOut::open_default(channels, rate) {
                Ok(a) => {
                    log!("VIDEO: SourceReader WASAPI audio ready ({channels}ch @{rate}Hz)");
                    Some(a)
                }
                Err(err) => {
                    log!("VIDEO: SourceReader WASAPI open failed ({err}); continuing video-only");
                    let audio_stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
                    let _ = unsafe { reader.SetStreamSelection(audio_stream, false) };
                    has_audio = false;
                    None
                }
            }
        } else {
            None
        };
        let has_audio = has_audio && audio.is_some();

        Ok(WorkerSession {
            reader,
            _dxgi_manager: dxgi_manager,
            width: 0,
            height: 0,
            duration_ms: 0,
            is_looping,
            want_play: autoplay,
            prepared: false,
            prepare_sent: false,
            temp_file: temp_file.take(),
            has_audio,
            audio,
            muted: false,
            volume: 1.0,
            rate: 1.0,
            color_matrix: YuvColorMatrix::BT709,
            full_range: false,
            wall_start: None,
            pts_origin_hns: 0,
            await_clock_anchor: true,
            seek_target_hns: None,
            pending_video: None,
            video_eos: false,
            fatal: false,
        })
    })();

    if result.is_err() {
        if let Some(path) = temp_file.take() {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}

fn nv12_decoder_bind_flags() -> u32 {
    // Decoder output must be sampleable for Texture2DArray plane SRVs (true ZC).
    (D3D11_BIND_DECODER.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32
}

fn open_source_reader(
    url: &str,
    dxgi_manager: &windows::core::IUnknown,
    hardware_mfts: bool,
) -> Result<(IMFSourceReader, bool, Option<(u32, u32)>), String> {
    let mut attrs = None;
    unsafe { MFCreateAttributes(&mut attrs, 5) }
        .map_err(|e| format!("MFCreateAttributes: {e:?}"))?;
    let attributes = attrs.ok_or_else(|| "null attributes".to_string())?;
    unsafe { attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, dxgi_manager) }
        .map_err(|e| format!("SetUnknown(D3D_MANAGER): {e:?}"))?;
    unsafe {
        if hardware_mfts {
            attributes
                .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| format!("SetUINT32(HW_TRANSFORMS): {e:?}"))?;
        }
        attributes
            .SetUINT32(&MF_SOURCE_READER_DISABLE_DXVA, 0)
            .map_err(|e| format!("SetUINT32(DISABLE_DXVA=0): {e:?}"))?;
        // Without SHADER_RESOURCE, DXGI NV12 surfaces cannot create plane SRVs and
        // we fall back to a GPU blit (still DXGI, but not true zero-copy).
        attributes
            .SetUINT32(&MF_SOURCE_READER_D3D11_BIND_FLAGS, nv12_decoder_bind_flags())
            .map_err(|e| format!("SetUINT32(D3D11_BIND_FLAGS): {e:?}"))?;
    }

    let reader = create_source_reader_from_url(url, &attributes)?;
    let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    unsafe {
        reader
            .SetStreamSelection(video, true)
            .map_err(|e| format!("SetStreamSelection(video): {e:?}"))?;
    }
    configure_nv12_output(&reader, video)?;

    let mut audio_fmt = None;
    let has_audio = match configure_float_audio(&reader, audio) {
        Ok(fmt) => {
            let _ = unsafe { reader.SetStreamSelection(audio, true) };
            audio_fmt = Some(fmt);
            true
        }
        Err(err) => {
            let _ = unsafe { reader.SetStreamSelection(audio, false) };
            log!("VIDEO: SourceReader audio disabled ({err})");
            false
        }
    };
    Ok((reader, has_audio, audio_fmt))
}

fn read_yuv_color(mt: &IMFMediaType, height: u32) -> (YuvColorMatrix, bool) {
    let matrix = match unsafe { mt.GetUINT32(&MF_MT_YUV_MATRIX) } {
        Ok(v) if v == MFVideoTransferMatrix_BT601.0 as u32 => YuvColorMatrix::BT601,
        Ok(v) if v == MFVideoTransferMatrix_BT709.0 as u32 => YuvColorMatrix::BT709,
        Ok(v)
            if v == MFVideoTransferMatrix_BT2020_10.0 as u32
                || v == MFVideoTransferMatrix_BT2020_12.0 as u32 =>
        {
            YuvColorMatrix::BT2020
        }
        // Unknown / missing: HD+ content is almost always BT.709; SD often BT.601.
        _ if height >= 720 => YuvColorMatrix::BT709,
        _ => YuvColorMatrix::BT601,
    };
    // MFNominalRange_0_255 / Normal = 1 (full); 16_235 / Wide = 2 (limited).
    let full_range = match unsafe { mt.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE) } {
        Ok(v) if v == MFNominalRange_0_255.0 as u32 => true,
        Ok(v) if v == MFNominalRange_16_235.0 as u32 => false,
        _ => false,
    };
    (matrix, full_range)
}

fn prepare_session(s: &mut WorkerSession) -> Result<(), String> {
    let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mt = unsafe { s.reader.GetCurrentMediaType(video) }
        .map_err(|e| format!("GetCurrentMediaType: {e:?}"))?;
    let size = unsafe { mt.GetUINT64(&MF_MT_FRAME_SIZE) }
        .map_err(|e| format!("MF_MT_FRAME_SIZE: {e:?}"))?;
    let width = (size >> 32) as u32;
    let height = (size & 0xFFFF_FFFF) as u32;
    if width == 0 || height == 0 {
        return Err(format!("invalid frame size {width}x{height}"));
    }
    s.width = width;
    s.height = height;
    s.duration_ms = query_duration_ms(&s.reader);
    let (matrix, full_range) = read_yuv_color(&mt, height);
    // Current type may omit color attrs after NV12 conversion — try native too.
    let (matrix, full_range) = if matches!(
        unsafe { mt.GetUINT32(&MF_MT_YUV_MATRIX) },
        Err(_)
    ) {
        if let Ok(native) = unsafe { s.reader.GetNativeMediaType(video, 0) } {
            read_yuv_color(&native, height)
        } else {
            (matrix, full_range)
        }
    } else {
        (matrix, full_range)
    };
    s.color_matrix = matrix;
    s.full_range = full_range;
    if s.has_audio {
        if let Some(a) = s.audio.as_mut() {
            a.muted = s.muted;
            a.volume = s.volume as f32;
        }
    }
    s.prepared = true;
    Ok(())
}

fn destroy_session(s: &mut WorkerSession) {
    if let Some(mut a) = s.audio.take() {
        a.set_playing(false);
        a.clear();
    }
    s.pending_video = None;
    if let Some(path) = s.temp_file.take() {
        let _ = std::fs::remove_file(path);
    }
}

fn apply_seek(s: &mut WorkerSession, position_ms: u64) {
    match seek_reader(&s.reader, position_ms) {
        Ok(()) => {
            s.pending_video = None;
            s.video_eos = false;
            if let Some(a) = s.audio.as_mut() {
                a.clear();
            }
            // Do not start the master clock yet — wait for the first post-seek
            // video sample so decode latency does not look like catch-up.
            s.wall_start = None;
            let target = (position_ms as i64).saturating_mul(10_000);
            s.pts_origin_hns = target;
            s.seek_target_hns = Some(target);
            s.await_clock_anchor = true;
        }
        Err(err) => {
            static LOGGED: AtomicBool = AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                error!("VIDEO: SourceReader seek failed: {err}");
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VideoSync {
    /// Frame PTS is ahead of the master clock — hold it.
    Wait,
    /// Frame is on time — present now.
    Present,
    /// Frame is late — drop and catch up.
    Drop,
}

/// Master clock in media seconds since the last anchor.
/// Prefers audio (WASAPI media frames consumed); falls back to wall clock.
fn master_clock_secs(s: &WorkerSession) -> Option<f64> {
    if let Some(a) = s.audio.as_ref() {
        if a.started && !s.await_clock_anchor {
            return Some(a.media_clock_secs());
        }
    }
    let start = s.wall_start?;
    let rate = s.rate.max(0.05);
    Some(start.elapsed().as_secs_f64() * rate)
}

fn video_sync(s: &WorkerSession, pts_hns: i64) -> VideoSync {
    if s.await_clock_anchor {
        return VideoSync::Present;
    }
    let Some(clock) = master_clock_secs(s) else {
        return VideoSync::Present;
    };
    let media_elapsed = (pts_hns - s.pts_origin_hns) as f64 / 10_000_000.0;
    let delta = media_elapsed - clock; // >0 = video ahead of clock
    if delta > 0.035 {
        VideoSync::Wait
    } else if delta < -0.080 {
        VideoSync::Drop
    } else {
        VideoSync::Present
    }
}

/// Anchor pacing to the first decoded PTS after start/seek so decode latency
/// does not turn into a catch-up burst. Resets the audio master clock too.
fn maybe_anchor_clock(s: &mut WorkerSession, pts_hns: i64) {
    if !s.await_clock_anchor {
        return;
    }
    s.pts_origin_hns = pts_hns;
    s.wall_start = Some(Instant::now());
    if let Some(a) = s.audio.as_mut() {
        a.reset_clock();
    }
    s.await_clock_anchor = false;
}

fn present_video_sample(session: u64, s: &mut WorkerSession, sample: IMFSample, timestamp: i64) {
    maybe_anchor_clock(s, timestamp);
    let position_ms = if timestamp > 0 {
        (timestamp as u128) / 10_000
    } else {
        0
    };
    match sample_to_dxgi_nv12(sample) {
        Ok((texture, array_slice, keep_alive)) => {
            push_event(SrEvent::Frame {
                session,
                frame: D3d11Nv12Frame {
                    texture,
                    array_slice,
                    width: s.width,
                    height: s.height,
                    matrix: s.color_matrix,
                    keep_alive,
                },
                position_ms,
                full_range: s.full_range,
            });
        }
        Err(err) => {
            static LOGGED: AtomicBool = AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                error!("VIDEO: SourceReader DXGI extract failed: {err}");
            }
        }
    }
}

fn handle_video_eos(session: u64, s: &mut WorkerSession) {
    if s.is_looping {
        // Seek back to start and keep playing. Do not emit Eos — the UI clears
        // `playing` on Eos and stops posting Ticks, which freezes the loop.
        apply_seek(s, 0);
        s.want_play = true;
        s.video_eos = false;
        if let Some(a) = s.audio.as_mut() {
            a.set_playing(true);
        }
        push_event(SrEvent::Playing {
            session,
            playing: true,
        });
    } else {
        s.video_eos = true;
        s.want_play = false;
        if let Some(a) = s.audio.as_mut() {
            a.set_playing(false);
        }
        push_event(SrEvent::Eos { session });
        push_event(SrEvent::Playing {
            session,
            playing: false,
        });
    }
}

fn drain_audio_samples(s: &mut WorkerSession) {
    if s.audio.is_none() {
        return;
    }
    let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    let drop_before = s.seek_target_hns.unwrap_or(s.pts_origin_hns);
    for _ in 0..8 {
        let pending_len = s.audio.as_ref().map(|a| a.pending.len()).unwrap_or(0);
        let channels = s.audio.as_ref().map(|a| a.channels).unwrap_or(2);
        if pending_len > channels.saturating_mul(48_000 / 5) {
            break;
        }
        let mut stream_flags = 0u32;
        let mut timestamp = 0i64;
        let mut sample: Option<IMFSample> = None;
        if unsafe {
            s.reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut stream_flags),
                Some(&mut timestamp),
                Some(&mut sample),
            )
        }
        .is_err()
        {
            break;
        }
        if (stream_flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            break;
        }
        if (stream_flags & MF_SOURCE_READERF_STREAMTICK.0 as u32) != 0 || sample.is_none() {
            continue;
        }
        // Accurate seek / post-seek: drop PCM that still sits before the target.
        if timestamp < drop_before {
            continue;
        }
        if let Ok(pcm) = sample_pcm_f32(sample.as_ref().unwrap()) {
            if let Some(audio) = s.audio.as_mut() {
                audio.push_pcm(&pcm);
            }
        }
    }
}

fn disable_audio_stream(s: &mut WorkerSession) {
    let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    let _ = unsafe { s.reader.SetStreamSelection(audio, false) };
    let all = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
    let _ = unsafe { s.reader.Flush(all) };
    if let Some(mut a) = s.audio.take() {
        a.set_playing(false);
        a.clear();
    }
    s.has_audio = false;
}

enum ReadVideo {
    Empty,
    Eos,
    Sample(IMFSample, i64),
}

fn read_video_sample(s: &mut WorkerSession) -> Result<ReadVideo, String> {
    let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut stream_flags = 0u32;
    let mut timestamp = 0i64;
    let mut sample: Option<IMFSample> = None;
    unsafe {
        s.reader.ReadSample(
            video,
            0,
            None,
            Some(&mut stream_flags),
            Some(&mut timestamp),
            Some(&mut sample),
        )
    }
    .map_err(|e| format!("ReadSample: {e:?}"))?;

    if (stream_flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
        return Ok(ReadVideo::Eos);
    }
    if (stream_flags & MF_SOURCE_READERF_STREAMTICK.0 as u32) != 0 || sample.is_none() {
        return Ok(ReadVideo::Empty);
    }
    Ok(ReadVideo::Sample(sample.unwrap(), timestamp))
}

/// Decode until we hold a video sample, or fail. Used before advertising Prepared
/// so DXGI-incompatible containers (e.g. many WMV/VC-1 files) fall back cleanly.
fn try_prime_video(s: &mut WorkerSession) -> Result<bool, String> {
    if s.pending_video.is_some() {
        return Ok(true);
    }
    match read_video_sample(s) {
        Ok(ReadVideo::Sample(sample, ts)) => {
            s.pending_video = Some((sample, ts));
            Ok(true)
        }
        Ok(ReadVideo::Empty) => Ok(false),
        Ok(ReadVideo::Eos) => Err("SourceReader EOS before first frame".into()),
        Err(err) => {
            // Audio type negotiation can poison some demuxers (classic WMV/ASF).
            if s.has_audio {
                log!("VIDEO: SourceReader prime ReadSample failed ({err}); retrying video-only");
                disable_audio_stream(s);
                return match read_video_sample(s) {
                    Ok(ReadVideo::Sample(sample, ts)) => {
                        s.pending_video = Some((sample, ts));
                        Ok(true)
                    }
                    Ok(ReadVideo::Empty) => Ok(false),
                    Ok(ReadVideo::Eos) => Err("SourceReader EOS before first frame".into()),
                    Err(err2) => Err(err2),
                };
            }
            Err(err)
        }
    }
}

fn tick_session(session: u64, s: &mut WorkerSession) {
    if s.fatal || !s.prepared {
        return;
    }

    if !s.prepare_sent {
        match try_prime_video(s) {
            Ok(true) => {
                s.prepare_sent = true;
                push_event(SrEvent::Prepared {
                    session,
                    width: s.width,
                    height: s.height,
                    duration_ms: s.duration_ms,
                    has_audio: s.has_audio,
                    matrix: s.color_matrix,
                    full_range: s.full_range,
                });
                if s.want_play {
                    push_event(SrEvent::Playing {
                        session,
                        playing: true,
                    });
                }
                static LOGGED: AtomicBool = AtomicBool::new(false);
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    let mode = if s.has_audio { "A/V" } else { "video-only" };
                    log!(
                        "VIDEO: MF SourceReader DXGI NV12 ready ({}x{}, {mode}, duration={}ms)",
                        s.width,
                        s.height,
                        s.duration_ms
                    );
                }
            }
            Ok(false) => return,
            Err(err) => {
                s.fatal = true;
                s.want_play = false;
                push_event(SrEvent::CreateFailed {
                    session,
                    error: err,
                });
                return;
            }
        }
    }

    if !s.want_play || s.video_eos {
        return;
    }

    // Hold audio until the video clock is anchored, otherwise PCM fills the
    // WASAPI buffer during seek decode latency and plays ahead of the picture.
    if !s.await_clock_anchor {
        drain_audio_samples(s);
        if let Some(a) = s.audio.as_mut() {
            a.write_available(s.rate);
        }
    }

    // Present at most one frame per tick; drop late frames to catch the audio clock.
    // Accurate seek: discard keyframe→target frames until PTS >= seek_target.
    for _ in 0..8 {
        if s.pending_video.is_none() {
            match read_video_sample(s) {
                Ok(ReadVideo::Empty) => return,
                Ok(ReadVideo::Eos) => {
                    handle_video_eos(session, s);
                    return;
                }
                Ok(ReadVideo::Sample(sample, timestamp)) => {
                    s.pending_video = Some((sample, timestamp));
                }
                Err(err) => {
                    s.fatal = true;
                    s.want_play = false;
                    push_event(SrEvent::Error {
                        session,
                        error: err,
                    });
                    return;
                }
            }
        }

        let pts = match s.pending_video.as_ref() {
            Some((_, pts)) => *pts,
            None => return,
        };
        if let Some(target) = s.seek_target_hns {
            if pts < target {
                s.pending_video = None;
                // Keep demuxer audio from piling up behind the discarded video.
                drain_audio_samples(s);
                if let Some(a) = s.audio.as_mut() {
                    a.clear();
                }
                continue;
            }
            s.seek_target_hns = None;
        }
        match video_sync(s, pts) {
            VideoSync::Wait => return,
            VideoSync::Drop => {
                s.pending_video = None;
                continue;
            }
            VideoSync::Present => {
                let (sample, pts) = s.pending_video.take().unwrap();
                present_video_sample(session, s, sample, pts);
                return;
            }
        }
    }
}

// ── UI-side player ───────────────────────────────────────────────────────────

pub struct WindowsMfSourceReaderPlayer {
    session: u64,
    #[allow(dead_code)]
    pub(crate) video_id: LiveId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    d3d11_device: ID3D11Device,
    nv12_present: crate::gpu_texture::D3d11Nv12PresentCache,
    gpu_frame_keep_alive: Option<Arc<dyn std::any::Any + Send + Sync>>,
    prepare_notified: bool,
    prepare_result: Option<Result<PlaybackPrepared, String>>,
    /// Fatal decode error after a successful prepare — unified player falls back.
    runtime_error: Option<String>,
    pending_eos: bool,
    eos_notified: bool,
    pending_frame: Option<(D3d11Nv12Frame, u128)>,
    position_ms: u128,
    playing: AtomicBool,
    preparing: AtomicBool,
    alive: bool,
    zero_copy: bool,
    yuv_matrix: f32,
    yuv_full_range: bool,
}

impl WindowsMfSourceReaderPlayer {
    pub fn try_new(
        d3d11_device: &ID3D11Device,
        video_id: LiveId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Option<Self> {
        if std::env::var_os("MAKEPAD_MF_ENGINE").is_some() {
            return None;
        }
        if source.is_adaptive_manifest() || source.is_network_stream() {
            return None;
        }
        // WMV/ASF (VC-1 / WMA) rarely produce DXGI NV12 via Source Reader; the
        // D3D allocator fails at ReadSample. Prefer MediaEngine for these.
        if source_prefers_media_engine(&source) {
            log!("VIDEO: skipping SourceReader for WMV/ASF; using MediaEngine");
            return None;
        }

        let create_source = match &source {
            VideoSource::Network(url) => CreateSource::Url(url.clone()),
            VideoSource::Filesystem(path) => CreateSource::Url(path_to_file_url(path)),
            VideoSource::InMemory(data) => CreateSource::Memory {
                bytes: data.as_ref().clone(),
                video_id: video_id.0,
            },
            VideoSource::Camera(..)
            | VideoSource::PlaybackSession(..)
            | VideoSource::Session(..) => return None,
        };

        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        post(SrCmd::Create {
            session,
            device: d3d11_device.clone(),
            source: create_source,
            is_looping,
            autoplay,
        });

        Some(Self {
            session,
            video_id,
            tex_y_id,
            tex_u_id,
            d3d11_device: d3d11_device.clone(),
            nv12_present: Default::default(),
            gpu_frame_keep_alive: None,
            prepare_notified: false,
            prepare_result: None,
            runtime_error: None,
            pending_eos: false,
            eos_notified: false,
            pending_frame: None,
            position_ms: 0,
            playing: AtomicBool::new(autoplay),
            preparing: AtomicBool::new(true),
            alive: true,
            zero_copy: false,
            yuv_matrix: YuvColorMatrix::BT709.as_f32(),
            yuv_full_range: false,
        })
    }

    fn drain_worker_events(&mut self) {
        for ev in drain_events_for(self.session) {
            match ev {
                SrEvent::CreateFailed { error, .. } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    if !self.prepare_notified {
                        self.prepare_result = Some(Err(error));
                    }
                }
                SrEvent::Prepared {
                    width,
                    height,
                    duration_ms,
                    has_audio,
                    matrix,
                    full_range,
                    ..
                } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    self.yuv_matrix = matrix.as_f32();
                    self.yuv_full_range = full_range;
                    if !self.prepare_notified {
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
                            duration_ms > 0,
                            video_tracks,
                            audio_tracks,
                        )));
                    }
                }
                SrEvent::Error { error, .. } => {
                    self.preparing.store(false, Ordering::Relaxed);
                    self.playing.store(false, Ordering::Relaxed);
                    // Prepared + first-frame DXGI failure can arrive in one drain;
                    // never overwrite an Ok prepare with a later frame Error.
                    if !self.prepare_notified && self.prepare_result.is_none() {
                        self.prepare_result = Some(Err(error));
                    } else if self.prepare_notified || matches!(self.prepare_result, Some(Ok(_))) {
                        self.runtime_error = Some(error);
                    } else {
                        error!("VIDEO: {}", error);
                    }
                }
                SrEvent::Eos { .. } => {
                    self.pending_eos = true;
                    self.playing.store(false, Ordering::Relaxed);
                }
                SrEvent::Playing { playing, .. } => {
                    self.playing.store(playing, Ordering::Relaxed);
                }
                SrEvent::Frame {
                    frame,
                    position_ms,
                    full_range,
                    ..
                } => {
                    self.position_ms = position_ms;
                    self.yuv_matrix = frame.matrix.as_f32();
                    self.yuv_full_range = full_range;
                    self.pending_frame = Some((frame, position_ms));
                }
            }
        }
    }

    /// Pump the MTA worker once and drain events. Call at most once per UI paint.
    pub fn sync_worker(&mut self) {
        post(SrCmd::Tick(self.session));
        self.drain_worker_events();
    }

    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        if self.prepare_notified {
            return None;
        }
        if let Some(result) = self.prepare_result.take() {
            self.prepare_notified = true;
            return Some(result);
        }
        None
    }

    /// Mid-playback fatal error after prepare succeeded (for MediaEngine fallback).
    pub fn take_runtime_error(&mut self) -> Option<String> {
        self.runtime_error.take()
    }

    pub fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        let Some((frame, position_ms)) = self.pending_frame.take() else {
            return false;
        };
        self.position_ms = position_ms;
        match crate::gpu_texture::adopt_d3d11_nv12_biplanar(
            &self.d3d11_device,
            textures,
            self.tex_y_id,
            self.tex_u_id,
            &frame,
            &mut self.nv12_present,
        ) {
            Ok(zero_copy) => {
                self.zero_copy = zero_copy;
                if zero_copy {
                    self.gpu_frame_keep_alive = Some(frame.keep_alive.clone());
                } else {
                    self.gpu_frame_keep_alive = None;
                }
                static LOGGED: AtomicBool = AtomicBool::new(false);
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    if zero_copy {
                        log!(
                            "VIDEO: MF SourceReader DXGI NV12 true zero-copy (IMFDXGIBuffer)"
                        );
                    } else {
                        log!(
                            "VIDEO: MF SourceReader DXGI NV12 present via GPU blit fallback"
                        );
                    }
                }
                true
            }
            Err(err) => {
                error!("VIDEO: SourceReader adopt failed: {err}");
                self.gpu_frame_keep_alive = None;
                false
            }
        }
    }

    pub fn presents_nv12(&self) -> bool {
        true
    }

    pub fn yuv_array(&self) -> bool {
        self.zero_copy
    }

    pub fn yuv_matrix(&self) -> f32 {
        self.yuv_matrix
    }

    pub fn yuv_full_range(&self) -> bool {
        self.yuv_full_range
    }

    pub fn check_eos(&mut self) -> bool {
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
        post(SrCmd::Play(self.session));
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn pause(&mut self) {
        self.playing.store(false, Ordering::Relaxed);
        post(SrCmd::Pause(self.session));
    }

    pub fn resume(&mut self) {
        self.play();
    }

    pub fn mute(&mut self) {
        post(SrCmd::Mute {
            session: self.session,
            muted: true,
        });
    }

    pub fn unmute(&mut self) {
        post(SrCmd::Mute {
            session: self.session,
            muted: false,
        });
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        self.eos_notified = false;
        self.pending_eos = false;
        post(SrCmd::Seek {
            session: self.session,
            position_ms,
        });
    }

    pub fn set_volume(&mut self, volume: f64) {
        post(SrCmd::SetVolume {
            session: self.session,
            volume,
        });
    }

    pub fn set_playback_rate(&mut self, rate: f64) {
        post(SrCmd::SetPlaybackRate {
            session: self.session,
            rate,
        });
    }

    pub fn current_position_ms(&self) -> u128 {
        self.position_ms
    }

    pub fn keep_polling(&self) -> bool {
        self.preparing.load(Ordering::Relaxed) || self.playing.load(Ordering::Relaxed)
    }

    pub fn cleanup(&mut self) {
        if self.alive {
            post(SrCmd::Destroy(self.session));
            self.alive = false;
            self.playing.store(false, Ordering::Relaxed);
            self.preparing.store(false, Ordering::Relaxed);
            self.gpu_frame_keep_alive = None;
        }
    }
}

impl Drop for WindowsMfSourceReaderPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
