//! The sweep: one picture from every text-to-image model Fal lists, the same
//! prompt and seed for each, timed, so the fastest models stand out.
//!
//! Preparing it costs nothing: the model list comes from Fal's model search
//! (`GET api.fal.ai/v1/models?category=text-to-image`, each model's OpenAPI
//! schema expanded, which says what inputs it requires), and the unit prices
//! from Fal's pricing API, turned into a cost per picture at the sweep's size.
//! The person then confirms once, seeing the count and the estimated total.
//! The run sends a few models at a time, so Fal's own queue does not inflate
//! the numbers, and waits out refusals instead of failing.
//! Every finished model rewrites `~/.makepad/fal/sweeps/<date-time>.json`.

use crate::engine::{Engine, Job, Origin, Stage};
use crate::fal::{self, FalError, InputSchema, Price};
use crate::store;
use makepad_strict_json::{obj, s, Value};
use makepad_widgets::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

/// Models in flight at once.
pub const CONCURRENCY: usize = 3;
/// The pricing API takes up to 50 endpoint ids per call.
const PRICE_BATCH: usize = 50;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Idle,
    Listing,
    Pricing,
    Ready,
    Running,
    Stopped,
    Finished,
}

pub struct Row {
    pub endpoint_id: String,
    pub display_name: String,
    /// The request this model gets, or why it gets none.
    pub input: Result<Value, String>,
    pub price: Option<Price>,
    pub job: Option<usize>,
    /// The model refused the sweep's size and ran again at its own default.
    pub own_size: bool,
}

/// A catalog call: the model list, or the prices of a set of models (Fal
/// answers 404 for the whole set when one id has no price, so a refused set
/// is split until the unpriced ones stand alone).
#[derive(Clone, Debug)]
enum Req {
    Models,
    Pricing(Vec<String>),
}

/// The cost shown before the confirm: one picture from each runnable model
/// at the sweep's size, from Fal's unit prices.
#[derive(Clone, Copy, Debug)]
pub struct Estimate {
    pub usd: f64,
    pub priced: usize,
    /// Models billed in other units (seconds, tokens, units) or unpriced.
    pub unpriced: usize,
}

pub struct Sweep {
    pub phase: Phase,
    pub rows: Vec<Row>,
    pub prompt: String,
    pub seed: i64,
    pub message: String,
    pub error: Option<String>,
    /// The estimate the person confirmed (kept in the sweep's file).
    pub confirmed: Option<Estimate>,
    cursor: Option<String>,
    requests: HashMap<LiveId, Req>,
    /// Catalog calls go one at a time (Fal's platform API rate-limits
    /// bursts): the ones waiting their turn.
    queue: VecDeque<Req>,
    retries: Vec<(f64, Req)>,
    /// Rows to run again (at the model's own size), before the next new one.
    again: VecDeque<usize>,
    retry_delay: f64,
    pricing_pending: usize,
    pub file: Option<PathBuf>,
    started_unix: u64,
    next_row: usize,
    pub generation: u64,
}

impl Default for Sweep {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            rows: Vec::new(),
            prompt: String::new(),
            seed: 42,
            message: String::new(),
            error: None,
            confirmed: None,
            cursor: None,
            requests: HashMap::new(),
            queue: VecDeque::new(),
            retries: Vec::new(),
            again: VecDeque::new(),
            retry_delay: 0.0,
            pricing_pending: 0,
            file: None,
            started_unix: 0,
            next_row: 0,
            generation: 0,
        }
    }
}

impl Sweep {
    pub fn runnable(&self) -> usize {
        self.rows.iter().filter(|row| row.input.is_ok()).count()
    }

