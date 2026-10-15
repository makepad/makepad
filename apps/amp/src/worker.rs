//! The threads that talk to the music system, the disk and the sound card's
//! feeder, so the UI never waits:
//!
//! * the **library** worker loads the saved index of the music system, then
//!   reads the library again in the background, one command at a time — the
//!   playlist tree, each playlist's tracks, lookups for lengths and art, the
//!   saved albums and artists — handing each snapshot on and saving it. The
//!   saved index loads whether or not the system is there, so an app whose
//!   system is closed, signed out or not installed still has its library;
//! * the **local** worker reads the folders of music files (`local.rs`);
//! * the **merge** worker lays the two side by side ([`Index::merged`]) and
//!   publishes what the UI shows, newest snapshots only;
//! * the **control** worker runs transport commands the moment they come,
//!   polls what is playing while the window is in use, keeps the system's
//!   own queue holding the next tracks of ours (`queue.rs`), lists devices
//!   and answers catalog searches and radios. It says when the system comes
//!   and goes ([`Availability`]) and, while it is away, only asks whether it
//!   is back, less and less often;
//! * the **disk** worker reads and writes the album art kept on disk and
//!   the settings (the column layout, the Up Next panel, the equalizer,
//!   the play queue);
//! * the **audio** worker (for a system whose player is another app) opens
//!   and drops the equalizer's route (`audio.rs`);
//! * the **decoder** worker feeds local files to the sound card (`player.rs`).
//!
//! Commands go in over bounded channels the UI never blocks on (a full
//! queue drops the command and says so); messages come back over a channel
//! the UI drains with `try_recv` on `Event::Signal`.

use crate::audio::{AudioCommand, AudioState, Knobs};
use crate::index::Index;
use crate::local::LocalCommand;
use crate::player::{PlayerCommand, Shared};
use crate::queue::{reconcile, QueueOp};
use crate::source::{self, CatalogResults, Device, MusicSource, NowPlaying, PlaylistTrack, SourceId, TrackDetail, Transport, LOOKUP_BATCH};
use makepad_widgets::makepad_platform::thread::{SignalToUI, ThreadOptions, ThreadSpawner};
use makepad_widgets::log;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the library is read again while the app runs.
const REFRESH_EVERY: Duration = Duration::from_secs(30 * 60);
/// After a failed read the library is tried again this soon, doubling up
/// to `RETRY_LAST` while it keeps failing.
const RETRY_FIRST: Duration = Duration::from_secs(10);
const RETRY_LAST: Duration = Duration::from_secs(5 * 60);
/// Playback is polled this often while the window is in use...
const POLL_ACTIVE: Duration = Duration::from_millis(1000);
/// ...and this often while it is not, so the bar is close when it returns.
const POLL_IDLE: Duration = Duration::from_secs(10);
/// While the system is away, whether it is back is asked after this,
/// doubling up to `AWAY_LAST`.
const AWAY_FIRST: Duration = Duration::from_secs(2);
const AWAY_LAST: Duration = Duration::from_secs(30);
const DEVICES_EVERY: Duration = Duration::from_secs(15);
/// Times in a row the system's queue is put right for one state of ours
/// before the worker waits for the next change.
const SYNC_TRIES: u32 = 3;
/// After putting the system's queue right, it is read again this soon.
const SYNC_CHECK: Duration = Duration::from_millis(400);
/// An index snapshot goes on after this many playlists were read.
const PUBLISH_EVERY: usize = 4;

pub enum Command {
    /// `seq` counts up per command sent; polls say which they come after.
    Transport { seq: u64, transport: Transport },
    /// Poll playback at the active or the idle rate.
    Active(bool),
    /// Search the system's catalog; `seq` comes back with the answer.
    Search { seq: u64, query: String },
    /// Keep the system's own queue holding `desired` (our next tracks)
    /// while `current` plays; `None` leaves the system's queue alone.
    SyncQueue(Option<(String, Vec<String>)>),
    /// Read the radio of a track or artist; `seq` comes back with it.
    Radio { seq: u64, seed: String, title: String },
}

