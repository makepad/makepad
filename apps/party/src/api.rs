//! What guests' phones may ask, under `/p/<code>/`:
//!
//! - `GET` (the bare code): the phone page.
//! - `POST join`: a new guest; answers `{"token": ...}`, which every other
//!   call sends as `Authorization: Bearer <token>`.
//! - `POST loop?fps=N&keep=me|wall|keep`: a short loop, its JPEG frames as
//!   `application/x-party-frames` ([u32 BE length][JPEG], repeated); wall or
//!   keep applies once the picture is redrawn.
//! - `POST photo?keep=...`: one JPEG; wall or keep applies once it is redrawn.
//! - `POST wish?source=N`: what the guest would like (UTF-8 text); a new take.
//! - `GET me`: the guest's sources and takes.
//! - `GET ideas?source=N`: `{"ideas": [...]}`, the vision model's ideas for
//!   redrawing it (`ideas.rs`; none unless the hub keeps that model
//!   pinned); the first ask waits for the model.
//! - `GET frame?source=N&i=K` or `GET frame?take=N&i=K`: one JPEG of theirs.
//! - `POST keep?source=N&level=me|wall|keep`: wall or keep applies once the
//!   picture is redrawn.
//! - `POST delete?source=N`, `POST leave`.
//! - `POST live/queue` admits the guest to call immediately. Send the SDP
//!   offer to `POST live/offer` for its SDP answer. The previous guest
//!   keeps the screen until the new call connects. A newer request
//!   displaces a taker whose call is still connecting.
//! - `GET live` keeps the guest present and returns `available`, `screen`
//!   and `state` (out, up, starting or live). An up guest gets `left` in
//!   seconds to connect. A displaced guest gets `"bumped":true` until
//!   they queue again or leave. A quiet page loses its slot.
//! - `GET live/stream` pushes the same state and the guest's own redrawn
//!   frames. `POST live/wish` changes their prompt, `POST live/screen`
//!   changes where the redraw is shown, and `POST live/leave` releases it.
//! - `camtest...`: the camera check (camtest.rs), so a phone can be tried
//!   on the party's own address.
//!
//! Every picture is checked as a whole JPEG and its metadata (EXIF, location,
//! comments) stripped before the store holds it. Nothing else of the box is
//! reachable: the hub and every other service stay behind this.
//!
//! Memory is bounded without losing what a guest was told is held: uploads
//! are limited per guest and per address, bodies being read share one
//! budget, and the store has its own (`Store::bytes_committed`); one address
//! may take only a share of either. Over any of them a request is answered
//! 429 or 503 with a Retry-After, before anything is added, and the phone
//! page sends it again then.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::camtest::CamTest;
use crate::hub::BoxHub;
use crate::jpeg;
use crate::live::{Live, OfferError, Standing};
use crate::queue::Queue;
use crate::server::{busy, html, json, limits, slow_down, streamed, text, ByteBudget, Handler, RateLimiter, Request};
use crate::settings::Settings;
use crate::store::{Budget, GuestId, IdeasClaim, Jpeg, Keep, Persist, Quality, Source, SourceKind, Store, StoreError, TakeId, TakeState, Who};
use makepad_bounded_http::{Method, Resp};
use makepad_strict_json::{self as sj, Value};
use std::net::IpAddr;
use std::path::PathBuf;
use makepad_qr::Qr;
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("../web/phone.html");
/// The party's screen: what guests put on it, full screen, for a TV or projector.
const SCREEN: &str = include_str!("../web/screen.html");

/// A loop's upload: 240 frames of a phone page's 512 px JPEGs come to well
/// under this; anything larger is not the phone page.
const MAX_LOOP_BYTES: u64 = 48 * 1024 * 1024;
const MAX_LOOP_FRAMES: usize = 240;
const MAX_PHOTO_BYTES: u64 = 24 * 1024 * 1024;
/// The page sends at most 1280 px on the long side; this leaves room for
/// any other client's choice without decoding giant pictures.
const MAX_SIDE: u16 = 4096;
const MAX_WISH_BYTES: u64 = 2048;
const MAX_WISH_CHARS: usize = 300;
/// A phone browser's offer is a few KB.
const MAX_OFFER_BYTES: u64 = 64 * 1024;

/// The store under its lock; a change made through it wakes the phones'
/// streams once the lock is let go.
pub struct StoreGuard<'a> {
    guard: Option<MutexGuard<'a, Store>>,
    version: u64,
    changed: &'a Condvar,
}

impl std::ops::Deref for StoreGuard<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.guard.as_ref().expect("a store guard holds its lock until it goes")
    }
}

impl std::ops::DerefMut for StoreGuard<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        self.guard.as_mut().expect("a store guard holds its lock until it goes")
    }
}

impl Drop for StoreGuard<'_> {
    fn drop(&mut self) {
        let moved = self.guard.as_ref().is_some_and(|store| store.version != self.version);
        drop(self.guard.take());
        if moved {
            self.changed.notify_all();
        }
    }
}

/// How long after a guest's last request a restart of the app still waits.
const GUESTS_ABOUT: Duration = Duration::from_secs(3 * 60);

/// The party: its store, the takes waiting for the hub, and live.
pub struct Party {
    stopped: std::sync::atomic::AtomicBool,
    store: Mutex<Store>,
    /// Woken when a store guard that changed the store goes: a phone's
    /// stream waits on it for something new to send.
    changed: Condvar,
    pub queue: Queue,
    pub live: Live,
    /// When a guest last asked anything, and whether the redraw model or the
    /// speech model is wanted warm: the keeper waits on `arrived` for it.
    seen: Mutex<Seen>,
    arrived: Condvar,
    /// A guest's quick asks of the hub (their spoken prompt, ideas for
    /// their loop): a take gives way to them.
    pub asks: Asks,
    /// Where guests join (the LAN address first), once the server listens:
    /// the screen shows it as a code to scan.
    pub join: OnceLock<String>,
    /// The host's settings (in memory only unless [`Party::with_settings`]).
    pub settings: Arc<Settings>,
}

#[derive(Default)]
struct Seen {
    at: Option<Instant>,
    warm: bool,
    warm_voice: bool,
}

/// What the keeper of the models is asked for (see [`Party::next_keep`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wanted {
    /// Ready the redraw model: a guest arrived, or started their camera.
    pub image: bool,
    /// Ready the speech model: a guest pressed the voice button.
    pub voice: bool,
    /// Guests are about: hold the models.
    pub about: bool,
}

/// Quick asks a guest waits for on the hub: their spoken prompt turned into
/// words, the vision model's ideas for their loop. A take being redrawn
/// gives way between frames while one waits (the hub runs one job at a
/// time), so the answer comes back in about a second, not after the take.
#[derive(Default)]
pub struct Asks {
    waiting: Mutex<usize>,
    done: Condvar,
}

impl Asks {
    /// One ask from now until the turn is dropped.
    pub fn begin(&self) -> AskTurn<'_> {
        *self.waiting.lock().unwrap_or_else(|p| p.into_inner()) += 1;
        AskTurn(self)
    }

    pub fn waiting(&self) -> bool {
        *self.waiting.lock().unwrap_or_else(|p| p.into_inner()) > 0
    }

    /// Blocks (the hub worker thread, never the UI) until no ask waits.
    pub fn wait_none(&self, stopped: impl Fn() -> bool) {
        let waiting = self.waiting.lock().unwrap_or_else(|p| p.into_inner());
        drop(self.done.wait_while(waiting, |n| *n > 0 && !stopped()).unwrap_or_else(|p| p.into_inner()));
    }
}

pub struct AskTurn<'a>(&'a Asks);

impl Drop for AskTurn<'_> {
    fn drop(&mut self) {
        *self.0.waiting.lock().unwrap_or_else(|p| p.into_inner()) -= 1;
        self.0.done.notify_all();
    }
}

impl Party {
    /// `budget`: the bytes of pictures the store may hold and promise.
    pub fn new(kept: PathBuf, budget: Budget) -> Party {
        Party {
            stopped: Default::default(),
            store: Mutex::new(Store::new(kept, budget)),
            changed: Condvar::new(),
            queue: Queue::default(),
            live: Live::default(),
            seen: Mutex::new(Seen::default()),
            arrived: Condvar::new(),
            asks: Asks::default(),
            join: OnceLock::new(),
            settings: Arc::default(),
        }
    }

    /// The party with the host's `settings` (the saved ones, at start).
    pub fn with_settings(mut self, settings: Settings) -> Party {
        self.settings = Arc::new(settings);
        self
    }

