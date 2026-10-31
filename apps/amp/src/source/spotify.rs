//! Spotify, through the command-line tool the client ships with it
//! (`Spotify.app/Contents/MacOS/spotify_cli` on macOS, `spotify_cli.exe`
//! beside `Spotify.exe` on Windows; same version as the client). Every
//! command takes `--format json`; positional arguments must come BEFORE the
//! flags, or the tool reads them as flag values and answers with nothing.
//!
//! Playback happens in the Spotify client itself (Spotify Connect): this
//! source is a library reader and a remote.

use super::*;
use makepad_micro_serde::*;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
#[cfg(any(unix, windows))]
use std::time::{Duration, Instant};

/// A command that has not answered by then is killed.
#[cfg(any(unix, windows))]
const TIMEOUT: Duration = Duration::from_secs(10);
/// The Microsoft Store package of the Spotify client.
#[cfg(windows)]
const STORE_FAMILY: &str = "SpotifyAB.SpotifyMusic_zpdnekdrzrea0";
/// Output larger than this is not a listing we asked for.
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
/// Items asked for per page of a listing.
const PAGE: usize = 500;
/// Tracks asked for per page of a playlist.
const TRACK_PAGE: usize = 2000;

pub struct Spotify {
    cli: PathBuf,
}

impl Spotify {
    /// The CLI where the client's installer puts it ([`installed_places`]).
    /// `MUSIC_SPOTIFY_CLI` names another place instead (a test run pointing
    /// it at nothing sees Spotify as not installed).
    pub fn find() -> Option<Self> {
        if let Some(cli) = std::env::var_os("MUSIC_SPOTIFY_CLI") {
            let cli = PathBuf::from(cli);
            return cli.is_file().then_some(Self { cli });
        }
        installed_places().into_iter().find(|path| path.is_file()).map(|cli| Self { cli })
    }

    /// Run one command and return what it printed. `args` are the command
    /// and its positional arguments; the JSON flag goes after them.
    pub(crate) fn run(&self, args: &[&str], extra_flags: &[&str]) -> Result<String, String> {
        run_with_timeout(&self.cli, args, extra_flags)
    }

    fn json<T: DeJson>(&self, args: &[&str], extra_flags: &[&str]) -> Result<T, String> {
        let text = self.run(args, extra_flags)?;
        T::deserialize_json_lenient(&text).map_err(|error| format!("{}: unexpected answer ({error:?})", args.join(" ")))
    }

    /// Every saved item of a kind. The tool answers a page at a time (50
    /// unless asked) and its `has_more` can say false with more to come, so
    /// pages are read until `total` is reached or one comes back empty.
    fn items(&self, kind: &str) -> Result<Vec<ItemJson>, String> {
        let mut items = Vec::new();
        loop {
            let offset = items.len().to_string();
            let limit = PAGE.to_string();
            let page = self.json::<ItemsJson>(&["library", "list"], &["--type", kind, "--limit", &limit, "--offset", &offset])?;
            let total = page.total.map(|total| total as usize);
            if page.items.is_empty() {
                return Ok(items);
            }
            items.extend(page.items);
            if total.is_none_or(|total| items.len() >= total) {
                return Ok(items);
            }
        }
    }
}

/// Where the client installs its CLI on macOS: Spotify.app, in
/// /Applications or the person's own Applications folder.
#[cfg(not(any(windows, target_os = "linux")))]
fn installed_places() -> Vec<PathBuf> {
    let mut places = vec![PathBuf::from("/Applications/Spotify.app/Contents/MacOS/spotify_cli")];
    if let Some(home) = std::env::var_os("HOME") {
        places.push(PathBuf::from(home).join("Applications/Spotify.app/Contents/MacOS/spotify_cli"));
    }
    places
}

/// Where the client installs its CLI on Windows: beside `Spotify.exe` in
/// the Microsoft Store package (its folder under WindowsApps cannot be
/// listed, so the package API names it), or in `%APPDATA%\Spotify` for the
/// installer from spotify.com.
#[cfg(windows)]
fn installed_places() -> Vec<PathBuf> {
    let mut places: Vec<PathBuf> = sys::package_folders(STORE_FAMILY).into_iter().map(|folder| folder.join("spotify_cli.exe")).collect();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        places.push(PathBuf::from(appdata).join("Spotify").join("spotify_cli.exe"));
    }
    places
}

