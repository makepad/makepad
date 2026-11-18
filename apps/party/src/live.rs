//! Live: one guest at a time sends their phone's camera, as a WebRTC call,
//! to the big screen, redrawn by the box's hub as it comes. The rest wait
//! their turn in a queue their phones show.
//!
//! The rules ([`Slots`], with the time given in seconds):
//! - One live guest at a time. The guest called up has [`CONNECT_WITHIN`]
//!   to get their call up.
//! - Then the hub starts: a take already running finishes first (its GPU
//!   work is never thrown away), then the hub's live session opens. The
//!   phone and the wall show "starting" until its first redrawn frame;
//!   from that frame the slot is [`SLOT`].
//! - With nobody waiting the live guest keeps going. Once someone queues,
//!   the slot ends at its minute, or [`WRAP_UP`] after they queued if the
//!   minute is past.
//! - A guest who leaves the queue, or whose page stops asking for
//!   [`SEEN_WITHIN`] before they are live, frees their place at once.
//! - A slot ends early when the call ends (hung up, lost, consent gone), or
//!   when the hub cannot start within [`HUB_START_WITHIN`] of opening.
//! - While a guest is up or live no new take starts; they resume right
//!   after.
//!
//! [`Live`] runs the rules beside the calls (`makepad_webrtc::calls`, on
//! the party's own port number over UDP; Linux only): one thread takes the
//! calls' events, ticks the slots, hangs up the calls they end, and carries
//! the live call's access units to the hub's session ([`Redrawer`]) from a
//! keyframe on, and its redrawn frames back to [`Live::latest`]. Nothing of
//! a call goes back to the phone but what the protocol needs; the phone
//! learns its place from the page's own requests.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::image::Rgb;
use crate::store::{GuestId, Jpeg};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
pub use makepad_webrtc::calls::CallId;
#[cfg(not(target_os = "linux"))]
pub type CallId = u64;

/// A live guest's time once their call is up.
pub const SLOT: f64 = 60.0;
/// A live guest past their slot when someone queues: this long to wrap up.
pub const WRAP_UP: f64 = 10.0;
/// From being called up to the call being up.
pub const CONNECT_WITHIN: f64 = 20.0;
/// A waiting (or called-up) guest whose page has not asked for this long
/// has gone. The page asks every second or two.
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
pub const KEYFRAME_AGAIN: f64 = 2.0;
/// The live frames' JPEG quality, for the screen page and the guest's phone.
const LIVE_JPEG_QUALITY: u8 = 82;

struct Waiter {
    guest: GuestId,
    queued: f64,
    seen: f64,
    /// The guest's call shows on the party's screens ("On the screen"), or
    /// only on their own phone ("Just me").
    screen: bool,
}

struct Current {
    guest: GuestId,
    up: f64,
    seen: f64,
    call: Option<CallId>,
    /// The redraw's size, from the camera's.
    size: (u32, u32),
    /// When the call came up.
    connected: Option<f64>,
    /// When the hub's first redrawn frame came: the slot's clock.
    live: Option<f64>,
    screen: bool,
}

/// Where a guest stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Standing {
    Out,
    /// `ahead` guests before them (0: next).
    Waiting { ahead: usize },
    /// Called up: their call must be up by `until`.
    Up { until: f64 },
    /// The call is up; the hub is starting (a take finishing, a model loading).
    Starting,
    /// On the big screen; until `until` once someone waits.
    Live { until: Option<f64> },
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
    pub waiting: usize,
    pub now: f64,
}

impl Board {
    /// What the wall writes under the live picture; `None` while the
    /// guest's call is not up yet (the wall stays as it is).
    pub fn caption(&self) -> Option<String> {
        let waiting = match self.waiting {
            0 => String::new(),
            n => format!(" · {n} waiting"),
        };
        match self.standing {
            Standing::Starting => Some(format!("starting…{waiting}")),
            Standing::Live { until: Some(until) } => Some(format!("live · {} s left{waiting}", (until - self.now).ceil().max(0.0) as u64)),
            Standing::Live { until: None } => Some("live".into()),
            _ => None,
        }
    }
}

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

/// The queue and the live slot.
#[derive(Default)]
pub struct Slots {
    waiting: VecDeque<Waiter>,
    current: Option<Current>,
}

