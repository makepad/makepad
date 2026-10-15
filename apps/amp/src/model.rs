//! What the views read: the library snapshot, which page is open and its
//! rows, the search, the play queue and the playback state. `App` owns it
//! and passes it to the widget tree through `Scope`; widgets change it only
//! through the methods here, which bump `version`, and leave what only
//! `App` can do (a command for the worker) in `requests`.

use crate::audio::{volume_to_linear, AudioState, EqSettings};
use crate::columns::{self, Column, Key};
use crate::index::{Hits, Index, Snapshot, LOCAL_FILE};
use crate::local::LOCAL_FOLDER;
use crate::queue::{Observed, PlayQueue, QueueItem};
use crate::source::{CatalogResults, Device, NowPlaying, Repeat, TrackDetail, Transport};
use crate::worker::Availability;
use crate::Theme;
use makepad_audio_route::AnalyzerHandle;
use makepad_widgets::{GridColumns, GridSort};
use std::sync::Arc;

/// Tracks a search lists at most; the count says how many matched.
pub const SEARCH_LIMIT: usize = 300;
/// Tracks of our queue the music system holds in its own queue.
pub const HANDED_OVER: usize = 3;
/// Previous past this far into a track starts it over.
const RESTART_AFTER_MS: f64 = 3000.0;
/// A play we asked the music system for shows at once; this long without
/// the system reporting it, what the system reports wins again.
const PENDING_SECS: f64 = 3.0;

/// The page the main pane shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    /// A playlist, by its index in `Index::entries`.
    Playlist(usize),
    Search,
    /// Library tracks by an artist.
    Artist(String),
    /// Library tracks on an album: name, artists, album uri (may be empty).
    Album(String, String, String),
    /// The equalizer and the picture of the sound.
    Equalizer,
    /// The folders of music files, to add and remove.
    Folders,
}

/// Which player plays now: the music system's own app, or this app (a
/// local file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    System,
    Local,
}

/// Which tracks a list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    /// Only tracks there is a local file of.
    Local,
}

/// What local playback is asked to do; `App` hands it to the decoder worker
/// and the sound card's shared state.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerRequest {
    /// Play `path` from its start; `now` is what the bar shows for it.
    Play { path: String, generation: u64, now: NowPlaying, duration_ms: u32, art_url: String },
    Pause,
    Resume,
    Seek(u32),
    Stop,
}

/// A line of the main table.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A group heading in search results.
    Section(String),
    /// A library track (index into `Index::tracks`), numbered in the
    /// list's own order.
    Track { id: u32, number: usize },
    /// A playlist of the library (index into `Index::entries`).
    Playlist(usize),
    Album { name: String, artists: String, uri: String },
    Artist { name: String, tracks: usize },
    /// A hit from the system's catalog: kind ("Track", "Album", ...), item.
    Catalog { kind: &'static str, uri: String, name: String, subtitle: String },
    /// A quiet line of text (an empty page, a failed search).
    Note(String),
    /// A folder of music files and the files read from it.
    Folder { path: String, files: usize },
}

/// What a row or a menu command does.
#[derive(Clone, Debug, PartialEq)]
pub enum Activate {
    /// Play the open page's track rows as shown, from this row on.
    PlayFrom(usize),
    /// Play one track on its own (a catalog hit).
    PlayOne(QueueItem),
    /// Hand playback to the system's own playlist, album or artist: the
    /// queue stops steering.
    PlayContext(String),
    /// A playlist's tracks in playlist order: played (shuffled or not) or
    /// queued.
    PlayPlaylist { at: usize, shuffle: bool },
    QueuePlaylist(usize),
    /// The open page's track rows as shown, shuffled, played.
    ShuffleShown,
    Enqueue(Vec<QueueItem>),
    PlayNext(QueueItem),
    /// The radio of a track or artist: (seed uri, title, the seed track
    /// to play first while the station loads, when it is a track).
    Radio { seed: String, title: String, first: Option<QueueItem> },
    Open(Page),
    /// Stop reading this folder of music files.
    RemoveFolder(String),
}

/// The commands a row's menu offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowCommand {
    Play,
    ShufflePlay,
    PlayNext,
    Enqueue,
    QueueAll,
    SongRadio,
    ArtistRadio,
    Open,
    Album,
    Artist,
    RemoveFolder,
}

impl RowCommand {
    pub fn label(self) -> &'static str {
        match self {
            RowCommand::Play => "Play",
            RowCommand::ShufflePlay => "Shuffle Play",
            RowCommand::PlayNext => "Play Next",
            RowCommand::Enqueue => "Add to Queue",
            RowCommand::QueueAll => "Queue All",
            RowCommand::SongRadio => "Song Radio",
            RowCommand::ArtistRadio => "Artist Radio",
            RowCommand::Open => "Open",
            RowCommand::Album => "Go to Album",
            RowCommand::Artist => "Go to Artist",
            RowCommand::RemoveFolder => "Remove Folder",
        }
    }
}

/// Where playback is. The Spotify CLI reports what plays and whether it
/// plays, not the position: the position here is an ESTIMATE, anchored
/// when a track change or a seek is seen and advanced by the clock while
/// playing. A track first seen already under way has no known position.
#[derive(Clone, Debug, Default)]
pub struct Playback {
    pub now: Option<NowPlaying>,
    pub duration_ms: u32,
    pub art_url: String,
    /// (position at the anchor in ms, anchor time in app seconds).
    anchor: Option<(f64, f64)>,
}

impl Playback {
    pub fn is_playing(&self) -> bool {
        self.now.as_ref().is_some_and(|now| now.is_playing)
    }

    /// The estimated position in ms, when it is known.
    pub fn position_ms(&self, now_secs: f64) -> Option<f64> {
        let (at, since) = self.anchor?;
        let elapsed = if self.is_playing() { (now_secs - since).max(0.0) * 1000.0 } else { 0.0 };
        let position = at + elapsed;
        Some(if self.duration_ms > 0 { position.min(self.duration_ms as f64) } else { position })
    }

    /// A poll came back. `poll_gap` is the time since the previous poll: a
    /// track change happened somewhere inside it, so the new track is
    /// assumed to have started half a gap ago.
    pub fn update(&mut self, now: Option<NowPlaying>, detail: Option<&TrackDetail>, now_secs: f64, poll_gap: f64) {
        let old_uri = self.now.as_ref().map(|n| n.uri.clone());
        let new_uri = now.as_ref().map(|n| n.uri.clone());
        let was_playing = self.is_playing();
        let position = self.position_ms(now_secs);
        if old_uri != new_uri {
            self.anchor = match (&old_uri, &new_uri) {
                // Seen changing while watched: it started within the gap.
                (Some(_), Some(_)) => Some((0.0, now_secs - poll_gap.min(2.0) * 0.5)),
                _ => None,
            };
            self.duration_ms = 0;
            self.art_url.clear();
        } else if let Some(position) = position {
            let playing = now.as_ref().is_some_and(|n| n.is_playing);
            if playing && was_playing && self.duration_ms > 0 && position >= self.duration_ms as f64 {
                // The estimate ran out while the same track still plays: it
                // started over (repeat one) or the estimate was wrong. Where
                // it is is no longer known.
                self.anchor = None;
            } else if playing != was_playing {
                // Paused or resumed: carry the position over.
                self.anchor = Some((position, now_secs));
            }
        }
        if let Some(detail) = detail {
            if Some(&detail.uri) == new_uri.as_ref() {
                self.duration_ms = detail.duration_ms;
                self.art_url = detail.art_url.clone();
            }
        }
        self.now = now;
    }

    /// A track this app plays itself starts: what it is, how long, its
    /// art, and that it is at `position_ms` now (the player's own count
    /// keeps the anchor right from then on, see `App::update_progress`).
    pub fn start(&mut self, now: NowPlaying, duration_ms: u32, art_url: String, position_ms: f64, now_secs: f64) {
        self.now = Some(now);
        self.duration_ms = duration_ms;
        self.art_url = art_url;
        self.anchor = Some((position_ms, now_secs));
    }

    /// The user sought: the position is known from here.
    pub fn seek(&mut self, position_ms: f64, now_secs: f64) {
        self.anchor = Some((position_ms, now_secs));
    }

    /// The playing track starts over (Previous past its first seconds, or
    /// Play of the track already playing): the URI does not change, so a
    /// poll cannot tell. When Previous goes to another track instead, the
    /// poll that sees it sets the anchor again.
    pub fn restart(&mut self, now_secs: f64) {
        if self.now.is_some() {
            self.anchor = Some((0.0, now_secs));
        }
    }

    pub fn uri(&self) -> Option<&str> {
        self.now.as_ref().map(|now| now.uri.as_str())
    }

    /// Our own play/pause, applied before the next poll confirms it.
    pub fn set_playing(&mut self, playing: bool, now_secs: f64) {
        let position = self.position_ms(now_secs);
        if let Some(now) = &mut self.now {
            now.is_playing = playing;
        }
        if let Some(position) = position {
            self.anchor = Some((position, now_secs));
        }
    }
}


/// A play asked of the music system and shown before it confirms.
#[derive(Clone, Debug)]
pub struct Pending {
    pub uri: String,
    /// When it was sent (app seconds); `None` until then.
    pub at: Option<f64>,
    /// What the system last really played.
    pub previous: Playback,
}

/// What a widget asked `App` to do.
#[derive(Default)]
pub struct Requests {
    pub transport: Vec<Transport>,
    /// The query changed: search the catalog after a pause.
    pub catalog: bool,
    /// The table's columns changed (chosen, moved, resized, sorted): save them.
    pub columns: bool,
    /// The play queue changed: hand its next tracks to the system.
    pub queue: bool,
    /// Fetch a radio: (seed uri, title).
    pub radio: Option<(String, String)>,
    /// For local playback, in order.
    pub player: Vec<PlayerRequest>,
    /// The knobs changed (our volume): turn them.
    pub knobs: bool,
    /// Keep the equalizer settings (our volume among them).
    pub save_eq: bool,
    /// Stop reading these folders of music files.
    pub remove_folders: Vec<String>,
    pub redraw: bool,
}