    pub(crate) fn stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Wake the party's workers and withdraw their requests from the hub.
    pub fn stop_workers(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::Release);
        // Take each waiter's lock so shutdown cannot race its entry into wait.
        drop(self.seen.lock().unwrap_or_else(|p| p.into_inner()));
        self.arrived.notify_all();
        drop(self.asks.waiting.lock().unwrap_or_else(|p| p.into_inner()));
        self.asks.done.notify_all();
        self.live.wake_worker();
        self.queue.wake_worker();
    }

    /// A guest asked something just now. The first after a quiet while
    /// wants the redraw model warm.
    pub fn saw_guest(&self) {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if !seen.at.is_some_and(|at| at.elapsed() < GUESTS_ABOUT) {
            seen.warm = true;
            self.arrived.notify_all();
        }
        seen.at = Some(Instant::now());
    }

    /// A guest is about to ask for a redraw (their camera just started), or
    /// with `voice` to say a prompt (they pressed the voice button).
    pub fn want_warm(&self, voice: bool) {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if voice {
            seen.warm_voice = true;
        } else {
            seen.warm = true;
        }
        self.arrived.notify_all();
    }

    /// For the keeper of the models: waits until one is wanted warm or
    /// `timeout` passes (no timeout: until one is wanted), and says which are
    /// wanted warm now and whether guests are about.
    pub fn next_keep(&self, timeout: Option<Duration>) -> Wanted {
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let wanted = |s: &mut Seen| !s.warm && !s.warm_voice && !self.stopped();
        let mut seen = match timeout {
            Some(timeout) => self.arrived.wait_timeout_while(seen, timeout, wanted).unwrap_or_else(|p| p.into_inner()).0,
            None => self.arrived.wait_while(seen, wanted).unwrap_or_else(|p| p.into_inner()),
        };
        Wanted {
            image: std::mem::take(&mut seen.warm),
            voice: std::mem::take(&mut seen.warm_voice),
            about: seen.at.is_some_and(|at| at.elapsed() < GUESTS_ABOUT),
        }
    }

    /// Why a restart of the app would cost guests now, if it would: a take
    /// being drawn or waiting, or a guest about. A restart starts with an
    /// empty store (guests' pictures are never on disk unless they keep
    /// them); a phone sends its loop again, but its redraw starts over.
    pub fn restart_waits(&self) -> Option<String> {
        if self.store().busy() {
            return Some("a take is being redrawn for a guest".into());
        }
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner()).at;
        match seen {
            Some(at) if at.elapsed() < GUESTS_ABOUT => Some(format!("a guest was here {} s ago", at.elapsed().as_secs())),
            _ => None,
        }
    }

    /// A quarter of the machine's memory: the hub and the screen have the rest.
    pub fn default_budget() -> Budget {
        Budget::of(limits::memory_bytes() / 4)
    }

    /// The store, for a few field updates or a snapshot: never held across
    /// I/O, a decode or a hub call.
    pub fn store(&self) -> StoreGuard<'_> {
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        let version = guard.version;
        StoreGuard { guard: Some(guard), version, changed: &self.changed }
    }

    /// Waits up to `timeout` for the store to move past `version` and
    /// returns the version it has then. Only for a connection's own thread
    /// (a phone's stream), never a UI or worker thread.
    pub fn wait_change(&self, version: u64, timeout: Duration) -> u64 {
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        let (guard, _) = self.changed.wait_timeout_while(guard, timeout, |store| store.version == version).unwrap_or_else(|p| p.into_inner());
        guard.version
    }

    /// Runs disk steps outside the lock and logs what failed.
    pub fn persist(&self, steps: Vec<Persist>) {
        for error in Persist::run(steps) {
            eprintln!("[party] keep: {error}");
        }
    }
}

pub struct Api {
    party: Arc<Party>,
    /// The camera check (`/p/<code>/camtest`) beside the party, so a phone
    /// can be tried on the party's own address.
    camtest: CamTest,
    joins: RateLimiter<IpAddr>,
    /// Per guest, and per address too: joining is cheap, so one address
    /// could otherwise upload as a crowd.
    uploads: RateLimiter<GuestId>,
    uploads_by_ip: RateLimiter<IpAddr>,
    reads: RateLimiter<GuestId>,
    /// Bodies read whole at once (party uploads and the camera check's).
    inflight: Arc<ByteBudget>,
    /// How long a phone's stream stays open once nothing of its guest is
    /// being redrawn.
    linger: Duration,
    /// The box's hub, for voice prompts (none: the party runs without one).
    hub: Option<BoxHub>,
    /// How long the screen's stream stays open with nothing new on it.
    screen_linger: Duration,
}

impl Api {
    pub fn new(party: Arc<Party>) -> Api {
        let inflight = Arc::new(ByteBudget::new(limits::memory_bytes() / 16));
        Api {
            party,
            camtest: CamTest::new(inflight.clone()),
            joins: RateLimiter::new(30.0, 0.5),
            uploads: RateLimiter::new(12.0, 0.25),
            uploads_by_ip: RateLimiter::new(40.0, 1.0),
            reads: RateLimiter::new(600.0, 60.0),
            inflight,
            linger: STREAM_LINGER,
            hub: None,
            screen_linger: SCREEN_LINGER,
        }
    }

    /// Voice prompts are turned into words by `hub`.
    pub fn with_hub(mut self, hub: BoxHub) -> Api {
        self.hub = Some(hub);
        self
    }

    /// A guest's spoken prompt (a WAV file) as words, for the page to put in
    /// its prompt field. The audio and the words stay in memory, here and
    /// on the hub, and only their length is logged.
    fn voice(&self, request: &mut Request<'_>) -> Resp {
        let Some(hub) = &self.hub else {
            return text(503, "this party has no hub to hear with");
        };
        let wav = match request.body_within(VOICE_MAX, &self.inflight) {
            Ok(body) => body,
            Err(resp) => return resp,
        };
        let started = Instant::now();
        let turn = self.party.asks.begin();
        let heard = hub.transcribe(&wav, &|| self.party.stopped());
        drop(turn);
        let seconds = wav_seconds(&wav);
        match heard {
            Ok((words, time)) => {
                eprintln!(
                    "[party] {} voice: {seconds:.1} s of speech, {} characters back after {:.2} s (the hub: queued {:.2} s, ran {:.2} s)",
                    crate::worker::clock(),
                    words.chars().count(),
                    started.elapsed().as_secs_f64(),
                    time.queued.as_secs_f64(),
                    time.run.as_secs_f64()
                );
                json(200, sj::obj(vec![("text", sj::s(words))]).to_json())
            }
            Err(e) => {
                eprintln!("[party] {} voice: {seconds:.1} s of speech not heard: {e}", crate::worker::clock());
                text(502, "the box could not make out the words")
            }
        }
    }

    /// The vision model's ideas for redrawing one of the guest's sources
    /// (`source`): `{"ideas": [...]}`, none when the party has no hub, the
    /// hub does not keep the vision model pinned, or the model could not say. The source's first ask has the vision model look
    /// at it (a take being redrawn gives way between frames); an ask while
    /// that runs waits for its answer, and later ones get it at once.
    fn ideas(&self, request: &Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        let source = id(request, "source")?;
        let answer = |ideas: &[String]| json(200, sj::obj(vec![("ideas", Value::Arr(ideas.iter().map(|idea| sj::s(idea.clone())).collect()))]).to_json());
        self.party.store().source(Who::Guest(guest), source).map_err(refused)?;
        let Some(hub) = self.hub.as_ref().filter(|hub| crate::ideas::available(hub)) else { return Ok(answer(&[])) };
        let waited = Instant::now();
        loop {
            let (claim, version) = {
                let mut store = self.party.store();
                let claim = store.claim_ideas(Who::Guest(guest), source).map_err(refused)?;
                (claim, store.version)
            };
            match claim {
                IdeasClaim::Ready(ideas) => return Ok(answer(&ideas)),
                IdeasClaim::Wait if waited.elapsed() < IDEAS_WAIT => {
                    self.party.wait_change(version, IDEAS_WAIT - waited.elapsed());
                }
                IdeasClaim::Wait => return Ok(answer(&[])),
                IdeasClaim::Ask(picture) => {
                    let started = Instant::now();
                    let turn = self.party.asks.begin();
                    let said = crate::ideas::ask(hub, &picture, &|| {
                        self.party.stopped() || self.party.store().source(Who::Guest(guest), source).is_err()
                    });
                    drop(turn);
                    let ideas = match said {
                        Ok(ideas) => {
                            eprintln!("[party] {} ideas: {} for source {source} after {:.2} s", crate::worker::clock(), ideas.len(), started.elapsed().as_secs_f64());
                            ideas
                        }
                        Err(e) => {
                            eprintln!("[party] {} ideas for source {source}: {e}", crate::worker::clock());
                            Vec::new()
                        }
                    };
                    // None the model could not say: the next ask tries again.
                    self.party.store().set_ideas(source, (!ideas.is_empty()).then(|| ideas.clone()));
                    return Ok(answer(&ideas));
                }
            }
        }
    }

    /// A phone's stream ends after `linger` with nothing of its guest being
    /// redrawn (tests: at once).
    /// Streams end after `linger` with nothing new: a phone's and the screen's.
    pub fn with_stream_linger(mut self, linger: Duration) -> Api {
        self.linger = linger;
        self.screen_linger = linger;
        self
    }

    fn guest(&self, request: &Request<'_>) -> Result<GuestId, Resp> {
        let token = request.head.authorization.as_deref().and_then(|a| a.strip_prefix("Bearer ")).map(str::trim);
        let guest = token.and_then(|t| self.party.store().guest(t)).ok_or_else(|| text(401, "join first"))?;
        self.party.saw_guest();
        Ok(guest)
    }

    fn limit<K: std::hash::Hash + Eq + Clone>(limiter: &RateLimiter<K>, key: &K) -> Result<(), Resp> {
        limiter
            .take(key, 1.0)
            .map_err(|wait| slow_down(wait, "a lot has come from your connection at once: sending again shortly"))
    }

    fn join(&self, request: &Request<'_>) -> Result<Resp, Resp> {
        Self::limit(&self.joins, &request.peer)?;
        let mut bytes = [0u8; 16];
        makepad_network::tls::os_random(&mut bytes)
            .map_err(|_| busy(5, "the box could not make a guest pass just now: trying again in a moment"))?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.party.store().join(token.clone());
        Ok(json(200, sj::obj(vec![("token", sj::s(token))]).to_json()))
    }

    fn upload(&self, request: &mut Request<'_>, guest: GuestId, photo: bool) -> Result<Resp, Resp> {
        Self::limit(&self.uploads, &guest)?;
        Self::limit(&self.uploads_by_ip, &request.peer)?;
        let keep = Keep::parse(request.query("keep").unwrap_or("me")).ok_or_else(|| text(400, "keep is me, wall or keep"))?;
        let kind = if photo {
            SourceKind::Photo
        } else {
            let fps = request.query("fps").and_then(|f| f.parse::<u32>().ok()).filter(|f| (1..=30).contains(f));
            SourceKind::Loop { fps: fps.ok_or_else(|| text(400, "fps is 1 to 30"))? }
        };
        let body = request.body_within(if photo { MAX_PHOTO_BYTES } else { MAX_LOOP_BYTES }, &self.inflight)?;
        let pieces = if photo { vec![&body[..]] } else { jpeg::split_frames(&body).map_err(|e| text(400, e))? };
        if pieces.is_empty() || pieces.len() > MAX_LOOP_FRAMES {
            return Err(text(400, "a loop is 1 to 240 frames"));
        }
        let mut frames: Vec<Jpeg> = Vec::with_capacity(pieces.len());
        for piece in pieces {
            let info = jpeg::info(piece).map_err(|e| text(400, &format!("not a whole JPEG: {e:?}")))?;
            if info.width == 0 || info.height == 0 || info.width > MAX_SIDE || info.height > MAX_SIDE {
                return Err(text(400, "a picture is at most 4096 px on a side"));
            }
            let clean = jpeg::strip_metadata(piece).map_err(|e| text(400, &format!("not a whole JPEG: {e:?}")))?;
            frames.push(Arc::from(clean));
        }
        let (source, take, persist) = {
            let mut store = self.party.store();
            let (source, persist) = store.add_source(guest, request.peer, kind, frames, None, keep).map_err(refused)?;
            let take = store.add_take(source, Quality::Fast).map_err(refused)?;
            (source, take, persist)
        };
        self.party.persist(persist);
        eprintln!("[party] {} take {take}: asked for, with a new picture", crate::worker::clock());
        self.party.queue.push(take);
        Ok(json(200, sj::obj(vec![("source", Value::Int(source as i64)), ("take", Value::Int(take as i64))]).to_json()))
    }

