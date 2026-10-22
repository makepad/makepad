//! Sessions and their object tables.
//!
//! A session is one `hal_open`. Every table entry is keyed by (session, index) and carries the
//! generation Cx chose; an id whose generation does not match is *stale*: counted in
//! [`Stats::stale_ids`] and ignored, never a panic. Session 0 is the host. Objects it owns that
//! may be handed to an app (the adoptable window) are *lent* by an attach command
//! (`WindowCreate{adopt}`): the host entry records `lent_to`, the app's table gets an alias entry
//! (`owner_session` 0). Closing the session, or the window, returns a lent object to the host and
//! never frees it; everything the session owns itself is freed.
//!
//! Sessions form a tree (domains, HAL.md 5c / 5d): a session with a parent has *embedded* windows,
//! each a render target in its own table that is lent to the parent as an alias texture (a
//! [`LENT_BASE`] index). Closing, killing or relaunching a session frees exactly its entries, drops
//! the aliases (a parent packet naming one is skipped and counted) and tells the parent.

use crate::buffers::{pack_strs, unpack_forward, DomainInfo, EventQueue};
use crate::codec::ArenaSink;
use crate::ledger::{Kind, Ledger, LedgerEntry};
use crate::types::*;
use crate::FastMap;
use std::collections::BTreeMap;

/// Session 0: the host.
pub const HOST: u32 = 0;

/// A table slot.
#[derive(Debug)]
pub struct Entry<T> {
    pub gen: u32,
    /// the session that created and frees it (0 = the host)
    pub owner_session: u32,
    /// the session an object of the host is lent to (0 = not lent)
    pub lent_to: u32,
    pub val: T,
}

/// Objects of one kind, keyed by (session, index).
#[derive(Debug)]
pub struct Table<T> {
    map: FastMap<(u32, u32), Entry<T>>,
    /// last destroyed generation per slot: a create or use with an older one is stale
    retired: FastMap<(u32, u32), u32>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Table { map: FastMap::default(), retired: FastMap::default() }
    }
}

/// Why an operation on an id did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// no live object under the id's slot, or an older generation
    Stale,
}

impl<T> Table<T> {
    pub(crate) fn can_insert(&self, session: u32, id: Id) -> bool {
        let key = (session, id.index);
        !self.retired.get(&key).is_some_and(|g| id.gen <= *g)
            && !self.map.get(&key).is_some_and(|e| id.gen < e.gen)
    }

    /// Inserts under `id`. A live older generation in the slot is replaced (returned); a create
    /// with a generation not newer than the slot's last is stale.
    pub fn insert(&mut self, session: u32, id: Id, owner_session: u32, val: T) -> Result<Option<T>, Miss> {
        let key = (session, id.index);
        if !self.can_insert(session, id) {
            return Err(Miss::Stale);
        }
        let old = self.map.insert(key, Entry { gen: id.gen, owner_session, lent_to: 0, val });
        Ok(old.map(|e| e.val))
    }

    pub fn get(&self, session: u32, id: Id) -> Result<&Entry<T>, Miss> {
        match self.map.get(&(session, id.index)) {
            Some(e) if e.gen == id.gen => Ok(e),
            _ => Err(Miss::Stale),
        }
    }

    pub fn get_mut(&mut self, session: u32, id: Id) -> Result<&mut Entry<T>, Miss> {
        match self.map.get_mut(&(session, id.index)) {
            Some(e) if e.gen == id.gen => Ok(e),
            _ => Err(Miss::Stale),
        }
    }

    pub fn remove(&mut self, session: u32, id: Id) -> Result<Entry<T>, Miss> {
        self.get(session, id)?;
        self.retired.insert((session, id.index), id.gen);
        Ok(self.map.remove(&(session, id.index)).expect("checked live"))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// (index, generation) of the session's entries.
    /// Whether any live entry of the session matches.
    pub fn any_of(&self, session: u32, f: impl Fn(&T) -> bool) -> bool {
        self.map.iter().any(|((s, _), e)| *s == session && f(&e.val))
    }

    pub fn keys_of(&self, session: u32) -> Vec<Id> {
        let mut v: Vec<Id> = self.map.iter().filter(|((s, _), _)| *s == session).map(|((_, i), e)| Id::new(*i, e.gen)).collect();
        v.sort_by_key(|i| i.index);
        v
    }

    /// Frees everything of the session; returns the entries (aliases included) for the caller to
    /// give lent objects back.
    pub fn take_session(&mut self, session: u32) -> Vec<(Id, Entry<T>)> {
        let ids = self.keys_of(session);
        self.retired.retain(|(s, _), _| *s != session);
        ids.into_iter().filter_map(|id| self.map.remove(&(session, id.index)).map(|e| (id, e))).collect()
    }
}

// ------------------------------------------------------------------ objects

#[derive(Clone, Debug)]
pub struct TextureObj {
    pub format: u32,
    pub w: u32,
    pub h: u32,
    pub depth: u32,
    pub mips: u32,
    pub usage: u16,
    pub bytes: u64,
    pub imported: bool,
    /// texture updates received (bytes), headless keeps no pixels
    pub uploaded: u64,
    /// set on the parent's alias of a child's embedded window
    pub lent: Option<LentRef>,
}

/// What an alias texture of the parent stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LentRef {
    pub child: u32,
    /// the render target in the child's table
    pub tex: Id,
    /// the child's window it is the surface of
    pub window: Id,
}

/// The render target behind an embedded window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Embedded {
    /// in the child's texture table
    pub tex: Id,
    /// in the parent's texture table
    pub alias: Id,
    /// bumped by every resize (the generation of both ids)
    pub version: u32,
    pub dpi: f32,
    /// frames the child rendered into it, and the id of the last
    pub frames: u64,
    pub last_frame: u64,
}

#[derive(Clone, Debug)]
pub struct BufferObj {
    pub kind: u16,
    pub size: u64,
    /// what the device holds for it (the ledger's number)
    pub bytes: u64,
    pub evictable: bool,
    pub uploaded: u64,
}

#[derive(Clone, Debug)]
pub struct PipelineObj {
    pub dialect: u32,
    pub desc: PipelineDesc,
    /// The `PipelineCreate` blob as received (header, code, bindings, vertex layout): frame
    /// capture replays exactly this code.
    pub blob: std::sync::Arc<[u8]>,
}

