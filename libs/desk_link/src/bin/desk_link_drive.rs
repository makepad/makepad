//! desk-link-drive <host[:port]> <step> [, <step>]...: an agent's eyes and hands on a Desk Link
//! desktop. It connects with this machine's fleet credential as one more viewer (view-only caps:
//! it never decides a followed desktop's size, never sends an Encoding, never switches the
//! screen), so it shares the desktop with a person's own viewer without taking it over. The
//! session opens paused; video flows from the start only when a step looks (`shot`,
//! `wait-change`). Every key and button it still holds goes up when the session ends (the
//! server's `HeldInput`).
//!
//! Coordinates are the stream picture's pixels (what `info` prints as `stream WxH`); a `shot`
//! line says how its PNG maps onto them.
//!
//! Steps (one line of output each):
//! - `info`: the desktop, its stream size and screens, whether input is injected
//! - `shot PATH [X,Y,W,H] [/N]`: the current picture as a PNG, optionally a crop of the stream
//!   picture and scaled down N times (default: N so the PNG is at most 1920 wide/high)
//! - `move X Y`, `click X Y [left|right|middle] [double]`, `drag X1 Y1 X2 Y2 [left|right]`
//! - `scroll NOTCHES [X Y]`: + scrolls the content up (wheel away from the user)
//! - `type TEXT`: US-layout keys, Shift where needed (`\n` Enter, `\t` Tab)
//! - `key COMBO`: `ctrl+s`, `enter`, `tab`, `win+r`, `alt+f4`, `ctrl+shift+e`, `esc`
//! - `wait MS`
//! - `wait-change [TIMEOUT_MS] [QUIET_MS]`: from the current picture, until it changes and then
//!   stays still QUIET_MS (default 10000, 600); prints where it changed. `changed at` counts from
//!   the step's start, after it took a picture (100 ms). After input it also prints `first change
//!   N ms after the press`: from sending the first press (button or key down, scroll notch; with
//!   none, the first input) since the last wait-change to the first picture composed here that
//!   differs from the one before that input, timed as it was composed (the frame and box named)
//! - `record DIR MS`: the video as it comes for MS, not decoded here (no picture is kept while it
//!   runs; the next `shot`/`wait-change` starts from a new key frame): per stream from its first
//!   key frame `DIR/stream-S.h264` (or `.hevc`, an Annex-B elementary stream another tool can
//!   decode frame by frame) and `DIR/packets.txt` (one line per packet in that file: ms, stream,
//!   frame, x0, y0, codec, key, bytes, and the desktop clock's built_ns, encoded_ns, sent_ns).
//!   A stream that changes codec while recording ends at the change.
//!
//! Example: `desk-link-drive 10.0.0.105 click 812 440 , wait-change , shot /tmp/a.png`

use makepad_desk_link::{self as proto, ClientMsg, Inbox, ServerMsg};
use makepad_key_code::{KeyCode, KEYCODE_VARIANTS};
use makepad_video::{StreamVideoCodec, VideoStreamDecoder};
use std::io::Read;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: desk-link-drive <host[:port]> <step> [, <step>]...\n  steps: info | shot PATH [X,Y,W,H] [/N] | move X Y | click X Y [left|right|middle] [double] | drag X1 Y1 X2 Y2 [button] | scroll N [X Y] | type TEXT | key COMBO | wait MS | wait-change [TIMEOUT_MS] [QUIET_MS] | record DIR MS";

/// How many already-queued messages `pump` still handles once its time is up (the rest wait for the next pump).
const PUMP_DRAIN_MAX: usize = 64;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(target) = args.first().filter(|a| !a.starts_with('-')) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let steps: Vec<Vec<String>> = args[1..].split(|a| a == ",").filter(|s| !s.is_empty()).map(|s| s.to_vec()).collect();
    if steps.is_empty() {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    // every step parses before anything is sent
    let parsed: Result<Vec<Step>, String> = steps.iter().map(|s| Step::parse(s)).collect();
    let steps = match parsed {
        Ok(s) => s,
        Err(e) => {
            eprintln!("drive: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.to_string()),
        _ => (target.clone(), proto::DEFAULT_PORT.to_string()),
    };
    let mut link = match Link::connect(&host, &port) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("drive: {e}");
            std::process::exit(1);
        }
    };
    // pictures from the start when a step looks: a wait-change sees what the input before it did
    if steps.iter().any(|s| matches!(s, Step::Shot { .. } | Step::WaitChange { .. })) {
        if let Err(e) = link.picture(Duration::ZERO) {
            eprintln!("drive: {e}");
            std::process::exit(1);
        }
    }
    for step in &steps {
        match link.run(step) {
            Ok(line) => println!("{line}"),
            Err(e) => {
                eprintln!("drive: {}: {e}", step.name());
                std::process::exit(1);
            }
        }
    }
    link.close();
}

