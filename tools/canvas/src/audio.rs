use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::spectrum::{SpectrumAnalyzer, NUM_BANDS};

// ============================================================================
// Shared audio state singleton
// ============================================================================

static AUDIO_STATE: OnceLock<Arc<AudioPlaybackState>> = OnceLock::new();

pub fn get_audio_state() -> Arc<AudioPlaybackState> {
    AUDIO_STATE.get_or_init(|| Arc::new(AudioPlaybackState::new())).clone()
}

// ============================================================================
// Atomic f64 helper
// ============================================================================

pub struct F64Atomic(AtomicU64);

impl F64Atomic {
    pub fn new(val: f64) -> Self {
        F64Atomic(AtomicU64::new(val.to_bits()))
    }
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn set(&self, val: f64) {
        self.0.store(val.to_bits(), Ordering::Relaxed);
    }
}

impl Default for F64Atomic {
    fn default() -> Self {
        F64Atomic::new(0.0)
    }
}

// ============================================================================
// Shared playback state
// ============================================================================

pub struct AudioPlaybackState {
    pub is_playing: AtomicBool,
    pub amplitude: F64Atomic,
    pub position_secs: F64Atomic,
    pub duration_secs: F64Atomic,
    pub samples: Mutex<Vec<f32>>,
    pub sample_rate: F64Atomic,
    pub play_cursor: AtomicUsize,
    pub channel_count: AtomicUsize,
    /// Spectrum bands shared from audio thread to UI
    pub spectrum: Mutex<[f32; NUM_BANDS]>,
}

impl AudioPlaybackState {
    pub fn new() -> Self {
        Self {
            is_playing: AtomicBool::new(false),
            amplitude: F64Atomic::new(0.0),
            position_secs: F64Atomic::new(0.0),
            duration_secs: F64Atomic::new(0.0),
            samples: Mutex::new(Vec::new()),
            sample_rate: F64Atomic::new(44100.0),
            play_cursor: AtomicUsize::new(0),
            channel_count: AtomicUsize::new(2),
            spectrum: Mutex::new([0.0; NUM_BANDS]),
        }
    }

    pub fn load_samples(&self, pcm: Vec<f32>, sample_rate: u32, channels: usize) {
        let total_frames = pcm.len() / channels;
        let duration = total_frames as f64 / sample_rate as f64;
        self.sample_rate.set(sample_rate as f64);
        self.channel_count.store(channels, Ordering::Relaxed);
        self.duration_secs.set(duration);
        self.play_cursor.store(0, Ordering::Relaxed);
        self.position_secs.set(0.0);
        self.amplitude.set(0.0);
        if let Ok(mut buf) = self.samples.lock() {
            *buf = pcm;
        }
    }

    pub fn play(&self) {
        self.play_cursor.store(0, Ordering::Relaxed);
        self.position_secs.set(0.0);
        self.is_playing.store(true, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.is_playing.store(false, Ordering::Relaxed);
        self.amplitude.set(0.0);
    }

    pub fn toggle(&self) {
        if self.is_playing.load(Ordering::Relaxed) {
            self.stop();
        } else {
            self.is_playing.store(true, Ordering::Relaxed);
        }
    }
}

// ============================================================================
// Decode MP3/AAC from bytes (Cursor<Vec<u8>>)
// ============================================================================

pub fn decode_audio_bytes(data: Vec<u8>) -> Result<(Vec<f32>, u32, usize), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let cursor = std::io::Cursor::new(data);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let mut hint = Hint::new();
    hint.with_extension("mp3");

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Probe failed: {}", e))?;

    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or("No audio track found")?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.ok_or("No sample rate")?;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(2);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Decoder creation failed: {}", e))?;

    let estimated = track
        .codec_params
        .n_frames
        .map(|n| (n as usize) * channels)
        .unwrap_or(44100 * channels * 60);
    let mut all_samples: Vec<f32> = Vec::with_capacity(estimated);
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let spec = *decoded.spec();
        let num_frames = decoded.frames();
        let buf = match &mut sample_buf {
            Some(buf) if buf.capacity() >= num_frames => buf,
            _ => {
                sample_buf = Some(SampleBuffer::<f32>::new(num_frames as u64, spec));
                sample_buf.as_mut().unwrap()
            }
        };
        buf.copy_interleaved_ref(decoded);
        all_samples.extend_from_slice(&buf.samples()[..num_frames * spec.channels.count()]);
    }

    Ok((all_samples, sample_rate, channels))
}

