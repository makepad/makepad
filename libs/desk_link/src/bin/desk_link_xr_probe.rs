//! desk-link-xr-probe <host:port | pair> [--pairing FILE] [--secs N] [--hz N] [--eye WxH] [--block FRAMES] [--video MS] [--dump FILE]: a headless
//! headset for Desk Link XR checks. It links like the Quest client (this machine's fleet
//! credential; `pair`: it finds the server by the pairing probe like the Quest, its instance id,
//! PIN and pairs kept in `--pairing FILE`, default `xr-probe-pairing`, its PIN printed), sends a synthetic pose per display frame as datagrams (the head turning
//! slowly), answers every picture tile at once (no decode: the server's flow control keeps
//! going) and plays the game audio through the headset's own receive path and playout
//! (`makepad_desk_link::xr_audio`) into a stand-in device: a thread that takes `--block` frames
//! (default 192, a Quest AAudio burst) at 48 kHz on its own clock. With the server's test
//! signal on (`MAKEPAD_XR_REMOTE_AUDIO_TEST=1`) it reports each second the clicks' latency
//! (due on the desktop to the output buffer) and the tone's discontinuities, and the
//! datagrams, losses, buffer, drift nudge, underruns and concealment; at the end a summary.

use makepad_desk_link::xr::{link::{HeadsetInfo, XrLink}, codecs, pose_datagram, report, XrClientMsg, XrServerMsg};
use makepad_desk_link::xr_audio::{XrAudioPlayer, XrAudioRx};
use makepad_hal::{buffers::Wire, clock_ns, XrEye, XrFrame, XrPose};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(target) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: desk-link-xr-probe <host:port | pair> [--pairing FILE] [--secs N] [--hz N] [--eye WxH] [--block FRAMES] [--video MS] [--dump FILE]");
        std::process::exit(2);
    };
    let arg = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let secs: u64 = arg("--secs").and_then(|v| v.parse().ok()).unwrap_or(30);
    let hz: f64 = arg("--hz").and_then(|v| v.parse().ok()).unwrap_or(90.0);
    let block: usize = arg("--block").and_then(|v| v.parse().ok()).unwrap_or(192).clamp(16, 4096);
    let eye: [u32; 2] = arg("--eye").and_then(|v| v.split_once('x').and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?]))).unwrap_or([960, 1008]);
    let mut pairing = (target == "pair").then(|| makepad_desk_link::xr_pair::HeadsetPairing::load(arg("--pairing").unwrap_or_else(|| "xr-probe-pairing".into()).into()));
    let mut server = 0u64;
    let (host, port) = match pairing.as_mut() {
        Some(p) => {
            let (me, fleet) = makepad_desk_link::xr_pair::identity("the pairing probe").unwrap_or_else(|e| {
                eprintln!("xr-probe: {e}");
                std::process::exit(1);
            });
            println!("xr-probe: instance {:016x} PIN {}: waiting for a server", p.instance, p.pin);
            loop {
                let answers = makepad_desk_link::xr_pair::sweep(me, &fleet, p, "xr-probe", Duration::from_millis(1500)).unwrap_or_default();
                match p.choose(&answers) {
                    Ok(Some(a)) => {
                        println!("xr-probe: {:?} answer from {} ({:016x}) on {}", a.kind, a.app, a.instance, a.tcp);
                        server = a.instance;
                        break (a.tcp.ip().to_string(), a.tcp.port());
                    }
                    Ok(None) => {}
                    Err(()) => {
                        p.reroll();
                        println!("xr-probe: PIN clash: new PIN {}", p.pin);
                    }
                }
            }
        }
        None => match target.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
            _ => {
                eprintln!("xr-probe: the target is host:port or pair");
                std::process::exit(2);
            }
        },
    };
    let info = HeadsetInfo { name: "xr-probe".into(), instance: pairing.as_ref().map_or(0, |p| p.instance), eye, refresh_mhz: (hz * 1000.0) as u32, codecs: codecs::HEVC | codecs::H264 };
    let link = match XrLink::connect(&host, port, info, Arc::new(|| {})) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            eprintln!("xr-probe: {e}");
            std::process::exit(1);
        }
    };
    println!("xr-probe: linked to {host}:{port}, eyes {}x{} at {hz} Hz, device blocks of {block} frames", eye[0], eye[1]);
    let rx = XrAudioRx::new();
    // --video MS: the pictures' latency the sound lines up with (a headset measures its own)
    if let Some(ms) = arg("--video").and_then(|v| v.parse::<f32>().ok()) {
        rx.set_video_latency((ms * 1000.0) as u32);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let udp: Arc<Mutex<Option<(Arc<UdpSocket>, Vec<u8>)>>> = Arc::default();

    // the poses: one per display frame, as datagrams once the server said where
    {
        let (udp, stop) = (udp.clone(), stop.clone());
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
                let yaw = (t * 0.5).sin() * 0.6;
                let q = [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()];
                let right = [yaw.cos(), 0.0, -yaw.sin()];
                let at = |s: f32| XrPose { orientation: q, position: [right[0] * s * 0.032, 1.6, right[2] * s * 0.032] };
                let fov = [-0.785, 0.785, 0.785, -0.785];
                let frame = XrFrame { head: at(0.0), eyes: [XrEye { pose: at(-1.0), fov }, XrEye { pose: at(1.0), fov }], ..Default::default() };
                wire.clear();
                frame.write(&mut wire);
                let sample_ns = clock_ns();
                let _ = socket.send(&pose_datagram::seal(&key, seq, sample_ns, sample_ns + 30_000_000, &wire));
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
    let mut clicks: Vec<f32> = Vec::new();
    let (mut all_clicks, mut totals) = (Vec::<f32>::new(), [0f32; 4]);
    let mut audio_ms: Vec<f32> = Vec::new();
    let why = loop {
        if started.elapsed() >= Duration::from_secs(secs) {
            break "done".to_string();
        }
        if let Some(why) = link.closed() {
            break why;
        }
        if pinged.elapsed() >= Duration::from_secs(1) {
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
            println!(
                "xr-probe: pictures {:.0}/s · audio {:.0}/s lost {} late {} · latency {:.1} ms (net {:.1} buffer {:.1}/target {:.1} device {:.1}) · underruns {} concealed {} · drift {:+.0} ppm · click {:.1} ms · glitches {}",
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
            XrServerMsg::Welcome { server, size, tiles: n, encoder, .. } => {
                tiles = n;
                println!("xr-probe: welcome from {server}: {}x{} in {n} tile(s) on {encoder}", size[0], size[1]);
            }
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
            XrServerMsg::Video { seq, .. } => {
                pictures += 1;
                link.send(&XrClientMsg::Stats { seq, decode_us: 1000, m2p_us: 0 });
            }
            XrServerMsg::Pong { client_ns, server_ns } => rx.pong(client_ns, server_ns),
            XrServerMsg::Note { text } => println!("xr-probe: server: {text}"),
            XrServerMsg::RoomField { wanted } => println!("xr-probe: the server {} the room field (this probe has none)", if wanted { "wants" } else { "does not want" }),
            XrServerMsg::Refresh { mhz } => println!("xr-probe: the server asks for {:.0} Hz (this probe keeps its own)", mhz as f64 / 1000.0),
            XrServerMsg::Streaming { on } => println!("xr-probe: stream {}", if on { "on" } else { "off" }),
            XrServerMsg::SeeThrough { on } => println!("xr-probe: {}", if on { "mixed reality" } else { "VR" }),
            XrServerMsg::Headset { foveation, eye_min_pct, eye_max_pct, multimodal, occlusion, ahead } => println!("xr-probe: headset settings: foveation {foveation}, eyes {eye_min_pct}..{eye_max_pct}%, multimodal {multimodal}, occlusion {occlusion}, ahead {ahead}"),
            XrServerMsg::Haptic { hand, amplitude, duration_us } => println!("xr-probe: haptic hand {hand} {amplitude:.2} for {duration_us} us"),
            XrServerMsg::Refused { reason } | XrServerMsg::Bye { reason } => break reason,
            XrServerMsg::Unpaired => {
                if let Some(p) = pairing.as_mut() {
                    p.forget(server);
                }
                break "the server unpaired this probe".to_string();
            }
        }
    };
    stop.store(true, Ordering::Release);
    rx.stop();
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
}