impl Slots {
    /// `guest` joins the queue (or stays where they are).
    pub fn queue(&mut self, guest: GuestId, now: f64) {
        if !self.holds(guest) {
            self.waiting.push_back(Waiter { guest, queued: now, seen: now, screen: true });
        }
        self.seen(guest, now);
    }

    /// Whether `guest`'s call shows on the party's screens, while they wait
    /// or are up or live.
    pub fn set_screen(&mut self, guest: GuestId, screen: bool) {
        if let Some(c) = self.current.as_mut().filter(|c| c.guest == guest) {
            c.screen = screen;
        }
        if let Some(w) = self.waiting.iter_mut().find(|w| w.guest == guest) {
            w.screen = screen;
        }
    }

    /// Whether `guest`'s call shows on the party's screens.
    pub fn screen(&self, guest: GuestId) -> bool {
        match self.current.as_ref().filter(|c| c.guest == guest) {
            Some(c) => c.screen,
            None => self.waiting.iter().find(|w| w.guest == guest).is_none_or(|w| w.screen),
        }
    }

    fn holds(&self, guest: GuestId) -> bool {
        self.current.as_ref().is_some_and(|c| c.guest == guest) || self.waiting.iter().any(|w| w.guest == guest)
    }

    /// The guest's page asked: they are still there.
    pub fn seen(&mut self, guest: GuestId, now: f64) {
        if let Some(c) = self.current.as_mut().filter(|c| c.guest == guest) {
            c.seen = now;
        }
        if let Some(w) = self.waiting.iter_mut().find(|w| w.guest == guest) {
            w.seen = now;
        }
    }

    /// `guest` leaves the queue, or their slot: answers the call to hang up.
    pub fn leave(&mut self, guest: GuestId) -> Option<CallId> {
        self.waiting.retain(|w| w.guest != guest);
        match &self.current {
            Some(c) if c.guest == guest => self.current.take().and_then(|c| c.call),
            _ => None,
        }
    }

    pub fn standing(&self, guest: GuestId) -> Standing {
        if let Some(c) = self.current.as_ref().filter(|c| c.guest == guest) {
            return match (c.connected, c.live) {
                (None, _) => Standing::Up { until: c.up + CONNECT_WITHIN },
                (Some(_), None) => Standing::Starting,
                (Some(_), Some(live)) => Standing::Live { until: self.live_until(live) },
            };
        }
        match self.waiting.iter().position(|w| w.guest == guest) {
            Some(ahead) => Standing::Waiting { ahead },
            None => Standing::Out,
        }
    }

    fn live_until(&self, live: f64) -> Option<f64> {
        self.waiting.front().map(|w| (live + SLOT).max(w.queued + WRAP_UP))
    }

    /// Whether `guest` is called up and may start their call.
    pub fn may_call(&self, guest: GuestId) -> bool {
        self.current.as_ref().is_some_and(|c| c.guest == guest && c.connected.is_none())
    }

    /// `guest`'s call started, its camera redrawn at `size`: the call it
    /// replaces (an offer sent again) to hang up, or `Err` when it is not
    /// their turn (any more), and the new call is to be hung up.
    pub fn call(&mut self, guest: GuestId, call: CallId, size: (u32, u32)) -> Result<Option<CallId>, ()> {
        match self.current.as_mut() {
            Some(c) if c.guest == guest && c.connected.is_none() => {
                c.size = size;
                Ok(c.call.replace(call))
            }
            _ => Err(()),
        }
    }

    /// The call came up: the hub starts. False when the call is not the
    /// slot's (one replaced or ended meanwhile).
    pub fn connected(&mut self, call: CallId, now: f64) -> bool {
        match self.current.as_mut() {
            Some(c) if c.call == Some(call) && c.connected.is_none() => {
                c.connected = Some(now);
                true
            }
            _ => false,
        }
    }

    /// The hub's first redrawn frame: the slot's minute starts.
    pub fn started(&mut self, call: CallId, now: f64) -> bool {
        match self.current.as_mut() {
            Some(c) if c.call == Some(call) && c.connected.is_some() && c.live.is_none() => {
                c.live = Some(now);
                true
            }
            _ => false,
        }
    }

    /// The call ended (either end, or lost): its slot with it.
    pub fn gone(&mut self, call: CallId) -> bool {
        if self.current.as_ref().is_some_and(|c| c.call == Some(call)) {
            self.current = None;
            return true;
        }
        false
    }