/// Whether the music system can be used now.
#[derive(Clone, Debug, PartialEq)]
pub enum Availability {
    /// Not asked yet: treated as there.
    Unknown,
    Ready,
    /// Not installed, not running, signed out or not answering, in words.
    Away(String),
}

pub enum Message {
    /// The library as far as it is read, local files beside it.
    Index(crate::index::Snapshot),
    /// What the library worker is doing, for the status line.
    Progress(String),
    /// What the local worker is doing, for the status line.
    LocalProgress(String),
    /// The music system came or went.
    Availability(Availability),
    /// `after` is the last transport command run before the poll started:
    /// a poll older than a command the UI sent shows the state before it.
    /// `player_next` is what the system's own queue has next.
    Playback { after: u64, now: Option<NowPlaying>, player_next: Option<String>, detail: Option<TrackDetail> },
    Devices { after: u64, devices: Vec<Device> },
    Catalog { seq: u64, results: Result<CatalogResults, String> },
    Radio { seq: u64, title: String, tracks: Result<Vec<PlaylistTrack>, String> },
    /// The art cache's answer for a URL: the picture, or `None` when it is
    /// not on disk yet.
    Art { url: String, bytes: Option<Arc<[u8]>> },
    /// A setting saved before, read at start.
    Saved { setting: Setting, text: String },
    /// Where the equalizer's route stands.
    Audio(AudioState),
    /// The folders of music files, as added.
    Folders(Vec<String>),
    /// The local file of this play (see `Model::local_generation`) played out.
    LocalEnded { generation: u64 },
    LocalError { generation: u64, error: String },
    Error(String),
}

/// What reaches the merge worker: the newest library of either side.
pub enum Merge {
    System(Arc<Index>),
    Local(Arc<Index>),
    Cached(Arc<crate::cache::Library>),
}

/// What the disk worker is asked.
pub enum DiskCommand {
    /// Read the picture for this URL from disk.
    Find(String),
    /// Keep a picture just downloaded.
    Keep { url: String, bytes: Arc<[u8]> },
    /// Save a setting.
    Save { setting: Setting, text: String },
}

/// A small text the app keeps between runs, one file each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// The track table's column layout (`GridColumns::serialize`).
    Columns,
    /// The Up Next panel: shown or not, and its width.
    UpNext,
    /// The equalizer and our volume (`EqSettings::serialize`).
    Equalizer,
    /// The play queue (`PlayQueue::serialize`).
    Queue,
    /// The equalizer page's picture: spectrum or MilkDrop, and MilkDrop's
    /// own choices (`MilkSettings::serialize`).
    Visualizer,
}

impl Setting {
    const ALL: [Setting; 5] = [Setting::Columns, Setting::UpNext, Setting::Equalizer, Setting::Queue, Setting::Visualizer];

    fn file(self) -> &'static str {
        match self {
            Setting::Columns => "columns.txt",
            Setting::UpNext => "up_next.txt",
            Setting::Equalizer => "eq.txt",
            Setting::Queue => "queue.txt",
            Setting::Visualizer => "visualizer.txt",
        }
    }

    /// A file bigger than this is not ours and is not read. A queue holds a
    /// whole played list (a few thousand tracks); the rest are a few lines.
    fn max_bytes(self) -> u64 {
        match self {
            Setting::Queue => 16 * 1024 * 1024,
            _ => 64 * 1024,
        }
    }
}

/// The art cache keeps at most this much on disk; the oldest files go.
const ART_CACHE_BYTES: u64 = 200 * 1024 * 1024;

/// The UI's side of the workers.
pub struct Workers {
    /// The music system installed here, if one is.
    pub source: Option<SourceId>,
    control: Option<SyncSender<Command>>,
    disk: SyncSender<DiskCommand>,
    audio: Option<SyncSender<AudioCommand>>,
    local: SyncSender<LocalCommand>,
    player: SyncSender<PlayerCommand>,
    /// The equalizer's knobs, our volume and the visualizer's rings.
    pub knobs: Knobs,
    /// What the decoder worker and the sound card share.
    pub shared: Arc<Shared>,
    messages: Receiver<Message>,
}

