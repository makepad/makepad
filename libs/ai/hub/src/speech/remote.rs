//! A speech pipe on another node: the co-located machine node over loopback,
//! or a LAN node found through the beacon listener. Utterance-shaped in v1
//! (one `/generate` job per utterance, polled fast); a streaming session
//! with word partials is the realtime-websocket shape and comes later.

use super::SpeechReach;
use crate::client::{ContentProvider, LocalService};
use crate::protocol::{
    GenerateRequestJson, TranscriptJson, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR,
    MODEL_STATE_DOWNLOADING, MODEL_STATE_LOADED, MODEL_STATE_READY,
};
use crate::registry::Domain;
use crate::wav;
use makepad_micro_serde::DeJson;
use makepad_system_speech::{Segment, SpeechAudio, Transcript};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Poll cadence for speech jobs: an utterance transcribes in well under a
/// second on a warm node, so the poll interval IS the latency floor.
const POLL: Duration = Duration::from_millis(40);
/// A job that has not finished in this long is lost (a node that fell over
/// mid-job keeps answering `running` until its lease janitor runs).
const JOB_TIMEOUT: Duration = Duration::from_secs(180);
/// First LAN lookup: beacons come every 2 s, so a fresh listener may need a
/// moment before it has heard anyone.
const FIRST_BEACON_WAIT: Duration = Duration::from_millis(2500);

pub(crate) struct RemotePipe {
    pub base_url: String,
    pub model: String,
    service: LocalService,
}

impl RemotePipe {
    /// The best node in reach serving `domain` with one of `backends`:
    /// loopback nodes first (the machine node is the RAM holder), then the
    /// LAN; a loaded model beats a ready one beats one still arriving.
    pub(crate) fn find(reach: SpeechReach, domain: Domain, backends: &[&str]) -> Option<RemotePipe> {
        let mut best: Option<(u8, RemotePipe)> = None;
        for url in candidate_urls(reach) {
            let Some((rank, pipe)) = Self::at(&url, domain, backends) else { continue };
            // Loopback wins ties: same rank, no network.
            if best.as_ref().map_or(true, |(r, _)| rank > *r) {
                best = Some((rank, pipe));
            }
        }
        best.map(|(_, pipe)| pipe)
    }

    /// The pipe at one node, with its readiness rank, when it serves one.
    pub(crate) fn at(base_url: &str, domain: Domain, backends: &[&str]) -> Option<(u8, RemotePipe)> {
        let service = LocalService::new(base_url);
        let models = service.list_models().ok()?;
        let mut best: Option<(u8, String)> = None;
        for model in models {
            if !model.available || model.domain != domain.as_str() {
                continue;
            }
            if !backends.iter().any(|b| *b == model.backend) {
                continue;
            }
            let rank = match model.state.as_str() {
                MODEL_STATE_LOADED => 3,
                MODEL_STATE_READY => 2,
                MODEL_STATE_DOWNLOADING => 1,
                // "absent": the node could acquire it, but a first utterance
                // would wait on a 1.6 GB download. Not a serving pipe today.
                _ => continue,
            };
            if best.as_ref().map_or(true, |(r, _)| rank > *r) {
                best = Some((rank, model.id));
            }
        }
        let (rank, model) = best?;
        Some((rank, RemotePipe { base_url: base_url.to_string(), model, service }))
    }

    pub(crate) fn transcribe(&self, samples_16k: &[f32], language: &str, timestamps: bool) -> Result<Transcript, String> {
        let wav = wav::encode_wav_pcm16_mono(samples_16k, super::STT_SAMPLE_RATE);
        let request = GenerateRequestJson {
            model: self.model.clone(),
            input_b64: Some(base64(&wav)),
            input_content_type: Some("audio/wav".to_string()),
            language: Some(language.to_string()),
            // The wire has no "timestamps" knob; the backend always times its
            // segments and a caller that does not want them ignores them.
            ..Default::default()
        };
        let _ = timestamps;
        let bytes = self.run_job(Domain::Stt, &request)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "transcript is not utf-8".to_string())?;
        let json = TranscriptJson::deserialize_json(text).map_err(|e| format!("transcript json: {e:?}"))?;
        Ok(Transcript {
            segments: json
                .segments
                .into_iter()
                .map(|s| Segment { start_ms: s.start_ms, end_ms: s.end_ms, text: s.text })
                .collect(),
        })
    }

    pub(crate) fn synthesize(&self, text: &str, voice: &str, speed: f32) -> Result<SpeechAudio, String> {
        let request = GenerateRequestJson {
            model: self.model.clone(),
            text: Some(text.to_string()),
            voice: (!voice.is_empty()).then(|| voice.to_string()),
            speed: Some(speed as f64),
            ..Default::default()
        };
        let bytes = self.run_job(Domain::Speech, &request)?;
        let (samples, sample_rate) = wav::decode_wav_to_mono_f32(&bytes)?;
        Ok(SpeechAudio { samples, sample_rate })
    }

    fn run_job(&self, domain: Domain, request: &GenerateRequestJson) -> Result<Vec<u8>, String> {
        let job_id = self
            .service
            .request(domain, request)
            .map_err(|e| format!("{}: {e}", self.base_url))?;
        let deadline = Instant::now() + JOB_TIMEOUT;
        loop {
            let status = self
                .service
                .poll(&job_id)
                .map_err(|e| format!("{}: {e}", self.base_url))?;
            match status.state.as_str() {
                JOB_STATE_DONE => {
                    let artifact = status
                        .artifacts
                        .first()
                        .ok_or_else(|| "job finished without an artifact".to_string())?;
                    return self
                        .service
                        .fetch_artifact(&artifact.id)
                        .map(|a| a.bytes)
                        .map_err(|e| format!("{}: {e}", self.base_url));
                }
                JOB_STATE_ERROR => {
                    return Err(status.error.unwrap_or_else(|| "job failed".to_string()));
                }
                JOB_STATE_CANCELLED => return Err("job cancelled".to_string()),
                _ => {}
            }
            if Instant::now() > deadline {
                let _ = self.service.cancel(&job_id);
                return Err(format!("{}: job {job_id} timed out", self.base_url));
            }
            std::thread::sleep(POLL);
        }
    }
}

/// Every node this reach allows, loopback first.
fn candidate_urls(reach: SpeechReach) -> Vec<String> {
    let mut urls = Vec::new();
    if reach >= SpeechReach::Machine {
        for (_, entry) in crate::machine::read_node_entries() {
            if entry.port > 0 {
                urls.push(format!("http://127.0.0.1:{}", entry.port));
            }
        }
    }
    if reach >= SpeechReach::Lan {
        let discovered = crate::discovery::start_listener();
        // Give a brand-new listener one beacon interval to hear the fleet,
        // once per process; afterwards the live set is whatever it is.
        static WAITED: OnceLock<()> = OnceLock::new();
        if discovered.nodes().is_empty() && WAITED.get().is_none() {
            let deadline = Instant::now() + FIRST_BEACON_WAIT;
            while discovered.nodes().is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = WAITED.set(());
        }
        for node in discovered.nodes() {
            let url = node.base_url.trim_end_matches('/').to_string();
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    }
    urls
}

fn base64(bytes: &[u8]) -> String {
    String::from_utf8(makepad_base64::base64_encode(bytes, &makepad_base64::BASE64_STANDARD))
        .unwrap_or_default()
}
