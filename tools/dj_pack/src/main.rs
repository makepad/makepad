use makepad_ai_hub::client::{ContentProvider, LocalService};
use makepad_ai_hub::protocol::{
    decode_stems_artifact, GenerateRequestJson, JOB_STATE_CANCELLED, JOB_STATE_DONE,
    JOB_STATE_ERROR, STEMS_ARTIFACT_CONTENT_TYPE,
};
use makepad_ai_hub::registry::Domain;
use makepad_ai_stems::{CacheHeader, StemCache, StemSet, StereoBuf, CHUNK_STEP, SAMPLE_RATE};
use makepad_asset_data::{
    AssetAlias, AssetFile, AssetId, AssetKind, AssetManifest, Axis, BlobId, Bounds,
    Capabilities, CoordinateSystem, DerivativePolicy, DeviceTier, FileRole, MediaType, Metrics,
    Pivot, Redistribution, Rights, Vec3,
};
use makepad_asset_store::{
    export_static, AssetAnnotation, AssetServerCore, Budgets, PublishBatchItem,
    StaticExportOptions, Visibility,
};
use makepad_audio_decode::{decode_any, read_tags, DecodedAudio};
use makepad_audio_sidechannels::{encode_stem_oggs, side_channel_files_with_analysis};
use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, SerJson};
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STEM_OGG_NAMES: [&str; 4] = ["drums.ogg", "bass.ogg", "vocals.ogg", "other.ogg"];
const DJ_ANALYSIS_NAME: &str = "analysis.wave";
const DJ_LOOP_SPLAT_NAME: &str = "loop-splat.bin";
const MAX_STEMS_JOB_FRAMES: usize = SAMPLE_RATE as usize * 60 * 20;

fn usage() -> &'static str {
    "makepad-dj-pack — bake web-DJ tracks through the AI hub and export a static store\n\
\n\
USAGE:\n\
  makepad-dj-pack stems <audio>... --hub <URL|auto> --out <stem-cache-dir>\n\
  makepad-dj-pack analyse <audio>... [--wave-cache <dir>] [--loop-cache <dir>]\n\
  makepad-dj-pack pack --store <root> --site-out <dir> [--stem-cache <dir>]\n\
                       [--lyrics-cache <dir>] [--wave-cache <dir>]\n\
                       [--loop-cache <dir>] [--require-stems] [--dry-run] <audio>...\n\
\n\
stems decodes locally with makepad-audio-decode, sends 44.1 kHz stereo PCM\n\
to the hub's stems capability, and writes VJ StemCache + four Ogg files.\n\
analyse runs the VJ's native beat/wave and loop-splat builders locally and\n\
writes digest-keyed caches; it does not contact the AI hub. stems never loads\n\
or runs a separation model. --hub auto listens for fleet beacons.\n\
pack only reads caches, publishes the original audio plus available side\n\
channels, then exports\n\
/v1/health, catalogs, aliases, revisions and /v1/blobs for StaticStore."
}

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("makepad-dj-pack: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        println!("{}", usage());
        return Ok(());
    };
    if matches!(command, "-h" | "--help" | "help") {
        println!("{}", usage());
        return Ok(());
    }
    match command {
        "stems" => run_stems(parse_stems(&args[1..])?),
        "analyse" => run_analyse(parse_analyse(&args[1..])?),
        "pack" => run_pack(parse_pack(&args[1..])?),
        other => Err(format!("unknown command {other:?}\n\n{}", usage())),
    }
}

struct StemsArgs {
    hub: String,
    out: PathBuf,
    audio: Vec<PathBuf>,
}

fn parse_stems(args: &[String]) -> Result<StemsArgs, String> {
    let mut hub = None;
    let mut out = None;
    let mut audio = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--hub" => {
                index += 1;
                hub = Some(args.get(index).ok_or("--hub needs URL or auto")?.clone());
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(args.get(index).ok_or("--out needs a directory")?));
            }
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            value if value.starts_with('-') => return Err(format!("unknown stems option {value}")),
            value => audio.push(PathBuf::from(value)),
        }
        index += 1;
    }
    if audio.is_empty() {
        return Err("stems needs at least one audio file".to_string());
    }
    Ok(StemsArgs {
        hub: hub.ok_or("stems requires --hub <URL|auto>")?,
        out: out.ok_or("stems requires --out <stem-cache-dir>")?,
        audio,
    })
}

struct PackArgs {
    store: PathBuf,
    site_out: PathBuf,
    stem_cache: Option<PathBuf>,
    lyrics_cache: Option<PathBuf>,
    wave_cache: PathBuf,
    loop_cache: PathBuf,
    require_stems: bool,
    dry_run: bool,
    audio: Vec<PathBuf>,
}

struct AnalyseArgs {
    wave_cache: PathBuf,
    loop_cache: PathBuf,
    audio: Vec<PathBuf>,
}

fn default_loop_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("VJ_LOOP_CACHE") {
        return PathBuf::from(dir);
    }
    makepad_vj_analysis::wave_analysis::cache_dir()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("loop-cache")
}

fn parse_analyse(args: &[String]) -> Result<AnalyseArgs, String> {
    let mut wave_cache = None;
    let mut loop_cache = None;
    let mut audio = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--wave-cache" => {
                index += 1;
                wave_cache = Some(PathBuf::from(
                    args.get(index).ok_or("--wave-cache needs a directory")?,
                ));
            }
            "--loop-cache" => {
                index += 1;
                loop_cache = Some(PathBuf::from(
                    args.get(index).ok_or("--loop-cache needs a directory")?,
                ));
            }
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            value if value.starts_with('-') => return Err(format!("unknown analyse option {value}")),
            value => audio.push(PathBuf::from(value)),
        }
        index += 1;
    }
    if audio.is_empty() {
        return Err("analyse needs at least one audio file".to_string());
    }
    Ok(AnalyseArgs {
        wave_cache: wave_cache
            .unwrap_or_else(makepad_vj_analysis::wave_analysis::cache_dir),
        loop_cache: loop_cache.unwrap_or_else(default_loop_cache),
        audio,
    })
}

fn parse_pack(args: &[String]) -> Result<PackArgs, String> {
    let mut store = None;
    let mut site_out = None;
    let mut stem_cache = None;
    let mut lyrics_cache = None;
    let mut wave_cache = None;
    let mut loop_cache = None;
    let mut require_stems = false;
    let mut dry_run = false;
    let mut audio = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--store" => {
                index += 1;
                store = Some(PathBuf::from(args.get(index).ok_or("--store needs a directory")?));
            }
            "--site-out" => {
                index += 1;
                site_out = Some(PathBuf::from(args.get(index).ok_or("--site-out needs a directory")?));
            }
            "--stem-cache" => {
                index += 1;
                stem_cache = Some(PathBuf::from(
                    args.get(index).ok_or("--stem-cache needs a directory")?,
                ));
            }
            "--lyrics-cache" => {
                index += 1;
                lyrics_cache = Some(PathBuf::from(
                    args.get(index).ok_or("--lyrics-cache needs a directory")?,
                ));
            }
            "--wave-cache" => {
                index += 1;
                wave_cache = Some(PathBuf::from(
                    args.get(index).ok_or("--wave-cache needs a directory")?,
                ));
            }
            "--loop-cache" => {
                index += 1;
                loop_cache = Some(PathBuf::from(
                    args.get(index).ok_or("--loop-cache needs a directory")?,
                ));
            }
            "--require-stems" => require_stems = true,
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            value if value.starts_with('-') => return Err(format!("unknown pack option {value}")),
            value => audio.push(PathBuf::from(value)),
        }
        index += 1;
    }
    if audio.is_empty() {
        return Err("pack needs at least one audio file".to_string());
    }
    Ok(PackArgs {
        store: store.ok_or("pack requires --store <root>")?,
        site_out: site_out.ok_or("pack requires --site-out <dir>")?,
        stem_cache,
        lyrics_cache,
        wave_cache: wave_cache
            .unwrap_or_else(makepad_vj_analysis::wave_analysis::cache_dir),
        loop_cache: loop_cache.unwrap_or_else(default_loop_cache),
        require_stems,
        dry_run,
        audio,
    })
}

