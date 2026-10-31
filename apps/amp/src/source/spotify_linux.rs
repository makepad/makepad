//! Spotify on Linux.
//!
//! The Linux client (1.2.96, the spotify.com .deb that `spotify-launcher`,
//! the AUR package, the Flatpak and the Snap all repackage) ships no
//! `spotify_cli`. When one is there after all (a later client, or
//! `MUSIC_SPOTIFY_CLI`), [`cli_places`] finds it and the source is the same
//! CLI source as on macOS. Otherwise the client is driven through its own
//! page (`spotify_cdp.rs`), and over MPRIS (`org.mpris.MediaPlayer2.spotify`
//! on the session bus) whenever that page cannot be reached: what plays,
//! the transport, volume, shuffle and repeat, and playing any Spotify URI.
//! MPRIS has no library, playlists, search, queue or devices; those calls
//! answer empty or say so.

use super::dbus::{Bus, Value};
use super::*;
use std::path::PathBuf;

const SERVICE: &str = "org.mpris.MediaPlayer2.spotify";
const OBJECT: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// The one device MPRIS knows: the client on this machine.
pub(crate) const LOCAL_DEVICE: &str = "local";
const NO_CLI: &str = "the Linux Spotify client has no command-line tool; over MPRIS Amp can only see what plays and control playback";

/// Where a `spotify_cli` would sit beside the Linux client: the .deb and
/// AUR installs, spotify-launcher's own copy, the Flatpak and the Snap.
pub fn cli_places() -> Vec<PathBuf> {
    client_dirs().into_iter().map(|dir| dir.join("spotify_cli")).collect()
}

/// The folders the Linux client's files are installed in.
pub(crate) fn client_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/spotify"),
        PathBuf::from("/usr/share/spotify"),
        PathBuf::from("/usr/lib/spotify"),
        PathBuf::from("/var/lib/flatpak/app/com.spotify.Client/current/active/files/extra/share/spotify"),
        PathBuf::from("/snap/spotify/current/usr/share/spotify"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/spotify-launcher/install/usr/share/spotify"));
        dirs.push(home.join(".local/share/flatpak/app/com.spotify.Client/current/active/files/extra/share/spotify"));
    }
    dirs
}

/// Spotify driven over MPRIS, when the client is installed here (or its
/// player is on the bus right now).
pub fn find() -> Option<SpotifyMpris> {
    let installed = client_dirs().iter().any(|dir| dir.join("spotify").is_file())
        || ["/usr/bin/spotify", "/usr/bin/spotify-launcher", "/var/lib/flatpak/exports/bin/com.spotify.Client", "/snap/bin/spotify"]
            .iter()
            .any(|path| std::path::Path::new(path).exists());
    let mut source = SpotifyMpris { bus: None, metadata: None };
    (installed || source.ready().unwrap_or(false)).then_some(source)
}

pub struct SpotifyMpris {
    bus: Option<Bus>,
    /// The metadata of the last poll: what plays, its length and art.
    metadata: Option<Value>,
}

impl SpotifyMpris {
    /// A call on the session bus; a connection that failed is made again
    /// once.
    fn call(&mut self, destination: &str, path: &str, interface: &str, member: &str, args: &[Value]) -> Result<Vec<Value>, String> {
        for attempt in 0..2 {
            if self.bus.is_none() {
                self.bus = Some(Bus::session()?);
            }
            match self.bus.as_mut().unwrap().call(destination, path, interface, member, args) {
                Ok(reply) => return Ok(reply),
                // A D-Bus error is an answer; anything else broke the connection.
                Err(error) if error.starts_with("org.") || attempt == 1 => return Err(error),
                Err(_) => self.bus = None,
            }
        }
        unreachable!()
    }

    fn player(&mut self, member: &str, args: &[Value]) -> Result<(), String> {
        self.call(SERVICE, OBJECT, PLAYER, member, args).map(|_| ())
    }

    fn set(&mut self, property: &str, value: Value) -> Result<(), String> {
        self.call(SERVICE, OBJECT, PROPERTIES, "Set", &[Value::Str(PLAYER.into()), Value::Str(property.into()), Value::Variant(Box::new(value))]).map(|_| ())
    }

    /// Ask the client to quit (MPRIS `Quit`): it closes as if the person
    /// closed it.
    pub(crate) fn quit(&mut self) -> Result<(), String> {
        self.call(SERVICE, OBJECT, "org.mpris.MediaPlayer2", "Quit", &[]).map(|_| ())
    }

    fn properties(&mut self) -> Result<Value, String> {
        self.call(SERVICE, OBJECT, PROPERTIES, "GetAll", &[Value::Str(PLAYER.into())])?.into_iter().next().ok_or_else(|| "the player answered nothing".to_string())
    }
}

