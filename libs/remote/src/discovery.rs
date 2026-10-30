//! Finding running endpoints by their discovery files (`makepad-remote-<pid>.json` in the user's
//! registry dir, [`registry_dir`]). Files of processes that ended are removed when listed.


#[derive(Debug, Clone)]
pub struct App {
    pub pid: u32,
    pub app: String,
    pub socket: String,
    pub started_ms: u64,
    pub caps: Vec<String>,
    /// every top-level field as (key, raw JSON value), the host's extra fields included
    pub fields: Vec<(String, String)>,
}

impl App {
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
/// process's TMPDIR. The socket itself stays in the process's temp dir (its path is in the file).
/// A makepad WM gives its JIT apps an on-disk TMPDIR (`~/.makepad/wm/jit.env`), and `makepad
/// inspect` from a shell has another: with the files in the temp dir it found none of them
/// (166, 2026-10-25). Linux (and other Unix with a session): `$XDG_RUNTIME_DIR`, the user's 0700
/// runtime dir; elsewhere the temp dir (macOS's is per user already).
pub fn registry_dir() -> std::path::PathBuf {
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|d| d.is_absolute() && d.is_dir()) {
        return dir;
    }
    std::env::temp_dir()
}

/// The running endpoints, oldest first by pid (stale discovery files of ended processes are
/// removed, with the socket each names).
pub fn apps() -> Vec<App> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(registry_dir()) else { return out };
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

/// `spec` is a pid, an app name (of several with that name the most recent: highest pid) or a
/// `tcp:` socket a discovery file names.
pub fn find(spec: &str) -> Option<App> {
    let list = apps();
    // a loopback endpoint as its discovery file names it (Windows): the file carries its token
    if spec.starts_with("tcp:") {
        return list.into_iter().rfind(|a| a.socket == spec);
    }
    match spec.parse::<u32>() {
        Ok(pid) => list.into_iter().find(|a| a.pid == pid),
        Err(_) => list.into_iter().rfind(|a| a.app == spec),
    }
}

#[cfg(test)]
mod rewrite_tests {
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
    #[test]
    fn discovery_files_go_to_the_registry_dir() {
        let (sock, disc) = crate::carrier::unix::paths(7);
        assert_eq!(disc.parent(), Some(super::registry_dir().as_path()));
        assert_eq!(sock.parent(), Some(std::env::temp_dir().as_path()));
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|d| d.is_absolute() && d.is_dir()) {
            assert_eq!(super::registry_dir(), xdg);
        }
    }
}