pub struct Model {
    pub theme: Theme,
    pub index: Arc<Index>,
    library: Snapshot,
    /// The cached songs were opened for the person once, with the music
    /// system away (`select_index`): not again over their own choice.
    offline_opened: bool,
    pub page: Page,
    pub rows: Vec<Row>,
    pub query: String,
    pub hits: Hits,
    pub catalog: Option<Result<CatalogResults, String>>,
    /// Bumped per query, so an old catalog answer is recognised.
    pub catalog_seq: u64,
    pub playback: Playback,
    /// A play asked of the music system, shown before it confirms
    /// ([`Self::reconcile`]).
    pub pending: Option<Pending>,
    /// Our play order: what plays after the current track.
    pub queue: PlayQueue,
    /// Bumped per radio asked for, so an old station answer is dropped.
    pub radio_seq: u64,
    pub devices: Vec<Device>,
    /// Local switches: the CLI sets them but does not report them.
    pub shuffle: bool,
    pub repeat: Repeat,
    /// The library worker's progress, for the status line.
    pub progress: String,
    pub error: Option<String>,
    /// How long the last local search took, in ms.
    pub search_ms: f64,
    /// The track table's columns and the sort a heading press chose.
    pub columns: GridColumns<Column>,
    /// The equalizer as set, and where its route stands.
    pub eq: EqSettings,
    pub audio: AudioState,
    /// What the route and local playback played, for the visualizers.
    pub route_sound: Option<AnalyzerHandle>,
    pub local_sound: Option<AnalyzerHandle>,
    /// Whether the music system is there.
    pub availability: Availability,
    /// Whether the equalizer has a player to route (the system's player is
    /// another app); without one it shapes local files only.
    pub has_route: bool,
    /// The music system plays but the Spotify audio connection is not up:
    /// the pictures show the connection's hint instead of their own.
    pub connect_hint: bool,
    /// Which player plays now.
    pub engine: Engine,
    /// The local file playing, and which play of it (a finish reported for
    /// an older play is not this one's).
    pub local_path: Option<String>,
    pub local_generation: u64,
    /// The local file played out with nothing of ours after it: Play
    /// starts it again.
    pub local_done: bool,
    pub filter: Filter,
    /// The folders of music files, as added.
    pub folders: Vec<String>,
    /// The local worker's progress, for the status line.
    pub local_progress: String,
    pub version: u64,
    /// Bumped when a style reload reapplied the widget tree.
    pub style_gen: u64,
    pub requests: Requests,
}

