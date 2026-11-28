//! Finding running endpoints by their discovery files (`makepad-remote-<pid>.json` in the user's
//! registry dir, [`registry_dir`]). Files of processes that ended are removed when listed.
//!
//! A pid says which process is meant. A bare name or `all` does not: on a machine where someone
//! works it would pick their app (one that drives a real printer, say) for whoever mistypes a
//! command. So those reach only what the [`Caller`] started, and everything else is named by pid.

use std::path::{Path, PathBuf};

/// The startup line that tells a caller how to reach this app.
pub fn startup_line(tool: &str, pid: u32, app: &str, socket: &Path) -> String {
    format!("[{tool}-remote] pid={pid} app={app} socket={}", socket.display())
}

/// Reads a [`startup_line`], keeping the entire socket path, including spaces.
pub fn parse_startup_line(line: &str) -> Option<(u32, String, PathBuf)> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (tag, fields) = line.strip_prefix('[')?.split_once(']')?;
    let tool = tag.strip_suffix("-remote")?;
    if tool.is_empty() || tool.contains(char::is_whitespace) {
        return None;
    }
    let (pid, fields) = fields.strip_prefix(" pid=")?.split_once(" app=")?;
    let (app, socket) = fields.split_once(" socket=")?;
    if app.is_empty() || socket.is_empty() {
        return None;
    }
    Some((pid.parse().ok()?, app.to_owned(), PathBuf::from(socket)))
}

/// Marks every process a lease's runs start: a process that moved to its own session is still
/// stopped with the lease, a WM with it is a lane's test desktop, not its node's, and an app with
/// it says so in its discovery file (`lease`), which makes it that lease's own.
pub const LEASE_ENV: &str = "MAKEPAD_DEVHUB_LEASE";

/// Who asks for an app without saying which one.
pub struct Caller {
    /// may reach any app by name
    any: bool,
    lease: Option<String>,
}

impl Caller {
    /// A program, which reaches by name only what its own lease started.
    pub fn program() -> Caller {
        Caller { any: false, lease: own_lease() }
    }

    /// A command line. Someone typing at a terminal sees what they mean; an agent's shell has no
    /// terminal, and one that does still has `DEVHUB_AGENT`. So nothing here can be set to reach
    /// further, only to reach less.
    pub fn command_line() -> Caller {
        use std::io::IsTerminal;
        Caller { any: std::io::stdin().is_terminal() && std::env::var_os("DEVHUB_AGENT").is_none(), lease: own_lease() }
    }

    /// An adapter set up once for one app (`makepad-remote mcp <app> --launch ..`, an installed
    /// bundle): the name is its configuration, not a command somebody typed.
    pub fn set_up_for_app() -> Caller {
        Caller { any: true, lease: None }
    }

    /// Whether `app` is one this caller may reach without naming its pid.
    pub fn reaches(&self, app: &App) -> bool {
        self.any || (self.lease.is_some() && app.text("lease") == self.lease)
    }

    /// Why `all` is refused: with no lease and no terminal it has no apps of its own to mean.
    pub fn all_refused(&self, list: &[App]) -> Option<String> {
        if self.any || self.lease.is_some() {
            return None;
        }
        let there: Vec<String> = list.iter().map(|a| format!("{} {}", a.pid, a.app)).collect();
        Some(format!("`all` would reach every app here ({}), and this command started none of them. Say which you mean: a pid", there.join(", ")))
    }
}

fn own_lease() -> Option<String> {
    std::env::var(LEASE_ENV).ok().filter(|l| !l.is_empty())
}

/// This process's `,"lease":..` discovery field (none outside a lease).
pub(crate) fn lease_field() -> String {
    own_lease().map_or(String::new(), |l| format!(",\"lease\":{}", crate::cmd::json_str(&l)))
}


#[derive(Debug, Clone)]
pub struct App {
    pub pid: u32,
    pub app: String,
    pub socket: String,
    pub started_ms: u64,
    pub caps: Vec<String>,
    /// Windows, macOS: the thin parent this run stands behind (restart_parent.rs in the JIT), whose
    /// pid is the one whoever started the run holds, and the one that stays when the run restarts
    pub restart_parent: Option<u32>,
    /// every top-level field as (key, raw JSON value), the host's extra fields included
    pub fields: Vec<(String, String)>,
}

