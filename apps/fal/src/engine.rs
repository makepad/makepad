//! The runs: every request this window sends to Fal, as a state machine per
//! run that the network events and one ticking timer drive. Nothing here
//! waits: a request goes out, its answer arrives later as an event, and the
//! tick decides what is due (a status poll, a retry after a refusal).
//!
//! A run is submitted to the queue, its status polled (with the model's log
//! lines) until it is COMPLETED, its result fetched, and the files the result
//! points at downloaded and saved under `~/.makepad/fal/images/<date>/`. The
//! same machine serves a single Generate and every model of a sweep.

use crate::fal::{self, FalError, Media, MediaKind, Phase, QueueUrls};
use crate::store;
use makepad_strict_json::Value;
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Origin {
    Single,
    /// The sweep row this run measures.
    Sweep(usize),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    /// Accepted by Fal? Not yet: the submit is out (or waits to be retried).
    Submitting,
    Queued,
    Running,
    /// COMPLETED: the result is being fetched.
    Fetching,
    /// The result is in; its files are downloading.
    Downloading,
    Done,
    Failed,
    Cancelled,
}

impl Stage {
    pub fn is_live(self) -> bool {
        matches!(self, Stage::Submitting | Stage::Queued | Stage::Running | Stage::Fetching | Stage::Downloading)
    }
}

/// One picture of a result.
pub struct Shot {
    pub media: Media,
    /// The image cache's key for it (unique per run and index).
    pub key: String,
    pub bytes: Option<Arc<[u8]>>,
    pub saved: Option<PathBuf>,
    pub error: Option<String>,
    pub inflight: Option<LiveId>,
    pub next_try: f64,
    pub retry_delay: f64,
}

pub struct Job {
    pub origin: Origin,
    pub endpoint_id: String,
    pub prompt: String,
    pub input: Value,
    pub stage: Stage,
    pub request_id: Option<String>,
    pub urls: Option<QueueUrls>,
    /// Monotonic seconds: the submit went out (the first one, retries included).
    pub t_submit: f64,
    /// Fal accepted the submit (its reply arrived).
    pub t_accepted: Option<f64>,
    /// The first status that said IN_PROGRESS.
    pub t_started: Option<f64>,
    /// The first status that said COMPLETED.
    pub t_completed: Option<f64>,
    /// The result JSON arrived.
    pub t_result: Option<f64>,
    /// Every file downloaded (or given up).
    pub t_files: Option<f64>,
    pub queue_position: Option<u64>,
    /// Fal's own measure of the run, `metrics.inference_time`, in seconds.
    pub inference_time: Option<f64>,
    /// The model's own `timings.inference`, when its output has one.
    pub model_inference_time: Option<f64>,
    pub logs: Vec<String>,
    /// (heading, plain text) of what went wrong.
    pub error: Option<(String, String)>,
    pub result: Option<Value>,
    pub shots: Vec<Shot>,
    /// Files that are not pictures: shown as links.
    pub other_media: Vec<Media>,
    /// A transient note: "rate limited, retrying in 4 s".
    pub note: String,
    pub inflight: Option<LiveId>,
    pub next_poll: f64,
    pub retry_delay: f64,
    pub cancel_sent: bool,
    /// The run completed but its result was not handed over: Fetch again.
    pub refetchable: bool,
    pub date: String,
    pub unix: u64,
}

impl Job {
    pub fn elapsed(&self, now: f64) -> f64 {
        let end = self.t_files.or(if self.stage.is_live() { None } else { self.t_result.or(self.t_completed) });
        end.unwrap_or(now) - self.t_submit
    }

    /// Submit to IN_PROGRESS. When the run went straight from the queue to
    /// COMPLETED between two polls, the wait is what's left of the time to
    /// COMPLETED after Fal's inference time.
    pub fn queue_wait(&self) -> Option<f64> {
        if let Some(started) = self.t_started {
            return Some(started - self.t_submit);
        }
        let completed = self.t_completed?;
        let inference = self.inference_time?;
        Some((completed - self.t_submit - inference).max(0.0))
    }