impl Model {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            index: Arc::new(Index::default()),
            library: Snapshot::default(),
            offline_opened: false,
            page: Page::Search,
            rows: Vec::new(),
            query: String::new(),
            hits: Hits::default(),
            catalog: None,
            catalog_seq: 0,
            playback: Playback::default(),
            pending: None,
            queue: PlayQueue::default(),
            radio_seq: 0,
            devices: Vec::new(),
            shuffle: false,
            repeat: Repeat::Off,
            progress: String::new(),
            error: None,
            search_ms: 0.0,
            columns: columns::layout(),
            eq: EqSettings::default(),
            audio: AudioState::Off,
            route_sound: None,
            local_sound: None,
            availability: Availability::Unknown,
            has_route: false,
            connect_hint: false,
            engine: Engine::System,
            local_path: None,
            local_generation: 0,
            local_done: false,
            filter: Filter::All,
            folders: Vec::new(),
            local_progress: String::new(),
            version: 0,
            style_gen: 0,
            requests: Requests::default(),
        }
    }

    pub fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.requests.redraw = true;
    }

    /// A transport command, to whichever player plays: the music system's,
    /// or this app's for a local file. A play goes through [`Self::start`].
    pub fn transport(&mut self, command: Transport) {
        if self.engine == Engine::Local {
            if command == Transport::Resume && self.local_done {
                let current = self.queue.current.clone();
                self.start(current);
                return;
            }
            let local = match command {
                Transport::Pause => Some(PlayerRequest::Pause),
                Transport::Resume => Some(PlayerRequest::Resume),
                Transport::Seek(ms) => Some(PlayerRequest::Seek(ms)),
                // Without a queue of ours to go on with, the track ends
                // here, or starts over.
                Transport::Next => Some(PlayerRequest::Stop),
                Transport::Previous => Some(PlayerRequest::Seek(0)),
                _ => None,
            };
            if let Some(request) = local {
                self.requests.player.push(request);
                return;
            }
        }
        if self.system_ready() {
            self.requests.transport.push(command);
        }
    }

    /// Whether the music system can take commands (not known to be away).
    pub fn system_ready(&self) -> bool {
        !matches!(self.availability, Availability::Away(_))
    }

    /// The music system came or went.
    pub fn set_availability(&mut self, availability: Availability) {
        let was_ready = self.system_ready();
        self.availability = availability;
        self.requests.queue = true;
        if was_ready != self.system_ready() { self.select_index(); } else { self.changed(); }
    }

    /// Whether the library track `id` can play now: the system is there,
    /// or there is a local file of it.
    pub fn playable(&self, id: u32) -> bool {
        let local = self.index.tracks.get(id as usize).is_some_and(|track| track.local_path().is_some());
        (self.system_ready() && !local) || self.index.has_local(id)
    }

    /// Whether the track of `uri` (in the library or not) can play now.
    pub fn can_play(&self, uri: &str) -> bool {
        self.index.local_file(uri).is_some() || (self.system_ready() && !uri.starts_with(LOCAL_FILE))
    }

    /// Whether the slider is our volume stage: a local file plays. A player
    /// the equalizer routes keeps its own volume, which the slider drives as
    /// when it is not routed, so routing never changes the level (our stage
    /// on top would compound with it).
    pub fn own_volume(&self) -> bool {
        self.engine == Engine::Local
    }

    /// Where the volume slider stands: our volume while the sound runs
    /// through this app, else the active device's.
    pub fn volume_shown(&self) -> Option<f64> {
        if self.own_volume() {
            Some(self.eq.volume as f64)
        } else {
            self.devices.iter().find(|device| device.active).map(|device| device.volume as f64)
        }
    }

    /// The volume slider moved (`done`: let go). Our own volume follows the
    /// slider as it moves and is kept when let go; the system's is told
    /// once, when let go, and never while the sound runs through us.
    pub fn set_volume(&mut self, level: f32, done: bool) {
        let level = level.clamp(0.0, 1.0);
        if self.own_volume() {
            self.eq.volume = level;
            self.requests.knobs = true;
            self.requests.save_eq |= done;
        } else {
            // The music system's volume follows the dial as it turns: the
            // worker runs only the last of a burst of volume changes.
            if let Some(device) = self.devices.iter_mut().find(|device| device.active) {
                device.volume = level;
            }
            self.transport(Transport::Volume(level));
        }
        self.requests.redraw = true;
    }

    /// The played sound the visualizers show: the local player's while it
    /// plays, else the route's while it is routed.
    /// The music system plays on another device (a speaker, a phone,
    /// another computer): this machine hears nothing, so the tap, the
    /// visuals and the equalizer have nothing to work on.
    pub fn remote_device(&self) -> Option<&Device> {
        if self.engine == Engine::Local {
            return None;
        }
        self.devices.iter().find(|device| device.active && !device.this_machine && device.kind != "Setup")
    }

    /// This machine's own player, to bring playback back to.
    pub fn this_machine_device(&self) -> Option<&Device> {
        self.devices.iter().find(|device| device.this_machine)
    }

    /// What our volume does to [`Model::sound`] on its way out: the tap
    /// sits before our volume (the visualizers show the music, whatever the
    /// volume), while a level meter shows what is heard. A routed player's
    /// own volume is already in its sound.
    pub fn level_gain(&self) -> f32 {
        if self.own_volume() { volume_to_linear(self.eq.volume) } else { 1.0 }
    }

    pub fn sound(&self) -> Option<&AnalyzerHandle> {
        if self.engine == Engine::Local {
            self.local_sound.as_ref()
        } else if matches!(self.audio, AudioState::Routed { .. }) {
            self.route_sound.as_ref()
        } else {
            None
        }
    }

    /// Show every track, or only those there is a local file of.
    pub fn set_filter(&mut self, filter: Filter) {
        if filter != self.filter {
            self.filter = filter;
            self.rebuild();
        }
    }

    /// The folders of music files, as the local worker has them now.
    pub fn set_folders(&mut self, folders: Vec<String>) {
        self.folders = folders;
        if self.page == Page::Folders {
            self.rebuild();
        } else {
            self.changed();
        }
    }

    /// A press on a column's heading: rising, falling, then back to the
    /// list's own order.
    pub fn press_heading(&mut self, column: Column) {
        if self.columns.press_heading(column) {
            self.requests.columns = true;
            self.rebuild();
        }
    }

    /// The columns were chosen, moved or read back: rebuild in case the
    /// sorted column went away.
    pub fn columns_changed(&mut self, save: bool) {
        self.requests.columns |= save;
        self.rebuild();
    }

    /// A new library snapshot: the open page follows it (a playlist is
    /// found again by URI, since the tree may have moved).
    pub fn set_index(&mut self, index: Snapshot) {
        self.library = index;
        self.select_index();
    }

    fn select_index(&mut self) {
        let open = match &self.page {
            Page::Playlist(at) => self.index.entries.get(*at).map(|entry| entry.uri.clone()),
            _ => None,
        };
        self.index = if self.system_ready() { self.library.all.clone() } else { self.library.offline.clone() };
        if let Some(uri) = open {
            self.page = match self.index.entry(&uri) {
                Some(at) => Page::Playlist(at),
                None => Page::Search,
            };
        }
        // No music system (a phone, Spotify not running): Amp opens on the
        // cached songs, once, instead of an empty search page.
        let away = !self.system_ready() || crate::worker::local_only();
        if away && !self.offline_opened && self.page == Page::Search && self.query.trim().is_empty() {
            if let Some(at) = self.index.entry(crate::cache::LIBRARY_URI) {
                self.offline_opened = true;
                self.page = Page::Playlist(at);
            }
        }
        if !self.query.is_empty() {
            self.run_search();
        }
        self.rebuild();
    }

    pub fn open(&mut self, page: Page) {
        self.page = page;
        self.rebuild();
    }

    /// The query changed: search the library now, the catalog later.
    pub fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        self.query = query.to_string();
        self.catalog = None;
        self.catalog_seq += 1;
        self.run_search();
        self.page = Page::Search;
        self.requests.catalog = !query.trim().is_empty();
        self.rebuild();
    }

    fn run_search(&mut self) {
        let started = std::time::Instant::now();
        self.hits = self.index.search(&self.query, SEARCH_LIMIT);
        self.search_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    pub fn set_catalog(&mut self, seq: u64, results: Result<CatalogResults, String>) {
        if seq != self.catalog_seq {
            return;
        }
        self.catalog = Some(results);
        if self.page == Page::Search {
            self.rebuild();
        }
    }

    /// Title and subtitle of the open page.
    pub fn heading(&self) -> (String, String) {
        let index = &self.index;
        match &self.page {
            Page::Playlist(at) => match index.entries.get(*at) {
                Some(entry) => {
                    let listed = entry.tracks.len();
                    let length: u64 = entry.tracks.iter().map(|&id| index.tracks[id as usize].duration_ms as u64).sum();
                    let mut subtitle = crate::widgets::tracks_word(entry.track_count.max(listed as u32) as u64);
                    if length > 0 {
                        subtitle.push_str(&format!(" · {}", long_duration(length)));
                    }
                    if !entry.owner.is_empty() {
                        subtitle = format!("{} · {subtitle}", entry.owner);
                    }
                    if entry.read_secs == 0 && entry.tracks.is_empty() {
                        subtitle.push_str(" · reading…");
                    }
                    (entry.name.clone(), subtitle)
                }
                None => (String::new(), String::new()),
            },
            Page::Search if self.query.trim().is_empty() => (
                "Search".into(),
                format!("{} tracks in {} playlists, searched as you type", index.tracks.len(), index.playlist_count()),
            ),
            Page::Search => (
                format!("“{}”", self.query.trim()),
                format!("{} in your playlists · {:.1} ms", crate::widgets::tracks_word(self.hits.track_total as u64), self.search_ms),
            ),
            Page::Artist(name) => (name.clone(), "Artist · in your playlists".into()),
            Page::Album(name, artists, _) => (name.clone(), format!("Album · {artists}")),
            Page::Equalizer => ("Equalizer".into(), self.audio_status()),
            Page::Folders => {
                let files = index.tracks.iter().filter(|track| track.local_path().is_some()).count();
                let mut subtitle = format!("{} folders · {files} files", self.folders.len());
                if !self.local_progress.is_empty() {
                    subtitle = format!("{subtitle} · {}", self.local_progress);
                }
                ("Local Files".into(), subtitle)
            }
        }
    }

    /// The line over the lists while the music system is away, else `None`.
    pub fn away_note(&self) -> Option<String> {
        if crate::worker::local_only() { return None; }
        let Availability::Away(reason) = &self.availability else { return None };
        let name = self.index.source.map(|s| s.name()).unwrap_or("Spotify");
        Some(format!("Offline library · cached songs and local files only ({name}: {reason})"))
    }

    /// Where the equalizer stands, in words.
    pub fn audio_status(&self) -> String {
        if !self.has_route {
            return if self.eq.on { "On · local files play through it".into() } else { "Off · local files play flat".into() };
        }
        let player = self.index.source.map(|s| s.name()).unwrap_or("The player");
        let eq = if self.eq.on { "On" } else { "Off (bypassed)" };
        match &self.audio {
            AudioState::Off => format!("{eq} · {player} audio not connected"),
            AudioState::Waiting => format!("{eq} · waiting for sound from {player}"),
            AudioState::Routed { device, sample_rate } => format!("{eq} · {player} → {device} · {:.1} kHz", sample_rate / 1000.0),
            AudioState::Failed(error) => error.clone(),
        }
    }

    // ---- the play queue ----

    fn track_item(&self, id: u32) -> QueueItem {
        let track = &self.index.tracks[id as usize];
        QueueItem { uri: track.uri.clone(), name: track.name.clone(), artist: track.artists.clone() }
    }

    /// The open page's track rows as shown (sorted as the headings chose),
    /// with their row numbers. On a search page, the library tracks only.
    pub fn shown_tracks(&self) -> Vec<(usize, QueueItem)> {
        self.rows.iter().enumerate().filter_map(|(row, line)| match line {
            Row::Track { id, .. } => Some((row, self.track_item(*id))),
            _ => None,
        }).collect()
    }

    /// What the open page is called where the queue says where it plays from.
    fn page_source(&self) -> String {
        match &self.page {
            Page::Search => format!("Search “{}”", self.query.trim()),
            _ => self.heading().0,
        }
    }

    /// A playlist's tracks in its own order.
    fn playlist_items(&self, at: usize) -> Vec<QueueItem> {
        self.index.entries.get(at).map(|entry| entry.tracks.iter().map(|&id| self.track_item(id)).collect()).unwrap_or_default()
    }

    /// Play `item` now: the queue already says it is current. A track that
    /// cannot play (the system is away and there is no local file of it) is
    /// skipped, with a note, for the next of ours that can.
    fn start(&mut self, item: Option<QueueItem>) {
        let mut item = item;
        // Each skip takes one from the queue; it runs out at the end.
        while let Some(current) = item.take() {
            if self.play_item(&current) {
                break;
            }
            let name = self.index.source.map(|s| s.name()).unwrap_or("The music system");
            self.error = Some(format!("Skipped “{}”: {name} is unavailable and there is no local file of it", current.name));
            if self.queue.driving {
                item = self.queue.advance();
            }
        }
        self.requests.queue = true;
        self.changed();
    }

    /// Optimistic: the track asked of the music system is the one shown
    /// (row, now playing, art, 0:00) before the system confirms it; what
    /// the system last really played is kept to go back to.
    fn show_pending(&mut self, item: &QueueItem) {
        let previous = match self.pending.take() {
            Some(pending) => pending.previous,
            None => self.playback.clone(),
        };
        let track = self.index.track(&item.uri);
        self.playback = Playback {
            now: Some(NowPlaying { uri: item.uri.clone(), title: item.name.clone(), artist: item.artist.clone(), context: self.queue.source.clone(), is_playing: true }),
            duration_ms: track.map_or(0, |track| track.duration_ms),
            art_url: track.map(|track| track.art_url.clone()).unwrap_or_default(),
            anchor: None,
        };
        self.pending = Some(Pending { uri: item.uri.clone(), at: None, previous });
    }

    /// The play was sent at `now_secs`: the pending track counts from 0:00.
    pub fn pending_sent(&mut self, uri: &str, now_secs: f64) {
        if let Some(pending) = self.pending.as_mut().filter(|pending| pending.uri == uri && pending.at.is_none()) {
            pending.at = Some(now_secs);
            self.playback.restart(now_secs);
        }
    }

    /// A poll of the system while a play is pending. It reports the track:
    /// confirmed, counted from when it most likely started. Something else
    /// within [`PENDING_SECS`]: still switching, the poll is held back
    /// (`true`). Later, or the play failed (`failed`): the system is right,
    /// the track it really played comes back and the queue follows it.
    pub fn reconcile(&mut self, polled: Option<&str>, now_secs: f64, poll_gap: f64, failed: bool) -> bool {
        let Some(pending) = &self.pending else { return false };
        let at = pending.at.unwrap_or(now_secs);
        if polled == Some(pending.uri.as_str()) && !failed {
            self.pending = None;
            self.playback.seek(0.0, (now_secs - poll_gap.min(2.0) * 0.5).max(at));
            return false;
        }
        if !failed && now_secs - at < PENDING_SECS {
            return true;
        }
        let Some(pending) = self.pending.take() else { return false };
        let name = self.index.source.map(|s| s.name()).unwrap_or("The music system");
        let title = self.playback.now.as_ref().map(|now| now.title.clone()).unwrap_or_default();
        self.error = Some(format!("{name} did not start “{title}”"));
        self.playback = pending.previous;
        if let Some(uri) = polled {
            self.follow_player(uri, false);
        }
        self.changed();
        false
    }

    /// Hand `item` to the player that can play it: this app for a local file
    /// (or the local copy of a system track while the system is away), else
    /// the system's. `false` when neither can.
    fn play_item(&mut self, item: &QueueItem) -> bool {
        let is_file = item.uri.starts_with(LOCAL_FILE);
        let local = self.index.local_file(&item.uri).map(str::to_string);
        if let Some(path) = local {
            if self.engine == Engine::System && self.playback.is_playing() && self.system_ready() {
                self.requests.transport.push(Transport::Pause);
            }
            self.engine = Engine::Local;
            self.pending = None;
            self.local_generation += 1;
            self.local_done = false;
            self.local_path = Some(path.clone());
            let track = self.index.track(&item.uri);
            let duration_ms = track.map_or(0, |track| track.duration_ms);
            // A local copy shows its original's art.
            let art_url = track
                .map(|track| match track.copy_of {
                    Some(original) => self.index.tracks[original as usize].art_url.clone(),
                    None => track.art_url.clone(),
                })
                .unwrap_or_default();
            let now = NowPlaying { uri: item.uri.clone(), title: item.name.clone(), artist: item.artist.clone(), context: self.queue.source.clone(), is_playing: true };
            self.requests.player.push(PlayerRequest::Play { path, generation: self.local_generation, now, duration_ms, art_url });
            true
        } else if self.system_ready() && !is_file {
            self.leave_local();
            self.requests.transport.push(Transport::Play(item.uri.clone()));
            self.show_pending(item);
            true
        } else {
            false
        }
    }

    /// The system's player takes over from a local file.
    fn leave_local(&mut self) {
        if self.engine == Engine::Local {
            self.requests.player.push(PlayerRequest::Stop);
            self.engine = Engine::System;
            self.local_path = None;
        }
    }

    /// The local file of play `generation` played out: our next track
    /// follows; repeat one plays it again.
    pub fn local_ended(&mut self, generation: u64) {
        self.local_finished(generation, false);
    }

    /// The local file of play `generation` could not be read or decoded:
    /// our next track follows even under repeat one — playing the same file
    /// again would fail again, for ever.
    pub fn local_failed(&mut self, generation: u64) {
        if generation == self.local_generation && self.engine == Engine::Local && !self.local_done && self.system_ready() {
            if let Some(item) = self.queue.current.clone().filter(|item| {
                self.index.track(&item.uri).is_some_and(|track| track.cached_path.as_deref() == self.local_path.as_deref() && track.cached_path.is_some())
            }) {
                self.leave_local();
                self.requests.transport.push(Transport::Play(item.uri.clone()));
                self.show_pending(&item);
                self.changed();
                return;
            }
        }
        self.local_finished(generation, true);
    }

    fn local_finished(&mut self, generation: u64, failed: bool) {
        // A file that failed also reports its end: the play is over once.
        if generation != self.local_generation || self.engine != Engine::Local || self.local_done {
            return;
        }
        if self.repeat == Repeat::Track && !failed {
            let current = self.queue.current.clone();
            self.start(current);
        } else if self.queue.driving && self.queue.upcoming_len() > 0 {
            let next = self.queue.advance();
            self.start(next);
        } else {
            // Played out: the output closes until something plays again.
            self.local_done = true;
            self.requests.player.push(PlayerRequest::Stop);
            self.changed();
        }
    }

    /// The system's player started playing while a local file played: the
    /// person chose something in it. The local file stops; the poll that
    /// saw it follows the system from here.
    pub fn system_took_over(&mut self) {
        self.leave_local();
        self.changed();
    }

    /// Our next tracks the system should hold in its own queue while our
    /// current one plays there: none while a local file plays or the system
    /// is away, and only up to the first local file (which this app plays).
    pub fn handed_over(&self) -> Option<(String, Vec<String>)> {
        let queue = &self.queue;
        let current = queue.current.as_ref().filter(|_| queue.driving && self.engine == Engine::System && self.system_ready())?;
        if current.uri.starts_with(LOCAL_FILE) {
            return None;
        }
        Some((current.uri.clone(), queue.desired(HANDED_OVER, |item| !item.uri.starts_with(LOCAL_FILE) && self.index.local_file(&item.uri).is_none())))
    }

    /// Steer again from what plays, if the queue had let go: the person is
    /// queuing, which only makes sense after the track playing now.
    fn adopt_playing(&mut self) {
        if let Some(now) = &self.playback.now {
            let item = QueueItem { uri: now.uri.clone(), name: now.title.clone(), artist: now.artist.clone() };
            self.queue.adopt(item);
        }
    }

    /// Do what a row or a menu command asks.
    pub fn run(&mut self, chosen: Activate) {
        match chosen {
            Activate::PlayFrom(row) => {
                let shown = self.shown_tracks();
                let Some(start) = shown.iter().position(|(at, _)| *at == row) else { return };
                let source = self.page_source();
                let first = self.queue.play_list(shown.into_iter().map(|(_, item)| item).collect(), start, &source);
                self.start(first);
            }
            Activate::PlayOne(item) => {
                let first = self.queue.play_one(item);
                self.start(Some(first));
            }
            Activate::PlayContext(uri) => {
                if !self.system_ready() {
                    self.error = Some("This plays in the music system, which is unavailable".into());
                    self.changed();
                    return;
                }
                self.queue.detach();
                self.leave_local();
                self.transport(Transport::Play(uri));
                self.requests.queue = true;
                self.changed();
            }
            Activate::PlayPlaylist { at, shuffle } => {
                let mut items = self.playlist_items(at);
                if shuffle {
                    shuffled(&mut items);
                }
                let name = self.index.entries.get(at).map(|entry| entry.name.clone()).unwrap_or_default();
                let first = self.queue.play_list(items, 0, &name);
                self.start(first);
            }
            Activate::ShuffleShown => self.shuffle_page(),
            Activate::QueuePlaylist(at) => {
                let items = self.playlist_items(at);
                self.run(Activate::Enqueue(items));
            }
            Activate::Enqueue(items) => {
                self.adopt_playing();
                self.queue.enqueue(items);
                self.requests.queue = true;
                self.changed();
            }
            Activate::PlayNext(item) => {
                self.adopt_playing();
                self.queue.play_next(item);
                self.requests.queue = true;
                self.changed();
            }
            Activate::Radio { seed, title, first } => {
                if !self.system_ready() {
                    self.error = Some("Radio comes from the music system, which is unavailable".into());
                    self.changed();
                    return;
                }
                self.radio_seq += 1;
                // The seed plays at once; the station fills the queue when
                // it answers. A seed already playing just keeps playing.
                match first {
                    Some(first) if self.playback.uri() != Some(first.uri.as_str()) => {
                        let first = self.queue.play_list(vec![first], 0, &format!("Radio: {title}"));
                        self.start(first);
                    }
                    Some(first) => {
                        self.queue.play_list(vec![first], 0, &format!("Radio: {title}"));
                        self.requests.queue = true;
                        self.changed();
                    }
                    None => {}
                }
                self.requests.radio = Some((seed, title));
            }
            Activate::Open(page) => self.open(page),
            Activate::RemoveFolder(path) => self.requests.remove_folders.push(path),
        }
    }

    /// A station answered for the radio asked last: its tracks follow the
    /// seed. Without a seed track (an artist's radio) the first plays.
    pub fn set_radio(&mut self, seq: u64, title: &str, tracks: Vec<QueueItem>) {
        if seq != self.radio_seq {
            return;
        }
        let source = format!("Radio: {title}");
        let seed = self.queue.current.as_ref().filter(|_| self.queue.source == source && self.queue.driving).map(|item| item.uri.clone());
        match seed {
            Some(seed) => {
                self.queue.list = tracks.into_iter().filter(|item| item.uri != seed).collect();
                self.requests.queue = true;
                self.changed();
            }
            None => {
                let first = self.queue.play_list(tracks, 0, &source);
                self.start(first);
            }
        }
    }

    /// The radio for the track at `row` of the open page, or for what plays
    /// when `row` is `None`.
    pub fn radio_for(&self, row: Option<usize>) -> Option<Activate> {
        let item = match row.and_then(|row| self.rows.get(row)) {
            Some(Row::Track { id, .. }) => self.track_item(self.radio_track(*id)?),
            Some(Row::Catalog { kind: "Track", uri, name, subtitle }) => QueueItem { uri: uri.clone(), name: name.clone(), artist: subtitle.clone() },
            Some(Row::Catalog { kind: "Artist", uri, name, .. }) => return Some(Activate::Radio { seed: uri.clone(), title: name.clone(), first: None }),
            Some(_) => return None,
            None => {
                let now = self.playback.now.as_ref()?;
                match self.index.id_of(&now.uri) {
                    Some(id) => self.track_item(self.radio_track(id)?),
                    None if now.uri.starts_with(LOCAL_FILE) => return None,
                    None => QueueItem { uri: now.uri.clone(), name: now.title.clone(), artist: now.artist.clone() },
                }
            }
        };
        Some(Activate::Radio { seed: item.uri.clone(), title: item.name.clone(), first: Some(item) })
    }

    /// The system track whose radio a library track has: itself, or for a
    /// local copy its original; `None` for a local file of no system track.
    fn radio_track(&self, id: u32) -> Option<u32> {
        let track = self.index.tracks.get(id as usize)?;
        match (track.local_path(), track.copy_of) {
            (None, _) => Some(id),
            (Some(_), original) => original,
        }
    }

    /// Next: our next track, played at once, whatever the system would
    /// have played. Without a queue steering, the system's own next.
    pub fn next(&mut self) {
        // A queue kept from the last run wakes here.
        if (self.queue.driving || self.queue.parked) && self.queue.upcoming_len() > 0 {
            let next = self.queue.advance();
            self.start(next);
        } else {
            self.transport(Transport::Next);
        }
    }

    /// Previous: the start of this track when it is under way, else our
    /// track before it. Without a queue steering, the system's own.
    pub fn previous(&mut self, now_secs: f64) {
        let under_way = self.playback.position_ms(now_secs).is_none_or(|position| position > RESTART_AFTER_MS);
        if !self.queue.driving && !self.queue.parked {
            self.transport(Transport::Previous);
        } else if self.queue.parked && self.queue.current.is_some() {
            // A queue kept from the last run: Previous starts its track.
            self.resume_parked();
        } else if under_way {
            self.transport(Transport::Seek(0));
        } else {
            let back = self.queue.previous();
            match back {
                Some(back) => self.start(Some(back)),
                None => self.transport(Transport::Seek(0)),
            }
        }
    }

    /// The system plays `uri` now. `ran_on`: it is what the system's own
    /// queue had next, so it went on by itself. Returns whether the queue
    /// changed.
    pub fn follow_player(&mut self, uri: &str, ran_on: bool) -> bool {
        match self.queue.observe(uri, ran_on) {
            Observed::Same | Observed::Ignored => return false,
            // A track ended before ours reached the system's queue: ours
            // plays instead of what the system picked.
            Observed::Overran => {
                let current = self.queue.current.clone();
                self.start(current);
            }
            Observed::Advanced | Observed::Back | Observed::Resumed => {
                if self.index.local_file(uri).is_some() {
                    let item = self.queue.current.clone();
                    self.start(item);
                }
                self.requests.queue = true;
                self.changed();
            }
            Observed::Detached => {
                self.requests.queue = true;
                self.changed();
            }
        }
        true
    }

    /// Play: a queue kept from the last run steers again from its track —
    /// resumed where the player has it paused, else started. `false` when
    /// there is no parked queue, and play means the player's own resume.
    pub fn resume_parked(&mut self) -> bool {
        if !self.queue.parked {
            return false;
        }
        // Only queued tracks were kept: the first of them plays.
        let Some(current) = self.queue.current.clone() else {
            if self.queue.upcoming_len() == 0 {
                return false;
            }
            let first = self.queue.advance();
            self.start(first);
            return true;
        };
        self.queue.drive();
        if self.engine == Engine::System && self.playback.uri() == Some(current.uri.as_str()) && self.system_ready() {
            self.transport(Transport::Resume);
            self.requests.queue = true;
            self.changed();
        } else {
            self.start(Some(current));
        }
        true
    }

    /// Play with nothing queued and no music system to resume: the open
    /// list plays from its first track, or the cached songs do. `false`
    /// when there is nothing to play.
    pub fn play_offline(&mut self) -> bool {
        if self.system_ready() && !crate::worker::local_only() {
            return false;
        }
        if self.queue.current.is_some() || self.queue.upcoming_len() > 0 {
            return false;
        }
        if self.shown_tracks().is_empty() {
            let Some(at) = self.index.entry(crate::cache::LIBRARY_URI) else { return false };
            self.open(Page::Playlist(at));
        }
        let Some((row, _)) = self.shown_tracks().into_iter().next() else { return false };
        self.run(Activate::PlayFrom(row));
        true
    }

    /// Play the upcoming track at `at` of the queue.
    pub fn jump(&mut self, at: usize) {
        let next = self.queue.jump(at);
        self.start(next);
    }

    /// Change the queue in place (a remove, a move, a clear).
    pub fn edit_queue(&mut self, edit: impl FnOnce(&mut PlayQueue)) {
        edit(&mut self.queue);
        self.requests.queue = true;
        self.changed();
    }

    /// What a row does when it is chosen (double-click, Return).
    pub fn activate(&self, row: usize) -> Option<Activate> {
        Some(match self.rows.get(row)? {
            Row::Track { .. } => Activate::PlayFrom(row),
            Row::Playlist(at) => Activate::Open(Page::Playlist(*at)),
            Row::Album { name, artists, uri } => Activate::Open(Page::Album(name.clone(), artists.clone(), uri.clone())),
            Row::Artist { name, .. } => Activate::Open(Page::Artist(name.clone())),
            Row::Catalog { kind: "Track", uri, name, subtitle } => Activate::PlayOne(QueueItem { uri: uri.clone(), name: name.clone(), artist: subtitle.clone() }),
            Row::Catalog { uri, .. } => Activate::PlayContext(uri.clone()),
            Row::Folder { path, .. } => Activate::Open(Page::Playlist(self.index.entry(&format!("{LOCAL_FOLDER}{path}"))?)),
            Row::Section(_) | Row::Note(_) => return None,
        })
    }

    /// What a row's menu offers, and what each does.
    pub fn row_commands(&self, row: usize) -> Vec<(RowCommand, Activate)> {
        let mut out = Vec::new();
        match self.rows.get(row) {
            Some(Row::Track { id, .. }) => {
                let track = &self.index.tracks[*id as usize];
                let item = self.track_item(*id);
                out.push((RowCommand::Play, Activate::PlayFrom(row)));
                out.push((RowCommand::PlayNext, Activate::PlayNext(item.clone())));
                out.push((RowCommand::Enqueue, Activate::Enqueue(vec![item.clone()])));
                if let Some(radio) = self.radio_for(Some(row)) {
                    out.push((RowCommand::SongRadio, radio));
                }
                let artist = track.artists.split(", ").next().filter(|a| !a.is_empty());
                if let Some(artist) = artist.filter(|_| !track.artist_uri.is_empty()) {
                    out.push((RowCommand::ArtistRadio, Activate::Radio { seed: track.artist_uri.clone(), title: artist.to_string(), first: None }));
                }
                if !track.album.is_empty() {
                    out.push((RowCommand::Album, Activate::Open(Page::Album(track.album.clone(), track.artists.clone(), track.album_uri.clone()))));
                }
                if let Some(artist) = artist {
                    out.push((RowCommand::Artist, Activate::Open(Page::Artist(artist.to_string()))));
                }
            }
            Some(Row::Catalog { kind, uri, name, subtitle }) => {
                if *kind == "Track" {
                    let item = QueueItem { uri: uri.clone(), name: name.clone(), artist: subtitle.clone() };
                    out.push((RowCommand::Play, Activate::PlayOne(item.clone())));
                    out.push((RowCommand::PlayNext, Activate::PlayNext(item.clone())));
                    out.push((RowCommand::Enqueue, Activate::Enqueue(vec![item.clone()])));
                    out.push((RowCommand::SongRadio, Activate::Radio { seed: uri.clone(), title: name.clone(), first: Some(item) }));
                } else {
                    out.push((RowCommand::Play, Activate::PlayContext(uri.clone())));
                    if *kind == "Artist" {
                        out.push((RowCommand::ArtistRadio, Activate::Radio { seed: uri.clone(), title: name.clone(), first: None }));
                    }
                }
            }
            Some(Row::Playlist(at)) => out.extend(self.playlist_commands(*at)),
            Some(Row::Album { name, artists, uri }) => {
                out.push((RowCommand::Open, Activate::Open(Page::Album(name.clone(), artists.clone(), uri.clone()))));
                if !uri.is_empty() {
                    out.push((RowCommand::Play, Activate::PlayContext(uri.clone())));
                }
            }
            Some(Row::Artist { name, .. }) => out.push((RowCommand::Open, Activate::Open(Page::Artist(name.clone())))),
            Some(Row::Folder { path, .. }) => {
                if let Some(open) = self.activate(row) {
                    out.push((RowCommand::Open, open));
                }
                out.push((RowCommand::RemoveFolder, Activate::RemoveFolder(path.clone())));
            }
            _ => {}
        }
        out
    }

    /// What a playlist offers: open it, play it (in order or shuffled),
    /// queue all of it.
    pub fn playlist_commands(&self, at: usize) -> Vec<(RowCommand, Activate)> {
        vec![
            (RowCommand::Open, Activate::Open(Page::Playlist(at))),
            (RowCommand::Play, Activate::PlayPlaylist { at, shuffle: false }),
            (RowCommand::ShufflePlay, Activate::PlayPlaylist { at, shuffle: true }),
            (RowCommand::QueueAll, Activate::QueuePlaylist(at)),
        ]
    }

    /// What the heading's menu offers for the open page: its tracks as
    /// shown, played (in order or shuffled) or queued.
    pub fn page_commands(&self) -> Vec<(RowCommand, Activate)> {
        let shown = self.shown_tracks();
        let Some((first, _)) = shown.first() else { return Vec::new() };
        vec![
            (RowCommand::Play, Activate::PlayFrom(*first)),
            (RowCommand::ShufflePlay, Activate::ShuffleShown),
            (RowCommand::QueueAll, Activate::Enqueue(shown.into_iter().map(|(_, item)| item).collect())),
        ]
    }

    /// Shuffle Play of the open page's tracks as shown (the page menu).
    pub fn shuffle_page(&mut self) {
        let mut items: Vec<QueueItem> = self.shown_tracks().into_iter().map(|(_, item)| item).collect();
        shuffled(&mut items);
        let source = self.page_source();
        let first = self.queue.play_list(items, 0, &source);
        self.start(first);
    }

    /// The rows of the open page, in the order the headings chose.
    pub fn rebuild(&mut self) {
        let index = self.index.clone();
        let tracks = |ids: &[u32]| ids.iter().enumerate().map(|(n, &id)| Row::Track { id, number: n + 1 }).collect::<Vec<_>>();
        self.rows = match &self.page {
            Page::Playlist(at) => index.entries.get(*at).map(|entry| tracks(&entry.tracks)).unwrap_or_default(),
            Page::Artist(name) => tracks(&index.by_artist(name)),
            Page::Album(name, artists, _) => tracks(&index.by_album(name, artists)),
            Page::Search => self.search_rows(),
            Page::Equalizer => Vec::new(),
            Page::Folders => self.folder_rows(),
        };
        if self.filter == Filter::Local && self.page != Page::Folders {
            self.rows.retain(|row| !matches!(row, Row::Track { id, .. } if !index.has_local(*id)));
        }
        if let Some(sort) = self.columns.sort() {
            sort_tracks(&index, &mut self.rows, sort);
        }
        if self.rows.is_empty() && self.page != Page::Equalizer {
            let note = match &self.page {
                Page::Search if self.query.trim().is_empty() => "Type to search every track in your playlists",
                Page::Search => "Nothing found",
                _ if self.filter == Filter::Local => "No local files here",
                Page::Playlist(_) => "No tracks read yet",
                _ => "Nothing here",
            };
            self.rows.push(Row::Note(note.into()));
        }
        self.changed();
    }

    /// The folders of music files, with how many files each gave.
    fn folder_rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Section("Music folders".into())];
        for path in &self.folders {
            let files = self.index.entry(&format!("{LOCAL_FOLDER}{path}")).map_or(0, |at| self.index.entries[at].tracks.len());
            rows.push(Row::Folder { path: path.clone(), files });
        }
        rows.push(Row::Note(if crate::worker::local_only() && self.folders.is_empty() {
            "Add a folder of music to play local files.".into()
        } else if self.folders.is_empty() {
            "Add a folder of music you own (MP3, Ogg Vorbis or FLAC): its files join your library, and copies of your playlists' tracks play here when Spotify is unavailable".into()
        } else {
            "Files matching a track of your playlists are its local copy; the rest are their own tracks".into()
        }));
        rows
    }

    fn search_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.query.trim().is_empty() {
            return rows;
        }
        let hits = &self.hits;
        if !hits.playlists.is_empty() {
            rows.push(Row::Section("Playlists".into()));
            rows.extend(hits.playlists.iter().map(|&at| Row::Playlist(at)));
        }
        if !hits.artists.is_empty() {
            rows.push(Row::Section("Artists in your library".into()));
            rows.extend(hits.artists.iter().take(8).map(|(name, tracks)| Row::Artist { name: name.clone(), tracks: *tracks }));
        }
        if !hits.albums.is_empty() {
            rows.push(Row::Section("Albums in your library".into()));
            rows.extend(hits.albums.iter().take(8).map(|(name, artists, uri)| Row::Album { name: name.clone(), artists: artists.clone(), uri: uri.clone() }));
        }
        if !hits.tracks.is_empty() {
            let shown = hits.tracks.len();
            let title = if hits.track_total > shown { format!("Tracks in your playlists ({} of {})", shown, hits.track_total) } else { format!("Tracks in your playlists ({})", hits.track_total) };
            rows.push(Row::Section(title));
            rows.extend(hits.tracks.iter().enumerate().map(|(n, &id)| Row::Track { id, number: n + 1 }));
        }
        // The catalog is the system's: nothing to ask while it is away.
        if !self.system_ready() {
            return rows;
        }
        let source = self.index.source.map(|s| s.name()).unwrap_or("catalog");
        match &self.catalog {
            None => rows.push(Row::Section(format!("{source} · searching…"))),
            Some(Err(error)) => {
                rows.push(Row::Section(source.to_string()));
                rows.push(Row::Note(format!("Search failed: {error}")));
            }
            Some(Ok(results)) => {
                let groups: [(&'static str, &Vec<crate::source::CatalogItem>); 4] =
                    [("Track", &results.tracks), ("Artist", &results.artists), ("Album", &results.albums), ("Playlist", &results.playlists)];
                if groups.iter().any(|(_, items)| !items.is_empty()) {
                    rows.push(Row::Section(format!("{source} catalog")));
                    for (kind, items) in groups {
                        rows.extend(items.iter().map(|item| Row::Catalog { kind, uri: item.uri.clone(), name: item.name.clone(), subtitle: item.subtitle.clone() }));
                    }
                }
            }
        }
        rows
    }
}