    /// Ends what is over and calls up the next guest; answers the calls to
    /// hang up.
    pub fn tick(&mut self, now: f64) -> Vec<CallId> {
        let mut hang_up = Vec::new();
        self.waiting.retain(|w| now - w.seen <= SEEN_WITHIN);
        if let Some(c) = &self.current {
            let over = match (c.connected, c.live) {
                (None, _) => now >= c.up + CONNECT_WITHIN || now - c.seen > SEEN_WITHIN,
                // The hub's start has its own deadline (the bridge's).
                (Some(_), None) => now - c.seen > SEEN_WITHIN,
                (Some(_), Some(live)) => self.live_until(live).is_some_and(|until| now >= until),
            };
            if over {
                hang_up.extend(self.current.take().and_then(|c| c.call));
            }
        }
        if self.current.is_none() {
            if let Some(w) = self.waiting.pop_front() {
                self.current = Some(Current { guest: w.guest, up: now, seen: w.seen, call: None, size: DEFAULT_SIZE, connected: None, live: None, screen: w.screen });
            }
        }
        hang_up
    }

    /// The call that is up (starting or live): its guest, its redraw's
    /// size, and whether the hub has started.
    pub fn connected_call(&self) -> Option<(CallId, GuestId, (u32, u32), bool)> {
        let c = self.current.as_ref().filter(|c| c.connected.is_some())?;
        Some((c.call?, c.guest, c.size, c.live.is_some()))
    }

    /// The live slot as the wall shows it, if one is under way.
    /// The live slot's standing when its guest's call shows on the screens.
    pub fn shown(&self) -> Option<Standing> {
        self.current.as_ref().filter(|c| c.screen).map(|c| self.standing(c.guest))
    }

    /// The guest who has the slot.
    pub fn current_guest(&self) -> Option<GuestId> {
        self.current.as_ref().map(|c| c.guest)
    }

    /// A guest is up or live: the takes wait.
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
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
}

/// Opens live redraws: the box's hub, or a stand-in in tests.
pub trait Redrawer: Send + Sync {
    fn open(&self, size: (u32, u32), prompt: &str, seed: u64) -> Result<Box<dyn Redraw>, String>;
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
    /// The live guest's wish, and a count of the wishes set.
    wish: Mutex<(u64, Option<(GuestId, String)>)>,
    /// The latest redrawn frame of the live call, for the wall; `framed`
    /// is signalled when it changes.
    latest: Mutex<Option<Arc<LiveFrame>>>,
    framed: Condvar,
    /// Redrawn frames so far (the wall redraws when it moves).
    pub frames: AtomicU64,
    /// Access units of the live call sent to the redraw.
    pub pictures: AtomicU64,
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
            wish: Mutex::new((0, None)),
            latest: Mutex::new(None),
            framed: Condvar::new(),
            frames: AtomicU64::new(0),
            pictures: AtomicU64::new(0),
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

    #[cfg(any(target_os = "linux", test))]
    fn set_latest(&self, frame: Option<LiveFrame>) {
        *lock(&self.latest) = frame.map(Arc::new);
        self.framed.notify_all();
    }