// ============================================================================
// Audio output callback + FFT spectrum
// ============================================================================

const FFT_SIZE: usize = 2048;

pub fn start_audio_output(cx: &mut Cx, state: Arc<AudioPlaybackState>, signal: SignalToUI) {
    let mut spectrum_analyzer = SpectrumAnalyzer::new(FFT_SIZE);
    let mut fft_counter: usize = 0;
    let mut update_counter: u32 = 0;

    cx.audio_output(0, move |_info, output_buffer| {
        output_buffer.zero();

        if !state.is_playing.load(Ordering::Relaxed) {
            return;
        }

        let src_rate = state.sample_rate.get();
        let src_channels = state.channel_count.load(Ordering::Relaxed);
        if src_rate <= 0.0 || src_channels == 0 {
            return;
        }

        let frame_count = output_buffer.frame_count();
        let out_channels = output_buffer.channel_count();
        let out_rate = _info.sample_rate;
        let ratio = if (src_rate - out_rate).abs() > 1.0 {
            src_rate / out_rate
        } else {
            1.0
        };

        let samples_guard = match state.samples.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if samples_guard.is_empty() {
            return;
        }

        let total_src_frames = samples_guard.len() / src_channels;
        let cursor = state.play_cursor.load(Ordering::Relaxed);

        let mut sum_sq: f64 = 0.0;
        let mut sample_count: usize = 0;

        for frame_idx in 0..frame_count {
            let src_pos = cursor as f64 + frame_idx as f64 * ratio;
            let src_frame = src_pos as usize;
            if src_frame >= total_src_frames {
                state.is_playing.store(false, Ordering::Relaxed);
                state.amplitude.set(0.0);
                signal.set();
                break;
            }

            let frac = (src_pos - src_frame as f64) as f32;
            let next_frame = (src_frame + 1).min(total_src_frames - 1);

            for out_ch in 0..out_channels {
                let src_ch = out_ch % src_channels;
                let idx0 = src_frame * src_channels + src_ch;
                let idx1 = next_frame * src_channels + src_ch;
                let s0 = samples_guard[idx0];
                let s1 = samples_guard[idx1];
                let sample = s0 + (s1 - s0) * frac;
                let scaled = sample.clamp(-1.0, 1.0);
                output_buffer.channel_mut(out_ch)[frame_idx] = scaled;

                if out_ch == 0 {
                    sum_sq += (scaled as f64) * (scaled as f64);
                    sample_count += 1;
                }
            }
        }

        // Advance cursor
        let frames_consumed = (frame_count as f64 * ratio) as usize;
        let new_cursor = cursor + frames_consumed;
        state.play_cursor.store(new_cursor, Ordering::Relaxed);
        state.position_secs.set(new_cursor as f64 / src_rate);

        // RMS amplitude
        if sample_count > 0 {
            let rms = (sum_sq / sample_count as f64).sqrt();
            state.amplitude.set(rms);
        }

        // FFT every ~4 callbacks (~15Hz at 44100/512)
        fft_counter += 1;
        if fft_counter >= 4 {
            fft_counter = 0;
            // Grab samples around current position for FFT
            let fft_start = if new_cursor >= FFT_SIZE { new_cursor - FFT_SIZE } else { 0 };
            let available = total_src_frames.saturating_sub(fft_start);
            if available >= FFT_SIZE {
                let slice_start = fft_start * src_channels;
                let slice_end = (fft_start + FFT_SIZE) * src_channels;
                if slice_end <= samples_guard.len() {
                    spectrum_analyzer.analyze(&samples_guard[slice_start..slice_end], src_channels);
                    if let Ok(mut bands) = state.spectrum.try_lock() {
                        *bands = spectrum_analyzer.bands;
                    }
                }
            }
        }

        // Signal UI ~10Hz
        update_counter += 1;
        if update_counter >= 5 {
            update_counter = 0;
            signal.set();
        }
    });
}

