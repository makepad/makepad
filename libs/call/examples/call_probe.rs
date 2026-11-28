//! A two-party call between two processes over UDP, on a test link that never ships in Town: a
//! synthetic picture (H.264) and a synthetic voice (a tone in talk spurts, ADPCM at 16 kHz)
//! go each way, sealed, with feedback, resends and key frame requests. What arrives is decoded,
//! played into a simulated speaker and measured; a few decoded pictures are saved as PNG.
//! Both sides' secrets come from the secret given on the command line.
//!
//! ```text
//! call_probe --name a --listen 0.0.0.0:47100 --peer 192.0.2.10:47100 --secret s
//!     [--seconds 20] [--size 1280x720] [--kbps 1500] [--fps 30] [--tone 440] [--grabs DIR]
//!     [--source noise] [--path-kbps N]
//! ```
//! Captures run at 30 fps. Size and fps cap the encoder ladder. A positive path
//! rate enables an emulated outgoing bottleneck with 60 ms of queue.

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn main() {}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn main() {
    probe::main()
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod probe {
    use makepad_call::guard::{video_ask, Guard, Rung, Tick, LADDER};
    use makepad_call::rate::LossEvents;
    use makepad_call::session::{Event, Peer, START_BPS};
    use makepad_call::wire::{stream, voice_flags, Voice};
    use makepad_network::digest::sha256_hash;
    use makepad_teamtalk::codec::{adpcm_decode, adpcm_encode_into, AdpcmState};
    use makepad_teamtalk::jitter::{JitterRing, Playout};
    use makepad_teamtalk::resample::Resampler;
    use makepad_teamtalk::{Framer, FramerConfig, PlayoutConfig, MAX_FRAME};
    use makepad_video::{StreamVideoCodec, VideoStreamDecoder, VideoStreamEncoder, VideoStreamEncoderOptions};
    use makepad_video::annex_b::{nal_unit_type, split_annex_b, NAL_TYPE_SPS};
    use std::collections::VecDeque;
    use std::net::{SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    const VOICE_RATE: f64 = 16_000.0;
    const VOICE_FRAME: usize = 320;
    const DEVICE_RATE: f64 = 48_000.0;
    const BLOCK: usize = 480;

    struct Args {
        name: String,
        listen: SocketAddr,
        peer: SocketAddr,
        secret: String,
        seconds: u64,
        top: usize,
        kbps: u32,
        path_kbps: u32,
        tone: f32,
        grabs: Option<String>,
        /// `--source noise`: a picture no encoder predicts well (fills any encoder's ask).
        noise: bool,
    }

    fn args(a: Vec<String>) -> Args {
        let get = |k: &str| a.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
        let need = |k: &str| get(k).unwrap_or_else(|| panic!("{k} is needed"));
        let size = get("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((1280, 720));
        let fps = get("--fps").and_then(|s| s.parse().ok()).unwrap_or(30);
        let top = LADDER.iter().position(|r| r.width <= size.0 && r.height <= size.1 && r.fps <= fps)
            .expect("--size and --fps must admit at least 320x180@4");
        let kbps = get("--kbps").and_then(|s| s.parse().ok()).unwrap_or(1500);
        assert!(kbps > 0, "--kbps must be positive");
        Args {
            name: need("--name"),
            listen: need("--listen").parse().expect("--listen address"),
            peer: need("--peer").parse().expect("--peer address"),
            secret: need("--secret"),
            seconds: get("--seconds").and_then(|s| s.parse().ok()).unwrap_or(20),
            top,
            kbps,
            path_kbps: get("--path-kbps").map_or(0, |s| s.parse().expect("--path-kbps needs a nonnegative integer")),
            tone: get("--tone").and_then(|s| s.parse().ok()).unwrap_or(440.0),
            grabs: get("--grabs"),
            noise: get("--source").is_some_and(|s| s == "noise"),
        }
    }

    /// What the media threads hand the network thread.
    enum Media {
        Voice { seq: u32, silent: bool, talk_start: bool, capture_us: u64, payload: Vec<u8> },
        Video { au: Vec<u8>, key: bool, capture_us: u64 },
    }

    #[derive(Default, Clone)]
    struct Status {
        rtt_us: Option<u64>,
        estimate_bps: u64,
        random_loss: f64,
        loss_events: LossEvents,
        asked_kbps: u32,
        stats: makepad_call::session::PeerStats,
        video: Option<makepad_call::video::ReceiverStats>,
        voice_frames: u64,
        voice_silent: u64,
        pictures_sent: u64,
        keys_sent: u64,
        video_bytes: u64,
        rung: Option<Rung>,
        skipped: u64,
        steps: u64,
        path_drops: u64,
        seconds_in_band: u64,
        seconds_measured: u64,
    }

    struct Link {
        estimate_bps: AtomicU64,
        pacer_delay_us: AtomicU64,
        loss_level_bits: AtomicU64,
        // Both threads use the same clock. Exclude the initial wait for the remote.
        start_us: OnceLock<u64>,
    }

    fn device_key(name: &str) -> Vec<u8> {
        let h = sha256_hash(format!("call probe device\0{name}").as_bytes());
        let mut k = vec![4u8];
        k.extend_from_slice(&h);
        k.extend_from_slice(&sha256_hash(&h));
        k
    }

    fn secret(secret: &str, sender: &str) -> [u8; 32] {
        sha256_hash(format!("call probe secret\0{secret}\0{sender}").as_bytes())
    }

    pub fn main() {
        let raw: Vec<String> = std::env::args().collect();
        // --pair: both ends on this machine over loopback, the other end a child of this one
        // (a run that takes no shell can start the call with one command)
        let mut other_end = None;
        let raw = if raw.iter().any(|x| x == "--pair") {
            let rest: Vec<String> = raw[1..].iter().filter(|x| *x != "--pair").cloned().collect();
            let end = |name: &str, listen: &str, peer: &str, tone: &str| -> Vec<String> {
                ["--name", name, "--listen", listen, "--peer", peer, "--tone", tone].iter().map(|s| s.to_string()).chain(rest.iter().cloned()).collect()
            };
            let exe = std::env::current_exe().expect("this program's path");
            other_end = Some(std::process::Command::new(exe).args(end("b", "127.0.0.1:47102", "127.0.0.1:47101", "660")).spawn().expect("the other end"));
            std::iter::once(raw[0].clone()).chain(end("a", "127.0.0.1:47101", "127.0.0.1:47102", "440")).collect()
        } else {
            raw
        };
        let args = Arc::new(args(raw));
        let peer_name = if args.name == "a" { "b" } else { "a" };
        let clock = Instant::now();
        let now_us = move || clock.elapsed().as_micros() as u64;
        let call_id = sha256_hash(format!("call probe\0{}", args.secret).as_bytes());
        let mut peer = Peer::new(&call_id[..16], &device_key(&args.name), &device_key(peer_name));
        peer.install_send_secret(0, &secret(&args.secret, &args.name)).unwrap();
        peer.install_recv_secret(0, &secret(&args.secret, peer_name)).unwrap();

        let socket = UdpSocket::bind(args.listen).expect("bind");
        eprintln!("call_probe {}: {} -> {}, top {} at up to {} kbit/s, {} s", args.name, args.listen, args.peer, LADDER[args.top], args.kbps, args.seconds);

        let stop = Arc::new(AtomicBool::new(false));
        let key_wanted = Arc::new(AtomicBool::new(true));
        // the other side's clock minus ours (i64::MIN until known), for capture-to-picture latency
        let offset = Arc::new(AtomicI64::new(i64::MIN));
        let link = Arc::new(Link {
            estimate_bps: AtomicU64::new(START_BPS), pacer_delay_us: AtomicU64::new(0),
            loss_level_bits: AtomicU64::new(f64::NAN.to_bits()), start_us: OnceLock::new(),
        });
        let status = Arc::new(Mutex::new(Status::default()));
        let ring = Arc::new(JitterRing::new());
        let (media_tx, media_rx) = channel::<Media>();
        let (frame_tx, frame_rx) = channel::<makepad_call::video::Frame>();

        let net = {
            let (stop, key_wanted, status, ring, offset) = (stop.clone(), key_wanted.clone(), status.clone(), ring.clone(), offset.clone());
            let (args, link) = (args.clone(), link.clone());
            std::thread::spawn(move || network(peer, socket, &args, media_rx, frame_tx, ring, key_wanted, offset, link, status, stop, now_us))
        };
        // the other side may still be building: its feedback heartbeat says it's there
        let waited = Instant::now();
        while status.lock().unwrap().stats.received == 0 {
            if waited.elapsed() > Duration::from_secs(900) {
                eprintln!("call_probe {}: no word from {} in 15 minutes", args.name, args.peer);
                stop.store(true, Ordering::Relaxed);
                net.join().ok();
                std::process::exit(2);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!("call_probe {}: {} is there after {:.1} s", args.name, peer_name, waited.elapsed().as_secs_f64());
        let start_us = now_us();
        link.start_us.set(start_us).unwrap();
        let enc = {
            let (stop, key_wanted, tx, status, args) = (stop.clone(), key_wanted.clone(), media_tx.clone(), status.clone(), args.clone());
            std::thread::spawn(move || encode(&args, tx, key_wanted, link, status, stop, now_us))
        };
        let dec = {
            let (stop, offset) = (stop.clone(), offset.clone());
            let (grabs, label) = (args.grabs.clone(), format!("{}-from-{}", args.name, peer_name));
            std::thread::spawn(move || decode(frame_rx, grabs, label, offset, stop, now_us))
        };
        let mic = {
            let (stop, tx, tone) = (stop.clone(), media_tx.clone(), args.tone);
            std::thread::spawn(move || microphone(tone, tx, stop, now_us))
        };
        drop(media_tx);
        let speaker = {
            let (stop, ring) = (stop.clone(), ring.clone());
            std::thread::spawn(move || speaker(ring, stop))
        };

        for s in 1..=args.seconds {
            std::thread::sleep(Duration::from_micros((start_us + s * 1_000_000).saturating_sub(now_us())));
            let st = status.lock().unwrap().clone();
            let v = st.video.unwrap_or_default();
            eprintln!(
                "[{} {s:>3} s] rtt {} | path {:.2} Mbit/s, video asked {:.2}, rung {}, skipped {}, steps {}, in band {} of {} s | sent {} pictures ({} key), {:.2} Mbit/s total | got {} pictures ({} key), voice {} frames | lost {} of {} hops, emulated path drops {}, resent {}, refused {}, bad {} | random loss {:.1}%, alarms {}, random {}, clean {}, dirty {}, suspects set {}, cleared {}",
                args.name,
                st.rtt_us.map_or("-".to_string(), |r| format!("{:.1} ms", r as f64 / 1000.0)),
                st.estimate_bps as f64 / 1e6,
                st.asked_kbps as f64 / 1e3,
                st.rung.map_or_else(|| "-".into(), |r| r.to_string()), st.skipped, st.steps, st.seconds_in_band, st.seconds_measured,
                st.pictures_sent,
                st.keys_sent,
                st.stats.sent_bytes as f64 * 8.0 / s as f64 / 1e6,
                v.frames,
                v.key_frames,
                st.voice_frames,
                st.stats.hops_lost,
                st.stats.hops_lost + st.stats.hops_delivered,
                st.path_drops,
                st.stats.resent,
                st.stats.refused,
                st.stats.bad_tag,
                st.random_loss * 100.0, st.loss_events.alarms, st.loss_events.random, st.loss_events.clean,
                st.loss_events.dirty, st.loss_events.suspects_set, st.loss_events.suspects_cleared,
            );
        }
        stop.store(true, Ordering::Relaxed);
        net.join().unwrap();
        enc.join().unwrap();
        mic.join().unwrap();
        let (decoded, decode_ms, decoder, latency) = dec.join().unwrap();
        let heard = speaker.join().unwrap();
        let st = status.lock().unwrap().clone();
        let v = st.video.unwrap_or_default();
        let secs = args.seconds as f64;
        println!(
            "call_probe {} with {}: {:.0} s; round trip {}; sent {} pictures ({} key) at {:.2} Mbit/s of video; got {} whole pictures ({} key, {} key asks, {} skipped, {} late fragments), decoded {} ({:.1}/s, median {:.2} ms, {decoder}); capture to decoded picture {latency}; voice {} frames ({} silent); heard {:.0} Hz in talk spurts ({} blocks loud, playout concealed {}, underruns {}); hops lost {} of {}, resent {}, refused {}, bad tag {}; rung {}, guard steps {}, captures skipped {}, emulated path drops {}, sent within 0.6–1.2x estimate after 5 s: {} of {}",
            args.name,
            peer_name,
            secs,
            st.rtt_us.map_or("-".to_string(), |r| format!("{:.2} ms", r as f64 / 1000.0)),
            st.pictures_sent,
            st.keys_sent,
            st.video_bytes as f64 * 8.0 / secs / 1e6,
            v.frames,
            v.key_frames,
            v.key_requests,
            v.skipped,
            v.late,
            decoded,
            decoded as f64 / secs,
            decode_ms,
            st.voice_frames,
            st.voice_silent,
            heard.hz,
            heard.loud_blocks,
            heard.concealed,
            heard.underruns,
            st.stats.hops_lost,
            st.stats.hops_lost + st.stats.hops_delivered,
            st.stats.resent,
            st.stats.refused,
            st.stats.bad_tag,
            st.rung.map_or_else(|| "-".into(), |r| r.to_string()), st.steps, st.skipped, st.path_drops, st.seconds_in_band, st.seconds_measured,
        );
        if let Some(mut other) = other_end {
            other.wait().ok();
        }
    }

    /// FIFO link serialization in bit-microseconds. Admission includes the new packet
    /// in the 60 ms capacity, and partially transmitted packets occupy only their remainder.
    struct Egress {
        bps: u64,
        finish: u128,
        queue: VecDeque<(u64, Vec<u8>)>,
        drops: u64,
    }

    impl Egress {
        fn new(kbps: u32) -> Self {
            Self { bps: u64::from(kbps) * 1_000, finish: 0, queue: VecDeque::new(), drops: 0 }
        }

        fn send(&mut self, now: u64, d: Vec<u8>, socket: &UdpSocket, to: SocketAddr) {
            if self.bps == 0 {
                socket.send_to(&d, to).ok();
                return;
            }
            let rate = u128::from(self.bps);
            let at = u128::from(now) * rate;
            let finish = self.finish.max(at) + d.len() as u128 * 8_000_000;
            if finish - at > rate * 60_000 {
                self.drops += 1;
                return;
            }
            self.finish = finish;
            self.queue.push_back((finish.div_ceil(rate) as u64, d));
        }

        fn drain(&mut self, now: u64, socket: &UdpSocket, to: SocketAddr) {
            while self.queue.front().is_some_and(|(due, _)| *due <= now) {
                let (_, d) = self.queue.pop_front().unwrap();
                socket.send_to(&d, to).ok();
            }
        }
    }

    /// Compare bytes released by the peer against the time integral of its estimate
    /// in each complete call-second after warmup. Attribute a send to its release time.
    struct RateWindows {
        at: u64,
        end: u64,
        next_second: u64,
        estimate: u64,
        budget: u128,
        bytes: u64,
        elapsed: u64,
        in_band: u64,
    }

    impl RateWindows {
        fn new(start: u64, seconds: u64, estimate: u64) -> Self {
            Self { at: start, end: start + seconds * 1_000_000, next_second: start + 1_000_000,
                estimate, budget: 0, bytes: 0, elapsed: 0, in_band: 0 }
        }

        fn record(&mut self, now: u64, estimate: u64, bytes: u64) {
            let until = now.min(self.end);
            while self.at < until {
                let to = until.min(self.next_second);
                self.budget += u128::from(to - self.at) * u128::from(self.estimate);
                self.at = to;
                if to == self.next_second {
                    self.elapsed += 1;
                    let sent = u128::from(self.bytes) * 8_000_000;
                    if self.elapsed > 5 && (6 * self.budget..=12 * self.budget).contains(&(10 * sent)) {
                        self.in_band += 1;
                    }
                    self.bytes = 0;
                    self.budget = 0;
                    self.next_second += 1_000_000;
                }
            }
            self.estimate = estimate;
            if now >= self.at && now < self.end {
                self.bytes += bytes;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn network(
        mut peer: Peer,
        socket: UdpSocket,
        args: &Args,
        media: Receiver<Media>,
        frames: Sender<makepad_call::video::Frame>,
        ring: Arc<JitterRing>,
        key_wanted: Arc<AtomicBool>,
        offset: Arc<AtomicI64>,
        link: Arc<Link>,
        status: Arc<Mutex<Status>>,
        stop: Arc<AtomicBool>,
        now_us: impl Fn() -> u64,
    ) {
        let to = args.peer;
        let mut egress = Egress::new(args.path_kbps);
        let mut windows: Option<RateWindows> = None;
        let mut read_timeout_us = 0;
        let mut buf = vec![0u8; 2048];
        let mut out = Vec::new();
        let mut events = Vec::new();
        let mut up = Resampler::new();
        up.set_ratio(VOICE_RATE, makepad_teamtalk::INTERNAL_RATE, 0.0);
        let mut pcm = Vec::with_capacity(MAX_FRAME);
        let (mut voice_frames, mut voice_silent, mut pictures, mut keys, mut video_bytes) = (0u64, 0u64, 0u64, 0u64, 0u64);
        let mut last_status = 0u64;
        while !stop.load(Ordering::Relaxed) {
            let now = now_us();
            let next = peer.next_send_us().into_iter().chain(egress.queue.front().map(|(due, _)| *due)).min();
            let timeout = next.map_or(2_000, |at| at.saturating_sub(now).clamp(100, 2_000));
            if timeout != read_timeout_us {
                socket.set_read_timeout(Some(Duration::from_micros(timeout))).expect("set UDP read timeout");
                read_timeout_us = timeout;
            }
            if let Ok((n, from)) = socket.recv_from(&mut buf) {
                if from == to {
                    peer.receive(now_us(), &buf[..n], &mut events);
                }
                // take whatever else is waiting without blocking
                socket.set_nonblocking(true).ok();
                while let Ok((n, from)) = socket.recv_from(&mut buf) {
                    if from == to {
                        peer.receive(now_us(), &buf[..n], &mut events);
                    }
                }
                socket.set_nonblocking(false).ok();
            }
            for e in events.drain(..) {
                match e {
                    Event::Voice { seq, flags, payload, .. } => {
                        voice_frames += 1;
                        pcm.clear();
                        let silent = flags & voice_flags::SILENT != 0;
                        if silent {
                            voice_silent += 1;
                        } else if let Some(samples) = adpcm_decode(&payload) {
                            for s in samples {
                                up.push(s, |x| pcm.push((x * 32768.0).round().clamp(-32768.0, 32767.0) as i16));
                            }
                        }
                        if silent || pcm.is_empty() {
                            pcm.resize(VOICE_FRAME * 3, 0);
                        }
                        pcm.truncate(MAX_FRAME);
                        ring.insert(seq, &pcm, silent);
                    }
                    Event::Video { frame, .. } => {
                        frames.send(frame).ok();
                    }
                }
            }
            let now = now_us();
            let sent_before = peer.stats.sent_bytes;
            while let Ok(m) = media.try_recv() {
                match m {
                    Media::Voice { seq, silent, talk_start, capture_us, payload } => {
                        let mut flags = 0;
                        if silent {
                            flags |= voice_flags::SILENT;
                        }
                        if talk_start {
                            flags |= voice_flags::TALK_START;
                        }
                        let v = Voice { flags, seq, capture_us, payload: &payload };
                        if let Ok(d) = peer.send_voice(now, stream::VOICE, &v, !silent) {
                            out.push(d);
                        }
                    }
                    Media::Video { au, key, capture_us } => {
                        pictures += 1;
                        keys += key as u64;
                        video_bytes += au.len() as u64;
                        peer.send_video(now, stream::CAMERA_HIGH, &au, key, capture_us, &mut out).ok();
                    }
                }
            }
            if peer.key_frame_due(stream::CAMERA_HIGH, now) {
                key_wanted.store(true, Ordering::Relaxed);
            }
            peer.poll(now, &mut out);
            link.estimate_bps.store(peer.estimate_bps(), Ordering::Relaxed);
            link.pacer_delay_us.store(peer.pacer_delay_us(now), Ordering::Relaxed);
            link.loss_level_bits.store(peer.video_loss_level().unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
            egress.drain(now, &socket, to);
            for d in out.drain(..) {
                egress.send(now, d, &socket, to);
            }
            if let Some(&start) = link.start_us.get() {
                let w = windows.get_or_insert_with(|| RateWindows::new(start, args.seconds, peer.estimate_bps()));
                w.record(now, peer.estimate_bps(), peer.stats.sent_bytes - sent_before);
            }
            if now - last_status > 200_000 || (last_status == 0 && peer.stats.received > 0) {
                last_status = now;
                if let Some(o) = peer.clock_offset_us() {
                    offset.store(o, Ordering::Relaxed);
                }
                let mut s = status.lock().unwrap();
                s.rtt_us = peer.rtt_us();
                s.estimate_bps = peer.estimate_bps();
                s.random_loss = peer.random_loss();
                s.loss_events = peer.loss_events();
                s.path_drops = egress.drops;
                if let Some(w) = &windows {
                    (s.seconds_in_band, s.seconds_measured) = (w.in_band, w.elapsed.saturating_sub(5));
                }
                s.stats = peer.stats;
                s.video = peer.video_stats().next().map(|(_, v)| v);
                (s.voice_frames, s.voice_silent, s.pictures_sent, s.keys_sent, s.video_bytes) = (voice_frames, voice_silent, pictures, keys, video_bytes);
            }
        }
        let mut s = status.lock().unwrap();
        if let Some(w) = &mut windows {
            w.record(now_us(), peer.estimate_bps(), 0);
            (s.seconds_in_band, s.seconds_measured) = (w.in_band, w.elapsed.saturating_sub(5));
        }
        s.path_drops = egress.drops;
        s.estimate_bps = peer.estimate_bps();
        s.random_loss = peer.random_loss();
        s.loss_events = peer.loss_events();
        s.rtt_us = peer.rtt_us();
        s.stats = peer.stats;
        s.video = peer.video_stats().next().map(|(_, v)| v);
        (s.voice_frames, s.voice_silent, s.pictures_sent, s.keys_sent, s.video_bytes) = (voice_frames, voice_silent, pictures, keys, video_bytes);
    }

    /// A test picture: moving diagonal bands, a square that crosses the picture once a second,
    /// and the picture's number in binary along the top.
    fn picture(w: u32, h: u32, n: u64, nv12: &mut Vec<u8>) {
        nv12.resize((w * h * 3 / 2) as usize, 0);
        let (sq, sx, sy) = (h / 4, ((n % 30) as u32 * (w - h / 4)) / 29, h / 2 - h / 8);
        for y in 0..h {
            for x in 0..w {
                let band = ((x + y + n as u32 * 4) / 16) % 2;
                let mut v = 60 + band * 40;
                if x >= sx && x < sx + sq && y >= sy && y < sy + sq {
                    v = 230;
                }
                if y < 24 {
                    let bit = x / (w / 32).max(1);
                    v = if bit < 32 && (n >> (31 - bit)) & 1 == 1 { 235 } else { 16 };
                }
                nv12[(y * w + x) as usize] = v as u8;
            }
        }
        let c = (w * h) as usize;
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                nv12[c + (y * w + 2 * x) as usize] = (96 + (x * 64 / (w / 2))) as u8;
                nv12[c + (y * w + 2 * x + 1) as usize] = (160 - (y * 64 / (h / 2))) as u8;
            }
        }
    }

    fn xorshift(s: &mut u64) -> u8 {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        (*s >> 56) as u8
    }

    /// A picture no encoder codes cheaply: noise scrolling 14 px right and 6 down a picture,
    /// fresh noise across a sixth of the height, three squares crossing fast, the frame number.
    /// Its cost has a floor above most asks (at 720p the NVIDIA encoders sent about 17 Mbit/s
    /// of it whatever they were asked), so it shows what the call does with an encoder that
    /// cannot hold its rate.
    fn noise_picture(w: u32, h: u32, n: u64, field: &[u8], nv12: &mut Vec<u8>) {
        let (w, h) = (w as usize, h as usize);
        nv12.resize(w * h * 3 / 2, 0);
        let (dx, dy) = ((n as usize * 14) % w, (n as usize * 6) % h);
        let mut fresh = 0x9e37_79b9_7f4a_7c15u64 ^ n.wrapping_mul(0x2545_f491_4f6c_dd1d);
        for y in 0..h {
            let row = &mut nv12[y * w..(y + 1) * w];
            if y >= h / 3 && y < h / 3 + h / 6 {
                for p in row.iter_mut() {
                    *p = 16 + (xorshift(&mut fresh) as u32 * 219 / 255) as u8;
                }
            } else {
                let src = &field[((y + dy) % h) * w..((y + dy) % h + 1) * w];
                for (x, p) in row.iter_mut().enumerate() {
                    *p = 16 + (src[(x + dx) % w] as u32 * 219 / 255) as u8;
                }
            }
        }
        let sq = h / 6;
        for k in 0..3usize {
            let sx = (n as usize * (37 + 11 * k)) % (w - sq);
            let sy = (k * h / 3 + n as usize * 7) % (h - sq);
            for y in sy..sy + sq {
                nv12[y * w + sx..y * w + sx + sq].fill(235 - 60 * k as u8);
            }
        }
        for y in 0..24.min(h) {
            for x in 0..w {
                let bit = x / (w / 32).max(1);
                nv12[y * w + x] = if bit < 32 && (n >> (31 - bit)) & 1 == 1 { 235 } else { 16 };
            }
        }
        let c = w * h;
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                let v = field[((y * 2 + dy) % h) * w + (x * 2 + dx) % w];
                nv12[c + y * w + 2 * x] = 64 + v / 2;
                nv12[c + y * w + 2 * x + 1] = 192 - v / 2;
            }
        }
    }

    fn encode(args: &Args, tx: Sender<Media>, key_wanted: Arc<AtomicBool>, link: Arc<Link>, status: Arc<Mutex<Status>>, stop: Arc<AtomicBool>, now_us: impl Fn() -> u64) {
        #[cfg(target_os = "linux")]
        if !makepad_hal::stream_encode::hal_stream_encoder_offscreen_device() {
            eprintln!("no Vulkan device: no video");
            return;
        }
        let initial_ask = video_ask(link.estimate_bps.load(Ordering::Relaxed));
        let mut guard = Guard::with_top(Guard::rung_for(initial_ask), args.top);
        let mut encoder = None;
        let mut reported_error = false;
        let mut kbps = 0;
        let mut captured = std::collections::BTreeMap::new();
        let (mut held_max, mut held_us) = (0usize, Vec::new());
        let mut nv12 = Vec::new();
        let mut field = Vec::new();
        let mut field_size = (0, 0);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let period = Duration::from_nanos(1_000_000_000 / 30);
        let mut due = Instant::now();
        for n in 0u64.. {
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let now = Instant::now();
            due += period;
            if due <= now {
                due = now + period;
            }
            let capture_us = now_us();
            let level = f64::from_bits(link.loss_level_bits.load(Ordering::Relaxed));
            let decision = guard.tick(capture_us, link.estimate_bps.load(Ordering::Relaxed),
                link.pacer_delay_us.load(Ordering::Relaxed), (!level.is_nan()).then_some(level));
            let step = guard.take_step();
            let rung = guard.rung();
            let want = (guard.ask_bps() / 1_000).clamp(1, u64::from(args.kbps)) as u32;
            if let Some(step) = step {
                eprintln!("[{}] guard: {} -> {} ({})", args.name, step.from, step.to, step.why);
                encoder = None;
                captured.clear();
                reported_error = false;
            }
            if encoder.is_none() && (decision == Tick::Encode || step.is_some()) {
                let options = VideoStreamEncoderOptions {
                    codec: StreamVideoCodec::H264,
                    width: rung.width,
                    height: rung.height,
                    fps: rung.fps,
                    bitrate_kbps: want,
                    keyint: 0,
                    low_latency: true,
                    full_chroma: false,
                };
                match VideoStreamEncoder::new(options) {
                    Ok(e) => {
                        eprintln!("[{}] encoder: {}", args.name, e.description());
                        encoder = Some(e);
                        kbps = want;
                    }
                    Err(e) => {
                        if !reported_error {
                            eprintln!("[{}] cannot open encoder for {rung}: {e:?}", args.name);
                            reported_error = true;
                        }
                        kbps = 0;
                    }
                }
            }
            if let Some(e) = &mut encoder {
                if u64::from(want.abs_diff(kbps)) * 20 > u64::from(kbps) {
                    e.set_bitrate_kbps(want);
                    kbps = want;
                }
            }
            {
                let mut s = status.lock().unwrap();
                s.rung = Some(rung);
                s.asked_kbps = kbps;
                s.steps += u64::from(step.is_some());
                s.skipped += u64::from(decision == Tick::Skip);
            }
            if decision == Tick::Skip {
                continue;
            }
            let Some(e) = &mut encoder else { continue };
            if args.noise {
                if field_size != (rung.width, rung.height) {
                    field.clear();
                    field.resize_with(rung.width as usize * rung.height as usize, || xorshift(&mut seed));
                    field_size = (rung.width, rung.height);
                }
                noise_picture(rung.width, rung.height, n, &field, &mut nv12);
            } else {
                picture(rung.width, rung.height, n, &mut nv12);
            }
            if key_wanted.swap(false, Ordering::Relaxed) {
                e.request_keyframe();
            }
            let pts = capture_us as i64 * 10;
            captured.insert(pts, capture_us);
            match e.push_frame_nv12(&nv12, pts) {
                Ok(packets) => {
                    for p in packets {
                        let packet_us = now_us();
                        guard.encoded(packet_us, p.data.len(), p.is_key);
                        let capture_us = captured.remove(&p.pts_100ns).unwrap_or(capture_us);
                        while captured.first_key_value().is_some_and(|(&pts, _)| pts < p.pts_100ns) {
                            captured.pop_first();
                        }
                        held_us.push(packet_us.saturating_sub(capture_us));
                        tx.send(Media::Video { au: p.data, key: p.is_key, capture_us }).ok();
                    }
                }
                Err(err) => eprintln!("encode: {err:?}"),
            }
            held_max = held_max.max(captured.len());
        }
        held_us.sort_unstable();
        let at = |q: f64| held_us.get(((held_us.len() as f64 - 1.0) * q) as usize).map_or(0.0, |&v| v as f64 / 1000.0);
        eprintln!("encoder held: capture to packet median {:.1} ms, p90 {:.1} ms, up to {held_max} pictures pending", at(0.5), at(0.9));
    }

    fn decode(
        rx: Receiver<makepad_call::video::Frame>,
        grabs: Option<String>,
        label: String,
        offset: Arc<AtomicI64>,
        stop: Arc<AtomicBool>,
        now_us: impl Fn() -> u64,
    ) -> (u64, f64, String, String) {
        let mut decoder = None;
        let mut last_sps = Vec::new();
        let mut decoded = 0u64;
        let mut times = Vec::new();
        let mut latencies = Vec::new();
        let mut saved = 0u32;
        // capture times of the pictures pushed and not yet out, by frame number (the pts)
        let mut pending: std::collections::BTreeMap<i64, u64> = std::collections::BTreeMap::new();
        let (mut pushed, mut held_max, mut never_out) = (0u64, 0usize, 0u64);
        while !stop.load(Ordering::Relaxed) {
            let Ok(frame) = rx.recv_timeout(Duration::from_millis(50)) else { continue };
            if frame.key {
                if let Some(sps) = split_annex_b(&frame.data).into_iter().find(|nal| nal_unit_type(nal) == NAL_TYPE_SPS) {
                    if sps != last_sps.as_slice() {
                        decoder = None;
                        never_out += pending.len() as u64;
                        pending.clear();
                        last_sps.clear();
                        last_sps.extend_from_slice(sps);
                    }
                }
                if decoder.is_none() {
                    match VideoStreamDecoder::new_low_latency(StreamVideoCodec::H264) {
                        Ok(d) => decoder = Some(d),
                        Err(e) => eprintln!("cannot open decoder for key frame {}: {e:?}", frame.frame),
                    }
                }
            }
            let Some(d) = &mut decoder else { continue };
            let t0 = Instant::now();
            pending.insert(frame.frame as i64, frame.capture_us);
            pushed += 1;
            match d.push_packet(&frame.data, frame.frame as i64) {
                Ok(pictures) => {
                    times.push(t0.elapsed().as_secs_f64() * 1000.0);
                    let o = offset.load(Ordering::Relaxed);
                    for p in pictures {
                        decoded += 1;
                        // pictures pushed before this one that never came out were dropped
                        let older: Vec<i64> = pending.range(..p.pts_100ns).map(|(k, _)| *k).collect();
                        never_out += older.len() as u64;
                        for k in older {
                            pending.remove(&k);
                        }
                        if let (Some(capture_us), true) = (pending.remove(&p.pts_100ns), o != i64::MIN) {
                            // their capture time on our clock, to this picture in our hands
                            latencies.push((now_us() as i64 - (capture_us as i64 - o)) as f64 / 1000.0);
                        }
                        // a picture now and then: the first, then one every five seconds
                        if let Some(dir) = &grabs {
                            if (decoded == 1 || decoded % 150 == 0) && saved < 8 && !p.nv12.is_empty() {
                                let png = encode_png(p.width, p.height, &p.to_rgb8());
                                let path = format!("{dir}/{label}-{decoded:05}.png");
                                if std::fs::write(&path, png).is_ok() {
                                    saved += 1;
                                }
                            }
                        }
                    }
                }
                Err(e) => eprintln!("decode picture {}: {e:?}", frame.frame),
            }
            held_max = held_max.max(pending.len());
        }
        times.sort_by(f64::total_cmp);
        let median = times.get(times.len() / 2).copied().unwrap_or(0.0);
        let name = decoder.as_ref().and_then(VideoStreamDecoder::name).unwrap_or_default();
        latencies.sort_by(f64::total_cmp);
        let latency = if latencies.is_empty() {
            "-".to_string()
        } else {
            let at = |p: f64| latencies[((latencies.len() - 1) as f64 * p) as usize];
            format!("median {:.1} ms, p90 {:.1} ms (decoder held up to {held_max} pictures, {never_out} of {pushed} never came out)", at(0.5), at(0.9))
        };
        (decoded, median, name, latency)
    }

    fn encode_png(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
        use makepad_zune_png::makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
        let options = EncoderOptions::default().set_width(width as usize).set_height(height as usize).set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGB);
        let mut out = Vec::new();
        makepad_zune_png::PngEncoder::new(rgb, options).encode(&mut out).ok();
        out
    }

    /// A simulated microphone: a tone in talk spurts (1.5 s on, 0.5 s off), in device blocks
    /// of 10 ms at 48 kHz, through the framer to ADPCM at 16 kHz.
    fn microphone(tone: f32, tx: Sender<Media>, stop: Arc<AtomicBool>, now_us: impl Fn() -> u64) {
        let mut framer = Framer::new(FramerConfig { out_rate: VOICE_RATE, frame_samples: VOICE_FRAME, gate_threshold_rms: 0.01, gate_hangover_ms: 200 }).unwrap();
        let mut adpcm = AdpcmState::default();
        let mut block = [0f32; BLOCK];
        let mut phase = 0f32;
        let start = Instant::now();
        let period = Duration::from_millis(10);
        let mut n = 0u32;
        while !stop.load(Ordering::Relaxed) {
            let due = start + period * n;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let block_us = now_us();
            let talking = (n % 200) < 150;
            for s in block.iter_mut() {
                *s = if talking { 0.3 * (phase * std::f32::consts::TAU).sin() } else { 0.0 };
                phase = (phase + tone / DEVICE_RATE as f32).fract();
            }
            framer.push_mono(DEVICE_RATE, &block, block_us, &mut |f| {
                let mut payload = Vec::new();
                if !f.silent {
                    adpcm_encode_into(&mut adpcm, 4, f.samples, &mut payload);
                }
                tx.send(Media::Voice { seq: f.seq, silent: f.silent, talk_start: f.talk_start, capture_us: f.capture_us, payload }).ok();
            });
            n += 1;
        }
    }

    #[derive(Default)]
    struct Heard {
        hz: f64,
        loud_blocks: u64,
        concealed: u32,
        underruns: u32,
    }

    /// A simulated speaker: renders the playout every 10 ms and measures the tone it hears.
    fn speaker(ring: Arc<JitterRing>, stop: Arc<AtomicBool>) -> Heard {
        let mut playout = Playout::new(PlayoutConfig { default_frame: VOICE_FRAME * 3, ..Default::default() });
        let mut out = [0f32; BLOCK];
        let start = Instant::now();
        let period = Duration::from_millis(10);
        let (mut crossings, mut loud_samples, mut loud_blocks) = (0u64, 0u64, 0u64);
        let mut n = 0u32;
        while !stop.load(Ordering::Relaxed) {
            let due = start + period * n;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            n += 1;
            if !playout.wants_render(&ring) {
                continue;
            }
            playout.render(&ring, DEVICE_RATE, &mut out);
            let rms = (out.iter().map(|s| s * s).sum::<f32>() / BLOCK as f32).sqrt();
            if rms > 0.1 {
                loud_blocks += 1;
                loud_samples += BLOCK as u64;
                crossings += out.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as u64;
            }
        }
        let hz = if loud_samples > 0 { crossings as f64 / 2.0 / (loud_samples as f64 / DEVICE_RATE) } else { 0.0 };
        Heard { hz, loud_blocks, concealed: playout.stats.concealed, underruns: playout.stats.underruns }
    }
}
