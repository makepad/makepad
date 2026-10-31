//! The preview generator (`STAGE_FLOW_GEN=preview`): a generation seam that
//! answers in-process, with no hub and no network. Pictures are procedural
//! test patterns at the requested size, sounds are short chords, vision and
//! OCR answer with a sentence; other domains refuse with a reason. It lets a
//! flow be wired, run, previewed and published end to end on a machine
//! without models, and it is how the studio is verified here.

use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson};
use makepad_ai_hub::registry::Domain;
use makepad_ai_hub::job::{FleetRouter, GenPick, GenRouter};
use makepad_widgets::makepad_micro_serde::DeJson;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a preview job "runs", so progress can be seen.
const RUN_TIME: Duration = Duration::from_millis(1200);
/// Longest preview sound; a preview song is a sketch, not the song.
const MAX_SECONDS: f64 = 8.0;
/// Largest preview picture side.
const MAX_SIDE: u32 = 1024;
const SAMPLE_RATE: u32 = 24_000;

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

pub struct PreviewGen;

impl GenRouter for PreviewGen {
    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        GenPick::only(|| Ok(Box::new(PreviewProvider::default())), request, excluded)
    }
}

/// The LAN's AI Hub nodes first, as the fleet routes, but only nodes that
/// have the model installed: a job never makes a node download weights. A
/// generator that no node offers (no node answers, or none has its model
/// ready) runs on the preview generator instead, and says so in its stage
/// and its `provider_url`. Without `fallback` such a generator fails with
/// the reason instead.
pub struct LanOrPreview {
    pub fallback: bool,
}

/// The `provider_url` a preview fallback reports.
pub const PREVIEW_NODE: &str = "preview";

impl GenRouter for LanOrPreview {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        FleetRouter::default().use_idle_admission(domain, request)
    }

    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        let installed = FleetRouter { installed_only: true };
        let snapshots = makepad_ai_hub::job::fleet_snapshots(excluded);
        if makepad_ai_hub::fleet::installed_model(&snapshots, domain, &request.model).is_some() || !excluded.is_empty() || !self.fallback {
            // A node already refused or failed this job: that is the job's
            // answer, never a silent preview.
            return installed.route_snapshots(snapshots, domain, request).map_err(|error| {
                if excluded.is_empty() { error } else { format!("no other LAN node has a {domain} model ready ({error})") }
            });
        }
        // A job the domain's models cannot take is never run, even as a preview.
        let domain_id = Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        if let Some(problem) = makepad_ai_hub::registry::request_problem(domain_id, request) {
            return Err(problem);
        }
        let wanted = if request.model.is_empty() { format!("a {domain} model") } else { request.model.clone() };
        makepad_widgets::log!("[flow-studio] no LAN node has {wanted} installed; running a preview");
        Ok(GenPick {
            provider: Box::new(PreviewProvider::default()),
            base_url: PREVIEW_NODE.to_string(),
            model: "preview".to_string(),
            model_state: None,
        })
    }
}

struct Job {
    started: Instant,
    artifact: Option<(String, Arc<Vec<u8>>)>,
    text: Option<String>,
}

#[derive(Default)]
struct PreviewProvider {
    // Only ever touched by the run lane that owns this provider.
    jobs: Mutex<HashMap<String, Job>>,
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

impl ContentProvider for PreviewProvider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        HealthJson::deserialize_json(&format!(
            "{{\"service\":{},\"version\":\"0.3.0\",\"models_loaded\":[]}}",
            quote(makepad_ai_hub::SERVICE_NAME)
        )).map_err(|error| AssetAiError::Backend(format!("{error:?}")))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Ok(Vec::new())
    }

    fn request(&self, domain: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
        let prompt = request.prompt.clone().or_else(|| request.text.clone()).unwrap_or_default();
        let seed = request.seed.unwrap_or(0);
        let (artifact, text) = match domain.as_str() {
            "image" | "edit" | "upscale" | "control" | "inpaint" | "matte" | "depth" | "segment" => {
                let width = request.width.unwrap_or(512).clamp(16, MAX_SIDE);
                let height = request.height.unwrap_or(512).clamp(16, MAX_SIDE);
                let pixels = pattern(&prompt, seed, width, height);
                let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, width as usize, height as usize)?;
                (Some(("image/png".to_string(), Arc::new(png))), None)
            }
            "audio" | "music" | "speech" => {
                let seconds = match domain.as_str() {
                    "speech" => (prompt.split_whitespace().count() as f64 * 0.32).max(1.0),
                    _ => request.seconds.unwrap_or(4.0),
                }.clamp(0.5, MAX_SECONDS);
                (Some(("audio/wav".to_string(), Arc::new(chord(&prompt, seed, seconds)))), None)
            }
            "vision" | "ocr" | "text" => (None, Some(format!("Preview answer for: {}", prompt.trim()))),
            other => return Err(AssetAiError::Unavailable(format!(
                "the preview generator makes pictures, sounds and text; `{other}` needs an AI Hub"
            ))),
        };
        let id = format!("preview-{}", NEXT_JOB.fetch_add(1, Ordering::Relaxed));
        self.jobs.lock().unwrap_or_else(|error| error.into_inner())
            .insert(id.clone(), Job { started: Instant::now(), artifact, text });
        Ok(id)
    }

    fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
        let job = jobs.get(job_id).ok_or_else(|| AssetAiError::Http(format!("no preview job {job_id}")))?;
        let elapsed = job.started.elapsed();
        let json = if elapsed < RUN_TIME {
            let progress = elapsed.as_secs_f64() / RUN_TIME.as_secs_f64();
            format!("{{\"job_id\":{},\"state\":\"running\",\"stage\":\"preview\",\"progress\":{progress:.3},\"artifacts\":[]}}", quote(job_id))
        } else {
            let artifacts = match &job.artifact {
                Some((content_type, bytes)) => format!(
                    "[{{\"id\":{},\"url\":\"/artifact/{job_id}\",\"content_type\":{},\"byte_len\":{}}}]",
                    quote(job_id), quote(content_type), bytes.len()
                ),
                None => "[]".to_string(),
            };
            let text = job.text.as_deref().map(|text| format!(",\"text\":{}", quote(text))).unwrap_or_default();
            format!("{{\"job_id\":{},\"state\":\"done\",\"artifacts\":{artifacts},\"model\":\"preview\"{text}}}", quote(job_id))
        };
        JobStatusJson::deserialize_json(&json).map_err(|error| AssetAiError::Backend(format!("{error:?}")))
    }

    fn fetch_artifact(&self, artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        let jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
        let (content_type, bytes) = jobs.get(artifact_id).and_then(|job| job.artifact.clone())
            .ok_or_else(|| AssetAiError::Http(format!("no preview artifact {artifact_id}")))?;
        Ok(ArtifactBytes { content_type, bytes: (*bytes).clone() })
    }
}

