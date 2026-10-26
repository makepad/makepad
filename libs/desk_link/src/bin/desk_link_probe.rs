//! desk-link-probe <host[:port]> [--secs N] [--input] [--paused] [--flap MS] [--codec auto|h264|hevc] [--chroma auto|444|420] [--preset auto|sharp|fast] [--mbps N] [--fps N] [--h264only] [--stall MS,EVERY_MS] [--window WxH[@SCALE] [--drive]] [--hz N] [--dump DIR] [--420] [--keyreq MS] [--decode MS[,serial]] [--key CODE@AT_MS+HOLD_MS]... [--screen N]: a headless Desk Link viewer for
//! checks. It connects with this machine's fleet credential, prints the Welcome, counts the
//! pictures and bytes, pings once a second (RTT), and with `--input` moves the desktop's pointer
//! between two points twice a second, timing each move until the desktop's own Cursor message
//! reports the pointer there (input injection end to end). `--window` tells the desktop a viewer
//! window of that many device pixels (scale in hundredths, default 200) and `--hz` its display
//! refresh (default 120 with `--window`). The probe is view-only (`caps::VIEW_ONLY`): a desktop
//! that follows its viewer takes that size only with `--drive`, so a probe never resizes a
//! desktop someone is watching.
//! `--dump DIR` writes each encoder stream (from its first key frame) to DIR/stream-<i>.h264 and
//! its tile position to DIR/tiles.txt (`<i> <x0> <y0>`), for decoding them elsewhere.
//!
//! It checks the bitstream as it goes: each stream starts (and restarts after a resume) at a
//! key frame, every key frame carries SPS, PPS and an IDR slice in band, and the `key` flag says
//! exactly whether a picture has an IDR. `--keyreq MS` asks for a key frame every MS ms and
//! counts the answers.
//!
//! `--decode MS` stands in for a viewer whose decoder takes MS ms for a whole picture, decoding
//! the bands at once (a band's share each: MS / parts; `MS,serial`: one after another, MS
//! whatever the bands): it reports that as `Stats.decode_us` once a second, as a viewer does,
//! so the desktop's decode-aware bands can be checked without a slow decoder.

