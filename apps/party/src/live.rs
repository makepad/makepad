//! One guest's phone camera streams over WebRTC to the party's big screen,
//! redrawn by the box's hub. Pressing live immediately admits the guest to
//! call. The current guest keeps the screen until the new call connects.
//! A newer taker replaces a taker who is still connecting.
//!
//! [`Slots`] gives each caller [`CONNECT_WITHIN`] seconds to connect.
//! A connected call starts its redraw after any running take finishes and
//! goes live on its first redrawn frame. The guest keeps it until they
//! leave, their call ends, their page goes quiet for [`SEEN_WITHIN`], or
//! another call takes over. A displaced guest sees that they were bumped
//! until they ask to go live again or leave. New takes wait while anyone
//! holds the slot. A redraw that fails to start within [`HUB_START_WITHIN`]
//! of opening releases the slot.
//!
//! On Linux, the live thread handles call events, expires slots and feeds
//! the connected camera to its [`Redrawer`] from a keyframe on. The wall
//! reads its redrawn frames from [`Live::latest`]. Guests get their status
//! and frames from the phone page's HTTP requests.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::image::Rgb;
use crate::store::{GuestId, Jpeg};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
pub use makepad_webrtc::calls::CallId;
#[cfg(not(target_os = "linux"))]
pub type CallId = u64;

/// From being called up to the call being up.
pub const CONNECT_WITHIN: f64 = 20.0;
/// A guest whose page has not asked for this long has gone. The page asks every second or two.
pub const SEEN_WITHIN: f64 = 10.0;
/// From the hub's session opening to its first redrawn frame: a model
/// load at worst.
pub const HUB_START_WITHIN: f64 = 90.0;
/// The longer side of the live redraw: what the fast model draws at
/// interactive speed.
pub const LIVE_SIDE: u32 = 512;
/// Until the first redrawn frame, the phone is asked for a keyframe this
/// often: an ask can be lost, and a hub that waited for its GPU may have
/// missed the last one.
pub const KEYFRAME_AGAIN: f64 = 0.4;
/// The live frames' JPEG quality, for the screen page and the guest's phone.
const LIVE_JPEG_QUALITY: u8 = 82;

#[derive(Clone, Default)]
struct Wish {
    count: u64,
    text: Option<String>,
}

struct Slot {
    guest: GuestId,
    seen: f64,
    standing: Standing,
    call: Option<CallId>,
    size: (u32, u32),
    screen: bool,
    wish: Wish,
}

/// Where a guest stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Standing {
    Out,
    /// The guest can call immediately and must connect by `until`.
    Up { until: f64 },
    /// The call is up and the hub is preparing its first frame.
    Starting,
    /// The hub is redrawing this guest's camera.
    Live,
}

/// A redrawn live frame, as texels for the wall's texture (made on the live
/// thread, so the wall only copies them in) and as a JPEG for the screen page
/// and the guest's phone.
pub struct LiveFrame {
    /// Which redrawn frame of the call (from 1).
    pub count: u64,
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u32>,
    rgb: Rgb,
    jpeg: OnceLock<Option<Jpeg>>,
}

impl LiveFrame {
    pub fn new(count: u64, rgb: Rgb) -> Self {
        let bgra = crate::image::bgra(&rgb);
        LiveFrame { count, width: rgb.width, height: rgb.height, bgra, rgb, jpeg: OnceLock::new() }
    }

    /// The frame as a JPEG: encoded by the first stream that sends it (a
    /// few milliseconds at the live size), never on the live thread, and
    /// not at all while no screen page or phone watches.
    pub fn jpeg(&self) -> Option<Jpeg> {
        self.jpeg
            .get_or_init(|| match crate::image::encode_jpeg(&self.rgb, LIVE_JPEG_QUALITY) {
                Ok(jpeg) => Some(Arc::from(jpeg)),
                Err(e) => {
                    eprintln!("[party] live: frame {} as a JPEG: {e}", self.count);
                    None
                }
            })
            .clone()
    }
}

/// The live slot, for the wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Board {
    pub standing: Standing,
}

impl Board {
    /// What the screen writes under the connected guest's picture.
    pub fn caption(&self) -> Option<String> {
        match self.standing {
            Standing::Starting => Some("starting…".into()),
            Standing::Live => Some("live".into()),
            _ => None,
        }
    }
}

/// The box's own camera, redrawn while no guest is live: the capture thread
/// leaves a frame here whenever the live thread asks for one, so frames are
/// converted only as fast as the hub redraws them.
#[derive(Default)]
pub struct CameraFeed {
    /// The live thread wants the next frame (the last one's redraw came back).
    wanted: AtomicBool,
    /// The frame it asked for, at the redraw's size.
    frame: Mutex<Option<Rgb>>,
    /// When the camera last gave a frame, and the redraw's size for it.
    seen: Mutex<Option<(Instant, (u32, u32))>>,
}

impl CameraFeed {
    /// The capture thread has a `width` x `height` frame: the size to
    /// convert it to when the live thread wants one now.
    pub fn seen(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        let size = live_size(width, height);
        *lock(&self.seen) = Some((Instant::now(), size));
        self.wanted.swap(false, std::sync::atomic::Ordering::AcqRel).then_some(size)
    }

    /// The frame asked for (`None`: the capture could not make it; the
    /// next one is taken instead).
    pub fn give(&self, frame: Option<Rgb>) {
        match frame {
            Some(frame) => *lock(&self.frame) = Some(frame),
            None => self.wanted.store(true, std::sync::atomic::Ordering::Release),
        }
    }

    /// The redraw's size for a camera that gave a frame within `within`.
    pub fn fresh(&self, within: Duration) -> Option<(u32, u32)> {
        let seen = *lock(&self.seen);
        seen.filter(|(at, _)| at.elapsed() <= within).map(|(_, size)| size)
    }
}

/// Seconds after its last poll that a guest's page counts as open (the
/// page polls every few seconds).
pub const PAGE_WITHIN: f64 = 12.0;

/// A redraw's size when the phone did not say its camera's: 4:3, as a
/// phone camera's default.
const DEFAULT_SIZE: (u32, u32) = (LIVE_SIDE, LIVE_SIDE / 4 * 3);

/// The redraw's size for a camera of `width` x `height`: [`LIVE_SIDE`] on
/// the longer side, both sides a multiple of 16.
pub fn live_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return DEFAULT_SIZE;
    }
    let short = |a: u32, b: u32| ((a as u64 * LIVE_SIDE as u64 / b as u64) as u32 / 16 * 16).max(16);
    if width >= height {
        (LIVE_SIDE, short(height, width))
    } else {
        (short(width, height), LIVE_SIDE)
    }
}

/// The guest holding the screen and the newest guest trying to take it.
#[derive(Default)]
pub struct Slots {
    current: Option<Slot>,
    taker: Option<Slot>,
    bumped: HashSet<GuestId>,
    /// The newest wish, for the host's prompt box. A host wish also
    /// applies to guests who arrive while it is the newest.
    wish: Wish,
    wished_by: Option<GuestId>,
    /// The party's prompts so far, oldest first, each once: shared by the
    /// box and every phone, kept in memory for this party only.
    history: Vec<String>,
    /// What the newest typist is typing (guest, text, when), for the box.
    draft: Option<(GuestId, String, f64)>,
    /// When each guest's open page last polled: who is at the party now.
    pages: HashMap<GuestId, f64>,
    /// Where the history goes as it changes (`Live::keep_history`).
    history_out: Option<std::sync::mpsc::Sender<Vec<String>>>,
}