enum Step {
    Info,
    Shot { path: String, crop: Option<[u32; 4]>, scale: Option<u32> },
    Move { x: u32, y: u32 },
    Click { x: u32, y: u32, button: u8, double: bool },
    Drag { from: [u32; 2], to: [u32; 2], button: u8 },
    Scroll { notches: i32, at: Option<[u32; 2]> },
    Type { keys: Vec<(KeyCode, bool)>, text: String },
    Key { combo: Vec<KeyCode>, text: String },
    Wait { ms: u64 },
    WaitChange { timeout: u64, quiet: u64 },
    Record { dir: String, ms: u64 },
}

fn num<T: std::str::FromStr>(args: &[String], i: usize, what: &str) -> Result<T, String> {
    args.get(i).ok_or_else(|| format!("{} needs {what}", args[0]))?.parse().map_err(|_| format!("{}: bad {what} '{}'", args[0], args[i]))
}

fn button_of(name: &str) -> Option<u8> {
    match name {
        "left" => Some(proto::button::LEFT),
        "right" => Some(proto::button::RIGHT),
        "middle" => Some(proto::button::MIDDLE),
        _ => None,
    }
}

fn key_of(name: &str) -> Option<KeyCode> {
    match name.to_ascii_lowercase().as_str() {
        "win" | "windows" | "super" => Some(KeyCode::Logo),
        "bksp" => Some(KeyCode::Backspace),
        "pgup" => Some(KeyCode::PageUp),
        "pgdn" => Some(KeyCode::PageDown),
        "plus" => Some(KeyCode::Equals),
        n => KeyCode::from_name(n).filter(|k| !k.is_unknown()),
    }
}

/// The key that types `c` on a US layout, and whether it needs Shift.
fn key_for_char(c: char) -> Option<(KeyCode, bool)> {
    match c {
        '\n' => return Some((KeyCode::ReturnKey, false)),
        '\t' => return Some((KeyCode::Tab, false)),
        _ => {}
    }
    // the main block comes before the numpad in the table
    let main = KEYCODE_VARIANTS.iter().filter(|k| !format!("{k:?}").starts_with("Numpad"));
    for k in main {
        if k.to_char(false) == Some(c) {
            return Some((*k, false));
        }
        if k.to_char(true) == Some(c) {
            return Some((*k, true));
        }
    }
    None
}

