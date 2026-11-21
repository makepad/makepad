//! desk-link-xr-probe <host:port | pair> [--list] [--pairing FILE] [--secs N] [--hz N] [--eye WxH] [--mono] [--standby] [--name NAME] [--block FRAMES] [--video MS] [--dump FILE]: a headless
//! headset for Desk Link XR checks. It prints its viewer PIN and keeps its pairing in
//! `--pairing FILE` (default `xr-probe-pairing`) and its key beside that file. Use `pair` to
//! discover the server by PIN, or supply its address to connect directly; `pair --list` only
//! prints each host that answers (every app of a machine) for `--secs` and links to none. After pairing the
//! probe sends a synthetic pose per display frame as datagrams (the head turning
//! slowly), answers every picture tile at once (no decode: the server's flow control keeps
//! going) and plays the game audio through the headset's own receive path and playout
//! (`makepad_desk_link::xr_audio`) into a stand-in device: a thread that takes `--block` frames
//! (default 192, a Quest AAudio burst) at 48 kHz on its own clock. With the server's test
//! signal on (`MAKEPAD_XR_REMOTE_AUDIO_TEST=1`) it reports each second the clicks' latency
//! (due on the desktop to the output buffer) and the tone's discontinuities, and the
//! datagrams, losses, buffer, drift nudge, underruns and concealment; at the end a summary.
//! With `--mono` it links as a handheld ([`codecs::HANDHELD`]) with one view ([`codecs::MONO`]), by default 1440x1080
//! at 60 Hz, the camera walking a slow circle around a point 0.6 m away and looking at it, and
//! checks that the server honours both capabilities, sends one unfoveated column in one tile
//! with the view of the pose it was rendered for, and never asks for a refresh rate change.
//! With `--standby` it says [`codecs::STANDBY`]: while another device has the view it stays
//! linked on standby (pings only), and it streams again on the server's next `Welcome`; each
//! change is printed, and the end line counts the streams and standbys.

