//! fake_phone <party url> [seconds] [<W>x<H>] [--say <W>x<H>] [--turn <s>]:
//! a phone going live at a party, for trying the live path without one. A
//! dev tool, never shipped. `--say` is the camera size the offer reports
//! (a phone's sensor size, landscape when held upright); `--turn` turns
//! the phone after that many seconds (the pictures' width and height swap,
//! a new keyframe with its new size).
//!
//! It joins the party at its address (`https://host:port/p/<code>`),
//! queues, calls when the party says it is up, and sends a camera's H.264
//! of a moving test scene: the GPU's own encoder (Vulkan Video), so P
//! frames, real fragment sizes and the parameter sets a hardware encoder
//! sends, a keyframe at the start and whenever the party asks for one (a
//! PLI), and a connectivity check every few seconds, as a browser does. It
//! prints its place as the party tells it, ends after `seconds` (40) or
//! when the party ends its slot, and leaves. Linux only.

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = phone::run() {
        eprintln!("fake_phone: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake_phone: Linux only (the GPU's H.264 encoder through Vulkan Video)");
}

#[cfg(target_os = "linux")]
mod phone {
    use makepad_strict_json::{self as sj, Value};
    use makepad_video::{StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
    use makepad_webrtc::browser::Browser;
    use makepad_webrtc::rtp::{self, AccessUnit};
    use std::io::{Read, Write};
    use std::net::{SocketAddr, UdpSocket};
    use std::time::{Duration, Instant};

    const FPS: u32 = 15;
    /// A browser checks the call's consent this often.
    const CHECK_EVERY: f64 = 5.0;

    struct Party {
        host: String,
        port: String,
        code: String,
    }

    impl Party {
        fn parse(url: &str) -> Result<Party, String> {
            let rest = url.strip_prefix("https://").ok_or("the party's address starts with https://")?;
            let (authority, path) = rest.split_once('/').ok_or("no /p/<code> in the address")?;
            let (host, port) = authority.rsplit_once(':').unwrap_or((authority, "443"));
            let code = path.strip_prefix("p/").ok_or("no /p/<code> in the address")?.trim_end_matches('/');
            Ok(Party { host: host.into(), port: port.into(), code: code.into() })
        }

        /// One request over its own TLS connection (the party's certificate
        /// is its own: not checked here).
        fn ask(&self, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
            let (mut tls, _) = makepad_network::SocketStream::connect_capture(&self.host, &self.port).map_err(|e| format!("connect: {e}"))?;
            let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
            let mut raw = format!("{method} /p/{}/{path} HTTP/1.1\r\nHost: {}\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n", self.code, self.host, body.len()).into_bytes();
            raw.extend_from_slice(body);
            tls.write_all(&raw).map_err(|e| format!("send: {e}"))?;
            let mut out = Vec::new();
            let _ = tls.read_to_end(&mut out);
            let split = out.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no answer")?;
            let status = std::str::from_utf8(&out[9..12]).ok().and_then(|s| s.parse().ok()).ok_or("no status")?;
            Ok((status, out[split + 4..].to_vec()))
        }

        fn state(&self, method: &str, path: &str, token: &str) -> Result<Value, String> {
            let (status, body) = self.ask(method, path, Some(token), b"")?;
            let value = sj::parse(&body).map_err(|e| format!("{path}: {status} {e:?}"))?;
            Ok(value)
        }
    }

    fn named(state: &Value) -> String {
        let field = |key: &str| state.get(key).and_then(Value::as_u64).map(|v| format!(" {key} {v}")).unwrap_or_default();
        format!("{}{}{}{}", state.get("state").and_then(Value::as_str).unwrap_or("?"), field("ahead"), field("left"), field("waiting"))
    }

    /// A park on a sunny day: a sky, the sun crossing it, hills, and a kite.
    fn scene(width: usize, height: usize, t: f64, out: &mut Vec<u8>) {
        out.clear();
        let (w, h) = (width as f64, height as f64);
        let sun = (w * (0.2 + 0.6 * (0.5 + 0.5 * (t * 0.3).sin())), h * 0.22);
        let kite = (w * (0.68 + 0.08 * (t * 0.9).sin()), h * (0.38 + 0.05 * (t * 1.7).cos()));
        for y in 0..height {
            let fy = y as f64;
            for x in 0..width {
                let fx = x as f64;
                let hill = h * (0.66 + 0.06 * ((fx / w) * 6.0 + t * 0.2).sin());
                let mut c = if fy > hill {
                    [70, (170.0 - (fy - hill) * 0.3).max(90.0) as u8, 80]
                } else {
                    [(110.0 + 90.0 * fy / h) as u8, (170.0 + 60.0 * fy / h) as u8, 245]
                };
                if (fx - sun.0).hypot(fy - sun.1) < h * 0.08 {
                    c = [255, 222, 100];
                }
                if (fx - kite.0).abs() / (w * 0.035) + (fy - kite.1).abs() / (h * 0.07) < 1.0 {
                    c = [230, 70, 90];
                }
                out.extend_from_slice(&c);
            }
        }
    }

    /// Whether some packet of an RTCP compound asks for a keyframe (PLI).
    fn asks_keyframe(rtcp: &[u8]) -> bool {
        let mut at = 0;
        while at + 4 <= rtcp.len() {
            if rtcp[at + 1] == 206 && rtcp[at] & 0x1f == 1 {
                return true;
            }
            at += 4 * (u16::from_be_bytes([rtcp[at + 2], rtcp[at + 3]]) as usize + 1);
        }
        false
    }

    pub fn run() -> Result<(), String> {
        let args: Vec<String> = std::env::args().collect();
        let party = Party::parse(args.get(1).ok_or("usage: fake_phone <party url> [seconds] [<W>x<H>]")?)?;
        let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40.0);
        let size = |s: &str| s.split_once('x').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)));
        let (mut width, mut height) = args.get(3).and_then(|s| size(s)).unwrap_or((640, 480));
        let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
        let said = flag("--say").and_then(|s| size(s)).unwrap_or((width, height));
        let mut turn: Option<f64> = flag("--turn").and_then(|s| s.parse().ok());

        if !makepad_hal::stream_encode::hal_stream_encoder_offscreen_device() {
            return Err("no Vulkan device for the encoder".into());
        }
        let options = VideoStreamEncoderOptions { codec: StreamVideoCodec::H264, width, height, fps: FPS, bitrate_kbps: 2_000, keyint: 0, low_latency: true, full_chroma: false };
        let mut encoder = VideoStreamEncoder::new(options).map_err(|e| format!("encoder: {e:?}"))?;
        eprintln!("fake_phone: encoder {}", encoder.description());

        let (status, body) = party.ask("POST", "join", None, b"")?;
        let token = sj::parse(&body).ok().and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_string)).ok_or(format!("join: {status}"))?;
        let mut state = party.state("POST", "live/queue", &token)?;
        if state.get("available").and_then(Value::as_bool) != Some(true) {
            return Err(format!("live is off at this party: {}", named(&state)));
        }
        let queued = Instant::now();
        while named(&state).split(' ').next() != Some("up") {
            eprintln!("fake_phone: {}", named(&state));
            std::thread::sleep(Duration::from_secs(1));
            state = party.state("GET", "live", &token)?;
        }
        eprintln!("fake_phone: up after {:.1} s", queued.elapsed().as_secs_f64());

        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("udp: {e}"))?;
        socket.set_read_timeout(Some(Duration::from_millis(50))).map_err(|e| e.to_string())?;
        let mut phone = Browser::new(&socket.local_addr().map_err(|e| e.to_string())?.to_string());
        let (status, answer) = party.ask("POST", &format!("live/offer?w={}&h={}", said.0, said.1), Some(&token), phone.offer().as_bytes())?;
        let answer = String::from_utf8(answer).map_err(|_| "the answer is not text")?;
        if status != 200 {
            return Err(format!("offer: {status} {answer}"));
        }
        let candidate = answer.lines().find_map(|l| l.strip_prefix("a=candidate:")).ok_or("no candidate in the answer")?;
        let fields: Vec<&str> = candidate.split_whitespace().collect();
        let to: SocketAddr = format!("{}:{}", fields.get(4).ok_or("candidate")?, fields.get(5).ok_or("candidate")?).parse().map_err(|_| "candidate address")?;
        let called = Instant::now();
        if !phone.connect(&socket, &answer, to, Duration::from_secs(10)) {
            return Err(format!("the call to {to} did not come up"));
        }
        eprintln!("fake_phone: the call to {to} is up after {:.2} s", called.elapsed().as_secs_f64());
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;

        let started = Instant::now();
        let (mut frames, mut keyframes, mut bytes, mut packets, mut asked) = (0u64, 0u64, 0u64, 0u64, 0u64);
        let (mut last_check, mut last_look) = (0.0f64, 0.0f64);
        let mut seen_live: Option<f64> = None;
        let mut rgb = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            let now = started.elapsed().as_secs_f64();
            if now > seconds {
                break;
            }
            while let Ok((n, _)) = socket.recv_from(&mut buf) {
                if rtp::is_rtcp(&buf[..n]) && phone.feedback(&buf[..n]).is_some_and(|clear| asks_keyframe(&clear)) {
                    asked += 1;
                    encoder.request_keyframe();
                }
            }
            if now - last_check >= CHECK_EVERY {
                last_check = now;
                let _ = socket.send_to(&phone.check(&answer, false), to);
            }
            if now - last_look >= 1.0 {
                last_look = now;
                let state = party.state("GET", "live", &token)?;
                let name = named(&state);
                eprintln!("fake_phone: {now:5.1} s {name}; {frames} frames ({keyframes} key, {asked} asked), {} KB in {packets} packets", bytes / 1024);
                if name.starts_with("live") && seen_live.is_none() {
                    seen_live = Some(now);
                }
                if name.starts_with("out") {
                    eprintln!("fake_phone: the party ended the slot");
                    break;
                }
            }
            if turn.is_some_and(|at| now >= at) {
                turn = None;
                (width, height) = (height, width);
                encoder = VideoStreamEncoder::new(VideoStreamEncoderOptions { width, height, ..options }).map_err(|e| format!("encoder: {e:?}"))?;
                eprintln!("fake_phone: {now:5.1} s turned: {width}x{height}");
            }
            scene(width as usize, height as usize, now, &mut rgb);
            let pts = (frames as i64) * 10_000_000 / FPS as i64;
            for packet in encoder.push_frame_rgb8(&rgb, pts).map_err(|e| format!("encode: {e:?}"))? {
                keyframes += packet.is_key as u64;
                bytes += packet.data.len() as u64;
                let au = AccessUnit { data: packet.data, timestamp: (frames * 90_000 / FPS as u64) as u32, keyframe: packet.is_key };
                for datagram in phone.video(&au) {
                    packets += 1;
                    let _ = socket.send_to(&datagram, to);
                }
            }
            frames += 1;
            let next = Duration::from_secs_f64(frames as f64 / FPS as f64);
            if let Some(wait) = next.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let left = party.state("POST", "live/leave", &token)?;
        eprintln!(
            "fake_phone: done: live {} after the call came up; {frames} frames ({keyframes} key, {asked} keyframes asked for), {} KB; left: {}",
            seen_live.map_or("never".into(), |t| format!("{t:.1} s")),
            bytes / 1024,
            named(&left)
        );
        Ok(())
    }
}