impl Slots {
    fn slot(&self, guest: GuestId) -> Option<&Slot> {
        self.current.iter().chain(&self.taker).find(|s| s.guest == guest)
    }

    fn slot_mut(&mut self, guest: GuestId) -> Option<&mut Slot> {
        self.current.iter_mut().chain(&mut self.taker).find(|s| s.guest == guest)
    }

    /// Admits the guest to call now. Returns a displaced taker's call to hang up.
    pub fn queue(&mut self, guest: GuestId, now: f64) -> Option<CallId> {
        self.bumped.remove(&guest);
        if let Some(slot) = self.slot_mut(guest) {
            slot.seen = now;
            return None;
        }
        let wish = if self.wished_by.is_none_or(|g| g == guest) { self.wish.clone() } else { Wish::default() };
        let slot = Slot {
            guest,
            seen: now,
            standing: Standing::Up { until: now + CONNECT_WITHIN },
            call: None,
            size: DEFAULT_SIZE,
            screen: true,
            wish,
        };
        if self.current.is_none() {
            self.current = Some(slot);
            return None;
        }
        let displaced = self.taker.replace(slot)?;
        self.bumped.insert(displaced.guest);
        displaced.call
    }

    /// The box takes the screen for its own camera, as a phone's tap takes
    /// it: the guest on it, and one about to take it, are bumped. Their calls,
    /// to hang up.
    pub fn box_takes(&mut self) -> Vec<CallId> {
        let mut calls = Vec::new();
        for slot in [self.current.take(), self.taker.take()].into_iter().flatten() {
            self.bumped.insert(slot.guest);
            calls.extend(slot.call);
        }
        calls
    }

    /// `guest`'s page polled; pages quiet for [`PAGE_WITHIN`] are gone.
    pub fn page(&mut self, guest: GuestId, now: f64) {
        self.pages.retain(|_, at| now - *at < PAGE_WITHIN);
        self.pages.insert(guest, now);
    }

    /// Other guests with the page open now: alone, a phone goes live by itself.
    pub fn others(&self, guest: GuestId, now: f64) -> usize {
        self.pages.iter().filter(|(g, at)| **g != guest && now - **at < PAGE_WITHIN).count()
    }

    /// `text` becomes the newest prompt of the history.
    fn remember(&mut self, text: &str) {
        self.history.retain(|t| t != text);
        self.history.push(text.to_string());
        if let Some(out) = &self.history_out {
            let _ = out.send(self.history.clone());
        }
    }

    pub fn bumped(&self, guest: GuestId) -> bool {
        self.bumped.contains(&guest)
    }

    /// Whether the guest shows their call on the party's screens.
    pub fn set_screen(&mut self, guest: GuestId, screen: bool) {
        if let Some(slot) = self.slot_mut(guest) {
            slot.screen = screen;
        }
    }

    pub fn screen(&self, guest: GuestId) -> bool {
        self.slot(guest).is_none_or(|s| s.screen)
    }

    /// The guest's page asked again.
    pub fn seen(&mut self, guest: GuestId, now: f64) {
        if let Some(slot) = self.slot_mut(guest) {
            slot.seen = now;
        }
    }

    /// Removes the guest and returns their call to hang up. A taker keeps
    /// their original connect deadline if the current guest leaves first.
    pub fn leave(&mut self, guest: GuestId) -> Option<CallId> {
        self.bumped.remove(&guest);
        if self.current_guest() == Some(guest) {
            let old = std::mem::replace(&mut self.current, self.taker.take());
            return old.and_then(|s| s.call);
        }
        self.taker.take_if(|s| s.guest == guest).and_then(|s| s.call)
    }

    pub fn standing(&self, guest: GuestId) -> Standing {
        self.slot(guest).map_or(Standing::Out, |s| s.standing)
    }

    pub fn may_call(&self, guest: GuestId) -> bool {
        matches!(self.standing(guest), Standing::Up { .. })
    }

    /// Records an offer and returns any earlier offer's call to hang up.
    pub fn call(&mut self, guest: GuestId, call: CallId, size: (u32, u32)) -> Result<Option<CallId>, ()> {
        let slot = self.slot_mut(guest).filter(|s| matches!(s.standing, Standing::Up { .. })).ok_or(())?;
        slot.size = size;
        Ok(slot.call.replace(call))
    }

    /// A taker becomes current only when their own call connects. Returns
    /// the previous guest's call to hang up, or Err for a stale event.
    pub fn connected(&mut self, call: CallId) -> Result<Option<CallId>, ()> {
        if let Some(mut taker) = self.taker.take_if(|s| s.call == Some(call)) {
            taker.standing = Standing::Starting;
            let old = self.current.replace(taker);
            return Ok(old.and_then(|s| {
                self.bumped.insert(s.guest);
                s.call
            }));
        }
        let slot = self.current.as_mut()
            .filter(|s| s.call == Some(call) && matches!(s.standing, Standing::Up { .. }))
            .ok_or(())?;
        slot.standing = Standing::Starting;
        Ok(None)
    }

    /// The hub delivered this call's first redrawn frame.
    pub fn started(&mut self, call: CallId) -> bool {
        if let Some(s) = self.current.as_mut().filter(|s| s.call == Some(call) && s.standing == Standing::Starting) {
            s.standing = Standing::Live;
            return true;
        }
        false
    }

    /// Handles a call ending, including a taker's call that never connected.
    pub fn gone(&mut self, call: CallId) -> bool {
        let guest = self.current.iter().chain(&self.taker).find(|s| s.call == Some(call)).map(|s| s.guest);
        guest.is_some_and(|g| self.leave(g).is_some())
    }

    /// Expires quiet pages and late connections, returning their calls to hang up.
    pub fn tick(&mut self, now: f64) -> Vec<CallId> {
        let mut hang_up = Vec::new();
        for slot in [&mut self.current, &mut self.taker] {
            let expired = slot.take_if(|s| now - s.seen > SEEN_WITHIN || matches!(s.standing, Standing::Up { until } if now >= until));
            hang_up.extend(expired.and_then(|s| s.call));
        }
        if self.current.is_none() {
            self.current = self.taker.take();
        }
        hang_up
    }

    /// The connected call, its guest and size, and whether its redraw has started.
    /// The call the screen's redraw is fed: the screen's guest's from its
    /// offer on (the redraw switches to it while it connects).
    pub fn redraw_call(&self) -> Option<(CallId, GuestId, (u32, u32), bool)> {
        let s = self.current.as_ref()?;
        Some((s.call?, s.guest, s.size, s.standing == Standing::Live))
    }
    pub fn connected_call(&self) -> Option<(CallId, GuestId, (u32, u32), bool)> {
        let s = self.current.as_ref().filter(|s| matches!(s.standing, Standing::Starting | Standing::Live))?;
        Some((s.call?, s.guest, s.size, s.standing == Standing::Live))
    }