/// Where a CLI would sit beside the Linux client (today's ships none; see
/// `spotify_linux`).
#[cfg(target_os = "linux")]
fn installed_places() -> Vec<PathBuf> {
    super::spotify_linux::cli_places()
}

/// Run the CLI and return what it printed. Both pipes are drained on the
/// calling worker itself, without blocking (a listing is larger than a pipe
/// buffer, and a child blocked on a full pipe never exits), so a call costs
/// no thread. The child leads its own process group (a job object on
/// Windows): past the deadline the whole group is killed, so a helper it
/// started cannot keep a pipe open and hold the worker. On Windows it
/// starts without a console window.
fn run_with_timeout(cli: &Path, args: &[&str], extra_flags: &[&str]) -> Result<String, String> {
    let what = || args.join(" ");
    let mut command = Command::new(cli);
    command.args(args).args(extra_flags).args(["--format", "json"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, sys::CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|error| format!("cannot start {}: {error}", cli.display()))?;
    let (out, err, success) = collect(&mut child).map_err(|error| format!("{}: {error}", what()))?;
    if !success {
        let message = String::from_utf8_lossy(&err).trim().to_string();
        return Err(if message.is_empty() { format!("{} failed", what()) } else { message });
    }
    String::from_utf8(out).map_err(|_| format!("{}: answer is not UTF-8", what()))
}

/// Stdout, stderr and whether the child succeeded; an error (and the child's
/// group killed and reaped) past [`TIMEOUT`] or [`MAX_OUTPUT_BYTES`].
#[cfg(unix)]
fn collect(child: &mut Child) -> Result<(Vec<u8>, Vec<u8>, bool), String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    for pipe in [stdout.as_ref().map(|p| p.as_raw_fd()), stderr.as_ref().map(|p| p.as_raw_fd())].into_iter().flatten() {
        sys::set_nonblocking(pipe);
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = loop {
        let moved = drain(&mut stdout, &mut out) | drain(&mut stderr, &mut err);
        if out.len() > MAX_OUTPUT_BYTES {
            break Err(format!("answer larger than {} MB", MAX_OUTPUT_BYTES >> 20));
        }
        match child.try_wait() {
            // Exited: what is still buffered is read; a helper still holding
            // the pipe is not waited for.
            Ok(Some(status)) => {
                drain(&mut stdout, &mut out);
                drain(&mut stderr, &mut err);
                return Ok((out, err, status.success()));
            }
            Ok(None) if Instant::now() >= deadline => break Err(format!("no answer in {} s", TIMEOUT.as_secs())),
            Ok(None) => {
                if !moved {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            Err(error) => break Err(error.to_string()),
        }
    };
    sys::kill_group(child.id());
    let _ = child.wait();
    result
}

/// Read what a non-blocking pipe has now; `true` when anything came. The
/// pipe is dropped at its end.
#[cfg(unix)]
fn drain(pipe: &mut Option<impl Read>, into: &mut Vec<u8>) -> bool {
    let Some(reader) = pipe.as_mut() else { return false };
    let mut moved = false;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                *pipe = None;
                return moved;
            }
            Ok(n) => {
                into.extend_from_slice(&buffer[..n]);
                moved = true;
                if into.len() > MAX_OUTPUT_BYTES {
                    return moved;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return moved,
        }
    }
}

/// The two calls std has no wrapper for.
#[cfg(unix)]
mod sys {
    use std::os::raw::c_int;

    extern "C" {
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        fn kill(pid: c_int, sig: c_int) -> c_int;
    }
    const F_GETFL: c_int = 3;
    const F_SETFL: c_int = 4;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_NONBLOCK: c_int = 0x0004;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const O_NONBLOCK: c_int = 0o4000;
    const SIGKILL: c_int = 9;

    pub fn set_nonblocking(fd: c_int) {
        // SAFETY: plain fcntl on a descriptor this process owns.
        unsafe {
            let flags = fcntl(fd, F_GETFL);
            if flags >= 0 {
                fcntl(fd, F_SETFL, flags | O_NONBLOCK);
            }
        }
    }

    /// Kill the process group led by `pid` (the child and anything it
    /// started). The child is not reaped yet, so its pid cannot have been
    /// handed out again.
    pub fn kill_group(pid: u32) {
        // SAFETY: kill(2) with a negative pid signals that process group.
        unsafe {
            kill(-(pid as c_int), SIGKILL);
        }
    }
}

/// Stdout, stderr and whether the child succeeded; an error (and the child's
/// job terminated and reaped) past [`TIMEOUT`] or [`MAX_OUTPUT_BYTES`].
/// Pipes are read only as far as they hold data (`PeekNamedPipe`), so the
/// worker never blocks on one.
#[cfg(windows)]
fn collect(child: &mut Child) -> Result<(Vec<u8>, Vec<u8>, bool), String> {
    let deadline = Instant::now() + TIMEOUT;
    // Its job: the child and whatever it starts, killed together.
    let job = sys::Job::holding(child.as_raw_handle());
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = loop {
        let moved = drain(&mut stdout, &mut out) | drain(&mut stderr, &mut err);
        if out.len() > MAX_OUTPUT_BYTES {
            break Err(format!("answer larger than {} MB", MAX_OUTPUT_BYTES >> 20));
        }
        match child.try_wait() {
            // Exited: what is still buffered is read; a helper still holding
            // the pipe is not waited for.
            Ok(Some(status)) => {
                drain(&mut stdout, &mut out);
                drain(&mut stderr, &mut err);
                return Ok((out, err, status.success()));
            }
            Ok(None) if Instant::now() >= deadline => break Err(format!("no answer in {} s", TIMEOUT.as_secs())),
            Ok(None) => {
                if !moved {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            Err(error) => break Err(error.to_string()),
        }
    };
    match &job {
        Some(job) => job.terminate(),
        None => {
            let _ = child.kill();
        }
    }
    let _ = child.wait();
    result
}

/// Read what a pipe holds now; `true` when anything came. The pipe is
/// dropped at its end (the writer closed it).
#[cfg(windows)]
fn drain<P: Read + AsRawHandle>(pipe: &mut Option<P>, into: &mut Vec<u8>) -> bool {
    let Some(reader) = pipe.as_mut() else { return false };
    let mut moved = false;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let available = match sys::available(reader.as_raw_handle()) {
            Some(0) => return moved,
            Some(available) => available,
            None => {
                *pipe = None;
                return moved;
            }
        };
        let want = available.min(buffer.len());
        match reader.read(&mut buffer[..want]) {
            Ok(0) => {
                *pipe = None;
                return moved;
            }
            Ok(n) => {
                into.extend_from_slice(&buffer[..n]);
                moved = true;
                if into.len() > MAX_OUTPUT_BYTES {
                    return moved;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                *pipe = None;
                return moved;
            }
        }
    }
}

/// The Windows calls std has no wrapper for: pipe peeking, job objects and
/// the package API.
#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::path::PathBuf;

    type Handle = *mut c_void;

    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: i32 = 122;

    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(pipe: Handle, buffer: *mut c_void, size: u32, read: *mut u32, available: *mut u32, left: *mut u32) -> i32;
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    // Linked by name at load time (Windows 8 and later), so no import
    // library has to carry them.
    #[link(name = "kernel32", kind = "raw-dylib")]
    extern "system" {
        fn GetPackagesByPackageFamily(family: *const u16, count: *mut u32, full_names: *mut *mut u16, buffer_length: *mut u32, buffer: *mut u16) -> i32;
        fn GetPackagePathByFullName(full_name: *const u16, path_length: *mut u32, path: *mut u16) -> i32;
    }

    /// Bytes waiting in a pipe; `None` once the writer has closed it.
    pub fn available(pipe: *mut c_void) -> Option<usize> {
        let mut available = 0u32;
        // SAFETY: a peek on a pipe handle this process owns, reading nothing.
        let ok = unsafe { PeekNamedPipe(pipe, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut available, std::ptr::null_mut()) };
        (ok != 0).then_some(available as usize)
    }

    /// A job object holding one child, so its whole tree can be killed.
    pub struct Job(Handle);

    impl Job {
        /// `None` when the child cannot be placed in a job; the caller
        /// kills the child alone then.
        pub fn holding(process: *mut c_void) -> Option<Job> {
            // SAFETY: plain kernel calls on handles this process owns.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                if AssignProcessToJobObject(job, process) == 0 {
                    CloseHandle(job);
                    return None;
                }
                Some(Job(job))
            }
        }

        pub fn terminate(&self) {
            // SAFETY: the job handle is ours until drop.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Closing the handle leaves the processes running (no
            // kill-on-close): only a timeout ends them.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The install folders of the packages of `family` installed for this
    /// user (usually one version).
    pub fn package_folders(family: &str) -> Vec<PathBuf> {
        let family = wide(family);
        let (mut count, mut length) = (0u32, 0u32);
        // SAFETY: the size query writes only the two counts.
        let status = unsafe { GetPackagesByPackageFamily(family.as_ptr(), &mut count, std::ptr::null_mut(), &mut length, std::ptr::null_mut()) };
        if (status != ERROR_SUCCESS && status != ERROR_INSUFFICIENT_BUFFER) || count == 0 {
            return Vec::new();
        }
        let mut names = vec![std::ptr::null_mut::<u16>(); count as usize];
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: both buffers are the sizes the query asked for.
        let status = unsafe { GetPackagesByPackageFamily(family.as_ptr(), &mut count, names.as_mut_ptr(), &mut length, buffer.as_mut_ptr()) };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }
        names.truncate(count as usize);
        let mut folders = Vec::new();
        for name in names {
            let mut path_length = 0u32;
            // SAFETY: `name` points into `buffer`, NUL-terminated.
            let status = unsafe { GetPackagePathByFullName(name, &mut path_length, std::ptr::null_mut()) };
            if status != ERROR_INSUFFICIENT_BUFFER || path_length == 0 {
                continue;
            }
            let mut path = vec![0u16; path_length as usize];
            if unsafe { GetPackagePathByFullName(name, &mut path_length, path.as_mut_ptr()) } != ERROR_SUCCESS {
                continue;
            }
            let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
            folders.push(PathBuf::from(String::from_utf16_lossy(&path[..end])));
        }
        folders
    }
}

/// Elsewhere the source never finds its tool; reading to the end on the
/// worker is enough.
#[cfg(not(any(unix, windows)))]
fn collect(child: &mut Child) -> Result<(Vec<u8>, Vec<u8>, bool), String> {
    let mut out = Vec::new();
    let mut err = Vec::new();
    if let Some(stdout) = child.stdout.as_mut() {
        let _ = stdout.take(MAX_OUTPUT_BYTES as u64).read_to_end(&mut out);
    }
    if let Some(stderr) = child.stderr.as_mut() {
        let _ = stderr.take(MAX_OUTPUT_BYTES as u64).read_to_end(&mut err);
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    Ok((out, err, status.success()))
}

// ---- the answers, as the tool prints them ----

#[derive(DeJson, Debug)]
struct StatusJson {
    running: Option<bool>,
    logged_in: Option<bool>,
}

#[derive(DeJson, Debug)]
struct ItemsJson {
    items: Vec<ItemJson>,
    total: Option<u32>,
}

#[derive(DeJson, Debug)]
struct ItemJson {
    uri: Option<String>,
    name: Option<String>,
    #[rename(type)]
    kind: Option<String>,
    artists: Option<Vec<String>>,
    depth: Option<u32>,
}

#[derive(DeJson, Debug)]
struct PlaylistJson {
    uri: Option<String>,
    name: Option<String>,
    description: Option<String>,
    owner: Option<String>,
    track_count: Option<u32>,
    tracks: Option<Vec<ItemTrackJson>>,
}

#[derive(DeJson, Debug)]
struct ItemTrackJson {
    uri: Option<String>,
    name: Option<String>,
    artists: Option<Vec<String>>,
    album: Option<String>,
}

#[derive(DeJson, Debug)]
struct LookupJson {
    entities: Vec<EntityJson>,
}

#[derive(DeJson, Debug)]
struct EntityJson {
    uri: Option<String>,
    parent: Option<ParentJson>,
    contributors: Option<Vec<ContributorJson>>,
    image_url: Option<String>,
    metadata: Option<Vec<FieldJson>>,
}

#[derive(DeJson, Debug)]
struct ParentJson {
    uri: Option<String>,
    name: Option<String>,
}

#[derive(DeJson, Debug)]
struct ContributorJson {
    uri: Option<String>,
}

#[derive(DeJson, Debug)]
struct FieldJson {
    field: Option<String>,
    value: Option<String>,
}

#[derive(DeJson, Debug)]
struct NowJson {
    currently_playing: Option<NowInnerJson>,
}

#[derive(DeJson, Debug)]
struct NowInnerJson {
    description: Option<String>,
    uri: Option<String>,
    context_description: Option<String>,
    is_playing: Option<bool>,
}

#[derive(DeJson, Debug)]
struct QueueJson {
    currently_playing: Option<QueueItemJson>,
    next_tracks: Option<Vec<QueueItemJson>>,
}

#[derive(DeJson, Debug)]
struct QueueItemJson {
    uri: Option<String>,
    name: Option<String>,
    artist: Option<String>,
    provider: Option<String>,
}

#[derive(DeJson, Debug)]
struct DevicesJson {
    devices: Vec<DeviceJson>,
}

#[derive(DeJson, Debug)]
struct DeviceJson {
    device_id: Option<String>,
    name: Option<String>,
    device_type: Option<String>,
    volume: Option<f64>,
    is_active: Option<bool>,
    /// This machine's own Spotify (older CLIs said only `is_local`).
    is_self: Option<bool>,
    is_local: Option<bool>,
}

#[derive(DeJson, Debug)]
struct SearchJson {
    tracks: Option<Vec<SearchItemJson>>,
    artists: Option<Vec<SearchItemJson>>,
    albums: Option<Vec<SearchItemJson>>,
    playlists: Option<Vec<SearchItemJson>>,
}

#[derive(DeJson, Debug)]
struct SearchItemJson {
    uri: Option<String>,
    name: Option<String>,
    artists: Option<Vec<String>>,
    author: Option<String>,
}

// ---- answers into the source's types ----

pub(crate) fn parse_library(folders: &str) -> Result<Vec<LibraryEntry>, String> {
    let folders = ItemsJson::deserialize_json_lenient(folders).map_err(|error| format!("folder list: {error:?}"))?;
    Ok(folders
        .items
        .into_iter()
        .filter_map(|item| {
            let kind = item.kind.unwrap_or_default();
            let is_folder = kind == "folder";
            (is_folder || kind == "playlist").then(|| LibraryEntry {
                uri: item.uri.unwrap_or_default(),
                name: item.name.unwrap_or_default(),
                is_folder,
                depth: item.depth.unwrap_or(1).clamp(1, 32) as u8,
            })
        })
        .filter(|entry| !entry.uri.is_empty())
        .collect())
}

/// One page of a playlist, and how many items the page listed (local files
/// and episodes included, which are left out of the tracks).
pub(crate) fn parse_playlist(text: &str) -> Result<(Playlist, usize), String> {
    let json = PlaylistJson::deserialize_json_lenient(text).map_err(|error| format!("playlist: {error:?}"))?;
    let listed = json.tracks.as_ref().map_or(0, Vec::len);
    let playlist = Playlist {
        uri: json.uri.unwrap_or_default(),
        name: json.name.unwrap_or_default(),
        owner: json.owner.unwrap_or_default(),
        description: json.description.unwrap_or_default(),
        track_count: json.track_count.unwrap_or(0),
        tracks: json
            .tracks
            .unwrap_or_default()
            .into_iter()
            .filter_map(|track| {
                let uri = track.uri?;
                // Local files and podcast episodes in a playlist are not
                // tracks this app can look up or play by URI.
                uri.starts_with("spotify:track:").then(|| PlaylistTrack {
                    uri,
                    name: track.name.unwrap_or_default(),
                    artists: track.artists.unwrap_or_default(),
                    album: track.album.unwrap_or_default(),
                })
            })
            .collect(),
    };
    Ok((playlist, listed))
}

/// The station of a track or artist URI: `spotify:track:<id>` is
/// `spotify:station:track:<id>`. `None` for anything else.
pub(crate) fn radio_station(seed: &str) -> Option<String> {
    let rest = seed.strip_prefix("spotify:")?;
    let (kind, id) = rest.split_once(':')?;
    (matches!(kind, "track" | "artist") && !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| format!("spotify:station:{kind}:{id}"))
}

pub(crate) fn parse_lookup(text: &str) -> Result<Vec<TrackDetail>, String> {
    let json = LookupJson::deserialize_json_lenient(text).map_err(|error| format!("lookup: {error:?}"))?;
    Ok(json
        .entities
        .into_iter()
        .filter_map(|entity| {
            let uri = entity.uri?;
            let metadata = entity.metadata.unwrap_or_default();
            let field = |name: &str| metadata.iter().find(|field| field.field.as_deref() == Some(name)).and_then(|field| field.value.as_deref());
            let duration_ms = field("duration").and_then(|value| value.parse::<u64>().ok()).map(|ms| ms.min(u32::MAX as u64) as u32).unwrap_or(0);
            let release = field("release_date").map(release_key).unwrap_or(0);
            Some(TrackDetail {
                uri,
                duration_ms,
                art_url: entity.image_url.unwrap_or_default(),
                album: entity.parent.as_ref().and_then(|parent| parent.name.clone()).unwrap_or_default(),
                album_uri: entity.parent.and_then(|parent| parent.uri).unwrap_or_default(),
                release,
                artist_uri: entity.contributors.and_then(|c| c.into_iter().find_map(|c| c.uri)).unwrap_or_default(),
            })
        })
        .collect())
}

pub(crate) fn parse_now_playing(now: &str, queue: &str) -> Result<(Option<NowPlaying>, Vec<QueueEntry>), String> {
    let now = NowJson::deserialize_json_lenient(now).map_err(|error| format!("now-playing: {error:?}"))?;
    let queue = QueueJson::deserialize_json_lenient(queue).map_err(|error| format!("queue: {error:?}"))?;
    let entry = |item: QueueItemJson| QueueEntry {
        uri: item.uri.unwrap_or_default(),
        name: item.name.unwrap_or_default(),
        artist: item.artist.unwrap_or_default(),
        queued: item.provider.as_deref() == Some("queue"),
    };
    let current = queue.currently_playing.map(entry);
    let mut next: Vec<QueueEntry> = queue.next_tracks.unwrap_or_default().into_iter().map(entry).collect();
    // The listing can start with the track playing now, again: shown, not
    // queued (a `queue remove 0` takes the entry after it), so it is not
    // one of what plays next.
    if next.first().is_some_and(|first| !first.queued && current.as_ref().is_some_and(|current| current.uri == first.uri)) {
        next.remove(0);
    }
    let playing = now.currently_playing.map(|now| {
        let uri = now.uri.unwrap_or_default();
        // The queue names title and artist apart; now-playing only joins
        // them ("Title — Artist"), so it is the fallback.
        let (title, artist) = match &current {
            Some(current) if current.uri == uri => (current.name.clone(), current.artist.clone()),
            _ => {
                let description = now.description.unwrap_or_default();
                match description.rsplit_once(" — ") {
                    Some((title, artist)) => (title.to_string(), artist.to_string()),
                    None => (description, String::new()),
                }
            }
        };
        NowPlaying { uri, title, artist, context: now.context_description.unwrap_or_default(), is_playing: now.is_playing.unwrap_or(false) }
    });
    Ok((playing, next))
}

pub(crate) fn parse_devices(text: &str) -> Result<Vec<Device>, String> {
    let json = DevicesJson::deserialize_json_lenient(text).map_err(|error| format!("devices: {error:?}"))?;
    Ok(json
        .devices
        .into_iter()
        .filter_map(|device| {
            Some(Device {
                id: device.device_id?,
                name: device.name.unwrap_or_default(),
                kind: device.device_type.unwrap_or_default(),
                volume: (device.volume.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) as f32,
                active: device.is_active.unwrap_or(false),
                this_machine: device.is_self.or(device.is_local).unwrap_or(false),
            })
        })
        .collect())
}

pub(crate) fn parse_search(text: &str) -> Result<CatalogResults, String> {
    let json = SearchJson::deserialize_json_lenient(text).map_err(|error| format!("search: {error:?}"))?;
    let items = |list: Option<Vec<SearchItemJson>>| -> Vec<CatalogItem> {
        list.unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                Some(CatalogItem {
                    uri: item.uri?,
                    name: item.name.unwrap_or_default(),
                    subtitle: item.artists.filter(|a| !a.is_empty()).map(|a| a.join(", ")).or(item.author).unwrap_or_default(),
                })
            })
            .collect()
    };
    Ok(CatalogResults { tracks: items(json.tracks), artists: items(json.artists), albums: items(json.albums), playlists: items(json.playlists) })
}

fn saved(items: Vec<ItemJson>) -> Vec<SavedItem> {
    items
        .into_iter()
        .filter_map(|item| {
            Some(SavedItem { uri: item.uri?, name: item.name.unwrap_or_default(), artists: item.artists.unwrap_or_default().join(", ") })
        })
        .collect()
}

impl MusicSource for Spotify {
    fn id(&self) -> SourceId {
        SourceId::Spotify
    }

    /// The client's macOS bundle id on every platform: the audio route
    /// maps it to the client's process there (on Windows `Spotify.exe`, or
    /// the Store package SpotifyAB.SpotifyMusic).
    fn player_bundle_id(&self) -> Option<&'static str> {
        Some("com.spotify.client")
    }

    fn ready(&mut self) -> Result<bool, String> {
        let status = self.json::<StatusJson>(&["status"], &[])?;
        Ok(status.running.unwrap_or(false) && status.logged_in.unwrap_or(false))
    }

    fn library(&mut self) -> Result<Vec<LibraryEntry>, String> {
        // `folder list` is the tree in the sidebar's order, with depths.
        parse_library(&self.run(&["folder", "list"], &[])?)
    }

    fn saved_albums(&mut self) -> Result<Vec<SavedItem>, String> {
        Ok(saved(self.items("album")?))
    }

    fn saved_artists(&mut self) -> Result<Vec<SavedItem>, String> {
        Ok(saved(self.items("artist")?))
    }

    /// A playlist in pages of [`TRACK_PAGE`] (today the tool lists a whole
    /// playlist unasked, but nothing promises it).
    fn playlist(&mut self, uri: &str) -> Result<Playlist, String> {
        let limit = TRACK_PAGE.to_string();
        let mut offset = 0;
        let mut playlist: Option<Playlist> = None;
        loop {
            let at = offset.to_string();
            let (page, listed) = parse_playlist(&self.run(&["playlist", "get", uri], &["--limit", &limit, "--offset", &at])?)?;
            offset += listed;
            match &mut playlist {
                Some(playlist) => playlist.tracks.extend(page.tracks),
                None => playlist = Some(page),
            }
            if listed < TRACK_PAGE {
                return Ok(playlist.unwrap_or_default());
            }
        }
    }

    fn lookup(&mut self, uris: &[String]) -> Result<Vec<TrackDetail>, String> {
        let mut args = vec!["lookup"];
        args.extend(uris.iter().take(LOOKUP_BATCH).map(String::as_str));
        parse_lookup(&self.run(&args, &["--fields", "duration,release_date"])?)
    }

    fn search(&mut self, query: &str) -> Result<CatalogResults, String> {
        // A query starting with a dash reads as a flag and gets the usage
        // text back; a leading space keeps it a query.
        let query = if query.starts_with('-') { format!(" {query}") } else { query.to_string() };
        parse_search(&self.run(&["search", &query], &["--limit", "10"])?)
    }

    /// Two commands: now-playing knows the play state, the queue knows
    /// the names.
    fn playback(&mut self) -> Result<(Option<NowPlaying>, Vec<QueueEntry>), String> {
        let now = self.run(&["now-playing"], &[])?;
        let queue = self.run(&["queue"], &[])?;
        parse_now_playing(&now, &queue)
    }

    /// A track's or artist's station: `playlist get` answers a station URI
    /// with its tracks (50) and plays nothing.
    fn radio(&mut self, seed: &str) -> Option<Result<Vec<PlaylistTrack>, String>> {
        let station = radio_station(seed)?;
        Some(self.run(&["playlist", "get", &station], &[]).and_then(|text| parse_playlist(&text)).map(|(playlist, _)| playlist.tracks))
    }

    fn devices(&mut self) -> Result<Vec<Device>, String> {
        parse_devices(&self.run(&["devices", "list"], &[])?)
    }

    fn transport(&mut self, command: &Transport) -> Result<(), String> {
        let number;
        let args: Vec<&str> = match command {
            Transport::Play(uri) => vec!["play", uri],
            Transport::Resume => vec!["resume"],
            Transport::Pause => vec!["pause"],
            Transport::Next => vec!["next"],
            Transport::Previous => vec!["previous"],
            Transport::Seek(ms) => {
                number = ms.to_string();
                vec!["seek", &number]
            }
            Transport::Shuffle(on) => vec!["shuffle", if *on { "on" } else { "off" }],
            Transport::Repeat(mode) => vec![
                "repeat",
                match mode {
                    Repeat::Off => "off",
                    Repeat::Context => "context",
                    Repeat::Track => "track",
                },
            ],
            Transport::Volume(level) => {
                number = format!("{:.3}", level.clamp(0.0, 1.0));
                vec!["volume", &number]
            }
            Transport::Transfer(device) => vec!["devices", "transfer", device],
            Transport::Enqueue(uri) => vec!["queue", "add", uri],
            Transport::Dequeue(position) => {
                number = position.to_string();
                vec!["queue", "remove", &number]
            }
        };
        self.run(&args, &[]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_reads_with_its_tracks() {
        let (playlist, listed) = parse_playlist(include_str!("../../tests/playlist.json")).unwrap();
        // The page lists an episode too, which is not a track.
        assert_eq!(listed, 4);
        assert_eq!(playlist.name, "Starred");
        assert_eq!(playlist.track_count, 1149);
        assert_eq!(playlist.tracks.len(), 3);
        assert_eq!(playlist.tracks[1].name, "Toxygene - 7\" Edit");
        assert_eq!(playlist.tracks[1].artists, vec!["The Orb".to_string()]);
        assert_eq!(playlist.tracks[1].album, "Toxygene");
    }

    #[test]
    fn a_lookup_gives_length_art_and_album() {
        let details = parse_lookup(include_str!("../../tests/lookup.json")).unwrap();
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].duration_ms, 300_000);
        assert_eq!(details[0].release, 19_950_000);
        // No release date given: unknown, not a guess.
        assert_eq!(details[1].release, 0);
        assert_eq!(details[0].album_uri, "spotify:album:49CWPw9MGkEvc78gTjLnEe");
        assert!(details[0].art_url.starts_with("https://i.scdn.co/image/"));
        assert_eq!(details[0].artist_uri, "spotify:artist:0RhnKJThw36HFitqY6dYgd");
    }

    #[test]
    fn the_library_tree_keeps_playlists_and_folders_in_order() {
        let library = parse_library(include_str!("../../tests/folders.json")).unwrap();
        assert_eq!(library.len(), 3);
        assert_eq!(library[0].name, "Toxygene - 7\" Edit Radio");
        assert!(library[1].is_folder);
        assert_eq!(library[2].depth, 2);
    }

    #[test]
    fn now_playing_takes_names_from_the_queue() {
        let (now, next) = parse_now_playing(include_str!("../../tests/now_playing.json"), include_str!("../../tests/queue.json")).unwrap();
        let now = now.unwrap();
        assert_eq!(now.title, "I'm Free - Full Length La Serrena");
        assert_eq!(now.artist, "Morgan King");
        assert_eq!(now.context, "Starred");
        assert!(now.is_playing);
        assert_eq!(next.len(), 2);
        assert_eq!(next[0].name, "Balloon");
        // Queued by hand, then what the playlist brings.
        assert!(next[0].queued);
        assert!(!next[1].queued);
        // A listing that starts with the playing track again drops it.
        let (_, next) = parse_now_playing(include_str!("../../tests/now_playing.json"), include_str!("../../tests/queue_repeat.json")).unwrap();
        assert_eq!(next.len(), 2);
        assert_eq!(next[0].name, "Club Bizarre");
        assert!(next[0].queued);
    }

    #[test]
    fn tracks_and_artists_have_stations() {
        assert_eq!(radio_station("spotify:track:4fHug3zCanK33sdvticISi").as_deref(), Some("spotify:station:track:4fHug3zCanK33sdvticISi"));
        assert_eq!(radio_station("spotify:artist:0Rhn").as_deref(), Some("spotify:station:artist:0Rhn"));
        assert_eq!(radio_station("spotify:playlist:x"), None);
        assert_eq!(radio_station("spotify:track:a b"), None);
        assert_eq!(radio_station("local:track:x"), None);
    }

    #[test]
    fn devices_and_search_read() {
        let devices = parse_devices(include_str!("../../tests/devices.json")).unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].active);
        assert_eq!(devices[0].volume, 1.0);
        assert!(devices[0].this_machine);
        // Cast to a speaker: this Mac is listed, the speaker plays.
        let cast = parse_devices(include_str!("../../tests/devices_remote.json")).unwrap();
        let active = cast.iter().find(|d| d.active).unwrap();
        assert_eq!(active.name, "Kitchen speaker");
        assert!(!active.this_machine);
        assert!(cast.iter().any(|d| d.this_machine && !d.active));
        let results = parse_search(include_str!("../../tests/search.json")).unwrap();
        assert_eq!(results.artists[0].name, "Kiasmos");
        assert_eq!(results.playlists[0].subtitle, "Spotify");
        assert_eq!(results.tracks[0].subtitle, "The Orb");
    }
}