use makepad_desk_link::{self as proto, ClientMsg, Inbox, ServerMsg};
use std::io::Read;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(target) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: desk-link-probe <host[:port]> [--secs N] [--input] [--paused] [--flap MS] [--codec auto|h264|hevc] [--chroma auto|444|420] [--preset auto|sharp|fast] [--mbps N] [--fps N] [--h264only] [--stall MS,EVERY_MS] [--window WxH[@SCALE] [--drive]] [--hz N] [--dump DIR] [--420] [--keyreq MS] [--decode MS[,serial]] [--key CODE@AT_MS+HOLD_MS]... [--screen N]");
        std::process::exit(2);
    };
    let secs: u64 = args.windows(2).find(|w| w[0] == "--secs").and_then(|w| w[1].parse().ok()).unwrap_or(10);
    let input = args.iter().any(|a| a == "--input");
    let paused = args.iter().any(|a| a == "--paused");
    // --flap MS: pause and resume every MS milliseconds (the desktop releases its picture
    // texture whenever nobody watches: a lifetime test)
    let flap: Option<u64> = args.windows(2).find(|w| w[0] == "--flap").and_then(|w| w[1].parse().ok());
    // --codec / --chroma / --preset / --mbps / --fps: this viewer's Encoding, sent after the
    // Welcome (any of them given)
    let pick = |name: &str, table: &[(&str, u8)]| args.windows(2).find(|w| w[0] == name).map(|w| table.iter().find(|(n, _)| *n == w[1]).map_or(0, |(_, v)| *v));
    let number = |name: &str| args.windows(2).find(|w| w[0] == name).and_then(|w| w[1].parse::<u16>().ok());
    let codec = pick("--codec", &[("h264", proto::pref::H264), ("hevc", proto::pref::HEVC)]);
    let chroma = pick("--chroma", &[("444", proto::pref::FULL), ("420", proto::pref::HALF)]);
    let preset = pick("--preset", &[("sharp", proto::pref::SHARP), ("fast", proto::pref::FAST)]);
    let (mbps, fps) = (number("--mbps"), number("--fps"));
    let encoding = (codec.is_some() || chroma.is_some() || preset.is_some() || mbps.is_some() || fps.is_some()).then(|| ClientMsg::Encoding {
        chroma: chroma.unwrap_or(0),
        codec: codec.unwrap_or(0),
        preset: preset.unwrap_or(0),
        mbps: mbps.unwrap_or(0),
        fps: fps.unwrap_or(0),
    });
    // --stall MS,EVERY_MS: stop reading for MS every EVERY_MS (a viewer that does not read for a
    // while: the desktop's socket fills and it must hold pictures back, not restart the stream)
    let stall: Option<(u64, u64)> = args.windows(2).find(|w| w[0] == "--stall").and_then(|w| {
        let (a, b) = w[1].split_once(',')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    });
    let window: Option<(u32, u32, u16)> = args.windows(2).find(|w| w[0] == "--window").and_then(|w| {
        let (size, scale) = w[1].split_once('@').unwrap_or((&w[1], "200"));
        let (a, b) = size.split_once('x')?;
        Some((a.parse().ok()?, b.parse().ok()?, scale.parse().ok()?))
    });
    let hz: Option<u16> = args.windows(2).find(|w| w[0] == "--hz").and_then(|w| w[1].parse().ok()).or(window.map(|_| 120));
    let dump: Option<std::path::PathBuf> = args.windows(2).find(|w| w[0] == "--dump").map(|w| w[1].clone().into());
    // --420: a viewer whose decoder does not take 4:4:4 (the desktop streams 4:2:0)
    let caps = if args.iter().any(|a| a == "--420") { 0 } else { proto::caps::FULL_CHROMA };
    // --h264only: a viewer without an H.265 decoder
    let caps = if args.iter().any(|a| a == "--h264only") { caps } else { caps | proto::caps::HEVC };
    // view-only unless --drive: a probe never resizes a followed desktop someone is using
    let caps = if args.iter().any(|a| a == "--drive") { caps } else { caps | proto::caps::VIEW_ONLY };
    let keyreq: Option<u64> = args.windows(2).find(|w| w[0] == "--keyreq").and_then(|w| w[1].parse().ok());
    let decode: Option<(f64, bool)> = args.windows(2).find(|w| w[0] == "--decode").and_then(|w| {
        let (ms, serial) = w[1].split_once(',').map_or((w[1].as_str(), false), |(a, b)| (a, b == "serial"));
        Some((ms.parse().ok()?, serial))
    });
    // --key CODE@AT_MS+HOLD_MS (repeatable): holds makepad KeyCode index CODE down from AT_MS after
    // the Welcome for HOLD_MS (e.g. 18@2000+3000: W for 3 s), to drive an app from a script
    let keys: Vec<(u32, u64, u64)> = args.windows(2).filter(|w| w[0] == "--key").filter_map(|w| {
        let (code, rest) = w[1].split_once('@')?;
        let (at, hold) = rest.split_once('+')?;
        Some((code.parse().ok()?, at.parse().ok()?, hold.parse().ok()?))
    }).collect();
    // --screen N: the screen (a JIT host: its source) to watch, as the Hello asks
    let screen: i32 = args.windows(2).find(|w| w[0] == "--screen").and_then(|w| w[1].parse().ok()).unwrap_or(-1);
    let opts = Opts { window, hz, dump, caps, keyreq, decode, keys, screen };
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.to_string()),
        _ => (target.clone(), proto::DEFAULT_PORT.to_string()),
    };
    if let Err(e) = run(&host, &port, secs, input, paused, flap, encoding, stall, opts) {
        eprintln!("probe: {e}");
        std::process::exit(1);
    }
}

struct Opts {
    window: Option<(u32, u32, u16)>,
    hz: Option<u16>,
    dump: Option<std::path::PathBuf>,
    caps: u32,
    keyreq: Option<u64>,
    decode: Option<(f64, bool)>,
    keys: Vec<(u32, u64, u64)>,
    screen: i32,
}

