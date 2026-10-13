//! The music systems this machine has, behind one interface.
//!
//! A [`MusicSource`] is a music system the app can read a library from and
//! remote-control: which playlists there are, what is in them, what is
//! playing, and the transport. Sources are found at startup by [`detect`];
//! each one is driven from the worker thread only (a call may take a few
//! hundred milliseconds), never from the UI thread.
//!
//! Spotify is the one remote-controlled system today; another (Apple Music)
//! is another implementation of the trait plus a line in [`detect`]. Local
//! files are a source of their own kind: the app reads them itself
//! (`local.rs`) and plays them itself (`player.rs`), so they need none of
//! this trait's remote control.

pub mod spotify;
#[cfg(target_os = "linux")]
mod cdp;
#[cfg(target_os = "linux")]
mod dbus;
#[cfg(target_os = "linux")]
pub mod spotify_cdp;
#[cfg(target_os = "linux")]
pub mod spotify_linux;

/// Which music system a source is. Stable: it names the index file on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceId {
    Spotify,
    /// Music files the person owns, in folders they added.
    Local,
}

impl SourceId {
    pub fn key(self) -> &'static str {
        match self {
            SourceId::Spotify => "spotify",
            SourceId::Local => "local",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SourceId::Spotify => "Spotify",
            SourceId::Local => "Local Files",
        }
    }
}

/// One row of the library's playlist tree, in the order the source shows
/// it: folders open a level, `depth` places each row under its folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryEntry {
    pub uri: String,
    pub name: String,
    pub is_folder: bool,
    /// 1 at the top level.
    pub depth: u8,
}

/// A saved album or followed artist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedItem {
    pub uri: String,
    pub name: String,
    /// The artists of an album; empty for an artist.
    pub artists: String,
}

/// A track as a playlist lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaylistTrack {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
}

/// A playlist with every track in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playlist {
    pub uri: String,
    pub name: String,
    pub owner: String,
    pub description: String,
    /// What the source says the playlist holds; the listing can be shorter
    /// (tracks no longer available are left out).
    pub track_count: u32,
    pub tracks: Vec<PlaylistTrack>,
}

/// What a lookup adds to a track: its length, album art, album and
/// release date.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackDetail {
    pub uri: String,
    pub duration_ms: u32,
    pub art_url: String,
    pub album: String,
    pub album_uri: String,
    /// `yyyymmdd` with the parts not given as 0 (see [`release_key`]); 0
    /// when unknown.
    pub release: u32,
    /// The first artist's URI.
    pub artist_uri: String,
}

/// A release date as a sortable key: "1995" is 19950000, "1997-1-1" is
/// 19970101. 0 for anything that is not a date.
pub fn release_key(text: &str) -> u32 {
    let mut parts = text.trim().splitn(3, '-').map(|part| part.trim().parse::<u32>().ok());
    let Some(Some(year)) = parts.next() else { return 0 };
    if !(1000..=9999).contains(&year) {
        return 0;
    }
    let month = parts.next().flatten().filter(|m| (1..=12).contains(m)).unwrap_or(0);
    let day = if month == 0 { 0 } else { parts.next().flatten().filter(|d| (1..=31).contains(d)).unwrap_or(0) };
    year * 10_000 + month * 100 + day
}

/// What is playing now. Sources that do not report the position leave the
/// app to estimate it (see `model::Playback`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub uri: String,
    pub title: String,
    pub artist: String,
    /// The playlist or album playback runs through, in words.
    pub context: String,
    pub is_playing: bool,
}

/// A track in the playback queue.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueEntry {
    pub uri: String,
    pub name: String,
    pub artist: String,
    /// Added to the queue (by this app or anyone), as opposed to what the
    /// playlist or album playing brings next. Queued tracks come first.
    pub queued: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// 0..1.
    pub volume: f32,
    pub active: bool,
    /// The player on this machine (Spotify.app here), whose sound Amp can
    /// tap; a speaker, a phone or another computer is not.
    pub this_machine: bool,
}