/// An opaque host panel. Its pixels and the application's pixels have separate
/// lifetimes; an idle or parked application need not redraw to show or dismiss it.
#[derive(Clone, Debug)]
pub struct WindowOverlay {
    pub rect: [f32; 4],
    pub frame: Option<u64>,
    /// the window layer's texture (NONE: an opaque panel)
    pub layer: Id,
}

#[derive(Clone, Debug, Default)]
pub struct WindowObj {
    pub kind: u32,
    pub parent: Id,
    pub geom: [f32; 4],
    /// The host's scale for HOSTED windows; other windows use their OS scale.
    pub hosted_dpi: f32,
    /// Last window-system scale; session defaults cannot describe windows on different displays.
    pub dpi: Option<f32>,
    pub state: u32,
    pub title: StrId,
    pub own_chrome: bool,
    /// a host window an app may take over (session 0 only)
    pub adoptable: bool,
    /// An alias entry of an app: the complete host window id it stands for.
    pub alias_of: Option<Id>,
    /// a window of a child domain: no OS window, a render target lent to the parent
    pub embedded: Option<Embedded>,
    /// Completed presentations of this physical window, across loans and relaunches.
    pub presentations: u64,
    pub presenter: u32,
    /// Lending transfers drawing when the borrower submits its first complete frame,
    /// and input when that frame reaches the window.
    pub borrower_frame: bool,
    pub borrower_presented: bool,
    pub overlay: Option<WindowOverlay>,
    /// Last submitted base layer (session, window alias, frame), independent of
    /// presentation counters and the overlay's own frame serial.
    pub base_frame: Option<(u32, Id, u64)>,
}

impl WindowObj {
    pub fn is_hosted(&self) -> bool {
        self.kind & 0xff == window_kind::HOSTED
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimerObj {
    pub interval_ns: u64,
    pub repeat: bool,
    pub due_ns: u64,
}

/// Counters of things the HAL refused to act on (each is visible, none panics).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// an id whose slot is empty, freed or of another generation
    pub stale_ids: u64,
    /// arguments out of range (zero size, rect outside the texture, unknown format, ..)
    pub bad_args: u64,
    pub unknown_cmds: u64,
    /// a CmdBuf whose slices could not be read
    pub bad_bufs: u64,
    /// draws skipped because their pipeline was not live
    pub skipped_draws: u64,
    /// passes larger than an attachment, drawn into the attachments' common area
    /// (`exec::clamped_pass`; a pass is sized as its target, so any is a caller's bug)
    pub clamped_passes: u64,
    pub frames: u64,
    /// a control command from a session that may not use it, or about a session that is not its
    /// child (rejected)
    pub denied: u64,
    /// keyboard / IME / cursor / clipboard requests of an embedded session while it had the focus
    pub input_applied: u64,
    /// the same while another child (or none) had it: counted and dropped
    pub input_dropped: u64,
    /// events a parent forwarded into this session
    pub forwarded: u64,
    /// draws skipped because they sampled an alias whose child window is gone
    pub stale_lent_draws: u64,
    /// frames answered with a GPU time, and the sum / longest of them (ns)
    pub gpu_frames: u64,
    pub gpu_ns_total: u64,
    pub gpu_ns_max: u64,
    pub passes: u64,
    pub draws: u64,
    /// the longest CPU time Cx spent building one frame (`FrameMeta` build start to end), ns
    pub max_frame_ns: u64,
    /// upload arenas (uniforms, inline instances, texture upload staging) the backend had to
    /// allocate for this session's frames and uploads; steady frames reuse theirs and add none
    pub arena_allocs: u64,
    /// media commands a backend without media received
    pub media_unsupported: u64,
    /// WindowOverlay requests a backend could not draw
    pub overlay_unsupported: u64,
}

/// The display-rate tick embedded windows are paced at (60 Hz): they have no display link of their
/// own, their pixels reach the screen through the parent's frames.
pub const EMBEDDED_FRAME_NS: u64 = 16_666_667;

pub struct Session {
    pub id: u32,
    pub app_key: u64,
    /// the dpi factor the backend reported for this session's windows
    pub dpi: f32,
    pub strings: FastMap<StrId, String>,
    pub textures: Table<TextureObj>,
    pub buffers: Table<BufferObj>,
    pub pipelines: Table<PipelineObj>,
    pub windows: Table<WindowObj>,
    pub timers: FastMap<u32, TimerObj>,
    pub ledger: Ledger,
    pub events: EventQueue,
    pub stats: Stats,
    /// the frame id of the newest frame submitted / finished
    pub frame: u64,
    pub last_frame_done: u64,
    /// embedded windows (lent to the parent, no OS window: no display link of their own) that
    /// asked for a frame, and when the next one is due: `Hal::pump` paces them at display rate
    pub embedded_frames: Vec<Id>,
    pub embedded_due_ns: u64,
    pub(crate) font_frames: std::sync::Arc<crate::font::FrameProgress>,
    pub quit: bool,
    /// pipelines a backend is still compiling (the app paints again when they are ready)
    pub pipelines_pending: u32,
    /// the targets of passes already reported larger than an attachment (logged once each)
    pub clamped_pass_targets: Vec<Id>,
    pub clipboard: String,
    /// (req, bytes) of `QueryReply` commands, taken by automation
    pub replies: Vec<(u32, Vec<u8>)>,
    /// (ns, text) of `TraceMark` commands
    pub marks: Vec<(u64, String)>,
    pub budgets: Vec<(u32, u64)>,
    /// the domain that started this session (0 = none)
    pub parent: u32,
    /// may use the domain control commands (set by the host at creation, never by a command)
    pub privileged: bool,
    /// holds the keyboard / IME / cursor / clipboard of its parent's window
    pub focused: bool,
    /// the app has opened it (a domain session exists from its `DomainStart` on)
    pub attached: bool,
    /// closing: no further commands, the close is queued
    pub closing: bool,
    /// the GPU device the session's objects live on (`DeviceLost` / `DeviceReset` name it)
    pub device_generation: u32,
}

impl Session {
    fn new(id: u32, app_key: u64) -> Session {
        Session {
            id,
            app_key,
            parent: 0,
            privileged: false,
            focused: false,
            dpi: 1.0,
            attached: true,
            closing: false,
            strings: FastMap::default(),
            textures: Table::default(),
            buffers: Table::default(),
            pipelines: Table::default(),
            windows: Table::default(),
            timers: FastMap::default(),
            ledger: Ledger::default(),
            events: EventQueue::new(),
            stats: Stats::default(),
            frame: 0,
            last_frame_done: 0,
            embedded_frames: Vec::new(),
            embedded_due_ns: 0,
            font_frames: Default::default(),
            quit: false,
            pipelines_pending: 0,
            clamped_pass_targets: Vec::new(),
            clipboard: String::new(),
            replies: Vec::new(),
            marks: Vec::new(),
            budgets: Vec::new(),
            device_generation: 1,
        }
    }

