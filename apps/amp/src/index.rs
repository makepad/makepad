//! The library index: every playlist and every track in them, read out of a
//! music system once and kept on disk, so the sidebar and search work the
//! moment the app opens and search never waits on the system.
//!
//! The library worker builds it and hands the UI immutable snapshots
//! (`Arc<Index>`); the UI searches a snapshot on every keystroke. A track
//! that sits in several playlists is one track here, which remembers the
//! playlists it is in.
//!
//! Search folds case and diacritics, splits the query into words and keeps
//! the tracks in which every word matches a title, artist or album: at the
//! start of a word first, anywhere second. The title outranks the artist and
//! the artist the album.

use crate::source::{LibraryEntry, Playlist, SavedItem, SourceId, TrackDetail};
use makepad_search::fold_words as fold;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// First line of the file; a file that does not start with one of these is
/// ignored. Version 2 added each track's release date (and per playlist a
/// date added the index kept itself, no longer read); version 3 adds each
/// track's first artist. The tracks of an older file are looked up again
/// for what their version did not have.
const MAGIC: &str = "makepad-music-index 3";
const MAGIC_V2: &str = "makepad-music-index 2";
const MAGIC_V1: &str = "makepad-music-index 1";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub uri: String,
    pub name: String,
    /// Joined with ", ".
    pub artists: String,
    pub album: String,
    pub album_uri: String,
    /// 0 until looked up.
    pub duration_ms: u32,
    /// The smallest album art the system offers; see `art::sized`.
    pub art_url: String,
    /// `yyyymmdd`, parts not known 0; 0 when not known at all.
    pub release: u32,
    /// The first artist's URI, for its radio; empty until looked up.
    pub artist_uri: String,
    /// A lookup ran for this track (whether or not it found anything), so
    /// the refresh does not ask again.
    pub looked_up: bool,
    /// The tempo a local file's tags claim, 0 when none (not stored in the
    /// index file: the local scan supplies it every time).
    pub bpm: f32,
    /// The playlists (indices into [`Index::entries`]) holding the track.
    pub playlists: Vec<u32>,
    /// A local file that is a copy of this track (an index into
    /// [`Index::tracks`] of a merged index, see [`Index::merged`]).
    pub local_copy: Option<u32>,
    /// For a local file: the music system's track it is a copy of. Search
    /// and the album and artist pages show that track (with its local mark)
    /// instead of the file twice.
    pub copy_of: Option<u32>,
    /// A complete recording, resolved against the portable offline directory.
    pub cached_path: Option<String>,
}

impl Track {
    /// The file a local track plays from.
    pub fn local_path(&self) -> Option<&str> {
        self.uri.strip_prefix(LOCAL_FILE)
    }
}

/// How the URI of a local file starts; the rest is its path.
pub const LOCAL_FILE: &str = "file://";

/// A row of the playlist tree: a folder or a playlist and its tracks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub uri: String,
    pub name: String,
    pub owner: String,
    pub is_folder: bool,
    pub depth: u8,
    /// What the system says the playlist holds.
    pub track_count: u32,
    /// The listed tracks in playlist order (indices into [`Index::tracks`]).
    pub tracks: Vec<u32>,
    /// When the tracks were last read, seconds since the epoch; 0 = never.
    pub read_secs: u64,
}

/// What a search found.
#[derive(Clone, Debug, Default)]
pub struct Hits {
    /// Track indices, best first; at most the limit asked for.
    pub tracks: Vec<u32>,
    /// How many tracks matched in all.
    pub track_total: usize,
    /// Playlist entries whose name matches.
    pub playlists: Vec<usize>,
    /// Albums in the library whose name matches: (album, artists, album uri).
    pub albums: Vec<(String, String, String)>,
    /// Artists in the library whose name matches, with their track counts.
    pub artists: Vec<(String, usize)>,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub source: Option<SourceId>,
    pub entries: Vec<Entry>,
    pub tracks: Vec<Track>,
    pub albums: Vec<SavedItem>,
    pub artists: Vec<SavedItem>,
    by_uri: HashMap<String, u32>,
    /// Per track, "title\u{1}artists\u{1}album" folded: one string, so a
    /// word that is nowhere in it is rejected with one scan.
    folded: Vec<String>,
    /// Every album in the library once: (name, artists, uri, folded name).
    album_table: Vec<(String, String, String, String)>,
    /// Every artist once: (name, folded name, tracks in the library).
    artist_table: Vec<(String, String, usize)>,
}

/// The merge worker prepares both libraries. Going offline only swaps an
/// Arc on the UI thread; searches and playlist counts use the pruned data.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub all: Arc<Index>,
    pub offline: Arc<Index>,
}

impl From<Index> for Snapshot {
    fn from(index: Index) -> Self {
        Self { offline: Arc::new(index.available_only()), all: Arc::new(index) }
    }
}

impl Index {
    pub fn new(source: SourceId) -> Self {
        Self { source: Some(source), ..Self::default() }
    }

    /// The id of the track with `uri`.
    pub fn id_of(&self, uri: &str) -> Option<u32> {
        self.by_uri.get(uri).copied()
    }

    pub fn track(&self, uri: &str) -> Option<&Track> {
        self.by_uri.get(uri).map(|&id| &self.tracks[id as usize])
    }