/// Shuffle in place with a small xorshift seeded by the clock: a play
/// order, not a secret.
fn shuffled<T>(items: &mut [T]) {
    let mut state = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
    for at in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(at, (state % (at as u64 + 1)) as usize);
    }
}

/// Sort each run of track rows (a search page has other groups around its
/// tracks, which stay where they are). Ties keep the list's own order.
fn sort_tracks(index: &Index, rows: &mut [Row], sort: GridSort<Column>) {
    let key = |row: &Row| track_key(index, row, sort.column);
    let mut start = 0;
    while start < rows.len() {
        if !matches!(rows[start], Row::Track { .. }) {
            start += 1;
            continue;
        }
        let end = rows[start..].iter().position(|row| !matches!(row, Row::Track { .. })).map_or(rows.len(), |len| start + len);
        rows[start..end].sort_by(|a, b| {
            let (a, a_number) = key(a);
            let (b, b_number) = key(b);
            columns::compare(&a, &b, sort.descending, a_number.cmp(&b_number))
        });
        start = end;
    }
}

/// What a track row sorts by in `column`, and its place in the list's own
/// order.
fn track_key<'a>(index: &'a Index, row: &Row, column: Column) -> (Key<'a>, usize) {
    let Row::Track { id, number } = row else { return (Key::Known(0), 0) };
    let track = &index.tracks[*id as usize];
    let (title, artists, album) = index.folded_fields(*id);
    let key = match column {
        Column::Number => Key::Known(*number as u64),
        Column::Title => Key::Text(title),
        Column::Artist => Key::Text(artists),
        Column::Album => Key::Text(album),
        Column::Released => Key::Known(track.release as u64),
        Column::Time => Key::Known(track.duration_ms as u64),
    };
    (key, *number)
}