    /// Fal's inference time when it reports one, else IN_PROGRESS to COMPLETED as seen.
    pub fn generation_time(&self) -> Option<f64> {
        self.inference_time.or(self.model_inference_time).or_else(|| Some(self.t_completed? - self.t_started?))
    }

    /// Submit to the result fetched.
    pub fn end_to_end(&self) -> Option<f64> {
        Some(self.t_result? - self.t_submit)
    }

    pub fn status_line(&self, now: f64) -> String {
        let elapsed = format!("{:.1} s", self.elapsed(now));
        let what = match self.stage {
            Stage::Submitting => "submitting".to_string(),
            Stage::Queued => match self.queue_position {
                Some(0) => "in queue, next up".to_string(),
                Some(n) => format!("in queue, position {n}"),
                None => "in queue".to_string(),
            },
            Stage::Running => "generating".to_string(),
            Stage::Fetching => "fetching the result".to_string(),
            Stage::Downloading => {
                let have = self.shots.iter().filter(|s| s.bytes.is_some() || s.error.is_some()).count();
                format!("downloading {have}/{}", self.shots.len())
            }
            Stage::Done => "done".to_string(),
            Stage::Failed => "failed".to_string(),
            Stage::Cancelled => "cancelled".to_string(),
        };
        if self.note.is_empty() {
            format!("{what} · {elapsed}")
        } else {
            format!("{what} · {elapsed} · {}", self.note)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ReqKind {
    Submit,
    Status,
    Result,
    Cancel,
    File(usize),
}

/// A finished run the app should react to (a sweep row to record).
pub enum JobEvent {
    Finished(usize),
}

#[derive(Default)]
pub struct Engine {
    pub jobs: Vec<Job>,
    requests: HashMap<LiveId, (usize, ReqKind)>,
    /// Bumped on every visible change, so views redraw only then.
    pub generation: u64,
    pub events: Vec<JobEvent>,
}

/// How long to wait before the next status poll: quick while a run is young
/// (latency is what a sweep measures), easing off for long runs. Polling never
/// stops while the run is alive.
fn poll_interval(age: f64) -> f64 {
    if age < 15.0 {
        0.25
    } else {
        (0.25 + (age - 15.0) * 0.05).min(2.0)
    }
}

fn next_retry_delay(delay: f64) -> f64 {
    if delay <= 0.0 {
        1.0
    } else {
        (delay * 2.0).min(30.0)
    }
}

impl Engine {
    pub fn any_live(&self) -> bool {
        self.jobs.iter().any(|job| job.stage.is_live())
    }

    /// Start a run now. Returns its index.
    pub fn start(&mut self, cx: &mut Cx, key: &str, origin: Origin, endpoint_id: String, prompt: String, input: Value) -> usize {
        let now = Cx::monotonic_now();
        let (unix, date, _) = store::now_stamps();
        let index = self.jobs.len();
        self.jobs.push(Job {
            origin,
            endpoint_id,
            prompt,
            input,
            stage: Stage::Submitting,
            request_id: None,
            urls: None,
            t_submit: now,
            t_accepted: None,
            t_started: None,
            t_completed: None,
            t_result: None,
            t_files: None,
            queue_position: None,
            inference_time: None,
            model_inference_time: None,
            logs: Vec::new(),
            error: None,
            result: None,
            shots: Vec::new(),
            other_media: Vec::new(),
            note: String::new(),
            inflight: None,
            next_poll: now,
            retry_delay: 0.0,
            cancel_sent: false,
            refetchable: false,
            date,
            unix,
        });
        self.send_submit(cx, key, index);
        self.generation += 1;
        index
    }

    fn send_submit(&mut self, cx: &mut Cx, key: &str, index: usize) {
        let job = &mut self.jobs[index];
        let request = fal::submit_request(key, &job.endpoint_id, &job.input);
        let id = LiveId::unique();
        cx.http_request(id, request);
        job.inflight = Some(id);
        self.requests.insert(id, (index, ReqKind::Submit));
    }

    fn send(&mut self, cx: &mut Cx, index: usize, kind: ReqKind, request: HttpRequest) -> LiveId {
        let id = LiveId::unique();
        cx.http_request(id, request);
        self.requests.insert(id, (index, kind));
        id
    }

    /// Ask Fal to cancel a run (the API supports it while queued or running).
    pub fn cancel(&mut self, cx: &mut Cx, key: &str, index: usize) {
        let Some(job) = self.jobs.get_mut(index) else { return };
        if !job.stage.is_live() || job.cancel_sent {
            return;
        }
        match job.urls.as_ref() {
            Some(urls) => {
                let request = fal::cancel_request(key, &urls.cancel);
                job.cancel_sent = true;
                job.note = "cancelling".to_string();
                self.send(cx, index, ReqKind::Cancel, request);
            }
            None => {
                // Not accepted yet: forget the submit's answer when it comes,
                // and cancel what it created then.
                job.cancel_sent = true;
                job.note = "cancelling once Fal accepts it".to_string();
            }
        }
        self.generation += 1;
    }

    /// The timer's tick: send what is due.
    pub fn tick(&mut self, cx: &mut Cx, key: Option<&str>) {
        let now = Cx::monotonic_now();
        let mut changed = false;
        for index in 0..self.jobs.len() {
            let job = &self.jobs[index];
            if !job.stage.is_live() {
                continue;
            }
            // Downloads have their own schedule.
            if job.stage == Stage::Downloading {
                let due: Vec<usize> = job
                    .shots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.bytes.is_none() && s.error.is_none() && s.inflight.is_none() && now >= s.next_try)
                    .map(|(i, _)| i)
                    .collect();
                for shot in due {
                    let url = self.jobs[index].shots[shot].media.url.clone();
                    let id = self.send(cx, index, ReqKind::File(shot), fal::media_request(&url));
                    self.jobs[index].shots[shot].inflight = Some(id);
                }
                continue;
            }
            if job.inflight.is_some() || now < job.next_poll {
                continue;
            }
            let Some(key) = key else {
                continue;
            };
            match job.stage {
                Stage::Submitting => {
                    self.jobs[index].note.clear();
                    self.send_submit(cx, key, index);
                    changed = true;
                }
                Stage::Queued | Stage::Running => {
                    let Some(urls) = job.urls.as_ref() else { continue };
                    let request = fal::status_request(key, &urls.status);
                    let id = self.send(cx, index, ReqKind::Status, request);
                    self.jobs[index].inflight = Some(id);
                }
                Stage::Fetching => {
                    let Some(urls) = job.urls.as_ref() else { continue };
                    let request = fal::result_request(key, &urls.response);
                    let id = self.send(cx, index, ReqKind::Result, request);
                    self.jobs[index].inflight = Some(id);
                }
                _ => {}
            }
        }
        if changed {
            self.generation += 1;
        }
    }

    /// A network event. Returns true when it was one of ours.
    pub fn handle_response(&mut self, cx: &mut Cx, key: Option<&str>, response: &NetworkResponse) -> bool {
        let (request_id, reply): (LiveId, Result<&HttpResponse, String>) = match response {
            NetworkResponse::HttpResponse { request_id, response } => (*request_id, Ok(response)),
            NetworkResponse::HttpError { request_id, error } => (*request_id, Err(error.message.clone())),
            _ => return false,
        };
        let Some((index, kind)) = self.requests.remove(&request_id) else {
            return false;
        };
        let now = Cx::monotonic_now();
        match kind {
            ReqKind::Submit => self.on_submit(cx, key, index, now, reply),
            ReqKind::Status => self.on_status(index, now, reply),
            ReqKind::Result => self.on_result(cx, index, now, reply),
            ReqKind::Cancel => self.on_cancel(index, reply),
            ReqKind::File(shot) => self.on_file(cx, index, shot, now, reply),
        }
        self.generation += 1;
        true
    }

    fn fail(&mut self, index: usize, error: FalError) {
        let job = &mut self.jobs[index];
        job.error = Some((error.title().to_string(), error.message()));
        job.stage = if matches!(error, FalError::Cancelled(_)) { Stage::Cancelled } else { Stage::Failed };
        job.inflight = None;
        job.note.clear();
        self.events.push(JobEvent::Finished(index));
    }

    /// A refusal worth waiting out (429, a 5xx, a dropped connection): try
    /// again later, backing off, for as long as the run is wanted.
    fn back_off(&mut self, index: usize, now: f64, error: &FalError) {
        let job = &mut self.jobs[index];
        job.retry_delay = match error {
            FalError::RateLimited { retry_after: Some(after), .. } => after.max(next_retry_delay(job.retry_delay)),
            _ => next_retry_delay(job.retry_delay),
        };
        job.next_poll = now + job.retry_delay;
        job.inflight = None;
        let detail: String = error.message().chars().take(160).collect();
        job.note = format!("{} ({detail}), retrying in {:.0} s", error.title().to_lowercase(), job.retry_delay);
    }

    fn on_submit(&mut self, cx: &mut Cx, key: Option<&str>, index: usize, now: f64, reply: Result<&HttpResponse, String>) {
        self.jobs[index].inflight = None;
        let endpoint_id = self.jobs[index].endpoint_id.clone();
        let submitted = match reply {
            Ok(response) => fal::parse_submit(&endpoint_id, response),
            Err(message) => Err(FalError::Network(message)),
        };
        match submitted {
            Ok(submitted) => {
                let job = &mut self.jobs[index];
                job.request_id = Some(submitted.request_id);
                job.t_accepted = Some(now);
                job.queue_position = submitted.queue_position;
                job.urls = Some(submitted.urls);
                job.stage = Stage::Queued;
                job.retry_delay = 0.0;
                job.note.clear();
                job.next_poll = now + 0.2;
                if job.cancel_sent {
                    job.cancel_sent = false;
                    if let Some(key) = key {
                        self.cancel(cx, key, index);
                    }
                }
            }
            // Rate limits and server trouble are waited out. A dropped
            // connection on submit is not retried: Fal may have queued (and
            // will charge for) a run whose answer was lost.
            Err(error) if error.retryable() && !matches!(error, FalError::Network(_)) && !self.jobs[index].cancel_sent => {
                self.back_off(index, now, &error);
            }
            Err(error) => self.fail(index, error),
        }
    }

    fn on_status(&mut self, index: usize, now: f64, reply: Result<&HttpResponse, String>) {
        self.jobs[index].inflight = None;
        let status = match reply {
            Ok(response) => fal::parse_status(response),
            Err(message) => Err(FalError::Network(message)),
        };
        match status {
            Ok(status) => {
                let job = &mut self.jobs[index];
                job.retry_delay = 0.0;
                if !job.cancel_sent {
                    job.note.clear();
                }
                if !status.logs.is_empty() {
                    job.logs = status
                        .logs
                        .iter()
                        .map(|line| match line.timestamp.get(11..19) {
                            Some(time) => format!("{time} {}", line.message),
                            None => line.message.clone(),
                        })
                        .collect();
                }
                match status.phase {
                    Phase::InQueue { position } => {
                        job.stage = Stage::Queued;
                        job.queue_position = position;
                    }
                    Phase::InProgress => {
                        if job.t_started.is_none() {
                            job.t_started = Some(now);
                        }
                        job.stage = Stage::Running;
                    }
                    Phase::Completed => {
                        job.t_completed = Some(now);
                        job.inference_time = status.inference_time;
                        if let Some(error) = status.error {
                            let error = FalError::Failed { error, error_type: status.error_type };
                            self.fail(index, error);
                            return;
                        }
                        job.stage = Stage::Fetching;
                        job.next_poll = now;
                        return;
                    }
                }
                let age = now - job.t_submit;
                job.next_poll = now + poll_interval(age);
            }
            Err(error) if error.retryable() => self.back_off(index, now, &error),
            Err(error) => self.fail(index, error),
        }
    }

    fn on_result(&mut self, cx: &mut Cx, index: usize, now: f64, reply: Result<&HttpResponse, String>) {
        self.jobs[index].inflight = None;
        let result = match reply {
            Ok(response) => fal::parse_result(response),
            Err(message) => Err(FalError::Network(message)),
        };
        match result {
            Ok(value) => {
                let job = &mut self.jobs[index];
                job.t_result = Some(now);
                job.model_inference_time = fal::result_inference_time(&value);
                job.note.clear();
                let request = job.request_id.clone().unwrap_or_default();
                for (n, media) in fal::collect_media(&value).into_iter().enumerate() {
                    if media.kind == MediaKind::Image {
                        let ext = store::image_extension(media.content_type.as_deref(), &media.url);
                        let mut shot = Shot {
                            key: format!("fal/{request}/{n}.{ext}"),
                            media,
                            bytes: None,
                            saved: None,
                            error: None,
                            inflight: None,
                            next_try: now,
                            retry_delay: 0.0,
                        };
                        // `sync_mode` puts the file in the result itself.
                        if shot.media.url.starts_with("data:") {
                            match fal::decode_data_uri(&shot.media.url) {
                                Some((_, bytes)) => shot.bytes = Some(Arc::from(bytes)),
                                None => shot.error = Some("the inline image could not be decoded".to_string()),
                            }
                        }
                        job.shots.push(shot);
                    } else {
                        job.other_media.push(media);
                    }
                }
                job.result = Some(value);
                job.stage = Stage::Downloading;
                self.save_record(cx, index);
                self.check_files_done(cx, index, now);
            }
            // Asked a moment too early: poll again.
            Err(FalError::StillRunning) => {
                let job = &mut self.jobs[index];
                job.stage = Stage::Running;
                job.next_poll = now + 0.25;
            }
            Err(error) if error.transient() => self.back_off(index, now, &error),
            // Fal said COMPLETED and then fails to hand the result over (it
            // answers 500 every time for some apps): a visible failure the
            // person can fetch again, never an endless wait.
            Err(FalError::Server { status, detail, .. }) => {
                self.jobs[index].refetchable = true;
                let error = FalError::ResultUnavailable(format!("Fal finished the run but answers HTTP {status} ({detail}) for its result. Fetch again to retry."));
                self.fail(index, error);
            }
            Err(error) => self.fail(index, error),
        }
    }

    /// Ask again for the result of a run that completed but whose result
    /// Fal did not hand over.
    pub fn refetch(&mut self, index: usize) {
        let Some(job) = self.jobs.get_mut(index) else { return };
        if !job.refetchable || job.stage != Stage::Failed {
            return;
        }
        job.refetchable = false;
        job.error = None;
        job.stage = Stage::Fetching;
        job.next_poll = Cx::monotonic_now();
        job.retry_delay = 0.0;
        job.note = "fetching the result again".to_string();
        self.generation += 1;
    }

    fn on_cancel(&mut self, index: usize, reply: Result<&HttpResponse, String>) {
        let outcome = match reply {
            Ok(response) => fal::parse_cancel(response),
            Err(message) => Err(FalError::Network(message)),
        };
        let job = &mut self.jobs[index];
        match outcome {
            Ok(fal::CancelOutcome::Removed) => {
                job.stage = Stage::Cancelled;
                job.note.clear();
                job.error = Some(("Cancelled".to_string(), "Removed from Fal's queue before it ran.".to_string()));
                self.events.push(JobEvent::Finished(index));
            }
            Ok(fal::CancelOutcome::Requested) => {
                job.note = "cancel requested; it may still finish".to_string();
            }
            Ok(fal::CancelOutcome::AlreadyCompleted) => {
                job.note = "already finished".to_string();
            }
            Ok(fal::CancelOutcome::NotFound) => {
                job.note = "Fal does not know this request".to_string();
            }
            Err(error) => {
                job.cancel_sent = false;
                job.note = format!("cancel failed: {}", error.message());
            }
        }
    }

    fn on_file(&mut self, cx: &mut Cx, index: usize, shot: usize, now: f64, reply: Result<&HttpResponse, String>) {
        {
            let Some(s) = self.jobs[index].shots.get_mut(shot) else { return };
            s.inflight = None;
            match reply {
                Ok(response) if (200..300).contains(&response.status_code) => match response.body.as_ref() {
                    Some(body) if !body.is_empty() => s.bytes = Some(body.clone()),
                    _ => s.error = Some("the file came back empty".to_string()),
                },
                Ok(response) => {
                    let error = fal::error_from_reply(response);
                    if error.transient() {
                        s.retry_delay = next_retry_delay(s.retry_delay);
                        s.next_try = now + s.retry_delay;
                    } else {
                        s.error = Some(format!("{}: {}", error.title(), error.message()));
                    }
                }
                Err(message) => {
                    s.retry_delay = next_retry_delay(s.retry_delay);
                    s.next_try = now + s.retry_delay;
                    self.jobs[index].note = format!("download failed ({message}), retrying");
                }
            }
        }
        self.check_files_done(cx, index, now);
    }

    fn check_files_done(&mut self, cx: &mut Cx, index: usize, now: f64) {
        let job = &mut self.jobs[index];
        if job.stage != Stage::Downloading || job.shots.iter().any(|s| s.bytes.is_none() && s.error.is_none()) {
            return;
        }
        job.t_files = Some(now);
        job.stage = Stage::Done;
        job.note.clear();
        if let Some(dir) = store::images_dir(&job.date) {
            let request = job.request_id.clone().unwrap_or_default();
            let mut files = Vec::new();
            for (n, shot) in job.shots.iter_mut().enumerate() {
                if let Some(bytes) = shot.bytes.as_ref() {
                    let ext = store::image_extension(shot.media.content_type.as_deref(), &shot.media.url);
                    let path = dir.join(format!("{request}-{n}.{ext}"));
                    shot.saved = Some(path.clone());
                    files.push(store::FileWrite { path, bytes: bytes.clone() });
                }
            }
            if !files.is_empty() {
                store::write_files(cx, files);
            }
        }
        self.events.push(JobEvent::Finished(index));
    }

    /// The sidecar beside a run's pictures: what was asked and what came back.
    fn save_record(&mut self, cx: &mut Cx, index: usize) {
        let job = &self.jobs[index];
        let Some(dir) = store::images_dir(&job.date) else { return };
        let request = job.request_id.clone().unwrap_or_default();
        let record = makepad_strict_json::obj(vec![
            ("endpoint_id", makepad_strict_json::s(job.endpoint_id.clone())),
            ("request_id", makepad_strict_json::s(request.clone())),
            ("prompt", makepad_strict_json::s(job.prompt.clone())),
            ("unix_time", Value::Int(job.unix as i64)),
            ("input", job.input.clone()),
            ("result", job.result.clone().unwrap_or(Value::Null)),
        ]);
        let bytes: Arc<[u8]> = Arc::from(fal::pretty(&record).into_bytes());
        store::write_files(cx, vec![store::FileWrite { path: dir.join(format!("{request}.json")), bytes }]);
    }
}