    pub fn string(&self, id: StrId) -> Option<&str> {
        (id != 0).then(|| self.strings.get(&id).map(|s| s.as_str())).flatten()
    }

    /// Counts a stale id.
    pub fn stale(&mut self) {
        self.stats.stale_ids += 1;
    }

    pub fn bad(&mut self) {
        self.stats.bad_args += 1;
    }
}

/// The clock: real raw-monotonic time, or a manual time automation sets.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    pub manual: Option<u64>,
}

impl Clock {
    pub fn now(&self) -> u64 {
        self.manual.unwrap_or_else(clock_ns)
    }
}

/// All sessions.
pub struct World {
    sessions: FastMap<u32, Session>,
    next_session: u32,
    next_host_window: u32,
    pub clock: Clock,
    /// the domains (sessions started by `DomainStart`), kept after they exit
    domains: BTreeMap<u32, DomainRec>,
    /// sessions the HAL ended (killed, crashed, parent closed, relaunched app key reused) whose
    /// app has not been told yet: its next pump delivers one quit request
    ended: Vec<u32>,
    next_lent: u32,
    /// the font service (made at its first command), and the call its workers wake the pumps with
    pub fonts: Option<Box<crate::font::FontService>>,
    pub font_notify: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    next_key: u64,
    /// sessions whose queue got an event from another session's command: the caller wakes them
    notified: Vec<u32>,
    /// who hears of a session's window frames besides itself (its parent for an embedded window,
    /// the host for a window lent by the host): a completion thread wakes them with the owner,
    /// so the news is drained at once and not at the owner's next pump
    pub listeners: std::sync::Arc<std::sync::Mutex<FastMap<u32, Vec<u32>>>>,
    /// sessions to close, children before their parent
    close_queue: Vec<(u32, CloseAs)>,
    /// calls to the registered `DomainHost`, made after the command batch (no lock held)
    host_calls: Vec<HostCall>,
    /// record every executed frame in [`World::exec_log`] (tests assert the order from it)
    pub trace_exec: bool,
    pub exec_log: Vec<ExecRecord>,
    /// the current GPU device, from 1; a device reset makes the next
    pub device_generation: u32,
}

/// How a session ends.
#[derive(Clone, Debug)]
pub enum CloseAs {
    /// the app ended its own session with `Quit`: it knows
    Quit,
    Exit(String),
    Crash(String),
    /// frees it and makes it again under the same id for the host to fill
    Relaunch,
}

/// A domain: what `DomainStart` asked for and where it stands.
#[derive(Clone, Debug)]
pub struct DomainRec {
    pub parent: u32,
    pub req: u32,
    pub app: String,
    pub args: String,
    /// the key the app opens its session with (the proof that the parent asked for it)
    pub app_key: u64,
    pub state: u32,
    pub phase: u32,
    pub pct: f32,
    pub msg: String,
}

/// What the registered host is asked to do for a domain.
#[derive(Clone, Debug)]
pub struct DomainLaunch {
    pub domain: u32,
    pub parent: u32,
    pub req: u32,
    pub app: String,
    /// NUL-terminated argv entries (empty string means no arguments).
    pub args: String,
    /// the app's `hal_open` must carry `parent` and this key
    pub app_key: u64,
}

#[derive(Clone, Debug)]
pub enum HostCall {
    Start(DomainLaunch),
    Relaunch(DomainLaunch),
    Stop { domain: u32, mode: u32 },
}