fn hash(text: &str, seed: u64) -> u64 {
    let mut value = 0xcbf2_9ce4_8422_2325u64 ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for byte in text.bytes() {
        value ^= byte as u64;
        value = value.wrapping_mul(0x0100_0000_01b3);
    }
    value
}

/// RGBA: two hues from the prompt, soft rings about an offset centre and a
/// fine grid, so every prompt and seed reads as a different picture.
pub fn pattern(prompt: &str, seed: u64, width: u32, height: u32) -> Vec<u8> {
    let h = hash(prompt, seed);
    let hue_a = (h & 0xffff) as f32 / 65535.0;
    let hue_b = (hue_a + 0.35 + ((h >> 16) & 0xff) as f32 / 1024.0).fract();
    let (cx, cy) = (0.3 + ((h >> 24) & 0xff) as f32 / 640.0, 0.3 + ((h >> 32) & 0xff) as f32 / 640.0);
    let rings = 5.0 + ((h >> 40) & 0x7) as f32;
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
            let d = ((u - cx).powi(2) + (v - cy).powi(2) * (height as f32 / width as f32).powi(2)).sqrt();
            let ring = (d * rings * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let grid = if (x % 32 == 0) || (y % 32 == 0) { 0.12 } else { 0.0 };
            let a = hsv(hue_a, 0.65, 0.35 + 0.5 * (1.0 - v));
            let b = hsv(hue_b, 0.75, 0.9);
            for channel in 0..3 {
                let value = a[channel] + (b[channel] - a[channel]) * ring * 0.6 + grid;
                out.push((value.clamp(0.0, 1.0) * 255.0) as u8);
            }
            out.push(255);
        }
    }
    out
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// A mono 16-bit WAV: a three-note chord from the prompt, with a fade.
pub fn chord(prompt: &str, seed: u64, seconds: f64) -> Vec<u8> {
    let h = hash(prompt, seed);
    let root = 110.0 * 2f64.powf(((h % 12) as f64) / 12.0);
    let notes = [root, root * 2f64.powf(4.0 / 12.0), root * 2f64.powf(7.0 / 12.0)];
    let frames = (seconds * SAMPLE_RATE as f64) as usize;
    let mut data = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let t = n as f64 / SAMPLE_RATE as f64;
        let envelope = (t / 0.05).min(1.0) * ((seconds - t) / 0.3).clamp(0.0, 1.0);
        let pulse = 0.6 + 0.4 * (t * std::f64::consts::TAU * 2.0).cos().abs();
        let sample: f64 = notes.iter().map(|f| (t * f * std::f64::consts::TAU).sin()).sum::<f64>() / 3.0;
        data.extend_from_slice(&(((sample * envelope * pulse * 0.5) * i16::MAX as f64) as i16).to_le_bytes());
    }
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_job_runs_then_delivers_its_artifact() {
        let provider = PreviewProvider::default();
        let request = GenerateRequestJson { prompt: Some("a lighthouse".into()), width: Some(64), height: Some(32), ..Default::default() };
        let job = provider.request(Domain::parse("image").unwrap(), &request).unwrap();
        assert_eq!(provider.poll(&job).unwrap().state, "running");
        std::thread::sleep(RUN_TIME);
        let done = provider.poll(&job).unwrap();
        assert_eq!(done.state, "done");
        let artifact = provider.fetch_artifact(&done.artifacts[0].id).unwrap();
        assert!(artifact.bytes.starts_with(b"\x89PNG"));
        makepad_ai_hub::client::verify_artifact_bytes(&artifact.bytes, &done.artifacts[0]).unwrap();
    }

    #[test]
    fn domains_without_a_preview_refuse_with_a_reason() {
        let provider = PreviewProvider::default();
        let error = provider.request(Domain::parse("video").unwrap(), &GenerateRequestJson::default()).unwrap_err();
        assert!(error.to_string().contains("AI Hub"));
    }

    #[test]
    fn preview_sounds_are_short_valid_wavs() {
        let wav = chord("dub", 3, 2.0);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + (2.0 * SAMPLE_RATE as f64) as usize * 2);
    }
}
