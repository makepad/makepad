//! What guests' phones may ask, under `/p/<code>/`:
//!
//! - `GET` (the bare code): the phone page.
//! - `POST join`: a new guest; answers `{"token": ...}`, which every other
//!   call sends as `Authorization: Bearer <token>`.
//! - `POST loop?fps=N&keep=me|wall|keep`: a short loop, its JPEG frames as
//!   `application/x-party-frames` ([u32 BE length][JPEG], repeated).
//! - `POST photo?keep=...`: one JPEG.
//! - `POST wish?source=N`: what the guest would like (UTF-8 text); a new take.
//! - `GET me`: the guest's sources and takes.
//! - `GET frame?source=N&i=K` or `GET frame?take=N&i=K`: one JPEG of theirs.
//! - `POST keep?source=N&level=me|wall|keep`, `POST delete?source=N`,
//!   `POST leave`.
//! - `camtest...`: the camera check (camtest.rs), so a phone can be tried
//!   on the party's own address.
//!
//! Every picture is checked as a whole JPEG and its metadata (EXIF, location,
//! comments) stripped before the store holds it. Nothing else of the box is
//! reachable: the hub and every other service stay behind this.
//!
//! Memory is bounded without losing what a guest was told is held: uploads
//! are limited per guest and per address, bodies being read share one
//! budget, and the store has its own (`Store::bytes_committed`). Over any
//! of them a request is answered 429 or 503 with a Retry-After, before
//! anything is added, and the phone page sends it again then.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::camtest::CamTest;
use crate::jpeg;
use crate::queue::Queue;
use crate::server::{busy, html, json, limits, text, ByteBudget, Handler, RateLimiter, Request};
use crate::store::{GuestId, Jpeg, Keep, Persist, Quality, Source, SourceKind, Store, StoreError, TakeState, Who};
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

/// The party: its store and the takes waiting for the hub.
pub struct Party {
    store: Mutex<Store>,
    pub queue: Queue,
}

impl Party {
    /// `budget`: the bytes of pictures the store may hold and promise.
    pub fn new(kept: PathBuf, budget: usize) -> Party {
        Party { store: Mutex::new(Store::new(kept, budget)), queue: Queue::default() }
    }

    /// A quarter of the machine's memory: the hub and the screen have the rest.
    pub fn default_budget() -> usize {
        limits::memory_bytes() / 4
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
            .map_err(|wait| text(429, "slow down").with_header("Retry-After", format!("{}", wait.as_secs().max(1))))
    }

    fn join(&self, request: &Request<'_>) -> Result<Resp, Resp> {
        Self::limit(&self.joins, &request.peer)?;
        let mut bytes = [0u8; 16];
        makepad_network::tls::os_random(&mut bytes).map_err(|_| text(503, "no randomness"))?;
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
        let (body, _reading) = request.body_within(if photo { MAX_PHOTO_BYTES } else { MAX_LOOP_BYTES }, &self.inflight)?;
        let pieces = if photo { vec![body.as_slice()] } else { jpeg::split_frames(&body).map_err(|e| text(400, e))? };
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
            let (source, persist) = store.add_source(guest, kind, frames, None, keep).map_err(refused)?;
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
        let body = request.body(MAX_WISH_BYTES)?;
        let wish = String::from_utf8(body).map_err(|_| text(400, "a wish is UTF-8 text"))?;
        let wish: String = wish.trim().chars().filter(|c| !c.is_control()).collect();
        if wish.chars().count() > MAX_WISH_CHARS {
            return Err(text(400, "a wish is at most 300 characters"));
        }
        let take = self.party.store().wish(Who::Guest(guest), source, (!wish.is_empty()).then_some(wish)).map_err(refused)?;
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
        let persist = self.party.store().leave(guest);
        self.party.persist(persist);
        json(200, "{}".into())
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
            _ => Err(text(404, "not found")),
        }
    }
}

impl Handler for Api {
    fn handle(&self, request: &mut Request<'_>) -> Resp {
        self.route(request).unwrap_or_else(|resp| resp)
    }
}

fn not_found(_: StoreError) -> Resp {
    text(404, "not yours or gone")
}

fn refused(error: StoreError) -> Resp {
    match error {
        StoreError::NotFound => not_found(error),
        StoreError::Full => busy(60, "the box is full of pictures right now: delete one of yours, or it is sent again in a minute"),
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

    /// A guest's evening over real TLS: join, a loop, a wish, keep, delete,
    /// leave; another guest sees none of it, and nothing is held without a token.
    #[test]
    fn a_guest_round_trip() {
        let kept = std::env::temp_dir().join(format!("party-api-test-{}", std::process::id()));
        let party = Arc::new(Party::new(kept.clone(), usize::MAX));
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
        assert_eq!(party.queue.pop(), Some(field(&sent.body, "take")), "the loop's first take waits for the hub");

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
        assert!(kept.join(source.to_string()).join("source-001.jpg").exists(), "keep it writes the frames");
        assert_eq!(ask(port, "POST", &format!("delete?source={source}"), Some(&b), b"").status, 404, "only its guest deletes it");
        assert_eq!(ask(port, "POST", &format!("delete?source={source}"), Some(&a), b"").status, 200);
        assert!(!kept.join(source.to_string()).exists(), "deleting removes the kept files");

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
        let party = Arc::new(Party::new(kept.clone(), held * 4));
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
        let wish = ask(port, "POST", &format!("wish?source={source}"), Some(&a), b"as a painting");
        assert_eq!(wish.status, 503, "a wish's take needs room too");
        let me = sj::parse(&ask(port, "GET", "me", Some(&a), b"").body).unwrap();
        let mine = me.get("sources").and_then(Value::as_arr).unwrap();
        assert_eq!(mine.len(), 1, "the refused loop added nothing");
        assert_eq!(mine[0].get("frames").and_then(Value::as_u64), Some(2), "and the held one is whole");
        assert!(mine[0].get("wish").is_none(), "the refused wish changed nothing");

        assert_eq!(ask(port, "POST", &format!("delete?source={source}"), Some(&a), b"").status, 200);
        assert_eq!(ask(port, "POST", "loop?fps=12&keep=me", Some(&a), &frames).status, 200, "room again once something goes");
        let crowd = ask(port, "POST", "loop?fps=12&keep=me", Some(&b), &frames);
        assert!(crowd.status == 429 && crowd.head.contains("Retry-After"), "a fresh guest from the same address shares its limit: {}", crowd.head);
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&kept);
    }
}