/// One executed frame (tracing on): which dependencies on other sessions' surfaces it had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRecord {
    pub seq: u64,
    pub session: u32,
    pub frame: u64,
    pub target: Id,
    /// (child session, child window, the child frame the sampled surface held)
    pub deps: Vec<(u32, Id, u64)>,
    /// Window composition in back-to-front order. Empty for texture/pick frames.
    pub layers: Vec<(u32, Id, u64)>,
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    pub fn new() -> World {
        let mut sessions = FastMap::default();
        sessions.insert(HOST, Session::new(HOST, 0));
        World {
            sessions,
            next_session: 1,
            next_host_window: 0,
            clock: Clock::default(),
            domains: BTreeMap::new(),
            ended: Vec::new(),
            next_lent: 0,
            fonts: None,
            font_notify: None,
            next_key: 1,
            notified: Vec::new(),
            listeners: Default::default(),
            close_queue: Vec::new(),
            host_calls: Vec::new(),
            trace_exec: false,
            exec_log: Vec::new(),
            device_generation: 1,
        }
    }

    pub fn now(&self) -> u64 {
        self.clock.now()
    }

    /// The font service, made on first use.
    pub fn font_service(&mut self) -> &mut crate::font::FontService {
        let notify = self.font_notify.clone();
        self.fonts.get_or_insert_with(|| Box::new(crate::font::FontService::new(notify)))
    }

    /// A fresh id of the HAL's own (lent) range: what a session samples a host-owned texture by.
    pub fn alloc_lent(&mut self) -> Id {
        let id = Id::new(LENT_BASE + self.next_lent, 1);
        self.next_lent += 1;
        id
    }

    pub fn session(&self, id: u32) -> Option<&Session> {
        self.sessions.get(&id)
    }

    pub fn session_mut(&mut self, id: u32) -> Option<&mut Session> {
        self.sessions.get_mut(&id)
    }

    pub fn session_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.sessions.keys().copied().collect();
        v.sort();
        v
    }

    /// The open app session with this non-zero key, if any (a relaunch closes it first).
    pub fn session_with_key(&self, app_key: u64) -> Option<u32> {
        if app_key == 0 {
            return None;
        }
        self.sessions.values().find(|s| s.id != HOST && s.parent == 0 && s.app_key == app_key).map(|s| s.id)
    }

    /// Opens a session and returns its id.
    pub fn open(&mut self, app_key: u64) -> u32 {
        let id = self.next_session;
        self.next_session += 1;
        let mut s = Session::new(id, app_key);
        s.device_generation = self.device_generation;
        self.sessions.insert(id, s);
        id
    }

    /// Opens a session under `parent` (0 = none). `privileged` is the host's capability, never a
    /// command's.
    pub fn open_session(&mut self, app_key: u64, parent: u32, privileged: bool, dpi: f32) -> u32 {
        let id = self.open(app_key);
        if let Some(s) = self.sessions.get_mut(&id) {
            s.parent = parent;
            s.privileged = privileged;
            s.dpi = dpi;
        }
        id
    }

    /// Closes a session now: frees all it owns, returns every lent object to the host, closes
    /// its children first. The host cannot be closed. Returns whether there was a session. (The
    /// HAL itself schedules closes so the backend can free its side first, see
    /// [`World::schedule_close`].)
    pub fn close(&mut self, id: u32) -> bool {
        if id == HOST || !self.sessions.contains_key(&id) {
            return false;
        }
        self.schedule_close(id, CloseAs::Exit(String::new()));
        while let Some((id, how)) = self.pop_close() {
            self.finish_close(id, how);
        }
        true
    }

    /// Marks `id` closing (it takes no more commands) and queues it after its children, which
    /// are closed with it (the host is asked to stop their apps).
    pub fn schedule_close(&mut self, id: u32, how: CloseAs) {
        if id == HOST {
            return;
        }
        let Some(s) = self.sessions.get_mut(&id) else { return };
        if s.closing {
            return;
        }
        s.closing = true;
        let mut children: Vec<u32> = self.sessions.values().filter(|c| c.parent == id).map(|c| c.id).collect();
        children.sort();
        for c in children {
            if self.domains.contains_key(&c) {
                self.host_calls.push(HostCall::Stop { domain: c, mode: domain_stop::KILL });
            }
            self.schedule_close(c, CloseAs::Exit("parent closed".into()));
        }
        self.close_queue.push((id, how));
    }

    pub fn pop_close(&mut self) -> Option<(u32, CloseAs)> {
        if self.close_queue.is_empty() {
            None
        } else {
            Some(self.close_queue.remove(0))
        }
    }

    /// Frees the session (its backend side is already freed): windows go (aliases back to the
    /// host, embedded ones away from the parent, which hears `ChildWindowClosed`), and a domain's
    /// record takes the end state and tells its parent. `Relaunch` makes the session again, empty,
    /// under the same id.
    pub fn finish_close(&mut self, id: u32, how: CloseAs) -> bool {
        let Some(mut s) = self.sessions.remove(&id) else { return false };
        self.listeners.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        // its fonts and its interest in glyphs end; the glyphs stay in the host's atlas
        if let Some(fonts) = self.fonts.as_mut() {
            fonts.session_closed(id, s.font_frames.clone());
        }
        let (parent, dpi, key) = (s.parent, s.dpi, s.app_key);
        let mut gone = Vec::new();
        for (wid, e) in s.windows.take_session(id) {
            if let Some(host) = e.val.alias_of {
                self.unlend(host);
            }
            if let Some(em) = e.val.embedded {
                if let Some(p) = self.sessions.get_mut(&parent) {
                    let _ = p.textures.remove(parent, em.alias);
                }
                gone.push(wid);
            }
        }
        for wid in gone {
            self.to_session(parent, |q| q.child_window_closed(wid, id));
        }
        // textures, buffers, pipelines, timers, ledger, queued events go with `s`
        s.timers.clear();
        if let Some(rec) = self.domains.get_mut(&id) {
            match &how {
                CloseAs::Exit(msg) => {
                    rec.state = domain_state::EXITED;
                    rec.msg = msg.clone();
                }
                CloseAs::Quit => {
                    rec.state = domain_state::EXITED;
                    rec.msg = "quit".into();
                }
                CloseAs::Crash(msg) => {
                    rec.state = domain_state::CRASHED;
                    rec.msg = msg.clone();
                }
                CloseAs::Relaunch => {
                    rec.state = domain_state::COMPILING;
                    rec.msg = "relaunch".into();
                }
            }
            if matches!(how, CloseAs::Relaunch) {
                rec.phase = 0;
                rec.pct = 0.0;
            }
            let rec = rec.clone();
            self.to_session(parent, |q| q.domain_state(id, rec.state, rec.phase, rec.pct, rec.req, &pack_strs(&[&rec.app, &rec.msg])));
        }
        if !matches!(how, CloseAs::Relaunch | CloseAs::Quit) {
            self.ended.push(id);
        }
        if matches!(how, CloseAs::Relaunch) {
            let mut ns = Session::new(id, key);
            ns.parent = parent;
            ns.dpi = self.domain_dpi(parent).unwrap_or(dpi);
            ns.attached = false;
            self.sessions.insert(id, ns);
        }
        true
    }

    /// Recomputes who hears of `sid`'s window frames (after its windows changed): the parent
    /// while it has an embedded window, the host while it presents into a host window.
    pub fn update_listeners(&mut self, sid: u32) {
        let (mut parent, mut host) = (None, None);
        if let Some(s) = self.sessions.get(&sid).filter(|_| sid != HOST) {
            if s.parent != 0 && s.windows.any_of(sid, |w| w.embedded.is_some()) { parent = Some(s.parent); }
            if s.windows.any_of(sid, |w| w.alias_of.is_some()) { host = Some(HOST); }
        }
        let mut l = self.listeners.lock().unwrap_or_else(|e| e.into_inner());
        let same = match l.get(&sid) {
            Some(who) => who.iter().copied().eq(parent.into_iter().chain(host)),
            None => parent.is_none() && host.is_none(),
        };
        if same { return; }
        let who: Vec<u32> = parent.into_iter().chain(host).collect();
        if who.is_empty() { l.remove(&sid); } else { l.insert(sid, who); }
    }

    /// A frame left draws out for a pipeline still being built (`FramePartial`), before its
    /// `frame_done`.
    pub fn frame_partial(&mut self, sid: u32, target: Id, frame: u64) {
        if self.sessions.contains_key(&sid) {
            self.to_session(sid, |q| q.frame_partial(target, frame));
        }
    }

    /// Reports completion through one path so every backend wakes both the presenter and,
    /// for a lent window, its compositor. The target kind disambiguates window/texture ids.
    pub fn frame_done(&mut self, sid: u32, target: Id, kind: u32, frame: u64, gpu_ns: u64, present_ns: u64, gpu_span: [f64; 2]) {
        let Some(s) = self.sessions.get_mut(&sid) else { return };
        s.last_frame_done = s.last_frame_done.max(frame);
        // a window frame (not a pick pass) of a window lent to the parent is also the parent's news
        let lent = if kind & !(crate::types::target::CAPTURE | crate::types::target::SNAPSHOT) == crate::types::target::WINDOW {
            s.windows.get(sid, target).ok().and_then(|w| w.val.embedded).map(|em| (s.parent, em.alias))
        } else {
            None
        };
        self.to_session(sid, |q| q.frame_done(target, frame, gpu_ns, present_ns, gpu_span[0], gpu_span[1]));
        if let Some((parent, texture)) = lent {
            self.to_session(parent, |q| q.child_frame(target, sid, texture, frame));
        }
    }

    /// A physical window actually received this frame. A skipped drawable or a texture-only
    /// packet may complete GPU work, but must not open the host's first-presentation gate.
    pub fn window_presented(&mut self, sid: u32, target: Id, _frame: u64, _gpu_ns: u64, _present_ns: u64) {
        let wid = if sid == HOST { Some(target) } else {
            self.session(sid).and_then(|s| s.windows.get(sid, target).ok()).and_then(|w| w.val.alias_of)
        };
        let Some(wid) = wid else { return };
        let Some(host) = self.sessions.get_mut(&HOST) else { return };
        let Ok(w) = host.windows.get_mut(HOST, wid) else { return };
        w.val.presentations += 1;
        w.val.presenter = sid;
        if w.lent_to == sid && sid != HOST { w.val.borrower_presented = true; }
        // The owner reads the presentation record above. Do not give its native Cx
        // another session's frame serial: it could retire its own buffers too early.
        if sid != HOST { self.to_session(HOST, |q| q.wake()); }
    }

    // ---- events across sessions

    /// Runs `f` on another session's event queue (stamped with the HAL time) and remembers to
    /// wake it.
    pub fn to_session(&mut self, sid: u32, f: impl FnOnce(&mut EventQueue)) {
        let now = self.clock.now();
        if let Some(s) = self.sessions.get_mut(&sid) {
            s.events.set_time(now);
            f(&mut s.events);
            self.notified.push(sid);
        }
    }

    /// The sessions that got events from other sessions' commands since the last call.
    pub fn take_notified(&mut self) -> Vec<u32> {
        let mut v = std::mem::take(&mut self.notified);
        v.sort();
        v.dedup();
        v
    }

    /// Whether the session was ended by the HAL and its app has not been told (once).
    pub fn take_ended(&mut self, id: u32) -> bool {
        match self.ended.iter().position(|e| *e == id) {
            Some(i) => {
                self.ended.swap_remove(i);
                true
            }
            None => false,
        }
    }

    pub fn take_host_calls(&mut self) -> Vec<HostCall> {
        std::mem::take(&mut self.host_calls)
    }

    pub fn push_host_call(&mut self, call: HostCall) {
        self.host_calls.push(call);
    }

    // ---- domains

    /// DomainStart names a session, not a window. Its main window supplies the initial
    /// scale; the compositor supplies the destination window's scale through ChildGeom.
    pub fn domain_dpi(&self, parent: u32) -> Option<f32> {
        let s = self.sessions.get(&parent).filter(|s| !s.closing)?;
        let main = s.windows.keys_of(parent).into_iter()
            .filter_map(|id| s.windows.get(parent, id).ok().map(|w| (id, &w.val)))
            .filter(|(_, w)| w.parent.is_none())
            .min_by_key(|(id, _)| id.index);
        Some(main.and_then(|(_, w)| w.dpi).unwrap_or(s.dpi))
    }

    /// The privileged-only half of `DomainStart`: the child's session exists from now (state
    /// compiling), the parent hears it, and the returned launch is what the host is asked to run.
    pub fn start_domain(&mut self, parent: u32, req: u32, app: String, args: String) -> DomainLaunch {
        let dpi = self.domain_dpi(parent).expect("domain parent");
        let n = self.next_key;
        self.next_key += 1;
        let key = (self.clock.now() ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1;
        let id = self.open_session(key, parent, false, dpi);
        if let Some(s) = self.sessions.get_mut(&id) {
            s.attached = false;
        }
        let rec = DomainRec { parent, req, app: app.clone(), args: args.clone(), app_key: key, state: domain_state::COMPILING, phase: 0, pct: 0.0, msg: String::new() };
        self.to_session(parent, |q| q.domain_state(id, rec.state, 0, 0.0, req, &pack_strs(&[&rec.app, ""])));
        self.domains.insert(id, rec);
        DomainLaunch { domain: id, parent, req, app, args, app_key: key }
    }

    /// The launch for a domain that is being (re)started.
    pub fn launch_of(&self, domain: u32) -> Option<DomainLaunch> {
        let r = self.domains.get(&domain)?;
        Some(DomainLaunch { domain, parent: r.parent, req: r.req, app: r.app.clone(), args: r.args.clone(), app_key: r.app_key })
    }

    pub fn domain(&self, id: u32) -> Option<&DomainRec> {
        self.domains.get(&id)
    }

    /// The domain a not yet attached session waits for: `key` must be the one `start_domain`
    /// handed out and `parent` the one that asked.
    pub fn domain_waiting(&self, key: u64, parent: u32) -> Option<u32> {
        if key == 0 {
            return None;
        }
        let (id, _) = self.domains.iter().find(|(_, r)| r.app_key == key && r.parent == parent)?;
        let s = self.sessions.get(id)?;
        (!s.attached && !s.closing).then_some(*id)
    }

    /// A domain session was opened by its app (or by the host for `open_domain` without a start).
    pub fn attach_domain(&mut self, id: u32, dpi: f32) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        s.attached = true;
        s.dpi = dpi;
        let parent = s.parent;
        let rec = self.domains.entry(id).or_insert(DomainRec {
            parent,
            req: 0,
            app: String::new(),
            args: String::new(),
            app_key: 0,
            state: domain_state::COMPILING,
            phase: 0,
            pct: 0.0,
            msg: String::new(),
        });
        rec.state = domain_state::RUNNING;
        rec.phase = 0;
        rec.pct = 1.0;
        rec.msg.clear();
        let rec = rec.clone();
        self.to_session(parent, |q| q.domain_state(id, rec.state, 0, 1.0, rec.req, &pack_strs(&[&rec.app, ""])));
    }

    /// Progress or end of a domain as the host sees it (compile phase, a crash message).
    pub fn report_domain(&mut self, id: u32, state: u32, phase: u32, pct: f32, msg: &str) -> bool {
        let Some(rec) = self.domains.get_mut(&id) else { return false };
        rec.state = state;
        rec.phase = phase;
        rec.pct = pct;
        rec.msg = msg.to_string();
        let rec = rec.clone();
        let parent = rec.parent;
        self.to_session(parent, |q| q.domain_state(id, rec.state, rec.phase, rec.pct, rec.req, &pack_strs(&[&rec.app, &rec.msg])));
        true
    }

    /// Frees the domain's session if it still exists and makes it again (same id, empty, waiting
    /// for its app to open it); an exited or crashed domain is made fresh. The launch is what the
    /// host runs.
    pub fn relaunch_domain(&mut self, id: u32) -> Option<DomainLaunch> {
        let rec = self.domains.get(&id)?;
        let parent = rec.parent;
        if self.sessions.contains_key(&id) {
            self.schedule_close(id, CloseAs::Relaunch);
        } else {
            let dpi = self.domain_dpi(parent)?;
            let mut ns = Session::new(id, rec.app_key);
            ns.parent = parent;
            ns.dpi = dpi;
            ns.attached = false;
            self.sessions.insert(id, ns);
            self.report_domain(id, domain_state::COMPILING, 0, 0.0, "relaunch");
        }
        self.launch_of(id)
    }

    /// The domains `parent` started.
    pub fn domains_of(&self, parent: u32) -> Vec<DomainInfo> {
        self.domains
            .iter()
            .filter(|(_, r)| r.parent == parent)
            .map(|(id, r)| DomainInfo { domain: *id, parent: r.parent, state: r.state, phase: r.phase, pct: r.pct, app: r.app.clone() })
            .collect()
    }

    /// `parent` may act on `child`: it is its child.
    pub fn is_child(&self, parent: u32, child: u32) -> bool {
        self.sessions.get(&child).is_some_and(|c| c.parent == parent && parent != HOST && !c.closing)
    }

    // ---- embedded windows

    fn surface(&mut self, sid: u32, parent: u32, win: Id, (w, h): (u32, u32), version: u32, dpi: f32, now: u64) -> Option<Embedded> {
        let tex = Id::new(LENT_BASE + win.index, version);
        let alias = Id::new(LENT_BASE + self.next_lent, 1);
        self.next_lent += 1;
        let bytes = w as u64 * h as u64 * format::texel_bytes(format::RGBA8) as u64;
        let obj = TextureObj { format: format::RGBA8, w, h, depth: 1, mips: 1, usage: usage::RENDER_TARGET | usage::SAMPLED, bytes, imported: false, uploaded: 0, lent: None };
        let s = self.sessions.get_mut(&sid)?;
        s.textures.insert(sid, tex, sid, obj.clone()).ok()?;
        s.ledger.add(LedgerEntry {
            kind: Kind::Texture,
            detail: "embedded window",
            index: tex.index,
            gen: tex.gen,
            bytes,
            format: format::RGBA8,
            width: w,
            height: h,
            mips: 1,
            attr: Attr::default(),
            created_ns: now,
            created_frame: s.frame,
            last_used_frame: 0,
            last_used_ns: 0,
        });
        let p = self.sessions.get_mut(&parent)?;
        let lent = TextureObj { lent: Some(LentRef { child: sid, tex, window: win }), ..obj };
        p.textures.insert(parent, alias, sid, lent).ok()?;
        Some(Embedded { tex, alias, version, dpi, frames: 0, last_frame: 0 })
    }

    fn retire_surface(&mut self, sid: u32, parent: u32, em: Embedded) {
        if let Some(s) = self.sessions.get_mut(&sid) {
            let _ = s.textures.remove(sid, em.tex);
            s.ledger.remove(Kind::Texture, em.tex.index);
        }
        if let Some(p) = self.sessions.get_mut(&parent) {
            let _ = p.textures.remove(parent, em.alias);
        }
    }

    fn surface_px(size: [f32; 2], dpi: f32) -> (u32, u32) {
        (((size[0] * dpi).round() as u32).max(1), ((size[1] * dpi).round() as u32).max(1))
    }

    /// Creates the embedded window `id` of a child (no OS window): a render target in the child's
    /// table, lent to the parent under an alias, announced with `ChildWindow`. Returns the
    /// pixel size, or `None` for a stale id or a missing parent.
    pub fn create_embedded(&mut self, sid: u32, id: Id, kind: u32, parent_window: Id, geom: [f32; 4], now: u64) -> Option<(u32, u32)> {
        let parent = self.sessions.get(&sid)?.parent;
        let dpi = self.domain_dpi(parent)?;
        let px = Self::surface_px([geom[2], geom[3]], dpi);
        let em = self.surface(sid, parent, id, px, 1, dpi, now)?;
        let s = self.sessions.get_mut(&sid)?;
        if s.windows.insert(sid, id, sid, WindowObj { kind, parent: parent_window, geom, dpi: Some(dpi), embedded: Some(em), ..Default::default() }).is_err() {
            self.retire_surface(sid, parent, em);
            return None;
        }
        s.dpi = dpi;
        self.announce_embedded(sid, id);
        Some(px)
    }

    /// An embedded window asked for a frame: it is answered by `deliver_embedded_frames` at the
    /// next display-rate tick, never by the window system (the window has no OS window).
    pub fn request_embedded_frame(&mut self, sid: u32, id: Id, now: u64) {
        let Some(s) = self.sessions.get_mut(&sid) else { return };
        if !s.embedded_frames.contains(&id) {
            s.embedded_frames.push(id);
        }
        if s.embedded_due_ns <= now.saturating_sub(EMBEDDED_FRAME_NS) {
            // idle since longer than a frame: the next frame is due now
            s.embedded_due_ns = now;
        }
    }

    /// FrameReady for the session's embedded windows that asked for a frame, once due; the next
    /// tick is one display frame later. Returns when the next is due (None: nothing asked).
    pub fn deliver_embedded_frames(&mut self, sid: u32, now: u64) -> Option<u64> {
        let s = self.sessions.get_mut(&sid)?;
        if s.embedded_frames.is_empty() {
            return None;
        }
        if now < s.embedded_due_ns {
            return Some(s.embedded_due_ns);
        }
        let next = now + EMBEDDED_FRAME_NS;
        s.embedded_due_ns = next;
        s.events.set_time(now);
        for id in std::mem::take(&mut s.embedded_frames) {
            s.events.frame_ready(id, next);
        }
        None
    }

    /// Tells the parent about the embedded window (again): its alias texture, size and title.
    pub fn announce_embedded(&mut self, sid: u32, id: Id) {
        let Some(s) = self.sessions.get(&sid) else { return };
        let Ok(w) = s.windows.get(sid, id) else { return };
        let Some(em) = w.val.embedded else { return };
        let Ok(t) = s.textures.get(sid, em.tex) else { return };
        let (pw, ph) = (t.val.w, t.val.h);
        let title = s.string(w.val.title).unwrap_or("").to_string();
        let parent = s.parent;
        let own_chrome = w.val.own_chrome;
        self.to_session(parent, |q| {
            q.child_window(id, sid, em.alias, pw, ph, em.dpi, title.as_bytes());
            if own_chrome { q.child_chrome(id, sid, true); }
        });
    }

    /// `ChildGeom`: the parent sets the child window's size (points) and dpi; the render target
    /// is made again when its pixels change. The child hears `Display` on scale changes and
    /// `Geom`; the parent hears `ChildWindow`.
    pub fn resize_embedded(&mut self, sid: u32, id: Id, size: [f32; 2], dpi: f32, now: u64) -> bool {
        let Some(parent) = self.sessions.get(&sid).map(|s| s.parent) else { return false };
        let Some(s) = self.sessions.get_mut(&sid) else { return false };
        let Ok(w) = s.windows.get_mut(sid, id) else { return false };
        let Some(old) = w.val.embedded else { return false };
        s.dpi = dpi;
        w.val.dpi = Some(dpi);
        w.val.geom[2] = size[0];
        w.val.geom[3] = size[1];
        let (geom, state) = (w.val.geom, w.val.state);
        let px = Self::surface_px(size, dpi);
        let same = s.textures.get(sid, old.tex).is_ok_and(|t| (t.val.w, t.val.h) == px);
        if !(same && old.dpi == dpi) {
            self.retire_surface(sid, parent, old);
            let Some(mut em) = self.surface(sid, parent, id, px, old.version + 1, dpi, now) else { return false };
            em.frames = old.frames;
            em.last_frame = old.last_frame;
            if let Some(w) = self.sessions.get_mut(&sid).and_then(|s| s.windows.get_mut(sid, id).ok()) {
                w.val.embedded = Some(em);
            }
        }
        self.to_session(sid, |q| {
            // Display precedes geometry so Cx invalidates scale-dependent layout and drawing.
            if old.dpi != dpi { q.display(id, dpi, 60.0); }
            q.geom(id, geom[0] as f64, geom[1] as f64, geom[2] as f64, geom[3] as f64, dpi, state);
        });
        self.announce_embedded(sid, id);
        true
    }

    /// `Focus{child}`: the child holds the parent's keyboard / IME / cursor / clipboard, its
    /// siblings lose it; every child window hears the change.
    pub fn focus_child(&mut self, parent: u32, child: u32) {
        let mut siblings: Vec<u32> = self.sessions.values().filter(|c| c.parent == parent).map(|c| c.id).collect();
        siblings.sort();
        for id in siblings {
            let on = id == child;
            let Some(s) = self.sessions.get_mut(&id) else { continue };
            if s.focused == on {
                continue;
            }
            s.focused = on;
            let wins: Vec<Id> = s.windows.keys_of(id).into_iter().filter(|w| s.windows.get(id, *w).is_ok_and(|e| e.val.embedded.is_some())).collect();
            self.to_session(id, |q| {
                for w in wins {
                    q.focus(w, on);
                }
            });
        }
    }

    /// `Forward`: the event goes into the child's queue under its window id.
    pub fn forward(&mut self, parent: u32, child: u32, window: Id, packed: &[u8]) -> bool {
        if !self.is_child(parent, child) {
            return false;
        }
        let Some((mut e, payload)) = unpack_forward(packed) else { return false };
        e.window = window;
        let now = self.clock.now();
        let Some(s) = self.sessions.get_mut(&child) else { return false };
        s.events.set_time(now);
        e.data = s.events.put_bytes(payload);
        s.events.push(e);
        s.stats.forwarded += 1;
        self.notified.push(child);
        true
    }

    /// Detaches (closes) a window of `session`: an alias goes back to the host, an owned window
    /// is freed, an embedded one is taken from the parent (which hears `ChildWindowClosed`).
    /// Stale id -> `Err`.
    pub fn release_window(&mut self, session: u32, id: Id) -> Result<(), Miss> {
        let s = self.sessions.get_mut(&session).ok_or(Miss::Stale)?;
        let parent = s.parent;
        let e = s.windows.remove(session, id)?;
        if let Some(host) = e.val.alias_of {
            self.unlend(host);
        }
        if let Some(em) = e.val.embedded {
            self.retire_surface(session, parent, em);
            self.to_session(parent, |q| q.child_window_closed(id, session));
        }
        Ok(())
    }

    // ---- host-owned windows (session 0) and lending

    /// A geometry notification updates the canonical host window too. Its host hears changes
    /// while the window is lent, so persistence never depends on the app surviving a crash.
    pub fn window_geom(&mut self, sid: u32, id: Id, geom: [f32; 4], dpi: f32, state: u32) {
        let Some(s) = self.sessions.get_mut(&sid) else { return };
        let Ok(w) = s.windows.get_mut(sid, id) else { return };
        s.dpi = dpi;
        w.val.geom = geom;
        w.val.dpi = Some(dpi);
        w.val.state = state;
        let Some(hid) = w.val.alias_of else { return };
        if let Some(host) = self.sessions.get_mut(&HOST) {
            host.dpi = dpi;
            if let Ok(w) = host.windows.get_mut(HOST, hid) {
                w.val.geom = geom;
                w.val.dpi = Some(dpi);
                w.val.state = state;
            }
        }
        self.to_session(HOST, |q| q.geom(hid, geom[0] as f64, geom[1] as f64, geom[2] as f64, geom[3] as f64, dpi, state));
    }

    /// Creates a window owned by the host that an app may adopt (the spinner window).
    /// A fresh id for a window of session 0 the session makes itself (a badge: `window_kind::BADGE`).
    pub fn host_window_id(&mut self) -> Id {
        let id = Id::new(self.next_host_window, 1);
        self.next_host_window += 1;
        id
    }

    pub fn host_window(&mut self, geom: [f32; 4]) -> Id {
        let id = Id::new(self.next_host_window, 1);
        self.next_host_window += 1;
        let host = self.sessions.get_mut(&HOST).expect("host session");
        let _ = host.windows.insert(HOST, id, HOST, WindowObj { geom, adoptable: true, ..Default::default() });
        id
    }

    /// Lends the first adoptable, unlent host window to `session` under its own `id`; the
    /// session's table gets an alias. `None` when there is nothing to adopt.
    pub fn adopt_window(&mut self, session: u32, id: Id) -> Option<[f32; 4]> {
        if session == HOST || !self.sessions.get(&session)?.windows.can_insert(session, id) {
            return None;
        }
        let host = self.sessions.get_mut(&HOST)?;
        let pick = host.windows.keys_of(HOST).into_iter().find(|hid| host.windows.get(HOST, *hid).is_ok_and(|e| e.val.adoptable && e.lent_to == 0))?;
        let e = host.windows.get_mut(HOST, pick).ok()?;
        let dpi = e.val.dpi.unwrap_or(host.dpi);
        e.lent_to = session;
        e.val.borrower_frame = false;
        e.val.borrower_presented = false;
        let alias = WindowObj { alias_of: Some(pick), overlay: None, ..e.val.clone() };
        let geom = e.val.geom;
        let s = self.sessions.get_mut(&session)?;
        s.dpi = dpi;
        s.windows.insert(session, id, HOST, alias).ok()?;
        Some(geom)
    }

    fn unlend(&mut self, id: Id) {
        if let Some(host) = self.sessions.get_mut(&HOST) {
            for hid in host.windows.keys_of(HOST) {
                if hid == id {
                    if let Ok(e) = host.windows.get_mut(HOST, hid) {
                        e.lent_to = 0;
                        e.val.borrower_frame = false;
                        e.val.borrower_presented = false;
                        e.val.overlay = None;
                    }
                }
            }
        }
    }

    /// Whether the host window `id` is lent, and to whom.
    pub fn lent_to(&self, id: Id) -> Option<u32> {
        let e = self.sessions.get(&HOST)?.windows.get(HOST, id).ok()?;
        (e.lent_to != 0).then_some(e.lent_to)
    }

    pub fn host_window_alive(&self, id: Id) -> bool {
        self.sessions.get(&HOST).is_some_and(|h| h.windows.get(HOST, id).is_ok())
    }

    /// Native input follows the visible owner through adoption. Geometry is mirrored
    /// regardless of ownership; frame beats keep reaching the borrower so it can boot.
    pub fn window_event(&mut self, sid: u32, mut event: RawEvent, payload: &[u8]) {
        let alias = self.session(sid).and_then(|s| s.windows.get(sid, event.window).ok())
            .and_then(|w| w.val.alias_of);
        if let Some(crate::HalEvent::Geom { window, x, y, w, h, dpi, state }) = event.decode(payload) {
            self.window_geom(sid, window, [x as f32, y as f32, w as f32, h as f32], dpi, state);
        }
        // the window buttons' rect goes to the host too, like the geometry: the host draws its
        // run-state view (a caption strip that leaves the native buttons room) in the lent window
        if let (Some(crate::HalEvent::Chrome { x, y, w, h, topmost, .. }), Some(hid)) = (event.decode(payload), alias) {
            self.to_session(HOST, |q| q.chrome(hid, x, y, w, h, topmost));
        }
        if let Some(crate::HalEvent::Display { window, dpi, .. }) = event.decode(payload) {
            if let Some(w) = self.session(sid).and_then(|s| s.windows.get(sid, window).ok()) {
                let (geom, state) = (w.val.geom, w.val.state);
                self.window_geom(sid, window, geom, dpi, state);
            }
        }
        use crate::events::event_tag as tag;
        let input = matches!(event.tag, tag::MouseDown | tag::MouseMove | tag::MouseUp | tag::MouseLeave
            | tag::Scroll | tag::Pinch | tag::Touch | tag::KeyDown | tag::KeyUp | tag::Text | tag::Ime
            | tag::Focus | tag::CloseRequested | tag::PopupDismissed
            | tag::DragQuery | tag::DragOver | tag::DragDrop);
        let host = alias.filter(|_| input).filter(|id| self.session(HOST)
                .and_then(|s| s.windows.get(HOST, *id).ok()).is_some_and(|w|
                    !w.val.borrower_presented || w.val.overlay.as_ref().is_some_and(|o| o.layer.is_none())));
        let recipient = if let Some(id) = host { event.window = id; HOST } else { sid };
        self.to_session(recipient, |q| {
            event.data = q.put_bytes(payload);
            q.push(event);
        });
    }

    /// Objects alive of one kind, summed over all sessions.
    pub fn count(&self, kind: Kind) -> usize {
        self.sessions
            .values()
            .map(|s| match kind {
                Kind::Texture => s.textures.len(),
                Kind::Buffer => s.buffers.len(),
                Kind::Pipeline => s.pipelines.len(),
                Kind::Window => s.windows.len(),
            })
            .sum()
    }
}