impl App {
    /// Whether `pid` means this endpoint: its process, or the parent that process stands behind.
    pub fn answers_to(&self, pid: u32) -> bool {
        self.pid == pid || self.restart_parent == Some(pid)
    }

    /// A string field (the host's extras such as `session_log`), unescaped.
    pub fn text(&self, key: &str) -> Option<String> {
        let raw = &self.fields.iter().find(|f| f.0 == key)?.1;
        let mut i = 0;
        string(raw.as_bytes(), &mut i)
    }

    /// Main window title as supplied by the application, without the remote marker.
    pub fn title(&self) -> Option<String> { self.text("title") }

    /// Visible window IDs and their titles, in window order. Older endpoints advertise none.
    pub fn visible_windows(&self) -> Vec<(usize, String)> {
        let Some((_, raw)) = self.fields.iter().find(|(key, _)| key == "windows") else { return Vec::new() };
        let mut windows: Vec<_> = fields(raw).into_iter().filter_map(|(id, title)|
            Some((id.parse().ok()?, string(title.as_bytes(), &mut 0)?))).collect();
        windows.sort_by_key(|(id, _)| *id);
        windows
    }

    /// Whether the endpoint serves `service` (any version).
    pub fn serves(&self, service: &str) -> bool {
        self.caps.iter().any(|c| c.split('/').next() == Some(service))
    }

    /// Why nothing can reach this process: it runs, but its socket could not be bound
    /// (`"socket_error"`, carrier/unix.rs `listen`).
    pub fn unreachable(&self) -> Option<String> {
        self.text("socket_error").map(|why| format!("pid {} ({}) runs but cannot be reached: {}", self.pid, self.app, why))
    }
}

pub(crate) fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // EPERM, or EACCES under SELinux (Android: a run-as shell and the app it inspects are
        // different domains): the process exists but may not be signalled -- alive, not a stale
        // discovery file to remove
        unsafe { kill(pid as i32, 0) == 0 || matches!(std::io::Error::last_os_error().raw_os_error(), Some(1) | Some(13)) }
    }
    #[cfg(windows)]
    {
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
            fn GetExitCodeProcess(h: *mut core::ffi::c_void, code: *mut u32) -> i32;
            fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
        }
        // PROCESS_QUERY_LIMITED_INFORMATION; STILL_ACTIVE = 259
        let h = unsafe { OpenProcess(0x1000, 0, pid) };
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0 && code == 259;
        unsafe { CloseHandle(h) };
        ok
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

pub(crate) fn string(b: &[u8], i: &mut usize) -> Option<String> {
    if b.get(*i) != Some(&b'"') {
        return None;
    }
    *i += 1;
    let mut out: Vec<u8> = Vec::new();
    while *i < b.len() {
        match b[*i] {
            b'"' => {
                *i += 1;
                return String::from_utf8(out).ok();
            }
            b'\\' => {
                *i += 1;
                match *b.get(*i)? {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'u' => {
                        let h = std::str::from_utf8(b.get(*i + 1..*i + 5)?).ok()?;
                        let c = char::from_u32(u32::from_str_radix(h, 16).ok()?).unwrap_or('\u{fffd}');
                        out.extend_from_slice(c.to_string().as_bytes());
                        *i += 4;
                    }
                    c => out.push(c),
                }
                *i += 1;
            }
            c => {
                out.push(c);
                *i += 1;
            }
        }
    }
    None
}

/// The top-level `"key": value` pairs of one JSON object (values kept as raw text).
pub(crate) fn fields(text: &str) -> Vec<(String, String)> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() && b[i] != b'{' {
        i += 1;
    }
    i += 1;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        if i >= b.len() || b[i] == b'}' {
            return out;
        }
        let Some(key) = string(b, &mut i) else { return out };
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b':') {
            i += 1;
        }
        let start = i;
        let mut depth = 0i32;
        while i < b.len() {
            match b[i] {
                b'"' => {
                    if string(b, &mut i).is_none() {
                        return out;
                    }
                    continue;
                }
                b'[' | b'{' => depth += 1,
                b']' | b'}' if depth == 0 => break,
                b']' | b'}' => depth -= 1,
                b',' if depth == 0 => break,
                _ => {}
            }
            i += 1;
        }
        out.push((key, text[start..i].trim().to_string()));
    }
}