/// Start a named worker thread; one that will not start is logged.
fn spawn(spawner: &ThreadSpawner, name: &str, run: impl FnOnce() + Send + 'static) {
    match spawner.spawn_worker(ThreadOptions { name: Some(name.into()), ..Default::default() }, run) {
        Ok(handle) => handle.detach(),
        Err(error) => log!("amp: cannot start the {name} worker: {error}"),
    }
}

/// A local player with no external library, controller or audio capture.
pub fn local_only() -> bool {
    static LOCAL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOCAL.get_or_init(|| std::env::var_os("AMP_LOCAL_ONLY").is_some())
}

impl Workers {
    /// Starts the workers: those for the music system installed here, when
    /// there is one, and those for the library, local files and playback
    /// either way.
    pub fn start(spawner: &ThreadSpawner) -> Self {
        let (message_tx, messages) = mpsc::sync_channel(256);
        let (merge_tx, merge_rx) = mpsc::sync_channel(16);
        let tx = message_tx.clone();
        spawn(spawner, "music-merge", move || run_merge(merge_rx, tx));
        let merge = merge_tx.clone();
        spawn(spawner, "music-offline", move || crate::cache::run_library(offline_dir(), merge));

        let library_source = if local_only() { None } else { source::detect().into_iter().next() };
        let control_source = if local_only() { None } else { source::detect().into_iter().next() };
        let source = library_source.as_ref().map(|source| source.id());
        let player = library_source.as_ref().and_then(|source| source.player_bundle_id());
        let (tx, merge) = (message_tx.clone(), merge_tx.clone());
        if !local_only() {
            spawn(spawner, "music-library", move || run_library(library_source, tx, merge));
        }

        let control = control_source.map(|mut source| {
            let (control, control_rx) = mpsc::sync_channel(64);
            let tx = message_tx.clone();
            spawn(spawner, "music-control", move || run_control(source.as_mut(), control_rx, tx));
            control
        });
        if control.is_none() && !local_only() {
            send(&message_tx, Message::Availability(Availability::Away("Spotify isn't installed".into())));
        }

        let knobs = Knobs::new();
        let audio = player.map(|player| {
            let (audio, audio_rx) = mpsc::sync_channel(16);
            let (knobs, tx) = (knobs.clone(), message_tx.clone());
            spawn(spawner, "music-audio", move || crate::audio::run_audio(player, knobs, audio_rx, tx));
            audio
        });

        let (local, local_rx) = mpsc::sync_channel(16);
        let tx = message_tx.clone();
        spawn(spawner, "music-local", move || crate::local::run_local(data_dir(), local_rx, tx, merge_tx));

        let shared = Shared::new();
        let (player, player_rx) = mpsc::sync_channel(16);
        let (decoder_shared, tx) = (shared.clone(), message_tx.clone());
        spawn(spawner, "music-decoder", move || crate::player::run_decoder(decoder_shared, player_rx, tx));

        let (disk, disk_rx) = mpsc::sync_channel(16);
        spawn(spawner, "music-disk", move || run_disk(data_dir(), disk_rx, message_tx));
        Self { source, control, disk, audio, local, player, knobs, shared, messages }
    }

