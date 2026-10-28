//! stream_bench <in.yuv> <W>x<H> <out> [--444] [--hevc] [--kbps K] [--fps N] [--quality L] [--cbr] [--fast]
//! [--buffer PICTURES] [--frames N] [--engine E]: the Linux hardware stream encoder (Vulkan Video, as Desk
//! Link drives it) on raw pictures from a file (NV12, or NV24 with `--444`: ffmpeg's `-pix_fmt
//! nv12/nv24`), without a window. Writes the Annex-B stream to <out> (decode it and compare it
//! with the source elsewhere, e.g. ffmpeg's psnr/ssim) and prints the rate it took at `--fps`,
//! the biggest picture and the encode time per picture. Compare codecs, quality levels and rate
//! control on the same pictures.

#[cfg(target_os = "linux")]
fn main() {
    use makepad_video::{StreamTile, StreamTuning, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
    use std::io::{Read, Write};
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: stream_bench <in.yuv> <W>x<H> <out> [--444] [--hevc] [--kbps K] [--fps N] [--quality L] [--cbr] [--fast] [--buffer PICTURES] [--frames N] [--engine E]");
        std::process::exit(2);
    }
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.windows(2).find(|w| w[0] == name).and_then(|w| w[1].parse::<u32>().ok());
    let (w, h) = args[2].split_once('x').map(|(a, b)| (a.parse::<u32>().unwrap(), b.parse::<u32>().unwrap())).expect("size WxH");
    let full_chroma = flag("--444");
    let fps = value("--fps").unwrap_or(120);
    let kbps = value("--kbps").unwrap_or(0);
    let tuning = StreamTuning { quality: value("--quality").unwrap_or(0), constant_rate: flag("--cbr"), buffer_pictures: value("--buffer").unwrap_or(2), sharp: !flag("--fast") };
    let codec = if flag("--hevc") { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
    if !makepad_hal::stream_encode::hal_stream_encoder_offscreen_device() {
        eprintln!("no Vulkan device");
        std::process::exit(1);
    }
    let options = VideoStreamEncoderOptions { codec, width: w, height: h, fps, bitrate_kbps: kbps, keyint: 0, low_latency: true, full_chroma };
    let mut encoder = VideoStreamEncoder::new_tuned(options, StreamTile { engine: value("--engine").unwrap_or(0), x0: 0, y0: 0, source_height: h }, tuning).unwrap_or_else(|e| {
        eprintln!("open: {e:?}");
        std::process::exit(1);
    });
    println!("{}", encoder.description());
    let picture = if full_chroma { w as usize * h as usize * 3 } else { w as usize * h as usize * 3 / 2 };
    let mut input = std::fs::File::open(&args[1]).expect("open input");
    let mut out = std::fs::File::create(&args[3]).expect("create output");
    let frames = value("--frames").unwrap_or(u32::MAX);
    let mut buf = vec![0u8; picture];
    let mut gpu_us: Vec<u32> = Vec::new();
    let (mut n, mut total, mut first, mut biggest, mut encode_ns) = (0u32, 0u64, 0usize, 0usize, 0u64);
    while n < frames && input.read_exact(&mut buf).is_ok() {
        let started = std::time::Instant::now();
        let packets = encoder.push_frame_nv12(&buf, n as i64).unwrap_or_else(|e| {
            eprintln!("picture {n}: {e:?}");
            std::process::exit(1);
        });
        encode_ns += started.elapsed().as_nanos() as u64;
        gpu_us.push(encoder.timings().encode_us);
        let bytes: usize = packets.iter().map(|p| p.data.len()).sum();
        for p in &packets {
            out.write_all(&p.data).unwrap();
        }
        if n == 0 {
            first = bytes;
        } else {
            biggest = biggest.max(bytes);
            total += bytes as u64;
        }
        n += 1;
    }
    let p = (n.max(2) - 1) as f64;
    let mut p_us: Vec<u32> = gpu_us.iter().skip(1).copied().collect();
    p_us.sort();
    let median_ms = p_us.get(p_us.len() / 2).map_or(0.0, |&u| u as f64 / 1000.0);
    let max_ms = p_us.last().map_or(0.0, |&u| u as f64 / 1000.0);
    println!(
        "{n} pictures: key {:.0} KB, then {:.1} Mbit/s at {fps} fps (mean {:.0} KB, biggest {:.0} KB), {:.2} ms a picture (CPU upload included); on the encoder (P pictures) median {:.2} ms, max {:.2} ms: {:.0} pictures/s on one engine",
        first as f64 / 1e3,
        total as f64 * 8.0 / p * fps as f64 / 1e6,
        total as f64 / p / 1e3,
        biggest as f64 / 1e3,
        encode_ns as f64 / n.max(1) as f64 / 1e6,
        median_ms,
        max_ms,
        if median_ms > 0.0 { 1000.0 / median_ms } else { 0.0 }
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("stream_bench: the Linux hardware stream encoder only");
}
