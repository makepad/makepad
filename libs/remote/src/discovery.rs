//! Finding running endpoints by their discovery files (`makepad-remote-<pid>.json` in the user's
//! registry dir, [`registry_dir`]). Files of processes that ended are removed when listed.
//!
//! A pid says which process is meant. A bare name or `all` does not: on a machine where someone
//! works it would pick their app (one that drives a real printer, say) for whoever mistypes a
//! command. So those reach only what the [`Caller`] started, and everything else is named by pid.

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
    /// Windows: the thin parent this run stands behind (restart_parent.rs in the JIT), whose pid is
    /// the one whoever started the run holds, and the one that stays when the run restarts
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
    let Ok(text) = std::fs::read_to_string(file) else { return };
    let Some(at) = text.find("\"caps\":[") else { return };
    let start = at + "\"caps\":[".len();
    let Some(len) = text[start..].find(']') else { return };
    let list: Vec<String> = caps.iter().map(|c| crate::cmd::json_str(c)).collect();
    let new = format!("{}{}{}", &text[..start], list.join(","), &text[start + len..]);
    let tmp = file.with_extension("json.new");
    if std::fs::write(&tmp, new).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
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
    let app = match spec.parse::<u32>() {
        Ok(pid) => list.into_iter().find(|a| a.answers_to(pid)).ok_or_else(|| no_endpoint(spec)),
        Err(_) => named(&list, spec, caller),
    }?;
    match app.unreachable() {
        Some(why) => Err(why),
        None => Ok(app),
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
        super::rewrite_caps(&file, &["cmd/1".into(), "app/1".into()]);
        let app = super::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(app.serves("app") && app.serves("cmd"));
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