impl Step {
    fn name(&self) -> &'static str {
        match self {
            Step::Info => "info",
            Step::Shot { .. } => "shot",
            Step::Move { .. } => "move",
            Step::Click { .. } => "click",
            Step::Drag { .. } => "drag",
            Step::Scroll { .. } => "scroll",
            Step::Type { .. } => "type",
            Step::Key { .. } => "key",
            Step::Wait { .. } => "wait",
            Step::WaitChange { .. } => "wait-change",
            Step::Record { .. } => "record",
        }
    }

    fn parse(a: &[String]) -> Result<Step, String> {
        Ok(match a[0].as_str() {
            "info" => Step::Info,
            "shot" => {
                let path = a.get(1).ok_or("shot needs a PATH")?.clone();
                let (mut crop, mut scale) = (None, None);
                for t in &a[2..] {
                    if let Some(n) = t.strip_prefix('/') {
                        scale = Some(n.parse::<u32>().ok().filter(|n| *n >= 1).ok_or_else(|| format!("shot: bad scale '{t}'"))?);
                    } else {
                        let v: Vec<u32> = t.split(',').filter_map(|p| p.parse().ok()).collect();
                        if v.len() != 4 || v[2] == 0 || v[3] == 0 {
                            return Err(format!("shot: bad crop '{t}' (X,Y,W,H)"));
                        }
                        crop = Some([v[0], v[1], v[2], v[3]]);
                    }
                }
                Step::Shot { path, crop, scale }
            }
            "move" => Step::Move { x: num(a, 1, "X")?, y: num(a, 2, "Y")? },
            "click" => {
                let mut button = proto::button::LEFT;
                let mut double = false;
                for t in &a[3..] {
                    match t.as_str() {
                        "double" => double = true,
                        b => button = button_of(b).ok_or_else(|| format!("click: bad option '{b}'"))?,
                    }
                }
                Step::Click { x: num(a, 1, "X")?, y: num(a, 2, "Y")?, button, double }
            }
            "drag" => {
                let button = match a.get(5) {
                    Some(b) => button_of(b).ok_or_else(|| format!("drag: bad button '{b}'"))?,
                    None => proto::button::LEFT,
                };
                Step::Drag { from: [num(a, 1, "X1")?, num(a, 2, "Y1")?], to: [num(a, 3, "X2")?, num(a, 4, "Y2")?], button }
            }
            "scroll" => {
                let at = if a.len() >= 4 { Some([num(a, 2, "X")?, num(a, 3, "Y")?]) } else { None };
                Step::Scroll { notches: num(a, 1, "NOTCHES")?, at }
            }
            "type" => {
                let text = a[1..].join(" ");
                if text.is_empty() {
                    return Err("type needs TEXT".into());
                }
                let text = text.replace("\\n", "\n").replace("\\t", "\t");
                let missing: String = text.chars().filter(|c| key_for_char(*c).is_none()).collect();
                if !missing.is_empty() {
                    return Err(format!("type: no US key for {missing:?}"));
                }
                Step::Type { keys: text.chars().filter_map(key_for_char).collect(), text }
            }
            "key" => {
                let text = a.get(1).ok_or("key needs a COMBO")?.clone();
                let combo: Option<Vec<KeyCode>> = text.split('+').map(key_of).collect();
                Step::Key { combo: combo.ok_or_else(|| format!("key: unknown key in '{text}'"))?, text }
            }
            "wait" => Step::Wait { ms: num(a, 1, "MS")? },
            "wait-change" => Step::WaitChange {
                timeout: if a.len() > 1 { num(a, 1, "TIMEOUT_MS")? } else { 10_000 },
                quiet: if a.len() > 2 { num(a, 2, "QUIET_MS")? } else { 600 },
            },
            "record" => Step::Record { dir: a.get(1).ok_or("record needs a DIR")?.clone(), ms: num(a, 2, "MS")? },
            other => return Err(format!("unknown step '{other}'")),
        })
    }
}

/// One stream's decoder (`codec`: the wire's `pref` value) and whether it decoded a picture
/// since the video (re)started.
struct Decoder {
    dec: VideoStreamDecoder,
    codec: u8,
    synced: bool,
    fresh: bool,
}

struct Link {
    /// To the network thread (it owns the socket and reads all the time, as a viewer's does: a
    /// desktop drops a viewer whose socket stays full, and decoding here takes its time).
    out: Sender<ClientMsg>,
    /// From it: the desktop's messages, or why the session ended.
    inbox: Receiver<Result<ServerMsg, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
    server: String,
    host_status: String,
    welcome: Option<(Vec<proto::ScreenInfo>, u32, [u32; 2], bool, String)>,
    cursor: Option<(u16, u16)>,
    /// The encoder streams a picture is cut into (`Rate`).
    streams: usize,
    decoders: Vec<Option<Decoder>>,
    /// The stream picture, RGB, composed from the decoded parts.
    canvas: Vec<u8>,
    size: [u32; 2],
    /// Pictures composed since the video (re)started, the last one's frame.
    composed: u64,
    frame: u32,
    paused: bool,
    /// The picture before the first input step since the last `wait-change` (its baseline).
    before: Option<Vec<u8>>,
    /// When the input since the last `wait-change` was sent: its first press (true), else its
    /// first input (false).
    input_at: Option<(Instant, bool)>,
    /// The first picture composed after `input_at` that differs from `before`: how long after the
    /// input, its frame, where.
    first_change: Option<(Duration, u32, [u32; 4])>,
    /// While a `record` step runs: where the packets go.
    record: Option<Recording>,
}

/// A `record` step's files: per stream its elementary stream (opened at its first key frame,
/// with the codec it holds), and the packet list.
struct Recording {
    dir: std::path::PathBuf,
    streams: Vec<Option<(u8, std::fs::File)>>,
    list: std::io::BufWriter<std::fs::File>,
    start: Instant,
    packets: u64,
    keys: u64,
    /// Packets not recorded because their stream changed codec.
    other_codec: u64,
}