/// One hit of a catalog search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogItem {
    pub uri: String,
    pub name: String,
    /// Artist, author or owner.
    pub subtitle: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogResults {
    pub tracks: Vec<CatalogItem>,
    pub artists: Vec<CatalogItem>,
    pub albums: Vec<CatalogItem>,
    pub playlists: Vec<CatalogItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Off,
    Context,
    Track,
}

/// A transport command.
#[derive(Clone, Debug, PartialEq)]
pub enum Transport {
    /// Play a track, playlist, album or artist.
    Play(String),
    Resume,
    Pause,
    Next,
    Previous,
    /// Absolute position in milliseconds.
    Seek(u32),
    Shuffle(bool),
    Repeat(Repeat),
    /// 0..1, on the active device.
    Volume(f32),
    /// Move playback to the device with this id.
    Transfer(String),
    /// Append a track to the queue.
    Enqueue(String),
    /// Take the track at this position (0-based, among what plays next)
    /// out of the queue.
    Dequeue(usize),
}

/// A music system. Every call may block for a while; only the worker
/// thread calls these.
pub trait MusicSource: Send {
    fn id(&self) -> SourceId;
    /// The app that plays this system's sound, by bundle id, when it is
    /// another process: the equalizer taps that process's output. `None`
    /// for a system that plays in this app.
    fn player_bundle_id(&self) -> Option<&'static str> {
        None
    }
    /// Whether the system is running and signed in.
    fn ready(&mut self) -> Result<bool, String>;
    /// The playlist tree, folders included, in the system's own order.
    fn library(&mut self) -> Result<Vec<LibraryEntry>, String>;
    fn saved_albums(&mut self) -> Result<Vec<SavedItem>, String>;
    fn saved_artists(&mut self) -> Result<Vec<SavedItem>, String>;
    fn playlist(&mut self, uri: &str) -> Result<Playlist, String>;
    /// Length, art and album for tracks; at most [`LOOKUP_BATCH`] a call.
    fn lookup(&mut self, uris: &[String]) -> Result<Vec<TrackDetail>, String>;
    fn search(&mut self, query: &str) -> Result<CatalogResults, String>;
    /// What is playing and what is queued after it.
    fn playback(&mut self) -> Result<(Option<NowPlaying>, Vec<QueueEntry>), String>;
    fn devices(&mut self) -> Result<Vec<Device>, String>;
    fn transport(&mut self, command: &Transport) -> Result<(), String>;
    /// The radio of a track or artist (by URI): tracks like it, without
    /// playing anything. `None` when this system has no radio for it.
    fn radio(&mut self, _seed: &str) -> Option<Result<Vec<PlaylistTrack>, String>> {
        None
    }
}

/// Tracks one [`MusicSource::lookup`] call takes at most.
pub const LOOKUP_BATCH: usize = 200;

/// The music systems installed here.
pub fn detect() -> Vec<Box<dyn MusicSource>> {
    let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();
    if let Some(spotify) = spotify::Spotify::find() {
        sources.push(Box::new(spotify));
    }
    // The Linux client has no CLI: it is driven through its own page (the
    // DevTools protocol), and over MPRIS while that cannot be reached.
    #[cfg(target_os = "linux")]
    if sources.is_empty() {
        if let Some(spotify) = spotify_cdp::find() {
            sources.push(Box::new(spotify));
        }
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::release_key;

    #[test]
    fn release_dates_of_any_precision_become_keys() {
        assert_eq!(release_key("1995"), 19_950_000);
        assert_eq!(release_key("1997-1-1"), 19_970_101);
        assert_eq!(release_key("2014-03"), 20_140_300);
        assert_eq!(release_key(" 2001-12-31 "), 20_011_231);
        // A month that is not one drops the rest; junk is unknown.
        assert_eq!(release_key("2001-13-02"), 20_010_000);
        assert_eq!(release_key(""), 0);
        assert_eq!(release_key("soon"), 0);
        assert_eq!(release_key("95"), 0);
    }
}