fn list_of_strings(raw: &str) -> Vec<String> {
    let b = raw.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i] == b'"' {
            match string(b, &mut i) {
                Some(s) => out.push(s),
                None => break,
            }
        } else {
            i += 1;
        }
    }
    out
}

pub fn parse(text: &str) -> Option<App> {
    let fields = fields(text);
    let get = |k: &str| fields.iter().find(|f| f.0 == k).map(|f| f.1.clone());
    let text_of = |k: &str| get(k).and_then(|r| string(r.as_bytes(), &mut 0)).unwrap_or_default();
    Some(App {
        pid: get("pid")?.parse().ok()?,
        app: text_of("app"),
        socket: text_of("socket"),
        started_ms: get("started_ms").and_then(|v| v.parse().ok()).unwrap_or(0),
        caps: get("caps").map(|r| list_of_strings(&r)).unwrap_or_default(),
        restart_parent: get("restart_parent").and_then(|v| v.parse().ok()),
        fields,
    })
}

/// Replaces the `caps` of a discovery file (a service registered after the endpoint started
/// listening): written to a temporary file and renamed, so a reader never sees half a file.
pub(crate) fn rewrite_caps(file: &std::path::Path, caps: &[String]) {
    let list = caps.iter().map(|c| crate::cmd::json_str(c)).collect::<Vec<_>>().join(",");
    let _ = rewrite_fields(file, &[("caps", format!("[{list}]"))]);
}