#[allow(clippy::too_many_arguments)]
fn run(host: &str, port: &str, secs: u64, input: bool, paused: bool, flap: Option<u64>, encoding: Option<ClientMsg>, stall: Option<(u64, u64)>, opts: Opts) -> Result<(), String> {
    use std::io::Write;
    // --dump: per stream its file (opened at its first key frame) and position
    let mut dumps: Vec<Option<std::fs::File>> = Vec::new();
    let mut positions: Vec<(u8, u32, u32)> = Vec::new();
    if let Some(dir) = &opts.dump {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // bytes and pictures per stream (Mbit/s per tile)
    let mut per_stream: Vec<(u64, u64)> = Vec::new();
    let mut check = Check::default();
    let (mut last_keyreq, mut keyreqs) = (Instant::now(), 0u32);
    let keyreq = opts.keyreq;
    let (mut stream, server, authorization) = makepad_network::fleet::id::connect(host, port).map_err(|e| format!("connect: {e}"))?;
    println!("probe: desktop {} ({})", server.name, server.role.as_str());
    let hello = ClientMsg::Hello { version: proto::VERSION, name: "probe".into(), authorization, screen: opts.screen, paused, caps: opts.caps };
    proto::write_all(&mut stream, &hello.encode()).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let mut inbox = Inbox::default();
    let mut buf = vec![0u8; 1 << 20];
    let (mut pictures, mut bytes, mut keys) = (0u64, 0u64, 0u64);
    let mut last_ping = Instant::now() - Duration::from_secs(5);
    // the last picture's frame and its parts (`--decode`'s Stats)
    let mut last_frame: Option<(u32, u8)> = None;
    let mut rtts: Vec<f64> = Vec::new();
    let mut welcomed = false;
    // --input: the point the pointer was sent to and when
    let mut moving: Option<((u16, u16), Instant)> = None;
    let mut next_move = Instant::now();
    // --key: when the Welcome came, and per key 0 not sent, 1 down, 2 down and up
    let mut welcomed_at: Option<Instant> = None;
    let mut keys_sent = vec![0u8; opts.keys.len()];
    let mut toggle = false;
    let mut latencies: Vec<f64> = Vec::new();
    // the desktop's frame submit to its bitstream, per picture (ms), and the encoder streams seen
    let mut encodes: Vec<f64> = Vec::new();
    let mut streams = 0u32;
    let mut slowest = (u32::MAX, 0f64);
    let epoch = Instant::now();
    let mut flapped = (Instant::now(), paused, 0u32);
    let (mut last_stall, mut stalls) = (Instant::now(), 0u32);
    // the longest time between two pictures, and the encode time of the slowest one
    let (mut last_picture, mut gap_max, mut encode_max): (Option<Instant>, f64, f64) = (None, 0.0, 0.0);
    while start.elapsed() < Duration::from_secs(secs) {
        if let Some((ms, every)) = stall.filter(|_| welcomed) {
            if last_stall.elapsed() >= Duration::from_millis(every) {
                std::thread::sleep(Duration::from_millis(ms));
                last_stall = Instant::now();
                stalls += 1;
            }
        }
        if let Some(ms) = flap.filter(|_| welcomed) {
            if flapped.0.elapsed() >= Duration::from_millis(ms) {
                flapped = (Instant::now(), !flapped.1, flapped.2 + 1);
                let msg = if flapped.1 { ClientMsg::Pause } else { ClientMsg::Resume };
                if !flapped.1 {
                    check.resumed();
                }
                proto::write_all(&mut stream, &msg.encode()).map_err(|e| e.to_string())?;
            }
        }
        if let Some(ms) = keyreq.filter(|_| welcomed) {
            if last_keyreq.elapsed() >= Duration::from_millis(ms) {
                last_keyreq = Instant::now();
                keyreqs += 1;
                proto::write_all(&mut stream, &ClientMsg::RequestKeyframe.encode()).map_err(|e| e.to_string())?;
            }
        }
        if last_ping.elapsed() >= Duration::from_secs(1) {
            last_ping = Instant::now();
            let ns = epoch.elapsed().as_nanos() as u64;
            proto::write_all(&mut stream, &ClientMsg::Ping { client_ns: ns }.encode()).map_err(|e| e.to_string())?;
            if let (Some((ms, serial)), Some((frame, parts))) = (opts.decode, last_frame) {
                let decode_ms = if serial { ms } else { ms / parts.max(1) as f64 };
                proto::write_all(&mut stream, &ClientMsg::Stats { frame, decode_us: (decode_ms * 1000.0) as u32, present_us: 0 }.encode()).map_err(|e| e.to_string())?;
            }
        }
        if let Some(at) = welcomed_at {
            let ms = at.elapsed().as_millis() as u64;
            for (i, &(code, from, hold)) in opts.keys.iter().enumerate() {
                let want = if ms >= from + hold { 2 } else if ms >= from { 1 } else { 0 };
                if want > keys_sent[i] {
                    if keys_sent[i] == 0 { proto::write_all(&mut stream, &ClientMsg::Key { down: true, keycode: code, mods: 0 }.encode()).map_err(|e| e.to_string())?; println!("probe: key {code} down"); }
                    if want == 2 { proto::write_all(&mut stream, &ClientMsg::Key { down: false, keycode: code, mods: 0 }.encode()).map_err(|e| e.to_string())?; println!("probe: key {code} up"); }
                    keys_sent[i] = want;
                }
            }
        } else if welcomed {
            welcomed_at = Some(Instant::now());
        }
        if input && welcomed && Instant::now() >= next_move {
            next_move = Instant::now() + Duration::from_millis(500);
            toggle = !toggle;
            let at = if toggle { (16384u16, 16384u16) } else { (49152, 49152) };
            proto::write_all(&mut stream, &ClientMsg::Pointer { x: at.0, y: at.1 }.encode()).map_err(|e| e.to_string())?;
            moving = Some((at, Instant::now()));
        }
        match stream.read(&mut buf) {
            Ok(0) => return Err("closed".into()),
            Ok(n) => inbox.feed(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
        while let Some((ty, payload)) = inbox.next().map_err(|e| e.to_string())? {
            if std::env::var_os("PROBE_VERBOSE").is_some() {
                println!("probe: message type {ty}, {} bytes", payload.len());
            }
            match ServerMsg::decode(ty, &payload).map_err(|e| e.to_string())? {
                ServerMsg::Welcome { screens, screen, stream: stream_size, input, input_note, server, .. } => {
                    welcomed = true;
                    if let Some(hz) = opts.hz {
                        proto::write_all(&mut stream, &ClientMsg::Refresh { hz }.encode()).map_err(|e| e.to_string())?;
                    }
                    if let Some((width, height, scale)) = opts.window {
                        proto::write_all(&mut stream, &ClientMsg::Window { width, height, scale }.encode()).map_err(|e| e.to_string())?;
                    }
                    if let Some(encoding) = &encoding {
                        proto::write_all(&mut stream, &encoding.encode()).map_err(|e| e.to_string())?;
                    }
                    println!("probe: welcome from {server}: screen {screen} of {} ({stream_size:?}), input {}{}", screens.len(), input, if input { String::new() } else { format!(" ({input_note})") });
                }
                ServerMsg::Refused { reason } => return Err(format!("refused: {reason}")),
                ServerMsg::Bye { reason } => return Err(format!("bye: {reason}")),
                ServerMsg::Stream { screen, stream: size } => println!("probe: the stream is now screen {screen} at {size:?}"),
                ServerMsg::Rate { fps, kbps, limit, full_chroma, codec, preset, streams, why } => println!(
                    "probe: the desktop streams {fps} pictures a second at {} Mbit/s {} {} {} in {streams} stream(s) (limit {limit}){}",
                    kbps / 1000,
                    if codec == proto::pref::HEVC { "H.265" } else { "H.264" },
                    if full_chroma { "4:4:4" } else { "4:2:0" },
                    if preset == proto::pref::SHARP { "sharp" } else { "fast" },
                    if why.is_empty() { String::new() } else { format!(": {why}") }
                ),
                ServerMsg::Video { key, data, stream, parts, built_ns, encoded_ns, frame, x0, y0, codec, .. } => {
                    let i = stream as usize;
                    last_frame = Some((frame, parts));
                    if per_stream.len() <= i {
                        per_stream.resize(i + 1, (0, 0));
                        dumps.resize_with(i + 1, || None);
                    }
                    per_stream[i].0 += data.len() as u64;
                    per_stream[i].1 += 1;
                    if let Some(dir) = &opts.dump {
                        if dumps[i].is_none() && key {
                            let path = dir.join(format!("stream-{i}.{}", if codec == proto::pref::HEVC { "h265" } else { "h264" }));
                            dumps[i] = Some(std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?);
                            positions.push((stream, x0, y0));
                        }
                        if let Some(f) = dumps[i].as_mut() {
                            f.write_all(&data).map_err(|e| e.to_string())?;
                        }
                    }
                    // a picture is counted with its last part (tiles) or as itself (interleaved);
                    // its encode time is its slowest part's
                    bytes += data.len() as u64;
                    streams = streams.max(stream as u32 + 1);
                    check.picture(stream, key, codec == proto::pref::HEVC, &data);
                    let ms = encoded_ns.saturating_sub(built_ns) as f64 / 1e6;
                    slowest = if slowest.0 == frame { (frame, slowest.1.max(ms)) } else { (frame, ms) };
                    if parts <= 1 || stream + 1 == parts {
                        pictures += 1;
                        keys += key as u64;
                        encodes.push(slowest.1);
                        encode_max = encode_max.max(slowest.1);
                        let now = Instant::now();
                        if let Some(at) = last_picture {
                            gap_max = gap_max.max(now.duration_since(at).as_secs_f64() * 1000.0);
                        }
                        last_picture = Some(now);
                    }
                }
                ServerMsg::Pong { client_ns, .. } => rtts.push((epoch.elapsed().as_nanos() as u64 - client_ns) as f64 / 1e6),
                ServerMsg::Cursor { x, y, .. } => {
                    if let Some(((tx, ty), at)) = moving {
                        // the desktop maps the point through its own pixels: within 1%
                        if (x as i32 - tx as i32).abs() < 700 && (y as i32 - ty as i32).abs() < 700 {
                            latencies.push(at.elapsed().as_secs_f64() * 1000.0);
                            moving = None;
                        }
                    }
                }
                other => {
                    if std::env::var_os("PROBE_VERBOSE").is_some() {
                        println!("probe: message {:?}", std::mem::discriminant(&other));
                    }
                }
            }
        }
    }
    let secs_run = start.elapsed().as_secs_f64();
    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    };
    println!(
        "probe: {pictures} pictures ({keys} key) {:.1} fps {:.2} Mbit/s; rtt median {:.2} ms over {}",
        pictures as f64 / secs_run,
        bytes as f64 * 8.0 / secs_run / 1e6,
        median(&mut rtts),
        rtts.len()
    );
    println!("probe: {streams} encoder stream(s); frame submit to bitstream median {:.2} ms, p90 {:.2} ms", median(&mut encodes.clone()), {
        let mut e = encodes.clone();
        e.sort_by(|a, b| a.partial_cmp(b).unwrap());
        e.get(e.len() * 9 / 10).copied().unwrap_or(f64::NAN)
    });
    println!("probe: longest gap between pictures {gap_max:.1} ms; slowest frame submit to bitstream {encode_max:.1} ms");
    for (i, (b, n)) in per_stream.iter().enumerate() {
        println!("probe: stream {i}: {n} parts, {:.2} Mbit/s", *b as f64 * 8.0 / secs_run / 1e6);
    }
    if let Some(dir) = &opts.dump {
        positions.sort();
        let text: String = positions.iter().map(|(i, x, y)| format!("{i} {x} {y}\n")).collect();
        std::fs::write(dir.join("tiles.txt"), text).map_err(|e| e.to_string())?;
        println!("probe: {} stream dump(s) in {}", positions.len(), dir.display());
    }
    if stall.is_some() {
        println!("probe: {stalls} read stalls");
    }
    if flap.is_some() {
        println!("probe: {} pause/resume flips", flapped.2);
    }
    println!("{}", check.report());
    if keyreq.is_some() {
        println!("probe: {keyreqs} key frame requests");
    }
    if input {
        println!("probe: pointer -> cursor round trip median {:.2} ms over {} moves", median(&mut latencies), latencies.len());
    }
    Ok(())
}

/// The bitstream checks (see the module doc).
#[derive(Default)]
struct Check {
    /// Per stream: a key frame seen since the start or the last resume.
    started: Vec<bool>,
    /// Pictures before their stream's first key frame (or after a resume, before its key).
    before_key: u32,
    /// Key frames without SPS, PPS or IDR in band.
    bare_keys: u32,
    /// Pictures whose `key` flag disagrees with an IDR slice in them.
    wrong_flags: u32,
    /// The first SPS's profile_idc, constraint flags (H.265: its tier), level_idc.
    sps: Option<[u8; 3]>,
    /// The streams are H.265.
    hevc: bool,
    /// NAL types seen in key frames (bit per type).
    key_nals: u64,
}

impl Check {
    fn resumed(&mut self) {
        self.started.iter_mut().for_each(|s| *s = false);
    }

    fn picture(&mut self, stream: u8, key: bool, hevc: bool, data: &[u8]) {
        let s = stream as usize;
        if self.started.len() <= s {
            self.started.resize(s + 1, false);
        }
        let nals = nal_units(data);
        // H.264: the type in the low 5 bits; H.265: bits 1..6 of the first header byte
        let ty = |b: u8| if hevc { (b >> 1) & 0x3f } else { b & 0x1f };
        let has = |t: u8| nals.iter().any(|n| n.first().map(|b| ty(*b)) == Some(t));
        // H.265 IDR_W_RADL / IDR_N_LP, VPS + SPS + PPS
        let idr = if hevc { has(19) || has(20) } else { has(5) };
        if key != idr {
            self.wrong_flags += 1;
        }
        if hevc {
            self.hevc = true;
            if key {
                self.started[s] = true;
                if !(has(32) && has(33) && has(34) && idr) {
                    self.bare_keys += 1;
                }
                for n in &nals {
                    if let Some(b) = n.first() {
                        self.key_nals |= 1u64 << ty(*b);
                    }
                }
                if self.sps.is_none() {
                    // after the 2-byte header and vps id / sub layers / nesting: the general
                    // profile_tier_level (profile space, tier, profile_idc; 4 + 6 flag bytes;
                    // level_idc), emulation prevention removed
                    self.sps = nals.iter().find(|n| n.first().map(|b| ty(*b)) == Some(33)).and_then(|n| {
                        let mut r = Vec::new();
                        let mut zeros = 0;
                        for &b in n.iter().skip(2) {
                            if zeros >= 2 && b == 3 {
                                zeros = 0;
                                continue;
                            }
                            zeros = if b == 0 { zeros + 1 } else { 0 };
                            r.push(b);
                        }
                        (r.len() >= 13).then(|| [r[1] & 0x1f, (r[1] >> 5) & 1, r[12]])
                    });
                }
            } else if !self.started[s] {
                self.before_key += 1;
            }
            return;
        }
        if key {
            self.started[s] = true;
            if !(has(7) && has(8) && idr) {
                self.bare_keys += 1;
            }
            for n in &nals {
                if let Some(b) = n.first() {
                    self.key_nals |= 1u64 << (b & 0x1f);
                }
            }
            if self.sps.is_none() {
                self.sps = nals.iter().find(|n| n.first().map(|b| b & 0x1f) == Some(7) && n.len() >= 4).map(|n| [n[1], n[2], n[3]]);
            }
        } else if !self.started[s] {
            self.before_key += 1;
        }
    }

    fn report(&self) -> String {
        let sps = match self.sps {
            // H.265 profile_idc 1 Main, 4 Format range extensions (4:4:4)
            Some([profile, tier, level]) if self.hevc => format!("H.265 SPS profile_idc {profile} tier {tier} level_idc {level}"),
            Some([profile, flags, level]) => format!("SPS profile_idc {profile} (constraints {flags:#04x}) level_idc {level}"),
            None => "no SPS".into(),
        };
        let types: Vec<String> = (0..64).filter(|t| self.key_nals & (1u64 << t) != 0).map(|t| t.to_string()).collect();
        let ok = self.before_key == 0 && self.bare_keys == 0 && self.wrong_flags == 0 && self.sps.is_some();
        format!(
            "probe: bitstream {}: {sps}; key frame NAL types [{}]; {} pictures before a key frame, {} key frames without SPS/PPS/IDR, {} wrong key flags",
            if ok { "OK" } else { "BAD" },
            types.join(","),
            self.before_key,
            self.bare_keys,
            self.wrong_flags
        )
    }
}

/// The NAL units of an Annex-B access unit (start codes of 3 or 4 bytes).
fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, &(_, begin)) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |s| s.0);
        // a 4-byte start code's leading zero belongs to it
        while end > begin && data[end - 1] == 0 && k + 1 < starts.len() {
            end -= 1;
        }
        out.push(&data[begin..end]);
    }
    out
}