#[derive(Clone)]
struct PreparedAudio {
    path: PathBuf,
    bytes: Vec<u8>,
    media: MediaType,
    frames: Vec<[i16; 2]>,
    sample_rate: u32,
    digest: String,
    title: String,
    artist: String,
    embedded_attribution: TrackAttribution,
}

#[derive(Clone, Debug, Default, DeJson)]
struct TrackAttribution {
    title: String,
    artist: String,
    artist_url: String,
    album: String,
    source_url: String,
    license: String,
    license_url: String,
}

fn prepare_audio(path: &Path) -> Result<PreparedAudio, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let decoded = decode_any(&bytes).map_err(|error| format!("{}: decode: {error}", path.display()))?;
    let media = match makepad_audio_decode::sniff(&bytes) {
        Some(makepad_audio_decode::AudioFormat::Mp3) => MediaType::Mp3,
        Some(makepad_audio_decode::AudioFormat::OggVorbis) => MediaType::Ogg,
        Some(makepad_audio_decode::AudioFormat::Flac) => MediaType::Bin,
        None => return Err(format!("{}: unsupported audio container", path.display())),
    };
    let frames = deck_frames(&decoded)?;
    let sample_rate = decoded.rate.max(1);
    let digest = makepad_audio_lyrics::track_digest(sample_rate, &frames);
    let tags = read_tags(&bytes).unwrap_or_default();
    let fallback = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("untitled")
        .to_string();
    let title = clean_text(tags.title.as_deref().unwrap_or(&fallback), 512);
    let artist = clean_text(tags.artist.as_deref().unwrap_or(&fallback), 512);
    let tag = |names: &[&str]| {
        names.iter().find_map(|name| tags.get(name)).unwrap_or("").to_string()
    };
    let tagged_source = tag(&["SOURCE_URL", "SOURCE", "WOAS"]);
    let embedded_attribution = TrackAttribution {
        title: title.clone(),
        artist: artist.clone(),
        artist_url: tag(&["ARTIST_URL", "ARTISTURL", "WOAR"]),
        album: tags.album.clone().unwrap_or_default(),
        source_url: if tagged_source.is_empty() {
            clean_text(
                id3_comment(&bytes).as_deref().unwrap_or(""),
                makepad_asset_client::wire::MAX_SNIPPET_BYTES,
            )
        } else {
            tagged_source
        },
        license: tag(&["LICENSE", "LICENCE"]),
        license_url: tag(&["LICENSE_URL", "LICENSEURL", "LICENCE_URL", "WCOP"]),
    };
    Ok(PreparedAudio {
        path: path.to_path_buf(), bytes, media, frames, sample_rate, digest, title, artist,
        embedded_attribution,
    })
}

fn deck_frames(decoded: &DecodedAudio) -> Result<Vec<[i16; 2]>, String> {
    if decoded.rate == 0 || decoded.channels == 0 || decoded.frames() == 0 {
        return Err("audio decoded to zero frames".to_string());
    }
    let channels = decoded.channels as usize;
    Ok(decoded
        .pcm_interleaved_f32
        .chunks_exact(channels)
        .map(|frame| {
            let sample = |value: f32| (value.clamp(-1.0, 1.0) * 32767.0) as i16;
            [sample(frame[0]), sample(frame[channels - 1])]
        })
        .collect())
}

const RESAMPLE_PHASES: usize = 256;
const RESAMPLE_TAPS: usize = 16;

/// The VJ importer's windowed-sinc resampler. Keeping this byte-for-byte in
/// step with its cache input path makes the packer's cache interchangeable.
struct ResampleKernel {
    taps: Vec<f32>,
    width: usize,
    span: usize,
}

impl ResampleKernel {
    fn new(ratio: f64) -> Self {
        let cutoff = 0.5 * ratio.min(1.0);
        let width = ((RESAMPLE_TAPS as f64) / (2.0 * cutoff)).ceil() as usize;
        let span = 2 * width + 1;
        let mut taps = vec![0.0f32; RESAMPLE_PHASES * span];
        for phase in 0..RESAMPLE_PHASES {
            let frac = phase as f64 / RESAMPLE_PHASES as f64;
            let row = phase * span;
            let mut sum = 0.0f64;
            for tap in 0..span {
                let x = frac - (tap as f64 - width as f64);
                let arg = 2.0 * cutoff * x;
                let sinc = if arg.abs() < 1e-9 {
                    1.0
                } else {
                    (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg)
                };
                let t = (x / width as f64).clamp(-1.0, 1.0);
                let angle = std::f64::consts::PI * (t + 1.0);
                let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
                let weight = sinc * window;
                taps[row + tap] = weight as f32;
                sum += weight;
            }
            if sum.abs() > 1e-12 {
                let inverse = (1.0 / sum) as f32;
                for tap in 0..span {
                    taps[row + tap] *= inverse;
                }
            }
        }
        Self { taps, width, span }
    }

    fn apply(&self, input: &[f32], ratio: f64) -> Vec<f32> {
        let out_len = ((input.len() as f64) * ratio).round() as usize;
        let mut out = Vec::with_capacity(out_len);
        let inverse_ratio = 1.0 / ratio;
        for index in 0..out_len {
            let center = index as f64 * inverse_ratio;
            let base = center.floor() as isize;
            let frac = center - base as f64;
            let phase = ((frac * RESAMPLE_PHASES as f64) as usize).min(RESAMPLE_PHASES - 1);
            let row = phase * self.span;
            let mut sum = 0.0f32;
            for tap in 0..self.span {
                let sample_index = base + tap as isize - self.width as isize;
                if sample_index < 0 {
                    continue;
                }
                let Some(sample) = input.get(sample_index as usize) else {
                    break;
                };
                sum += sample * self.taps[row + tap];
            }
            out.push(sum);
        }
        out
    }
}

fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
    if (from_rate - to_rate).abs() < 0.5 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate / from_rate;
    ResampleKernel::new(ratio).apply(input, ratio)
}

fn model_pcm(track: &PreparedAudio) -> StereoBuf {
    let mut left = Vec::with_capacity(track.frames.len());
    let mut right = Vec::with_capacity(track.frames.len());
    for frame in &track.frames {
        left.push(frame[0] as f32 / 32768.0);
        right.push(frame[1] as f32 / 32768.0);
    }
    if track.sample_rate == SAMPLE_RATE {
        StereoBuf { left, right }
    } else {
        StereoBuf {
            left: resample(&left, track.sample_rate as f64, SAMPLE_RATE as f64),
            right: resample(&right, track.sample_rate as f64, SAMPLE_RATE as f64),
        }
    }
}

fn encode_pcm16_wav(stereo: &StereoBuf) -> Result<Vec<u8>, String> {
    if stereo.left.len() != stereo.right.len() || stereo.left.is_empty() {
        return Err("PCM must be non-empty stereo".to_string());
    }
    let data_len = stereo
        .left
        .len()
        .checked_mul(4)
        .and_then(|len| u32::try_from(len).ok())
        .ok_or("PCM WAV exceeds RIFF size")?;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for (left, right) in stereo.left.iter().zip(&stereo.right) {
        for sample in [left, right] {
            let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(out)
}

fn stems_request(input_b64: String) -> GenerateRequestJson {
    let mut request = GenerateRequestJson::default();
    request.model = "bs-roformer-4stem".to_string();
    request.input_b64 = Some(input_b64);
    request.input_content_type = Some("audio/wav".to_string());
    request
}

/// Exact serialized size: standard base64 contains no JSON escape characters.
fn stems_request_body_len(frames: usize) -> Result<usize, String> {
    let wav_bytes = frames
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(44))
        .ok_or("stems request size overflow")?;
    let base64_bytes = wav_bytes
        .checked_add(2)
        .and_then(|bytes| bytes.checked_div(3))
        .and_then(|groups| groups.checked_mul(4))
        .ok_or("stems request size overflow")?;
    stems_request(String::new())
        .serialize_json()
        .len()
        .checked_add(base64_bytes)
        .ok_or_else(|| "stems request size overflow".to_string())
}

fn max_window_frames_for_body(limit: u64) -> Result<usize, String> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let envelope = stems_request(String::new()).serialize_json().len();
    let encoded_capacity = limit.saturating_sub(envelope) / 4 * 4;
    let wav_capacity = encoded_capacity / 4 * 3;
    let frames = wav_capacity.saturating_sub(44) / 4;
    Ok((frames / CHUNK_STEP) * CHUNK_STEP)
}

