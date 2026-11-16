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
//! - `GET frame?source=N&i=K` or `GET frame?take=N&i=K`: one JPEG of theirs.
//! - `POST keep?source=N&level=me|wall|keep`: wall or keep applies once the
//!   picture is redrawn.
//! - `POST delete?source=N`, `POST leave`.
//! - Live (`live.rs`): `POST live/queue` to wait for a turn, `GET live` for
//!   where the guest stands (the page asks every second or so; a page that
//!   stops asking loses its place), `POST live/offer` with the call's SDP
//!   offer once it is their turn (answered with the SDP answer), and
//!   `POST live/leave`. Each answers `{"available", "state":
//!   "out|waiting|up|live", "ahead", "left", "waiting"}`.
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
use crate::jpeg;
use crate::live::{Live, OfferError, Standing};
use crate::queue::Queue;
use crate::server::{busy, html, json, limits, slow_down, text, ByteBudget, Handler, RateLimiter, Request};
use crate::store::{Budget, GuestId, Jpeg, Keep, Persist, Quality, Source, SourceKind, Store, StoreError, TakeState, Who};
use makepad_bounded_http::{Method, Resp};
use makepad_strict_json::{self as sj, Value};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

const PAGE: &str = include_str!("../web/phone.html");

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

/// The party: its store, the takes waiting for the hub, and live.
pub struct Party {
    store: Mutex<Store>,
    pub queue: Queue,
    pub live: Live,
}

impl Party {
    /// `budget`: the bytes of pictures the store may hold and promise.
    pub fn new(kept: PathBuf, budget: Budget) -> Party {
        Party { store: Mutex::new(Store::new(kept, budget)), queue: Queue::default(), live: Live::default() }
    }

    /// A quarter of the machine's memory: the hub and the screen have the rest.
    pub fn default_budget() -> Budget {
        Budget::of(limits::memory_bytes() / 4)
    }

