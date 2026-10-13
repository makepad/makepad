//! The translation worker: one thread that runs chat jobs on the person's own
//! AI hub (the fleet protocol), one at a time, newest wins.
//!
//! Every job is a fresh single-turn conversation, so nothing leaks from one
//! translation into the next. The model named in the settings is strict: the
//! hub node must serve exactly that id, or the job fails with the reason. No
//! other provider exists in this app.

use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole, ProviderAvailability};
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_ai_hub::providers::qwen::{FleetPickCache, FleetQwenChatProvider, HttpFleetTransport};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Which hub, and which model on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubSettings {
    /// `host:port` (or a full URL) of the hub; empty = listen for the fleet.
    pub hub: String,
    /// The exact fleet model id. Never substituted.
    pub model: String,
}

impl Default for HubSettings {
    fn default() -> Self {
        // The phone's own models when their weights are on the phone.
        let model = if crate::ondevice::available() { crate::ondevice::MODEL } else { "qwen3.8-27b" };
        HubSettings { hub: String::new(), model: model.to_string() }
    }
}

/// `10.0.0.149:8123` -> `https://10.0.0.149:8123`; full URLs are kept.
pub fn hub_base(hub: &str) -> Option<String> {
    let hub = hub.trim().trim_end_matches('/');
    if hub.is_empty() {
        None
    } else if hub.starts_with("http://") || hub.starts_with("https://") {
        Some(hub.to_string())
    } else {
        Some(format!("https://{hub}"))
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub system: String,
    pub text: String,
    pub settings: HubSettings,
    /// Cap on the reply; `None` is the generous default of a text translation.
    pub max_tokens: Option<u32>,
    /// A picture the text asks about (on-device model only, so unread
    /// where the phone's models do not exist).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub image: Option<crate::reader::Frame>,
}

#[derive(Clone, Debug)]
pub enum Update {
    /// Where the job runs: `qwen3.8-27b on 10.0.0.149`.
    Route { id: u64, label: String },
    /// The reply so far (cleaned of any think block by the caller).
    Partial { id: u64, text: String },
    Done { id: u64, text: String },
    Failed { id: u64, error: String },
}

pub struct Engine {
    jobs: Sender<Job>,
    updates: Receiver<Update>,
    latest: Arc<AtomicU64>,
}

impl Engine {
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>) -> Engine {
        let (jobs, job_rx) = channel::<Job>();
        let (update_tx, updates) = channel::<Update>();
        let latest = Arc::new(AtomicU64::new(0));
        let worker_latest = latest.clone();
        std::thread::Builder::new()
            .name("translator-hub".into())
            .spawn(move || worker(job_rx, update_tx, worker_latest, wake))
            .expect("spawn translator worker");
        Engine { jobs, updates, latest }
    }

    /// Run `job`; any job still running is abandoned.
    pub fn submit(&self, job: Job) {
        self.latest.store(job.id, Ordering::SeqCst);
        let _ = self.jobs.send(job);
    }

    /// Abandon whatever runs.
    pub fn cancel(&self, next_id: u64) {
        self.latest.store(next_id, Ordering::SeqCst);
    }

    pub fn poll(&self) -> Vec<Update> {
        self.updates.try_iter().collect()
    }
}

fn worker(jobs: Receiver<Job>, updates: Sender<Update>, latest: Arc<AtomicU64>, wake: Arc<dyn Fn() + Send + Sync>) {
    let picks = Arc::new(FleetPickCache::new());
    let send = |u: Update| {
        let _ = updates.send(u);
        wake();
    };
    while let Ok(mut job) = jobs.recv() {
        // Newest wins: skip jobs that were replaced while queued.
        while let Ok(next) = jobs.try_recv() {
            job = next;
        }
        if latest.load(Ordering::SeqCst) != job.id {
            continue;
        }
        #[cfg(target_os = "android")]
        if job.settings.model == crate::ondevice::MODEL {
            local::LOCAL.lock().unwrap_or_else(|e| e.into_inner()).run(&job, &latest, &send);
            continue;
        }
        if job.settings.model == crate::ondevice::MODEL {
            send(Update::Failed { id: job.id, error: "the on-device models run on the phone app only".into() });
            continue;
        }
        run(&job, &picks, &latest, &send);
    }
}

