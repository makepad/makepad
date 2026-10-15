//! Loopback JSON-lines transport. Only this thread owns compiler sessions; socket threads
//! carry text, never Program/Tcx pointers (the compiler also has process-global registries).
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, atomic::Ordering};
use std::time::{Duration, Instant};
use crate::json::Json;
use crate::serve::Session;
use crate::serve_os;

pub fn quote(s: &str) -> String { crate::diag::quote(s) }
pub fn encode(v: &Json) -> String {
    match v {
        Json::Null => "null".into(), Json::Bool(b) => b.to_string(), Json::Num(n) => n.to_string(), Json::Str(s) => quote(s),
        Json::Arr(v) => format!("[{}]", v.iter().map(encode).collect::<Vec<_>>().join(",")),
        Json::Obj(v) => format!("{{{}}}", v.iter().map(|(k,v)| format!("{}:{}", quote(k), encode(v))).collect::<Vec<_>>().join(",")),
    }
}
pub fn string<'a>(v: &'a Json, key: &str) -> Result<&'a str, String> {
    match v.get(key) { Some(Json::Str(s)) if !s.is_empty() => Ok(s), _ => Err(format!("missing string `{key}`")) }
}
pub fn workspace(dir: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(crate::resolve::find_root(&dir.to_string_lossy())?).map_err(|e| e.to_string())
}
pub fn directory(root: &Path) -> Result<PathBuf, String> {
    // the served workspace's relax dir (dirs.rs), whatever the current folder
    let cache = crate::dirs::override_dir().unwrap_or_else(|| crate::dirs::relax_dir_for(&root.to_string_lossy()));
    crate::pack::ensure_private_dir(&cache)?;
    let serve = cache.join("serve"); crate::pack::ensure_private_dir(&serve)?;
    let mut h = crate::sip::Sip128::new(); h.str(&root.to_string_lossy());
    let dir = serve.join(format!("{:016x}", h.finish128() as u64));
    crate::pack::ensure_private_dir(&dir)?; Ok(dir)
}
struct Lease { dir: PathBuf, _lock: std::fs::File }
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("endpoint"));
        let _ = std::fs::remove_file(self.dir.join("lock"));
    }
}
fn lock(dir: &Path) -> Result<Option<Lease>, String> {
    let path = dir.join("lock");
    let f = match crate::os::proc::create_private(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::symlink_metadata(&path).map_err(|e|e.to_string())?.file_type().is_symlink() { return Err("lock is a symlink".into()); }
            std::fs::OpenOptions::new().write(true).open(&path).map_err(|e|e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    match f.try_lock() {
        Ok(()) => {
            // An exiting server can unlink a lock opened just before its shutdown.
            #[cfg(unix)] {
                use std::os::unix::fs::MetadataExt;
                let a = f.metadata().map_err(|e|e.to_string())?;
                let b = std::fs::metadata(&path).map_err(|_|"server is shutting down; retry".to_string())?;
                if a.ino() != b.ino() || a.dev() != b.dev() { return Err("server is shutting down; retry".into()); }
            }
            Ok(Some(Lease { dir: dir.into(), _lock: f }))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn endpoint(dir: &Path) -> Result<(u16, String, u32), String> {
    let path = dir.join("endpoint");
    let m = std::fs::symlink_metadata(&path).map_err(|e|e.to_string())?;
    if !m.is_file() || m.file_type().is_symlink() { return Err("invalid endpoint file".into()); }
    #[cfg(unix)] {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        extern "C" { fn getuid() -> u32; }
        if m.uid() != unsafe { getuid() } || m.permissions().mode() & 0o077 != 0 { return Err("endpoint is not owner-only".into()); }
    }
    let text = std::fs::read_to_string(path).map_err(|e|e.to_string())?;
    let v: Vec<_> = text.split_whitespace().collect();
    if v.len() != 3 || v[1].len() != 64 || !v[1].bytes().all(|b|b.is_ascii_hexdigit()) { return Err("invalid endpoint".into()); }
    let pid = v[2].parse().map_err(|_|"invalid endpoint pid")?;
    if !serve_os::alive(pid) { return Err("stale endpoint".into()); }
    Ok((v[0].parse().map_err(|_|"invalid endpoint port")?, v[1].into(), pid))
}
fn error(id: &str, code: i32, message: &str) -> String {
    format!("{{\"id\":{id},\"error\":{{\"code\":{code},\"message\":{}}}}}", quote(message))
}
const LIMIT: usize = 8 * 1024 * 1024;
fn line(r: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut out = Vec::new();
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() { return if out.is_empty() { Ok(None) } else { Err(std::io::Error::other("incomplete JSON line")) }; }
        let n = buf.iter().position(|b| *b == b'\n').map(|n|n+1).unwrap_or(buf.len());
        if out.len() + n > LIMIT { return Err(std::io::Error::other("JSON line too large")); }
        let done = buf[n-1] == b'\n'; out.extend_from_slice(&buf[..n]); r.consume(n);
        if done { return String::from_utf8(out).map(Some).map_err(std::io::Error::other); }
    }
}
enum Event { Request(u64, String), Closed(u64) }
struct Client { out: mpsc::SyncSender<String>, socket: TcpStream, subscriptions: HashSet<String> }
fn send(c: &Client, text: String) -> bool {
    if c.out.try_send(text).is_ok() { true } else { let _ = c.socket.shutdown(std::net::Shutdown::Both); false }
}
struct Package { session: Session, loaded_ms: f64, stamps: HashMap<String, Option<(std::time::SystemTime, u64)>> }
impl Package {
    fn watch(&mut self, overlays: &BTreeMap<String, String>) -> bool {
        let paths: Vec<_> = self.stamps.iter_mut().filter_map(|(p, old)| {
            let now = crate::reload::stamp(p);
            if now == *old { None } else { *old = now; if overlays.contains_key(p) { None } else { Some(p.clone()) } }
        }).collect();
        if paths.is_empty() { return false; }
        self.session.apply(&paths); self.refresh(); true
    }
    fn refresh(&mut self) {
        let std = crate::std_dir();
        for p in self.session.paths() {
            let p = canonical(&p);
            if crate::vfs::relative_path(&p, &std).is_none() && !p.contains("/.cargo/") && !p.contains("/rustlib/") {
                self.stamps.entry(p.clone()).or_insert_with(||crate::reload::stamp(&p));
            }
        }
    }
}
fn package<'a>(sess: &std::sync::Arc<crate::Session>, packages: &'a mut BTreeMap<String, Package>, pkg: &str, overlays: &BTreeMap<String, String>) -> Result<&'a mut Package, String> {
    if !packages.contains_key(pkg) {
        if pkg.starts_with('-') { return Err("invalid package name".into()); }
        let start = Instant::now();
        crate::diag::enable();
        let mut args = vec!["--json".into()];
        args.push(pkg.into());
        let mut session = Session::load(sess, &args)?;
        session.check();
        for (path, text) in overlays { if session.contains(path) { session.did_change(path.clone(), text.clone()); } }
        let mut p = Package { session, loaded_ms: start.elapsed().as_secs_f64()*1000.0, stamps: HashMap::new() };
        p.refresh(); packages.insert(pkg.into(), p);
    }
    Ok(packages.get_mut(pkg).unwrap())
}
fn result(s: &Session, paths: Option<&Json>) -> String {
    let mut ds = s.diagnostics().into_iter().filter(|d| paths.map_or(true, |p|p.arr().iter().any(|p| canonical(p.str()) == canonical(&d.file)))).collect::<Vec<_>>();
    // One-shot check sorts the legacy source-location strings before emitting J1 objects.
    ds.sort_by_cached_key(|d|format!("{}:{}:{}: {}",d.file,d.line,d.col,d.message));
    let errors = ds.iter().filter(|d|d.severity == crate::diag::Severity::Error).count();
    let warnings = ds.iter().filter(|d|d.severity == crate::diag::Severity::Warning).count();
    format!("{{\"diagnostics\":[{}],\"summary\":{{\"errors\":{errors},\"warnings\":{warnings},\"ms\":{:.3},\"rechecked\":{},\"rss_mb\":{:.3}}}}}",
        ds.iter().map(|d|d.to_json()).collect::<Vec<_>>().join(","), s.ms, s.rechecked, s.rss_mb)
}
fn owner_package(path: &str, root: &Path) -> Result<String, String> {
    let path = crate::vfs::normalize_path(path);
    let root = crate::vfs::normalize_path(&root.to_string_lossy());
    if crate::vfs::relative_path(&path, &root).is_none() { return Err("file outside this workspace needs an explicit pkg".into()); }
    let path = Path::new(&path);
    let root = Path::new(&root);
    for dir in path.ancestors().skip(1) {
        if let Ok(text) = crate::vfs::read_to_string(dir.join("Cargo.toml")) {
            let manifest = crate::toml::parse_toml(&text).map_err(|e|e.to_string())?;
            if let Some(name) = manifest.root.get("package").and_then(|p|p.as_table()).and_then(|p|p.get("name")).and_then(|n|n.as_str()) { return Ok(name.into()); }
        }
        if dir == root { break; }
    }
    Err("cannot identify the file's package; supply pkg".into())
}
fn canonical(p: &str) -> String { crate::vfs::canonicalize(p).map(|p|crate::vfs::normalize_path(&p.to_string_lossy())).unwrap_or_else(|_|crate::vfs::normalize_path(p)) }

