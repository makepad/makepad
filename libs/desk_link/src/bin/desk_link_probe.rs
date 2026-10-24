//! desk-link-probe <host[:port]> [--secs N] [--input] [--paused] [--flap MS] [--split tiles|interleave|off]: a headless Desk Link viewer for
//! checks. It connects with this machine's fleet credential, prints the Welcome, counts the
//! pictures and bytes, pings once a second (RTT), and with `--input` moves the desktop's pointer
//! between two points twice a second, timing each move until the desktop's own Cursor message
//! reports the pointer there (input injection end to end).

use makepad_network::fleet::auth as fleet_auth;
use makepad_desk_link::{self as proto, ClientMsg, Inbox, ServerMsg};
use makepad_network::SocketStream;
use std::io::Read;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(target) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: desk-link-probe <host[:port]> [--secs N] [--input] [--paused] [--flap MS] [--split tiles|interleave|off]");
        std::process::exit(2);
    };
    let secs: u64 = args.windows(2).find(|w| w[0] == "--secs").and_then(|w| w[1].parse().ok()).unwrap_or(10);
    let input = args.iter().any(|a| a == "--input");
    let paused = args.iter().any(|a| a == "--paused");
    // --flap MS: pause and resume every MS milliseconds (the desktop releases its picture
    // texture whenever nobody watches: a lifetime test)
    let flap: Option<u64> = args.windows(2).find(|w| w[0] == "--flap").and_then(|w| w[1].parse().ok());
    // --split: how the desktop shares its encode engines (for every viewer), asked after the Welcome
    let split = args.windows(2).find(|w| w[0] == "--split").map(|w| match w[1].as_str() {
        "tiles" => proto::split::TILES,
        "interleave" => proto::split::INTERLEAVE,
        _ => proto::split::OFF,
    });
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.to_string()),
        _ => (target.clone(), proto::DEFAULT_PORT.to_string()),
    };
    if let Err(e) = run(&host, &port, secs, input, paused, flap, split) {
        eprintln!("probe: {e}");
        std::process::exit(1);
    }
}

fn run(host: &str, port: &str, secs: u64, input: bool, paused: bool, flap: Option<u64>, split: Option<u8>) -> Result<(), String> {
    let credential = fleet_auth::own_credential().ok_or("no fleet credential")?;
    let (mut stream, fp) = SocketStream::connect_capture(host, port).map_err(|e| format!("connect: {e}"))?;
    println!("probe: certificate {}", makepad_network::tls::to_hex(&fp));
    let hello = ClientMsg::Hello { version: proto::VERSION, name: "probe".into(), authorization: credential.authorization(&fp), screen: -1, paused, caps: proto::caps::FULL_CHROMA };
    proto::write_all(&mut stream, &hello.encode()).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let mut inbox = Inbox::default();
    let mut buf = vec![0u8; 1 << 20];
    let (mut pictures, mut bytes, mut keys) = (0u64, 0u64, 0u64);
    let mut last_ping = Instant::now() - Duration::from_secs(5);
    let mut rtts: Vec<f64> = Vec::new();
    let mut welcomed = false;
    // --input: the point the pointer was sent to and when
    let mut moving: Option<((u16, u16), Instant)> = None;
    let mut next_move = Instant::now();
    let mut toggle = false;
    let mut latencies: Vec<f64> = Vec::new();
    // the desktop's frame submit to its bitstream, per picture (ms), and the encoder streams seen
    let mut encodes: Vec<f64> = Vec::new();
    let mut streams = 0u32;
    let mut slowest = (u32::MAX, 0f64);
    let epoch = Instant::now();
    let mut flapped = (Instant::now(), paused, 0u32);
    while start.elapsed() < Duration::from_secs(secs) {
        if let Some(ms) = flap.filter(|_| welcomed) {
            if flapped.0.elapsed() >= Duration::from_millis(ms) {
                flapped = (Instant::now(), !flapped.1, flapped.2 + 1);
                let msg = if flapped.1 { ClientMsg::Pause } else { ClientMsg::Resume };
                proto::write_all(&mut stream, &msg.encode()).map_err(|e| e.to_string())?;
            }
        }
        if last_ping.elapsed() >= Duration::from_secs(1) {
            last_ping = Instant::now();
            let ns = epoch.elapsed().as_nanos() as u64;
            proto::write_all(&mut stream, &ClientMsg::Ping { client_ns: ns }.encode()).map_err(|e| e.to_string())?;
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
            match ServerMsg::decode(ty, &payload).map_err(|e| e.to_string())? {
                ServerMsg::Welcome { screens, screen, stream: stream_size, input, input_note, server, .. } => {
                    welcomed = true;
                    if let Some(split) = split {
                        proto::write_all(&mut stream, &ClientMsg::SetSplit { split }.encode()).map_err(|e| e.to_string())?;
                        proto::write_all(&mut stream, &ClientMsg::RequestKeyframe.encode()).map_err(|e| e.to_string())?;
                    }
                    println!("probe: welcome from {server}: screen {screen} of {} ({stream_size:?}), input {}{}", screens.len(), input, if input { String::new() } else { format!(" ({input_note})") });
                }
                ServerMsg::Refused { reason } => return Err(format!("refused: {reason}")),
                ServerMsg::Bye { reason } => return Err(format!("bye: {reason}")),
                ServerMsg::Video { key, data, stream, parts, built_ns, encoded_ns, frame, .. } => {
                    // a picture is counted with its last part (tiles) or as itself (interleaved);
                    // its encode time is its slowest part's
                    bytes += data.len() as u64;
                    streams = streams.max(stream as u32 + 1);
                    let ms = encoded_ns.saturating_sub(built_ns) as f64 / 1e6;
                    slowest = if slowest.0 == frame { (frame, slowest.1.max(ms)) } else { (frame, ms) };
                    if parts <= 1 || stream + 1 == parts {
                        pictures += 1;
                        keys += key as u64;
                        encodes.push(slowest.1);
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
                _ => {}
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
    if flap.is_some() {
        println!("probe: {} pause/resume flips", flapped.2);
    }
    if input {
        println!("probe: pointer -> cursor round trip median {:.2} ms over {} moves", median(&mut latencies), latencies.len());
    }
    Ok(())
}