    pub fn entry(&self, uri: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.uri == uri)
    }

    pub fn playlist_count(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.is_folder).count()
    }

    /// The tree as the system lists it now. Known playlists keep their
    /// tracks until they are read again; ones no longer listed are dropped.
    pub fn set_library(&mut self, library: Vec<LibraryEntry>) {
        let mut old: HashMap<String, Entry> = self.entries.drain(..).map(|entry| (entry.uri.clone(), entry)).collect();
        self.entries = library
            .into_iter()
            .map(|item| {
                let mut entry = old.remove(&item.uri).unwrap_or_default();
                entry.uri = item.uri;
                entry.name = item.name;
                entry.is_folder = item.is_folder;
                entry.depth = item.depth;
                entry
            })
            .collect();
        self.relink();
    }

    /// A playlist's tracks, as just read.
    pub fn set_playlist(&mut self, playlist: Playlist, now_secs: u64) {
        let Some(at) = self.entry(&playlist.uri) else { return };
        let mut ids = Vec::with_capacity(playlist.tracks.len());
        for listed in playlist.tracks {
            let id = match self.by_uri.get(&listed.uri) {
                Some(&id) => {
                    let track = &mut self.tracks[id as usize];
                    track.name = listed.name;
                    track.artists = listed.artists.join(", ");
                    track.album = listed.album;
                    id
                }
                None => {
                    let id = self.tracks.len() as u32;
                    self.by_uri.insert(listed.uri.clone(), id);
                    self.tracks.push(Track { uri: listed.uri, name: listed.name, artists: listed.artists.join(", "), album: listed.album, ..Track::default() });
                    self.folded.push(String::new());
                    id
                }
            };
            self.refold(id);
            ids.push(id);
        }
        let entry = &mut self.entries[at];
        entry.name = playlist.name;
        entry.owner = playlist.owner;
        entry.track_count = playlist.track_count;
        entry.tracks = ids;
        entry.read_secs = now_secs;
        self.relink();
    }

    pub fn set_saved(&mut self, albums: Vec<SavedItem>, artists: Vec<SavedItem>) {
        self.albums = albums;
        self.artists = artists;
        self.tabulate();
    }

    /// Tracks no lookup has run for yet, in playlist order.
    pub fn unlooked(&self) -> Vec<String> {
        self.tracks.iter().filter(|track| !track.looked_up && !track.playlists.is_empty()).map(|track| track.uri.clone()).collect()
    }

    /// Record a lookup of `asked`: what it found, and that it ran.
    /// Record the tempo each of these tracks' files claims.
    pub fn set_tempos(&mut self, tempos: Vec<(String, f32)>) {
        for (uri, bpm) in tempos {
            if let Some(&id) = self.by_uri.get(&uri) {
                self.tracks[id as usize].bpm = bpm;
            }
        }
    }

    pub fn set_details(&mut self, asked: &[String], details: Vec<TrackDetail>) {
        for uri in asked {
            if let Some(&id) = self.by_uri.get(uri) {
                self.tracks[id as usize].looked_up = true;
            }
        }
        for detail in details {
            if let Some(&id) = self.by_uri.get(&detail.uri) {
                let track = &mut self.tracks[id as usize];
                track.duration_ms = detail.duration_ms;
                track.art_url = detail.art_url;
                if !detail.album.is_empty() { track.album = detail.album; }
                track.album_uri = detail.album_uri;
                track.release = detail.release;
                track.artist_uri = detail.artist_uri;
                self.refold(id);
            }
        }
        self.tabulate();
    }

    /// The folded title, artists and album of a track: what the table sorts
    /// text columns by.
    pub fn folded_fields(&self, id: u32) -> (&str, &str, &str) {
        let mut fields = self.folded.get(id as usize).map(String::as_str).unwrap_or("").split('\u{1}');
        (fields.next().unwrap_or(""), fields.next().unwrap_or(""), fields.next().unwrap_or(""))
    }

    /// Recompute which playlists hold each track, and drop tracks no
    /// playlist holds any more.
    fn relink(&mut self) {
        let mut held = vec![Vec::new(); self.tracks.len()];
        for (at, entry) in self.entries.iter().enumerate() {
            for &id in &entry.tracks {
                if let Some(list) = held.get_mut(id as usize) {
                    if list.last() != Some(&(at as u32)) {
                        list.push(at as u32);
                    }
                }
            }
        }
        if held.iter().all(|list| !list.is_empty()) {
            for (track, list) in self.tracks.iter_mut().zip(held) {
                track.playlists = list;
            }
            self.tabulate();
            return;
        }
        // Renumber the tracks that are still held.
        let mut remap = vec![u32::MAX; self.tracks.len()];
        let mut tracks = Vec::with_capacity(self.tracks.len());
        let mut folded = Vec::with_capacity(self.tracks.len());
        for (old, (mut track, list)) in self.tracks.drain(..).zip(held).enumerate() {
            if list.is_empty() {
                continue;
            }
            remap[old] = tracks.len() as u32;
            track.playlists = list;
            tracks.push(track);
            folded.push(std::mem::take(&mut self.folded[old]));
        }
        for entry in &mut self.entries {
            for id in &mut entry.tracks {
                *id = remap[*id as usize];
            }
        }
        self.tracks = tracks;
        self.folded = folded;
        self.by_uri = self.tracks.iter().enumerate().map(|(id, track)| (track.uri.clone(), id as u32)).collect();
        self.tabulate();
    }

    /// The albums and artists search looks through, folded once here
    /// rather than on every keystroke: from the tracks, then the saved
    /// ones the playlists do not mention.
    fn tabulate(&mut self) {
        let mut albums: HashMap<(String, String), usize> = HashMap::new();
        self.album_table.clear();
        let mut artists: HashMap<String, usize> = HashMap::new();
        self.artist_table.clear();
        for track in self.tracks.iter().filter(|track| track.copy_of.is_none()) {
            if !track.album.is_empty() {
                let key = (track.album.clone(), track.artists.clone());
                match albums.get(&key) {
                    Some(&at) => {
                        if self.album_table[at].2.is_empty() {
                            self.album_table[at].2 = track.album_uri.clone();
                        }
                    }
                    None => {
                        albums.insert(key, self.album_table.len());
                        self.album_table.push((track.album.clone(), track.artists.clone(), track.album_uri.clone(), fold(&track.album)));
                    }
                }
            }
            for artist in track.artists.split(", ").filter(|artist| !artist.is_empty()) {
                match artists.get(artist) {
                    Some(&at) => self.artist_table[at].2 += 1,
                    None => {
                        artists.insert(artist.to_string(), self.artist_table.len());
                        self.artist_table.push((artist.to_string(), fold(artist), 1));
                    }
                }
            }
        }
        for album in &self.albums {
            if !self.album_table.iter().any(|a| a.2 == album.uri) {
                self.album_table.push((album.name.clone(), album.artists.clone(), album.uri.clone(), fold(&album.name)));
            }
        }
        for artist in &self.artists {
            if !artists.contains_key(&artist.name) {
                artists.insert(artist.name.clone(), self.artist_table.len());
                self.artist_table.push((artist.name.clone(), fold(&artist.name), 0));
            }
        }
    }

    fn refold(&mut self, id: u32) {
        let track = &self.tracks[id as usize];
        self.folded[id as usize] = format!("{}\u{1}{}\u{1}{}", fold(&track.name), fold(&track.artists), fold(&track.album));
    }

    /// Everything in the library that matches `query`; at most `limit`
    /// tracks are returned (all are counted).
    pub fn search(&self, query: &str, limit: usize) -> Hits {
        let folded_query = fold(query);
        let words: Vec<&str> = folded_query.split_whitespace().collect();
        let mut hits = Hits::default();
        if words.is_empty() {
            return hits;
        }
        let mut scored: Vec<(u32, u32)> = Vec::new();
        'tracks: for (id, text) in self.folded.iter().enumerate() {
            if self.tracks[id].copy_of.is_some() {
                continue;
            }
            let mut score = 0;
            for word in &words {
                match score_word(text, word) {
                    Some(points) => score += points,
                    None => continue 'tracks,
                }
            }
            let title = text.split('\u{1}').next().unwrap_or("");
            if title == folded_query {
                score += 40;
            } else if title.starts_with(folded_query.as_str()) {
                score += 12;
            }
            scored.push((id as u32, score));
        }
        hits.track_total = scored.len();
        scored.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| self.tracks[a.0 as usize].name.cmp(&self.tracks[b.0 as usize].name)));
        hits.tracks = scored.into_iter().take(limit).map(|(id, _)| id).collect();

        let matches_all = |text: &str| {
            let text = fold(text);
            words.iter().all(|word| text.contains(word))
        };
        hits.playlists = self.entries.iter().enumerate().filter(|(_, entry)| !entry.is_folder && matches_all(&entry.name)).map(|(at, _)| at).take(50).collect();

        let words_in = |folded: &str| words.iter().all(|word| folded.contains(word));
        hits.albums = self.album_table.iter().filter(|album| words_in(&album.3)).take(30).map(|album| (album.0.clone(), album.1.clone(), album.2.clone())).collect();
        let mut artists: Vec<(String, usize)> = self.artist_table.iter().filter(|artist| words_in(&artist.1)).map(|artist| (artist.0.clone(), artist.2)).collect();
        artists.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        artists.truncate(30);
        hits.artists = artists;
        hits
    }

    /// Tracks of the library by an artist (by name), in title order.
    pub fn by_artist(&self, artist: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = (0..self.tracks.len() as u32)
            .filter(|&id| {
                let track = &self.tracks[id as usize];
                track.copy_of.is_none() && track.artists.split(", ").any(|a| a == artist)
            })
            .collect();
        ids.sort_by(|a, b| self.tracks[*a as usize].name.cmp(&self.tracks[*b as usize].name));
        ids
    }

    /// Tracks of the library on an album (by name and artists).
    pub fn by_album(&self, album: &str, artists: &str) -> Vec<u32> {
        (0..self.tracks.len() as u32).filter(|&id| {
            let track = &self.tracks[id as usize];
            track.copy_of.is_none() && track.album == album && track.artists == artists
        }).collect()
    }

    /// Whether a track can play from a local file: it is one, or has a copy.
    pub fn has_local(&self, id: u32) -> bool {
        self.tracks.get(id as usize).is_some_and(|track| track.local_copy.is_some() || track.local_path().is_some() || track.cached_path.is_some())
    }

    pub fn is_cached(&self, uri: &str) -> bool {
        self.track(uri).is_some_and(|track| track.cached_path.is_some())
    }

    /// The file `uri` can play from, when there is one: a local track's own,
    /// or the local copy of a system track.
    pub fn local_file<'a>(&'a self, uri: &'a str) -> Option<&'a str> {
        // A file the folder scan no longer finds (removed, or a queue kept
        // from before it went) is not played.
        if let Some(path) = uri.strip_prefix(LOCAL_FILE) {
            return self.track(uri).is_some().then_some(path);
        }
        let track = self.track(uri)?;
        track.local_copy.and_then(|copy| self.tracks.get(copy as usize)?.local_path())
            .or(track.cached_path.as_deref())
    }

    /// Attach the self-contained offline song database even when Spotify is
    /// absent. This runs on the merge worker, never in a draw or search.
    pub fn with_cache(mut self, library: &crate::cache::Library) -> Self {
        let mut ids = Vec::new();
        let entry = self.entries.len() as u32;
        for song in &library.songs {
            let Some(path) = crate::cache::relative(&library.root, &song.audio) else { continue };
            let id = match self.id_of(&song.uri) {
                Some(id) => id,
                None => {
                    let id = self.tracks.len() as u32;
                    self.by_uri.insert(song.uri.clone(), id);
                    self.tracks.push(Track {
                        uri: song.uri.clone(), name: song.title.clone(), artists: song.artist.clone(),
                        album: song.album.clone(), album_uri: song.album_uri.clone(), artist_uri: song.artist_uri.clone(),
                        duration_ms: song.duration_ms, release: song.release, looked_up: true,
                        ..Default::default()
                    });
                    self.folded.push(String::new());
                    self.refold(id);
                    id
                }
            };
            let track = &mut self.tracks[id as usize];
            track.cached_path = Some(path.to_string_lossy().into_owned());
            if !song.artwork.is_empty() {
                track.art_url = format!("{}{}", crate::cache::ART_URI, song.artwork);
            }
            track.playlists.push(entry);
            ids.push(id);
        }
        self.entries.push(Entry {
            uri: crate::cache::LIBRARY_URI.into(), name: "Cached songs".into(), depth: 1,
            track_count: ids.len() as u32, tracks: ids, read_secs: 1, ..Default::default()
        });
        self.tabulate();
        self
    }

    /// A complete offline index: playable files, their albums and artists,
    /// and nonempty playlists with correct counts. The online index stays
    /// intact so reconnecting restores the full library.
    fn available_only(&self) -> Self {
        let mut out = Self { source: self.source, ..Self::default() };
        let mut ids = vec![None; self.tracks.len()];
        for (old, track) in self.tracks.iter().enumerate() {
            if !self.has_local(old as u32) { continue; }
            let id = out.tracks.len() as u32;
            ids[old] = Some(id);
            out.by_uri.insert(track.uri.clone(), id);
            out.tracks.push(track.clone());
            out.folded.push(self.folded[old].clone());
        }
        for track in &mut out.tracks {
            track.local_copy = track.local_copy.and_then(|id| ids[id as usize]);
            track.copy_of = track.copy_of.and_then(|id| ids[id as usize]);
            track.playlists.clear();
        }
        let mut keep = vec![false; self.entries.len()];
        let mut parents: Vec<usize> = Vec::new();
        for (at, entry) in self.entries.iter().enumerate() {
            parents.retain(|&parent| self.entries[parent].depth < entry.depth);
            if entry.is_folder { parents.push(at); }
            // Keep the local-folder management action even with no files.
            if entry.uri == crate::local::LOCAL_ROOT || entry.tracks.iter().any(|&id| ids[id as usize].is_some()) {
                keep[at] = true;
                for &parent in &parents { keep[parent] = true; }
            }
        }
        for (at, entry) in self.entries.iter().enumerate() {
            if !keep[at] { continue; }
            let mut entry = entry.clone();
            entry.tracks = entry.tracks.iter().filter_map(|&id| ids[id as usize]).collect();
            entry.track_count = entry.tracks.len() as u32;
            entry.read_secs = entry.read_secs.max(1);
            for &id in &entry.tracks { out.tracks[id as usize].playlists.push(out.entries.len() as u32); }
            out.entries.push(entry);
        }
        out.tabulate();
        out
    }

    /// The music system's library with the local files beside it: the local
    /// folders after the system's playlists, and every local file that is a
    /// copy of one of the system's tracks linked to it both ways.
    ///
    /// A file is a copy when its title and first artist match the track's
    /// once both are folded and the title's decorations are cut ("- 2011
    /// Remaster", "(feat. …)", "[Live]"); when several tracks match, or the
    /// title only matches that loosely, the length decides: within
    /// [`MATCH_SLACK_MS`] of the track's, the closest one. A length either
    /// side does not know leaves only an exact title with one candidate.
    pub fn merged(system: &Index, local: &Index) -> Index {
        if local.tracks.is_empty() && local.entries.is_empty() {
            return system.clone();
        }
        let mut out = system.clone();
        let entry_offset = out.entries.len() as u32;
        let track_offset = out.tracks.len() as u32;
        for track in &local.tracks {
            let mut track = track.clone();
            for at in &mut track.playlists {
                *at += entry_offset;
            }
            out.by_uri.insert(track.uri.clone(), out.tracks.len() as u32);
            out.tracks.push(track);
        }
        out.folded.extend(local.folded.iter().cloned());
        out.entries.extend(local.entries.iter().map(|entry| {
            let mut entry = entry.clone();
            for id in &mut entry.tracks {
                *id += track_offset;
            }
            entry
        }));
        let mut by_key: HashMap<(String, String), Vec<u32>> = HashMap::new();
        for id in 0..track_offset {
            let track = &out.tracks[id as usize];
            by_key.entry(match_key(&track.name, &track.artists)).or_default().push(id);
        }
        for id in track_offset..out.tracks.len() as u32 {
            let file = &out.tracks[id as usize];
            let Some(candidates) = by_key.get(&match_key(&file.name, &file.artists)) else { continue };
            let exact = fold(&file.name);
            let pick = pick_copy(candidates, file.duration_ms, |candidate| {
                let track = &out.tracks[candidate as usize];
                (track.duration_ms, fold(&track.name) == exact, track.local_copy.is_some())
            });
            if let Some(original) = pick {
                out.tracks[original as usize].local_copy = Some(id);
                out.tracks[id as usize].copy_of = Some(original);
            }
        }
        out.tabulate();
        out
    }

    // ---- on disk ----

    /// Written whole to a temporary file and renamed over the old one, so
    /// a crash mid-write leaves the previous index.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut out = String::with_capacity(self.tracks.len() * 170);
        out.push_str(MAGIC);
        out.push('\n');
        for track in &self.tracks {
            line(
                &mut out,
                'T',
                &[
                    &track.uri,
                    &track.name,
                    &track.artists,
                    &track.album,
                    &track.album_uri,
                    &track.duration_ms.to_string(),
                    &track.art_url,
                    if track.looked_up { "1" } else { "0" },
                    &track.release.to_string(),
                    &track.artist_uri,
                ],
            );
        }
        for entry in &self.entries {
            let ids: Vec<String> = entry.tracks.iter().map(u32::to_string).collect();
            line(
                &mut out,
                'P',
                &[&entry.uri, &entry.name, &entry.owner, if entry.is_folder { "1" } else { "0" }, &entry.depth.to_string(), &entry.track_count.to_string(), &entry.read_secs.to_string(), &ids.join(",")],
            );
        }
        for album in &self.albums {
            line(&mut out, 'A', &[&album.uri, &album.name, &album.artists]);
        }
        for artist in &self.artists {
            line(&mut out, 'R', &[&artist.uri, &artist.name]);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        let mut file = fs::File::create(&temp)?;
        file.write_all(out.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)
    }

    /// The index saved at `path`, or `None` when there is none or it is not
    /// ours. Lines it cannot read are skipped.
    pub fn load(path: &Path, source: SourceId) -> Option<Self> {
        Self::parse(&fs::read_to_string(path).ok()?, source)
    }

    fn parse(text: &str, source: SourceId) -> Option<Self> {
        let mut lines = text.lines();
        let version = match lines.next()? {
            MAGIC => 3,
            MAGIC_V2 => 2,
            MAGIC_V1 => 1,
            _ => return None,
        };
        let mut index = Self::new(source);
        for raw in lines {
            let fields: Vec<String> = raw.split('\t').map(unescape).collect();
            let field = |at: usize| fields.get(at).map(String::as_str).unwrap_or("");
            match field(0) {
                "T" if fields.len() >= 9 => {
                    let id = index.tracks.len() as u32;
                    index.by_uri.insert(field(1).to_string(), id);
                    index.tracks.push(Track {
                        uri: field(1).into(),
                        name: field(2).into(),
                        artists: field(3).into(),
                        album: field(4).into(),
                        album_uri: field(5).into(),
                        duration_ms: field(6).parse().unwrap_or(0),
                        art_url: field(7).into(),
                        release: field(9).parse().unwrap_or(0),
                        artist_uri: field(10).into(),
                        // A track from an older file was looked up without
                        // what its version lacked: the next refresh asks again.
                        looked_up: field(8) == "1" && version == 3,
                        ..Track::default()
                    });
                    index.folded.push(String::new());
                    index.refold(id);
                }
                "P" if fields.len() >= 9 => {
                    let count = index.tracks.len() as u32;
                    // A version 2 line's dates added (field 9) are not read.
                    let tracks: Vec<u32> = field(8).split(',').filter_map(|id| id.parse::<u32>().ok()).filter(|&id| id < count).collect();
                    index.entries.push(Entry {
                        uri: field(1).into(),
                        name: field(2).into(),
                        owner: field(3).into(),
                        is_folder: field(4) == "1",
                        depth: field(5).parse().unwrap_or(1),
                        track_count: field(6).parse().unwrap_or(0),
                        read_secs: field(7).parse().unwrap_or(0),
                        tracks,
                    });
                }
                "A" if fields.len() >= 4 => index.albums.push(SavedItem { uri: field(1).into(), name: field(2).into(), artists: field(3).into() }),
                "R" if fields.len() >= 3 => index.artists.push(SavedItem { uri: field(1).into(), name: field(2).into(), artists: String::new() }),
                _ => {}
            }
        }
        index.relink();
        Some(index)
    }
}