    pub fn shown(&self) -> Option<Standing> {
        self.current.as_ref().filter(|s| s.screen).map(|s| s.standing)
    }

    pub fn current_guest(&self) -> Option<GuestId> {
        self.current.as_ref().map(|s| s.guest)
    }

    /// A guest is up or connected, so new takes wait.
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }
}

/// A live redraw of a camera: access units in, redrawn frames out. Opened
/// and used on the live thread only.
pub trait Redraw {
    /// One access unit (Annex-B), from a keyframe on; never waits for the model.
    fn picture(&mut self, au: &[u8]) -> Result<(), String>;
    /// The latest redrawn frame since the last call, without waiting; an
    /// error once the redraw has ended.
    fn frame(&mut self) -> Result<Option<Rgb>, String>;
    fn prompt(&mut self, prompt: &str) -> Result<(), String>;
    /// Redraws at `size` from the next picture on: the camera's pictures
    /// changed shape (the phone turned, or its first keyframe showed the
    /// size the offer did not say right).
    fn resize(&mut self, size: (u32, u32)) -> Result<(), String>;
    /// One RGB frame at the redraw's size (the box camera's redraw); sent
    /// once the redraw of the one before came back.
    fn raw(&mut self, _frame: &Rgb) -> Result<(), String> {
        Err("this redraw takes no camera frames".into())
    }
}

/// Opens live redraws: the box's hub, or a stand-in in tests.
pub trait Redrawer: Send + Sync {
    fn open(&self, size: (u32, u32), prompt: &str, seed: u64) -> Result<Box<dyn Redraw>, String>;
    /// A redraw fed RGB frames ([`Redraw::raw`]): the box's own camera.
    fn open_raw(&self, _size: (u32, u32), _prompt: &str, _seed: u64) -> Result<Box<dyn Redraw>, String> {
        Err("no redraw for the box camera".into())
    }
}

/// The slots and the calls of a party, and the thread between them.
pub struct Live {
    slots: Mutex<Slots>,
    /// Signalled when the slot is taken or freed: the hub worker waits on
    /// it while a guest has the slot.
    changed: Condvar,
    started: Instant,
    #[cfg(target_os = "linux")]
    calls: OnceLock<makepad_webrtc::calls::Calls>,
    redrawer: OnceLock<Box<dyn Redrawer>>,
    /// Set by the hub worker while it runs a take: the live redraw opens
    /// once it is done.
    pub take_running: AtomicBool,
    /// The latest redrawn frame of the live call, for the wall; `framed`
    /// is signalled when it changes.
    latest: Mutex<Option<Arc<LiveFrame>>>,
    framed: Condvar,
    /// Redrawn frames so far (the wall redraws when it moves).
    pub frames: AtomicU64,
    /// Access units of the live call sent to the redraw.
    pub pictures: AtomicU64,
    /// The screen's redraw is open at the hub: the hub worker's warm-up
    /// opens no session beside it (two do not fit the GPU).
    pub redrawing: AtomicBool,
    /// The box's own camera, for the screen between live guests.
    pub camera: Arc<CameraFeed>,
    /// The screen's redraw has a picture up the wall may show (the box
    /// camera's, or a call's on the screen): it stays across a takeover until
    /// the new source's frames come. A "Just me" call's frames never set it.
    picture_up: AtomicBool,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            slots: Mutex::new(Slots::default()),
            changed: Condvar::new(),
            started: Instant::now(),
            #[cfg(target_os = "linux")]
            calls: OnceLock::new(),
            redrawer: OnceLock::new(),
            take_running: AtomicBool::new(false),
            latest: Mutex::new(None),
            framed: Condvar::new(),
            frames: AtomicU64::new(0),
            pictures: AtomicU64::new(0),
            redrawing: AtomicBool::new(false),
            camera: Arc::default(),
            picture_up: AtomicBool::new(false),
        }
    }
}

/// Why an offer (or a wish) was not taken.
#[derive(Debug, PartialEq, Eq)]
pub enum OfferError {
    /// Live is off on this box.
    Off,
    /// Not this guest's turn.
    NotUp,
    /// The offer cannot be answered (why, for the phone).
    Offer(String),
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

impl Live {
    /// Seconds since the party started: the slots' clock.
    pub fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// The slots, for a few field updates: never held across I/O.
    pub fn slots(&self) -> MutexGuard<'_, Slots> {
        lock(&self.slots)
    }

    /// What redraws the live call; once, at the start.
    pub fn redraw_with(&self, redrawer: Box<dyn Redrawer>) {
        let _ = self.redrawer.set(redrawer);
    }