    /// Queue a command for the music system; `false` when there is none or
    /// it is too far behind to take it.
    pub fn send(&self, command: Command) -> bool {
        let Some(control) = &self.control else { return false };
        match control.try_send(command) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Ask the disk worker; `false` when it is too far behind to take it.
    pub fn disk(&self, command: DiskCommand) -> bool {
        self.disk.try_send(command).is_ok()
    }

    /// Ask the audio worker; `false` when there is none or it is too far
    /// behind to take it.
    pub fn audio(&self, command: AudioCommand) -> bool {
        self.audio.as_ref().is_some_and(|audio| audio.try_send(command).is_ok())
    }

    /// Whether there is a route for the equalizer to open (a system whose
    /// player is another app).
    pub fn has_route(&self) -> bool {
        self.audio.is_some()
    }

    /// Ask the local worker; `false` when it is too far behind to take it.
    pub fn local(&self, command: LocalCommand) -> bool {
        self.local.try_send(command).is_ok()
    }

    /// Ask the decoder worker; `false` when it is too far behind to take it.
    pub fn player(&self, command: PlayerCommand) -> bool {
        self.player.try_send(command).is_ok()
    }

    /// Everything published since the last call. Never waits.
    pub fn poll(&self) -> Vec<Message> {
        let mut out = Vec::new();
        while let Ok(message) = self.messages.try_recv() {
            out.push(message);
        }
        out
    }
}

pub(crate) fn send(tx: &SyncSender<Message>, message: Message) {
    // The UI drains the channel on every signal; a full channel means it is
    // not running any more, and the message has nobody to go to.
    if tx.try_send(message).is_ok() {
        SignalToUI::set_ui_signal();
    }
}

/// Worker-only delivery for snapshots that must not disappear behind a
/// busy UI. Wake the UI before waiting for space in the bounded channel.
fn send_snapshot(tx: &SyncSender<Message>, message: Message) {
    SignalToUI::set_ui_signal();
    if tx.send(message).is_ok() { SignalToUI::set_ui_signal(); }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Where the index of `source` is kept.
pub fn index_path(source: SourceId) -> Option<PathBuf> {
    Some(data_dir()?.join(format!("{}-index.txt", source.key())))
}

/// The app's data directory: a hosted launch's `SANDBOX_HOME` (its settings
/// stay in the host's home), else [`own_dir`].
pub fn data_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("SANDBOX_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Some(home.join("amp"));
    }
    own_dir()
}

/// The portable offline library (`cache`): `offline/` in [`own_dir`], also
/// for a hosted launch, so Amp inside a host shows the same songs.
pub fn offline_dir() -> Option<PathBuf> {
    Some(own_dir()?.join("offline"))
}

/// Settings, artwork, indexes and the offline library: wherever
/// `AMP_DATA_DIR` (or `MUSIC_DATA_DIR`) says (a test run keeps its data
/// apart), else `local/amp` in the Makepad checkout Amp runs from, else
/// (an installed Amp) the app's data folder. Resolved once.
fn own_dir() -> Option<PathBuf> {
    static OWN_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    OWN_DIR.get_or_init(|| {
        for var in ["AMP_DATA_DIR", "MUSIC_DATA_DIR"] {
            if let Some(dir) = std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()) {
                return Some(dir);
            }
        }
        // The running binary's checkout first; the working directory covers
        // a binary built into a target dir outside it. Not on Android: there
        // the checkout is the APK's read-only source pack.
        #[cfg(not(target_os = "android"))]
        for start in [std::env::current_exe().ok(), std::env::current_dir().ok()].into_iter().flatten() {
            for root in start.ancestors() {
                if root.join("makepad.splash").is_file() && root.join("apps/amp/Cargo.toml").is_file() {
                    return Some(root.join("local/amp"));
                }
            }
        }
        makepad_widgets::makepad_platform::home::app_data_dir("amp")
    }).clone()
}

// ---- merge ----

/// Lay the newest system library and the newest local one side by side and
/// publish them; a burst of snapshots merges once.
fn run_merge(rx: Receiver<Merge>, tx: SyncSender<Message>) {
    let mut system = Arc::new(Index::default());
    let mut local = Arc::new(Index::default());
    let mut cached = Arc::new(crate::cache::Library::default());
    while let Ok(first) = rx.recv() {
        let mut take = |part: Merge| match part {
            Merge::System(index) => system = index,
            Merge::Local(index) => local = index,
            Merge::Cached(library) => cached = library,
        };
        take(first);
        while let Ok(more) = rx.try_recv() {
            take(more);
        }
        let index = crate::index::Snapshot::from(Index::merged(&system, &local).with_cache(&cached));
        send_snapshot(&tx, Message::Index(index));
    }
}