    /// The live slot as the wall shows it, if a guest has it.
    pub fn board(&self) -> Option<Board> {
        let now = self.now();
        let slots = self.slots();
        Some(Board { standing: slots.shown()?, waiting: slots.waiting(), now })
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

    /// Changes the slots and ticks them; hangs up what that ends.
    pub fn change<R>(&self, f: impl FnOnce(&mut Slots, f64) -> R) -> R {
        let now = self.now();
        let (result, hang_up, flipped) = {
            let mut slots = self.slots();
            let before = slots.busy();
            let result = f(&mut slots, now);
            let hang_up = slots.tick(now);
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

    /// `guest` leaves the queue, or their slot (hanging up their call).
    pub fn leave(&self, guest: GuestId) {
        if let Some(call) = self.change(|slots, _| slots.leave(guest)) {
            self.hang_up(call);
        }
    }

    fn hang_up(&self, _call: CallId) {
        #[cfg(target_os = "linux")]
        if let Some(calls) = self.calls.get() {
            calls.close(_call);
        }
    }

    /// The live guest's wish for their redraw (`None`: the box picks).
    pub fn wish(&self, guest: GuestId, wish: Option<String>) -> Result<(), OfferError> {
        let ours = self.slots().current.as_ref().is_some_and(|c| c.guest == guest);
        if !ours {
            return Err(OfferError::NotUp);
        }
        if let Some(wish) = &wish {
            if !crate::worker::kind_wish(wish) {
                return Err(OfferError::Offer("the box skips that wish; try another".into()));
            }
        }
        let mut current = lock(&self.wish);
        current.0 += 1;
        current.1 = wish.map(|wish| (guest, wish));
        Ok(())
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
            let (call, answer) = calls.offer(sdp).map_err(|e| OfferError::Offer(format!("{e:?}")))?;
            match self.change(|slots, _| slots.call(guest, call, live_size(camera.0, camera.1))) {
                Ok(replaced) => {
                    replaced.into_iter().for_each(|old| calls.close(old));
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
    pub fn wait_idle(&self) {
        let mut slots = self.slots();
        while slots.busy() {
            slots = self.changed.wait(slots).unwrap_or_else(|p| p.into_inner());
        }
    }
}

/// The live thread's side of the live call: its redraw, once open.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct Bridge {
    call: Option<CallId>,
    redraw: Option<Box<dyn Redraw>>,
    opened: f64,
    /// When a keyframe was last asked for.
    asked: f64,
    /// A keyframe has gone to the redraw: what follows can be decoded.
    keyframe: bool,
    wish: u64,
}

#[cfg(target_os = "linux")]
impl Live {
    fn event(&self, call: CallId, event: makepad_webrtc::peer::Event, bridge: &mut Bridge) {
        use makepad_webrtc::peer::Event;
        match event {
            Event::Connected => {
                if self.change(|slots, now| slots.connected(call, now)) {
                    eprintln!("[party] live: call {call} is up; the hub starts");
                }
            }
            Event::Picture(au) => {
                if bridge.call != Some(call) {
                    return;
                }
                let Some(redraw) = bridge.redraw.as_mut() else { return };
                // The redraw's decoder starts at a keyframe (asked for when it opened).
                if !(bridge.keyframe || au.keyframe) {
                    return;
                }
                bridge.keyframe = true;
                match redraw.picture(&au.data) {
                    Ok(()) => {
                        self.pictures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(e) => self.end(call, &e),
                }
            }
            Event::Gone(why) => {
                if self.change(|slots, _| slots.gone(call)) {
                    eprintln!("[party] live: call {call} ended ({why})");
                }
            }
        }
    }

    /// Ends the slot of `call` (and hangs it up), for `why`.
    fn end(&self, call: CallId, why: &str) {
        if self.change(|slots, _| slots.gone(call)) {
            self.hang_up(call);
            eprintln!("[party] live: call {call} ended: {why}");
        }
    }

    fn prompt(&self, guest: GuestId, seed: u64) -> (u64, String) {
        let wish = lock(&self.wish);
        let theirs = wish.1.as_ref().filter(|(g, _)| *g == guest).map(|(_, w)| w.as_str());
        (wish.0, crate::worker::plain_prompt(theirs, seed))
    }

    /// Opens the live call's redraw once the hub is free, keeps its prompt
    /// and takes its frames; lets it go when the slot ends.
    fn bridge(&self, bridge: &mut Bridge) {
        let now = self.now();
        let connected = self.slots().connected_call();
        let Some((call, guest, size, started)) = connected else {
            if bridge.call.take().is_some() {
                *bridge = Bridge::default();
                self.set_latest(None);
            }
            return;
        };
        if bridge.call != Some(call) {
            *bridge = Bridge { call: Some(call), ..Bridge::default() };
            self.set_latest(None);
        }
        let Some(redraw) = bridge.redraw.as_mut() else {
            // A take under way finishes first.
            if self.take_running.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let Some(redrawer) = self.redrawer.get() else { return };
            let (wish, prompt) = self.prompt(guest, call);
            match redrawer.open(size, &prompt, call) {
                Ok(redraw) => {
                    bridge.redraw = Some(redraw);
                    (bridge.opened, bridge.asked, bridge.wish) = (now, now, wish);
                    if let Some(calls) = self.calls.get() {
                        calls.keyframe(call);
                    }
                    eprintln!("[party] live: call {call}: the redraw opened at {}x{}", size.0, size.1);
                }
                Err(e) => self.end(call, &format!("the hub: {e}")),
            }
            return;
        };
        if !started && now - bridge.asked >= KEYFRAME_AGAIN {
            bridge.asked = now;
            if let Some(calls) = self.calls.get() {
                calls.keyframe(call);
            }
        }
        let (wish, prompt) = self.prompt(guest, call);
        if wish != bridge.wish {
            bridge.wish = wish;
            if let Err(e) = redraw.prompt(&prompt) {
                return self.end(call, &e);
            }
        }
        match redraw.frame() {
            Ok(Some(frame)) => {
                let count = self.frames.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                self.set_latest(Some(LiveFrame::new(count, frame)));
                if !started && self.change(|slots, now| slots.started(call, now)) {
                    eprintln!("[party] live: call {call} is on the big screen");
                }
            }
            Ok(None) if !started && now - bridge.opened > HUB_START_WITHIN => self.end(call, "the hub did not start in time"),
            Ok(None) => {}
            Err(e) => self.end(call, &e),
        }
    }
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
            let event = heard.recv_timeout(if bridge.redraw.is_some() { FRAME_POLL } else { TICK });
            let Some(party) = party.upgrade() else { return };
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
}

impl Redrawer for HubRedrawer {
    fn open(&self, size: (u32, u32), prompt: &str, seed: u64) -> Result<Box<dyn Redraw>, String> {
        let compute = self.settings.live().compute();
        let session = self.hub.live(crate::hub::FAST_MODEL, crate::hub::LiveInput::H264, size, prompt, LIVE_STEPS, LIVE_STRENGTH, seed, compute)?;
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
        live.change(|slots, now| {
            slots.queue(a, now);
            slots.queue(b, now);
        });
        assert!(matches!(live.slots().standing(a), Standing::Up { .. }), "the first guest is up");
        assert_eq!(live.wish(a, Some("a garden of paper lanterns".into())), Ok(()));
        let before = live.prompt(a, 7);
        assert_eq!(before.0, 1, "the kind wish is counted");
        assert!(before.1.contains("a garden of paper lanterns"), "the prompt carries the kind wish");
        assert_eq!(live.wish(b, Some("a gun".into())), Err(OfferError::NotUp), "the turn check comes first");
        assert_eq!(live.wish(a, Some("a gun".into())), Err(OfferError::Offer("the box skips that wish; try another".into())));
        assert_eq!(live.prompt(a, 7), before, "a refused wish changes neither the prompt nor its count");
        assert_eq!(live.wish(a, None), Ok(()), "the guest can still let the box pick");
        let cleared = live.prompt(a, 7);
        assert_eq!(cleared.0, 2, "clearing the wish is counted");
        assert!(cleared.1.starts_with("Redraw this picture as"), "the box picks after the wish is cleared");
    }

    /// The rules, minute by minute: the queue in order, the hub's start
    /// outside the slot, a slot's minute, keeping on while nobody waits, the
    /// wrap-up once someone does, and places freed at once by leaving, by a
    /// quiet page, or by the call.
    /// A guest's call shows on the screens unless they choose "Just me",
    /// which they may change while they wait or are up; a stream waiting for
    /// a frame wakes when one comes.
    #[test]
    fn just_me_keeps_a_call_off_the_screens() {
        let mut slots = Slots::default();
        let (a, b) = (1, 2);
        slots.queue(a, 0.0);
        slots.queue(b, 0.0);
        slots.set_screen(b, false);
        slots.tick(0.0);
        assert!(slots.screen(a) && !slots.screen(b), "on the screen unless asked otherwise");
        assert!(slots.shown().is_some(), "the guest up shows");
        slots.set_screen(a, false);
        assert_eq!(slots.shown(), None, "just me: nothing of the call on the screens");
        slots.leave(a);
        slots.tick(1.0);
        assert_eq!((slots.current_guest(), slots.shown()), (Some(b), None), "a choice made while waiting holds once up");
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
    fn one_live_guest_at_a_time() {
        const SIZE: (u32, u32) = (512, 288);
        assert_eq!((live_size(1280, 720), live_size(720, 1280), live_size(640, 480), live_size(0, 0)), (SIZE, (288, 512), (512, 384), (512, 384)));
        let (a, b, c) = (1, 2, 3);
        let board = |slots: &Slots, now: f64| Board { standing: slots.shown().unwrap(), waiting: slots.waiting(), now }.caption();
        let mut slots = Slots::default();
        slots.queue(a, 0.0);
        slots.queue(b, 0.0);
        assert!(slots.tick(0.0).is_empty());
        assert_eq!(slots.standing(a), Standing::Up { until: CONNECT_WITHIN });
        assert_eq!(board(&slots, 0.0), None, "the wall stays as it is until the call is up");
        assert_eq!(slots.standing(b), Standing::Waiting { ahead: 0 });
        assert!(slots.busy() && !slots.may_call(b) && slots.may_call(a));
        assert_eq!(slots.call(b, 7, SIZE), Err(()), "only the guest called up may call");
        assert_eq!(slots.call(a, 10, SIZE), Ok(None));
        assert_eq!(slots.call(a, 11, SIZE), Ok(Some(10)), "an offer sent again replaces the first call");
        assert!(!slots.connected(10, 1.0), "the replaced call is not the slot's");
        assert!(slots.connected(11, 1.5));
        assert_eq!(slots.standing(a), Standing::Starting, "the call is up; the hub starts");
        assert_eq!(board(&slots, 2.0).as_deref(), Some("starting… · 1 waiting"));
        for t in [10.0, 20.0, 30.0] {
            slots.seen(a, t);
            slots.seen(b, t);
            assert!(slots.tick(t + 0.5).is_empty(), "a take finishing first can outlast the connect window ({t})");
        }
        assert_eq!(slots.connected_call(), Some((11, a, SIZE, false)));
        assert!(slots.started(11, 32.0));
        assert_eq!(slots.standing(a), Standing::Live { until: Some(32.0 + SLOT) }, "b waits: a has a minute from the first frame");
        assert_eq!(board(&slots, 50.0).as_deref(), Some("live · 42 s left · 1 waiting"));
        assert_eq!(slots.connected_call(), Some((11, a, SIZE, true)));

        // b leaves: a keeps going past the minute; c queues late and a wraps up.
        assert_eq!(slots.leave(b), None);
        assert_eq!(slots.standing(a), Standing::Live { until: None }, "nobody waits: no end");
        assert_eq!(board(&slots, 40.0).as_deref(), Some("live"));
        for t in [40.0, 93.0, 99.0] {
            assert!(slots.tick(t).is_empty(), "still live at {t}, the page asking or not");
        }
        slots.queue(c, 100.0);
        assert_eq!(slots.standing(a), Standing::Live { until: Some(100.0 + WRAP_UP) });
        slots.seen(c, 105.0);
        assert!(slots.tick(105.0).is_empty());
        assert_eq!(slots.tick(110.0), vec![11], "the wrap-up is over: a's call is hung up");
        assert_eq!(slots.standing(a), Standing::Out);
        assert_eq!(slots.standing(c), Standing::Up { until: 110.0 + CONNECT_WITHIN }, "and c is up at once");

        // c never connects: their turn passes; a page gone quiet loses its place.
        slots.queue(a, 111.0);
        slots.queue(b, 111.0);
        assert_eq!(slots.call(c, 12, SIZE), Ok(None));
        slots.seen(c, 120.0);
        slots.seen(a, 120.0);
        slots.seen(b, 120.0);
        assert_eq!(slots.tick(110.0 + CONNECT_WITHIN), vec![12], "not connected in time");
        assert_eq!(slots.standing(a), Standing::Up { until: 130.0 + CONNECT_WITHIN });
        assert_eq!(slots.tick(130.0 + SEEN_WITHIN + 0.5), Vec::<CallId>::new(), "a called-up page gone quiet");
        assert_eq!(slots.standing(a), Standing::Out, "loses its turn");
        assert_eq!(slots.standing(b), Standing::Out, "and so does a waiting one");
        assert!(!slots.busy(), "nobody up: the takes resume");

        // The call ends early: the slot with it.
        slots.queue(b, 200.0);
        slots.tick(200.0);
        slots.call(b, 13, SIZE).unwrap();
        slots.connected(13, 201.0);
        slots.started(13, 202.0);
        assert!(!slots.gone(99) && slots.gone(13), "only the slot's own call ends it");
        assert!(!slots.busy());
        slots.queue(a, 210.0);
        slots.tick(210.0);
        slots.call(a, 14, SIZE).unwrap();
        assert_eq!(slots.leave(a), Some(14), "leaving hangs up the call");
    }
}
