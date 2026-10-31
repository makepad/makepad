//! Throwaway fleet and logging host for multi-viewer transport/ownership checks.
//! Pictures are synthetic bytes, not a decodable video stream.
use makepad_desk_link::{self as proto, server::{self, Control, Feed, Input, Out, Server, ServerConfig, ToUi}, GamepadState, ScreenInfo, ServerMsg};
use makepad_network::fleet::id::{self, Role};
use std::{fs, io, net::IpAddr, path::Path, sync::{atomic::Ordering, mpsc::Sender, Arc, OnceLock}, time::{Duration, Instant}};

fn log(message: &str) { println!("{} {message}", id::now_secs()); }
fn clock() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[cfg(unix)]
fn mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}
#[cfg(not(unix))]
fn mode(_: &Path, _: u32) -> io::Result<()> {
    Err(io::Error::other("mint requires Unix file permissions"))
}

fn mint(dir: &Path, ips: &[String]) -> Result<(), String> {
    let ips = ips.iter().map(|s| s.parse::<IpAddr>().map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?;
    let work = || -> io::Result<()> {
        // Refuse an existing destination; never overwrite a real fleet identity.
        fs::create_dir(dir)?;
        mode(dir, 0o700)?;
        let root = id::Root::generate()?;
        let issue = |path: &Path, name, role, from| -> io::Result<()> {
            fs::create_dir(path)?;
            mode(path, 0o700)?;
            let key = id::generate_key()?;
            let cred = root.issue(name, role, id::now_secs() + 86400, from, &id::point_of(&key))?;
            id::write_key(&path.join(id::ID_KEY), &key)?;
            // Precreate public/credential files privately, including with a permissive umask.
            for file in [id::ID_CRED, id::ROOT_PUB] {
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)] {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options.open(path.join(file))?;
            }
            fs::write(path.join(id::ID_CRED), format!("{}\n", cred.text()))?;
            id::write_root_pub(path, &root.point())?;
            for file in [id::ID_KEY, id::ID_CRED, id::ROOT_PUB] { mode(&path.join(file), 0o600)?; }
            id::Me::load(path).map_err(io::Error::other)?;
            id::Fleet::load(path).map_err(io::Error::other)?;
            Ok(())
        };
        issue(&dir.join("host"), "testhost", Role::Node, None)?;
        for ip in ips { issue(&dir.join(format!("viewer-{ip}")), "admin", Role::Device, Some(ip))?; }
        Ok(())
    };
    work().map_err(|e| e.to_string())?;
    log("MINT ready (credentials expire in 24 hours)");
    Ok(())
}

