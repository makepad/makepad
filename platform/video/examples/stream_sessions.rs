//! stream_sessions [<W>x<H>] [--hevc] [--max N]: how many hardware stream encoders (Vulkan Video, as Desk Link
//! opens them) this machine holds at once. Opens encoders one after another, each encoding one picture, and keeps
//! them all open until one fails or `--max` (default 64) is reached; prints the count and the first failure. Every
//! encoder is closed on exit. Run it while no stream is live: the encoders it holds count against the GPU's limit.

#[cfg(target_os = "linux")]
fn main() {
    use makepad_video::{StreamTile, StreamTuning, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.windows(2).find(|w| w[0] == name).and_then(|w| w[1].parse::<u32>().ok());
    let (w, h) = args.get(1).and_then(|s| s.split_once('x')).and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))).unwrap_or((1920, 1088));
    let max = value("--max").unwrap_or(64);
    let codec = if flag("--hevc") { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
    if !makepad_hal::stream_encode::hal_stream_encoder_offscreen_device() {
        eprintln!("no Vulkan device");
        std::process::exit(1);
    }
    let picture = vec![128u8; w as usize * h as usize * 3 / 2];
    let tuning = StreamTuning { quality: 0, constant_rate: false, buffer_pictures: 2, sharp: true };
    let mut open = Vec::new();
    let mut failure = String::new();
    while (open.len() as u32) < max {
        let options = VideoStreamEncoderOptions { codec, width: w, height: h, fps: 90, bitrate_kbps: 0, keyint: 0, low_latency: true, full_chroma: false };
        match VideoStreamEncoder::new_tuned(options, StreamTile { engine: (open.len() % 2) as u32, x0: 0, y0: 0, source_height: h }, tuning) {
            Ok(mut encoder) => match encoder.push_frame_nv12(&picture, 0) {
                Ok(_) => open.push(encoder),
                Err(e) => {
                    failure = format!("encode on encoder {}: {e:?}", open.len() + 1);
                    break;
                }
            },
            Err(e) => {
                failure = format!("open of encoder {}: {e:?}", open.len() + 1);
                break;
            }
        }
    }
    println!("stream_sessions: {} encoders at {w}x{h} {} open at once{}", open.len(), if codec == StreamVideoCodec::Hevc { "HEVC" } else { "H.264" }, if failure.is_empty() { format!(" (--max {max} reached)") } else { format!("; then {failure}") });
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("stream_sessions: Linux only (Vulkan Video)");
}