impl Recording {
    #[allow(clippy::too_many_arguments)]
    fn packet(&mut self, key: bool, data: &[u8], s: usize, frame: u32, x0: u32, y0: u32, codec: u8, ns: [u64; 3]) -> Result<(), String> {
        use std::io::Write;
        if self.streams.len() <= s {
            self.streams.resize_with(s + 1, || None);
        }
        if self.streams[s].is_none() {
            if !key {
                return Ok(());
            }
            let ext = if codec == proto::pref::HEVC { "hevc" } else { "h264" };
            let path = self.dir.join(format!("stream-{s}.{ext}"));
            self.streams[s] = Some((codec, std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?));
        }
        let (held, file) = self.streams[s].as_mut().unwrap();
        if *held != codec {
            // the file is one codec's bitstream: the new codec's packets would not decode in it
            self.other_codec += 1;
            return Ok(());
        }
        file.write_all(data).map_err(|e| format!("record: {e}"))?;
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        let [built, encoded, sent] = ns;
        writeln!(self.list, "{ms:.1} {s} {frame} {x0} {y0} {codec} {} {} {built} {encoded} {sent}", key as u8, data.len()).map_err(|e| format!("record: {e}"))?;
        self.packets += 1;
        self.keys += key as u64;
        Ok(())
    }
}

