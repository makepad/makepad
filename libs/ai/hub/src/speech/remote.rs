//! A speech pipe on another node: the co-located machine node over loopback,
//! or a LAN node found through the beacon listener. Utterance-shaped in v1
//! (one `/generate` job per utterance, polled fast); a streaming session
//! with word partials is the realtime-websocket shape and comes later.

use super::SpeechReach;
use crate::client::{ArtifactBytes, ContentProvider, LocalService};
use crate::job::{process_origin, GenJob, GenOutput, GenPick, GenRouter, JobPoll, Want};
use crate::protocol::{
    GenerateRequestJson, SpeechTimingsJson, TranscriptJson, MODEL_STATE_DOWNLOADING, MODEL_STATE_LOADED,
    MODEL_STATE_READY, SPEECH_TIMINGS_FORMAT,
};
use crate::registry::Domain;
use crate::wav;
use makepad_micro_serde::DeJson;
use makepad_system_speech::{Segment, SpeechAudio, Transcript};
use crate::progress_watch::{ProgressWatch, STALL_AFTER};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Poll cadence for speech jobs: an utterance transcribes in well under a
/// second on a warm node, so the poll interval IS the latency floor.
const POLL: Duration = Duration::from_millis(40);
/// First LAN lookup: beacons come every 2 s, so a fresh listener may need a
/// moment before it has heard anyone.
const FIRST_BEACON_WAIT: Duration = Duration::from_millis(2500);

pub(crate) struct RemotePipe {
    pub base_url: String,
    pub model: String,
}