    /// Whether calls are taken here: calls and a hub to redraw them.
    pub fn available(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.calls.get().is_some() && self.redrawer.get().is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// The UDP port calls come to.
    pub fn port(&self) -> Option<u16> {
        #[cfg(target_os = "linux")]
        {
            self.calls.get().map(|c| c.port())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// The latest redrawn frame of the live call.
    pub fn latest(&self) -> Option<Arc<LiveFrame>> {
        lock(&self.latest).clone()
    }

    /// The latest redrawn frame once it is newer than frame `after` (frames
    /// count up across calls), waiting at most `timeout` for it: for a
    /// stream's thread, never a UI thread.
    pub fn frame_after(&self, after: u64, timeout: Duration) -> Option<Arc<LiveFrame>> {
        let newer = |f: &Option<Arc<LiveFrame>>| f.as_ref().is_some_and(|f| f.count > after);
        let latest = lock(&self.latest);
        let (latest, _) = self.framed.wait_timeout_while(latest, timeout, |f| !newer(f)).unwrap_or_else(|p| p.into_inner());
        latest.clone().filter(|f| f.count > after)
    }

    /// A frame for the guest's own phone, or for the public screen. A
    /// takeover while the stream waits must not send it the next call's frame.
    pub(crate) fn stream_frame(&self, guest: Option<GuestId>, after: u64, timeout: Duration) -> Option<Arc<LiveFrame>> {
        let visible_call = || {
            let slots = self.slots();
            let s = slots.current.as_ref()?;
            let visible = guest.map_or(s.screen, |g| g == s.guest);
            if visible && s.standing == Standing::Live {
                s.call
            } else {
                None
            }
        };
        let call = visible_call();
        let frame = self.frame_after(if call.is_some() { after } else { u64::MAX }, timeout)?;
        (visible_call() == call).then_some(frame)
    }

    #[cfg(any(target_os = "linux", test))]
    fn set_latest(&self, frame: Option<LiveFrame>) {
        *lock(&self.latest) = frame.map(Arc::new);
        self.framed.notify_all();
    }

    /// The live slot as the wall shows it, if a guest has it (or the box
    /// camera's picture between guests). A guest who asked for "Just me" is
    /// on no screen from that moment, whatever the redraw last showed.
    pub fn board(&self) -> Option<Board> {
        let slots = self.slots();
        if slots.current.as_ref().is_some_and(|s| s.call.is_some() && !s.screen) {
            return None;
        }
        let shown = slots.shown();
        if shown != Some(Standing::Live) && self.picture_up.load(std::sync::atomic::Ordering::Relaxed) {
            return Some(Board { standing: Standing::Live });
        }
        Some(Board { standing: shown? })
    }

    /// The host ends the live guest's slot (hanging up their call); true
    /// when a guest had it.
    pub fn stop(&self) -> bool {
        let stopped = self.change(|slots, _| {
            let guest = slots.current.as_ref()?.guest;
            Some(slots.leave(guest))
        });
        if let Some(Some(call)) = stopped {
            self.hang_up(call);
        }
        stopped.is_some()
    }

    /// Expires slots before handling the next request or event, then hangs up expired calls.
    pub fn change<R>(&self, f: impl FnOnce(&mut Slots, f64) -> R) -> R {
        let (result, hang_up, flipped) = {
            let mut slots = self.slots();
            let now = self.now();
            let before = slots.busy();
            let hang_up = slots.tick(now);
            let result = f(&mut slots, now);
            (result, hang_up, before != slots.busy())
        };
        if flipped {
            self.changed.notify_all();
        }
        for call in hang_up {
            self.hang_up(call);
        }
        result
    }

    /// A guest withdraws or leaves the screen.
    pub fn leave(&self, guest: GuestId) {
        if let Some(call) = self.change(|slots, _| slots.leave(guest)) {
            self.hang_up(call);
        }
    }

    pub(crate) fn hang_up(&self, _call: CallId) {
        #[cfg(target_os = "linux")]
        if let Some(calls) = self.calls.get() {
            calls.close(_call);
        }
    }

    /// The live guest's wish for their redraw (`None`: the box picks).
    pub fn wish(&self, guest: GuestId, wish: Option<String>) -> Result<(), OfferError> {
        self.set_wish(Some(guest), wish)
    }

    /// The host's wish applies to the current guest and carries into a takeover.
    pub fn host_wish(&self, wish: Option<String>) -> Result<(), OfferError> {
        self.set_wish(None, wish)
    }

    /// The newest accepted wish and its revision, for the host's prompt box.
    pub fn wish_text(&self) -> (u64, Option<String>) {
        let slots = self.slots();
        (slots.wish.count, slots.wish.text.clone())
    }

    fn set_wish(&self, guest: Option<GuestId>, text: Option<String>) -> Result<(), OfferError> {
        let kind = text.as_deref().is_none_or(crate::worker::kind_wish);
        self.change(|slots, _| {
            if guest.is_some_and(|g| slots.slot(g).is_none()) {
                return Err(OfferError::NotUp);
            }
            if !kind {
                return Err(OfferError::Offer("the box skips that wish; try another".into()));
            }
            let wish = Wish { count: slots.wish.count + 1, text };
            for slot in slots.current.iter_mut().chain(&mut slots.taker) {
                if guest.is_none_or(|g| g == slot.guest) {
                    slot.wish = wish.clone();
                }
            }
            if let Some(text) = &wish.text {
                slots.remember(text);
            }
            if slots.draft.as_ref().is_some_and(|d| Some(d.0) == guest) {
                slots.draft = None;
            }
            slots.wish = wish;
            slots.wished_by = guest;
            Ok(())
        })
    }

    /// What `guest` is typing on their phone (empty: they stopped); the
    /// box shows the newest typist's. A draft the box would skip as a wish
    /// is not shown.
    pub fn draft(&self, guest: GuestId, text: &str) {
        let text = text.trim();
        let shown = !text.is_empty() && crate::worker::kind_wish(text);
        self.change(|slots, now| match shown {
            true => slots.draft = Some((guest, text.to_string(), now)),
            false if slots.draft.as_ref().is_some_and(|d| d.0 == guest) => slots.draft = None,
            false => {}
        });
    }

    /// The newest typist's draft, while they typed within `within` seconds.
    pub fn draft_text(&self, within: f64) -> Option<(GuestId, String)> {
        let now = self.now();
        self.slots().draft.as_ref().filter(|d| now - d.2 <= within).map(|d| (d.0, d.1.clone()))
    }

    /// The box's "Use my camera": the screen goes to the box's own camera
    /// and the phone on it is bumped; the newest press, a phone's or the
    /// box's, has the screen.
    pub fn box_takes(&self) {
        for call in self.change(|slots, _| slots.box_takes()) {
            self.hang_up(call);
        }
    }

    /// The box could take the screen for its camera: a phone has it, and
    /// the camera is giving frames.
    pub fn box_camera_offer(&self) -> bool {
        self.slots().current.is_some() && self.camera.fresh(CAMERA_STALL).is_some()
    }

    /// A prompt a guest gave a loop, for the history.
    pub fn remember(&self, text: &str) {
        self.slots().remember(text);
    }

    /// Keeps the prompt history in `file` (JSON, newest first) so a
    /// restart has it: loads it, and its newest as the prompt set now, and
    /// writes it on a thread of its own whenever it changes (a temp file,
    /// then a rename).
    pub fn keep_history(&self, file: std::path::PathBuf) {
        let kept: Vec<String> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| makepad_strict_json::parse(&bytes).ok())
            .and_then(|v| v.as_arr().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()))
            .unwrap_or_default();
        let (out, written) = std::sync::mpsc::channel::<Vec<String>>();
        let spawned = std::thread::Builder::new().name("party-history".into()).spawn(move || {
            while let Ok(mut history) = written.recv() {
                // Only the newest matters.
                while let Ok(newer) = written.try_recv() {
                    history = newer;
                }
                let json = makepad_strict_json::Value::Arr(history.iter().rev().map(|p| makepad_strict_json::s(p.clone())).collect()).to_json();
                let temp = file.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&temp, json).and_then(|()| std::fs::rename(&temp, &file)) {
                    eprintln!("[party] the prompt history was not written: {e}");
                }
            }
        });
        let mut slots = self.slots();
        slots.history = kept.into_iter().rev().collect();
        if let Some(newest) = slots.history.last().cloned() {
            slots.wish = Wish { count: slots.wish.count + 1, text: Some(newest) };
            slots.wished_by = None;
        }
        if spawned.is_ok() {
            slots.history_out = Some(out);
        }
        eprintln!("[party] {} prompts in the history from before", slots.history.len());
    }

    /// The party's prompts, newest first.
    pub fn history(&self) -> Vec<String> {
        self.slots().history.iter().rev().cloned().collect()
    }