/// `3:07`, or `1:02:07`.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 3600 {
        format!("{}:{:02}:{:02}", seconds / 3600, seconds % 3600 / 60, seconds % 60)
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

/// `4 hr 12 min`, or `37 min`.
pub fn long_duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes >= 60 {
        format!("{} hr {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(uri: &str, on: bool) -> Option<NowPlaying> {
        Some(NowPlaying { uri: uri.into(), is_playing: on, ..NowPlaying::default() })
    }

    #[test]
    fn a_track_first_seen_under_way_has_no_position() {
        let mut playback = Playback::default();
        playback.update(playing("t:1", true), None, 10.0, 1.0);
        assert_eq!(playback.position_ms(12.0), None);
    }

    #[test]
    fn a_change_seen_while_watching_starts_the_clock() {
        let mut playback = Playback::default();
        playback.update(playing("t:1", true), None, 10.0, 1.0);
        let detail = TrackDetail { uri: "t:2".into(), duration_ms: 200_000, ..TrackDetail::default() };
        playback.update(playing("t:2", true), Some(&detail), 11.0, 1.0);
        // Assumed to have started half a poll gap before it was seen.
        assert_eq!(playback.position_ms(11.0), Some(500.0));
        assert_eq!(playback.position_ms(13.0), Some(2500.0));
        // A pause holds the position.
        playback.update(playing("t:2", false), None, 13.0, 1.0);
        assert_eq!(playback.position_ms(20.0), Some(2500.0));
        // Past the end it waits at the end.
        playback.seek(199_000.0, 20.0);
        playback.update(playing("t:2", true), None, 20.0, 1.0);
        assert_eq!(playback.position_ms(30.0), Some(200_000.0));
    }

    #[test]
    fn a_track_that_starts_over_is_not_left_at_its_end() {
        let mut playback = Playback::default();
        playback.update(playing("t:1", true), None, 10.0, 1.0);
        let detail = TrackDetail { uri: "t:2".into(), duration_ms: 10_000, ..TrackDetail::default() };
        playback.update(playing("t:2", true), Some(&detail), 11.0, 1.0);
        // Previous (or Play of the same track) restarts the clock.
        playback.restart(15.0);
        assert_eq!(playback.position_ms(16.0), Some(1000.0));
        // Repeat one: the estimate runs out while the same track plays on.
        playback.update(playing("t:2", true), None, 30.0, 1.0);
        assert_eq!(playback.position_ms(31.0), None);
    }

    #[test]
    fn durations_read_like_a_player() {
        assert_eq!(duration(187_000), "3:07");
        assert_eq!(duration(3_727_000), "1:02:07");
        assert_eq!(long_duration(15_120_000), "4 hr 12 min");
    }

    fn playlist_model() -> Model {
        use crate::source::{LibraryEntry, Playlist, PlaylistTrack, SourceId};
        let mut index = Index::new(SourceId::Spotify);
        index.set_library(vec![LibraryEntry { uri: "p:1".into(), name: "Mix".into(), is_folder: false, depth: 1 }]);
        let track = |uri: &str, name: &str| PlaylistTrack { uri: uri.into(), name: name.into(), artists: vec!["A".into()], album: "B".into() };
        index.set_playlist(
            Playlist { uri: "p:1".into(), name: "Mix".into(), tracks: vec![track("t:1", "Beta"), track("t:2", "alpha"), track("t:3", "Gamma"), track("t:4", "Ämber")], ..Playlist::default() },
            100,
        );
        let mut model = Model::new(Theme::default());
        model.set_index(index.into());
        model.open(Page::Playlist(0));
        model
    }

    fn names(model: &Model) -> Vec<String> {
        model.rows.iter().filter_map(|row| if let Row::Track { id, .. } = row { Some(model.index.tracks[*id as usize].name.clone()) } else { None }).collect()
    }

    fn row_of(model: &Model, name: &str) -> usize {
        model.rows.iter().position(|row| matches!(row, Row::Track { id, .. } if model.index.tracks[*id as usize].name == name)).unwrap()
    }

    fn upcoming(model: &Model) -> Vec<String> {
        model.queue.upcoming().map(|(_, item)| item.name.clone()).collect()
    }

    fn played(model: &mut Model) -> Vec<String> {
        std::mem::take(&mut model.requests.transport).into_iter().map(|t| match t {
            Transport::Play(uri) => uri,
            other => format!("{other:?}"),
        }).collect()
    }

    /// A heading sorts rising, falling, then back to the playlist's order;
    /// text sorts folded (Ämber with the a's).
    #[test]
    fn headings_sort_every_way() {
        let mut model = playlist_model();
        assert_eq!(names(&model), ["Beta", "alpha", "Gamma", "Ämber"]);
        model.press_heading(Column::Title);
        assert_eq!(names(&model), ["alpha", "Ämber", "Beta", "Gamma"]);
        model.press_heading(Column::Title);
        assert_eq!(names(&model), ["Gamma", "Beta", "Ämber", "alpha"]);
        model.press_heading(Column::Title);
        assert_eq!(names(&model), ["Beta", "alpha", "Gamma", "Ämber"]);
        // # falling: the playlist backwards, newest additions first.
        model.press_heading(Column::Number);
        model.press_heading(Column::Number);
        assert_eq!(names(&model), ["Ämber", "Gamma", "alpha", "Beta"]);
        assert!(model.requests.columns, "a sort is saved with the layout");
    }

    /// Playing a row of a list sorted by # falling plays on in that order,
    /// and Next plays our next track itself rather than the system's next.
    #[test]
    fn next_plays_the_next_row_of_the_list_as_shown() {
        let mut model = playlist_model();
        model.press_heading(Column::Number);
        model.press_heading(Column::Number);
        let row = row_of(&model, "Gamma");
        model.run(Activate::PlayFrom(row));
        assert_eq!(played(&mut model), ["t:3"]);
        assert_eq!(upcoming(&model), ["alpha", "Beta"]);
        assert_eq!(model.queue.source, "Mix");
        assert!(model.requests.queue);
        model.next();
        assert_eq!(played(&mut model), ["t:2"]);
        assert_eq!(model.handed_over().unwrap().1, ["t:1"]);
        // The system confirms the track: nothing more to do.
        assert!(!model.follow_player("t:2", false));
        model.next();
        assert_eq!(played(&mut model), ["t:1"]);
        // Past the end, Next is the system's own.
        model.next();
        assert_eq!(played(&mut model), ["Next"]);
    }

    /// A track ended and the system ran on into its own next before ours
    /// reached its queue: ours plays instead.
    #[test]
    fn a_player_that_runs_on_by_itself_is_put_back_on_our_list() {
        let mut model = playlist_model();
        model.run(Activate::PlayFrom(row_of(&model, "Beta")));
        played(&mut model);
        assert!(model.follow_player("spotify:autoplay", true));
        assert_eq!(played(&mut model), ["t:2"]);
        assert_eq!(model.queue.current.as_ref().unwrap().name, "alpha");
        // Something the person chose in the system itself lets go.
        assert!(model.follow_player("elsewhere", false));
        assert!(!model.queue.driving);
        assert!(played(&mut model).is_empty());
    }

    /// A queue kept from the last run comes back parked: it hands the
    /// system nothing and follows nothing the system plays, until Play or
    /// Next wakes it. A track read back keeps its saved name even when the
    /// library no longer has it; a local file that is gone cannot play and
    /// is skipped.
    #[test]
    fn a_restored_queue_steers_nothing_until_played() {
        let mut model = playlist_model();
        model.run(Activate::PlayFrom(row_of(&model, "Beta")));
        model.queue.enqueue(vec![QueueItem { uri: format!("{LOCAL_FILE}/gone/mine.ogg"), name: "Mine".into(), artist: "Me".into() }]);
        let saved = model.queue.serialize();

        let mut model = playlist_model();
        model.queue = PlayQueue::parse(&saved).unwrap();
        assert!(model.handed_over().is_none(), "a parked queue fills nothing");
        assert!(!model.follow_player("elsewhere", false));
        assert!(model.queue.parked);
        assert!(played(&mut model).is_empty(), "restoring plays nothing");
        assert_eq!(upcoming(&model), ["Mine", "alpha", "Gamma", "Ämber"]);
        assert!(!model.can_play(&model.queue.user[0].uri), "the gone file reads unavailable");
        // Play starts our track and steers again.
        assert!(model.resume_parked());
        assert_eq!(played(&mut model), ["t:1"]);
        assert!(model.queue.driving && !model.queue.parked);
        // Handed over up to the local file, which this app would play.
        assert!(model.handed_over().unwrap().1.is_empty());

        // Next on a parked queue plays our next; the gone file is skipped.
        let mut model = playlist_model();
        model.queue = PlayQueue::parse(&saved).unwrap();
        model.next();
        assert_eq!(played(&mut model), ["t:2"]);
        assert!(model.error.as_ref().is_some_and(|error| error.contains("Mine")));

        // Kept with only queued tracks, Play starts the first of them.
        let mut model = playlist_model();
        model.queue.enqueue(vec![QueueItem { uri: "t:3".into(), name: "Gamma".into(), artist: "A".into() }]);
        let mut model2 = playlist_model();
        model2.queue = PlayQueue::parse(&model.queue.serialize()).unwrap();
        assert!(model2.resume_parked());
        assert_eq!(played(&mut model2), ["t:3"]);
        assert!(model2.queue.driving);
    }

    /// Queued tracks play before the rest of the list; queuing while the
    /// queue has let go steers again from what plays now.
    #[test]
    fn queuing_steers_from_what_plays() {
        let mut model = playlist_model();
        model.playback.now = Some(NowPlaying { uri: "t:x".into(), title: "Elsewhere".into(), is_playing: true, ..NowPlaying::default() });
        model.run(Activate::Enqueue(vec![QueueItem { uri: "t:4".into(), name: "Ämber".into(), artist: "A".into() }]));
        assert!(model.queue.driving);
        assert_eq!(model.queue.current.as_ref().unwrap().name, "Elsewhere");
        assert_eq!(model.handed_over().unwrap().1, ["t:4"]);
        assert!(played(&mut model).is_empty(), "queuing plays nothing");
    }

    /// A song radio plays its seed at once and fills the queue behind it
    /// when the station answers, without the seed twice; a late answer for
    /// an older radio is dropped.
    #[test]
    fn a_song_radio_plays_the_seed_and_fills_the_queue() {
        let mut model = playlist_model();
        let radio = model.radio_for(Some(row_of(&model, "Beta"))).unwrap();
        model.run(radio);
        assert_eq!(played(&mut model), ["t:1"]);
        assert_eq!(model.requests.radio, Some(("t:1".to_string(), "Beta".to_string())));
        let station = |uris: &[&str]| uris.iter().map(|uri| QueueItem { uri: uri.to_string(), name: uri.to_string(), artist: String::new() }).collect::<Vec<_>>();
        model.set_radio(model.radio_seq - 1, "Beta", station(&["old"]));
        assert_eq!(model.queue.upcoming_len(), 0);
        model.set_radio(model.radio_seq, "Beta", station(&["s:1", "t:1", "s:2"]));
        assert_eq!(upcoming(&model), ["s:1", "s:2"]);
        assert_eq!(model.queue.source, "Radio: Beta");
        assert!(played(&mut model).is_empty(), "the seed keeps playing");
    }

    #[test]
    fn previous_restarts_a_track_under_way_else_goes_back() {
        let mut model = playlist_model();
        model.run(Activate::PlayFrom(0));
        model.next();
        played(&mut model);
        // Under way (or not known): back to its start.
        model.previous(10.0);
        assert_eq!(played(&mut model), ["Seek(0)"]);
        // Just started: the track before.
        model.playback.update(Some(NowPlaying { uri: "t:1".into(), is_playing: true, ..NowPlaying::default() }), None, 1.0, 1.0);
        model.playback.update(Some(NowPlaying { uri: "t:2".into(), is_playing: true, ..NowPlaying::default() }), None, 2.0, 0.0);
        model.previous(2.5);
        assert_eq!(played(&mut model), ["t:1"]);
        assert_eq!(upcoming(&model)[0], "alpha");
    }

    /// A library of two system tracks, one with a local copy, and a local
    /// file of its own, laid side by side.
    fn mixed_model() -> Model {
        use crate::source::{LibraryEntry, Playlist, PlaylistTrack, SourceId};
        let mut system = Index::new(SourceId::Spotify);
        system.set_library(vec![LibraryEntry { uri: "p:1".into(), name: "Mix".into(), is_folder: false, depth: 1 }]);
        let track = |uri: &str, name: &str| PlaylistTrack { uri: uri.into(), name: name.into(), artists: vec!["A".into()], album: "B".into() };
        system.set_playlist(Playlist { uri: "p:1".into(), name: "Mix".into(), tracks: vec![track("s:1", "Kept"), track("s:2", "Streamed")], ..Playlist::default() }, 1);
        let mut local = Index::new(SourceId::Local);
        local.set_library(vec![LibraryEntry { uri: "local:folder:/m".into(), name: "m".into(), is_folder: false, depth: 2 }]);
        local.set_playlist(
            Playlist { uri: "local:folder:/m".into(), name: "m".into(), tracks: vec![track("file:///m/kept.ogg", "Kept"), track("file:///m/mine.ogg", "Mine")], ..Playlist::default() },
            1,
        );
        let mut model = Model::new(Theme::default());
        model.set_index(Index::merged(&system, &local).into());
        model.open(Page::Playlist(0));
        model
    }

    fn item(uri: &str, name: &str) -> QueueItem {
        QueueItem { uri: uri.into(), name: name.into(), artist: "A".into() }
    }

    fn player_plays(model: &mut Model) -> Vec<String> {
        std::mem::take(&mut model.requests.player).into_iter().map(|request| match request {
            PlayerRequest::Play { path, .. } => path,
            other => format!("{other:?}"),
        }).collect()
    }

    /// A play of a system track shows at once (optimistic): the system
    /// still reporting the old track within the grace time is held back,
    /// reporting the new one confirms it from 0:00, and nothing for three
    /// seconds brings back what it really plays, with an error.
    #[test]
    fn a_system_play_shows_before_the_system_confirms() {
        let mut model = mixed_model();
        model.playback.now = Some(NowPlaying { uri: "s:1".into(), title: "Kept".into(), artist: "A".into(), context: String::new(), is_playing: true });
        model.run(Activate::PlayOne(item("s:2", "Streamed")));
        assert_eq!(model.playback.uri(), Some("s:2"), "shown on the click");
        assert_eq!(model.requests.transport, [Transport::Play("s:2".into())]);
        model.pending_sent("s:2", 10.0);
        assert_eq!(model.playback.position_ms(10.0), Some(0.0));
        assert!(model.reconcile(Some("s:1"), 10.7, 1.0, false), "still switching: held");
        assert_eq!(model.playback.uri(), Some("s:2"));
        assert!(!model.reconcile(Some("s:2"), 11.0, 1.0, false), "confirmed");
        assert!(model.pending.is_none() && model.error.is_none());
        assert_eq!(model.playback.uri(), Some("s:2"));

        // Another play the system never takes: back to what it plays.
        model.run(Activate::PlayOne(item("s:3", "Another streamed song")));
        model.pending_sent("s:3", 20.0);
        assert_eq!(model.playback.uri(), Some("s:3"));
        assert!(model.reconcile(Some("s:2"), 21.0, 1.0, false));
        assert!(!model.reconcile(Some("s:2"), 23.5, 1.0, false), "timed out");
        assert_eq!(model.playback.uri(), Some("s:2"), "rolled back");
        assert!(model.pending.is_none() && model.error.is_some());
        assert_eq!(model.queue.current.as_ref().map(|c| c.uri.as_str()), Some("s:2"), "the queue follows the system");
    }

    /// Cast to a speaker, the model says where it plays and which device
    /// brings it back here; a local file playing, or this machine playing,
    /// is not remote.
    #[test]
    fn a_cast_is_seen_and_this_machine_is_the_way_back() {
        let mut model = mixed_model();
        let mac = Device { id: "a1".into(), name: "Mac".into(), kind: "computer".into(), volume: 1.0, active: false, this_machine: true };
        let speaker = Device { id: "b2".into(), name: "Kitchen speaker".into(), kind: "speaker".into(), volume: 0.4, active: true, this_machine: false };
        model.devices = vec![mac.clone(), speaker.clone()];
        model.engine = Engine::System;
        assert_eq!(model.remote_device().map(|d| d.name.as_str()), Some("Kitchen speaker"));
        assert_eq!(model.this_machine_device().map(|d| d.id.as_str()), Some("a1"));
        model.engine = Engine::Local;
        assert!(model.remote_device().is_none());
        model.engine = Engine::System;
        model.devices = vec![Device { active: true, ..mac }, Device { active: false, ..speaker }];
        assert!(model.remote_device().is_none());
    }

    /// The volume slider drives our own stage while a local file plays, and
    /// the system's volume otherwise, routed through the equalizer or not;
    /// switching never touches the system's volume.
    #[test]
    fn the_volume_slider_follows_where_the_sound_runs() {
        let mut model = mixed_model();
        model.devices = vec![Device { id: "d".into(), name: "Mac".into(), kind: String::new(), volume: 0.8, active: true, this_machine: true }];
        // The system plays by itself: its own volume, told as the dial
        // turns (the worker runs only the last of a burst).
        assert_eq!(model.volume_shown(), Some(0.8f32 as f64));
        model.set_volume(0.3, false);
        assert_eq!(std::mem::take(&mut model.requests.transport), [Transport::Volume(0.3)], "told while dragging");
        model.set_volume(0.3, true);
        assert_eq!(std::mem::take(&mut model.requests.transport), [Transport::Volume(0.3)]);
        assert!(!model.requests.knobs);
        // The equalizer routes it: still the system's volume, so the level
        // routed is the level unrouted at every slider position.
        model.audio = AudioState::Routed { device: "Mac".into(), sample_rate: 48_000.0 };
        assert!(!model.own_volume());
        assert_eq!(model.volume_shown(), Some(0.3f32 as f64));
        model.set_volume(0.4, true);
        assert_eq!(std::mem::take(&mut model.requests.transport), [Transport::Volume(0.4)]);
        assert!(!model.requests.knobs);
        assert_eq!(model.eq.volume, 1.0);
        // A local file plays: ours, live, kept when let go.
        model.audio = AudioState::Off;
        model.run(Activate::PlayOne(item("file:///m/mine.ogg", "Mine")));
        assert!(model.own_volume());
        assert_eq!(model.volume_shown(), Some(1.0));
        model.set_volume(0.5, false);
        assert!(model.requests.knobs && !model.requests.save_eq);
        model.set_volume(0.5, true);
        assert!(model.requests.save_eq);
        assert!(model.requests.transport.is_empty(), "the system's volume is left alone");
        assert_eq!(model.eq.volume, 0.5);
        assert_eq!(model.devices[0].volume, 0.4);
    }

    /// While the system is away a track with a local copy plays here, one
    /// without is skipped with a note for the next that can play, and a
    /// local file plays here whether or not the system is there.
    #[test]
    fn away_the_local_copies_play_and_the_rest_is_skipped() {
        let mut model = mixed_model();
        model.set_availability(Availability::Away("not running".into()));
        let kept = model.index.id_of("s:1").unwrap();
        assert!(model.playable(kept) && !model.can_play("s:2"));
        assert_eq!(names(&model), ["Kept"], "the offline library is pruned");
        // An uncached item retained in a queue still skips safely.
        model.run(Activate::PlayOne(item("s:2", "Streamed")));
        assert!(player_plays(&mut model).is_empty());
        assert!(model.error.as_ref().is_some_and(|note| note.contains("Streamed")));
        let mut model = mixed_model();
        model.run(Activate::PlayFrom(0));
        model.set_availability(Availability::Away("not running".into()));
        assert_eq!(player_plays(&mut model), ["/m/kept.ogg"]);
        assert_eq!(model.engine, Engine::Local);
        // Its end moves on; the next cannot play and there is nothing after it.
        model.local_ended(model.local_generation);
        assert!(model.error.as_ref().is_some_and(|note| note.contains("Streamed")));
        assert!(player_plays(&mut model).is_empty());
        assert!(model.requests.transport.is_empty(), "nothing goes to a system that is away");
        assert!(model.handed_over().is_none());
    }

    /// No music system (a phone): Amp opens on the cached songs, once, and
    /// Play with nothing queued plays them from the top.
    #[test]
    fn away_amp_opens_on_the_cached_songs_and_play_plays_them() {
        use crate::source::SourceId;
        let song = |uri: &str, title: &str| crate::cache::Song { uri: uri.into(), title: title.into(), artist: "A".into(), audio: format!("songs/{title}.ogg"), duration_ms: 1000, ..Default::default() };
        let cached = crate::cache::Library { root: "/off".into(), songs: vec![song("c:1", "one"), song("c:2", "two")] };
        let index = Index::merged(&Index::new(SourceId::Spotify), &Index::new(SourceId::Local)).with_cache(&cached);
        let mut model = Model::new(Theme::default());
        model.set_index(index.into());
        assert_eq!(model.page, Page::Search, "the system may still come: no jump yet");
        model.set_availability(Availability::Away("Spotify isn't installed".into()));
        let Page::Playlist(at) = model.page else { panic!("not on a list: {:?}", model.page) };
        assert_eq!(model.index.entries[at].uri, crate::cache::LIBRARY_URI);
        assert!(model.play_offline());
        assert_eq!(player_plays(&mut model), ["/off/songs/one.ogg"]);
        // Only once: back on the search page, a new snapshot leaves it there.
        model.open(Page::Search);
        model.set_index(Index::merged(&Index::new(SourceId::Spotify), &Index::new(SourceId::Local)).with_cache(&cached).into());
        assert_eq!(model.page, Page::Search);
    }

    /// A queue mixing the system's tracks and local files: the system holds
    /// ours only up to the first local file; the file plays here, the
    /// system's player paused; when it ends the system plays the next.
    #[test]
    fn a_mixed_queue_plays_each_track_where_it_can() {
        let mut model = mixed_model();
        model.set_availability(Availability::Ready);
        let first = model.queue.play_list(vec![item("s:2", "Streamed"), item("s:9", "Other"), item("file:///m/mine.ogg", "Mine"), item("s:1", "Kept")], 0, "Mix");
        model.start(first);
        assert_eq!(played(&mut model), ["s:2"]);
        assert_eq!(model.handed_over(), Some(("s:2".to_string(), vec!["s:9".to_string()])), "held up to the local file");
        model.playback.now = Some(NowPlaying { uri: "s:9".into(), is_playing: true, ..NowPlaying::default() });
        model.follow_player("s:9", true);
        model.next();
        assert_eq!(played(&mut model), ["Pause"], "the system's player pauses for the file");
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"]);
        assert!(model.handed_over().is_none());
        // Pause and seek go to this app's player now.
        model.transport(Transport::Pause);
        model.transport(Transport::Seek(5000));
        assert_eq!(player_plays(&mut model), ["Pause", "Seek(5000)"]);
        // A stale end (an older play) changes nothing; this play's end moves on.
        model.local_ended(model.local_generation - 1);
        assert!(played(&mut model).is_empty());
        model.local_ended(model.local_generation);
        assert!(played(&mut model).is_empty(), "the available local file takes priority");
        assert_eq!(player_plays(&mut model), ["/m/kept.ogg"]);
        assert_eq!(model.engine, Engine::Local);
    }

    /// The system's player started again while a file played here: the
    /// file stops and the system is followed.
    #[test]
    fn the_system_can_take_over_from_a_local_file() {
        let mut model = mixed_model();
        model.run(Activate::PlayOne(item("file:///m/mine.ogg", "Mine")));
        player_plays(&mut model);
        model.system_took_over();
        assert_eq!(player_plays(&mut model), ["Stop"]);
        assert_eq!(model.engine, Engine::System);
    }

    /// "Available Locally" keeps the tracks there is a file of.
    #[test]
    fn the_local_filter_keeps_what_has_a_file() {
        let mut model = mixed_model();
        assert_eq!(names(&model), ["Kept", "Streamed"]);
        model.set_filter(Filter::Local);
        assert_eq!(names(&model), ["Kept"]);
        model.open(Page::Playlist(1));
        assert_eq!(names(&model), ["Kept", "Mine"], "the local folder's own files");
        model.set_filter(Filter::All);
        model.open(Page::Playlist(0));
        assert_eq!(names(&model), ["Kept", "Streamed"]);
    }

    /// A local file that played out with nothing after it closes the
    /// output; Play starts it again rather than resuming silence.
    #[test]
    fn play_after_the_end_starts_the_file_again() {
        let mut model = mixed_model();
        model.run(Activate::PlayOne(item("file:///m/mine.ogg", "Mine")));
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"]);
        model.local_ended(model.local_generation);
        assert_eq!(player_plays(&mut model), ["Stop"]);
        model.transport(Transport::Resume);
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"]);
        assert!(!model.local_done);
    }

    /// A file that cannot be read or decoded moves on even under repeat
    /// one — restarting it would fail again for ever; with nothing
    /// playable after it, playback stops.
    #[test]
    fn a_broken_file_is_never_played_again_in_a_loop() {
        let mut model = mixed_model();
        model.repeat = Repeat::Track;
        let first = model.queue.play_list(vec![item("file:///m/mine.ogg", "Mine"), item("file:///m/kept.ogg", "Kept")], 0, "Mix");
        model.start(first);
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"]);
        model.local_failed(model.local_generation);
        assert_eq!(player_plays(&mut model), ["/m/kept.ogg"], "on to the next, not the same again");
        model.local_failed(model.local_generation);
        assert_eq!(player_plays(&mut model), ["Stop"], "nothing left: it stops");
        model.local_failed(model.local_generation);
        assert!(player_plays(&mut model).is_empty(), "and stays stopped");
        // A file that played out still repeats.
        model.run(Activate::PlayOne(item("file:///m/mine.ogg", "Mine")));
        player_plays(&mut model);
        model.local_ended(model.local_generation);
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"]);
    }

    /// With the system away, every track of the queue that cannot play is
    /// skipped, however many, until one with a local file.
    #[test]
    fn a_run_of_unplayable_tracks_is_skipped_to_a_local_file() {
        let mut model = mixed_model();
        model.set_availability(Availability::Away("gone".into()));
        let first = model.queue.play_list(vec![item("s:7", "A"), item("s:8", "B"), item("s:9", "C"), item("file:///m/mine.ogg", "Mine")], 0, "Mix");
        model.start(first);
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"]);
        assert_eq!(model.queue.current.as_ref().unwrap().name, "Mine");
    }
}