// ---- library ----

/// Load the system's saved index, then keep reading the library while the
/// system is there. Without a system (not installed) the saved index is all
/// there is.
fn run_library(source: Option<Box<dyn MusicSource>>, tx: SyncSender<Message>, merge: SyncSender<Merge>) {
    let id = source.as_ref().map_or(SourceId::Spotify, |source| source.id());
    let path = index_path(id);
    let started = Instant::now();
    let mut index = path.as_ref().and_then(|path| Index::load(path, id)).unwrap_or_else(|| Index::new(id));
    log!("amp: index loaded in {:.1} ms: {} playlists, {} tracks", started.elapsed().as_secs_f64() * 1000.0, index.playlist_count(), index.tracks.len());
    let _ = merge.send(Merge::System(Arc::new(index.clone())));
    let Some(mut source) = source else { return };
    let mut retry = RETRY_FIRST;
    loop {
        let started = Instant::now();
        let wait = match refresh(source.as_mut(), &mut index, &tx, &merge) {
            Ok(()) => {
                log!("amp: library read in {:.1} s: {} playlists, {} tracks", started.elapsed().as_secs_f64(), index.playlist_count(), index.tracks.len());
                send(&tx, Message::Progress(String::new()));
                retry = RETRY_FIRST;
                REFRESH_EVERY
            }
            // Not running, signed out, busy: try again soon, then less
            // often while it keeps failing. The control worker says the
            // system is away; this only keeps what it read.
            Err(error) => {
                log!("amp: library read failed: {error}; again in {} s", retry.as_secs());
                send(&tx, Message::Progress(String::new()));
                let wait = retry;
                retry = (retry * 2).min(RETRY_LAST);
                wait
            }
        };
        // What was read before a failure is kept too.
        if let Some(path) = &path {
            if let Err(error) = index.save(path) {
                log!("amp: cannot save the index at {}: {error}", path.display());
            }
        }
        std::thread::sleep(wait);
    }
}

/// Read the whole library again into `index`, handing it on as it goes.
fn refresh(source: &mut dyn MusicSource, index: &mut Index, tx: &SyncSender<Message>, merge: &SyncSender<Merge>) -> Result<(), String> {
    if !source.ready()? {
        return Err(format!("{} is not running or not signed in", source.id().name()));
    }
    let publish = |index: &Index| {
        let _ = merge.send(Merge::System(Arc::new(index.clone())));
    };
    send(tx, Message::Progress("Reading playlists…".into()));
    let library = source.library()?;
    // A system still syncing can list nothing for a moment; that never
    // replaces a library already read (and saved).
    if library.is_empty() && !index.entries.is_empty() {
        return Err(format!("{} listed no playlists; keeping the ones read before", source.id().name()));
    }
    index.set_library(library);
    publish(index);

    let playlists: Vec<String> = index.entries.iter().filter(|entry| !entry.is_folder).map(|entry| entry.uri.clone()).collect();
    let total = playlists.len();
    for (at, uri) in playlists.into_iter().enumerate() {
        send(tx, Message::Progress(format!("Reading playlists {}/{total}", at + 1)));
        match source.playlist(&uri) {
            Ok(playlist) => index.set_playlist(playlist, now_secs()),
            // One playlist that will not read does not stop the rest.
            Err(error) => log!("amp: cannot read {uri}: {error}"),
        }
        if (at + 1) % PUBLISH_EVERY == 0 {
            publish(index);
        }
    }
    publish(index);

    let mut unlooked: VecDeque<String> = index.unlooked().into();
    let total = unlooked.len();
    while !unlooked.is_empty() {
        let batch: Vec<String> = unlooked.drain(..unlooked.len().min(LOOKUP_BATCH)).collect();
        send(tx, Message::Progress(format!("Looking up tracks {}/{total}", total - unlooked.len())));
        match source.lookup(&batch) {
            Ok(details) => index.set_details(&batch, details),
            Err(error) => {
                log!("amp: lookup failed: {error}");
                break;
            }
        }
    }
    publish(index);

    let albums = source.saved_albums().unwrap_or_default();
    let artists = source.saved_artists().unwrap_or_default();
    index.set_saved(albums, artists);
    publish(index);
    Ok(())
}