fn window_fields(title: Option<&str>, windows: &[(usize, String)]) -> [(&'static str, String); 2] {
    let windows = windows.iter().map(|(id, title)| format!(r#""{id}":{}"#, crate::cmd::json_str(title)))
        .collect::<Vec<_>>().join(",");
    [("title", title.map_or_else(|| "null".into(), crate::cmd::json_str)), ("windows", format!("{{{windows}}}"))]
}

/// Publish a window snapshot in this process's discovery file. Call off the UI thread.
pub fn publish_windows(title: Option<&str>, windows: &[(usize, String)]) -> std::io::Result<()> {
    let file = crate::carrier::unix::paths(std::process::id()).1;
    rewrite_fields(&file, &window_fields(title, windows))
}

fn rewrite_fields(file: &Path, updates: &[(&str, String)]) -> std::io::Result<()> {
    use std::io::Write;
    // In a hosted run, the endpoint and the application may hold different copies of this
    // library. Serialize their read-modify-renames so a late registration preserves the windows.
    #[cfg(unix)]
    let dir = crate::carrier::unix::own_socket_dir()?;
    #[cfg(not(unix))]
    let dir = crate::carrier::unix::socket_dir();
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(dir.join(".discovery-write.lock"))?;
    lock.lock()?;
    let mut entries = fields(&std::fs::read_to_string(file)?);
    entries.retain(|(name, _)| !updates.iter().any(|(key, _)| name == key));
    entries.extend(updates.iter().map(|(key, value)| (key.to_string(), value.clone())));
    let body = entries.iter().map(|(key, value)| format!("{}:{value}", crate::cmd::json_str(key)))
        .collect::<Vec<_>>().join(",");
    let tmp = file.with_extension("json.new");
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    // A previous writer can have exited before its rename.
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut out = options.open(&tmp)?;
    let result = writeln!(out, "{{{body}}}");
    drop(out);
    let result = result.and_then(|()| std::fs::rename(&tmp, file));
    if result.is_err() { let _ = std::fs::remove_file(tmp); }
    result
}

/// Where every process of this user registers its endpoint: one directory whatever the
/// process's TMPDIR. The socket itself is in the user's socket dir
/// ([`crate::carrier::unix::socket_dir`]; its path is in the file).
/// A makepad WM gives its JIT apps an on-disk TMPDIR (`~/.makepad/wm/jit.env`), and `makepad
/// inspect` from a shell has another: with the files in the temp dir it found none of them
/// (2026-10-25). Linux (and other Unix with a session): `$XDG_RUNTIME_DIR`, the user's 0700
/// runtime dir; elsewhere the temp dir (macOS's is per user already). This crate's own tests use
/// [`crate::carrier::unix::test_root`] instead.
pub fn registry_dir() -> std::path::PathBuf {
    #[cfg(not(test))]
    {
        registry_dir_of(std::env::var_os("MAKEPAD_REMOTE_REGISTRY"), std::env::var_os("XDG_RUNTIME_DIR"))
    }
    #[cfg(test)]
    {
        crate::carrier::unix::test_root()
    }
}

/// [`registry_dir`] from `MAKEPAD_REMOTE_REGISTRY` and `XDG_RUNTIME_DIR`.
fn registry_dir_of(registry: Option<std::ffi::OsString>, xdg: Option<std::ffi::OsString>) -> std::path::PathBuf {
    // a run whose TMPDIR is its own (a devhub lease on a Mac) still announces where its node's hub looks
    if let Some(dir) = registry.map(std::path::PathBuf::from).filter(|d| d.is_absolute()) {
        return dir;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(dir) = xdg.map(std::path::PathBuf::from).filter(|d| d.is_absolute() && d.is_dir()) {
        return dir;
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    drop(xdg);
    std::env::temp_dir()
}

/// The running endpoints, oldest first by pid (stale discovery files of ended processes are
/// removed, with the socket each names).
pub fn apps() -> Vec<App> {
    apps_in(&registry_dir())
}

/// [`apps`] announced in `registry` (what a process started with `MAKEPAD_REMOTE_REGISTRY` names).
pub fn apps_in(registry: &std::path::Path) -> Vec<App> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(registry) else { return out };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("makepad-remote-").and_then(|r| r.strip_suffix(".json")).and_then(|p| p.parse::<u32>().ok()) else { continue };
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let app = parse(&text);
        if !alive(pid) {
            let _ = std::fs::remove_file(e.path());
            if let Some(socket) = app.as_ref().map(|a| &a.socket).filter(|s| s.ends_with(&format!("makepad-remote-{pid}.sock"))) {
                let _ = std::fs::remove_file(socket);
            }
            continue;
        }
        if let Some(app) = app {
            out.push(app);
        }
    }
    out.sort_by_key(|a| a.pid);
    out
}

/// The running endpoint of `pid`, or of the parent it stands behind: [`App::answers_to`].
pub fn by_pid(pid: u32) -> Option<App> {
    apps().into_iter().find(|a| a.answers_to(pid))
}

/// The app `name` reaches for `caller`: of several with that name the most recent (highest pid)
/// it may reach.
/// Err: no app has that name, or the one it would reach is not this caller's, and says which.
pub fn named(list: &[App], name: &str, caller: &Caller) -> Result<App, String> {
    let same: Vec<&App> = list.iter().filter(|a| a.app == name).collect();
    if let Some(a) = same.iter().rev().find(|a| caller.reaches(a)) {
        return Ok((*a).clone());
    }
    let Some(newest) = same.last() else { return Err(no_endpoint(name)) };
    let mins = crate::cmd::wall_ms().saturating_sub(newest.started_ms) / 60_000;
    let more = match same.len() {
        1 => String::new(),
        n => format!(", the newest of {n}"),
    };
    Err(format!(
        "`{name}` would reach pid {} (running {mins} min{more}), which this command did not start. Say which you mean: the pid in place of the name, `<node>:<pid>` for another machine; `list` shows them",
        newest.pid
    ))
}

fn no_endpoint(spec: &str) -> String {
    format!("no running makepad-remote endpoint `{spec}`")
}

/// Lists [`apps`] on a thread of its own, which removes the files killed launches left in a phone
/// app's temp dir. A desktop keeps them, since the devhub reads one left behind as a crash.
#[cfg(any(target_os = "android", target_os = "ios", target_os = "tvos"))]
pub(crate) fn remove_stale(hooks: crate::conn::Hooks) {
    let _ = std::thread::Builder::new().name("makepad-remote-sweep".into()).spawn(move || {
        (hooks.enter)();
        let _ = apps();
        (hooks.exit)();
    });
}

/// `spec` is a pid, an app name for `caller` ([`named`]), or a
/// `tcp:` socket a discovery file names.
pub fn find(spec: &str, caller: &Caller) -> Result<App, String> {
    let list = apps();
    // a loopback endpoint as its discovery file names it (Windows): the file carries its token
    if spec.starts_with("tcp:") {
        return list.into_iter().rfind(|a| a.socket == spec).ok_or_else(|| no_endpoint(spec));
    }
    // an endpoint file an app published in a directory it owns ([`publish`]): like a pid, an
    // explicit choice, so any caller may use it
    if is_endpoint_file(spec) {
        return published(std::path::Path::new(spec), &list);
    }
    let app = match spec.parse::<u32>() {
        Ok(pid) => list.into_iter().find(|a| a.answers_to(pid)).ok_or_else(|| no_endpoint(spec)),
        Err(_) => named(&list, spec, caller),
    }?;
    match app.unreachable() {
        Some(why) => Err(why),
        None => Ok(app),
    }
}

/// Whether a target names an endpoint file (a path) rather than a pid, a name or a socket.
pub fn is_endpoint_file(spec: &str) -> bool {
    (spec.contains('/') || spec.contains('\\')) && !is_socket_path(spec)
}

/// Whether a target is an endpoint's socket: a path ending in `.sock` (as [`crate::carrier::unix`]
/// names them), or one that is a socket on disk.
pub fn is_socket_path(spec: &str) -> bool {
    if spec.ends_with(".sock") {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(meta) = std::fs::metadata(spec) {
            return meta.file_type().is_socket();
        }
    }
    false
}

/// Writes `path`, a file naming this process's endpoint (its pid, app and start time), for an app
/// that owns a directory (a home it holds the lock of) so that callers can find "the instance of
/// this home" across restarts by naming the file as their target. Another instance with another
/// home can't be confused with it, and a reused pid doesn't match the start time. Written beside
/// and renamed, so a reader never sees half a file; 0600 on Unix. Not a secret: any process of the
/// user can read it, as it can list the registry.
pub fn publish(path: &std::path::Path) -> std::io::Result<()> {
    let pid = std::process::id();
    let me = apps()
        .into_iter()
        .find(|a| a.pid == pid)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "this process has no remote endpoint yet"))?;
    let tmp = path.with_extension("new");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, published_line(&me).as_bytes())?;
    }
    std::fs::rename(&tmp, path)
}