use makepad_desk_link::xr::{link::{HeadsetInfo, XrLink}, codecs, pose_datagram, report, server_caps, Fovea, XrClientMsg, XrServerMsg};
use makepad_desk_link::xr_audio::{XrAudioPlayer, XrAudioRx};
use makepad_desk_link::xr_pair::{self, ViewerPairing};
use makepad_hal::{buffers::Wire, clock_ns, XrEye, XrFrame, XrPose};
use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(target) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: desk-link-xr-probe <host:port | pair> [--pairing FILE] [--secs N] [--hz N] [--eye WxH] [--mono] [--standby] [--name NAME] [--block FRAMES] [--video MS] [--dump FILE]");
        std::process::exit(2);
    };
    let arg = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let mono = args.iter().any(|a| a == "--mono");
    let standby_ok = args.iter().any(|a| a == "--standby");
    let list = args.iter().any(|a| a == "--list");
    // the name the server's panel lists it by (two probes on one server: two names)
    let name = arg("--name").unwrap_or_else(|| "xr-probe".to_string());
    let secs: u64 = arg("--secs").and_then(|v| v.parse().ok()).unwrap_or(30);
    let hz: f64 = arg("--hz").and_then(|v| v.parse().ok()).unwrap_or(if mono { 60.0 } else { 90.0 });
    let block: usize = arg("--block").and_then(|v| v.parse().ok()).unwrap_or(192).clamp(16, 4096);
    let eye: [u32; 2] = arg("--eye").and_then(|v| v.split_once('x').and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?]))).unwrap_or(if mono { [1440, 1080] } else { [960, 1008] });
    let fail = |why: &str| -> ! {
        eprintln!("xr-probe: {why}");
        if mono {
            println!("xr-probe: MONO-FAIL {why}");
        }
        std::process::exit(1)
    };
    let address = if target == "pair" {
        None
    } else if let Some((host, port)) = target.rsplit_once(':').and_then(|(h, p)| Some((h.to_string(), p.parse::<u16>().ok()?))) {
        Some((host, port))
    } else {
        eprintln!("xr-probe: the target is host:port or pair");
        std::process::exit(2);
    };
    let path = arg("--pairing").unwrap_or_else(|| "xr-probe-pairing".into());
    let mut pairing = ViewerPairing::load(path.into()).unwrap_or_else(|e| fail(&format!("load viewer pairing: {e}")));
    println!("xr-probe: instance {:016x} PIN {}: enter it in the computer's XR menu to pair", pairing.device.instance, pairing.device.pin);
    let (host, port) = if let Some(address) = address {
        address
    } else {
        let mut round = 0u32;
        let (listing, mut listed) = (Instant::now(), std::collections::HashSet::new());
        loop {
            let started = Instant::now();
            let targets = xr_pair::local_viewer_targets(None, round);
            let (answers, _) = xr_pair::viewer_sweep(&pairing, &name, &xr_pair::HostRef::default(), &targets, Duration::from_millis(250), Duration::from_millis(700))
                .unwrap_or_else(|e| fail(&e));
            if list {
                for a in answers.iter().filter(|a| listed.insert((a.instance, a.kind as u8))) {
                    println!("xr-probe: {:?} answer from {} on {} test {:?} ({:016x}) on {}", a.kind, a.app, a.machine, a.test, a.instance, a.tcp);
                }
                if listing.elapsed() > Duration::from_secs(secs) {
                    println!("xr-probe: {} host(s) answered", listed.iter().map(|(i, _)| i).collect::<std::collections::HashSet<_>>().len());
                    std::process::exit(0);
                }
            } else if let Some(a) = pairing.choose(&answers) {
                println!("xr-probe: {:?} answer from {} on {} test {:?} ({:016x}) on {}", a.kind, a.app, a.machine, a.test, a.instance, a.tcp);
                break (a.tcp.ip().to_string(), a.tcp.port());
            }
            round = round.wrapping_add(1);
            std::thread::sleep(xr_pair::SWEEP_EVERY.saturating_sub(started.elapsed()));
        }
    };
    let info = HeadsetInfo { name: name.clone(), instance: pairing.device.instance, eye, refresh_mhz: (hz * 1000.0) as u32, codecs: codecs::HEVC | codecs::H264 | if mono { codecs::MONO | codecs::HANDHELD } else { 0 } | if standby_ok { codecs::STANDBY } else { 0 } };
    let connect = |viewer: &ViewerPairing| XrLink::connect_viewer(&host, port, info.clone(), viewer, Arc::new(|| {})).unwrap_or_else(|e| fail(&e));
    let (mut link, mut hello) = connect(&pairing);
    let rx = XrAudioRx::new();
    // --video MS: the pictures' latency the sound lines up with (a headset measures its own)
    if let Some(ms) = arg("--video").and_then(|v| v.parse::<f32>().ok()) {
        rx.set_video_latency((ms * 1000.0) as u32);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let udp: Arc<Mutex<Option<(Arc<UdpSocket>, Vec<u8>)>>> = Arc::default();
    const POSE_HISTORY: usize = 512;
    let mono_poses = mono.then(|| Arc::new(Mutex::new(VecDeque::with_capacity(POSE_HISTORY))));

    // the poses: one per display frame, as datagrams once the server said where
    {
        let (udp, stop) = (udp.clone(), stop.clone());
        let mono_poses = mono_poses.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let period = Duration::from_secs_f64(1.0 / hz);
            let mut next = Instant::now();
            let mut seq = 0u64;
            let mut wire = Vec::with_capacity(<XrFrame as Wire>::SIZE);
            while !stop.load(Ordering::Acquire) {
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                let Some((socket, key)) = udp.lock().unwrap().clone() else { continue };
                seq += 1;
                let t = start.elapsed().as_secs_f32();
                let frame = if mono {
                    let c = [0.0, 1.0, -0.6];
                    let a = t * 0.3;
                    // Yaw faces the centre horizontally. The camera stays level (no pitch).
                    let pose = XrPose {
                        orientation: [0.0, (a * 0.5).sin(), 0.0, (a * 0.5).cos()],
                        position: [c[0] + 0.6 * a.sin(), 1.3, c[2] + 0.6 * a.cos()],
                    };
                    let h = (0.5f32 * 4.0 / 3.0).atan();
                    let v = 0.5f32.atan();
                    XrFrame { head: pose, eyes: [XrEye { pose, fov: [-h, h, v, -v] }; 2], ..Default::default() }
                } else {
                    let yaw = (t * 0.5).sin() * 0.6;
                    let q = [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()];
                    let right = [yaw.cos(), 0.0, -yaw.sin()];
                    let at = |s: f32| XrPose { orientation: q, position: [right[0] * s * 0.032, 1.6, right[2] * s * 0.032] };
                    let fov = [-0.785, 0.785, 0.785, -0.785];
                    XrFrame { head: at(0.0), eyes: [XrEye { pose: at(-1.0), fov }, XrEye { pose: at(1.0), fov }], ..Default::default() }
                };
                wire.clear();
                frame.write(&mut wire);
                let sample_ns = clock_ns();
                let packet = pose_datagram::seal(&key, seq, sample_ns, sample_ns + 30_000_000, &wire);
                if let Some(poses) = &mono_poses {
                    // Publish before sending so even an immediate Video can find its pose.
                    let mut poses = poses.lock().unwrap();
                    if poses.len() == POSE_HISTORY {
                        poses.pop_front();
                    }
                    poses.push_back((seq, frame.eyes[0].pose.orientation));
                }
                if socket.send(&packet).is_err() {
                    if let Some(poses) = &mono_poses {
                        poses.lock().unwrap().pop_back();
                    }
                }
            }
        });
    }

    // the stand-in audio device: `block` frames at 48 kHz on this machine's clock
    // (`--dump FILE`: what it played, the left channel as f32 LE, for a look at the waveform)
    let dump = arg("--dump");
    {
        let (rx, stop) = (rx.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut out = dump.and_then(|p| std::fs::File::create(p).ok());
            let mut player = XrAudioPlayer::new(rx);
            let mut data = vec![0.0f32; 2 * block];
            let period = Duration::from_secs_f64(block as f64 / 48_000.0);
            let mut next = Instant::now();
            while !stop.load(Ordering::Acquire) {
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                player.render_planar(48_000.0, block, 2, &mut data);
                if let Some(f) = out.as_mut() {
                    use std::io::Write;
                    let bytes: Vec<u8> = data[..block].iter().flat_map(|v| v.to_le_bytes()).collect();
                    let _ = f.write_all(&bytes);
                }
            }
        });
    }

    let started = Instant::now();
    let (mut pinged, mut window) = (Instant::now() - Duration::from_secs(2), Instant::now());
    let (mut pictures, mut tiles) = (0u64, 1u8);
    // streaming now; paired (the first Welcome or Standby confirmed the pairing); on standby
    let mut welcomed = false;
    let (mut paired_once, mut on_standby) = (false, false);
    let (mut streams, mut standbys) = (0u32, 0u32);
    let mut mono_welcome = None;
    let mut refresh_asks = 0u64;
    let (mut mono_ok, mut mono_wrong, mut mono_unknown) = (0u64, 0u64, 0u64);
    let mut clicks: Vec<f32> = Vec::new();
    let (mut all_clicks, mut totals) = (Vec::<f32>::new(), [0f32; 4]);
    let mut audio_ms: Vec<f32> = Vec::new();
    let why = loop {
        if started.elapsed() >= Duration::from_secs(secs) {
            break if welcomed { "done" } else { "timed out waiting for a viewer Welcome" }.to_string();
        }
        if let Some(why) = link.closed() {
            break why;
        }
        if (welcomed || on_standby) && pinged.elapsed() >= Duration::from_secs(1) {
            pinged = Instant::now();
            link.send(&XrClientMsg::Ping { client_ns: clock_ns() });
        }
        if window.elapsed() >= Duration::from_secs(1) {
            let secs_w = window.elapsed().as_secs_f32();
            window = Instant::now();
            let mut v = vec![0.0f32; report::COUNT];
            rx.report(&mut v);
            if v[report::AUDIO_CLICK_MS] > 0.0 {
                clicks.push(v[report::AUDIO_CLICK_MS]);
            }
            for (k, i) in [report::AUDIO_LOST, report::AUDIO_LATE, report::AUDIO_UNDERRUNS, report::AUDIO_CONCEALED].into_iter().enumerate() {
                totals[k] += v[i];
            }
            if v[report::AUDIO_MS] > 0.0 {
                audio_ms.push(v[report::AUDIO_MS]);
            }
            let mono_stats = if mono { format!(" · mono eyes ok {mono_ok} wrong {mono_wrong} unknown {mono_unknown}") } else { String::new() };
            println!(
                "xr-probe: pictures {:.0}/s · audio {:.0}/s lost {} late {} · latency {:.1} ms (net {:.1} buffer {:.1}/target {:.1} device {:.1}) · underruns {} concealed {} · drift {:+.0} ppm · click {:.1} ms · glitches {}{mono_stats}",
                pictures as f32 / tiles.max(1) as f32 / secs_w,
                v[report::AUDIO_RECEIVED] / secs_w,
                v[report::AUDIO_LOST],
                v[report::AUDIO_LATE],
                v[report::AUDIO_MS],
                v[report::AUDIO_NET_MS],
                v[report::AUDIO_BUFFER_MS],
                v[report::AUDIO_TARGET_MS],
                v[report::AUDIO_DEVICE_MS],
                v[report::AUDIO_UNDERRUNS],
                v[report::AUDIO_CONCEALED],
                v[report::AUDIO_DRIFT_PPM],
                v[report::AUDIO_CLICK_MS],
                rx.glitches()
            );
            all_clicks.append(&mut clicks);
            pictures = 0;
        }
        let Some(msg) = link.recv_timeout(Duration::from_millis(5)) else { continue };
        match msg {
            XrServerMsg::Refused { reason } => {
                if !welcomed && reason == xr_pair::VIEWER_PIN_REQUIRED {
                    pairing.forget_server(&hello).unwrap_or_else(|e| fail(&format!("forget viewer server: {e}")));
                    // A direct connection can reach the desktop before the user enters the PIN.
                    drop(link);
                    std::thread::sleep(xr_pair::SWEEP_EVERY);
                    (link, hello) = connect(&pairing);
                    continue;
                }
                break reason;
            }
            XrServerMsg::Bye { reason } => break reason,
            XrServerMsg::Welcome { server, instance, size, tiles: n, encoder, eye, fovea, caps, pin_ok, .. } => {
                if welcomed {
                    fail("duplicate viewer Welcome");
                }
                if !paired_once {
                    pairing.accept_welcome(&hello, instance, &pin_ok).unwrap_or_else(|e| fail(&e));
                }
                (welcomed, paired_once, on_standby) = (true, true, false);
                streams += 1;
                println!("xr-probe: linked to {host}:{port}, eyes {}x{} at {hz} Hz, device blocks of {block} frames", eye[0], eye[1]);
                tiles = n;
                println!("xr-probe: welcome from {server}: {}x{} in {n} tile(s) on {encoder} caps {caps:#x}", size[0], size[1]);
                if mono {
                    let enc = fovea.encoded(eye);
                    let height = u64::from(enc[1]) * 3 / 2;
                    let mut faults = Vec::new();
                    if n != 1 {
                        faults.push(format!("{n} tiles, expected 1"));
                    }
                    if size[0] != enc[0] || u64::from(size[1]) != height {
                        faults.push(format!("size {}x{}, expected {}x{height}", size[0], size[1], enc[0]));
                    }
                    if fovea != Fovea::OFF {
                        faults.push(format!("fovea {fovea:?}, expected OFF"));
                    }
                    let one_view = caps & server_caps::MONO != 0;
                    let handheld = caps & server_caps::HANDHELD != 0;
                    if !one_view || !handheld {
                        faults.push(format!("server too old (no one-view caps): MONO {one_view}, HANDHELD {handheld}"));
                    }
                    let correct = faults.is_empty();
                    if correct {
                        println!("xr-probe: mono welcome OK");
                    } else {
                        println!("xr-probe: mono welcome WRONG: {}", faults.join(", "));
                    }
                    mono_welcome = Some(correct);
                }
            }
            XrServerMsg::Standby { server, instance, showing, pin_ok, .. } => {
                if !standby_ok {
                    fail("a Standby to a probe that did not say STANDBY");
                }
                if !paired_once {
                    pairing.accept_welcome(&hello, instance, &pin_ok).unwrap_or_else(|e| fail(&e));
                }
                // the stream (if any) stopped: no poses until the next Welcome
                *udp.lock().unwrap() = None;
                (welcomed, paired_once, on_standby) = (false, true, true);
                standbys += 1;
                println!("xr-probe: on standby with {server}, the view on {showing:?}");
            }
            // (a standby link gets nothing of a stream; one still on the wire is dropped)
            _ if !welcomed && on_standby => {}
            _ if !welcomed => fail("expected a viewer Welcome before stream messages"),
            XrServerMsg::Uplink { port, key } => match UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect((host.as_str(), port)).map(|_| s)) {
                Ok(socket) => {
                    if let Err(e) = rx.start(&socket, key.clone()) {
                        println!("xr-probe: no audio: {e}");
                    }
                    *udp.lock().unwrap() = Some((Arc::new(socket), key));
                    println!("xr-probe: poses up and audio down on port {port}");
                }
                Err(e) => println!("xr-probe: no datagram socket: {e}"),
            },
            XrServerMsg::Video { seq, eyes, .. } => {
                pictures += 1;
                link.send(&XrClientMsg::Stats { seq, decode_us: 1000, m2p_us: 0 });
                if let Some(poses) = &mono_poses {
                    let expected = poses.lock().unwrap().iter().rev().find(|(s, _)| *s == seq).map(|(_, q)| *q);
                    match expected {
                        None => mono_unknown += 1,
                        Some(q) if eyes[0] == eyes[1] && eyes[0].orientation.iter().zip(q).all(|(a, b)| (a - b).abs() <= 1e-4) => mono_ok += 1,
                        Some(_) => mono_wrong += 1,
                    }
                }
            }
            XrServerMsg::Pong { client_ns, server_ns } => rx.pong(client_ns, server_ns),
            XrServerMsg::Note { text } => println!("xr-probe: server: {text}"),
            XrServerMsg::RoomField { wanted } => println!("xr-probe: the server {} the room field (this probe has none)", if wanted { "wants" } else { "does not want" }),
            XrServerMsg::Refresh { mhz } => {
                if mono {
                    refresh_asks += 1;
                }
                println!("xr-probe: the server asks for {:.0} Hz (this probe keeps its own)", mhz as f64 / 1000.0);
            }
            XrServerMsg::Streaming { on } => println!("xr-probe: stream {}", if on { "on" } else { "off" }),
            XrServerMsg::SeeThrough { on } => println!("xr-probe: {}", if on { "mixed reality" } else { "VR" }),
            XrServerMsg::Headset { foveation, eye_min_pct, eye_max_pct, multimodal, occlusion, ahead } => println!("xr-probe: headset settings: foveation {foveation}, eyes {eye_min_pct}..{eye_max_pct}%, multimodal {multimodal}, occlusion {occlusion}, ahead {ahead}"),
            XrServerMsg::Haptic { hand, amplitude, duration_us } => println!("xr-probe: haptic hand {hand} {amplitude:.2} for {duration_us} us"),
            XrServerMsg::Capture { on, .. } => println!("xr-probe: capture asked {} (this probe has no cameras)", if on { "on" } else { "off" }),
            XrServerMsg::Mic { on } => println!("xr-probe: microphone asked {} (this probe has no microphone)", if on { "on" } else { "off" }),
            XrServerMsg::Unpaired => {
                pairing.forget_server(&hello).unwrap_or_else(|e| fail(&format!("forget viewer server: {e}")));
                break "the server unpaired this probe".to_string();
            }
        }
    };
    stop.store(true, Ordering::Release);
    rx.stop();
    if !paired_once {
        fail(&why);
    }
    if standby_ok {
        println!("xr-probe: streams {streams}, standbys {standbys}");
    }
    all_clicks.sort_by(|a, b| a.total_cmp(b));
    audio_ms.sort_by(|a, b| a.total_cmp(b));
    let pct = |v: &[f32], p: f32| v.get(((v.len() as f32 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
    println!(
        "xr-probe: end ({why}) after {:.0} s · clicks {} latency min {:.1} median {:.1} max {:.1} ms (to the output buffer) · estimated latency median {:.1} ms · lost {} late {} underruns {} concealed {} · tone discontinuities {}",
        started.elapsed().as_secs_f32(),
        all_clicks.len(),
        pct(&all_clicks, 0.0),
        pct(&all_clicks, 0.5),
        pct(&all_clicks, 1.0),
        pct(&audio_ms, 0.5),
        totals[0],
        totals[1],
        totals[2],
        totals[3],
        rx.glitches()
    );
    if mono {
        let pictures = mono_ok + mono_wrong + mono_unknown;
        if mono_welcome == Some(true) && pictures > 0 && mono_wrong == 0 && refresh_asks == 0 {
            println!("xr-probe: MONO-OK pictures {pictures}");
        } else {
            let welcome = match mono_welcome {
                Some(true) => "OK",
                Some(false) => "wrong",
                None => "missing",
            };
            println!("xr-probe: MONO-FAIL welcome {welcome}, pictures {pictures}, wrong eyes {mono_wrong}, refresh asks {refresh_asks}");
            std::process::exit(1);
        }
    }
}