/// Points for one query word in a folded "title\u{1}artists\u{1}album":
/// a word start beats a match inside a word, and the title beats the
/// artist, the artist the album. `None` when the word is nowhere.
fn score_word(text: &str, word: &str) -> Option<u32> {
    let mut best = None;
    for (at, _) in text.match_indices(word) {
        let field = text[..at].matches('\u{1}').count();
        let weight = [3, 2, 1].get(field).copied().unwrap_or(1);
        let starts_word = at == 0 || matches!(text.as_bytes()[at - 1], b' ' | 1);
        let points = weight * if starts_word { 2 } else { 1 };
        best = Some(best.map_or(points, |b: u32| b.max(points)));
        if points == 6 {
            break;
        }
    }
    best
}

/// How far a local file's length may be from a track's for it to be a copy.
pub const MATCH_SLACK_MS: u32 = 3000;

/// The title and first artist a local file and a track are matched on:
/// folded, the title without what editions add to it ("- Remastered 2011",
/// "(feat. X)", "[Live]"), the artist the first one named.
pub fn match_key(title: &str, artists: &str) -> (String, String) {
    (fold(&base_title(title)), fold(first_artist(artists)))
}

/// A title without its bracketed parts, its " - " suffix and a featured
/// artist: "Wish You Were Here - 2011 Remaster" is "Wish You Were Here".
fn base_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut depth = 0u32;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    if let Some(at) = out.find(" - ") {
        out.truncate(at);
    }
    for marker in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(at) = find_ascii_ci(&out, marker) {
            out.truncate(at);
            break;
        }
    }
    let trimmed = out.trim();
    // A title that was only decoration keeps its own words.
    if trimmed.is_empty() { title.to_string() } else { trimmed.to_string() }
}