/// The line [`publish`] writes for `app`.
fn published_line(app: &App) -> String {
    let [(_, title), (_, windows)] = window_fields(app.title().as_deref(), &app.visible_windows());
    format!("{{\"pid\":{},\"app\":{},\"started_ms\":{},\"title\":{title},\"windows\":{windows}}}\n", app.pid, crate::cmd::json_str(&app.app), app.started_ms)
}

/// The running endpoint an endpoint file names: the same pid and the same start time.
fn published(path: &std::path::Path, list: &[App]) -> Result<App, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("no endpoint file {}: {e}", path.display()))?;
    let named = parse(text.lines().next().unwrap_or("")).ok_or_else(|| format!("{} is not an endpoint file", path.display()))?;
    let app = list
        .iter()
        .find(|a| a.pid == named.pid && a.started_ms == named.started_ms)
        .cloned()
        .ok_or_else(|| format!("the {} that wrote {} (pid {}) is not running", named.app, path.display(), named.pid))?;
    match app.unreachable() {
        Some(why) => Err(why),
        None => Ok(app),
    }
}

#[cfg(test)]
mod published_tests {
    use super::*;

    #[test]
    fn startup_line_identifies_the_remote_endpoint() {
        let socket = Path::new("/tmp/app logs/test.sock");
        assert_eq!(
            startup_line("makepad", 42, "test", socket),
            "[makepad-remote] pid=42 app=test socket=/tmp/app logs/test.sock",
            "Makepad prints the documented startup line"
        );
        assert_eq!(
            parse_startup_line(&startup_line("relax", 42, "test", socket)),
            Some((42, "test".into(), socket.to_owned())),
            "another tool round-trips the full socket path with spaces"
        );
        assert_eq!(
            parse_startup_line("[makepad-remote] run=40 pid=42"),
            None,
            "the restart parent's line is separate from the startup line"
        );
    }