    fn wish(&self, request: &mut Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        Self::limit(&self.uploads, &guest)?;
        Self::limit(&self.uploads_by_ip, &request.peer)?;
        let source = id(request, "source")?;
        let wish = clean_wish(request.body(MAX_WISH_BYTES)?)?;
        let take = self.party.store().wish(Who::Guest(guest), source, wish.clone()).map_err(refused)?;
        if let Some(wish) = &wish {
            self.party.live.remember(wish);
        }
        eprintln!("[party] {} take {take}: asked for, with a wish", crate::worker::clock());
        self.party.queue.push(take);
        Ok(json(200, sj::obj(vec![("take", Value::Int(take as i64))]).to_json()))
    }

    fn me(&self, guest: GuestId) -> Resp {
        json(200, me_json(&self.party.store(), guest))
    }

    /// One phone's stream: what `me` says whenever it changes, and each frame
    /// of the take its loop shows as it is redrawn, on one open connection
    /// (`application/x-party-stream`: messages of [u32 BE length][u8 kind]
    /// [payload]; kind 1 is `me`'s JSON, kind 2 a frame: [u32 BE take]
    /// [u16 BE index][JPEG], kind 0 a heartbeat). It ends once nothing of
    /// the guest has been redrawn for a while, with a goodbye (kind 3); the
    /// phone opens it again when it asks for something new. `take` and
    /// `have`: the phone has that take's first `have` frames already.
    fn stream(&self, request: &mut Request<'_>, guest: GuestId) -> Resp {
        // Per take shown: the frames sent of it (or the phone has).
        let mut sent: Vec<(TakeId, usize)> = Vec::new();
        if let (Some(take), Some(have)) = (request.query("take").and_then(|t| t.parse().ok()), request.query("have").and_then(|h| h.parse().ok())) {
            sent.push((take, have));
        }
        if request.start_stream("application/x-party-stream").is_err() {
            return streamed();
        }
        let mut version = None;
        let mut quiet: Option<Instant> = None;
        loop {
            let (now, state, frames, drawing) = {
                let store = self.party.store();
                let now = store.version;
                let state = (version != Some(now)).then(|| me_json(&store, guest));
                let (mut frames, mut drawing) = (Vec::new(), false);
                for source in store.sources_of(guest) {
                    let takes: Vec<_> = source.takes.iter().filter_map(|t| store.take(*t)).collect();
                    drawing |= takes.iter().any(|t| matches!(t.state, TakeState::Queued | TakeState::Running));
                    // What the phone plays: the newest take with frames.
                    let shown = takes.iter().rev().find(|t| matches!(t.state, TakeState::Running | TakeState::Done) && !t.frames.is_empty());
                    if let Some(take) = shown {
                        let from = sent.iter().find(|(id, _)| *id == take.id).map_or(0, |(_, n)| *n);
                        frames.extend((from..take.frames.len()).filter_map(|i| Some((take.id, i, take.preview(i)?.clone()))));
                    }
                }
                (now, state, frames, drawing)
            };
            if let Some(state) = state {
                if request.send(&stream_message(1, &[state.as_bytes()])).is_err() {
                    return streamed();
                }
                version = Some(now);
            }
            for (take, i, jpeg) in frames {
                let at = [(take as u32).to_be_bytes().as_slice(), (i as u16).to_be_bytes().as_slice()].concat();
                if request.send(&stream_message(2, &[&at[..], &jpeg[..]])).is_err() {
                    return streamed();
                }
                match sent.iter_mut().find(|(id, _)| *id == take) {
                    Some(entry) => entry.1 = i + 1,
                    None => sent.push((take, i + 1)),
                }
            }
            if drawing {
                quiet = None;
            } else if quiet.get_or_insert_with(Instant::now).elapsed() >= self.linger {
                let _ = request.send(&stream_message(3, &[]));
                return streamed();
            }
            let wait = if drawing { STREAM_BEAT } else { STREAM_BEAT.min(self.linger) };
            if self.party.wait_change(now, wait) == now && request.send(&stream_message(0, &[])).is_err() {
                return streamed();
            }
        }
    }

    /// The party's screen, pushed as it changes (`screen/stream`, messages as
    /// on a phone's stream): kind 1 the JSON of what it shows (`take`, `fps`,
    /// `frames` so far of `total`, `drawing`, and `join`, where guests
    /// join), kind 2 that take's frames at the size they were drawn, kind 0
    /// a heartbeat. The newest redraw for the screen shows from its first
    /// frame; between redraws the wall's loops take turns (see
    /// [`Store::screen_next`]). After a long while with nothing new it says
    /// goodbye (kind 3), and the screen opens it again.
    fn screen_stream(&self, request: &mut Request<'_>) -> Resp {
        if request.start_stream("application/x-party-stream").is_err() {
            return streamed();
        }
        let (mut shown, mut seen, mut since, mut sent) = (None, 0, Instant::now(), 0usize);
        let mut said = String::new();
        let mut news = Instant::now();
        let (mut live_shown, mut live_sent) = (0u64, Instant::now());
        loop {
            // A guest live on the screen has it: their redraw as it comes,
            // the loops again once their turn is over.
            let caption = self.party.live.board().filter(|b| b.standing == Standing::Live).and_then(|b| b.caption());
            if let Some(caption) = caption {
                let state = {
                    let store = self.party.store();
                    screen_json(&store, None, self.party.join.get(), Some(&caption))
                };
                if state != said {
                    if request.send(&stream_message(1, &[state.as_bytes()])).is_err() {
                        return streamed();
                    }
                    (said, live_sent) = (state, Instant::now());
                }
                news = Instant::now();
                match self.party.live.stream_frame(None, live_shown, LIVE_LOOK) {
                    Some(frame) => {
                        live_shown = frame.count;
                        if let Some(jpeg) = frame.jpeg() {
                            if request.send(&stream_message(4, &[&jpeg[..]])).is_err() {
                                return streamed();
                            }
                            live_sent = Instant::now();
                        }
                    }
                    None if live_sent.elapsed() >= STREAM_BEAT => {
                        if request.send(&stream_message(0, &[])).is_err() {
                            return streamed();
                        }
                        live_sent = Instant::now();
                    }
                    None => {}
                }
                continue;
            }
            let (version, state, frames) = {
                let store = self.party.store();
                let next = store.screen_next(shown, seen, since.elapsed() >= SCREEN_TURN);
                if next != shown {
                    (shown, since, sent) = (next, Instant::now(), 0);
                }
                seen = seen.max(next.unwrap_or(0));
                let take = next.and_then(|t| store.take(t));
                let frames: Vec<Jpeg> = take.map(|t| t.frames.iter().skip(sent).cloned().collect()).unwrap_or_default();
                (store.version, screen_json(&store, take, self.party.join.get(), None), frames)
            };
            if state != said {
                if request.send(&stream_message(1, &[state.as_bytes()])).is_err() {
                    return streamed();
                }
                said = state;
                news = Instant::now();
            }
            let take = shown.unwrap_or(0) as u32;
            for jpeg in frames {
                let at = [take.to_be_bytes().as_slice(), (sent as u16).to_be_bytes().as_slice()].concat();
                if request.send(&stream_message(2, &[&at[..], &jpeg[..]])).is_err() {
                    return streamed();
                }
                sent += 1;
            }
            if news.elapsed() >= self.screen_linger {
                let _ = request.send(&stream_message(3, &[]));
                return streamed();
            }
            // Woken by a change, at the end of the shown loop's turn or of the
            // stream's linger, or for a heartbeat; soon while a guest on the
            // screen is about to go live.
            let turn_left = SCREEN_TURN.saturating_sub(since.elapsed());
            let linger_left = self.screen_linger.saturating_sub(news.elapsed()).max(Duration::from_millis(1));
            let live_soon = if self.party.live.slots().shown().is_some() { LIVE_LOOK } else { STREAM_BEAT };
            let wait = if turn_left.is_zero() { STREAM_BEAT } else { STREAM_BEAT.min(turn_left) }.min(linger_left).min(live_soon);
            if self.party.wait_change(version, wait) == version && request.send(&stream_message(0, &[])).is_err() {
                return streamed();
            }
        }
    }