/// A Spotify URI for what MPRIS names a track: `mpris:trackid` is a URI
/// (`spotify:track:…`) or an object path (`/com/spotify/track/…`) by client
/// version, and `xesam:url` is an open.spotify.com link.
pub(crate) fn track_uri(metadata: &Value) -> String {
    if let Some(id) = metadata.get("mpris:trackid").and_then(Value::as_str) {
        if id.starts_with("spotify:") {
            return id.to_string();
        }
        if let Some(rest) = id.strip_prefix("/com/spotify/") {
            return format!("spotify:{}", rest.replace('/', ":"));
        }
    }
    if let Some(url) = metadata.get("xesam:url").and_then(Value::as_str) {
        if let Some(rest) = url.strip_prefix("https://open.spotify.com/") {
            let rest = rest.split(['?', '#']).next().unwrap_or_default();
            return format!("spotify:{}", rest.trim_matches('/').replace('/', ":"));
        }
    }
    String::new()
}

pub(crate) fn now_playing(properties: &Value) -> Option<NowPlaying> {
    let metadata = properties.get("Metadata")?;
    let uri = track_uri(metadata);
    let title = metadata.get("xesam:title").and_then(Value::as_str).unwrap_or_default().to_string();
    if uri.is_empty() && title.is_empty() {
        return None;
    }
    Some(NowPlaying {
        uri,
        title,
        artist: metadata.get("xesam:artist").map(Value::as_strings).unwrap_or_default().join(", "),
        // MPRIS does not say which playlist plays; the album is what it knows.
        context: metadata.get("xesam:album").and_then(Value::as_str).unwrap_or_default().to_string(),
        is_playing: properties.get("PlaybackStatus").and_then(Value::as_str) == Some("Playing"),
    })
}

pub(crate) fn detail(metadata: &Value) -> TrackDetail {
    TrackDetail {
        uri: track_uri(metadata),
        // mpris:length is in microseconds.
        duration_ms: metadata.get("mpris:length").and_then(Value::as_f64).map_or(0, |us| (us / 1000.0).clamp(0.0, u32::MAX as f64) as u32),
        art_url: metadata.get("mpris:artUrl").and_then(Value::as_str).unwrap_or_default().replace("https://open.spotify.com/image/", "https://i.scdn.co/image/"),
        album_uri: String::new(),
        album: metadata.get("xesam:album").and_then(Value::as_str).unwrap_or_default().to_string(),
        release: 0,
        artist_uri: String::new(),
    }
}

impl MusicSource for SpotifyMpris {
    fn id(&self) -> SourceId {
        SourceId::Spotify
    }