fn split_stem_windows(frames: usize, max_window_frames: usize) -> Result<Vec<Range<usize>>, String> {
    if frames == 0 {
        return Err("cannot split empty PCM".to_string());
    }
    if frames <= max_window_frames {
        return Ok(vec![0..frames]);
    }
    if max_window_frames <= CHUNK_STEP || max_window_frames % CHUNK_STEP != 0 {
        return Err(format!(
            "AI hub job-body limit cannot hold two {CHUNK_STEP}-frame stems spans"
        ));
    }
    let mut windows = Vec::new();
    let mut start = 0usize;
    loop {
        let end = start.saturating_add(max_window_frames).min(frames);
        windows.push(start..end);
        if end == frames {
            break;
        }
        start = end - CHUNK_STEP;
    }
    Ok(windows)
}

fn stem_windows(frames: usize, body_limit: Option<u64>) -> Result<Vec<Range<usize>>, String> {
    let fits_body = match body_limit {
        Some(limit) => stems_request_body_len(frames)? as u64 <= limit,
        None => true,
    };
    if frames <= MAX_STEMS_JOB_FRAMES && fits_body {
        return Ok(vec![0..frames]);
    }
    let backend_window = (MAX_STEMS_JOB_FRAMES / CHUNK_STEP) * CHUNK_STEP;
    let body_window = match body_limit {
        Some(limit) => max_window_frames_for_body(limit)?,
        None => usize::MAX,
    };
    split_stem_windows(frames, backend_window.min(body_window))
}

fn stitch_stem_window(
    output: &mut StemSet,
    filled: &mut usize,
    range: &Range<usize>,
    window: &StemSet,
) -> Result<(), String> {
    let frames = range.end.saturating_sub(range.start);
    if range.start > *filled
        || range.end > output[0].left.len()
        || window
            .iter()
            .any(|stem| stem.left.len() != frames || stem.right.len() != frames)
    {
        return Err("invalid stems window geometry while stitching".to_string());
    }
    let overlap = filled.saturating_sub(range.start).min(frames);
    for stem in 0..output.len() {
        for (dst, src) in [
            (&mut output[stem].left, &window[stem].left),
            (&mut output[stem].right, &window[stem].right),
        ] {
            for index in 0..overlap {
                let mix = (index + 1) as f32 / (overlap + 1) as f32;
                let at = range.start + index;
                dst[at] = dst[at] * (1.0 - mix) + src[index] * mix;
            }
            dst[range.start + overlap..range.end].copy_from_slice(&src[overlap..]);
        }
    }
    *filled = (*filled).max(range.end);
    Ok(())
}

fn upload_was_rejected_early(error: &str) -> bool {
    error.contains("http 413")
        || error.contains("request body write")
        || error.contains("Broken pipe")
        || error.contains("connection closed in headers")
        || error.contains("WinHttpSendRequest failed")
}

fn separate_stems_window(
    hub: &LocalService,
    pcm: &StereoBuf,
    body_limit: Option<u64>,
) -> Result<StemSet, String> {
    let wav = encode_pcm16_wav(pcm)?;
    let input_b64 = String::from_utf8(makepad_ai_hub::makepad_base64::base64_encode(
        &wav,
        &makepad_ai_hub::makepad_base64::BASE64_STANDARD,
    ))
    .map_err(|_| "base64 encoder returned non-UTF-8".to_string())?;
    drop(wav);
    let request = stems_request(input_b64);
    let body_bytes = stems_request_body_len(pcm.left.len())?;
    if body_limit.is_some_and(|limit| body_bytes as u64 > limit) {
        return Err(format!(
            "AI hub {} stems upload is {body_bytes} bytes, above max_job_body_bytes={}",
            hub.base_url(),
            body_limit.unwrap()
        ));
    }
    let job = hub.request(Domain::Stems, &request).map_err(|error| {
        let error = error.to_string();
        if upload_was_rejected_early(&error) {
            let limit = body_limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| "not advertised".to_string());
            format!(
                "AI hub {} rejected a {body_bytes}-byte stems upload before it finished (max_job_body_bytes={limit}): {error}",
                hub.base_url()
            )
        } else {
            format!("AI hub {} submit failed: {error}", hub.base_url())
        }
    })?;
    let artifact = wait_for_stems(hub, &job)?;
    if artifact.content_type != STEMS_ARTIFACT_CONTENT_TYPE {
        return Err(format!(
            "AI hub returned {}, expected {STEMS_ARTIFACT_CONTENT_TYPE}",
            artifact.content_type
        ));
    }
    let wire = decode_stems_artifact(&artifact.bytes)
        .map_err(|error| format!("AI hub stems artifact: {error}"))?;
    if wire.sample_rate != SAMPLE_RATE || wire.frames != pcm.left.len() {
        return Err(format!(
            "AI hub stems geometry mismatch: {} Hz/{} frames, expected {} Hz/{} frames",
            wire.sample_rate,
            wire.frames,
            SAMPLE_RATE,
            pcm.left.len()
        ));
    }
    let [drums_l, drums_r, bass_l, bass_r, other_l, other_r, vocals_l, vocals_r] = wire.channels;
    Ok([
        StereoBuf { left: drums_l, right: drums_r },
        StereoBuf { left: bass_l, right: bass_r },
        StereoBuf { left: other_l, right: other_r },
        StereoBuf { left: vocals_l, right: vocals_r },
    ])
}

struct ResolvedHub {
    service: LocalService,
    max_job_body_bytes: Option<u64>,
}

fn run_stems(args: StemsArgs) -> Result<(), String> {
    let mut service = None;
    for path in &args.audio {
        let track = prepare_audio(path)?;
        let pcm = model_pcm(&track);
        let header = CacheHeader::for_track(pcm.left.len() as u64);
        if makepad_ai_stems::cache_is_complete_on_disk(&args.out, &track.digest, &header) {
            ensure_cached_oggs(&args.out, &track.digest, &header, true)?;
            println!("{}: cached {}", path.display(), track.digest);
            continue;
        }
        if service.is_none() {
            service = Some(resolve_hub(&args.hub)?);
        }
        let resolved = service.as_ref().unwrap();
        let hub = &resolved.service;
        let windows = stem_windows(pcm.left.len(), resolved.max_job_body_bytes)?;
        let stems = if windows.len() == 1 {
            separate_stems_window(hub, &pcm, resolved.max_job_body_bytes)?
        } else {
            let mut output = makepad_ai_stems::model::empty_stem_set(pcm.left.len());
            let mut filled = 0;
            for range in &windows {
                let window_pcm = StereoBuf {
                    left: pcm.left[range.clone()].to_vec(),
                    right: pcm.right[range.clone()].to_vec(),
                };
                let window = separate_stems_window(hub, &window_pcm, resolved.max_job_body_bytes)?;
                stitch_stem_window(&mut output, &mut filled, range, &window)?;
            }
            if filled != pcm.left.len() {
                return Err(format!("stems stitching stopped at frame {filled}"));
            }
            output
        };
        write_cache(&args.out, &track.digest, &stems)?;
        ensure_cached_oggs(&args.out, &track.digest, &header, true)?;
        println!("{}: separated {}", path.display(), track.digest);
    }
    Ok(())
}