/// The first of the artists a track or a file names.
fn first_artist(artists: &str) -> &str {
    let mut end = artists.len();
    for separator in [", ", " & ", "; ", " feat. ", " feat ", " ft. ", " featuring ", " x "] {
        if let Some(at) = find_ascii_ci(artists, separator) {
            end = end.min(at);
        }
    }
    artists[..end].trim()
}

/// Where the ASCII `needle` first starts in `text`, ignoring ASCII case: a
/// byte offset into `text` itself, at a char boundary. (Offsets found in a
/// lower-cased copy do not hold for the original: "İ" lower-cases longer.)
fn find_ascii_ci(text: &str, needle: &str) -> Option<usize> {
    let (text_bytes, needle) = (text.as_bytes(), needle.as_bytes());
    text.char_indices().map(|(at, _)| at).find(|&at| text_bytes.get(at..at + needle.len()).is_some_and(|here| here.eq_ignore_ascii_case(needle)))
}

/// Which of `candidates` (tracks whose match key is a file's) the file of
/// `duration_ms` (0 unknown) is a copy of. `about(candidate)` tells a
/// candidate's length (0 unknown), whether its folded title is the file's
/// exactly, and whether it has a copy already. See [`Index::merged`].
fn pick_copy(candidates: &[u32], duration_ms: u32, about: impl Fn(u32) -> (u32, bool, bool)) -> Option<u32> {
    let free: Vec<(u32, u32, bool)> = candidates.iter().filter_map(|&id| {
        let (length, exact, taken) = about(id);
        (!taken).then_some((id, length, exact))
    }).collect();
    if duration_ms > 0 {
        let closest = free
            .iter()
            .filter(|(_, length, _)| *length > 0)
            .map(|(id, length, _)| (*id, length.abs_diff(duration_ms)))
            .filter(|(_, apart)| *apart <= MATCH_SLACK_MS)
            .min_by_key(|(_, apart)| *apart);
        if let Some((id, _)) = closest {
            return Some(id);
        }
    }
    // Lengths do not decide: only the one track of the very same title, whose
    // length does not contradict the file's.
    let unknown: Vec<&(u32, u32, bool)> = free.iter().filter(|(_, length, exact)| *exact && (*length == 0 || duration_ms == 0)).collect();
    match unknown.as_slice() {
        [(id, _, _)] if free.len() == 1 => Some(*id),
        _ => None,
    }
}