// ============================================================================
// Download MP3 from URL in background thread
// ============================================================================

/// In-memory cache of decoded audio: URL -> (samples, sample_rate, channels)
static AUDIO_CACHE: Mutex<Option<std::collections::HashMap<String, (Vec<f32>, u32, usize)>>> = Mutex::new(None);

const DISK_CACHE_DIR: &str = "/tmp/canvas_audio_cache";

/// Get a disk cache path for a URL (hash-based filename)
fn disk_cache_path(url: &str) -> std::path::PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    std::path::PathBuf::from(DISK_CACHE_DIR).join(format!("{:x}.mp3", hash))
}

pub fn download_and_decode(
    url: String,
    state: Arc<AudioPlaybackState>,
    signal: SignalToUI,
) {
    // Check in-memory cache first
    {
        let cache_guard = AUDIO_CACHE.lock().unwrap();
        if let Some(cache) = cache_guard.as_ref() {
            if let Some((samples, rate, channels)) = cache.get(&url) {
                eprintln!("[AUDIO] Memory cache hit for: {}", url);
                state.load_samples(samples.clone(), *rate, *channels);
                state.play();
                signal.set();
                return;
            }
        }
    }

    std::thread::spawn(move || {
        // Check disk cache
        let cache_path = disk_cache_path(&url);
        let data = if cache_path.exists() {
            eprintln!("[AUDIO] Disk cache hit: {}", cache_path.display());
            match std::fs::read(&cache_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[AUDIO] Disk cache read error: {}", e);
                    return;
                }
            }
        } else {
            eprintln!("[AUDIO] Downloading: {}", url);
            match download_url(&url) {
                Ok(d) => {
                    eprintln!("[AUDIO] Downloaded {} bytes", d.len());
                    // Save to disk cache
                    let _ = std::fs::create_dir_all(DISK_CACHE_DIR);
                    if let Err(e) = std::fs::write(&cache_path, &d) {
                        eprintln!("[AUDIO] Disk cache write error: {}", e);
                    } else {
                        eprintln!("[AUDIO] Saved to disk cache: {}", cache_path.display());
                    }
                    d
                }
                Err(e) => {
                    eprintln!("[AUDIO] Download error: {}", e);
                    return;
                }
            }
        };

        eprintln!("[AUDIO] Decoding {} bytes...", data.len());
        match decode_audio_bytes(data) {
            Ok((samples, rate, channels)) => {
                eprintln!("[AUDIO] Decoded: {} samples, {}Hz, {} ch", samples.len(), rate, channels);
                // Memory cache
                {
                    let mut cache_guard = AUDIO_CACHE.lock().unwrap();
                    let cache = cache_guard.get_or_insert_with(std::collections::HashMap::new);
                    cache.insert(url.clone(), (samples.clone(), rate, channels));
                }
                state.load_samples(samples, rate, channels);
                state.play();
                signal.set();
                eprintln!("[AUDIO] Playback started");
            }
            Err(e) => eprintln!("[AUDIO] Decode error: {}", e),
        }
    });
}

/// Minimal blocking HTTP GET using std::net::TcpStream + basic HTTP/1.1
fn download_url(url: &str) -> Result<Vec<u8>, String> {
    // Parse URL
    let url = if url.starts_with("http://") {
        url.to_string()
    } else if url.starts_with("https://") {
        // For HTTPS, we'll use a simple approach: shell out to curl
        // This is pragmatic for a demo app
        return download_via_command(url);
    } else {
        return Err(format!("Unsupported URL scheme: {}", url));
    };

    // For HTTP URLs, also use curl for simplicity
    download_via_command(&url)
}

fn download_via_command(url: &str) -> Result<Vec<u8>, String> {
    use std::process::Command;
    let output = Command::new("curl")
        .args(["-sL", "--max-time", "120", "--connect-timeout", "10", url])
        .output()
        .map_err(|e| format!("curl failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("curl returned status: {}", output.status));
    }
    Ok(output.stdout)
}