// ---- control ----

/// Whether the system is there, as the control worker last found it, and
/// how long to wait before asking again while it is not.
struct Presence {
    last: Option<Availability>,
    wait: Duration,
}

impl Presence {
    fn ready(&mut self) -> bool {
        self.last == Some(Availability::Ready)
    }

    /// Say what was found when it differs from what was said before.
    fn found(&mut self, availability: Availability, tx: &SyncSender<Message>) {
        if availability == Availability::Ready {
            self.wait = AWAY_FIRST;
        }
        if self.last.as_ref() != Some(&availability) {
            log!("amp: the music system is {availability:?}");
            send(tx, Message::Availability(availability.clone()));
            self.last = Some(availability);
        }
    }

    /// The system is away: say so once, and wait longer before the next ask.
    fn away(&mut self, reason: String, tx: &SyncSender<Message>) -> Duration {
        self.found(Availability::Away(reason), tx);
        let wait = self.wait;
        self.wait = (self.wait * 2).min(AWAY_LAST);
        wait
    }
}

/// What `ready` says, as an availability.
fn presence_of(source: &mut dyn MusicSource) -> Availability {
    let name = source.id().name();
    match source.ready() {
        Ok(true) => Availability::Ready,
        Ok(false) => Availability::Away(format!("{name} isn't running or isn't signed in")),
        Err(error) => Availability::Away(format!("{name} isn't answering ({error})")),
    }
}