pub fn cmd_serve(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> i32 {
    match run(sess, args) { Ok(()) => 0, Err(e) => { eprintln!("makepad serve: {e}"); 2 } }
}
fn run(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> Result<(), String> {
    let mut cwd = std::env::current_dir().map_err(|e|e.to_string())?;
    let mut idle = 30.0;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace" => { i += 1; cwd = PathBuf::from(args.get(i).ok_or("--workspace needs a directory")?); }
            "--idle-min" => { i += 1; idle = args.get(i).ok_or("--idle-min needs minutes")?.parse::<f64>().map_err(|_|"invalid idle minutes")?; }
            _ => return Err(format!("unknown serve argument {}", args[i])),
        }
        i += 1;
    }
    if !idle.is_finite() || idle <= 0.0 || idle > 525600.0 { return Err("idle minutes must be positive and at most one year".into()); }
    let root = workspace(&cwd)?;
    let dir = directory(&root)?;
    let Some(_lease) = lock(&dir)? else {
        let start = Instant::now();
        loop {
            if endpoint(&dir).is_ok() { print!("{}", std::fs::read_to_string(dir.join("endpoint")).map_err(|e|e.to_string())?); return Ok(()); }
            if start.elapsed() > Duration::from_secs(30) { return Err("existing server did not publish an endpoint".into()); }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    std::env::set_current_dir(&root).map_err(|e|e.to_string())?;
    serve_os::signals();
    crate::diag::enable();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e|e.to_string())?;
    listener.set_nonblocking(true).map_err(|e|e.to_string())?;
    let token = serve_os::token().map_err(|e|e.to_string())?;
    let announcement = format!("{} {} {}\n", listener.local_addr().map_err(|e|e.to_string())?.port(), token, std::process::id());
    let ep = dir.join("endpoint");
    match std::fs::remove_file(&ep) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => return Err(e.to_string()) }
    let mut f = crate::os::proc::create_private(&ep).map_err(|e|e.to_string())?;
    f.write_all(announcement.as_bytes()).map_err(|e|e.to_string())?; f.sync_all().map_err(|e|e.to_string())?;
    print!("{announcement}"); let _ = std::io::stdout().flush();
    let start = Instant::now(); let mut empty_since = Instant::now(); let mut watch_at = Instant::now();
    let (tx, context) = mpsc::sync_channel(128);
    let mut clients = BTreeMap::<u64, Client>::new(); let mut next = 0; let mut writers = Vec::new();
    let mut packages = BTreeMap::<String, Package>::new(); let mut overlays = BTreeMap::<String, String>::new();
    let mut pending = Vec::<(String, String)>::new();
    while !serve_os::STOP.load(Ordering::Relaxed) {
        loop {
            match listener.accept() {
                Ok((socket, peer)) => {
                    if !peer.ip().is_loopback() || clients.len() >= 128 { continue; }
                    // accepted sockets inherit the listener's O_NONBLOCK on macOS/BSD: the reader
                    // thread blocks on its socket
                    socket.set_nonblocking(false).map_err(|e|e.to_string())?;
                    socket.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e|e.to_string())?;
                    let reader = socket.try_clone().map_err(|e|e.to_string())?;
                    let mut writer = socket.try_clone().map_err(|e|e.to_string())?;
                    let (out, output) = mpsc::sync_channel::<String>(32);
                    next += 1; let id = next; let input = tx.clone();
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(reader);
                        while let Ok(Some(line)) = line(&mut reader) { if input.send(Event::Request(id, line)).is_err() { return; } }
                        let _ = input.send(Event::Closed(id));
                    });
                    writers.push(std::thread::spawn(move || {
                        while let Ok(text) = output.recv() { if writeln!(writer, "{text}").is_err() { break; } }
                        let _ = writer.shutdown(std::net::Shutdown::Both);
                    }));
                    clients.insert(id, Client { out, socket, subscriptions: HashSet::new() });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        if clients.is_empty() && empty_since.elapsed().as_secs_f64() >= idle*60.0 { break; }
        if !clients.is_empty() { empty_since = Instant::now(); }
        match context.recv_timeout(Duration::from_millis(10)) {
            Ok(Event::Closed(c)) => { clients.remove(&c); }
            Ok(Event::Request(c, text)) => {
                let Some(client) = clients.get_mut(&c) else { continue; };
                let r = match crate::json::parse(text.as_bytes()) {
                    Ok(r) => r, Err(e) => { send(client, error("null", -32700, &e)); clients.remove(&c); continue; }
                };
                let id = match r.get("id") { Some(Json::Num(n)) if n.fract() == 0.0 && n.abs() <= 9007199254740991.0 => n.to_string(), _ => "null".into() };
                let supplied = r.get("token").map_or("", Json::str);
                let different = supplied.len() != token.len() || supplied.bytes().zip(token.bytes()).fold(0u8, |v,(a,b)|v | (a ^ b)) != 0;
                if different { send(client, error(&id, -32001, "invalid token")); clients.remove(&c); continue; }
                if id == "null" { send(client, error(&id, -32600, "integer id required")); continue; }
                let params = r.get("params").unwrap_or(&Json::Null);
                let method = r.get("method").map_or("", Json::str);
                let answer = (|| -> Result<String, String> {
                    match method {
                        "status" => Ok(format!("{{\"workspace\":{},\"packages\":[{}],\"uptime_s\":{:.3}}}", quote(&root.to_string_lossy()), packages.iter().map(|(name,p)|format!("{{\"pkg\":{},\"loaded_ms\":{:.3},\"last_check_ms\":{:.3},\"items\":{},\"rss_mb\":{:.3}}}",quote(name),p.loaded_ms,p.session.ms,p.session.items.len(),p.session.rss_mb)).collect::<Vec<_>>().join(","),start.elapsed().as_secs_f64())),
                        "shutdown" => { serve_os::STOP.store(true, Ordering::Relaxed); Ok("null".into()) }
                        "check" | "subscribe" | "schema" => {
                            let pkg = string(params,"pkg")?;
                            let p = package(sess, &mut packages, pkg, &overlays)?;
                            if p.watch(&overlays) { pending.push((pkg.into(), result(&p.session,None))); }
                            if method == "subscribe" { client.subscriptions.insert(pkg.into()); return Ok("null".into()); }
                            if method == "schema" {
                                let result = crate::serve_query::schema(&mut p.session, string(params,"path")?);
                                p.refresh();
                                return result;
                            }
                            if let Some(paths) = params.get("paths") {
                                if !matches!(paths, Json::Arr(v) if v.iter().all(|p|matches!(p, Json::Str(_)))) { return Err("paths must be an array of strings".into()); }
                            }
                            Ok(result(&p.session, params.get("paths")))
                        }
                        "didChange" => {
                            let path = canonical(string(params,"path")?);
                            match params.get("text") {
                                Some(Json::Str(text)) => { overlays.insert(path.clone(),text.clone()); }
                                Some(Json::Null) => { overlays.remove(&path); }
                                _ => return Err("text must be a string or null".into()),
                            }
                            for p in packages.values_mut().filter(|p|p.session.contains(&path)) {
                                if let Some(text) = overlays.get(&path) { p.session.did_change(path.clone(), text.clone()); }
                                else { p.session.apply(&[path.clone()]); }
                                p.refresh();
                            }
                            Ok("null".into())
                        }
                        "hover" | "definition" => {
                            let path = canonical(string(params,"path")?);
                            if let Some(Json::Str(pkg)) = params.get("pkg") { package(sess, &mut packages, pkg, &overlays)?; }
                            if !packages.values().any(|p|p.session.contains(&path)) {
                                let pkg = owner_package(&path,&root)?; package(sess, &mut packages,&pkg,&overlays)?;
                            }
                            let (pkg,p) = packages.iter_mut().find(|(_,p)|p.session.contains(&path)).ok_or("file has no loaded package; supply pkg or check it first")?;
                            if p.watch(&overlays) { pending.push((pkg.clone(), result(&p.session,None))); }
                            crate::serve_query::position(&mut p.session, &path, params, method == "definition")
                        }
                        _ => Err("unknown method".into()),
                    }
                })();
                send(client, match answer { Ok(v) => format!("{{\"id\":{id},\"result\":{v}}}"), Err(e) => error(&id, if method.is_empty() || e == "unknown method" { -32601 } else { -32602 }, &e) });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}, Err(_) => break,
        }
        for (pkg,r) in pending.drain(..) {
            let event = format!("{{\"event\":\"diagnostics\",\"pkg\":{},{}", quote(&pkg), &r[1..]);
            for c in clients.values().filter(|c|c.subscriptions.contains(&pkg)) { send(c,event.clone()); }
        }
        let mut i = 0;
        while i < writers.len() {
            if writers[i].is_finished() { let _ = writers.swap_remove(i).join(); } else { i += 1; }
        }
        if watch_at.elapsed() >= Duration::from_millis(100) {
            for (pkg,p) in &mut packages {
                // An unsaved buffer wins over filesystem notifications until text:null.
                let edited: Vec<_> = p.stamps.iter_mut().filter_map(|(path, old)| {
                    let now = crate::reload::stamp(path);
                    if now == *old { return None; } *old = now;
                    if overlays.contains_key(path) { None } else { Some(path.clone()) }
                }).collect();
                if !edited.is_empty() {
                    p.session.apply(&edited); p.refresh();
                    let r = result(&p.session,None);
                    let event = format!("{{\"event\":\"diagnostics\",\"pkg\":{},{}", quote(pkg), &r[1..]);
                    for c in clients.values().filter(|c|c.subscriptions.contains(pkg)) { send(c,event.clone()); }
                }
            }
            watch_at = Instant::now();
        }
    }
    // Dropping senders lets writers drain the shutdown response before closing sockets.
    drop(clients);
    for writer in writers { let _ = writer.join(); }
    Ok(())
}