struct LoggingInput(u32);
impl LoggingInput {
    fn event(&self, event: std::fmt::Arguments<'_>) { log(&format!("INPUT session={} {event}", self.0)); }
}
impl Input for LoggingInput {
    fn pointer(&mut self, x: u16, y: u16) { self.event(format_args!("pointer x={x} y={y}")); }
    fn button(&mut self, down: bool, button: u8) { self.event(format_args!("button down={down} button={button}")); }
    fn scroll(&mut self, dx: i32, dy: i32) { self.event(format_args!("scroll dx={dx} dy={dy}")); }
    fn key(&mut self, down: bool, keycode: u32) { self.event(format_args!("key down={down} keycode={keycode}")); }
    fn gamepad(&mut self, slot: u8, state: &GamepadState) { self.event(format_args!("gamepad slot={slot} {state:?}")); }
    fn gamepad_gone(&mut self, slot: u8) { self.event(format_args!("gamepad_gone slot={slot}")); }
}

struct Viewer {
    session: u32,
    feed: Arc<Feed>,
    out: Sender<Out>,
    window: Option<(u32, u32, u16)>,
    view_only: bool,
    sent: u64,
}

fn serve(port: u16, name: String, width: u32, height: u32, fps: u16, bytes: usize) -> Result<(), String> {
    let payload = vec![0u8; bytes];
    let server = Server::start(ServerConfig {
        addr: ([0, 0, 0, 0], port).into(), tls_name: name.clone(), log: Arc::new(log), clock,
        input: Arc::new(|| {
            // Server names each session thread before invoking this factory.
            let thread = std::thread::current();
            let session = thread.name().and_then(|s| s.strip_prefix("desk-link-")).and_then(|s| s.parse().ok()).ok_or("missing server session thread name")?;
            Ok(Box::new(LoggingInput(session)))
        }),
    }, Arc::new(|| {}))?;
    log(&format!("READY addr={} name={name} size={width}x{height} fps={fps}", server.addr));
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut size = None;
    log("SIZE owner=none");
    let mut key_due = true;
    let mut frame = 0u32;
    let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut picture_at = Instant::now();
    let mut second_at = Instant::now();
    loop {
        while let Ok(message) = server.rx.try_recv() {
            match message {
                ToUi::Opened { session, peer, name: viewer_name, paused, caps, out, feed, .. } => {
                    log(&format!("OPEN session={session} peer={peer} name={viewer_name:?} paused={paused}"));
                    let welcome = ServerMsg::Welcome { version: proto::VERSION, session,
                        screens: vec![ScreenInfo { name: name.clone(), x: 0, y: 0, width, height }], screen: 0,
                        stream: [width, height], input: true, input_note: String::new(), server: name.clone() };
                    let rate = ServerMsg::Rate { fps, kbps: 0, limit: proto::limit::FIXED, full_chroma: false,
                        codec: proto::pref::H264, preset: proto::pref::FAST, streams: 1, why: "synthetic transport test pictures".into() };
                    for message in [welcome, rate] {
                        if out.send(Out::Msg(message.encode())).is_err() { log(&format!("SEND_CLOSED session={session}")); }
                    }
                    viewers.push(Viewer { session, feed, out, window: None, view_only: caps & proto::caps::VIEW_ONLY != 0, sent: 0 });
                    key_due = true;
                }
                ToUi::Closed { session, reason } => {
                    log(&format!("CLOSE session={session} reason={reason:?}"));
                    viewers.retain(|v| v.session != session);
                }
                ToUi::Control { session, control } => {
                    if let Some(v) = viewers.iter_mut().find(|v| v.session == session) {
                        match control {
                            Control::Pause => { v.feed.paused.store(true, Ordering::Release); log(&format!("PAUSE session={session}")); }
                            Control::Resume => { v.feed.paused.store(false, Ordering::Release); key_due = true; log(&format!("RESUME session={session}")); }
                            Control::Window { width, height, scale } => {
                                v.window = Some((width, height, scale));
                                log(&format!("WINDOW session={session} {width}x{height} scale={scale}"));
                            }
                            Control::Keyframe => { key_due = true; log(&format!("KEYFRAME session={session}")); }
                            Control::SelectScreen(screen) => log(&format!("SELECT session={session} screen={screen}")),
                            Control::Refresh(hz) => log(&format!("REFRESH session={session} hz={hz}")),
                            Control::Clipboard(text) => log(&format!("CLIPBOARD session={session} bytes={}", text.len())),
                            Control::Stats { frame, decode_us, present_us } => log(&format!("STATS session={session} frame={frame} decode_us={decode_us} present_us={present_us}")),
                            Control::Encoding { chroma, codec, preset, mbps, fps } => log(&format!("ENCODING session={session} chroma={chroma} codec={codec} preset={preset} mbps={mbps} fps={fps}")),
                        }
                    }
                }
            }
            let owner = server::size_owner(viewers.iter().map(|v| (v.session, v.window.is_some() && !v.view_only)));
            let next = owner.and_then(|s| viewers.iter().find(|v| v.session == s).and_then(|v| v.window.map(|w| (s, w))));
            if next != size {
                size = next;
                if let Some((s, (w, h, scale))) = size { log(&format!("SIZE owner={s} {w}x{h} scale={scale}")); }
                else { log("SIZE owner=none"); }
            }
        }
        if picture_at.elapsed() >= interval {
            picture_at = Instant::now();
            if viewers.iter().any(|v| v.feed.watching()) && !server::hold(viewers.iter().map(|v| v.feed.as_ref())) {
                let resync = server::resync_due(viewers.iter().map(|v| v.feed.as_ref()));
                let key = key_due || resync || frame % 60 == 0;
                key_due = false;
                let now = clock();
                let picture = proto::encode_video(frame, key, now, now, 0, (0, 1, 0, 0), proto::pref::H264, &payload);
                for v in &mut viewers {
                    if v.feed.admit(frame, key) {
                        v.feed.backlog.fetch_add(1, Ordering::AcqRel);
                        if v.out.send(Out::Video(picture.clone())).is_ok() { v.sent += 1; }
                        else { v.feed.backlog.fetch_sub(1, Ordering::AcqRel); log(&format!("SEND_CLOSED session={}", v.session)); }
                    }
                }
                frame = frame.wrapping_add(1);
            }
        }
        if second_at.elapsed() >= Duration::from_secs(1) {
            second_at = Instant::now();
            let mut line = String::from("SECOND");
            for v in &mut viewers {
                line.push_str(&format!(" session={} paused={} sent={} backlog={} skipping={}", v.session,
                    v.feed.paused.load(Ordering::Acquire), v.sent, v.feed.backlog.load(Ordering::Acquire), v.feed.skipping()));
                v.sent = 0;
            }
            log(&line);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: desk-link-testhost mint <new-dir> <viewer-ip>... | serve --port P --name N --size WxH [--fps F] [--bytes B (a picture's size, default 2048)]";
    match args.first().map(String::as_str) {
        Some("mint") if args.len() >= 3 => mint(Path::new(&args[1]), &args[2..]),
        Some("serve") => {
            let (mut port, mut name, mut size, mut fps, mut bytes) = (None, None, None, 30u16, 2048usize);
            let mut options = args[1..].chunks_exact(2);
            for pair in &mut options {
                match pair[0].as_str() {
                    "--port" => port = Some(pair[1].parse::<u16>().map_err(|e| e.to_string())?),
                    "--name" => name = Some(pair[1].clone()),
                    "--size" => {
                        let (w, h) = pair[1].split_once('x').ok_or(usage)?;
                        size = Some((w.parse::<u32>().map_err(|e| e.to_string())?, h.parse::<u32>().map_err(|e| e.to_string())?));
                    }
                    "--fps" => fps = pair[1].parse::<u16>().map_err(|e| e.to_string())?,
                    "--bytes" => bytes = pair[1].parse::<usize>().map_err(|e| e.to_string())?,
                    _ => return Err(usage.into()),
                }
            }
            if !options.remainder().is_empty() { return Err(usage.into()); }
            let port = port.filter(|p| *p != 0).ok_or(usage)?;
            let name = name.filter(|n| !n.is_empty()).ok_or(usage)?;
            let (w, h) = size.filter(|(w, h)| *w > 0 && *h > 0).ok_or(usage)?;
            if fps == 0 { return Err("fps must be positive".into()); }
            serve(port, name, w, h, fps, bytes)
        }
        _ => Err(usage.into()),
    }
}
fn main() {
    if let Err(e) = run() { eprintln!("desk-link-testhost: {e}"); std::process::exit(1); }
}