    fn player_bundle_id(&self) -> Option<&'static str> {
        Some("com.spotify.client")
    }

    /// The player is on the bus. MPRIS does not say whether anyone is
    /// signed in; the client registers only once it runs.
    fn ready(&mut self) -> Result<bool, String> {
        let reply = self.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &[Value::Str(SERVICE.into())])?;
        Ok(reply.first().and_then(Value::as_bool).unwrap_or(false))
    }

    /// No playlists over MPRIS: an empty library, not an error, so the
    /// rest of the app (local files, what plays) carries on.
    fn library(&mut self) -> Result<Vec<LibraryEntry>, String> {
        Ok(Vec::new())
    }

    fn saved_albums(&mut self) -> Result<Vec<SavedItem>, String> {
        Ok(Vec::new())
    }

    fn saved_artists(&mut self) -> Result<Vec<SavedItem>, String> {
        Ok(Vec::new())
    }

    fn playlist(&mut self, _uri: &str) -> Result<Playlist, String> {
        Err(format!("Playlists are not available: {NO_CLI}"))
    }

    /// Only the playing track is known (from the last poll).
    fn lookup(&mut self, uris: &[String]) -> Result<Vec<TrackDetail>, String> {
        let Some(metadata) = &self.metadata else { return Ok(Vec::new()) };
        let detail = detail(metadata);
        Ok((!detail.uri.is_empty() && uris.iter().any(|uri| *uri == detail.uri)).then_some(detail).into_iter().collect())
    }

    fn search(&mut self, _query: &str) -> Result<CatalogResults, String> {
        Err(format!("Spotify search is not available: {NO_CLI}"))
    }

    fn playback(&mut self) -> Result<(Option<NowPlaying>, Vec<QueueEntry>), String> {
        let properties = self.properties()?;
        let playing = now_playing(&properties);
        self.metadata = properties.get("Metadata").cloned();
        Ok((playing, Vec::new()))
    }

    fn devices(&mut self) -> Result<Vec<Device>, String> {
        let volume = self.properties().ok().and_then(|p| p.get("Volume").and_then(Value::as_f64)).unwrap_or(1.0);
        Ok(vec![Device { id: LOCAL_DEVICE.into(), name: "This computer".into(), kind: "Computer".into(), volume: volume.clamp(0.0, 1.0) as f32, active: true, this_machine: true }])
    }

    fn transport(&mut self, command: &Transport) -> Result<(), String> {
        match command {
            Transport::Play(uri) => self.player("OpenUri", &[Value::Str(uri.clone())]),
            Transport::Resume => self.player("Play", &[]),
            Transport::Pause => self.player("Pause", &[]),
            Transport::Next => self.player("Next", &[]),
            Transport::Previous => self.player("Previous", &[]),
            Transport::Seek(ms) => {
                // Spotify ignores SetPosition (its trackid is a string, not the
                // object path the call takes); a relative Seek from where it
                // is works.
                let reply = self.call(SERVICE, OBJECT, PROPERTIES, "Get", &[Value::Str(PLAYER.into()), Value::Str("Position".into())])?;
                let now_us = reply.first().and_then(Value::as_f64).ok_or("the player did not say where it is")? as i64;
                self.player("Seek", &[Value::I64(*ms as i64 * 1000 - now_us)])
            }
            Transport::Shuffle(on) => self.set("Shuffle", Value::Bool(*on)),
            Transport::Repeat(mode) => self.set(
                "LoopStatus",
                Value::Str(
                    match mode {
                        Repeat::Off => "None",
                        Repeat::Context => "Playlist",
                        Repeat::Track => "Track",
                    }
                    .into(),
                ),
            ),
            Transport::Volume(level) => self.set("Volume", Value::F64(level.clamp(0.0, 1.0) as f64)),
            Transport::Transfer(device) if device == LOCAL_DEVICE => Ok(()),
            Transport::Transfer(_) => Err(format!("Other devices are not available: {NO_CLI}")),
            Transport::Enqueue(_) | Transport::Dequeue(_) => Err(format!("The play queue is not available: {NO_CLI}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: Vec<(&str, Value)>) -> Value {
        Value::Dict(entries.into_iter().map(|(k, v)| (Value::Str(k.into()), Value::Variant(Box::new(v)))).collect())
    }

    #[test]
    fn track_ids_of_every_client_version_become_uris() {
        let path = dict(vec![("mpris:trackid", Value::Path("/com/spotify/track/4fHug3zCanK33sdvticISi".into()))]);
        assert_eq!(track_uri(&path), "spotify:track:4fHug3zCanK33sdvticISi");
        let uri = dict(vec![("mpris:trackid", Value::Str("spotify:track:abc".into()))]);
        assert_eq!(track_uri(&uri), "spotify:track:abc");
        let url = dict(vec![("xesam:url", Value::Str("https://open.spotify.com/track/xyz?si=1".into()))]);
        assert_eq!(track_uri(&url), "spotify:track:xyz");
    }

    /// Against the Spotify (or stand-in) player on this session bus:
    /// `cargo test -p makepad-app-amp live_player -- --ignored --nocapture`.
    /// Pauses and resumes; leaves the volume where it was.
    #[test]
    #[ignore]
    fn live_player_answers() {
        let mut source = SpotifyMpris { bus: None, metadata: None };
        assert!(source.ready().unwrap(), "no {SERVICE} on the session bus");
        let (now, queue) = source.playback().unwrap();
        println!("now playing: {now:?}; queue {}", queue.len());
        let uri = now.as_ref().map(|now| now.uri.clone()).unwrap_or_default();
        println!("lookup: {:?}", source.lookup(&[uri]).unwrap());
        let devices = source.devices().unwrap();
        println!("devices: {devices:?}");
        source.transport(&Transport::Pause).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("after pause: {:?}", source.playback().unwrap().0.map(|now| now.is_playing));
        source.transport(&Transport::Resume).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("after resume: {:?}", source.playback().unwrap().0.map(|now| now.is_playing));
        source.transport(&Transport::Volume(devices[0].volume)).unwrap();
        if let Some(now) = &now {
            source.transport(&Transport::Play(now.uri.clone())).unwrap();
            source.transport(&Transport::Seek(60_000)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            source.transport(&Transport::Pause).unwrap();
            println!("played {} again, sought to 60 s", now.uri);
        }
        // Shuffle and repeat are set, not read back: only against a stand-in.
        if std::env::var_os("MUSIC_LIVE_ALL").is_some() {
            source.transport(&Transport::Shuffle(true)).unwrap();
            source.transport(&Transport::Repeat(Repeat::Track)).unwrap();
            source.transport(&Transport::Next).unwrap();
            source.transport(&Transport::Previous).unwrap();
        }
        println!("search: {:?}", source.search("orb").err());
        println!("queue add: {:?}", source.transport(&Transport::Enqueue("spotify:track:x".into())).err());
    }

    #[test]
    fn metadata_reads_as_now_playing_and_detail() {
        let metadata = dict(vec![
            ("mpris:trackid", Value::Path("/com/spotify/track/abc".into())),
            ("xesam:title", Value::Str("Toxygene".into())),
            ("xesam:artist", Value::Array(vec![Value::Str("The Orb".into())])),
            ("xesam:album", Value::Str("Toxygene".into())),
            ("mpris:length", Value::U64(300_000_000)),
            ("mpris:artUrl", Value::Str("https://i.scdn.co/image/ab67".into())),
        ]);
        let properties = dict(vec![("Metadata", metadata.clone()), ("PlaybackStatus", Value::Str("Playing".into()))]);
        let now = now_playing(&properties).unwrap();
        assert_eq!(now.uri, "spotify:track:abc");
        assert_eq!(now.artist, "The Orb");
        assert!(now.is_playing);
        let detail = detail(&metadata);
        assert_eq!(detail.duration_ms, 300_000);
        assert_eq!(detail.art_url, "https://i.scdn.co/image/ab67");
    }
}