/// One line of a tab-separated file: a tag, then escaped fields.
pub(crate) fn line(out: &mut String, tag: char, fields: &[&str]) {
    out.push(tag);
    for field in fields {
        out.push('\t');
        escape(out, field);
    }
    out.push('\n');
}

fn escape(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

pub(crate) fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::PlaylistTrack;

    fn track(uri: &str, name: &str, artist: &str, album: &str) -> PlaylistTrack {
        PlaylistTrack { uri: uri.into(), name: name.into(), artists: vec![artist.into()], album: album.into() }
    }

    fn library() -> Index {
        let mut index = Index::new(SourceId::Spotify);
        index.set_library(vec![
            LibraryEntry { uri: "p:1".into(), name: "Starred".into(), is_folder: false, depth: 1 },
            LibraryEntry { uri: "f:1".into(), name: "Mixes".into(), is_folder: true, depth: 1 },
            LibraryEntry { uri: "p:2".into(), name: "Deep Beats".into(), is_folder: false, depth: 2 },
        ]);
        index.set_playlist(
            Playlist {
                uri: "p:1".into(),
                name: "Starred".into(),
                track_count: 3,
                tracks: vec![track("t:1", "Toxygene", "The Orb", "Orblivion"), track("t:2", "Déjà Vu", "Beyoncé", "B'Day"), track("t:3", "Orbital Sunrise", "Kiasmos", "Blurred")],
                ..Playlist::default()
            },
            100,
        );
        index.set_playlist(Playlist { uri: "p:2".into(), name: "Deep Beats".into(), track_count: 1, tracks: vec![track("t:1", "Toxygene", "The Orb", "Orblivion")], ..Playlist::default() }, 100);
        index
    }

    #[test]
    fn a_track_in_two_playlists_is_one_track_that_knows_both() {
        let index = library();
        assert_eq!(index.tracks.len(), 3);
        assert_eq!(index.track("t:1").unwrap().playlists, vec![0, 2]);
        assert_eq!(index.playlist_count(), 2);
    }

    #[test]
    fn every_word_must_match_and_titles_rank_first() {
        let index = library();
        let hits = index.search("orb", 10);
        // "Orbital Sunrise" starts its title with the word; "Toxygene" by
        // The Orb matches the artist; both outrank an album-only match.
        assert_eq!(hits.track_total, 2);
        assert_eq!(index.tracks[hits.tracks[0] as usize].name, "Orbital Sunrise");
        assert_eq!(index.tracks[hits.tracks[1] as usize].name, "Toxygene");
        let hits = index.search("deja beyonce", 10);
        assert_eq!(hits.track_total, 1);
        assert_eq!(index.tracks[hits.tracks[0] as usize].name, "Déjà Vu");
        assert_eq!(index.search("orb nothing", 10).track_total, 0);
        assert_eq!(index.search("  ", 10).track_total, 0);
    }

    #[test]
    fn playlists_albums_and_artists_are_found_by_name() {
        let index = library();
        let hits = index.search("deep", 10);
        assert_eq!(hits.playlists, vec![2]);
        let hits = index.search("blur", 10);
        assert_eq!(hits.albums[0].0, "Blurred");
        let hits = index.search("kias", 10);
        assert_eq!(hits.artists, vec![("Kiasmos".to_string(), 1)]);
    }

    #[test]
    fn details_fill_in_and_are_asked_for_once() {
        let mut index = library();
        assert_eq!(index.unlooked().len(), 3);
        index.set_details(&["t:1".into(), "t:2".into()], vec![TrackDetail { uri: "t:1".into(), duration_ms: 1000, art_url: "https://x".into(), album_uri: "a:1".into(), release: 19_950_000, artist_uri: "ar:1".into(), ..Default::default() }]);
        assert_eq!(index.unlooked(), vec!["t:3".to_string()]);
        assert_eq!(index.track("t:1").unwrap().duration_ms, 1000);
        assert_eq!(index.track("t:1").unwrap().release, 19_950_000);
    }

    #[test]
    fn a_playlist_dropped_from_the_library_takes_its_only_tracks_along() {
        let mut index = library();
        index.set_library(vec![LibraryEntry { uri: "p:2".into(), name: "Deep Beats".into(), is_folder: false, depth: 1 }]);
        assert_eq!(index.tracks.len(), 1);
        assert_eq!(index.track("t:1").unwrap().playlists, vec![0]);
        assert_eq!(index.search("toxy", 10).track_total, 1);
        assert_eq!(index.search("kiasmos", 10).track_total, 0);
    }

    #[test]
    fn the_index_reads_back_as_written() {
        let mut index = library();
        index.set_details(&["t:2".into()], vec![TrackDetail { uri: "t:2".into(), duration_ms: 240_000, art_url: "https://i/x".into(), album_uri: "spotify:album:x".into(), release: 20_010_203, artist_uri: "spotify:artist:y".into(), ..Default::default() }]);
        index.set_saved(vec![SavedItem { uri: "a:9".into(), name: "Tab\there".into(), artists: "Line\nbreak \\ slash".into() }], vec![]);
        let dir = std::env::temp_dir().join(format!("makepad-music-index-test-{}", std::process::id()));
        let path = dir.join("index.txt");
        index.save(&path).unwrap();
        let back = Index::load(&path, SourceId::Spotify).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(back.tracks, index.tracks);
        assert_eq!(back.entries, index.entries);
        assert_eq!(back.albums, index.albums);
        assert_eq!(back.search("deja", 5).track_total, 1);
    }

    #[test]
    fn search_over_tens_of_thousands_of_tracks_is_quick() {
        let mut index = Index::new(SourceId::Spotify);
        index.set_library(vec![LibraryEntry { uri: "p:big".into(), name: "Big".into(), is_folder: false, depth: 1 }]);
        let tracks: Vec<PlaylistTrack> = (0..40_000).map(|i| track(&format!("t:{i}"), &format!("Song number {i} of the night"), &format!("Artist {}", i % 997), &format!("Album {}", i % 3001))).collect();
        index.set_playlist(Playlist { uri: "p:big".into(), name: "Big".into(), track_count: 40_000, tracks, ..Playlist::default() }, 1);
        let start = std::time::Instant::now();
        let hits = index.search("night artist 99", 200);
        let elapsed = start.elapsed();
        assert!(hits.track_total > 0);
        // Debug builds are several times slower than the release app.
        assert!(elapsed.as_millis() < 250, "search took {elapsed:?}");
    }

    /// Older files still read: a version 1 or 2 track keeps what it had but
    /// is looked up again, and a version 2 playlist's dates added (no longer
    /// kept) are skipped.
    #[test]
    fn older_index_files_read_and_ask_again() {
        let v1 = "makepad-music-index 1\n\
            T\tt:1\tToxygene\tThe Orb\tOrblivion\ta:1\t217000\thttps://i/x\t1\n\
            T\tt:2\tBent\tKiasmos\tKiasmos\ta:2\t345000\thttps://i/y\t1\n\
            P\tp:1\tStarred\tme\t0\t1\t2\t100\t0,1\n";
        let index = Index::parse(v1, SourceId::Spotify).unwrap();
        assert_eq!(index.tracks.len(), 2);
        assert_eq!(index.track("t:1").unwrap().duration_ms, 217_000);
        assert_eq!(index.unlooked().len(), 2);
        let v2 = "makepad-music-index 2\n\
            T\tt:1\tToxygene\tThe Orb\tOrblivion\ta:1\t217000\thttps://i/x\t1\t19970101\n\
            P\tp:1\tStarred\tme\t0\t1\t1\t100\t0\t1790000000\n";
        let index = Index::parse(v2, SourceId::Spotify).unwrap();
        assert_eq!(index.track("t:1").unwrap().release, 19_970_101);
        assert_eq!(index.entries[0].tracks, vec![0]);
        assert_eq!(index.unlooked(), vec!["t:1".to_string()]);
        assert!(Index::parse("makepad-music-index 9\n", SourceId::Spotify).is_none());
    }

    fn system_library(tracks: &[(&str, &str, &str, u32)]) -> Index {
        let mut index = Index::new(SourceId::Spotify);
        index.set_library(vec![crate::source::LibraryEntry { uri: "p:1".into(), name: "Mix".into(), is_folder: false, depth: 1 }]);
        index.set_playlist(
            Playlist { uri: "p:1".into(), name: "Mix".into(), tracks: tracks.iter().map(|(uri, name, artist, _)| track(uri, name, artist, "Album")).collect(), ..Playlist::default() },
            1,
        );
        let uris: Vec<String> = tracks.iter().map(|t| t.0.to_string()).collect();
        let details = tracks.iter().map(|(uri, _, _, ms)| TrackDetail { uri: uri.to_string(), duration_ms: *ms, ..TrackDetail::default() }).collect();
        index.set_details(&uris, details);
        index
    }

    fn files(files: &[(&str, &str, &str, u32)]) -> Index {
        let mut index = Index::new(SourceId::Local);
        index.set_library(vec![crate::source::LibraryEntry { uri: "local:folder:/m".into(), name: "m".into(), is_folder: false, depth: 2 }]);
        index.set_playlist(
            Playlist { uri: "local:folder:/m".into(), name: "m".into(), tracks: files.iter().map(|(path, name, artist, _)| track(&format!("{LOCAL_FILE}{path}"), name, artist, "")).collect(), ..Playlist::default() },
            1,
        );
        let uris: Vec<String> = files.iter().map(|f| format!("{LOCAL_FILE}{}", f.0)).collect();
        let details = files.iter().map(|(path, _, _, ms)| TrackDetail { uri: format!("{LOCAL_FILE}{path}"), duration_ms: *ms, ..TrackDetail::default() }).collect();
        index.set_details(&uris, details);
        index
    }

    fn copy_of<'a>(merged: &'a Index, uri: &str) -> Option<&'a str> {
        let copy = merged.track(uri)?.local_copy?;
        merged.tracks[copy as usize].local_path()
    }

    /// Titles match once folded and their edition decorations cut, artists
    /// on the first one named.
    #[test]
    fn match_keys_ignore_what_editions_add() {
        let key = |title: &str, artists: &str| match_key(title, artists);
        assert_eq!(key("Wish You Were Here - 2011 Remaster", "Pink Floyd"), key("Wish You Were Here", "Pink Floyd"));
        assert_eq!(key("Get Lucky (feat. Pharrell Williams)", "Daft Punk, Pharrell Williams"), key("Get Lucky", "Daft Punk"));
        assert_eq!(key("Heroes [Live]", "David Bowie"), key("heroes", "DAVID BOWIE"));
        assert_eq!(key("Déjà Vu", "Beyoncé feat. Jay-Z"), key("Deja Vu", "Beyonce"));
        assert_eq!(key("Song ft. Someone", "A & B"), key("Song", "A"));
        assert_ne!(key("Heroes", "David Bowie"), key("Heroes", "Wallflowers"));
        // A title that is only decoration keeps its words.
        assert_eq!(key("(Untitled)", "X").0, "untitled");
        // Text that lower-cases longer ("İ") does not shift where a marker
        // is cut.
        assert_eq!(key("İİİ Song FEAT. Someone", "İİ Band, Other"), key("iii song", "ii band"));
    }

    /// A file that is a copy links both ways; the length decides between
    /// editions within the slack; a file of no track stays its own; search
    /// and the artist page show the track once.
    #[test]
    fn local_copies_link_to_their_tracks() {
        let system = system_library(&[
            ("s:1", "Wish You Were Here - 2011 Remaster", "Pink Floyd", 334_000),
            ("s:2", "Toxygene - 7\" Edit", "The Orb", 217_000),
            ("s:3", "Toxygene", "The Orb", 480_000),
            ("s:4", "Heroes", "David Bowie", 0),
        ]);
        let local = files(&[
            ("/m/wywh.flac", "Wish You Were Here", "Pink Floyd", 335_500),
            ("/m/tox.mp3", "Toxygene", "The Orb", 481_000),
            ("/m/heroes.ogg", "Heroes", "David Bowie", 371_000),
            ("/m/mine.ogg", "My Own Song", "Me", 100_000),
        ]);
        let merged = Index::merged(&system, &local);
        assert_eq!(copy_of(&merged, "s:1"), Some("/m/wywh.flac"), "within the slack");
        assert_eq!(copy_of(&merged, "s:3"), Some("/m/tox.mp3"), "the edition of the right length");
        assert_eq!(copy_of(&merged, "s:2"), None, "the 7\" edit is another length");
        assert_eq!(copy_of(&merged, "s:4"), Some("/m/heroes.ogg"), "an exact title with no length to compare");
        let mine = merged.id_of(&format!("{LOCAL_FILE}/m/mine.ogg")).unwrap();
        assert_eq!(merged.tracks[mine as usize].copy_of, None);
        assert!(merged.has_local(mine));
        assert_eq!(merged.local_file("s:1"), Some("/m/wywh.flac"));
        assert_eq!(merged.local_file("s:2"), None);
        // The local folder follows the system's playlists.
        assert_eq!(merged.entries.len(), 2);
        assert_eq!(merged.entries[1].tracks.len(), 4);
        // Search shows the track, not its copy too; the own file shows.
        let hits = merged.search("wish you were", 10);
        assert_eq!(hits.tracks.len(), 1);
        assert_eq!(merged.tracks[hits.tracks[0] as usize].uri, "s:1");
        assert_eq!(merged.search("my own", 10).tracks, [mine]);
        assert_eq!(merged.by_artist("Pink Floyd").len(), 1);
    }

    /// Lengths too far apart are not a copy, even of the one title; two
    /// tracks of one title with no length to tell them apart take neither.
    #[test]
    fn a_copy_needs_the_length_to_agree() {
        let system = system_library(&[("s:1", "Intro", "A", 60_000), ("s:2", "Outro", "B", 0), ("s:3", "Outro", "B", 0)]);
        let local = files(&[("/m/intro.ogg", "Intro", "A", 200_000), ("/m/outro.ogg", "Outro", "B", 90_000)]);
        let merged = Index::merged(&system, &local);
        assert_eq!(copy_of(&merged, "s:1"), None);
        assert_eq!(copy_of(&merged, "s:2"), None);
        assert_eq!(copy_of(&merged, "s:3"), None);
        // Without local files the system's library is as it was.
        let alone = Index::merged(&system, &Index::new(SourceId::Local));
        assert_eq!(alone.tracks.len(), 3);
        assert_eq!(alone.entries.len(), 1);
    }
}