    /// What one picture from every runnable model costs at the sweep's size
    /// by Fal's unit prices (a finished model's at the size it came back at).
    pub fn estimate(&self, engine: &Engine) -> Estimate {
        let mut estimate = Estimate { usd: 0.0, priced: 0, unpriced: 0 };
        for row in self.rows.iter().filter(|row| row.input.is_ok()) {
            match cost_per_image(row, row.job.map(|job| &engine.jobs[job])) {
                Some(cost) => {
                    estimate.usd += cost;
                    estimate.priced += 1;
                }
                None => estimate.unpriced += 1,
            }
        }
        estimate
    }

    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Listing | Phase::Pricing | Phase::Running)
    }

    /// Fetch the model list (and then the prices). Free: nothing is generated.
    pub fn load(&mut self, cx: &mut Cx, key: &str, prompt: String, seed: i64) {
        self.rows.clear();
        self.requests.clear();
        self.confirmed = None;
        self.pricing_pending = 0;
        self.error = None;
        self.cursor = None;
        self.prompt = prompt;
        self.seed = seed;
        self.next_row = 0;
        self.file = None;
        self.retries.clear();
        self.queue.clear();
        self.again.clear();
        self.retry_delay = 0.0;
        self.phase = Phase::Listing;
        self.message = "Asking Fal for its text-to-image models…".to_string();
        self.send(cx, Some(key), Req::Models);
        self.generation += 1;
    }

    /// Queue a catalog call; it goes out when the one before it is answered.
    fn send(&mut self, cx: &mut Cx, key: Option<&str>, req: Req) {
        self.queue.push_back(req);
        self.pump(cx, key);
    }

    fn pump(&mut self, cx: &mut Cx, key: Option<&str>) {
        if self.requests.is_empty() && self.retries.is_empty() {
            if let Some(req) = self.queue.pop_front() {
                self.dispatch(cx, key, req);
            }
        }
    }

    fn dispatch(&mut self, cx: &mut Cx, key: Option<&str>, req: Req) {
        let request = match &req {
            Req::Models => fal::models_request(key, "text-to-image", self.cursor.as_deref()),
            Req::Pricing(ids) => fal::pricing_request(key.unwrap_or(""), ids),
        };
        let id = LiveId::unique();
        cx.http_request(id, request);
        self.requests.insert(id, req);
    }

    /// Start spending: the one confirm press, on the estimate it showed.
    pub fn run(&mut self, estimate: Estimate) {
        if self.phase != Phase::Ready && self.phase != Phase::Stopped {
            return;
        }
        self.confirmed.get_or_insert(estimate);
        let (unix, _, stamp) = store::now_stamps();
        self.started_unix = unix;
        self.file = store::sweeps_dir().map(|dir| dir.join(format!("{stamp}.json")));
        self.phase = Phase::Running;
        self.generation += 1;
    }

    /// Send no more models; the ones in flight finish and are recorded.
    pub fn stop(&mut self) {
        if self.phase == Phase::Running {
            self.phase = Phase::Stopped;
            self.generation += 1;
        }
    }

    /// The timer's tick: retry a refused catalog call, start the next models.
    pub fn tick(&mut self, cx: &mut Cx, key: Option<&str>, engine: &mut Engine) {
        let now = Cx::monotonic_now();
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retries).into_iter().partition(|(at, _)| now >= *at);
        self.retries = later;
        for (_, req) in due {
            self.dispatch(cx, key, req);
        }
        if self.phase != Phase::Running {
            return;
        }
        let Some(key) = key else { return };
        let live = self
            .rows
            .iter()
            .filter_map(|row| row.job)
            .filter(|&job| engine.jobs[job].stage.is_live())
            .count();
        let mut room = CONCURRENCY.saturating_sub(live);
        while room > 0 && (!self.again.is_empty() || self.next_row < self.rows.len()) {
            let index = match self.again.pop_front() {
                Some(index) => index,
                None => {
                    self.next_row += 1;
                    self.next_row - 1
                }
            };
            let row = &mut self.rows[index];
            let Ok(input) = row.input.clone() else { continue };
            let job = engine.start(cx, key, Origin::Sweep(index), row.endpoint_id.clone(), self.prompt.clone(), input);
            row.job = Some(job);
            room -= 1;
            self.generation += 1;
        }
        if self.next_row >= self.rows.len() && self.again.is_empty() && live == 0 && room == CONCURRENCY {
            self.phase = Phase::Finished;
            self.message = "Sweep finished.".to_string();
            self.generation += 1;
            self.save(cx, engine);
        }
    }

    pub fn handle_response(&mut self, cx: &mut Cx, key: Option<&str>, response: &NetworkResponse) -> bool {
        let (request_id, reply): (LiveId, Result<&HttpResponse, String>) = match response {
            NetworkResponse::HttpResponse { request_id, response } => (*request_id, Ok(response)),
            NetworkResponse::HttpError { request_id, error } => (*request_id, Err(error.message.clone())),
            _ => return false,
        };
        let Some(req) = self.requests.remove(&request_id) else {
            return false;
        };
        let now = Cx::monotonic_now();
        self.generation += 1;
        match req {
            Req::Models => {
                let page = match reply {
                    Ok(response) => fal::parse_models_page(response),
                    Err(message) => Err(FalError::Network(message)),
                };
                match page {
                    Ok(page) => {
                        self.retry_delay = 0.0;
                        for model in page.models {
                            // A `/stream` endpoint answers with server-sent events, not
                            // a queued result: the sweep times queued runs only.
                            let input = if model.endpoint_id.ends_with("/stream") {
                                Err("streams its answer (the sweep times queued runs)".to_string())
                            } else {
                                sweep_input(model.input.as_ref(), &self.prompt, self.seed)
                            };
                            self.rows.push(Row {
                                endpoint_id: model.endpoint_id,
                                display_name: model.display_name,
                                input,
                                price: None,
                                job: None,
                                own_size: false,
                            });
                        }
                        self.message = format!("Found {} text-to-image models so far…", self.rows.len());
                        if page.has_more && page.next_cursor.is_some() {
                            self.cursor = page.next_cursor;
                            self.send(cx, key, Req::Models);
                        } else {
                            self.start_pricing(cx, key);
                        }
                    }
                    Err(error) => self.refused(now, Req::Models, error),
                }
            }
            Req::Pricing(ids) => {
                let prices = match reply {
                    Ok(response) => fal::parse_pricing(response),
                    Err(message) => Err(FalError::Network(message)),
                };
                match prices {
                    Ok(prices) => {
                        self.retry_delay = 0.0;
                        for price in prices {
                            if let Some(row) = self.rows.iter_mut().find(|row| row.endpoint_id == price.endpoint_id) {
                                row.price = Some(price);
                            }
                        }
                        self.pricing_done(cx, key);
                    }
                    Err(FalError::NotFound(_)) if ids.len() > 1 => self.split(cx, key, ids),
                    // One model without a price: it runs, unpriced.
                    Err(FalError::NotFound(_)) => self.pricing_done(cx, key),
                    Err(error) if error.retryable() => self.refused(now, Req::Pricing(ids), error),
                    // Prices are a courtesy: without them the sweep still runs.
                    Err(error) => {
                        self.error = Some(format!("Some prices are unavailable: {}", error.message()));
                        self.pricing_done(cx, key);
                    }
                }
            }
        }
        self.pump(cx, key);
        true
    }

    /// A set Fal refused as a whole: ask for each half on its own.
    fn split(&mut self, cx: &mut Cx, key: Option<&str>, mut ids: Vec<String>) {
        let half = ids.split_off(ids.len() / 2);
        self.pricing_pending += 1;
        self.send(cx, key, Req::Pricing(ids));
        self.send(cx, key, Req::Pricing(half));
    }

    fn pricing_done(&mut self, _cx: &mut Cx, _key: Option<&str>) {
        self.pricing_pending = self.pricing_pending.saturating_sub(1);
        if self.pricing_pending == 0 {
            self.ready();
        }
    }

    fn start_pricing(&mut self, cx: &mut Cx, key: Option<&str>) {
        let ids: Vec<String> = self.rows.iter().filter(|row| row.input.is_ok()).map(|row| row.endpoint_id.clone()).collect();
        if ids.is_empty() || key.is_none() {
            self.ready();
            return;
        }
        self.phase = Phase::Pricing;
        self.message = format!("{} models; asking Fal for their prices…", self.rows.len());
        for chunk in ids.chunks(PRICE_BATCH) {
            self.pricing_pending += 1;
            self.send(cx, key, Req::Pricing(chunk.to_vec()));
        }
    }

    fn ready(&mut self) {
        self.phase = Phase::Ready;
        let skipped = self.rows.len() - self.runnable();
        self.message = format!(
            "{} text-to-image models from Fal's model search; {} can run from a prompt alone{}.",
            self.rows.len(),
            self.runnable(),
            if skipped > 0 { format!(", {skipped} are skipped (they need other inputs or stream their answer)") } else { String::new() }
        );
    }

    /// A catalog call was refused: wait it out when that makes sense.
    fn refused(&mut self, now: f64, req: Req, error: FalError) {
        if error.retryable() {
            self.retry_delay = if self.retry_delay <= 0.0 { 2.0 } else { (self.retry_delay * 2.0).min(30.0) };
            if let FalError::RateLimited { retry_after: Some(after), .. } = error {
                self.retry_delay = self.retry_delay.max(after);
            }
            self.retries.push((now + self.retry_delay, req));
            self.message = format!("{} — trying again in {:.0} s…", error.message(), self.retry_delay);
        } else {
            self.phase = Phase::Idle;
            self.error = Some(format!("{}: {}", error.title(), error.message()));
            self.message = "Could not load the model list.".to_string();
        }
    }

    /// The rows in the table's order: finished by end-to-end time (fastest
    /// first), then the ones running, then waiting, then failed and skipped.
    pub fn order(&self, engine: &Engine) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        let rank = |row: &Row| -> (u8, f64) {
            match row.job.map(|job| &engine.jobs[job]) {
                Some(job) => match (job.stage, job.end_to_end()) {
                    (Stage::Done | Stage::Downloading, Some(e2e)) => (0, e2e),
                    (stage, _) if stage.is_live() => (1, job.t_submit),
                    _ => (3, 0.0),
                },
                None if row.input.is_ok() => (2, 0.0),
                None => (4, 0.0),
            }
        };
        order.sort_by(|&a, &b| {
            let (ra, ka) = rank(&self.rows[a]);
            let (rb, kb) = rank(&self.rows[b]);
            ra.cmp(&rb).then(ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)).then(a.cmp(&b))
        });
        order
    }

    /// What the models that returned a picture cost, by the same prices.
    pub fn spent(&self, engine: &Engine) -> f64 {
        self.rows
            .iter()
            .filter_map(|row| Some((row, &engine.jobs[row.job?])))
            .filter(|(_, job)| matches!(job.stage, Stage::Done | Stage::Downloading))
            .filter_map(|(row, job)| cost_per_image(row, Some(job)))
            .sum()
    }

    /// A sweep model finished. One that rejected the sweep's size (some need
    /// 768 px or more) runs once more without a size, at its own default,
    /// so it still gets a time; the row says so.
    pub fn job_finished(&mut self, engine: &Engine, job: usize) {
        let job = &engine.jobs[job];
        let Origin::Sweep(index) = job.origin else { return };
        let Some(row) = self.rows.get_mut(index) else { return };
        let size_refused = job.stage == Stage::Failed
            && job.error.as_ref().is_some_and(|(title, message)| {
                let message = message.to_ascii_lowercase();
                title == "Input rejected" && ["width", "height", "size", "pixel", "resolution"].iter().any(|word| message.contains(word))
            });
        if !size_refused || row.own_size {
            return;
        }
        let Ok(Value::Obj(fields)) = &mut row.input else { return };
        let before = fields.len();
        fields.retain(|(name, _)| name != "image_size" && name != "aspect_ratio");
        if fields.len() == before {
            return;
        }
        row.own_size = true;
        row.job = None;
        self.again.push_back(index);
        self.generation += 1;
    }

    /// A model finished: write the table so far.
    pub fn record(&mut self, cx: &mut Cx, engine: &Engine) {
        self.generation += 1;
        self.save(cx, engine);
    }

    fn save(&mut self, cx: &mut Cx, engine: &Engine) {
        let Some(path) = self.file.clone() else { return };
        let num = |v: Option<f64>| v.map(|v| Value::F64((v * 1000.0).round() / 1000.0)).unwrap_or(Value::Null);
        let mut models = Vec::new();
        for index in self.order(engine) {
            let row = &self.rows[index];
            let job = row.job.map(|job| &engine.jobs[job]);
            let price = match &row.price {
                Some(price) => obj(vec![
                    ("unit_price", Value::F64(price.unit_price)),
                    ("unit", s(price.unit.clone())),
                    ("currency", s(price.currency.clone())),
                ]),
                None => Value::Null,
            };
            let status = match (job, &row.input) {
                (Some(job), _) => format!("{:?}", job.stage).to_lowercase(),
                (None, Ok(_)) => "not run".to_string(),
                (None, Err(_)) => "skipped".to_string(),
            };
            let error = match (job.and_then(|job| job.error.as_ref()), &row.input) {
                (Some((title, message)), _) => s(format!("{title}: {message}")),
                (None, Err(why)) => s(why.clone()),
                _ => Value::Null,
            };
            let image = job
                .and_then(|job| job.shots.first())
                .and_then(|shot| shot.saved.as_ref())
                .map(|path| s(path.display().to_string()))
                .unwrap_or(Value::Null);
            models.push(obj(vec![
                ("endpoint_id", s(row.endpoint_id.clone())),
                ("name", s(row.display_name.clone())),
                ("status", s(status)),
                ("size", s(if row.own_size { "model default (refused the sweep's size)" } else { "sweep" })),
                ("request_id", job.and_then(|job| job.request_id.clone()).map(s).unwrap_or(Value::Null)),
                ("end_to_end_s", num(job.and_then(|job| job.end_to_end()))),
                ("generation_s", num(job.and_then(|job| job.generation_time()))),
                ("fal_inference_s", num(job.and_then(|job| job.inference_time))),
                ("queue_wait_s", num(job.and_then(|job| job.queue_wait()))),
                ("download_s", num(job.and_then(|job| Some(job.t_files? - job.t_result?)))),
                ("submit_s", num(job.and_then(|job| Some(job.t_accepted? - job.t_submit)))),
                ("cost_per_image", num(cost_per_image(row, job))),
                ("price", price),
                ("image", image),
                ("error", error),
            ]));
        }
        let doc = obj(vec![
            ("unix_time", Value::Int(self.started_unix as i64)),
            ("prompt", s(self.prompt.clone())),
            ("seed", Value::Int(self.seed)),
            ("concurrency", Value::Int(CONCURRENCY as i64)),
            ("source", s("GET https://api.fal.ai/v1/models?category=text-to-image&status=active")),
            (
                "estimate",
                match &self.confirmed {
                    Some(estimate) => obj(vec![
                        ("usd", num(Some(estimate.usd))),
                        ("models_priced", Value::Int(estimate.priced as i64)),
                        ("models_unpriced", Value::Int(estimate.unpriced as i64)),
                        ("basis", s("one picture per model at the sweep's size, from Fal's unit prices; per-megapixel prices count whole megapixels (1024x1024 = 1), rounded up as Fal bills")),
                    ]),
                    None => Value::Null,
                },
            ),
            ("spent_usd", num(Some(self.spent(engine)))),
            ("models", Value::Arr(models)),
        ]);
        let bytes: Arc<[u8]> = Arc::from(fal::pretty(&doc).into_bytes());
        store::write_files(cx, vec![store::FileWrite { path, bytes }]);
    }
}