fn resolve_hub(requested: &str) -> Result<ResolvedHub, String> {
    if requested != "auto" {
        let service = LocalService::new(requested);
        let health = service
            .health()
            .map_err(|error| format!("AI hub {requested} is unreachable: {error}"))?;
        ensure_stems_capability(&service)?;
        return Ok(ResolvedHub {
            service,
            max_job_body_bytes: health.max_job_body_bytes,
        });
    }
    let discovered = makepad_ai_hub::discovery::start_listener();
    let deadline = std::time::Instant::now() + Duration::from_secs(7);
    loop {
        let mut nodes = discovered.nodes();
        nodes.sort_by(|a, b| a.base_url.cmp(&b.base_url));
        for node in nodes {
            let service = LocalService::new(&node.base_url);
            if let Ok(health) = service.health() {
                if ensure_stems_capability(&service).is_ok() {
                    return Ok(ResolvedHub {
                        service,
                        max_job_body_bytes: health.max_job_body_bytes,
                    });
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "--hub auto found no reachable fleet node with the stems capability in 7 seconds"
                    .to_string(),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn ensure_stems_capability(service: &LocalService) -> Result<(), String> {
    let available = service
        .list_models()
        .map_err(|error| format!("AI hub {} model query failed: {error}", service.base_url()))?
        .into_iter()
        .any(|model| model.available && model.domain == Domain::Stems.as_str());
    if available {
        Ok(())
    } else {
        Err(format!("AI hub {} does not advertise a stems model", service.base_url()))
    }
}

fn wait_for_stems(
    hub: &LocalService,
    job: &str,
) -> Result<makepad_ai_hub::client::ArtifactBytes, String> {
    loop {
        let status = hub
            .poll(job)
            .map_err(|error| format!("AI hub {} job {job} poll failed: {error}", hub.base_url()))?;
        match status.state.as_str() {
            JOB_STATE_DONE => {
                let artifact = status.artifacts.first().ok_or_else(|| {
                    format!("AI hub {} job {job} finished without an artifact", hub.base_url())
                })?;
                return hub
                    .fetch_artifact(&artifact.id)
                    .map_err(|error| format!("AI hub {} artifact fetch failed: {error}", hub.base_url()));
            }
            JOB_STATE_ERROR => {
                return Err(format!(
                    "AI hub {} job {job} failed: {}",
                    hub.base_url(),
                    status.error.unwrap_or_else(|| "unknown error".to_string())
                ))
            }
            JOB_STATE_CANCELLED => return Err(format!("AI hub {} job {job} was cancelled", hub.base_url())),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn write_cache(root: &Path, digest: &str, stems: &StemSet) -> Result<(), String> {
    let frames = stems[0].left.len();
    if frames == 0
        || stems
            .iter()
            .any(|stem| stem.left.len() != frames || stem.right.len() != frames)
    {
        return Err("separated stem lengths do not match".to_string());
    }
    let mut cache = StemCache::open(root, digest, CacheHeader::for_track(frames as u64))
        .map_err(|error| error.to_string())?;
    for start in (0..frames).step_by(CHUNK_STEP) {
        let end = (start + CHUNK_STEP).min(frames);
        let span: StemSet = std::array::from_fn(|stem| StereoBuf {
            left: stems[stem].left[start..end].to_vec(),
            right: stems[stem].right[start..end].to_vec(),
        });
        cache.write_span(start, &span).map_err(|error| error.to_string())?;
    }
    if !cache.is_complete() {
        return Err("stem cache remained incomplete after write".to_string());
    }
    Ok(())
}

fn ensure_cached_oggs(
    root: &Path,
    digest: &str,
    header: &CacheHeader,
    write: bool,
) -> Result<[Vec<u8>; 4], String> {
    let dir = root.join(digest);
    let paths = STEM_OGG_NAMES.map(|name| dir.join(name));
    if paths.iter().all(|path| path.is_file()) {
        let read = |index: usize| {
            std::fs::read(&paths[index])
                .map_err(|error| format!("{}: {error}", paths[index].display()))
        };
        return Ok([read(0)?, read(1)?, read(2)?, read(3)?]);
    }
    let mut cache = StemCache::open(root, digest, header.clone()).map_err(|error| error.to_string())?;
    if !cache.is_complete() {
        return Err(format!("stem cache {digest} is incomplete"));
    }
    let stems = cache.read_all().map_err(|error| error.to_string())?;
    let oggs = encode_stem_oggs(&stems);
    if write {
        for (path, bytes) in paths.iter().zip(&oggs) {
            write_atomic(path, bytes)?;
        }
    }
    Ok(oggs)
}

fn wave_cache_path(root: &Path, digest: &str) -> PathBuf {
    root.join(format!("{digest}.wave"))
}

fn loop_cache_path(root: &Path, digest: &str) -> PathBuf {
    root.join(digest).join(DJ_LOOP_SPLAT_NAME)
}

fn analysis_pcm(audio: &PreparedAudio) -> makepad_vj_analysis::mixer::TrackPcm {
    makepad_vj_analysis::mixer::TrackPcm {
        frames: audio.frames.clone(),
        sample_rate: audio.sample_rate,
    }
}

fn analyze_track(audio: &PreparedAudio) -> (
    makepad_vj_analysis::wave_analysis::TrackAnalysis,
    Option<makepad_vj_analysis::loop_splat::SplatGrid>,
) {
    let pcm = analysis_pcm(audio);
    let analysis = makepad_vj_analysis::wave_analysis::analyze(&pcm);
    let splat = makepad_vj_analysis::loop_splat::build_splat(&analysis, None);
    (analysis, splat)
}

fn read_cache(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn load_analysis_caches(
    args: &PackArgs,
    digest: &str,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), String> {
    let wave_path = wave_cache_path(&args.wave_cache, digest);
    let analysis = read_cache(&wave_path)?;
    if let Some(bytes) = &analysis {
        makepad_vj_analysis::wave_analysis::decode_analysis(bytes)
            .map_err(|error| format!("{}: {error}", wave_path.display()))?;
    }
    let loop_path = loop_cache_path(&args.loop_cache, digest);
    let splat = read_cache(&loop_path)?;
    if let Some(bytes) = &splat {
        makepad_vj_analysis::loop_splat::decode_splat(bytes)
            .map_err(|error| format!("{}: {error}", loop_path.display()))?;
    }
    Ok((analysis, splat))
}

fn run_analyse(args: AnalyseArgs) -> Result<(), String> {
    for path in &args.audio {
        let audio = prepare_audio(path)?;
        let (analysis, splat) = analyze_track(&audio);
        let key = makepad_vj_analysis::wave_analysis::AnalysisKey::from_digest(&audio.digest)?;
        let wave_path = makepad_vj_analysis::wave_analysis::store_analysis_in(
            &args.wave_cache,
            &key,
            &analysis,
        )?;
        debug_assert_eq!(wave_path, wave_cache_path(&args.wave_cache, &audio.digest));
        let mut roles = DJ_ANALYSIS_NAME.to_string();
        if let Some(splat) = splat {
            let loop_path = loop_cache_path(&args.loop_cache, &audio.digest);
            write_atomic(
                &loop_path,
                &makepad_vj_analysis::loop_splat::encode_splat(&splat),
            )?;
            roles.push_str(",loop-splat.bin");
        }
        println!(
            "{}: analysed {} caches={roles}",
            audio.path.display(),
            audio.digest
        );
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("output has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = path.with_extension(format!("part-{}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())
}

struct PackTrack {
    audio: PreparedAudio,
    alias: AssetAlias,
    side_files: Vec<makepad_asset_client::side_channels::SideChannelFile>,
    rights: Rights,
    attribution: TrackAttribution,
    description: String,
}

fn run_pack(args: PackArgs) -> Result<(), String> {
    let mut tracks = Vec::new();
    let mut aliases = BTreeSet::new();
    for path in &args.audio {
        let mut audio = prepare_audio(path)?;
        let attribution = load_attribution(&audio)?;
        audio.title = attribution.title.clone();
        audio.artist = attribution.artist.clone();
        if audio.media == MediaType::Bin {
            return Err(format!(
                "{}: FLAC can be separated but FileRole::Audio has no FLAC media type; transcode before pack",
                path.display()
            ));
        }
        let alias = track_alias(&audio.artist, &audio.title)?;
        if !aliases.insert(alias.clone()) {
            return Err(format!("two inputs resolve to the same alias {alias}"));
        }
        let model_frames = model_pcm(&audio).left.len();
        let header = CacheHeader::for_track(model_frames as u64);
        let stems_complete = args.stem_cache.as_deref().is_some_and(|root| {
            makepad_ai_stems::cache_is_complete_on_disk(root, &audio.digest, &header)
        });
        if !stems_complete && args.require_stems {
            return Err(format!("required stem cache {} is not complete", audio.digest));
        }
        let (analysis, splat) = load_analysis_caches(&args, &audio.digest)?;
        if args.dry_run {
            let has_lyrics = load_lyrics(&args, &audio.digest)?.is_some();
            let (_, description) = attribution_rights(&audio, &attribution)?;
            let mut side_channels = Vec::new();
            if stems_complete {
                side_channels.extend(["stem_drums", "stem_bass", "stem_vocals", "stem_other"]);
            }
            if has_lyrics {
                side_channels.push("lyrics");
            }
            if analysis.is_some() {
                side_channels.push("dj_analysis");
            }
            if splat.is_some() {
                side_channels.push("dj_loop_splat");
            }
            let stem_report = if stems_complete {
                "cached"
            } else {
                "omitted (no complete cache)"
            };
            println!(
                "DRY RUN {} -> {} digest={} side-channels=[{}] stems={} description={description:?}",
                audio.path.display(),
                alias,
                audio.digest,
                side_channels.join(","),
                stem_report
            );
            continue;
        }
        let oggs = if stems_complete {
            let root = args.stem_cache.as_deref().expect("complete cache has a root");
            Some(ensure_cached_oggs(root, &audio.digest, &header, true)?)
        } else {
            println!(
                "{}: packing without stems (no complete stem cache {})",
                audio.path.display(),
                audio.digest
            );
            None
        };
        let lyrics = load_lyrics(&args, &audio.digest)?;
        let side_files = side_channel_files_with_analysis(oggs, lyrics, analysis, splat);
        let (rights, description) = attribution_rights(&audio, &attribution)?;
        tracks.push(PackTrack { audio, alias, side_files, rights, attribution, description });
    }
    if args.dry_run {
        return Ok(());
    }
    let core = AssetServerCore::open(&args.store, Budgets::default_v1())
        .map_err(|error| format!("open store {}: {error}", args.store.display()))?;
    let now = now_ms();
    let mut items = Vec::with_capacity(tracks.len());
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    for track in &tracks {
        let asset_id = core
            .catalog()
            .resolve_asset_alias(&track.alias)
            .map_err(|error| error.to_string())?
            .map(|head| head.asset_id)
            .unwrap_or_else(|| asset_id_from_audio(&track.audio.bytes));
        let (manifest, item_blobs) = make_manifest(track, asset_id)?;
        let manifest_bytes = manifest.to_canonical_bytes().map_err(|error| error.to_string())?;
        items.push(PublishBatchItem {
            namespace: "music".to_string(),
            manifest_bytes,
            annotation: make_annotation(track),
            alias: Some(track.alias.clone()),
        });
        blobs.extend(item_blobs);
    }
    for bytes in &blobs {
        core.put_blob(bytes, now).map_err(|error| format!("store blob: {error}"))?;
    }
    let outcomes = core
        .publish_batch(&items, now)
        .map_err(|error| format!("store publish: {error}"))?;
    replace_static_export(&core, &args.site_out)?;
    let updated = outcomes.iter().filter(|outcome| !outcome.already_published).count();
    println!(
        "packed {} track(s), {} new revision(s), static site {}",
        outcomes.len(),
        updated,
        args.site_out.display()
    );
    Ok(())
}

fn load_lyrics(args: &PackArgs, digest: &str) -> Result<Option<String>, String> {
    let mut candidates = Vec::new();
    if let Some(root) = &args.stem_cache {
        candidates.push(root.join(digest).join("lyrics.json"));
    }
    if let Some(root) = &args.lyrics_cache {
        candidates.push(root.join(format!("{digest}.json")));
    }
    let Some(path) = candidates.iter().find(|path| path.is_file()) else {
        return Ok(None);
    };
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if makepad_audio_lyrics::TrackLyrics::from_json(&bytes, digest).is_none() {
        return Err(format!("{}: lyrics JSON is invalid or belongs to another digest", path.display()));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("{}: lyrics JSON is not UTF-8", path.display()))
}

fn attribution_path(audio: &Path) -> PathBuf {
    let mut name = audio.file_name().unwrap_or_default().to_os_string();
    name.push(".attribution.json");
    audio.with_file_name(name)
}

fn load_attribution(audio: &PreparedAudio) -> Result<TrackAttribution, String> {
    let path = attribution_path(&audio.path);
    let mut attribution = if path.is_file() {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: read attribution sidecar: {error}", path.display()))?;
        TrackAttribution::deserialize_json(&text)
            .map_err(|error| format!("{}: invalid attribution JSON: {error:?}", path.display()))?
    } else {
        audio.embedded_attribution.clone()
    };
    for value in [
        &mut attribution.title,
        &mut attribution.artist,
        &mut attribution.artist_url,
        &mut attribution.album,
        &mut attribution.source_url,
        &mut attribution.license,
        &mut attribution.license_url,
    ] {
        *value = clean_text(value, makepad_asset_client::wire::MAX_SNIPPET_BYTES);
    }
    if attribution.title.is_empty() {
        attribution.title = audio.title.clone();
    }
    if attribution.artist.is_empty() {
        attribution.artist = audio.artist.clone();
    }
    if attribution.license.is_empty() {
        return Err(format!(
            "{}: track has no licence; add {} with a non-empty license field (unlicensed tracks must not ship)",
            audio.path.display(),
            path.display(),
        ));
    }
    for (name, url) in [
        ("artist_url", &attribution.artist_url),
        ("source_url", &attribution.source_url),
        ("license_url", &attribution.license_url),
    ] {
        if !url.is_empty() && !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(format!("{}: attribution {name} must be an http(s) URL", path.display()));
        }
    }
    Ok(attribution)
}

fn attribution_rights(
    audio: &PreparedAudio,
    attribution: &TrackAttribution,
) -> Result<(Rights, String), String> {
    let (license, revision) = match attribution.license.as_str() {
        "CC BY 4.0" => ("CC-BY-4.0", "4.0"),
        "CC BY-SA 3.0" => ("CC-BY-SA-3.0", "3.0"),
        "Public Domain (CC0 1.0)" => ("CC0-1.0", "1.0"),
        other => {
            return Err(format!(
                "{}: unsupported licence {other:?}; expected CC BY 4.0, CC BY-SA 3.0, or Public Domain (CC0 1.0)",
                audio.path.display()
            ));
        }
    };
    let default_terms_url = match attribution.license.as_str() {
        "CC BY 4.0" => "https://creativecommons.org/licenses/by/4.0/legalcode",
        "CC BY-SA 3.0" => "https://creativecommons.org/licenses/by-sa/3.0/legalcode",
        _ => "https://creativecommons.org/publicdomain/zero/1.0/legalcode",
    };
    let rights = Rights {
        license: license.to_string(),
        license_revision: revision.to_string(),
        terms_digest: None,
        terms_url: if attribution.license_url.is_empty() {
            default_terms_url.to_string()
        } else {
            attribution.license_url.clone()
        },
        credits: attribution.artist.clone(),
        source: if attribution.source_url.is_empty() {
            audio.path.file_name().and_then(|name| name.to_str()).unwrap_or("track").to_string()
        } else {
            attribution.source_url.clone()
        },
        source_archive: None,
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    };
    let description = track_description(attribution);
    Ok((rights, description))
}

fn track_description(attribution: &TrackAttribution) -> String {
    let text = if attribution.source_url.is_empty() {
        format!("{} — {} — {}", attribution.title, attribution.artist, attribution.license)
    } else {
        format!(
            "{} — {} — {} — {}",
            attribution.title, attribution.artist, attribution.license, attribution.source_url
        )
    };
    clean_text(&text, makepad_asset_client::wire::MAX_SNIPPET_BYTES)
}

fn track_alias(artist: &str, title: &str) -> Result<AssetAlias, String> {
    let artist = slug(artist, 48, "unknown-artist");
    let title = slug(title, 48, "untitled");
    AssetAlias::from_str(&format!("music/{artist}/{title}")).map_err(|error| error.to_string())
}

fn slug(input: &str, max: usize, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for character in input.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() || character == '_' {
            if dash && !out.is_empty() && out.len() < max {
                out.push('-');
            }
            dash = false;
            if out.len() < max {
                out.push(character);
            }
        } else {
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { fallback.to_string() } else { out }
}

fn clean_text(input: &str, max: usize) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for character in input.chars() {
        if character.is_whitespace() || character.is_control() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(character);
        }
    }
    if out.len() > max {
        const ELLIPSIS: &str = "…";
        let mut end = max.saturating_sub(ELLIPSIS.len()).min(out.len());
        while !out.is_char_boundary(end) { end -= 1; }
        out.truncate(end);
        while out.ends_with(' ') { out.pop(); }
        if max >= ELLIPSIS.len() { out.push_str(ELLIPSIS); }
    }
    out
}

fn id3_comment(bytes: &[u8]) -> Option<String> {
    let header = bytes.get(..10)?;
    if &header[..3] != b"ID3" { return None; }
    let version = header[3];
    if !(2..=4).contains(&version) { return None; }
    let body_len = syncsafe(&header[6..10]);
    let end = 10usize.checked_add(body_len)?.min(bytes.len());
    let mut at = 10usize;
    if header[5] & 0x40 != 0 {
        let raw = bytes.get(at..at + 4)?;
        let size = if version >= 4 {
            syncsafe(raw)
        } else {
            (u32::from_be_bytes(raw.try_into().ok()?) as usize).saturating_add(4)
        };
        at = at.checked_add(size.max(4))?;
    }
    let (id_len, frame_header_len) = if version == 2 { (3usize, 6usize) } else { (4, 10) };
    while at.checked_add(frame_header_len)? <= end {
        let id = bytes.get(at..at + id_len)?;
        if id.first() == Some(&0) { break; }
        let size = if version == 2 {
            let raw = bytes.get(at + 3..at + 6)?;
            ((raw[0] as usize) << 16) | ((raw[1] as usize) << 8) | raw[2] as usize
        } else if version >= 4 {
            syncsafe(bytes.get(at + 4..at + 8)?)
        } else {
            u32::from_be_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize
        };
        let body_start = at.checked_add(frame_header_len)?;
        let body_end = body_start.checked_add(size)?;
        if body_end > end { break; }
        if id == b"COMM" || id == b"COM" {
            return decode_id3_comment(bytes.get(body_start..body_end)?);
        }
        at = body_end;
    }
    None
}

fn syncsafe(raw: &[u8]) -> usize {
    raw.iter().fold(0usize, |value, byte| (value << 7) | (byte & 0x7f) as usize)
}

fn decode_id3_comment(body: &[u8]) -> Option<String> {
    let encoding = *body.first()?;
    let payload = body.get(4..)?; // encoding byte plus the three-byte language
    let text = match encoding {
        0 | 3 => {
            let start = payload.iter().position(|byte| *byte == 0).map_or(0, |at| at + 1);
            decode_id3_string(encoding, &payload[start..])
        }
        1 | 2 => {
            let start = payload
                .chunks_exact(2)
                .position(|pair| pair == [0, 0])
                .map_or(0, |at| at * 2 + 2);
            decode_id3_string(encoding, &payload[start..])
        }
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

fn decode_id3_string(encoding: u8, bytes: &[u8]) -> String {
    match encoding {
        0 => bytes.iter().map(|byte| *byte as char).collect(),
        1 | 2 => {
            let (bytes, big_endian) = match (encoding, bytes) {
                (1, [0xff, 0xfe, rest @ ..]) => (rest, false),
                (1, [0xfe, 0xff, rest @ ..]) => (rest, true),
                (1, rest) => (rest, false),
                (_, rest) => (rest, true),
            };
            let units = bytes.chunks_exact(2).map(|pair| {
                if big_endian {
                    u16::from_be_bytes([pair[0], pair[1]])
                } else {
                    u16::from_le_bytes([pair[0], pair[1]])
                }
            }).collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn asset_id_from_audio(bytes: &[u8]) -> AssetId {
    let digest = BlobId::hash_of(bytes);
    AssetId::from_bytes(digest.as_bytes()[..16].try_into().unwrap())
}

fn make_manifest(track: &PackTrack, asset_id: AssetId) -> Result<(AssetManifest, Vec<Vec<u8>>), String> {
    let mut blobs = vec![track.audio.bytes.clone()];
    blobs.extend(track.side_files.iter().map(|file| file.bytes.clone()));
    let mut files = vec![AssetFile {
        role: FileRole::Audio,
        tier: DeviceTier::Any,
        lod: 0,
        media: track.audio.media,
        blob: BlobId::hash_of(&track.audio.bytes),
        byte_len: track.audio.bytes.len() as u64,
        dims: None,
    }];
    files.extend(track.side_files.iter().map(|file| AssetFile {
        role: file.role,
        tier: DeviceTier::Any,
        lod: 0,
        media: file.media,
        blob: BlobId::hash_of(&file.bytes),
        byte_len: file.bytes.len() as u64,
        dims: None,
    }));
    let total_bytes = files.iter().map(|file| file.byte_len).sum();
    let media_millis = ((track.audio.frames.len() as u128 * 1000)
        / track.audio.sample_rate.max(1) as u128)
        .clamp(1, u32::MAX as u128) as u32;
    let mut manifest = AssetManifest {
        asset_id,
        kind: AssetKind::Audio,
        files,
        dependencies: Vec::new(),
        thumbnail: None,
        metrics: Metrics { total_bytes, media_millis, ..Metrics::default() },
        coordinate_system: CoordinateSystem {
            units_per_meter: 1.0,
            up: Axis::YPos,
            forward: Axis::ZNeg,
            pivot: Pivot::Origin,
        },
        bounds: Bounds { min: Vec3::new(-0.5, -0.5, -0.5), max: Vec3::new(0.5, 0.5, 0.5) },
        anchors: Vec::new(),
        capabilities: Capabilities { loopable: true, ..Capabilities::default() },
        spawn_recipe: None,
        provenance: None,
        rights: track.rights.clone(),
    };
    manifest.canonicalize();
    manifest.validate().map_err(|error| error.to_string())?;
    Ok((manifest, blobs))
}

fn make_annotation(track: &PackTrack) -> AssetAnnotation {
    AssetAnnotation {
        title: track.audio.title.clone(),
        description: track.description.clone(),
        kind: Some(AssetKind::Audio),
        categories: vec!["music".to_string()],
        tags: vec!["music".to_string(), "stems".to_string()],
        creator: track.audio.artist.clone(),
        artist: track.attribution.artist.clone(),
        artist_url: track.attribution.artist_url.clone(),
        album: track.attribution.album.clone(),
        source_url: track.attribution.source_url.clone(),
        license: track.attribution.license.clone(),
        license_url: track.attribution.license_url.clone(),
        owner: None,
        generator: "makepad-dj-pack".to_string(),
        backend: "ai-hub".to_string(),
        model: "bs-roformer-4stem".to_string(),
        prompt: String::new(),
        provenance: String::new(),
        visibility: Visibility::Public,
    }
}

fn replace_static_export(core: &AssetServerCore, site_out: &Path) -> Result<(), String> {
    let name = site_out.file_name().and_then(|name| name.to_str()).ok_or("bad --site-out")?;
    let parent = site_out.parent().unwrap_or_else(|| Path::new("."));
    let fresh = parent.join(format!(".{name}.dj-pack-new-{}", std::process::id()));
    let old = parent.join(format!(".{name}.dj-pack-old-{}", std::process::id()));
    if fresh.exists() || old.exists() {
        return Err("stale dj-pack export staging directory exists".to_string());
    }
    let options = StaticExportOptions {
        namespace: Some("music".to_string()),
        kind: Some(AssetKind::Audio),
        ..StaticExportOptions::default()
    };
    let report = export_static(core, &fresh, &options)
        .map_err(|error| format!("static export selected nothing or failed: {error}"))?;
    if report.assets == 0 {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(format!(
            "static export selected nothing: namespace=music kind=audio (excluded rights={}, budget={}, kind={})",
            report.excluded_rights, report.excluded_budget, report.excluded_kind_mismatch
        ));
    }
    if site_out.exists() {
        std::fs::rename(site_out, &old).map_err(|error| format!("replace old site: {error}"))?;
    }
    if let Err(error) = std::fs::rename(&fresh, site_out) {
        if old.exists() {
            let _ = std::fs::rename(&old, site_out);
        }
        return Err(format!("install static site: {error}"));
    }
    if old.exists() {
        std::fs::remove_dir_all(&old).map_err(|error| format!("remove old static site: {error}"))?;
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_encode::{encode_vorbis, EncodeOptions};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/dj-pack-tests")
                .join(format!("{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            if root.exists() {
                std::fs::remove_dir_all(&root).unwrap();
            }
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tone_stems(seconds: usize) -> StemSet {
        let frames = SAMPLE_RATE as usize * seconds;
        std::array::from_fn(|stem| {
            let hz = [220.0, 110.0, 660.0, 440.0][stem];
            let samples = (0..frames)
                .map(|frame| {
                    (2.0 * std::f32::consts::PI * hz * frame as f32 / SAMPLE_RATE as f32).sin()
                        * 0.2
                })
                .collect::<Vec<_>>();
            StereoBuf { left: samples.clone(), right: samples }
        })
    }

    fn tone_ogg(seconds: usize) -> Vec<u8> {
        let frames = SAMPLE_RATE as usize * seconds;
        let mut pcm = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let value = (2.0 * std::f32::consts::PI * 330.0 * frame as f32 / SAMPLE_RATE as f32)
                .sin()
                * 0.2;
            pcm.extend_from_slice(&[value, value]);
        }
        encode_vorbis(SAMPLE_RATE, 2, &pcm, &EncodeOptions::default()).unwrap()
    }

    fn write_attribution(audio: &Path, license: &str) {
        std::fs::write(
            attribution_path(audio),
            format!(
                "{{\"title\":\"tone\",\"artist\":\"tone\",\"artist_url\":\"https://example.test/artist\",\"album\":\"Fixture Album\",\"source_url\":\"https://example.test/tone\",\"license\":\"{license}\",\"license_url\":\"https://creativecommons.org/licenses/by/4.0/\"}}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn two_second_cache_round_trip_and_ogg_encode() {
        let dir = TestDir::new("cache");
        let stems = tone_stems(2);
        write_cache(&dir.0, "abcd", &stems).unwrap();
        let header = CacheHeader::for_track((SAMPLE_RATE * 2) as u64);
        assert!(makepad_ai_stems::cache_is_complete_on_disk(&dir.0, "abcd", &header));
        let oggs = ensure_cached_oggs(&dir.0, "abcd", &header, true).unwrap();
        for bytes in oggs {
            let decoded = decode_any(&bytes).unwrap();
            assert_eq!(decoded.rate, SAMPLE_RATE);
            assert_eq!(decoded.channels, 2);
            assert_eq!(decoded.frames(), SAMPLE_RATE as usize * 2);
        }
    }

    #[test]
    fn resample_uses_vj_frame_geometry() {
        assert_eq!(resample(&vec![0.0; 48_000], 48_000.0, 44_100.0).len(), 44_100);
        assert_eq!(resample(&vec![0.0; 32_000], 32_000.0, 44_100.0).len(), 44_100);
    }

    #[test]
    fn loop_splat_side_channel_round_trips() {
        use makepad_vj_analysis::decks::LoopSpan;
        use makepad_vj_analysis::loop_splat::{
            decode_splat, encode_splat, SplatCell, SplatGrid, SplatSection, SPLAT_COLS,
            SPLAT_ROWS,
        };
        let mut cells = [[None; SPLAT_COLS]; SPLAT_ROWS];
        cells[0][0] = Some(SplatCell {
            span: LoopSpan { start_secs: 1.0, end_secs: 9.0 },
            bars: 4,
            energy: 0.75,
            silent: false,
        });
        let grid = SplatGrid {
            bpm: 120.0,
            bar_secs: 2.0,
            first_bar_secs: 1.0,
            sections: vec![SplatSection { start_secs: 1.0, end_secs: 17.0, bars: 8 }],
            cells,
            bars_per_col: [4, 0, 0, 0, 0, 0, 0, 0],
        };
        let bytes = encode_splat(&grid);
        assert_eq!(decode_splat(&bytes).unwrap(), grid);
        let dir = TestDir::new("loop-cache");
        let path = loop_cache_path(&dir.0, "abcd");
        write_atomic(&path, &bytes).unwrap();
        assert_eq!(path, dir.0.join("abcd/loop-splat.bin"));
        assert_eq!(decode_splat(&std::fs::read(path).unwrap()).unwrap(), grid);
    }

    #[test]
    fn id3_comment_is_available_as_the_source_fallback() {
        let mut payload = vec![3];
        payload.extend_from_slice(b"eng\0https://example.test/from-id3");
        let mut frame = b"COMM".to_vec();
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&payload);
        let mut tag = b"ID3\x03\0\0".to_vec();
        let size = frame.len();
        tag.extend_from_slice(&[
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]);
        tag.extend_from_slice(&frame);
        assert_eq!(id3_comment(&tag).as_deref(), Some("https://example.test/from-id3"));
    }

    #[test]
    fn thirty_second_tone_splits_and_stitches_without_a_seam() {
        let frames = SAMPLE_RATE as usize * 30;
        let body_limit = stems_request_body_len(3 * CHUNK_STEP).unwrap() as u64;
        let windows = stem_windows(frames, Some(body_limit)).unwrap();
        assert!(windows.len() > 1);
        for pair in windows.windows(2) {
            assert_eq!(pair[0].end - pair[1].start, CHUNK_STEP);
            assert_eq!(pair[1].start % CHUNK_STEP, 0);
        }

        let tone: Vec<f32> = (0..frames)
            .map(|frame| {
                (2.0 * std::f32::consts::PI * 330.0 * frame as f32 / SAMPLE_RATE as f32).sin()
                    * 0.2
            })
            .collect();
        let mut stitched = makepad_ai_stems::model::empty_stem_set(frames);
        let mut filled = 0;
        for range in &windows {
            let samples = tone[range.clone()].to_vec();
            let window: StemSet = std::array::from_fn(|_| StereoBuf {
                left: samples.clone(),
                right: samples.clone(),
            });
            stitch_stem_window(&mut stitched, &mut filled, range, &window).unwrap();
        }
        assert_eq!(filled, frames);
        for frame in (0..frames).step_by(997) {
            assert!((stitched[0].left[frame] - tone[frame]).abs() < 1e-6);
            assert!((stitched[3].right[frame] - tone[frame]).abs() < 1e-6);
        }
    }

    #[test]
    fn publish_and_export_matches_static_store_layout() {
        let dir = TestDir::new("export");
        let tracks = dir.0.join("tracks");
        let cache = dir.0.join("stem-cache");
        let lyrics = dir.0.join("lyrics-cache");
        let wave_cache = dir.0.join("wave-cache");
        let loop_cache = dir.0.join("loop-cache");
        std::fs::create_dir_all(&tracks).unwrap();
        std::fs::create_dir_all(&lyrics).unwrap();
        let audio_path = tracks.join("tone.ogg");
        std::fs::write(&audio_path, tone_ogg(2)).unwrap();
        write_attribution(&audio_path, "CC BY 4.0");
        let audio = prepare_audio(&audio_path).unwrap();
        let stems = tone_stems(2);
        write_cache(&cache, &audio.digest, &stems).unwrap();
        let lyrics_json = makepad_audio_lyrics::TrackLyrics {
            backend: "fixture".into(),
            model: "fixture".into(),
            language: "en".into(),
            duration_secs: 2.0,
            onset: makepad_audio_lyrics::OnsetStats::default(),
            lines: Vec::new(),
        }
        .to_json(&audio.digest);
        std::fs::write(lyrics.join(format!("{}.json", audio.digest)), lyrics_json).unwrap();
        run_analyse(AnalyseArgs {
            wave_cache: wave_cache.clone(),
            loop_cache: loop_cache.clone(),
            audio: vec![audio_path.clone()],
        })
        .unwrap();
        let wave_path = wave_cache_path(&wave_cache, &audio.digest);
        assert_eq!(wave_path, wave_cache.join(format!("{}.wave", audio.digest)));
        makepad_vj_analysis::wave_analysis::decode_analysis(
            &std::fs::read(wave_path).unwrap(),
        )
        .unwrap();
        run_pack(PackArgs {
            store: dir.0.join("store"),
            site_out: dir.0.join("site"),
            stem_cache: Some(cache),
            lyrics_cache: Some(lyrics),
            wave_cache: wave_cache.clone(),
            loop_cache: loop_cache.clone(),
            require_stems: false,
            dry_run: false,
            audio: vec![audio_path],
        })
        .unwrap();
        let site = dir.0.join("site/v1");
        assert!(site.join("health").is_file());
        assert!(site.join("static/manifest.json").is_file());
        let static_manifest = std::fs::read_to_string(site.join("static/manifest.json")).unwrap();
        assert!(static_manifest.contains("\"assets\""));
        assert!(static_manifest.contains("\"aliases\""));
        assert!(static_manifest.contains("\"revisions\""));
        assert!(static_manifest.contains("\"role\":\"lyrics\""));
        assert!(static_manifest.contains("\"role\":\"dj_analysis\""));
        assert!(static_manifest.contains(
            "\"description\":\"tone — tone — CC BY 4.0 — https://example.test/tone\""
        ));
        for field in [
            "\"artist\":\"tone\"",
            "\"artist_url\":\"https://example.test/artist\"",
            "\"album\":\"Fixture Album\"",
            "\"source_url\":\"https://example.test/tone\"",
            "\"license\":\"CC BY 4.0\"",
            "\"license_url\":\"https://creativecommons.org/licenses/by/4.0/\"",
        ] {
            assert!(static_manifest.contains(field), "missing {field}");
        }
        assert!(std::fs::read_dir(site.join("assets")).unwrap().next().is_some());
        assert!(std::fs::read_dir(site.join("revisions")).unwrap().next().is_some());
        assert!(std::fs::read_dir(site.join("blobs")).unwrap().next().is_some());

        {
            let core = AssetServerCore::open(&dir.0.join("store"), Budgets::default_v1()).unwrap();
            let head = core
                .catalog()
                .resolve_asset_alias(&AssetAlias::from_str("music/tone/tone").unwrap())
                .unwrap()
                .unwrap();
            let annotation = core.search().annotation(&head.asset_id).unwrap().unwrap();
            assert_eq!(annotation.artist, "tone");
            assert_eq!(annotation.album, "Fixture Album");
            assert_eq!(annotation.license, "CC BY 4.0");
            assert_eq!(annotation.source_url, "https://example.test/tone");
        }

        // The exact same pack is a revision replay and atomically replaces the snapshot.
        run_pack(PackArgs {
            store: dir.0.join("store"),
            site_out: dir.0.join("site"),
            stem_cache: Some(dir.0.join("stem-cache")),
            lyrics_cache: Some(dir.0.join("lyrics-cache")),
            wave_cache,
            loop_cache,
            require_stems: false,
            dry_run: false,
            audio: vec![tracks.join("tone.ogg")],
        })
        .unwrap();
    }

    #[test]
    fn pack_omits_missing_stems_without_touching_empty_cache() {
        let dir = TestDir::new("empty-cache");
        let tracks = dir.0.join("tracks");
        let cache = dir.0.join("stem-cache");
        std::fs::create_dir_all(&tracks).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let audio_path = tracks.join("tone.ogg");
        std::fs::write(&audio_path, tone_ogg(1)).unwrap();
        write_attribution(&audio_path, "Public Domain (CC0 1.0)");
        let digest = prepare_audio(&audio_path).unwrap().digest;

        let error = run_pack(PackArgs {
            store: dir.0.join("required-store"),
            site_out: dir.0.join("required-site"),
            stem_cache: Some(cache.clone()),
            lyrics_cache: None,
            wave_cache: dir.0.join("wave-cache"),
            loop_cache: dir.0.join("loop-cache"),
            require_stems: true,
            dry_run: false,
            audio: vec![audio_path.clone()],
        })
        .unwrap_err();
        assert!(error.contains(&digest));

        run_pack(PackArgs {
            store: dir.0.join("store"),
            site_out: dir.0.join("site"),
            stem_cache: Some(cache.clone()),
            lyrics_cache: None,
            wave_cache: dir.0.join("wave-cache"),
            loop_cache: dir.0.join("loop-cache"),
            require_stems: false,
            dry_run: false,
            audio: vec![audio_path],
        })
        .unwrap();

        assert!(std::fs::read_dir(&cache).unwrap().next().is_none());
        let static_manifest =
            std::fs::read_to_string(dir.0.join("site/v1/static/manifest.json")).unwrap();
        assert!(!static_manifest.contains("\"role\":\"stem_"));
        assert!(!static_manifest.contains("\"role\":\"dj_analysis\""));
        assert!(!static_manifest.contains("\"role\":\"dj_loop_splat\""));
    }

    #[test]
    fn pack_refuses_a_track_without_a_licence() {
        let dir = TestDir::new("missing-licence");
        let audio_path = dir.0.join("tone.ogg");
        std::fs::write(&audio_path, tone_ogg(1)).unwrap();
        let error = run_pack(PackArgs {
            store: dir.0.join("store"),
            site_out: dir.0.join("site"),
            stem_cache: None,
            lyrics_cache: None,
            wave_cache: dir.0.join("wave-cache"),
            loop_cache: dir.0.join("loop-cache"),
            require_stems: false,
            dry_run: true,
            audio: vec![audio_path],
        })
        .unwrap_err();
        assert!(error.contains("track has no licence"), "{error}");
        assert!(error.contains("unlicensed tracks must not ship"), "{error}");
    }

    #[test]
    fn dry_run_writes_nothing_in_read_only_directory() {
        let dir = TestDir::new("dry-run-read-only");
        let tracks = dir.0.join("tracks");
        std::fs::create_dir_all(&tracks).unwrap();
        let audio_path = tracks.join("tone.ogg");
        std::fs::write(&audio_path, tone_ogg(1)).unwrap();
        write_attribution(&audio_path, "Public Domain (CC0 1.0)");

        let original_permissions = std::fs::metadata(&dir.0).unwrap().permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_readonly(true);
        std::fs::set_permissions(&dir.0, read_only_permissions).unwrap();
        let result = run_pack(PackArgs {
            store: dir.0.join("store"),
            site_out: dir.0.join("site"),
            stem_cache: Some(dir.0.join("stem-cache")),
            lyrics_cache: Some(dir.0.join("lyrics-cache")),
            wave_cache: dir.0.join("wave-cache"),
            loop_cache: dir.0.join("loop-cache"),
            require_stems: false,
            dry_run: true,
            audio: vec![audio_path],
        });
        std::fs::set_permissions(&dir.0, original_permissions).unwrap();

        result.unwrap();
        assert!(!dir.0.join("store").exists());
        assert!(!dir.0.join("site").exists());
        assert!(!dir.0.join("stem-cache").exists());
        assert!(!dir.0.join("lyrics-cache").exists());
    }

    #[test]
    fn pack_cache_directories_are_opt_in() {
        let args = parse_pack(&[
            "--store".into(),
            "store".into(),
            "--site-out".into(),
            "site".into(),
            "tone.ogg".into(),
        ])
        .unwrap();
        assert!(args.stem_cache.is_none());
        assert!(args.lyrics_cache.is_none());
        assert_eq!(
            args.wave_cache,
            makepad_vj_analysis::wave_analysis::cache_dir()
        );
        assert_eq!(args.loop_cache, default_loop_cache());

        let args = parse_analyse(&[
            "tone.ogg".into(),
            "--wave-cache".into(),
            "waves".into(),
            "--loop-cache".into(),
            "loops".into(),
        ])
        .unwrap();
        assert_eq!(args.wave_cache, PathBuf::from("waves"));
        assert_eq!(args.loop_cache, PathBuf::from("loops"));
    }
}