impl Link {
    fn connect(host: &str, port: &str) -> Result<Link, String> {
        let (mut stream, peer, authorization) = makepad_network::fleet::id::connect(host, port).map_err(|e| format!("connect: {e}"))?;
        let caps = proto::caps::FULL_CHROMA | proto::caps::HEVC | proto::caps::VIEW_ONLY;
        let hello = ClientMsg::Hello { version: proto::VERSION, name: "agent".into(), authorization, screen: -1, paused: true, caps };
        proto::write_all(&mut stream, &hello.encode()).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_millis(2))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        let (out, out_rx) = channel::<ClientMsg>();
        let (in_tx, inbox) = channel();
        let thread = std::thread::spawn(move || {
            let end = network(stream, out_rx, &in_tx);
            let _ = in_tx.send(Err(end));
        });
        let mut link = Link {
            out,
            inbox,
            thread: Some(thread),
            server: peer.name,
            host_status: String::new(),
            welcome: None,
            cursor: None,
            streams: 0,
            decoders: Vec::new(),
            canvas: Vec::new(),
            size: [0, 0],
            composed: 0,
            frame: 0,
            paused: true,
            before: None,
            input_at: None,
            first_change: None,
            record: None,
        };
        let until = Instant::now() + Duration::from_secs(10);
        while link.welcome.is_none() {
            if Instant::now() > until {
                return Err("no welcome in 10 s".into());
            }
            link.pump(Duration::from_millis(20))?;
        }
        Ok(link)
    }

    /// Ends the session once what was sent went out.
    fn close(mut self) {
        let thread = self.thread.take();
        drop(self);
        if let Some(t) = thread {
            let _ = t.join();
        }
    }

    fn send(&mut self, msg: ClientMsg) -> Result<(), String> {
        self.out.send(msg).map_err(|_| "the session ended".to_string())
    }

    fn stream_size(&self) -> [u32; 2] {
        self.welcome.as_ref().map_or([0, 0], |w| w.2)
    }

    /// Reads and handles what arrives for `for_`.
    fn pump(&mut self, for_: Duration) -> Result<(), String> {
        let until = Instant::now() + for_;
        loop {
            match self.inbox.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(msg) => self.handle(msg?)?,
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => return Err("the session ended".into()),
            }
            if Instant::now() >= until {
                // what already came is handled too, but only a bounded batch: a 4K desktop's video can
                // arrive faster than it decodes, and draining until the inbox is empty never ended
                for _ in 0..PUMP_DRAIN_MAX {
                    let Ok(msg) = self.inbox.try_recv() else { break };
                    self.handle(msg?)?;
                }
                return Ok(());
            }
        }
    }

    fn handle(&mut self, msg: ServerMsg) -> Result<(), String> {
        match msg {
            ServerMsg::Welcome { screens, screen, stream, input, input_note, server, .. } => {
                self.server = server;
                self.welcome = Some((screens, screen, stream, input, input_note));
            }
            ServerMsg::Refused { reason } => return Err(format!("refused: {reason}")),
            ServerMsg::Bye { reason } => return Err(format!("bye: {reason}")),
            ServerMsg::Stream { screen, stream } => {
                if let Some(w) = self.welcome.as_mut() {
                    w.1 = screen;
                    w.2 = stream;
                }
                self.restart_video();
            }
            ServerMsg::Rate { streams, fps, why, .. } => {
                self.streams = streams.max(1) as usize;
                let status = if fps == 0 { why } else { String::new() };
                if status != self.host_status && !status.is_empty() {
                    eprintln!("desktop: {status}");
                }
                self.host_status = status;
            }
            ServerMsg::Cursor { x, y, .. } => self.cursor = Some((x, y)),
            ServerMsg::Video { key, data, stream, frame, x0, y0, codec, built_ns, encoded_ns, sent_ns, .. } => {
                if let Some(r) = self.record.as_mut() {
                    // as it came: no decoding, so no backlog and no key frames asked for a decoder
                    r.packet(key, &data, stream as usize, frame, x0, y0, codec, [built_ns, encoded_ns, sent_ns])?;
                } else if !self.paused {
                    self.video(key, &data, stream as usize, frame, x0, y0, codec)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn restart_video(&mut self) {
        for d in self.decoders.iter_mut().flatten() {
            d.synced = false;
            d.fresh = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn video(&mut self, key: bool, data: &[u8], s: usize, frame: u32, x0: u32, y0: u32, codec: u8) -> Result<(), String> {
        if self.decoders.len() <= s {
            self.decoders.resize_with(s + 1, || None);
        }
        if self.decoders[s].as_ref().is_none_or(|d| d.codec != codec && key) {
            let c = if codec == proto::pref::HEVC { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
            let dec = VideoStreamDecoder::new(c).map_err(|e| format!("decoder: {e:?}"))?;
            self.decoders[s] = Some(Decoder { dec, codec, synced: false, fresh: false });
        }
        let size = self.stream_size();
        if self.size != size {
            self.size = size;
            self.canvas = vec![0u8; size[0] as usize * size[1] as usize * 3];
        }
        let d = self.decoders[s].as_mut().unwrap();
        if key {
            d.synced = true;
        }
        if !d.synced {
            return Ok(());
        }
        let Ok(frames) = d.dec.push_packet(data, frame as i64) else {
            // a damaged reference: a new decoder from the key frame asked for
            self.decoders[s] = None;
            return self.send(ClientMsg::RequestKeyframe);
        };
        let (cw, ch) = (size[0] as usize, size[1] as usize);
        for f in frames {
            let rgb = f.to_rgb8();
            let (fw, fh) = (f.width as usize, f.height as usize);
            let (x0, y0) = (x0 as usize, y0 as usize);
            if x0 >= cw {
                continue;
            }
            let w = fw.min(cw - x0);
            for row in 0..fh.min(ch.saturating_sub(y0)) {
                let src = &rgb[row * fw * 3..(row * fw + w) * 3];
                let at = ((y0 + row) * cw + x0) * 3;
                self.canvas[at..at + w * 3].copy_from_slice(src);
            }
            d.fresh = true;
        }
        let n = self.streams.max(1);
        if self.decoders.len() >= n && self.decoders[..n].iter().all(|d| d.as_ref().is_some_and(|d| d.fresh)) {
            self.composed += 1;
            self.frame = frame;
            // after input: the first picture that differs from the one before it, timed as composed
            if let (Some((sent, _)), None, Some(base)) = (self.input_at, self.first_change, self.before.as_ref()) {
                let now = Instant::now();
                if let Some(b) = diff_box(base, &self.canvas, self.size[0]) {
                    self.first_change = Some((now.duration_since(sent), frame, b));
                }
            }
        }
        Ok(())
    }

    /// Video on (it stays on for the rest of the session: the picture is kept current), until
    /// a whole picture is composed, and what follows within `settle`.
    fn picture(&mut self, settle: Duration) -> Result<(), String> {
        self.video_on()?;
        let start = Instant::now();
        let mut said = false;
        while self.composed == 0 {
            // a desktop sends pictures when its screen changes; one that does not send the last
            // picture again to a viewer that comes waits for the next change (a clock: a minute)
            if !said && start.elapsed() > Duration::from_secs(3) {
                eprintln!("drive: no picture yet: the screen is still and the desktop waits for it to change");
                said = true;
            }
            if start.elapsed() > Duration::from_secs(70) {
                return Err("no picture in 70 s".into());
            }
            self.pump(Duration::from_millis(10))?;
        }
        self.pump(settle)
    }

    fn video_on(&mut self) -> Result<(), String> {
        if self.paused {
            self.paused = false;
            self.composed = 0;
            self.restart_video();
            self.send(ClientMsg::Resume)?;
        }
        Ok(())
    }

    /// A stream pixel on the wire's 0..=65535.
    fn wire(&self, x: u32, y: u32) -> Result<(u16, u16), String> {
        let [w, h] = self.stream_size();
        if x >= w || y >= h {
            return Err(format!("{x},{y} is outside the stream ({w}x{h})"));
        }
        let f = |v: u32, n: u32| ((v as f64 + 0.5) / n as f64 * 65535.0).round().min(65535.0) as u16;
        Ok((f(x, w), f(y, h)))
    }

    /// Input is being sent now: a press is the input's time (a change seen after an earlier move
    /// was that move's, e.g. a hover, and is forgotten), else the first input is.
    fn note_input(&mut self, press: bool) {
        match self.input_at {
            Some((_, true)) => {}
            Some((_, false)) if !press => {}
            _ => {
                self.input_at = Some((Instant::now(), press));
                self.first_change = None;
            }
        }
    }

    fn pointer(&mut self, x: u32, y: u32) -> Result<(), String> {
        let (x, y) = self.wire(x, y)?;
        self.note_input(false);
        self.send(ClientMsg::Pointer { x, y })?;
        self.pump(Duration::from_millis(30))
    }

    fn button(&mut self, down: bool, button: u8) -> Result<(), String> {
        self.note_input(down);
        self.send(ClientMsg::Button { down, button })?;
        self.pump(Duration::from_millis(40))
    }

    fn key(&mut self, down: bool, k: KeyCode) -> Result<(), String> {
        self.note_input(down);
        self.send(ClientMsg::Key { down, keycode: k.to_index(), mods: 0 })?;
        self.pump(Duration::from_millis(12))
    }

    fn input_ok(&self) -> Result<(), String> {
        match &self.welcome {
            Some((_, _, _, true, _)) => Ok(()),
            Some((_, _, _, false, note)) => Err(format!("the desktop takes no input: {note}")),
            None => Err("no welcome".into()),
        }
    }

    fn cursor_px(&self) -> String {
        let [w, h] = self.stream_size();
        self.cursor.map_or("-".into(), |(x, y)| format!("{},{}", (x as u64 * w as u64 / 65536), (y as u64 * h as u64 / 65536)))
    }

    fn run(&mut self, step: &Step) -> Result<String, String> {
        let input = !matches!(step, Step::Info | Step::Shot { .. } | Step::Wait { .. } | Step::WaitChange { .. } | Step::Record { .. });
        if input && self.before.is_none() && !self.paused && self.composed > 0 {
            self.before = Some(self.canvas.clone());
        }
        match step {
            Step::Info => {
                let (screens, screen, stream, input, note) = self.welcome.clone().unwrap();
                let list: Vec<String> = screens.iter().enumerate().map(|(i, s)| format!("{i}:{}x{}@{},{}", s.width, s.height, s.x, s.y)).collect();
                Ok(format!(
                    "info {} stream {}x{} screen {screen} of [{}] input {}{} cursor {}{}",
                    self.server,
                    stream[0],
                    stream[1],
                    list.join(" "),
                    if input { "ok" } else { "off" },
                    if input { String::new() } else { format!(" ({note})") },
                    self.cursor_px(),
                    if self.host_status.is_empty() { String::new() } else { format!("; {}", self.host_status) }
                ))
            }
            Step::Shot { path, crop, scale } => {
                self.picture(Duration::from_millis(250))?;
                let [w, h] = self.size;
                let [cx, cy, cw, ch] = crop.unwrap_or([0, 0, w, h]);
                if cx + cw > w || cy + ch > h {
                    return Err(format!("crop {cx},{cy},{cw},{ch} is outside the stream ({w}x{h})"));
                }
                let n = scale.unwrap_or_else(|| cw.max(ch).div_ceil(1920)).max(1);
                let (ow, oh) = (cw / n, ch / n);
                let mut out = vec![0u8; ow as usize * oh as usize * 3];
                let stride = w as usize * 3;
                for oy in 0..oh as usize {
                    for ox in 0..ow as usize {
                        let mut sum = [0u32; 3];
                        for dy in 0..n as usize {
                            let row = (cy as usize + oy * n as usize + dy) * stride;
                            for dx in 0..n as usize {
                                let at = row + (cx as usize + ox * n as usize + dx) * 3;
                                for c in 0..3 {
                                    sum[c] += self.canvas[at + c] as u32;
                                }
                            }
                        }
                        let o = (oy * ow as usize + ox) * 3;
                        for c in 0..3 {
                            out[o + c] = (sum[c] / (n * n)) as u8;
                        }
                    }
                }
                let png = encode_png(ow, oh, &out)?;
                if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::write(path, png).map_err(|e| format!("{path}: {e}"))?;
                Ok(format!(
                    "shot {path} {ow}x{oh} frame {}: stream px = png px * {n} + ({cx},{cy}); stream {w}x{h}; cursor {}",
                    self.frame,
                    self.cursor_px()
                ))
            }
            Step::Move { x, y } => {
                self.input_ok()?;
                self.pointer(*x, *y)?;
                Ok(format!("move {x},{y} ok"))
            }
            Step::Click { x, y, button, double } => {
                self.input_ok()?;
                self.pointer(*x, *y)?;
                for _ in 0..if *double { 2 } else { 1 } {
                    self.button(true, *button)?;
                    self.button(false, *button)?;
                }
                Ok(format!("click {x},{y}{}{} ok", if *button == proto::button::LEFT { "" } else if *button == proto::button::RIGHT { " right" } else { " middle" }, if *double { " double" } else { "" }))
            }
            Step::Drag { from, to, button } => {
                self.input_ok()?;
                self.pointer(from[0], from[1])?;
                self.button(true, *button)?;
                let steps = 12;
                for i in 1..=steps {
                    let lerp = |a: u32, b: u32| (a as i64 + (b as i64 - a as i64) * i / steps) as u32;
                    self.pointer(lerp(from[0], to[0]), lerp(from[1], to[1]))?;
                }
                self.pump(Duration::from_millis(60))?;
                self.button(false, *button)?;
                Ok(format!("drag {},{} -> {},{} ok", from[0], from[1], to[0], to[1]))
            }
            Step::Scroll { notches, at } => {
                self.input_ok()?;
                if let Some([x, y]) = at {
                    self.pointer(*x, *y)?;
                }
                for _ in 0..notches.unsigned_abs() {
                    self.note_input(true);
                    self.send(ClientMsg::Scroll { dx: 0, dy: 120 * notches.signum() })?;
                    self.pump(Duration::from_millis(30))?;
                }
                Ok(format!("scroll {notches} ok"))
            }
            Step::Type { keys, text } => {
                self.input_ok()?;
                for &(k, shift) in keys {
                    if shift {
                        self.key(true, KeyCode::Shift)?;
                    }
                    self.key(true, k)?;
                    self.key(false, k)?;
                    if shift {
                        self.key(false, KeyCode::Shift)?;
                    }
                }
                Ok(format!("type {} chars ok", text.chars().count()))
            }
            Step::Key { combo, text } => {
                self.input_ok()?;
                for k in combo {
                    self.key(true, *k)?;
                }
                for k in combo.iter().rev() {
                    self.key(false, *k)?;
                }
                Ok(format!("key {text} ok"))
            }
            Step::Wait { ms } => {
                self.pump(Duration::from_millis(*ms))?;
                Ok(format!("wait {ms} ok"))
            }
            Step::WaitChange { timeout, quiet } => {
                let start = Instant::now();
                // from the picture before this session's input since the last wait, else now
                // (kept in `before` while this step runs: pictures composed meanwhile are compared too)
                let before = self.before.clone();
                let had_base = before.is_some();
                self.picture(Duration::from_millis(100))?;
                let base = before.unwrap_or_else(|| self.canvas.clone());
                let mut last = self.canvas.clone();
                let now = Instant::now();
                let (mut changed_at, mut still_since, mut seen) = (diff_box(&base, &self.canvas, self.size[0]).map(|_| now), now, self.composed);
                let deadline = start + Duration::from_millis(*timeout);
                let result = loop {
                    self.pump(Duration::from_millis(20))?;
                    if self.composed != seen {
                        seen = self.composed;
                        let now = Instant::now();
                        if diff_box(&last, &self.canvas, self.size[0]).is_some() {
                            still_since = now;
                            last.copy_from_slice(&self.canvas);
                        }
                        if changed_at.is_none() && diff_box(&base, &self.canvas, self.size[0]).is_some() {
                            changed_at = Some(now);
                        }
                    }
                    if let Some(at) = changed_at {
                        if still_since.elapsed() >= Duration::from_millis(*quiet) {
                            let b = diff_box(&base, &self.canvas, self.size[0]);
                            break match b {
                                Some([x, y, w, h]) => format!(
                                    "wait-change changed at {} ms, still at {} ms; box {x},{y},{w},{h}",
                                    at.duration_since(start).as_millis(),
                                    still_since.duration_since(start).as_millis()
                                ),
                                None => format!("wait-change changed at {} ms and back to as it was", at.duration_since(start).as_millis()),
                            };
                        }
                    }
                    if Instant::now() >= deadline {
                        break match changed_at {
                            Some(at) => format!("wait-change changed at {} ms, still moving at {timeout} ms", at.duration_since(start).as_millis()),
                            None => format!("wait-change no change in {timeout} ms"),
                        };
                    }
                };
                let first = match (self.input_at.take(), self.first_change.take()) {
                    (Some((_, press)), Some((after, f, [x, y, w, h]))) => format!(
                        "; first change {:.1} ms after the {} (frame {f}, box {x},{y},{w},{h})",
                        after.as_secs_f64() * 1000.0,
                        if press { "press" } else { "input" }
                    ),
                    (Some(_), None) if had_base => "; no changed picture after the input".to_string(),
                    _ => String::new(),
                };
                self.before = None;
                Ok(format!("{result}{first}"))
            }
            Step::Record { dir, ms } => {
                // undecoded: it does not wait for a picture this machine can decode
                std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
                let path = std::path::Path::new(dir).join("packets.txt");
                let list = std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?);
                self.record = Some(Recording { dir: dir.into(), streams: Vec::new(), list, start: Instant::now(), packets: 0, keys: 0, other_codec: 0 });
                self.video_on()?;
                self.send(ClientMsg::RequestKeyframe)?;
                self.pump(Duration::from_millis(*ms))?;
                let mut r = self.record.take().unwrap();
                std::io::Write::flush(&mut r.list).map_err(|e| format!("record: {e}"))?;
                // the picture missed what was recorded: decode again from a new key frame
                self.restart_video();
                self.composed = 0;
                self.send(ClientMsg::RequestKeyframe)?;
                let streams = r.streams.iter().filter(|f| f.is_some()).count();
                let other = if r.other_codec > 0 { format!(", {} packets after a codec change not recorded", r.other_codec) } else { String::new() };
                Ok(format!("record {dir} {} packets ({} key) in {ms} ms, {streams} streams{other}", r.packets, r.keys))
            }
        }
    }
}

/// The session's socket: reads all the time, writes what `out` brings, pings once a second.
/// Returns why it ended.
fn network(mut stream: makepad_network::SocketStream, out: Receiver<ClientMsg>, to: &Sender<Result<ServerMsg, String>>) -> String {
    let mut inbox = Inbox::default();
    let mut buf = vec![0u8; 1 << 20];
    let epoch = Instant::now();
    let mut last_ping = Instant::now();
    loop {
        loop {
            match out.try_recv() {
                Ok(msg) => {
                    if let Err(e) = proto::write_all(&mut stream, &msg.encode()) {
                        return format!("write: {e}");
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // the steps are done: the session ends (the desktop lets go of what it held)
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return "done".into(),
            }
        }
        if last_ping.elapsed() >= Duration::from_secs(1) {
            last_ping = Instant::now();
            let ping = ClientMsg::Ping { client_ns: epoch.elapsed().as_nanos() as u64 };
            if let Err(e) = proto::write_all(&mut stream, &ping.encode()) {
                return format!("write: {e}");
            }
        }
        match stream.read(&mut buf) {
            Ok(0) => return "the desktop closed the session".into(),
            Ok(n) => inbox.feed(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return format!("read: {e}"),
        }
        loop {
            let msg = match inbox.next() {
                Ok(Some((ty, payload))) => ServerMsg::decode(ty, &payload).map_err(|e| e.to_string()),
                Ok(None) => break,
                Err(e) => Err(e.to_string()),
            };
            let failed = msg.is_err();
            if to.send(msg).is_err() || failed {
                return "done".into();
            }
        }
    }
}

/// The box (x, y, w, h) of the pixels that differ (any channel by more than 24, sampled every
/// 2nd pixel and row: the decoder's noise stays out), None when none do.
fn diff_box(a: &[u8], b: &[u8], width: u32) -> Option<[u32; 4]> {
    if a.len() != b.len() || width == 0 {
        return None;
    }
    let w = width as usize;
    let h = a.len() / 3 / w;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in (0..h).step_by(2) {
        let row = y * w * 3;
        for x in (0..w).step_by(2) {
            let i = row + x * 3;
            if (0..3).any(|c| a[i + c].abs_diff(b[i + c]) > 24) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != usize::MAX).then(|| [x0 as u32, y0 as u32, (x1 - x0 + 2) as u32, (y1 - y0 + 2) as u32])
}

fn encode_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    use makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    };
    let options = EncoderOptions::default().set_width(width as usize).set_height(height as usize).set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGB);
    let mut out = Vec::new();
    PngEncoder::new(rgb, options).encode(&mut out).map_err(|e| format!("png: {e:?}"))?;
    Ok(out)
}