/// What one picture from a row costs, from Fal's unit price: a per-image
/// price as it is; a per-megapixel one times the picture's megapixels rounded
/// up, as Fal bills images ("rounding up to the nearest megapixel"; 1024x1024
/// counts as one). The picture's size is the one it came back at, else the
/// one asked for, else one megapixel. None for prices in other units
/// (seconds, tokens, units) and models without a price.
pub fn cost_per_image(row: &Row, job: Option<&Job>) -> Option<f64> {
    let price = row.price.as_ref()?;
    let unit = price.unit.to_ascii_lowercase();
    if unit.contains("image") {
        return Some(price.unit_price);
    }
    if !unit.contains("megapixel") {
        return None;
    }
    const MEGAPIXEL: u64 = 1 << 20;
    let pixels = job
        .and_then(|job| job.shots.first())
        .and_then(|shot| Some(shot.media.width? * shot.media.height?))
        .or_else(|| row.input.as_ref().ok().and_then(requested_pixels))
        .unwrap_or(MEGAPIXEL);
    let megapixels = (pixels as f64 / MEGAPIXEL as f64).ceil().max(1.0);
    Some(price.unit_price * megapixels)
}

/// The size a request asks for, in pixels (a preset, "WxH" or width and
/// height; an aspect ratio alone says nothing about pixels).
fn requested_pixels(input: &Value) -> Option<u64> {
    fal::size_pixels(input.get("image_size")?)
}

/// A model with no readable schema gets the prompt and nothing else.
fn sweep_input(schema: Option<&InputSchema>, prompt: &str, seed: i64) -> Result<Value, String> {
    match schema {
        Some(schema) => fal::sweep_input(schema, prompt, seed),
        None => Ok(obj(vec![("prompt", s(prompt))])),
    }
}