    /// The code guests scan to join, as SVG with its quiet zone.
    fn join_code(&self) -> Resp {
        let Some(qr) = self.party.join.get().and_then(|url| Qr::encode(url.as_bytes()).ok()) else {
            return text(404, "the party has no address to join yet");
        };
        let side = qr.size + 8;
        let mut dark = String::new();
        for y in 0..qr.size {
            for x in 0..qr.size {
                if qr.get(x, y) {
                    let _ = write!(dark, "M{} {}h1v1h-1z", x + 4, y + 4);
                }
            }
        }
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" shape-rendering=\"crispEdges\"><rect width=\"{side}\" height=\"{side}\" fill=\"#fff\"/><path fill=\"#000\" d=\"{dark}\"/></svg>"
        );
        Resp::bytes(200, "image/svg+xml", svg.into_bytes()).with_header("Cache-Control", "no-store".into())
    }

    fn frame(&self, request: &Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        let i = request.query("i").and_then(|i| i.parse::<usize>().ok()).ok_or_else(|| text(400, "i is a frame number"))?;
        let store = self.party.store();
        let frame = if let Some(take) = request.query("take") {
            let take = take.parse().ok().and_then(|t| store.take(t)).ok_or_else(|| text(404, "no such take"))?;
            store.source(Who::Guest(guest), take.source).map_err(not_found)?;
            take.preview(i).cloned()
        } else {
            let source = store.source(Who::Guest(guest), id(request, "source")?).map_err(not_found)?;
            source.frames.get(i).cloned()
        };
        let frame = frame.ok_or_else(|| text(404, "no such frame"))?;
        drop(store);
        Ok(Resp::bytes(200, "image/jpeg", frame.to_vec()).with_header("Cache-Control", "no-store".into()))
    }

    fn keep(&self, request: &Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        let source = id(request, "source")?;
        let level = Keep::parse(request.query("level").unwrap_or("")).ok_or_else(|| text(400, "level is me, wall or keep"))?;
        let persist = self.party.store().set_keep(Who::Guest(guest), source, level).map_err(not_found)?;
        self.party.persist(persist);
        Ok(json(200, "{}".into()))
    }

    fn delete(&self, request: &Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        let source = id(request, "source")?;
        let persist = self.party.store().delete(Who::Guest(guest), source).map_err(not_found)?;
        self.party.persist(persist);
        Ok(json(200, "{}".into()))
    }

    fn leave(&self, guest: GuestId) -> Resp {
        let persist = self.party.store().leave(guest);
        self.party.live.leave(guest);
        self.party.persist(persist);
        json(200, "{}".into())
    }

    /// Reading the state keeps the guest's slot alive.
    fn live_state(&self, guest: GuestId) -> Resp {
        json(200, self.live_json(guest, true).0)
    }

    /// The phone polls and streams the same state. Only a poll marks it seen.
    fn live_json(&self, guest: GuestId, seen: bool) -> (String, Standing) {
        let live = &self.party.live;
        let (standing, bumped, screen, others, now) = live.change(|slots, now| {
            if seen {
                slots.seen(guest, now);
                slots.page(guest, now);
            }
            (slots.standing(guest), slots.bumped(guest), slots.screen(guest), slots.others(guest, now), now)
        });
        let mut fields = vec![
            ("available", Value::Bool(live.available())),
            ("screen", Value::Bool(screen)),
            ("others", Value::Int(others as i64)),
        ];
        match standing {
            Standing::Out => {
                fields.push(("state", sj::s("out")));
                if bumped {
                    fields.push(("bumped", Value::Bool(true)));
                }
            }
            Standing::Up { until } => fields.extend([("state", sj::s("up")), ("left", Value::Int((until - now).ceil() as i64))]),
            Standing::Starting => fields.push(("state", sj::s("starting"))),
            Standing::Live => fields.push(("state", sj::s("live"))),
        }
        (sj::obj(fields).to_json(), standing)
    }

    fn live_queue(&self, request: &Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        let live = &self.party.live;
        if live.available() {
            // The guest could have left the party since route authentication.
            // Keep membership locked until admission holds the slots lock.
            let store = self.party.store();
            let token = request.head.authorization.as_deref().and_then(|a| a.strip_prefix("Bearer "));
            if token.and_then(|t| store.guest(t.trim())) != Some(guest) {
                return Err(text(401, "join first"));
            }
            let screen = request.query("screen") != Some("0");
            let replaced = live.change(|slots, now| {
                drop(store);
                let replaced = slots.queue(guest, now);
                slots.set_screen(guest, screen);
                replaced
            });
            if let Some(call) = replaced {
                live.hang_up(call);
            }
        }
        Ok(self.live_state(guest))
    }

    /// A guest's live turn, pushed (`live/stream`, messages as on the phone's
    /// stream): kind 1 where they stand, as `GET live` answers, kind 4 the
    /// newest redrawn frame of their own call (a JPEG) as it comes, kind 0 a
    /// heartbeat. It ends with a goodbye (kind 3) once they are out.
    /// An open stream does not keep their place: a phone put away can hold
    /// its connection for minutes, so the page asks `GET live` while shown.
    fn live_stream(&self, request: &mut Request<'_>, guest: GuestId) -> Resp {
        if request.start_stream("application/x-party-stream").is_err() {
            return streamed();
        }
        let (mut said, mut shown, mut sent_at) = (String::new(), 0u64, Instant::now());
        loop {
            let (state, standing) = self.live_json(guest, false);
            if state != said {
                if request.send(&stream_message(1, &[state.as_bytes()])).is_err() {
                    return streamed();
                }
                said = state;
                sent_at = Instant::now();
            }
            if standing == Standing::Out {
                let _ = request.send(&stream_message(3, &[]));
                return streamed();
            }
            // Their own frames as they come; where they stand at least every
            // half second (another guest's frames wake nobody else's stream).
            match self.party.live.stream_frame(Some(guest), shown, LIVE_LOOK) {
                Some(frame) => {
                    shown = frame.count;
                    if let Some(jpeg) = frame.jpeg() {
                        if request.send(&stream_message(4, &[&jpeg[..]])).is_err() {
                            return streamed();
                        }
                        sent_at = Instant::now();
                    }
                }
                None if sent_at.elapsed() >= STREAM_BEAT => {
                    if request.send(&stream_message(0, &[])).is_err() {
                        return streamed();
                    }
                    sent_at = Instant::now();
                }
                None => {}
            }
        }
    }

    fn live_offer(&self, request: &mut Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        Self::limit(&self.uploads, &guest)?;
        Self::limit(&self.uploads_by_ip, &request.peer)?;
        let body = request.body(MAX_OFFER_BYTES)?;
        let offer = String::from_utf8(body).map_err(|_| text(400, "an offer is SDP text"))?;
        let side = |key: &str| request.query(key).and_then(|v| v.parse::<u32>().ok()).filter(|v| *v <= 8192).unwrap_or(0);
        let camera = (side("w"), side("h"));
        match self.party.live.offer(guest, &offer, camera) {
            Ok(answer) => Ok(Resp::bytes(200, "application/sdp", answer.into_bytes()).with_header("Cache-Control", "no-store".into())),
            Err(OfferError::Off) => Err(text(404, "no live here")),
            Err(OfferError::NotUp) => Err(text(409, "not your turn")),
            Err(OfferError::Offer(why)) => Err(text(400, &format!("the offer cannot be answered: {why}"))),
        }
    }

    /// The live guest's wish for their redraw (empty: the box picks).
    fn live_wish(&self, request: &mut Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        Self::limit(&self.uploads, &guest)?;
        let wish = clean_wish(request.body(MAX_WISH_BYTES)?)?;
        self.party.live.wish(guest, wish).map_err(|e| match e {
            OfferError::Off => text(404, "no live here"),
            OfferError::NotUp => text(409, "not your turn"),
            OfferError::Offer(why) => text(400, &why),
        })?;
        Ok(self.live_state(guest))
    }

    fn route(&self, request: &mut Request<'_>) -> Result<Resp, Resp> {
        let route: Vec<String> = request.route.clone();
        let route: Vec<&str> = route.iter().map(String::as_str).collect();
        if route.first() == Some(&"camtest") {
            return Ok(self.camtest.handle(request));
        }
        match (request.method(), route.as_slice()) {
            (Method::Get | Method::Head, []) => return Ok(html(PAGE)),
            (Method::Post, ["join"]) => return self.join(request),
            // The party's screen: what the guests put on it, for anyone
            // with the party's address (as the room's screen is).
            (Method::Get | Method::Head, ["screen"]) => {
                Self::limit(&self.joins, &request.peer)?;
                return Ok(html(SCREEN));
            }
            (Method::Get, ["screen", "stream"]) => {
                Self::limit(&self.joins, &request.peer)?;
                return Ok(self.screen_stream(request));
            }
            (Method::Get, ["screen", "join.svg"]) => {
                Self::limit(&self.joins, &request.peer)?;
                return Ok(self.join_code());
            }
            _ => {}
        }
        let guest = self.guest(request)?;
        match (request.method(), route.as_slice()) {
            (Method::Post, ["loop"]) => self.upload(request, guest, false),
            (Method::Post, ["photo"]) => self.upload(request, guest, true),
            (Method::Post, ["wish"]) => self.wish(request, guest),
            (Method::Post, ["keep"]) => self.keep(request, guest),
            (Method::Post, ["delete"]) => self.delete(request, guest),
            (Method::Post, ["leave"]) => Ok(self.leave(guest)),
            (Method::Post, ["warm"]) => {
                Self::limit(&self.reads, &guest)?;
                self.party.want_warm(request.query("for") == Some("voice"));
                Ok(text(204, ""))
            }
            (Method::Get, ["ideas"]) => {
                Self::limit(&self.reads, &guest)?;
                self.ideas(request, guest)
            }
            (Method::Post, ["voice"]) => {
                Self::limit(&self.uploads, &guest)?;
                Self::limit(&self.uploads_by_ip, &request.peer)?;
                Ok(self.voice(request))
            }
            (Method::Get, ["me"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.me(guest))
            }
            (Method::Get, ["stream"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.stream(request, guest))
            }
            (Method::Get, ["frame"]) => {
                Self::limit(&self.reads, &guest)?;
                self.frame(request, guest)
            }
            (Method::Post, ["draft"]) => {
                Self::limit(&self.reads, &guest)?;
                let draft = String::from_utf8(request.body(MAX_WISH_BYTES)?).map_err(|_| text(400, "a draft is UTF-8 text"))?;
                self.party.live.draft(guest, &draft.chars().filter(|c| !c.is_control()).take(MAX_WISH_CHARS).collect::<String>());
                Ok(json(200, "{}".into()))
            }
            (Method::Get, ["history"]) => {
                Self::limit(&self.reads, &guest)?;
                let prompts = self.party.live.history().into_iter().map(sj::s).collect();
                Ok(json(200, sj::obj(vec![("prompts", Value::Arr(prompts))]).to_json()))
            }
            (Method::Get, ["live"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.live_state(guest))
            }
            (Method::Post, ["live", "queue"]) => {
                Self::limit(&self.reads, &guest)?;
                self.live_queue(request, guest)
            }
            (Method::Post, ["live", "screen"]) => {
                Self::limit(&self.reads, &guest)?;
                let on = request.query("on") != Some("0");
                self.party.live.change(|slots, _| slots.set_screen(guest, on));
                Ok(self.live_state(guest))
            }
            (Method::Get, ["live", "stream"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.live_stream(request, guest))
            }
            (Method::Post, ["live", "offer"]) => self.live_offer(request, guest),
            (Method::Post, ["live", "wish"]) => self.live_wish(request, guest),
            (Method::Post, ["live", "leave"]) => {
                self.party.live.leave(guest);
                Ok(self.live_state(guest))
            }
            _ => Err(text(404, "not found")),
        }
    }
}