    /// `guest`'s offer, if it is their turn, from a camera of `camera`
    /// pixels (0 x 0: not said): the answer to send back.
    pub fn offer(&self, guest: GuestId, sdp: &str, camera: (u32, u32)) -> Result<String, OfferError> {
        #[cfg(target_os = "linux")]
        {
            let calls = self.calls.get().filter(|_| self.available()).ok_or(OfferError::Off)?;
            if !self.change(|slots, _| slots.may_call(guest)) {
                return Err(OfferError::NotUp);
            }
            let asked = Instant::now();
            let (call, answer) = calls.offer(sdp).map_err(|e| OfferError::Offer(format!("{e:?}")))?;
            match self.change(|slots, _| slots.call(guest, call, live_size(camera.0, camera.1))) {
                Ok(replaced) => {
                    replaced.into_iter().for_each(|old| calls.close(old));
                    eprintln!("[party] live: call {call}: offered at {}x{}, answered in {} ms", camera.0, camera.1, asked.elapsed().as_millis());
                    Ok(answer)
                }
                Err(()) => {
                    calls.close(call);
                    Err(OfferError::NotUp)
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (guest, sdp, camera);
            Err(OfferError::Off)
        }
    }

    /// Blocks while a guest is up or live. The hub worker's thread only.
    pub fn wait_idle(&self, stopped: impl Fn() -> bool) {
        let mut slots = self.slots();
        while slots.busy() && !stopped() {
            slots = self.changed.wait(slots).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub(crate) fn wake_worker(&self) {
        drop(self.slots());
        self.changed.notify_all();
    }
}

/// What the screen's one redraw is fed now.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Source {
    #[default]
    None,
    /// The screen's guest's call, from its offer on (the redraw switches to
    /// it while it connects).
    Call { call: CallId, guest: GuestId, size: (u32, u32), started: bool },
    /// The box's own camera, at the redraw's size for it.
    Camera { size: (u32, u32) },
}

/// The live thread's side of the screen: ONE redraw at the hub for the
/// party, fed whichever source has the screen (a guest's call or the box
/// camera; the hub takes their H.264 and RGB frames in one session). A
/// takeover switches its source and never opens a second session: two do
/// not fit the GPU (the hub runs out of memory, and the picture waits).
#[cfg(target_os = "linux")]
#[derive(Default)]
struct Bridge {
    redraw: Option<Box<dyn Redraw>>,
    opened: f64,
    /// The redraw gave a frame since it opened: the hub is warm.
    warm: bool,
    /// The size the redraw draws at (a call's follows its keyframes' SPS).
    size: (u32, u32),
    /// The wish count the redraw draws with.
    wish: u64,
    /// The source fed now, and when it took the screen.
    source: Source,
    since: f64,
    /// A call's: when a keyframe was last asked for, and whether one has
    /// gone to the redraw (what follows can be decoded).
    asked: f64,
    keyframe: bool,
    /// The camera's: a frame went to the redraw and has not come back.
    sent: bool,
    /// After the hub refused or failed, the redraw opens again from then
    /// on (the camera shows as it is meanwhile); the refusal, logged once.
    retry_at: f64,
    failed: String,
}

#[cfg(target_os = "linux")]
impl Bridge {
    fn call(&self) -> Option<CallId> {
        match self.source {
            Source::Call { call, .. } => Some(call),
            _ => None,
        }
    }
}

/// A box camera quiet this long is let go (unplugged, or stopped).
const CAMERA_STALL: Duration = Duration::from_secs(3);
/// Seconds before a refused or failed redraw is opened again.
#[cfg(target_os = "linux")]
const REDRAW_RETRY: f64 = 10.0;

#[cfg(target_os = "linux")]
impl Live {
    fn event(&self, call: CallId, event: makepad_webrtc::peer::Event, bridge: &mut Bridge) {
        use makepad_webrtc::peer::Event;
        match event {
            Event::Connected => {
                let connected = self.change(|slots, _| {
                    let replaced = slots.connected(call)?;
                    // Clear the previous frame before the new owner is visible.
                    self.set_latest(None);
                    Ok::<_, ()>(replaced)
                });
                if let Ok(replaced) = connected {
                    if let Some(old) = replaced {
                        self.hang_up(old);
                    }
                    let ms = if bridge.call() == Some(call) { ms_since(self.now(), bridge.since) } else { 0 };
                    eprintln!("[party] live: call {call} is up (ICE, DTLS), {ms} ms after it took the screen");
                }
                if bridge.call() == Some(call) {
                    // The first keyframe at once: SRTP is up.
                    bridge.asked = self.now();
                    if let Some(calls) = self.calls.get() {
                        calls.keyframe(call);
                    }
                }
            }
            Event::Picture(au) => {
                if bridge.call() != Some(call) {
                    return;
                }
                let Some(redraw) = bridge.redraw.as_mut() else { return };
                // The redraw's decoder starts at a keyframe (asked for until one comes).
                if !(bridge.keyframe || au.keyframe) {
                    return;
                }
                if !bridge.keyframe {
                    eprintln!("[party] live: call {call}: the first keyframe, {} ms after it took the screen", ms_since(self.now(), bridge.since));
                }
                bridge.keyframe = true;
                // A phone reports its sensor's size, landscape even when held
                // upright, and turns its pictures when it turns: each
                // keyframe's SPS says their real size, and the redraw follows
                // it, so a picture is never squeezed into another shape.
                if let Some(size) = au.keyframe.then(|| au.picture_size()).flatten().map(|(w, h)| live_size(w, h)) {
                    if size != bridge.size {
                        eprintln!("[party] live: call {call}: the pictures are {}x{}: the redraw follows, from {}x{}", size.0, size.1, bridge.size.0, bridge.size.1);
                        if let Err(e) = redraw.resize(size) {
                            return self.close_redraw(bridge, &e);
                        }
                        bridge.size = size;
                    }
                }
                match redraw.picture(&au.data) {
                    Ok(()) => {
                        self.pictures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(e) => self.close_redraw(bridge, &e),
                }
            }
            Event::Gone(why) => {
                if self.change(|slots, _| slots.gone(call)) {
                    eprintln!("[party] live: call {call} ended ({why})");
                }
            }
        }
    }

    fn prompt(&self, guest: GuestId, seed: u64) -> (u64, String) {
        let slots = self.slots();
        let wish = if slots.wished_by.is_none() {
            &slots.wish
        } else if let Some(slot) = slots.slot(guest) {
            &slot.wish
        } else {
            return (slots.wish.count, crate::worker::plain_prompt(None, seed));
        };
        (wish.count, crate::worker::plain_prompt(wish.text.as_deref(), seed))
    }

    /// What has the screen: the screen's guest's call once offered, else
    /// the box camera while it gives frames and no take runs.
    fn source(&self) -> Source {
        if let Some((call, guest, size, started)) = self.slots().redraw_call() {
            return Source::Call { call, guest, size, started };
        }
        let take = self.take_running.load(std::sync::atomic::Ordering::SeqCst);
        match self.camera.fresh(CAMERA_STALL).filter(|_| !take) {
            Some(size) => Source::Camera { size },
            None => Source::None,
        }
    }

    fn source_prompt(&self, source: Source) -> (u64, String) {
        match source {
            Source::Call { call, guest, .. } => self.prompt(guest, call),
            _ => {
                let (wish, text) = self.wish_text();
                (wish, crate::worker::plain_prompt(text.as_deref(), 0))
            }
        }
    }

    /// Feeds the screen's one redraw from whatever has the screen, opening
    /// it when something does and the hub is free, switching its source on
    /// a takeover, and letting it go when nothing has the screen.
    fn bridge(&self, b: &mut Bridge) {
        let now = self.now();
        let source = self.source();
        if source == Source::None {
            if b.redraw.is_some() || self.picture_up.load(std::sync::atomic::Ordering::SeqCst) {
                self.close_redraw(b, "nothing has the screen");
            }
            b.source = Source::None;
            return;
        }
        let switched = match (b.source, source) {
            (Source::Call { call: was, .. }, Source::Call { call, .. }) => was != call,
            (Source::Camera { .. }, Source::Camera { .. }) => false,
            _ => true,
        };
        if switched {
            let what = match source {
                Source::Call { call, .. } => format!("call {call}"),
                _ => "the box camera".to_string(),
            };
            eprintln!("[party] live: the screen: {what} takes it{}", if b.redraw.is_some() { ", in the same redraw" } else { "" });
            (b.since, b.keyframe, b.asked, b.sent, b.wish) = (now, false, f64::NEG_INFINITY, false, u64::MAX);
            if !self.picture_up.load(std::sync::atomic::Ordering::SeqCst) {
                // A picture the wall may not show (a "Just me" call's) is not
                // kept for the next source.
                self.set_latest(None);
            }
            self.camera.wanted.store(true, std::sync::atomic::Ordering::Release);
            // A call takes the redraw in its offer's shape, as a fresh one
            // would open (its keyframes' size corrects it); feed keeps the
            // camera's.
            if let (Source::Call { call, size, .. }, Some(redraw)) = (source, b.redraw.as_mut()) {
                if size != b.size {
                    match redraw.resize(size) {
                        Ok(()) => {
                            eprintln!("[party] live: call {call}: the redraw takes its offer's {}x{}, from {}x{}", size.0, size.1, b.size.0, b.size.1);
                            b.size = size;
                        }
                        Err(e) => {
                            self.close_redraw(b, &e);
                            b.retry_at = now + REDRAW_RETRY;
                        }
                    }
                }
            }
        }
        b.source = source;
        if b.redraw.is_none() && now >= b.retry_at && !self.take_running.load(std::sync::atomic::Ordering::SeqCst) {
            self.open_redraw(b, source, now);
        }
        let Some(mut redraw) = b.redraw.take() else {
            // No redraw (the hub refused or is warming up a retry): the camera as it is.
            if let Source::Camera { .. } = source {
                if let Some(frame) = lock(&self.camera.frame).take() {
                    self.camera.wanted.store(true, std::sync::atomic::Ordering::Release);
                    self.show_frame(frame, true);
                }
            }
            return;
        };
        match self.feed(&mut *redraw, b, source, now) {
            Ok(()) => b.redraw = Some(redraw),
            Err(e) => {
                b.redraw = Some(redraw);
                self.close_redraw(b, &e);
                b.retry_at = now + REDRAW_RETRY;
            }
        }
    }

    fn open_redraw(&self, b: &mut Bridge, source: Source, now: f64) {
        let Some(redrawer) = self.redrawer.get().filter(|_| source != Source::None) else { return };
        let (wish, prompt) = self.source_prompt(source);
        // Opening counts as open: nothing else opens a session meanwhile.
        self.redrawing.store(true, std::sync::atomic::Ordering::SeqCst);
        let opened = match source {
            Source::Call { call, size, .. } => redrawer.open(size, &prompt, call).map(|r| (r, size)),
            Source::Camera { size } => redrawer.open_raw(size, &prompt, 0).map(|r| (r, size)),
            Source::None => return,
        };
        match opened {
            Ok((redraw, size)) => {
                (b.redraw, b.opened, b.warm, b.size, b.wish, b.sent, b.asked) = (Some(redraw), now, false, size, wish, false, f64::NEG_INFINITY);
                self.redrawing.store(true, std::sync::atomic::Ordering::SeqCst);
                self.camera.wanted.store(true, std::sync::atomic::Ordering::Release);
                eprintln!("[party] live: the screen's redraw opened at {}x{}, {} ms after its source took the screen", size.0, size.1, ms_since(now, b.since));
            }
            Err(e) => {
                self.redrawing.store(false, std::sync::atomic::Ordering::SeqCst);
                b.retry_at = now + REDRAW_RETRY;
                if e != b.failed {
                    eprintln!("[party] live: the hub: {e}: tried again every {REDRAW_RETRY} s; the box camera shows as it is meanwhile");
                    b.failed = e;
                }
            }
        }
    }

    /// One step of the open redraw: the prompt if it changed, the size, the
    /// source's frames (a call's come as events), the redraw that came back.
    fn feed(&self, redraw: &mut dyn Redraw, b: &mut Bridge, source: Source, now: f64) -> Result<(), String> {
        let (wish, prompt) = self.source_prompt(source);
        if wish != b.wish {
            b.wish = wish;
            redraw.prompt(&prompt)?;
        }
        match source {
            Source::Call { call, .. } => {
                if !b.keyframe && now - b.asked >= KEYFRAME_AGAIN {
                    b.asked = now;
                    if let Some(calls) = self.calls.get() {
                        calls.keyframe(call);
                    }
                }
            }
            Source::Camera { size } => {
                if size != b.size {
                    redraw.resize(size)?;
                    b.size = size;
                }
                if !b.sent || !b.warm {
                    if let Some(frame) = lock(&self.camera.frame).take() {
                        redraw.raw(&frame)?;
                        b.sent = true;
                        if !b.warm {
                            // Warming up: the camera shows as it is meanwhile, and
                            // the hub keeps only the newest frame it was sent.
                            self.camera.wanted.store(true, std::sync::atomic::Ordering::Release);
                            self.show_frame(frame, true);
                        }
                    }
                }
            }
            Source::None => {}
        }
        match redraw.frame()? {
            Some(frame) => {
                if !b.warm {
                    b.warm = true;
                    eprintln!("[party] live: the screen's redraw gave its first frame, {} ms after it opened", ms_since(now, b.opened));
                }
                if matches!(source, Source::Camera { .. }) && b.sent {
                    b.sent = false;
                    self.camera.wanted.store(true, std::sync::atomic::Ordering::Release);
                }
                let public = match source {
                    Source::Call { call, .. } => self.slots().current.as_ref().is_some_and(|s| s.call == Some(call) && s.screen),
                    _ => true,
                };
                self.show_frame(frame, public);
                if let Source::Call { call, started: false, .. } = source {
                    if b.keyframe && self.change(|slots, _| slots.started(call)) {
                        eprintln!("[party] live: call {call} is on the big screen, {} ms after it took the screen", ms_since(now, b.since));
                    }
                }
            }
            None if !b.warm && now - b.opened > HUB_START_WITHIN => return Err("the hub did not start in time".into()),
            None => {}
        }
        Ok(())
    }

    /// Puts a frame up for the screens and the guest's own phone; only a
    /// `public` one (the box camera's, or a call's on the screen) may reach
    /// the wall. The flag is lowered before a private frame lands and raised
    /// after a public one, so the wall never pairs it with the wrong frame.
    fn show_frame(&self, frame: Rgb, public: bool) {
        let count = self.frames.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if !public {
            self.picture_up.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        self.set_latest(Some(LiveFrame::new(count, frame)));
        if public {
            self.picture_up.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn close_redraw(&self, b: &mut Bridge, why: &str) {
        let had = b.redraw.take().is_some();
        (b.warm, b.sent) = (false, false);
        self.redrawing.store(false, std::sync::atomic::Ordering::SeqCst);
        self.picture_up.store(false, std::sync::atomic::Ordering::SeqCst);
        if self.slots().shown() != Some(Standing::Live) {
            self.set_latest(None);
        }
        if had {
            eprintln!("[party] live: the screen's redraw closed: {why}");
        }
    }
}

/// Milliseconds from `then` to `now` (seconds of [`Live::now`]).
#[cfg(target_os = "linux")]
fn ms_since(now: f64, then: f64) -> u64 {
    ((now - then).max(0.0) * 1000.0) as u64
}

/// How often the slots are ticked when no call event comes, and how often
/// an open redraw's frames are taken.
#[cfg(target_os = "linux")]
const TICK: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(target_os = "linux")]
const FRAME_POLL: std::time::Duration = std::time::Duration::from_millis(15);

/// Takes calls on `bind` (UDP; the party's own port), answering for
/// `addresses`, and starts the thread that runs the slots and the live
/// redraw beside them. It ends when the party is dropped.
#[cfg(target_os = "linux")]
pub fn serve(party: &Arc<crate::api::Party>, bind: std::net::SocketAddr, addresses: &[std::net::IpAddr]) -> std::io::Result<()> {
    use std::sync::mpsc::RecvTimeoutError;
    let (events, heard) = std::sync::mpsc::channel();
    let calls = makepad_webrtc::calls::Calls::start(bind, addresses, events)?;
    if party.live.calls.set(calls).is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "calls are taken already"));
    }
    let party = Arc::downgrade(party);
    std::thread::Builder::new().name("party-live".into()).spawn(move || {
        let mut bridge = Bridge::default();
        loop {
            let busy = bridge.redraw.is_some() || matches!(bridge.source, Source::Camera { .. });
            let event = heard.recv_timeout(if busy { FRAME_POLL } else { TICK });
            let Some(party) = party.upgrade() else { return };
            if party.stopped() {
                // The app quits: the redraw's session ends now (`stop`), so the
                // next start's fits on the GPU.
                party.live.close_redraw(&mut bridge, "the app quits");
                return;
            }
            match event {
                Ok((call, event)) => party.live.event(call, event, &mut bridge),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            party.live.change(|_, _| ());
            party.live.bridge(&mut bridge);
        }
    })?;
    Ok(())
}

/// The box's hub redraws the live call: its fast model's live session,
/// fed the camera's H.264, computed as the host's settings say when it opens
/// (fast unless the host picked exact).
pub struct HubRedrawer {
    pub hub: crate::hub::BoxHub,
    pub settings: std::sync::Arc<crate::settings::Settings>,
}

/// The live redraw's steps and how much of each frame it redraws. At 0.6
/// the hub runs 2 of the 3 steps (a loop runs 3 of 4): on the party box
/// that is 4.7 frames a second at the live size against 3.7, and the same
/// look (`makepad-party-server --live-steps` measures and shows them).
pub const LIVE_STEPS: u32 = 3;
pub const LIVE_STRENGTH: f32 = 0.6;

struct HubRedraw {
    session: crate::hub::LiveSession,
    size: (u32, u32),
}

impl Redraw for HubRedraw {
    fn picture(&mut self, au: &[u8]) -> Result<(), String> {
        self.session.send_h264(au, self.size)
    }

    fn frame(&mut self) -> Result<Option<Rgb>, String> {
        self.session.try_frame()
    }

    fn prompt(&mut self, prompt: &str) -> Result<(), String> {
        self.session.set_prompt(prompt)
    }

    fn resize(&mut self, size: (u32, u32)) -> Result<(), String> {
        self.session.set_size(size)?;
        self.size = size;
        Ok(())
    }

    fn raw(&mut self, frame: &Rgb) -> Result<(), String> {
        self.session.send(frame)
    }
}

impl Redrawer for HubRedrawer {
    fn open(&self, size: (u32, u32), prompt: &str, seed: u64) -> Result<Box<dyn Redraw>, String> {
        let compute = self.settings.live().compute();
        let session = self.hub.live(crate::hub::FAST_MODEL, crate::hub::LiveInput::H264, size, prompt, LIVE_STEPS, LIVE_STRENGTH, seed, compute)?;
        Ok(Box::new(HubRedraw { session, size }))
    }

    fn open_raw(&self, size: (u32, u32), prompt: &str, seed: u64) -> Result<Box<dyn Redraw>, String> {
        let compute = self.settings.live().compute();
        let session = self.hub.live(crate::hub::FAST_MODEL, crate::hub::LiveInput::Raw, size, prompt, LIVE_STEPS, LIVE_STRENGTH, seed, compute)?;
        Ok(Box::new(HubRedraw { session, size }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn unkind_live_wish_preserves_the_previous_wish_and_count() {
        let live = Live::default();
        let (a, b) = (1, 2);
        assert_eq!(live.wish_text(), (0, None));
        live.change(|slots, now| assert_eq!(slots.queue(a, now), None));
        assert!(matches!(live.slots().standing(a), Standing::Up { .. }), "the first guest is up");
        assert_eq!(live.wish(a, Some("a garden of paper lanterns".into())), Ok(()));
        let before = live.prompt(a, 7);
        assert_eq!(live.wish_text(), (1, Some("a garden of paper lanterns".into())));
        assert_eq!(before.0, 1, "the kind wish is counted");
        assert!(before.1.contains("a garden of paper lanterns"), "the prompt carries the kind wish");
        assert_eq!(live.wish(b, Some("a gun".into())), Err(OfferError::NotUp), "the turn check comes first");
        let refused = Err(OfferError::Offer("the box skips that wish; try another".into()));
        assert_eq!(live.wish(a, Some("a gun".into())), refused);
        assert_eq!(live.prompt(a, 7), before, "a refused wish changes neither the prompt nor its count");

        assert_eq!(live.host_wish(Some("a moonlit garden".into())), Ok(()));
        let host = live.prompt(a, 7);
        assert!(host.0 == 2 && host.1.contains("a moonlit garden"), "the host edits the current redraw");
        assert_eq!(live.host_wish(Some("a gun".into())), refused);
        assert_eq!(live.wish_text(), (2, Some("a moonlit garden".into())), "the screen keeps the accepted host wish");
        assert_eq!(live.prompt(a, 7), host);
        live.change(|slots, now| assert_eq!(slots.queue(b, now), None));
        assert_eq!(live.prompt(b, 7), host, "a new taker inherits the host's prompt");
        assert_eq!(live.wish(b, Some("paper stars".into())), Ok(()));
        assert_eq!(live.wish_text(), (3, Some("paper stars".into())), "the screen sees the newest guest edit");
        assert_eq!(live.prompt(a, 7), host, "the taker's edit leaves the current redraw alone");
        live.change(|slots, _| {
            assert_eq!(slots.call(b, 20, DEFAULT_SIZE), Ok(None));
            assert_eq!(slots.connected(20), Ok(None));
        });
        let taken = live.prompt(b, 7);
        assert!(taken.0 == 3 && taken.1.contains("paper stars"), "the taker's own wish follows their call");
        assert_eq!(live.wish(b, None), Ok(()), "the guest can still let the box pick");
        let cleared = live.prompt(b, 7);
        assert_eq!(live.wish_text(), (4, None));
        assert_eq!(cleared.0, 4, "clearing the wish is counted");
        assert!(cleared.1.starts_with("Redraw this picture as"), "the box picks after the wish is cleared");
    }

    /// A taker's screen choice travels with their slot. A stream waiting
    /// for a frame wakes when one comes.
    #[test]
    fn just_me_keeps_a_call_off_the_screens() {
        let mut slots = Slots::default();
        let (a, b) = (1, 2);
        assert_eq!(slots.queue(a, 0.0), None);
        assert_eq!(slots.queue(b, 0.0), None);
        slots.set_screen(b, false);
        assert!(slots.screen(a) && !slots.screen(b), "on the screen unless asked otherwise");
        assert!(slots.shown().is_some(), "the guest up shows");
        slots.set_screen(a, false);
        assert_eq!(slots.shown(), None, "just me: nothing of the call on the screens");
        assert_eq!(slots.call(b, 7, DEFAULT_SIZE), Ok(None));
        assert_eq!(slots.connected(7), Ok(None));
        assert_eq!((slots.current_guest(), slots.shown()), (Some(b), None), "the taker's choice holds once connected");
        slots.set_screen(b, true);
        assert!(slots.shown().is_some());

        let live = Arc::new(Live::default());
        assert!(live.frame_after(0, Duration::ZERO).is_none(), "no call, no frame");
        let framer = live.clone();
        let sent = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            framer.set_latest(Some(LiveFrame::new(1, Rgb { width: 2, height: 1, data: vec![255, 0, 0, 0, 0, 255] })));
        });
        let frame = live.frame_after(0, Duration::from_secs(5)).expect("woken by the frame");
        assert_eq!((frame.count, frame.bgra.clone()), (1, vec![0xffff_0000, 0xff00_00ff]));
        assert!(frame.jpeg().is_some_and(|j| j.starts_with(&[0xff, 0xd8])), "a JPEG for the screens");
        assert!(live.frame_after(1, Duration::ZERO).is_none(), "a frame already shown is not new");
        sent.join().unwrap();
    }

    #[test]
    fn the_newest_guest_takes_the_screen() {
        const SIZE: (u32, u32) = (512, 288);
        assert_eq!((live_size(1280, 720), live_size(720, 1280), live_size(640, 480), live_size(0, 0)), (SIZE, (288, 512), (512, 384), (512, 384)));
        let (a, b, c) = (1, 2, 3);
        let mut slots = Slots::default();
        assert_eq!(slots.queue(a, 0.0), None);
        assert_eq!(slots.standing(a), Standing::Up { until: CONNECT_WITHIN });
        assert!(slots.busy() && slots.may_call(a));
        assert_eq!(slots.call(b, 7, SIZE), Err(()), "a guest must ask to go live first");
        assert_eq!(slots.call(a, 10, SIZE), Ok(None));
        assert_eq!(slots.call(a, 11, SIZE), Ok(Some(10)), "a second offer replaces the first");
        assert_eq!(slots.connected(10), Err(()), "the superseded offer cannot take the slot");
        assert_eq!(slots.connected(11), Ok(None));
        assert_eq!(slots.standing(a), Standing::Starting);
        for t in [9.0, 18.0, 27.0] {
            slots.seen(a, t);
            assert!(slots.tick(t).is_empty(), "the connected call can wait for the hub");
        }
        assert_eq!(slots.connected_call(), Some((11, a, SIZE, false)));
        assert!(slots.started(11));
        for t in (36..=99).step_by(9) {
            slots.seen(a, f64::from(t));
            assert!(slots.tick(f64::from(t)).is_empty(), "no time limit on the live guest");
        }

        assert_eq!(slots.queue(b, 100.0), None);
        assert_eq!(slots.standing(b), Standing::Up { until: 100.0 + CONNECT_WITHIN });
        assert!(slots.may_call(b) && !slots.may_call(a));
        assert_eq!(slots.call(b, 12, SIZE), Ok(None));
        for t in [108.0, 117.0] {
            slots.seen(a, t);
            assert_eq!(slots.queue(b, t), None, "asking again keeps the same pending call");
            assert!(slots.tick(t).is_empty());
        }
        assert_eq!(slots.tick(100.0 + CONNECT_WITHIN), vec![12], "the original connect deadline still applies");
        assert_eq!(slots.standing(b), Standing::Out);
        assert_eq!(slots.connected(12), Err(()));
        assert!(!slots.bumped(a) && !slots.bumped(b), "timing out displaces nobody");
        assert_eq!(slots.connected_call(), Some((11, a, SIZE, true)));

        assert_eq!(slots.queue(b, 121.0), None);
        assert_eq!(slots.call(b, 13, SIZE), Ok(None));
        slots.seen(a, 126.0);
        assert_eq!(slots.tick(121.5 + SEEN_WITHIN), vec![13], "a quiet taker's call is closed");
        assert!(!slots.bumped(b) && slots.standing(a) == Standing::Live);

        assert_eq!(slots.queue(b, 132.0), None);
        assert_eq!(slots.call(b, 14, SIZE), Ok(None));
        assert_eq!(slots.queue(c, 133.0), Some(14), "the newer taker hangs up the earlier offer");
        assert!(slots.bumped(b) && slots.standing(b) == Standing::Out && slots.may_call(c));
        assert_eq!(slots.connected(14), Err(()), "the replaced taker cannot take over late");
        assert_eq!(slots.shown(), Some(Standing::Live));
        assert_eq!(slots.queue(b, 134.0), None, "b can try again, displacing c even without an offer");
        assert!(!slots.bumped(b) && slots.bumped(c) && slots.may_call(b));
        assert_eq!(slots.call(b, 15, SIZE), Ok(None));
        assert_eq!(slots.connected(15), Ok(Some(11)), "b's connection hangs up a's live call");
        assert!(slots.bumped(a) && slots.standing(a) == Standing::Out);
        assert_eq!(slots.connected_call(), Some((15, b, SIZE, false)));
        assert_eq!(slots.shown(), Some(Standing::Starting));
        assert!(!slots.gone(11) && !slots.started(11), "late events from a leave b alone");
        assert!(slots.started(15));
        assert_eq!(slots.shown(), Some(Standing::Live));
        assert_eq!(slots.leave(c), None);
        assert!(!slots.bumped(c), "leaving clears the bump");

        assert_eq!(slots.queue(c, 135.0), None);
        assert_eq!(slots.call(c, 16, SIZE), Ok(None));
        assert_eq!(slots.leave(b), Some(15), "leaving hangs up the current guest");
        assert_eq!(slots.current_guest(), Some(c));
        assert_eq!(slots.standing(c), Standing::Up { until: 135.0 + CONNECT_WITHIN });
        assert!(slots.busy(), "there is no admission gap for a take");
        assert_eq!(slots.connected(16), Ok(None));
        assert!(!slots.gone(99) && slots.gone(16), "only the slot's own call ends it");
        assert!(!slots.busy());

        assert_eq!(slots.queue(a, 140.0), None);
        assert_eq!(slots.call(a, 17, SIZE), Ok(None));
        assert_eq!(slots.connected(17), Ok(None));
        assert!(slots.started(17));
        assert_eq!(slots.tick(140.5 + SEEN_WITHIN), vec![17], "even a live page must stay present");
        assert!(!slots.bumped(a) && !slots.busy(), "a quiet live guest releases the hub");
    }
}
