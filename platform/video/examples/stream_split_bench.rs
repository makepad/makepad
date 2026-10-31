//! stream_split_bench [<W>x<H>] [--cols C] [--rows R] [--frames N] [--fps F] [--kbps K] [--h264] [--sharp]:
//! a `W`x`H` picture (default 4032x3168: an XR stream picture of two 2016x2112 eyes and their band)
//! cut into `C` x `R` tiles, each a stream of its own on its own thread (engine `i % 2`), encoded
//! in lockstep a picture at a time at `F` pictures/s (default 90), then each tile's stream decoded
//! the same way (a decoder and a thread per tile). Prints per tile and per picture (the slowest
//! tile: when the picture is whole) the median and p90 encode and decode ms, the pictures/s the
//! encode kept up with, and the rate. Synthetic moving content; macOS (VideoToolbox) and Linux
//! (Vulkan Video encode, GStreamer decode). Compare cuts and settings on one machine.

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    use makepad_video::{StreamTile, StreamTuning, StreamVideoCodec, VideoStreamDecoder, VideoStreamEncoder, VideoStreamEncoderOptions};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.windows(2).find(|w| w[0] == name).and_then(|w| w[1].parse::<u32>().ok());
    let (w, h) = args.get(1).and_then(|s| s.split_once('x')).and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))).unwrap_or((4032, 3168));
    let (cols, rows) = (value("--cols").unwrap_or(1).max(1), value("--rows").unwrap_or(1).max(1));
    let frames = value("--frames").unwrap_or(300).max(10);
    let fps = value("--fps").unwrap_or(90).max(1);
    let kbps = value("--kbps").unwrap_or(60_000);
    let codec = if flag("--h264") { StreamVideoCodec::H264 } else { StreamVideoCodec::Hevc };
    let sharp = flag("--sharp");
    #[cfg(target_os = "linux")]
    if !makepad_hal::stream_encode::hal_stream_encoder_offscreen_device() {
        eprintln!("no Vulkan device");
        std::process::exit(1);
    }
    let (tw, th) = ((w / cols) & !15, (h / rows) & !15);
    let tiles = (cols * rows) as usize;
    // a few distinct moving pictures per tile, cycled (made before the clock runs)
    const CYCLE: u32 = 12;
    let picture = |tile: usize, t: u32| -> Vec<u8> {
        let (x0, y0) = ((tile as u32 % cols) * tw, (tile as u32 / cols) * th);
        let mut nv12 = vec![0u8; (tw * th * 3 / 2) as usize];
        for y in 0..th {
            for x in 0..tw {
                let (gx, gy) = (x0 + x, y0 + y);
                let m = gx.wrapping_add(t * 7) ^ gy.wrapping_add(t * 3);
                let n = (gx.wrapping_mul(2654435761) ^ gy.wrapping_mul(40503) ^ (t * 97)) >> 28;
                nv12[(y * tw + x) as usize] = (((gx + gy + t * 5) & 255) / 2 + (m & 63) + n) as u8;
            }
        }
        let c = (tw * th) as usize;
        for y in 0..th / 2 {
            for x in 0..tw / 2 {
                let (gx, gy) = (x0 / 2 + x, y0 / 2 + y);
                nv12[c + (y * tw + x * 2) as usize] = (96 + ((gx + t * 2) & 63)) as u8;
                nv12[c + (y * tw + x * 2 + 1) as usize] = (96 + ((gy + t) & 63)) as u8;
            }
        }
        nv12
    };
    let pictures: Vec<Arc<Vec<Vec<u8>>>> = (0..tiles).map(|i| Arc::new((0..CYCLE).map(|t| picture(i, t)).collect())).collect();
    let period = Duration::from_nanos(1_000_000_000 / fps as u64);
    let tuning = StreamTuning { quality: 0, constant_rate: true, buffer_pictures: 1, sharp };

    // encode: a thread per tile, a picture at a time on one clock
    let barrier = Arc::new(Barrier::new(tiles));
    let start = Instant::now() + Duration::from_millis(200);
    let mut handles = Vec::new();
    for i in 0..tiles {
        let (barrier, pictures) = (barrier.clone(), pictures[i].clone());
        handles.push(std::thread::spawn(move || {
            let options = VideoStreamEncoderOptions { codec, width: tw, height: th, fps, bitrate_kbps: (kbps / tiles as u32).max(1), keyint: 0, low_latency: true, full_chroma: false };
            let mut encoder = VideoStreamEncoder::new_tuned(options, StreamTile { engine: (i % 2) as u32, x0: 0, y0: 0, source_height: th }, tuning).expect("encoder");
            let description = encoder.description();
            let (mut times, mut units, mut bytes) = (Vec::new(), Vec::new(), 0u64);
            for n in 0..frames {
                barrier.wait();
                let due = start + period * n;
                let now = Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
                let packets = encoder.push_frame_nv12(&pictures[(n % CYCLE) as usize], n as i64 * 1000).expect("encode");
                let t = encoder.timings();
                times.push((t.encode_us, Instant::now()));
                let unit: Vec<u8> = packets.into_iter().flat_map(|p| p.data).collect();
                bytes += unit.len() as u64;
                units.push(unit);
            }
            (description, times, units, bytes)
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let pct = |v: &mut Vec<u32>, p: f64| {
        v.sort();
        v.get(((v.len() as f64 - 1.0) * p) as usize).map_or(0.0, |u| *u as f64 / 1000.0)
    };
    println!("stream_split_bench: {w}x{h} as {cols}x{rows} tiles of {tw}x{th} {codec:?} at {fps}/s, {frames} pictures{}; {}", if sharp { " (sharp)" } else { "" }, results[0].0);
    let skip = 5usize;
    let mut picture_enc: Vec<u32> = vec![0; frames as usize];
    for (i, (_, times, _, bytes)) in results.iter().enumerate() {
        let mut v: Vec<u32> = times.iter().skip(skip).map(|t| t.0).collect();
        println!("  tile {i}: encode median {:.2} ms, p90 {:.2} ms, {:.1} Mbit/s", pct(&mut v.clone(), 0.5), pct(&mut v, 0.9), *bytes as f64 * 8.0 / (frames as f64 / fps as f64) / 1e6);
        for (n, t) in times.iter().enumerate() {
            picture_enc[n] = picture_enc[n].max(t.0);
        }
    }
    // the pictures done: when the last tile of each finished
    let done: Vec<Instant> = (0..frames as usize).map(|n| results.iter().map(|r| r.1[n].1).max().unwrap()).collect();
    let span = done[frames as usize - 1].duration_since(done[skip]).as_secs_f64();
    let mut pe: Vec<u32> = picture_enc.iter().skip(skip).copied().collect();
    println!("  picture (slowest tile): encode median {:.2} ms, p90 {:.2} ms; kept up with {:.1} pictures/s of {fps}", pct(&mut pe.clone(), 0.5), pct(&mut pe, 0.9), (frames as usize - 1 - skip) as f64 / span);

    // decode: a decoder and a thread per tile, the same clock
    let barrier = Arc::new(Barrier::new(tiles));
    let start = Instant::now() + Duration::from_millis(200);
    let mut handles = Vec::new();
    for (i, (_, _, units, _)) in results.into_iter().enumerate() {
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            let mut decoder = VideoStreamDecoder::with_size_hint_gpu(codec, [tw, th]).expect("decoder");
            let mut times = Vec::new();
            let mut out = 0u32;
            for (n, unit) in units.iter().enumerate() {
                barrier.wait();
                let due = start + period * n as u32;
                let now = Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
                let t0 = Instant::now();
                let frames = decoder.push_packet(unit, n as i64 * 1000).unwrap_or_else(|e| panic!("tile {i}: decode: {e:?}"));
                out += frames.len() as u32;
                times.push(t0.elapsed().as_micros() as u32);
            }
            (times, out)
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let mut picture_dec: Vec<u32> = vec![0; frames as usize];
    for (i, (times, out)) in results.iter().enumerate() {
        let mut v: Vec<u32> = times.iter().skip(skip).copied().collect();
        println!("  tile {i}: decode median {:.2} ms, p90 {:.2} ms ({out} pictures out)", pct(&mut v.clone(), 0.5), pct(&mut v, 0.9));
        for (n, t) in times.iter().enumerate() {
            picture_dec[n] = picture_dec[n].max(*t);
        }
    }
    let mut pd: Vec<u32> = picture_dec.iter().skip(skip).copied().collect();
    println!("  picture (slowest tile): decode median {:.2} ms, p90 {:.2} ms", pct(&mut pd.clone(), 0.5), pct(&mut pd, 0.9));
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("stream_split_bench: macOS and Linux");
}