impl Handler for Api {
    fn stop(&self) {
        self.party.stop_workers();
    }

    fn handle(&self, request: &mut Request<'_>) -> Resp {
        self.route(request).unwrap_or_else(|resp| resp)
    }
}

/// A wish as typed: UTF-8, at most 300 characters, no control characters
/// (`None` when empty).
fn clean_wish(body: Vec<u8>) -> Result<Option<String>, Resp> {
    let wish = String::from_utf8(body).map_err(|_| text(400, "a wish is UTF-8 text"))?;
    let wish: String = wish.trim().chars().filter(|c| !c.is_control()).collect();
    if wish.chars().count() > MAX_WISH_CHARS {
        return Err(text(400, "a wish is at most 300 characters"));
    }
    Ok((!wish.is_empty()).then_some(wish))
}

fn not_found(_: StoreError) -> Resp {
    text(404, "not yours or gone")
}

fn refused(error: StoreError) -> Resp {
    match error {
        StoreError::NotFound => not_found(error),
        StoreError::Full => busy(60, "the box is full of pictures right now: delete one of yours, or it is sent again in a minute"),
        StoreError::Share => busy(60, "a lot has come from your connection right now: delete one of yours, or it is sent again in a minute"),
    }
}

fn id(request: &Request<'_>, key: &str) -> Result<u64, Resp> {
    request.query(key).and_then(|v| v.parse().ok()).ok_or_else(|| text(400, "which one?"))
}

/// How long an ask for a source's ideas waits for another ask's answer.
const IDEAS_WAIT: Duration = Duration::from_secs(60);

/// The largest spoken prompt taken: a minute of 16 kHz 16-bit mono WAV,
/// what the page sends, is under 2 MB.
const VOICE_MAX: u64 = 4 << 20;

/// The length of a WAV file's sound, from its header (0 when it has none).
fn wav_seconds(wav: &[u8]) -> f64 {
    if wav.len() < 44 || &wav[..4] != b"RIFF" {
        return 0.0;
    }
    let byte_rate = u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]);
    if byte_rate == 0 { 0.0 } else { (wav.len() - 44) as f64 / byte_rate as f64 }
}

/// How long the screen shows a finished loop before the wall's next one.
const SCREEN_TURN: Duration = Duration::from_secs(20);
/// How long the screen's stream stays open with nothing new on it.
const SCREEN_LINGER: Duration = Duration::from_secs(600);

/// What the screen shows, as JSON (see [`Api::screen_stream`]).
fn screen_json(store: &Store, take: Option<&crate::store::Take>, join: Option<&String>, live: Option<&str>) -> String {
    let source = take.and_then(|t| store.source(Who::Host, t.source).ok());
    let fps = source.map_or(0, |s| match s.kind {
        SourceKind::Loop { fps } => fps,
        SourceKind::Photo => 0,
    });
    sj::obj(vec![
        ("take", take.map_or(Value::Null, |t| Value::Int(t.id as i64))),
        ("fps", Value::Int(fps as i64)),
        ("frames", Value::Int(take.map_or(0, |t| t.frames.len()) as i64)),
        ("total", Value::Int(source.map_or(0, |s| s.frames.len()) as i64)),
        ("drawing", Value::Bool(take.is_some_and(|t| matches!(t.state, TakeState::Queued | TakeState::Running)))),
        ("join", join.map_or(Value::Null, |j| sj::s(j.clone()))),
        ("live", live.map_or(Value::Null, |l| sj::s(l.to_string()))),
    ])
    .to_json()
}

/// How long a live stream waits for a frame before it looks again at where
/// its guest stands.
const LIVE_LOOK: Duration = Duration::from_millis(500);

/// How long a phone's stream stays open with nothing of its guest being redrawn.
const STREAM_LINGER: Duration = Duration::from_secs(10);
/// A quiet stream's heartbeat: it finds a phone that went away.
const STREAM_BEAT: Duration = Duration::from_secs(5);

fn stream_message(kind: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut message = Vec::with_capacity(5 + len);
    message.extend_from_slice(&((len + 1) as u32).to_be_bytes());
    message.push(kind);
    for part in parts {
        message.extend_from_slice(part);
    }
    message
}

fn me_json(store: &Store, guest: GuestId) -> String {
    let sources: Vec<Value> = store.sources_of(guest).into_iter().map(|s| source_json(store, s)).collect();
    sj::obj(vec![("version", Value::Int(store.version as i64)), ("sources", Value::Arr(sources))]).to_json()
}