fn run_control(source: &mut dyn MusicSource, rx: Receiver<Command>, tx: SyncSender<Message>) {
    let mut active = true;
    let mut next_poll = Instant::now();
    let mut next_devices = Instant::now();
    let mut presence = Presence { last: None, wait: AWAY_FIRST };
    // The last error said, so a failure repeating every poll is said once.
    let mut last_error = String::new();
    // The track whose length and art were looked up.
    let mut looked_up = String::new();
    // The last transport command run.
    let mut done = 0u64;
    // Our queue as the system should hold it: (current, next tracks), and
    // how many times in a row the system's queue was put right for it.
    let mut sync: Option<(String, Vec<String>)> = None;
    let mut sync_tries = 0u32;
    loop {
        let wait = next_poll.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(command) => {
                // Take everything queued: a burst of volume changes or a
                // search typed quickly only runs its last command.
                let mut commands = vec![command];
                while let Ok(more) = rx.try_recv() {
                    commands.push(more);
                }
                // Commands dropped from the burst count as run.
                let last = commands.iter().filter_map(|c| if let Command::Transport { seq, .. } = c { Some(*seq) } else { None }).max();
                for command in coalesce(commands) {
                    match command {
                        Command::Transport { transport, .. } => {
                            let started = Instant::now();
                            if let Err(error) = source.transport(&transport) {
                                send(&tx, Message::Error(error));
                            }
                            log!("amp: {transport:?} took {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
                            next_poll = Instant::now();
                            if matches!(transport, Transport::Volume(_) | Transport::Transfer(_)) {
                                next_devices = Instant::now();
                            }
                        }
                        Command::Active(on) => {
                            active = on;
                            if on && presence.ready() {
                                next_poll = Instant::now();
                            }
                        }
                        Command::Search { seq, query } => {
                            let results = source.search(&query);
                            send(&tx, Message::Catalog { seq, results });
                        }
                        Command::SyncQueue(wanted) => {
                            if wanted != sync {
                                sync = wanted;
                                sync_tries = 0;
                                if presence.ready() {
                                    next_poll = Instant::now();
                                }
                            }
                        }
                        Command::Radio { seq, seed, title } => {
                            let tracks = source.radio(&seed).unwrap_or_else(|| Err(format!("{} has no radio for this", source.id().name())));
                            send(&tx, Message::Radio { seq, title, tracks });
                        }
                    }
                }
                done = done.max(last.unwrap_or(0));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if Instant::now() < next_poll {
            continue;
        }
        // Away (or not asked yet): only ask whether it is there.
        if !presence.ready() {
            match presence_of(source) {
                Availability::Away(reason) => {
                    next_poll = Instant::now() + presence.away(reason, &tx);
                    continue;
                }
                found => presence.found(found, &tx),
            }
        }
        let after = done;
        match source.playback() {
            Ok((playing, queue)) => {
                last_error.clear();
                // A track the app has not seen gets its length and art
                // looked up once, so the bar can show both; a lookup that
                // failed is tried again on the next poll.
                let uri = playing.as_ref().map(|p| p.uri.clone()).unwrap_or_default();
                let detail = if !uri.is_empty() && uri != looked_up && uri.starts_with("spotify:track:") {
                    match source.lookup(std::slice::from_ref(&uri)) {
                        Ok(details) => {
                            looked_up = uri.clone();
                            details.into_iter().next()
                        }
                        Err(error) => {
                            log!("amp: lookup of the playing track failed: {error}");
                            None
                        }
                    }
                } else {
                    None
                };
                let player_next = queue.first().map(|entry| entry.uri.clone());
                // Put the system's queue right while our current track
                // plays; a few tries per change, then it waits for the next
                // change rather than fight the system.
                let mut fixed = false;
                if let Some((current, desired)) = &sync {
                    if uri == *current && sync_tries < SYNC_TRIES {
                        let actual: Vec<String> = queue.iter().take_while(|entry| entry.queued).map(|entry| entry.uri.clone()).collect();
                        let ops = reconcile(&actual, desired);
                        if ops.is_empty() {
                            sync_tries = 0;
                        } else {
                            sync_tries += 1;
                            fixed = true;
                            for op in ops {
                                let command = match op {
                                    QueueOp::Remove(at) => Transport::Dequeue(at),
                                    QueueOp::Add(uri) => Transport::Enqueue(uri),
                                };
                                if let Err(error) = source.transport(&command) {
                                    log!("amp: keeping the queue: {error}");
                                    break;
                                }
                            }
                        }
                    }
                }
                send(&tx, Message::Playback { after, now: playing, player_next, detail });
                next_poll = Instant::now() + if fixed { SYNC_CHECK } else if active { POLL_ACTIVE } else { POLL_IDLE };
            }
            Err(error) => {
                // A poll that fails because the system went away says that
                // once; any other failure says itself once.
                match presence_of(source) {
                    Availability::Away(reason) => {
                        next_poll = Instant::now() + presence.away(reason, &tx);
                        continue;
                    }
                    _ => {
                        if error != last_error {
                            send(&tx, Message::Error(error.clone()));
                            last_error = error;
                        }
                        next_poll = Instant::now() + if active { POLL_ACTIVE } else { POLL_IDLE };
                    }
                }
            }
        }
        if active && Instant::now() >= next_devices {
            let after = done;
            match source.devices() {
                Ok(devices) => send(&tx, Message::Devices { after, devices }),
                Err(error) => log!("amp: devices: {error}"),
            }
            next_devices = Instant::now() + DEVICES_EVERY;
        }
    }
}

// ---- disk ----

/// Where the picture for `url` is kept: an FNV-1a name per URL.
fn art_file(dir: &Path, url: &str) -> PathBuf {
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    dir.join(format!("{hash:016x}.jpg"))
}

/// Every file read and write the UI needs, off the UI thread: the album
/// art cache and the settings. Writes go to a temporary file renamed into
/// place, so a crash never leaves half a picture that is then shown
/// forever, or half a setting.
fn run_disk(data: Option<PathBuf>, rx: Receiver<DiskCommand>, tx: SyncSender<Message>) {
    let Some(data) = data else { return };
    let dir = data.join("art");
    for setting in Setting::ALL {
        let path = data.join(setting.file());
        let fits = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= setting.max_bytes());
        if let Some(text) = fits.then(|| std::fs::read_to_string(&path).ok()).flatten() {
            send(&tx, Message::Saved { setting, text });
        }
    }
    while let Ok(command) = rx.recv() {
        match command {
            DiskCommand::Find(url) => {
                let path = match url.strip_prefix(crate::cache::ART_URI) {
                    Some(name) => offline_dir().and_then(|root| crate::cache::relative(&root, name)),
                    None => Some(art_file(&dir, &url)),
                };
                let bytes = path.and_then(|p| std::fs::read(p).ok()).filter(|bytes| !bytes.is_empty()).map(Arc::from);
                send(&tx, Message::Art { url, bytes });
            }
            DiskCommand::Keep { url, bytes } => {
                let path = art_file(&dir, &url);
                let temp = path.with_extension("part");
                let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&temp, &bytes[..])).and_then(|_| std::fs::rename(&temp, &path));
                match written {
                    Ok(()) => prune_art(&dir, ART_CACHE_BYTES),
                    Err(error) => {
                        let _ = std::fs::remove_file(&temp);
                        log!("amp: cannot keep album art at {}: {error}", path.display());
                    }
                }
            }
            DiskCommand::Save { setting, text } => {
                let path = data.join(setting.file());
                let temp = path.with_extension("part");
                let written = std::fs::create_dir_all(&data).and_then(|_| std::fs::write(&temp, text)).and_then(|_| std::fs::rename(&temp, &path));
                if let Err(error) = written {
                    let _ = std::fs::remove_file(&temp);
                    log!("amp: cannot save {}: {error}", path.display());
                }
            }
        }
    }
}