impl RemotePipe {
    /// The best node in reach serving `domain` with one of `backends`:
    /// loopback nodes first (the machine node is the RAM holder), then the
    /// LAN; a loaded model beats a ready one beats one still arriving.
    pub(crate) fn find(reach: SpeechReach, domain: Domain, backends: &[&str]) -> Option<RemotePipe> {
        let mut best: Option<((std::cmp::Reverse<u8>, u8), RemotePipe)> = None;
        for url in candidate_urls(reach) {
            let Some((rank, pipe)) = Self::at(&url, domain, backends) else { continue };
            // The hub's tier decides first (this machine, pinned, LAN): a node of a
            // better tier that serves the pipe at all beats a LAN node that merely
            // has it more loaded. Readiness only breaks ties inside a tier.
            let key = (std::cmp::Reverse(crate::hub_order::tier(&url)), rank);
            if best.as_ref().map_or(true, |(k, _)| key > *k) {
                best = Some((key, pipe));
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
        Some((rank, RemotePipe { base_url: base_url.to_string(), model }))
    }

    /// The node's installed voices (`GET /voices`), or the built-in English
    /// catalogue from a node that predates the route.
    pub(crate) fn voices(&self) -> Vec<makepad_system_speech::Voice> {
        use makepad_system_speech::{Voice, VoiceGender};
        match LocalService::new(&self.base_url).list_voices() {
            Ok(list) if !list.is_empty() => list
                .into_iter()
                .map(|v| Voice {
                    id: v.id,
                    name: v.name,
                    language: v.language,
                    gender: match v.gender.as_str() {
                        "female" => VoiceGender::Female,
                        "male" => VoiceGender::Male,
                        _ => VoiceGender::Unknown,
                    },
                    offline: true,
                })
                .collect(),
            _ => super::weights::kokoro_voice_catalogue(),
        }
    }

    pub(crate) fn transcribe(&self, samples_16k: &[f32], language: &str, prompt: &str, timestamps: bool) -> Result<Transcript, String> {
        let wav = wav::encode_wav_pcm16_mono(samples_16k, super::STT_SAMPLE_RATE);
        let request = GenerateRequestJson {
            model: self.model.clone(),
            input_b64: Some(base64(&wav)),
            input_content_type: Some("audio/wav".to_string()),
            language: Some(language.to_string()),
            prompt: (!prompt.trim().is_empty()).then(|| prompt.to_string()),
            // The wire has no "timestamps" knob; the backend always times its
            // segments and a caller that does not want them ignores them.
            ..Default::default()
        };
        let _ = timestamps;
        let output = self.run_job(Domain::Stt, request, Want::Outputs { media: 1, text: 0 })?;
        let bytes = output.artifacts.into_iter().next().ok_or_else(|| "job finished without an artifact".to_string())?.bytes;
        let text = std::str::from_utf8(&bytes).map_err(|_| "transcript is not utf-8".to_string())?;
        let json = TranscriptJson::deserialize_json(text).map_err(|e| format!("transcript json: {e:?}"))?;
        Ok(Transcript {
            segments: json
                .segments
                .into_iter()
                .map(|s| Segment { start_ms: s.start_ms, end_ms: s.end_ms, text: s.text })
                .collect(),
            language: json.language,
        })
    }

    /// The PCM, and the word timings when the node sent them (a node that
    /// predates them sends the WAV alone).
    pub(crate) fn synthesize(
        &self,
        text: &str,
        voice: &str,
        speed: f32,
    ) -> Result<(SpeechAudio, Option<Vec<super::WordTiming>>), String> {
        let request = GenerateRequestJson {
            model: self.model.clone(),
            text: Some(text.to_string()),
            voice: (!voice.is_empty()).then(|| voice.to_string()),
            speed: Some(speed as f64),
            ..Default::default()
        };
        let artifacts = self.run_job(Domain::Speech, request, Want::All)?.artifacts;
        self.take_from(artifacts)
    }

    fn take_from(&self, artifacts: Vec<ArtifactBytes>) -> Result<(SpeechAudio, Option<Vec<super::WordTiming>>), String> {
        let first = artifacts.first().ok_or_else(|| "job finished without an artifact".to_string())?;
        let (samples, sample_rate) = wav::decode_wav_to_mono_f32(&first.bytes)?;
        // Timings are a nicety: an unreadable sidecar costs the timings, not
        // the take.
        let timings = artifacts
            .iter()
            .skip(1)
            .find(|artifact| artifact.content_type.starts_with("application/json"))
            .and_then(|artifact| std::str::from_utf8(&artifact.bytes).ok())
            .and_then(|text| SpeechTimingsJson::deserialize_json(text).ok())
            .filter(|json| json.format == SPEECH_TIMINGS_FORMAT)
            .map(|json| {
                json.words
                    .into_iter()
                    .map(|w| super::WordTiming { word: w.word, start: w.start, end: w.end })
                    .collect()
            });
        Ok((SpeechAudio { samples, sample_rate }, timings))
    }

    /// [`Self::synthesize`] with `stream: true`: `on_chunk` gets each clause's audio
    /// (mono f32 PCM + rate) as the node publishes it; the return value is the whole
    /// take with its word timings, exactly as the non-streaming call gives it.
    pub(crate) fn synthesize_streaming(
        &self,
        text: &str,
        voice: &str,
        speed: f32,
        on_chunk: &mut dyn FnMut(SpeechAudio),
    ) -> Result<(SpeechAudio, Option<Vec<super::WordTiming>>), String> {
        let request = GenerateRequestJson {
            model: self.model.clone(),
            text: Some(text.to_string()),
            voice: (!voice.is_empty()).then(|| voice.to_string()),
            speed: Some(speed as f64),
            stream: Some(true),
            ..Default::default()
        };
        let output = self.run_job_with(Domain::Speech, request, Want::All, &mut |chunk| {
            if let Ok((samples, sample_rate)) = wav::decode_wav_to_mono_f32(&chunk.bytes) {
                on_chunk(SpeechAudio { samples, sample_rate });
            }
        })?;
        self.take_from(output.artifacts)
    }

    /// Run one job on this node to completion.
    fn run_job(&self, domain: Domain, request: GenerateRequestJson, want: Want) -> Result<GenOutput, String> {
        self.run_job_with(domain, request, want, &mut |_| {})
    }

    fn run_job_with(
        &self,
        domain: Domain,
        request: GenerateRequestJson,
        want: Want,
        on_chunk: &mut dyn FnMut(ArtifactBytes),
    ) -> Result<GenOutput, String> {
        let router = Arc::new(PipeRouter(self.base_url.clone()));
        let mut job = GenJob::new(router, process_origin("speech"));
        job.start(domain, request, want).map_err(|e| format!("{}: {e}", self.base_url))?;
        // A job is lost only when it stops moving, never for taking long: a node
        // that fell over mid-job keeps answering `running` with an unchanged
        // stage until its lease janitor runs. This caller has no one to ask, so
        // it ends the job with an error naming the silence.
        let mut watch = ProgressWatch::new(Instant::now());
        let mut last = None;
        loop {
            let moved = match job.poll() {
                JobPoll::Done(output) => return Ok(output),
                JobPoll::Chunk(chunk) => {
                    on_chunk(chunk);
                    true
                }
                JobPoll::Failed(error) => return Err(format!("{}: {error}", self.base_url)),
                JobPoll::Delta(_) => true,
                JobPoll::Progress { permille, stage } => {
                    let state = Some((permille, stage));
                    let moved = last != state;
                    last = state;
                    moved
                }
                JobPoll::Pending => false,
            };
            let now = Instant::now();
            if moved {
                watch.progress(now);
            } else if watch.stalled(now, STALL_AFTER) {
                job.cancel();
                return Err(format!(
                    "{}: speech job made no progress for {} min",
                    self.base_url,
                    STALL_AFTER.as_secs() / 60
                ));
            }
            std::thread::sleep(POLL);
        }
    }
}

/// This pipe's node and nothing else: it was chosen for its model already.
struct PipeRouter(String);

impl GenRouter for PipeRouter {
    fn route(&self, _domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        if excluded.contains(&self.0) {
            return Err(format!("{} refused the job", self.0));
        }
        Ok(GenPick { provider: Box::new(LocalService::new(&self.0)), base_url: self.0.clone(), model: request.model.clone(), model_state: None })
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
    for pinned in crate::hub_order::pinned_bases() {
        if !urls.contains(&pinned) {
            urls.push(pinned);
        }
    }
    // One ranking for every client: this machine's hub, a pinned hub, the rest
    // sorted; and `find` takes the first of equal readiness, not the last heard.
    crate::hub_order::order(&mut urls);
    urls
}

fn base64(bytes: &[u8]) -> String {
    String::from_utf8(makepad_base64::base64_encode(bytes, &makepad_base64::BASE64_STANDARD))
        .unwrap_or_default()
}