pub fn request(dir: &Path, method: &str, params: &str) -> Result<Json, String> {
    let (port, token, _) = endpoint(dir)?;
    let mut socket = TcpStream::connect_timeout(&([127,0,0,1],port).into(), Duration::from_secs(2)).map_err(|e|e.to_string())?;
    socket.set_read_timeout(Some(Duration::from_secs(1800))).map_err(|e|e.to_string())?;
    socket.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e|e.to_string())?;
    writeln!(socket,"{{\"id\":1,\"token\":{},\"method\":{},\"params\":{params}}}", quote(&token),quote(method)).map_err(|e|e.to_string())?;
    let text = line(&mut BufReader::new(socket)).map_err(|e|e.to_string())?.ok_or("server closed connection")?;
    let response = crate::json::parse(text.as_bytes())?;
    if let Some(e) = response.get("error") { return Err(e.get("message").map_or("server error",Json::str).into()); }
    match response { Json::Obj(v) => v.into_iter().find(|(k,_)|k=="result").map(|(_,v)|v).ok_or("missing result".into()), _ => Err("invalid response".into()) }
}
fn connect(start: bool) -> Result<PathBuf, String> {
    let root = workspace(&std::env::current_dir().map_err(|e|e.to_string())?)?;
    let dir = directory(&root)?;
    if request(&dir,"status","{}").is_ok() { return Ok(dir); }
    if !start { return Err("no check server".into()); }
    eprintln!("makepad: starting the check server...");
    let log_path = dir.join("serve.log");
    let log = if log_path.exists() { std::fs::OpenOptions::new().append(true).open(&log_path) } else { crate::os::proc::create_private(&log_path) }.map_err(|e|e.to_string())?;
    let mut cmd = std::process::Command::new(std::env::current_exe().map_err(|e|e.to_string())?);
    cmd.arg("serve").arg("--workspace").arg(&root).stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e|e.to_string())?).stderr(log);
    serve_os::detach(&mut cmd);
    let mut child = cmd.spawn().map_err(|e|e.to_string())?; let started = Instant::now();
    loop {
        if request(&dir,"status","{}").is_ok() { return Ok(dir); }
        if let Some(status) = child.try_wait().map_err(|e|e.to_string())? { if !status.success() { return Err(format!("check server exited {status}; see {}",log_path.display())); } }
        if started.elapsed() > Duration::from_secs(1800) { return Err("timed out starting check server".into()); }
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// Only the ordinary package invocation auto-connects: flags changing compilation retain
/// their one-shot meaning. An explicit --serve with such flags is rejected, never ignored.
pub fn check(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> i32 {
    let start = args.iter().any(|a|a=="--serve");
    let never = args.iter().any(|a|a=="--no-serve");
    let json = args.iter().any(|a|a=="--json");
    let rest: Vec<_> = args.iter().filter(|a| !matches!(a.as_str(),"--serve"|"--no-serve"|"--json")).collect();
    if start && never { eprintln!("makepad: --serve and --no-serve conflict"); return 2; }
    if !never && rest.len() == 1 && !rest[0].starts_with('-') {
        let response = connect(start).and_then(|dir|request(&dir,"check",&format!("{{\"pkg\":{}}}",quote(rest[0]))));
        match response {
            Ok(v) => {
                for d in v.get("diagnostics").map_or(&[][..],Json::arr) {
                    if json { println!("{}",encode(d)); }
                    else { eprintln!("{}:{}:{}: {}",d.get("file").map_or("",Json::str),d.get("line").map_or(0.0,Json::num),d.get("col").map_or(0.0,Json::num),d.get("message").map_or("",Json::str)); }
                }
                let summary = v.get("summary").unwrap_or(&Json::Null);
                let errors = summary.get("errors").map_or(0.0,Json::num) as usize;
                let ms = summary.get("ms").map_or(0.0,Json::num) as u64;
                if json { println!("{{\"type\":\"summary\",\"errors\":{errors},\"warnings\":{},\"ms\":{ms}}}",summary.get("warnings").map_or(0.0,Json::num)); }
                else { eprintln!("makepad check: {errors} errors, {ms} ms"); }
                return if errors == 0 { 0 } else { 1 };
            }
            Err(e) if start => { eprintln!("makepad: {e}"); return 2; }
            Err(_) => {},
        }
    } else if start { eprintln!("makepad: --serve supports check [--json] <package>; use --no-serve for other check options"); return 2; }
    let mut a = vec!["--check".into()]; a.extend(args.iter().filter(|a| !matches!(a.as_str(),"--serve"|"--no-serve")).cloned());
    crate::run::cmd_run(sess, &a)
}
pub fn query(sess: &std::sync::Arc<crate::Session>, method: &str, args: &[String]) -> i32 {
    let values: Vec<_> = args.iter().filter(|a|a.as_str()!="--serve" && a.as_str()!="--json").collect();
    let params = match (method, values.as_slice()) {
        ("schema", [pkg,path]) => format!("{{\"pkg\":{},\"path\":{}}}",quote(pkg),quote(path)),
        ("hover"|"definition", [pkg,path,line,col]) if line.parse::<u32>().is_ok() && col.parse::<u32>().is_ok() => format!("{{\"pkg\":{},\"path\":{},\"line\":{},\"col\":{}}}",quote(pkg),quote(&canonical(path)),line,col),
        _ => { eprintln!("usage: makepad schema [--serve] <pkg> <item> | makepad hover|definition [--serve] <pkg> <file> <line> <col>"); return 2; }
    };
    let start = args.iter().any(|a|a=="--serve");
    let response = match connect(start) {
        Ok(dir) => request(&dir,method,&params).map(|v|encode(&v)),
        Err(_) if !start => (|| -> Result<String,String> {
            crate::diag::enable();
            let mut session = Session::load(sess, &["--json".into(),values[0].to_string()])?;
            session.check();
            if method == "schema" { crate::serve_query::schema(&mut session,values[1]) }
            else { crate::serve_query::position(&mut session,&canonical(values[1]),&crate::json::parse(params.as_bytes())?,method=="definition") }
        })(),
        Err(e) => Err(e),
    };
    match response {
        Ok(v) => { println!("{v}"); 0 }, Err(e) => { eprintln!("makepad: {e}"); 2 }
    }
}