/// Remove the oldest pictures until the folder holds at most `limit` bytes.
fn prune_art(dir: &Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| (meta.modified().unwrap_or(UNIX_EPOCH), meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if total <= limit {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// The commands worth running out of a burst, in order: only the last
/// search, volume, seek and queue state survive, since each replaces the
/// one before it.
fn coalesce(commands: Vec<Command>) -> Vec<Command> {
    let last_search = commands.iter().rposition(|c| matches!(c, Command::Search { .. }));
    let last_volume = commands.iter().rposition(|c| matches!(c, Command::Transport { transport: Transport::Volume(_), .. }));
    let last_seek = commands.iter().rposition(|c| matches!(c, Command::Transport { transport: Transport::Seek(_), .. }));
    let last_sync = commands.iter().rposition(|c| matches!(c, Command::SyncQueue(_)));
    commands
        .into_iter()
        .enumerate()
        .filter(|(at, command)| match command {
            Command::Search { .. } => Some(*at) == last_search,
            Command::Transport { transport: Transport::Volume(_), .. } => Some(*at) == last_volume,
            Command::Transport { transport: Transport::Seek(_), .. } => Some(*at) == last_seek,
            Command::SyncQueue(_) => Some(*at) == last_sync,
            _ => true,
        })
        .map(|(_, command)| command)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_keeps_only_the_last_search_volume_and_seek() {
        let commands = vec![
            Command::Search { seq: 1, query: "a".into() },
            Command::Transport { seq: 1, transport: Transport::Volume(0.1) },
            Command::Transport { seq: 2, transport: Transport::Next },
            Command::Search { seq: 2, query: "ab".into() },
            Command::Transport { seq: 3, transport: Transport::Volume(0.3) },
        ];
        let kept = coalesce(commands);
        assert_eq!(kept.len(), 3);
        assert!(matches!(kept[0], Command::Transport { transport: Transport::Next, .. }));
        assert!(matches!(&kept[1], Command::Search { seq: 2, .. }));
        assert!(matches!(kept[2], Command::Transport { transport: Transport::Volume(v), .. } if v == 0.3));
    }
}