    #[test]
    fn an_endpoint_file_reaches_its_own_instance_only() {
        let line = |pid: u32, started: u64| format!("{{\"pid\":{pid},\"app\":\"create\",\"socket\":\"/s{pid}\",\"started_ms\":{started},\"caps\":[]}}\n");
        let list = vec![parse(&line(41, 1000)).unwrap(), parse(&line(42, 2000)).unwrap()];
        let dir = crate::carrier::unix::test_root().join(format!("published-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("endpoint");
        let written = published_line(&list[1]);
        assert_eq!(parse(&written).map(|a| (a.pid, a.started_ms, a.app)), Some((42, 2000, "create".into())), "the line round-trips");
        std::fs::write(&file, &written).unwrap();
        let spec = file.to_str().unwrap();
        assert!(is_endpoint_file(spec) && !is_endpoint_file("42") && !is_endpoint_file("create"), "paths only");
        assert!(!is_endpoint_file("/run/user/x/makepad-remote-42.sock"), "a socket path is not an endpoint file");
        assert_eq!(published(&file, &list).map(|a| a.pid), Ok(42), "the instance that wrote it");
        let reused = vec![parse(&line(42, 3000)).unwrap()];
        assert!(published(&file, &reused).unwrap_err().contains("not running"), "a reused pid is not that instance");
        std::fs::write(&file, "not json").unwrap();
        assert!(published(&file, &list).unwrap_err().contains("not an endpoint file"), "garbage is refused");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(published(&file, &list).unwrap_err().contains("no endpoint file"), "a missing file says so");
    }
}

#[cfg(test)]
mod caller_tests {
    use super::{named, parse, App, Caller};

    fn app(pid: u32, name: &str, lease: Option<&str>) -> App {
        let lease = lease.map_or(String::new(), |l| format!(",\"lease\":\"{l}\""));
        parse(&format!("{{\"pid\":{pid},\"app\":\"{name}\",\"socket\":\"/s\",\"caps\":[]{lease}}}\n")).unwrap()
    }

    fn caller(any: bool, lease: Option<&str>) -> Caller {
        Caller { any, lease: lease.map(str::to_string) }
    }

    /// An agent's mistyped command on a machine where someone works: it must not reach their app.
    #[test]
    fn a_bare_name_refuses_an_app_the_caller_did_not_start_and_says_which() {
        let list = [app(7, "tool-a", None), app(9, "tool-a", Some("L1"))];
        for who in [caller(false, None), caller(false, Some("L2"))] {
            let why = named(&list, "tool-a", &who).unwrap_err();
            assert!(why.contains("pid 9") && why.contains("newest of 2") && why.contains("the pid in place of the name"), "{why}");
        }
        assert!(named(&[app(7, "tool-a", None)], "tool-a", &caller(false, None)).unwrap_err().contains("would reach pid 7"));
        assert!(named(&[], "tool-a", &caller(false, None)).unwrap_err().contains("no running"));
    }

    #[test]
    fn a_lease_reaches_its_own_by_name_and_a_person_the_newest() {
        let list = [app(7, "tool-a", Some("L1")), app(9, "tool-a", Some("L2")), app(11, "tool-a", None)];
        assert_eq!(named(&list, "tool-a", &caller(false, Some("L1"))).unwrap().pid, 7);
        assert_eq!(named(&list, "tool-a", &caller(true, None)).unwrap().pid, 11);
    }

    #[test]
    fn all_needs_a_terminal_or_a_lease_of_its_own() {
        let list = [app(7, "tool-a", None)];
        assert!(caller(false, None).all_refused(&list).unwrap().contains("7 tool-a"));
        assert!(caller(false, Some("L1")).all_refused(&list).is_none() && !caller(false, Some("L1")).reaches(&list[0]));
        assert!(caller(true, None).all_refused(&list).is_none() && caller(true, None).reaches(&list[0]));
    }
}

#[cfg(test)]
mod rewrite_tests {
    #[test]
    fn a_run_answers_to_the_parent_it_stands_behind() {
        let run = super::parse("{\"pid\":7,\"app\":\"x\",\"socket\":\"/s\",\"caps\":[],\"restart_parent\":3}\n").unwrap();
        assert!(run.answers_to(7) && run.answers_to(3) && !run.answers_to(4));
        let plain = super::parse("{\"pid\":7,\"app\":\"x\",\"socket\":\"/s\",\"caps\":[]}\n").unwrap();
        assert!(plain.answers_to(7) && !plain.answers_to(3));
    }

    #[test]
    fn caps_follow_a_later_registration() {
        let file = std::env::temp_dir().join(format!("makepad-remote-caps-test-{}.json", std::process::id()));
        std::fs::write(&file, "{\"pid\":1,\"app\":\"x\",\"socket\":\"/s\",\"started_ms\":2,\"caps\":[\"cmd/1\"],\"extra\":3}\n").unwrap();
        let read = || super::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(read().title().is_none() && read().visible_windows().is_empty(), "an older entry has no advertised windows");
        let windows = vec![(2, "Board ]".into()), (7, "Editor · \"notes\"".into())];
        super::rewrite_fields(&file, &super::window_fields(Some("Board ]"), &windows)).unwrap();
        super::rewrite_caps(&file, &["cmd/1".into(), "app/1".into()]);
        let app = super::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(app.serves("app") && app.serves("cmd"));
        for app in [read(), super::parse(&super::published_line(&app)).unwrap()] {
            assert_eq!(app.title().as_deref(), Some("Board ]"));
            assert_eq!(app.visible_windows(), windows);
        }
        super::rewrite_fields(&file, &super::window_fields(None, &[])).unwrap();
        assert!(read().visible_windows().is_empty() && read().serves("app"));
        assert_eq!(app.text("app").as_deref(), Some("x"));
        std::fs::remove_file(&file).unwrap();
    }
}

#[cfg(test)]
mod registry_tests {
    /// The discovery file is in the registry dir, not the temp dir: a process with another TMPDIR
    /// (a WM's JIT apps, `~/.makepad/wm/jit.env`) still lists in `makepad inspect` from a shell.
    /// Its socket is in the socket dir, not the temp dir either (a deep one cannot hold it).
    /// The registry is the one a run is told (`MAKEPAD_REMOTE_REGISTRY`), else on Linux the
    /// session's runtime dir, else the temp dir. This crate's tests announce in neither the
    /// process's own registry nor its socket dir: a live JIT's endpoint is there, under their pid.
    #[test]
    fn discovery_files_go_to_the_registry_dir() {
        use super::registry_dir_of as of;
        use std::path::PathBuf;
        let (sock, disc) = crate::carrier::unix::paths(7);
        assert_eq!(disc.parent(), Some(super::registry_dir().as_path()));
        assert_eq!(sock.parent(), Some(crate::carrier::unix::socket_dir().as_path()));
        assert_eq!(of(Some("/told".into()), Some("/".into())), PathBuf::from("/told"), "the registry a run is told comes first");
        assert_eq!(of(Some("told".into()), None), std::env::temp_dir(), "a relative one is not a registry");
        let runtime = if cfg!(all(unix, not(target_os = "macos"))) { PathBuf::from("/") } else { std::env::temp_dir() };
        assert_eq!(of(None, Some("/".into())), runtime, "Linux: the session's runtime dir");
        assert_eq!(of(None, Some("/no/such/runtime".into())), std::env::temp_dir(), "a runtime dir that is not there is not one");
        let own = of(std::env::var_os("MAKEPAD_REMOTE_REGISTRY"), std::env::var_os("XDG_RUNTIME_DIR"));
        assert_ne!(super::registry_dir(), own, "the tests announce in a registry of their own");
        assert_ne!(crate::carrier::unix::socket_dir(), crate::carrier::unix::user_socket_dir(), "and listen in a socket dir of their own");
    }
}