fn bases(settings: &HubSettings) -> Vec<String> {
    if let Some(base) = hub_base(&settings.hub) {
        return vec![base];
    }
    let discovery = makepad_ai_hub::discovery::start_listener();
    let mut bases: Vec<String> = discovery
        .nodes()
        .into_iter()
        .filter(|n| makepad_ai_hub::fleet::role_allows(&n.base_url, "chat"))
        .map(|n| n.base_url)
        .collect();
    bases.extend(
        makepad_ai_hub::machine::read_node_entries()
            .into_iter()
            .filter_map(|(_, e)| (e.port != 0).then(|| format!("http://127.0.0.1:{}", e.port))),
    );
    bases.sort();
    bases.dedup();
    bases
}

fn host_of(base: &str) -> &str {
    let rest = base.trim_start_matches("http://").trim_start_matches("https://");
    rest.split(['/']).next().unwrap_or(rest)
}

fn run(job: &Job, picks: &Arc<FleetPickCache>, latest: &AtomicU64, send: &impl Fn(Update)) {
    let id = job.id;
    let t0 = Instant::now();
    let superseded = || latest.load(Ordering::SeqCst) != id;
    // With no explicit hub the fleet beacons take up to a few seconds to be
    // heard on first use: wait for them, honestly, instead of failing early.
    let started = Instant::now();
    let patience = if hub_base(&job.settings.hub).is_some() { Duration::ZERO } else { Duration::from_secs(6) };
    let (mut provider, route) = loop {
        let bases = bases(&job.settings);
        let reason = if bases.is_empty() {
            "no AI hub found on the network. Set the hub address in settings.".to_string()
        } else {
            let mut provider = FleetQwenChatProvider::with_pick_cache(HttpFleetTransport, bases, picks.clone())
                .with_preferred_model(Some(job.settings.model.clone()))
                .with_thinking(Some(false))
                .with_max_tokens(Some(job.max_tokens.unwrap_or(4096)));
            match provider.availability() {
                ProviderAvailability::Available { model, detail } => {
                    break (provider, format!("{model} on {}", host_of(&detail)));
                }
                ProviderAvailability::Unavailable { reason } => {
                    format!("the hub does not serve {}: {reason}", job.settings.model)
                }
            }
        };
        if superseded() {
            return;
        }
        if started.elapsed() >= patience {
            send(Update::Failed { id, error: reason });
            return;
        }
        std::thread::sleep(Duration::from_millis(400));
    };
    let t_route = t0.elapsed();
    send(Update::Route { id, label: route });
    if job.text.is_empty() {
        // A probe: the hub answered and serves the model.
        send(Update::Done { id, text: String::new() });
        return;
    }
    let input = TurnInput {
        system: job.system.clone(),
        messages: vec![ChatMessage::new(ChatRole::User, job.text.clone())],
        tools_enabled: false,
        dynamic_context: String::new(),
    };
    if let Err(error) = provider.begin_turn(&input) {
        send(Update::Failed { id, error });
        return;
    }
    let t_begun = t0.elapsed();
    let mut first: Option<Duration> = None;
    let mut text = String::new();
    loop {
        if superseded() {
            provider.cancel();
            return;
        }
        for event in provider.poll() {
            match event {
                ProviderEvent::Delta(d) => {
                    first.get_or_insert_with(|| t0.elapsed());
                    text.push_str(&d);
                    send(Update::Partial { id, text: text.clone() });
                }
                ProviderEvent::Done { text: full } => {
                    makepad_widgets::log!("translator: chat job: route {} ms, begun {} ms, first text {} ms, done {} ms", t_route.as_millis(), t_begun.as_millis(), first.map_or(0, |d| d.as_millis()), t0.elapsed().as_millis());
                    send(Update::Done { id, text: if full.is_empty() { text } else { full } });
                    return;
                }
                ProviderEvent::Error(error) => {
                    send(Update::Failed { id, error });
                    return;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// Jobs on the phone's own Qwen (makepad_ai_llm's TurnEngine on Vulkan):
/// loaded at the first on-device job and kept. One model per process: the
/// text screen's and the voice screen's engines share it (each worker had
/// its own and the voice screen loaded a second 3.7 GB copy), one job at a
/// time.
/// The camera is left: free the on-device vision tower's memory.
/// Off the UI thread: the model's lock is held while a job runs.
pub fn release_vision() {
    #[cfg(target_os = "android")]
    std::thread::spawn(|| local::LOCAL.lock().unwrap_or_else(|e| e.into_inner()).release_vision());
}

#[cfg(target_os = "android")]
mod local {
    use super::{Job, Update};
    use makepad_ai_llm::turns::{TurnConfig, TurnEngine, TurnEvent, TurnImage, TurnRequest};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    pub static LOCAL: std::sync::Mutex<Local> = std::sync::Mutex::new(Local { engine: None, label: None });

    pub struct Local {
        engine: Option<TurnEngine>,
        /// `Qwen3.5-4B on this phone (<GPU>)`, once loaded.
        label: Option<String>,
    }

    impl Local {
        fn start(&mut self) -> Result<(), String> {
            if self.label.is_some() {
                return Ok(());
            }
            let (model, mmproj) = crate::ondevice::model_paths()?;
            let mut config = TurnConfig::new(model);
            config.mmproj = mmproj;
            // A picture is ~256 tokens on top of the text.
            config.max_context = 4096;
            let engine = self.engine.get_or_insert_with(|| TurnEngine::start(config, None));
            loop {
                for event in engine.poll() {
                    match event {
                        TurnEvent::Ready { device, load_ms } => {
                            makepad_widgets::log!("translator: on-device Qwen ready on {device} in {load_ms:.0} ms");
                            self.label = Some(format!("Qwen3.5-4B on this phone ({device})"));
                            return Ok(());
                        }
                        TurnEvent::LoadFailed { error } => {
                            self.engine = None;
                            return Err(format!("the on-device model did not load: {error}"));
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// Frees the vision tower (the camera is left); the next picture loads it again.
        pub fn release_vision(&self) {
            if let Some(engine) = &self.engine {
                engine.release_vision();
                makepad_widgets::log!("translator: vision tower released");
            }
        }

        pub fn run(&mut self, job: &Job, latest: &AtomicU64, send: &impl Fn(Update)) {
            let id = job.id;
            if let Err(error) = self.start() {
                send(Update::Failed { id, error });
                return;
            }
            send(Update::Route { id, label: self.label.clone().unwrap_or_default() });
            if job.text.is_empty() && job.image.is_none() {
                // A probe: the model is loaded.
                send(Update::Done { id, text: String::new() });
                return;
            }
            let engine = self.engine.as_ref().unwrap();
            let image = job.image.as_ref().map(|f| TurnImage {
                rgb: f.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
                width: f.width as usize,
                height: f.height as usize,
            });
            engine.submit(TurnRequest {
                id,
                system: job.system.clone(),
                user: job.text.clone(),
                image,
                max_new_tokens: job.max_tokens.unwrap_or(512) as usize,
            });
            loop {
                if latest.load(Ordering::SeqCst) != id {
                    engine.cancel(u64::MAX);
                    return;
                }
                for event in engine.poll() {
                    match event {
                        TurnEvent::Partial { id: turn, text } if turn == id => send(Update::Partial { id, text }),
                        TurnEvent::Done { id: turn, text, stats } if turn == id => {
                            makepad_widgets::log!(
                                "translator: on-device job: vision {:.0} ms, prefill {} tok {:.0} ms, {} tok {:.0} ms",
                                stats.vision_ms, stats.prefill_tokens, stats.prefill_ms, stats.tokens, stats.decode_ms
                            );
                            send(Update::Done { id, text });
                            return;
                        }
                        TurnEvent::Failed { id: turn, error } if turn == id => {
                            send(Update::Failed { id, error });
                            return;
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_addresses_become_urls() {
        assert_eq!(hub_base(""), None);
        assert_eq!(hub_base(" 10.0.0.149:8123 ").as_deref(), Some("https://10.0.0.149:8123"));
        assert_eq!(hub_base("http://127.0.0.1:9000/").as_deref(), Some("http://127.0.0.1:9000"));
    }
}