    /// The store, for a few field updates or a snapshot: never held across
    /// I/O, a decode or a hub call.
    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
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
        }
    }

    fn guest(&self, request: &Request<'_>) -> Result<GuestId, Resp> {
        let token = request.head.authorization.as_deref().and_then(|a| a.strip_prefix("Bearer ")).map(str::trim);
        token.and_then(|t| self.party.store().guest(t)).ok_or_else(|| text(401, "join first"))
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
        self.party.queue.push(take);
        Ok(json(200, sj::obj(vec![("source", Value::Int(source as i64)), ("take", Value::Int(take as i64))]).to_json()))
    }

    fn wish(&self, request: &mut Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        Self::limit(&self.uploads, &guest)?;
        Self::limit(&self.uploads_by_ip, &request.peer)?;
        let source = id(request, "source")?;
        let wish = clean_wish(request.body(MAX_WISH_BYTES)?)?;
        let take = self.party.store().wish(Who::Guest(guest), source, wish).map_err(refused)?;
        self.party.queue.push(take);
        Ok(json(200, sj::obj(vec![("take", Value::Int(take as i64))]).to_json()))
    }

    fn me(&self, guest: GuestId) -> Resp {
        let store = self.party.store();
        let sources: Vec<Value> = store.sources_of(guest).into_iter().map(|s| source_json(&store, s)).collect();
        json(200, sj::obj(vec![("version", Value::Int(store.version as i64)), ("sources", Value::Arr(sources))]).to_json())
    }

    fn frame(&self, request: &Request<'_>, guest: GuestId) -> Result<Resp, Resp> {
        let i = request.query("i").and_then(|i| i.parse::<usize>().ok()).ok_or_else(|| text(400, "i is a frame number"))?;
        let store = self.party.store();
        let frame = if let Some(take) = request.query("take") {
            let take = take.parse().ok().and_then(|t| store.take(t)).ok_or_else(|| text(404, "no such take"))?;
            store.source(Who::Guest(guest), take.source).map_err(not_found)?;
            take.frames.get(i).cloned()
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
        self.party.live.leave(guest);
        let persist = self.party.store().leave(guest);
        self.party.persist(persist);
        json(200, "{}".into())
    }

    /// Where `guest` stands in live, as the page shows it. Asking keeps their place.
    fn live_state(&self, guest: GuestId) -> Resp {
        let live = &self.party.live;
        let (standing, waiting, now) = live.change(|slots, now| {
            slots.seen(guest, now);
            (slots.standing(guest), slots.waiting(), now)
        });
        let left = |until: f64| Value::Int((until - now).max(0.0).ceil() as i64);
        let mut fields = vec![("available", Value::Bool(live.available())), ("waiting", Value::Int(waiting as i64))];
        match standing {
            Standing::Out => fields.push(("state", sj::s("out"))),
            Standing::Waiting { ahead } => fields.extend([("state", sj::s("waiting")), ("ahead", Value::Int(ahead as i64))]),
            Standing::Up { until } => fields.extend([("state", sj::s("up")), ("left", left(until))]),
            Standing::Starting => fields.push(("state", sj::s("starting"))),
            Standing::Live { until } => {
                fields.push(("state", sj::s("live")));
                fields.extend(until.map(|until| ("left", left(until))));
            }
        }
        json(200, sj::obj(fields).to_json())
    }

    fn live_queue(&self, guest: GuestId) -> Resp {
        if self.party.live.available() {
            self.party.live.change(|slots, now| slots.queue(guest, now));
        }
        self.live_state(guest)
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
            (Method::Get, ["me"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.me(guest))
            }
            (Method::Get, ["frame"]) => {
                Self::limit(&self.reads, &guest)?;
                self.frame(request, guest)
            }
            (Method::Get, ["live"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.live_state(guest))
            }
            (Method::Post, ["live", "queue"]) => {
                Self::limit(&self.reads, &guest)?;
                Ok(self.live_queue(guest))
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
            store.take_frame(take, Arc::from(photo.clone())).unwrap();
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
            store.take_frame(take, Arc::from(sample(512, 512))).unwrap();
            store.take_frame(take, Arc::from(sample(512, 512))).unwrap();
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
    }

    #[cfg(target_os = "linux")]
    impl Drop for EchoRedraw {
        fn drop(&mut self) {
            self.0.lock().unwrap().closed += 1;
        }
    }

    /// One guest live at a time over a real call, TLS and UDP: the queue,
    /// the call, a running take finishing before the hub starts (the slot's
    /// clock waiting), a keyframe asked for and the camera's pictures
    /// redrawn from it, the guest's wish, and leaving.
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
        let waits = state("POST", "live/queue", &b);
        assert_eq!((named(&waits).as_deref(), waits.get("ahead").and_then(Value::as_u64)), (Some("waiting"), Some(0)), "the next waits, next in line");
        assert_eq!(ask(port, "POST", "live/offer", Some(&b), Browser::new("127.0.0.1:9").offer().as_bytes()).status, 409, "not their turn: no call");
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

        // The camera's first pictures come while the take finishes: the hub
        // does not start, and the slot's clock waits.
        let sent = pictures();
        let send = |phone: &mut Browser, shift: u32, pictures: &[makepad_webrtc::rtp::AccessUnit]| {
            for picture in pictures {
                let mut picture = picture.clone();
                picture.timestamp += shift;
                for packet in phone.video(&picture) {
                    socket.send_to(&packet, calls).unwrap();
                }
            }
        };
        send(&mut phone, 0, &sent);
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
        send(&mut phone, 12_000, &sent[1..2]);
        send(&mut phone, 15_000, &sent);
        until("the pictures go to the redraw", &|| redrawn.lock().unwrap().pictures.len() >= sent.len());
        assert_eq!(redrawn.lock().unwrap().pictures, sent.iter().map(|p| p.data.clone()).collect::<Vec<_>>(), "from the keyframe on");
        until("the first redrawn frame puts the guest live", &|| named(&state("GET", "live", &a)).as_deref() == Some("live"));
        let live = state("GET", "live", &a);
        assert!(live.get("left").and_then(Value::as_u64).is_some_and(|left| left <= 60), "someone waits: the slot has its minute: {live:?}");
        assert!(party.live.latest().is_some() && party.live.frames.load(Ordering::Relaxed) > 0, "the wall has a frame");
        let caption = party.live.board().and_then(|board| board.caption()).unwrap_or_default();
        assert!(caption.starts_with("live · ") && caption.ends_with("s left · 1 waiting"), "{caption}");
        assert!(party.live.slots().busy(), "the takes wait meanwhile");

        assert_eq!(ask(port, "POST", "live/wish", Some(&a), b"a garden of paper lanterns").status, 200);
        until("the wish reaches the redraw", &|| redrawn.lock().unwrap().prompts.last().is_some_and(|p| p.contains("paper lanterns")));
        let skipped = ask(port, "POST", "live/wish", Some(&a), b"a gun");
        assert_eq!(skipped.status, 400, "the live guest cannot replace their wish with an unkind one");
        assert_eq!(skipped.body, b"the box skips that wish; try another", "the guest gets the reason for the refusal");

        let left = state("POST", "live/leave", &a);
        assert_eq!(named(&left).as_deref(), Some("out"));
        assert_eq!(named(&state("GET", "live", &b)).as_deref(), Some("up"), "and the next guest is up at once");
        let mut alert = false;
        while let Ok((n, _)) = socket.recv_from(&mut buf) {
            alert |= n > 0 && buf[0] == 21;
        }
        assert!(alert, "leaving hangs up the call");
        until("leaving lets the hub go", &|| redrawn.lock().unwrap().closed == 1);
        assert!(party.live.latest().is_none(), "and the wall's frame with it");
        assert!(party.live.stop(), "the host ends a slot");
        assert_eq!(named(&state("GET", "live", &b)).as_deref(), Some("out"));
        assert!(!party.live.slots().busy() && !party.live.stop(), "nobody up: the takes resume");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&kept);
    }
}