fn source_json(store: &Store, source: &Source) -> Value {
    let (kind, fps) = match source.kind {
        SourceKind::Loop { fps } => ("loop", fps),
        SourceKind::Photo => ("photo", 0),
    };
    let takes: Vec<Value> = source
        .takes
        .iter()
        .filter_map(|t| store.take(*t))
        .map(|t| {
            let (state, error) = match &t.state {
                TakeState::Queued => ("queued", None),
                TakeState::Running => ("running", None),
                TakeState::Done => ("done", None),
                TakeState::Failed(e) => ("failed", Some(e.clone())),
            };
            let mut fields = vec![
                ("id", Value::Int(t.id as i64)),
                ("quality", sj::s(t.quality.name())),
                ("state", sj::s(state)),
                ("frames", Value::Int(t.frames.len() as i64)),
            ];
            if let Some(prompt) = &t.prompt {
                fields.push(("prompt", sj::s(prompt.clone())));
            }
            if let Some(error) = error {
                fields.push(("error", sj::s(error)));
            }
            sj::obj(fields)
        })
        .collect();
    let mut fields = vec![
        ("id", Value::Int(source.id as i64)),
        ("kind", sj::s(kind)),
        ("fps", Value::Int(fps as i64)),
        ("frames", Value::Int(source.frames.len() as i64)),
        ("keep", sj::s(source.keep.name())),
        ("takes", Value::Arr(takes)),
    ];
    if let Some(wish) = &source.wish {
        fields.push(("wish", sj::s(wish.clone())));
    }
    sj::obj(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jpeg::tests::{pack, sample};
    use crate::server::tests::{exchange_bytes, start};

    struct Answer {
        status: u16,
        head: String,
        body: Vec<u8>,
    }

    fn ask(port: u16, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Answer {
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        // The page is the bare code: a trailing slash is an empty segment, refused.
        let target = if path.is_empty() { "/p/abcdefgh".to_string() } else { format!("/p/abcdefgh/{path}") };
        let mut raw = format!("{method} {target} HTTP/1.1\r\nHost: a\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        raw.extend_from_slice(body);
        let out = exchange_bytes(port, &raw);
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").expect("an answer");
        let head = String::from_utf8_lossy(&out[..split]).into_owned();
        let status = head[9..12].parse().unwrap();
        Answer { status, head, body: out[split + 4..].to_vec() }
    }

    fn field(body: &[u8], key: &str) -> u64 {
        sj::parse(body).unwrap().get(key).and_then(Value::as_u64).unwrap_or_else(|| panic!("{key} in {}", String::from_utf8_lossy(body)))
    }

    #[test]
    fn a_phone_stream_carries_its_state_and_the_shown_takes_frames() {
        let kept = std::env::temp_dir().join(format!("party-api-stream-{}", std::process::id()));
        let party = Arc::new(Party::new(kept.clone(), Budget::UNLIMITED));
        let (mut server, dir) = start("abcdefgh", Arc::new(Api::new(party.clone()).with_stream_linger(Duration::ZERO)));
        let port = server.port;
        let token = sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        let photo = sample(8, 6);
        let sent = ask(port, "POST", "loop?fps=12&keep=me", Some(&token), &pack(&[photo.clone(), photo.clone()]));
        let take = field(&sent.body, "take");
        assert_eq!(party.queue.pop(), Some(take));
        {
            let mut store = party.store();
            store.take_running(take).unwrap();
            // Drawn larger than the phone shows: it gets the preview.
            store.take_frame(take, Arc::from(sample(16, 12)), Arc::from(photo.clone())).unwrap();
        }
        // A take still being redrawn keeps the stream open: here it ends
        // once the take is done, after its last frame.
        let party_done = party.clone();
        let finisher = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let mut store = party_done.store();
            let f: Jpeg = Arc::from(sample(8, 6));
            store.take_frame(take, f.clone(), f).unwrap();
            store.take_finished(take, Ok(())).unwrap();
        });
        let streamed = ask(port, "GET", "stream", Some(&token), b"");
        finisher.join().unwrap();
        assert_eq!(streamed.status, 200, "{}", streamed.head);
        assert!(streamed.head.contains("application/x-party-stream"), "{}", streamed.head);
        let (mut states, mut frames, mut kinds) = (Vec::new(), Vec::new(), Vec::new());
        let mut rest = streamed.body.as_slice();
        while rest.len() >= 5 {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let (kind, payload) = (rest[4], &rest[5..4 + len]);
            kinds.push(kind);
            match kind {
                1 => states.push(sj::parse(payload).unwrap()),
                2 => frames.push((u32::from_be_bytes(payload[..4].try_into().unwrap()) as u64, u16::from_be_bytes(payload[4..6].try_into().unwrap()), payload[6..].to_vec())),
                _ => {}
            }
            rest = &rest[4 + len..];
        }
        assert!(rest.is_empty(), "the stream is whole messages");
        assert_eq!(kinds.last(), Some(&3), "a stream that ends on its own says goodbye");
        assert_eq!(frames.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>(), vec![(take, 0), (take, 1)], "each redrawn frame once, in order, as it comes");
        assert_eq!(frames[0].2, photo, "a frame is its preview's JPEG");
        let last = states.last().expect("the state at least once").to_json();
        assert!(last.contains("\"done\""), "the last state has the take done: {last}");
        // A phone that has the first frame already gets only the rest.
        let again = ask(port, "GET", &format!("stream?take={take}&have=1"), Some(&token), b"");
        let mut rest = again.body.as_slice();
        let mut indices = Vec::new();
        while rest.len() >= 5 {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            if rest[4] == 2 {
                indices.push(u16::from_be_bytes(rest[9..11].try_into().unwrap()));
            }
            rest = &rest[4 + len..];
        }
        assert_eq!(indices, vec![1], "only the frames the phone does not have");
        server.stop();
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(kept);
    }

    /// A stream's whole messages, (kind, payload), and the bytes after them.
    fn messages(mut rest: &[u8]) -> (Vec<(u8, Vec<u8>)>, usize) {
        let mut out = Vec::new();
        while rest.len() >= 5 {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            if rest.len() < 4 + len {
                break;
            }
            out.push((rest[4], rest[5..4 + len].to_vec()));
            rest = &rest[4 + len..];
        }
        (out, rest.len())
    }

    /// Reads a stream route until a message of `kind` comes, and hangs up:
    /// its payload.
    #[cfg(target_os = "linux")]
    fn stream_until(port: u16, path: &str, token: Option<&str>, kind: u8) -> Vec<u8> {
        use std::io::{Read, Write};
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        let raw = format!("GET /p/abcdefgh/{path} HTTP/1.1\r\nHost: a\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n");
        let (mut client, _) = makepad_network::SocketStream::connect_capture("127.0.0.1", &port.to_string()).unwrap();
        client.write_all(raw.as_bytes()).unwrap();
        let (mut got, mut buf) = (Vec::new(), vec![0u8; 1 << 16]);
        loop {
            let n = client.read(&mut buf).unwrap();
            assert!(n > 0, "{path} ended before a message of kind {kind}");
            got.extend_from_slice(&buf[..n]);
            if let Some(split) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                if let Some((_, payload)) = messages(&got[split + 4..]).0.into_iter().find(|(k, _)| *k == kind) {
                    return payload;
                }
            }
        }
    }

    /// The screen gets a redraw put on the wall at the size it was drawn,
    /// frame by frame, with where guests join; the page and the join code
    /// need no guest. The phone's page records at the size the box draws a
    /// loop and plays the size it is sent.
    #[test]
    fn the_screen_streams_the_walls_redraws_at_full_size() {
        assert!(PAGE.contains(&format!("SIDE = {}, SEND_SIDE = {};", crate::worker::PREVIEW_SIDE, crate::worker::LOOP_SIDE)), "the page's sizes are the box's");
        let kept = std::env::temp_dir().join(format!("party-api-screen-{}", std::process::id()));
        let party = Arc::new(Party::new(kept.clone(), Budget::UNLIMITED));
        party.join.set("https://192.0.2.10:8446/p/abcdefgh".into()).unwrap();
        let (mut server, dir) = start("abcdefgh", Arc::new(Api::new(party.clone()).with_stream_linger(Duration::from_millis(400))));
        let port = server.port;
        let token = sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        let sent = ask(port, "POST", "loop?fps=12&keep=wall", Some(&token), &pack(&[sample(8, 6), sample(8, 6)]));
        let take = field(&sent.body, "take");
        assert_eq!(party.queue.pop(), Some(take));
        let full = sample(16, 12);
        {
            let mut store = party.store();
            store.take_running(take).unwrap();
            store.take_frame(take, Arc::from(full.clone()), Arc::from(sample(8, 6))).unwrap();
        }
        let party_done = party.clone();
        let finisher = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let mut store = party_done.store();
            let f: Jpeg = Arc::from(sample(16, 12));
            store.take_frame(take, f.clone(), f).unwrap();
            store.take_finished(take, Ok(())).unwrap();
        });
        let screen = ask(port, "GET", "screen/stream", None, b"");
        finisher.join().unwrap();
        assert_eq!(screen.status, 200, "{}", screen.head);
        let (got, rest) = messages(&screen.body);
        assert_eq!(rest, 0, "the stream is whole messages");
        let frames: Vec<_> = got.iter().filter(|(kind, _)| *kind == 2).map(|(_, p)| (u32::from_be_bytes(p[..4].try_into().unwrap()) as u64, u16::from_be_bytes(p[4..6].try_into().unwrap()), p[6..].to_vec())).collect();
        assert_eq!(frames.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>(), vec![(take, 0), (take, 1)], "each frame once, in order, as it is drawn");
        assert_eq!(frames[0].2, full, "the screen gets the frame at the size it was drawn");
        let last = got.iter().rev().find(|(kind, _)| *kind == 1).map(|(_, p)| String::from_utf8_lossy(p).into_owned()).unwrap();
        for part in [format!("\"take\":{take}"), "\"frames\":2".into(), "\"drawing\":false".into(), "\"join\":\"https://192.0.2.10:8446/p/abcdefgh\"".into()] {
            assert!(last.contains(&part), "the screen's last state has {part}: {last}");
        }
        assert_eq!(got.last().map(|m| m.0), Some(3), "a quiet screen stream says goodbye");
        let page = ask(port, "GET", "screen", None, b"");
        assert!(page.status == 200 && page.head.contains("text/html"), "{}", page.head);
        let code = ask(port, "GET", "screen/join.svg", None, b"");
        assert!(code.status == 200 && code.body.starts_with(b"<svg"), "{}", code.head);
        server.stop();
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(kept);
    }

    #[test]
    fn a_restart_waits_and_the_model_is_kept_warm_while_guests_are_about() {
        let party = Party::new(std::env::temp_dir().join(format!("party-api-restart-{}", std::process::id())), Budget::UNLIMITED);
        assert_eq!(party.restart_waits(), None, "an empty party restarts at once");
        let keep = |image, voice, about| Wanted { image, voice, about };
        assert_eq!(party.next_keep(Some(Duration::ZERO)), keep(false, false, false), "nobody about: nothing to keep");
        party.saw_guest();
        assert!(party.restart_waits().is_some(), "a guest who just asked holds a restart");
        assert_eq!(party.next_keep(Some(Duration::ZERO)), keep(true, false, true), "a guest's arrival wants the redraw model warm");
        party.saw_guest();
        assert_eq!(party.next_keep(Some(Duration::ZERO)), keep(false, false, true), "a guest already about only keeps it held");
        party.want_warm(false);
        assert_eq!(party.next_keep(None), keep(true, false, true), "a camera starting wants it warm again");
        party.want_warm(true);
        assert_eq!(party.next_keep(None), keep(false, true, true), "the voice button wants the speech model warm");
        party.seen.lock().unwrap().at = Instant::now().checked_sub(GUESTS_ABOUT + Duration::from_secs(1));
        assert_eq!(party.restart_waits(), None, "a guest gone quiet does not");
        assert_eq!(party.next_keep(Some(Duration::ZERO)), keep(false, false, false), "and the models may go");
        let turn = party.asks.begin();
        assert!(party.asks.waiting(), "a voice prompt waits until its words are back");
        drop(turn);
        assert!(!party.asks.waiting());
        party.asks.wait_none(|| party.stopped());
        let mut store = party.store();
        let guest = store.join("a-token".into());
        let (source, _) = store.add_source(guest, IpAddr::from([127, 0, 0, 1]), SourceKind::Photo, vec![Arc::from(sample(8, 6))], None, Keep::Me).unwrap();
        store.wish(Who::Guest(guest), source, None).unwrap();
        drop(store);
        assert!(party.restart_waits().is_some(), "a take waiting for its redraw holds a restart");
    }

    #[test]
    fn wall_uploads_and_keep_requests_succeed_but_wait_for_a_finished_take() {
        let kept = std::env::temp_dir().join(format!("party-api-waiting-wall-{}", std::process::id()));
        let party = Arc::new(Party::new(kept.clone(), Budget::UNLIMITED));
        let (mut server, dir) = start("abcdefgh", Arc::new(Api::new(party.clone())));
        let port = server.port;
        let token = sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        let photo = sample(8, 6);
        let frames = pack(&[photo.clone()]);
        for (route, body, ask_keep) in [
            ("photo?keep=wall", photo.as_slice(), false),
            ("loop?fps=12&keep=wall", frames.as_slice(), false),
            ("photo?keep=me", photo.as_slice(), true),
        ] {
            let sent = ask(port, "POST", route, Some(&token), body);
            assert_eq!(sent.status, 200, "an upload is accepted before its redraw: {}", String::from_utf8_lossy(&sent.body));
            let source = field(&sent.body, "source");
            let take = field(&sent.body, "take");
            assert_eq!(party.queue.pop(), Some(take));
            if ask_keep {
                assert_eq!(ask(port, "POST", &format!("keep?source={source}&level=wall"), Some(&token), b"").status, 200, "a wall request is accepted before the redraw");
            }
            let mut store = party.store();
            assert!(store.wall().is_empty(), "the raw upload stays off the wall");
            let s = store.source(Who::Host, source).unwrap();
            assert_eq!((s.keep, s.wanted), (Keep::Me, Some(Keep::Wall)));
            store.take_running(take).unwrap();
            let f: Jpeg = Arc::from(photo.clone());
            store.take_frame(take, f.clone(), f).unwrap();
            assert!(store.wall().is_empty(), "the wall waits for the whole take");
            assert!(store.take_finished(take, Ok(())).unwrap().is_empty());
            let wall = store.wall();
            assert_eq!(wall.len(), 1);
            assert_eq!(wall[0].0.id, source);
            assert_eq!(wall[0].1.id, take, "the finished redraw reaches the wall");
            store.delete(Who::Host, source).unwrap();
        }
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&kept);
    }

    /// A guest's evening over real TLS: join, a loop, a wish, keep, delete,
    /// leave; another guest sees none of it, and nothing is held without a token.
    #[test]
    fn a_guest_round_trip() {
        let kept = std::env::temp_dir().join(format!("party-api-test-{}", std::process::id()));
        let party = Arc::new(Party::new(kept.clone(), Budget::UNLIMITED));
        let (mut server, dir) = start("abcdefgh", Arc::new(Api::new(party.clone())));
        let port = server.port;

        let page = ask(port, "GET", "", None, b"");
        assert!(page.status == 200 && page.head.contains("text/html"), "the page: {}", page.head);
        assert_eq!(ask(port, "POST", "loop?fps=12&keep=me", None, b"").status, 401, "no token, nothing held");

        let a = sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        let b = sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        assert!(a.len() == 32 && a != b, "tokens are 128 random bits");

        let frames = pack(&[sample(512, 512), sample(512, 512)]);
        assert_eq!(ask(port, "POST", "loop?fps=99&keep=me", Some(&a), &frames).status, 400, "fps is checked");
        assert_eq!(ask(port, "POST", "loop?fps=12&keep=all", Some(&a), &frames).status, 400, "keep is checked");
        assert_eq!(ask(port, "POST", "photo?keep=me", Some(&a), b"not a picture").status, 400, "a photo is a whole JPEG");
        let sent = ask(port, "POST", "loop?fps=12&keep=wall", Some(&a), &frames);
        assert_eq!(sent.status, 200, "{}", String::from_utf8_lossy(&sent.body));
        let source = field(&sent.body, "source");
        let take = field(&sent.body, "take");
        assert_eq!(party.queue.pop(), Some(take), "the loop's first take waits for the hub");
        {
            let mut store = party.store();
            let f: Jpeg = Arc::from(sample(512, 512));
            store.take_frame(take, f.clone(), f).unwrap();
            let f: Jpeg = Arc::from(sample(512, 512));
            store.take_frame(take, f.clone(), f).unwrap();
            store.take_finished(take, Ok(())).unwrap();
        }

        let me = sj::parse(&ask(port, "GET", "me", Some(&a), b"").body).unwrap();
        let mine = &me.get("sources").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(mine.get("frames").and_then(Value::as_u64), Some(2));
        assert_eq!(mine.get("keep").and_then(Value::as_str), Some("wall"));
        let theirs = sj::parse(&ask(port, "GET", "me", Some(&b), b"").body).unwrap();
        assert!(theirs.get("sources").and_then(Value::as_arr).unwrap().is_empty(), "another guest has nothing");

        let frame = ask(port, "GET", &format!("frame?source={source}&i=1"), Some(&a), b"");
        assert!(frame.status == 200 && frame.head.contains("image/jpeg"));
        assert!(!jpeg::info(&frame.body).unwrap().had_metadata, "the store holds the frame without its metadata");
        assert_eq!(ask(port, "GET", &format!("frame?source={source}&i=1"), Some(&b), b"").status, 404, "nor its frames");
        assert_eq!(ask(port, "GET", &format!("frame?source={source}&i=2"), Some(&a), b"").status, 404);
        let ideas = ask(port, "GET", &format!("ideas?source={source}"), Some(&a), b"");
        assert_eq!((ideas.status, String::from_utf8_lossy(&ideas.body).into_owned()), (200, r#"{"ideas":[]}"#.to_string()), "no hub: no ideas, at once");
        assert_eq!(ask(port, "GET", &format!("ideas?source={source}"), Some(&b), b"").status, 404, "nor ideas for its loop");

        let wish = ask(port, "POST", &format!("wish?source={source}"), Some(&a), "as a painting".as_bytes());
        assert_eq!(party.queue.pop(), Some(field(&wish.body, "take")), "a wish asks for a new take");
        assert_eq!(party.store().source(Who::Host, source).unwrap().wish.as_deref(), Some("as a painting"));
        assert_eq!(ask(port, "POST", &format!("wish?source={source}"), Some(&b), b"mine now").status, 404);

        assert_eq!(ask(port, "POST", &format!("keep?source={source}&level=keep"), Some(&a), b"").status, 200);
        assert!(party.store().kept_dir(source).join("source-001.jpg").exists(), "keep it writes the frames");
        assert_eq!(ask(port, "POST", &format!("delete?source={source}"), Some(&b), b"").status, 404, "only its guest deletes it");
        assert_eq!(ask(port, "POST", &format!("delete?source={source}"), Some(&a), b"").status, 200);
        assert!(!party.store().kept_dir(source).exists(), "deleting removes the kept files");

        assert_eq!(ask(port, "POST", "leave", Some(&a), b"").status, 200);
        assert_eq!(ask(port, "GET", "me", Some(&a), b"").status, 401, "a guest who left is unknown");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&kept);
    }

    /// Over a limit a guest is told when to come back, and nothing held is
    /// lost: uploads are limited per address across guests, and a full box
    /// refuses new pictures (and wishes) until room comes back.
    #[test]
    fn limits_refuse_without_losing_what_is_held() {
        let kept = std::env::temp_dir().join(format!("party-api-limits-{}", std::process::id()));
        let frames = pack(&[sample(512, 512), sample(512, 512)]);
        let held: usize = jpeg::split_frames(&frames).unwrap().iter().map(|f| jpeg::strip_metadata(f).unwrap().len()).sum();
        // Room for one such loop and its take, not for two.
        let party = Arc::new(Party::new(kept.clone(), Budget { total: held * 4, per_address: held * 4 }));
        let mut api = Api::new(party.clone());
        api.uploads_by_ip = RateLimiter::new(4.0, 0.001);
        let (mut server, dir) = start("abcdefgh", Arc::new(api));
        let port = server.port;
        let join = || sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        let (a, b) = (join(), join());

        let first = ask(port, "POST", "loop?fps=12&keep=me", Some(&a), &frames);
        assert_eq!(first.status, 200);
        let source = field(&first.body, "source");
        let full = ask(port, "POST", "loop?fps=12&keep=me", Some(&a), &frames);
        assert!(full.status == 503 && full.head.contains("Retry-After: 60"), "a full box says when to come back: {}", full.head);
        assert_eq!(full.body, b"the box is full of pictures right now: delete one of yours, or it is sent again in a minute", "a full box explains the wait");
        let wish = ask(port, "POST", &format!("wish?source={source}"), Some(&a), b"as a painting");
        assert_eq!(wish.status, 503, "a wish's take needs room too");
        assert_eq!(wish.body, full.body, "a refused wish explains the wait too");
        let me = sj::parse(&ask(port, "GET", "me", Some(&a), b"").body).unwrap();
        let mine = me.get("sources").and_then(Value::as_arr).unwrap();
        assert_eq!(mine.len(), 1, "the refused loop added nothing");
        assert_eq!(mine[0].get("frames").and_then(Value::as_u64), Some(2), "and the held one is whole");
        assert!(mine[0].get("wish").is_none(), "the refused wish changed nothing");

        assert_eq!(ask(port, "POST", &format!("delete?source={source}"), Some(&a), b"").status, 200);
        assert_eq!(ask(port, "POST", "loop?fps=12&keep=me", Some(&a), &frames).status, 200, "room again once something goes");
        let crowd = ask(port, "POST", "loop?fps=12&keep=me", Some(&b), &frames);
        assert!(crowd.status == 429 && crowd.head.contains("Retry-After"), "a fresh guest from the same address shares its limit: {}", crowd.head);
        assert_eq!(crowd.body, b"a lot has come from your connection at once: sending again shortly", "the rate limit explains the wait");
        let share = refused(StoreError::Share);
        assert!(share.status == 503 && share.headers.iter().any(|(k, v)| *k == "Retry-After" && v == "60"), "past an address's share: come back in a minute");
        assert_eq!(share.body, b"a lot has come from your connection right now: delete one of yours, or it is sent again in a minute", "the address's share explains the wait");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&kept);
    }

    /// What a stand-in for the hub's live redraw was opened with and sent.
    #[cfg(target_os = "linux")]
    #[derive(Default)]
    struct Redrawn {
        opened: Vec<(u32, u32)>,
        pictures: Vec<Vec<u8>>,
        prompts: Vec<String>,
        closed: usize,
    }

    /// The stand-in: a frame back for each picture.
    #[cfg(target_os = "linux")]
    struct Echo(Arc<std::sync::Mutex<Redrawn>>);

    #[cfg(target_os = "linux")]
    struct EchoRedraw(Arc<std::sync::Mutex<Redrawn>>, usize);

    #[cfg(target_os = "linux")]
    impl crate::live::Redrawer for Echo {
        fn open(&self, size: (u32, u32), prompt: &str, _seed: u64) -> Result<Box<dyn crate::live::Redraw>, String> {
            let mut seen = self.0.lock().unwrap();
            seen.opened.push(size);
            seen.prompts.push(prompt.into());
            Ok(Box::new(EchoRedraw(self.0.clone(), 0)))
        }
    }

    #[cfg(target_os = "linux")]
    impl crate::live::Redraw for EchoRedraw {
        fn picture(&mut self, au: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().pictures.push(au.to_vec());
            self.1 += 1;
            Ok(())
        }
        fn frame(&mut self) -> Result<Option<crate::image::Rgb>, String> {
            Ok((std::mem::take(&mut self.1) > 0).then(|| crate::image::Rgb { width: 2, height: 2, data: vec![0; 12] }))
        }
        fn prompt(&mut self, prompt: &str) -> Result<(), String> {
            self.0.lock().unwrap().prompts.push(prompt.into());
            Ok(())
        }
        fn resize(&mut self, _size: (u32, u32)) -> Result<(), String> {
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for EchoRedraw {
        fn drop(&mut self) {
            self.0.lock().unwrap().closed += 1;
        }
    }

    /// Two guests take the screen over TLS and UDP. An existing take can
    /// finish, each camera redraw starts from a keyframe, and a takeover
    /// retires the old call and its frames.
    #[cfg(target_os = "linux")]
    #[test]
    fn live_one_guest_at_a_time_over_a_real_call() {
        use makepad_webrtc::browser::{pictures, Browser};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};
        let kept = std::env::temp_dir().join(format!("party-api-live-{}", std::process::id()));
        let party = Arc::new(Party::new(kept.clone(), Budget::UNLIMITED));
        let redrawn = Arc::new(std::sync::Mutex::new(Redrawn::default()));
        party.live.redraw_with(Box::new(Echo(redrawn.clone())));
        crate::live::serve(&party, "127.0.0.1:0".parse().unwrap(), &["127.0.0.1".parse().unwrap()]).unwrap();
        let (mut server, dir) = start("abcdefgh", Arc::new(Api::new(party.clone())));
        let port = server.port;
        let join = || sj::parse(&ask(port, "POST", "join", None, b"").body).unwrap().get("token").and_then(Value::as_str).unwrap().to_string();
        let (a, b) = (join(), join());
        let state = |method: &str, path: &str, token: &str| sj::parse(&ask(port, method, path, Some(token), b"").body).unwrap();
        let named = |v: &Value| v.get("state").and_then(Value::as_str).map(str::to_string);
        let started = Instant::now();
        let until = |what: &str, done: &dyn Fn() -> bool| {
            while !done() {
                assert!(started.elapsed() < Duration::from_secs(8), "{what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };

        let up = state("POST", "live/queue", &a);
        assert_eq!((named(&up).as_deref(), up.get("available").and_then(Value::as_bool)), (Some("up"), Some(true)), "the first to queue is up");
        assert_eq!(ask(port, "POST", "live/offer", Some(&b), Browser::new("127.0.0.1:9").offer().as_bytes()).status, 409, "a guest outside live cannot call");
        assert_eq!(ask(port, "POST", "live/offer", Some(&a), b"v=0\r\n").status, 400, "an offer that cannot be answered");
        assert_eq!(ask(port, "POST", "live/wish", Some(&b), b"a wish").status, 409, "only the guest up wishes for the live redraw");
        assert_eq!(ask(port, "POST", "live/wish", Some(&a), b"a gun").status, 400, "the guest up cannot set an unkind wish");

        // A take is under way on the hub.
        party.live.take_running.store(true, Ordering::SeqCst);
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut phone = Browser::new(&socket.local_addr().unwrap().to_string());
        let answered = ask(port, "POST", "live/offer?w=1280&h=720", Some(&a), phone.offer().as_bytes());
        assert!(answered.status == 200 && answered.head.contains("application/sdp"), "{}", answered.head);
        let answer = String::from_utf8(answered.body).unwrap();
        let calls = std::net::SocketAddr::from(([127, 0, 0, 1], party.live.port().unwrap()));
        assert!(phone.connect(&socket, &answer, calls, Duration::from_secs(3)), "the call comes up");
        until("the call is up: the hub starts", &|| named(&state("GET", "live", &a)).as_deref() == Some("starting"));

        // Pictures arriving while the take finishes cannot open the redraw.
        let sent = pictures();
        let send = |phone: &mut Browser, socket: &std::net::UdpSocket, shift: u32, pictures: &[makepad_webrtc::rtp::AccessUnit]| {
            for picture in pictures {
                let mut picture = picture.clone();
                picture.timestamp += shift;
                for packet in phone.video(&picture) {
                    socket.send_to(&packet, calls).unwrap();
                }
            }
        };
        send(&mut phone, &socket, 0, &sent);
        let mut buf = [0u8; 2048];
        let quiet = Instant::now();
        while quiet.elapsed() < Duration::from_millis(300) {
            let _ = socket.recv_from(&mut buf);
        }
        let starting = state("GET", "live", &a);
        assert!(named(&starting).as_deref() == Some("starting") && starting.get("left").is_none(), "no clock while starting: {starting:?}");
        assert!(redrawn.lock().unwrap().opened.is_empty() && party.live.pictures.load(Ordering::Relaxed) == 0, "a take under way finishes first");

        // The take is done: the redraw opens at the camera's shape and asks
        // the phone for a keyframe.
        party.live.take_running.store(false, Ordering::SeqCst);
        let pli = |rtcp: &[u8]| {
            let mut at = 0;
            while at + 4 <= rtcp.len() {
                if rtcp[at + 1] == 206 && rtcp[at] & 0x1f == 1 {
                    return true;
                }
                at += 4 * (u16::from_be_bytes([rtcp[at + 2], rtcp[at + 3]]) as usize + 1);
            }
            false
        };
        let mut asked = false;
        while !asked {
            assert!(started.elapsed() < Duration::from_secs(8), "the phone is asked for a keyframe");
            if let Ok((n, _)) = socket.recv_from(&mut buf) {
                let rtcp = n > 8 && buf[0] >> 6 == 2 && (192..=223).contains(&buf[1]);
                asked |= rtcp && phone.feedback(&buf[..n]).is_some_and(|clear| pli(&clear));
            }
        }
        {
            let seen = redrawn.lock().unwrap();
            assert_eq!(seen.opened, vec![(512, 288)], "the redraw has the camera's shape");
            assert!(seen.prompts[0].starts_with("Redraw this picture as"), "no wish: the box picks a style");
        }
        // A picture that needs an earlier one is not sent on; from the
        // keyframe on they all are.
        send(&mut phone, &socket, 12_000, &sent[1..2]);
        send(&mut phone, &socket, 15_000, &sent);
        until("the pictures go to the redraw", &|| redrawn.lock().unwrap().pictures.len() >= sent.len());
        assert_eq!(redrawn.lock().unwrap().pictures, sent.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), "from the keyframe on");
        until("the first redrawn frame puts the guest live", &|| named(&state("GET", "live", &a)).as_deref() == Some("live"));
        let up = state("POST", "live/queue", &b);
        assert_eq!(named(&up).as_deref(), Some("up"), "the taker can call immediately");
        assert_eq!(named(&state("GET", "live", &b)).as_deref(), Some("up"));
        let live = state("GET", "live", &a);
        assert_eq!(named(&live).as_deref(), Some("live"), "asking alone leaves the screen with a");
        assert!(live.get("left").is_none() && up.get("waiting").is_none() && up.get("ahead").is_none(), "no live countdown or queue position");
        assert!(party.live.latest().is_some() && party.live.frames.load(Ordering::Relaxed) > 0, "the wall has a frame");
        let caption = party.live.board().and_then(|board| board.caption()).unwrap_or_default();
        assert_eq!(caption, "live", "the wall shows the current call during the takeover");
        assert!(party.live.slots().busy(), "the takes wait meanwhile");
        // The screen page shows the live redraw, and the guest's own phone
        // gets it too; "Just me" takes it off every screen.
        let on_screen = stream_until(port, "screen/stream", None, 4);
        assert_eq!(&on_screen[..2], [0xff, 0xd8], "the screen gets the live frame as a JPEG");
        let own = stream_until(port, "live/stream", Some(&a), 4);
        assert_eq!(&own[..2], [0xff, 0xd8], "the live guest's phone gets their redraw");
        assert_eq!(state("POST", "live/screen?on=0", &a).get("screen").and_then(Value::as_bool), Some(false));
        assert!(party.live.board().is_none(), "just me: the wall shows nothing of the call");
        let off = String::from_utf8(stream_until(port, "screen/stream", None, 1)).unwrap();
        assert!(off.contains("\"live\":null"), "nor does the screen page: {off}");
        state("POST", "live/screen?on=1", &a);

        assert_eq!(ask(port, "POST", "live/wish", Some(&a), b"a garden of paper lanterns").status, 200);
        until("the wish reaches the redraw", &|| redrawn.lock().unwrap().prompts.last().is_some_and(|p| p.contains("paper lanterns")));
        let skipped = ask(port, "POST", "live/wish", Some(&a), b"a gun");
        assert_eq!(skipped.status, 400, "the live guest cannot replace their wish with an unkind one");
        assert_eq!(skipped.body, b"the box skips that wish; try another", "the guest gets the reason for the refusal");

        let taker_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        taker_socket.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut taker = Browser::new(&taker_socket.local_addr().unwrap().to_string());
        let offer = ask(port, "POST", "live/offer?w=720&h=1280", Some(&b), taker.offer().as_bytes());
        assert_eq!(offer.status, 200, "the new guest can negotiate while a is still live");
        let old_frame = party.live.latest().unwrap().count;
        assert_eq!(redrawn.lock().unwrap().closed, 0, "the offer alone keeps a's redraw");
        assert_eq!(party.live.host_wish(Some("paper stars".into())), Ok(()));
        until("the host edits a's live redraw", &|| redrawn.lock().unwrap().prompts.last().is_some_and(|p| p.contains("paper stars")));
        let answer = String::from_utf8(offer.body).unwrap();
        assert!(taker.connect(&taker_socket, &answer, calls, Duration::from_secs(3)), "the taker's call connects");
        until("b takes the slot", &|| named(&state("GET", "live", &b)).as_deref() == Some("starting"));
        let bumped = state("GET", "live", &a);
        assert_eq!((named(&bumped).as_deref(), bumped.get("bumped").and_then(Value::as_bool)), (Some("out"), Some(true)));
        let pushed = stream_until(port, "live/stream", Some(&a), 1);
        assert_eq!(sj::parse(&pushed).unwrap(), bumped, "the displaced phone gets the same state on its stream");
        assert!(party.live.latest().is_none(), "b starts with no frame from a");
        until("b opens a fresh redraw", &|| redrawn.lock().unwrap().opened.len() == 2);
        {
            let seen = redrawn.lock().unwrap();
            assert_eq!(seen.closed, 1, "a's redraw has closed");
            assert_eq!(seen.opened[1], (288, 512), "b's redraw uses b's offer");
            assert!(seen.prompts.last().unwrap().contains("paper stars"), "the host wish carries into b's redraw");
        }
        let hung_up = |socket: &std::net::UdpSocket| {
            let mut alert = [0; 2048];
            socket.recv_from(&mut alert).is_ok_and(|(n, _)| n > 0 && alert[0] == 21)
        };
        until("the takeover hangs up a's call", &|| hung_up(&socket));
        send(&mut taker, &taker_socket, 0, &sent[..1]);
        until("b's first picture goes live", &|| named(&state("GET", "live", &b)).as_deref() == Some("live"));
        assert!(party.live.latest().unwrap().count > old_frame, "the screen has a frame from b");

        let left = state("POST", "live/leave", &b);
        assert_eq!(named(&left).as_deref(), Some("out"));
        assert!(left.get("bumped").is_none(), "leaving voluntarily clears the slot");
        until("leaving hangs up b's call", &|| hung_up(&taker_socket));
        until("leaving lets the hub and frame go", &|| redrawn.lock().unwrap().closed == 2 && party.live.latest().is_none());
        let again = state("POST", "live/queue", &a);
        assert_eq!(named(&again).as_deref(), Some("up"));
        assert!(again.get("bumped").is_none(), "rejoining clears the bump");
        assert!(party.live.stop(), "the host ends a slot");
        assert_eq!(named(&state("GET", "live", &a)).as_deref(), Some("out"));
        assert!(!party.live.slots().busy() && !party.live.stop(), "nobody up: the takes resume");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&kept);
    }
}
