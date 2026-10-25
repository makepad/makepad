//! stream_decode_check <in.h264|in.h265> [--cpu]: the platform's stream decoder (as the Desk Link
//! viewer drives it: GPU pictures, one access unit per push) over an Annex-B file, without a
//! window. Prints the pictures decoded, their size and chroma, whether the session runs on the
//! hardware decoder and the decode time per picture; exits 1 unless every picture decoded.
//! `.h265` / `.hevc` (or `--hevc`): H.265, else H.264.

fn main() {
    use makepad_video::annex_b;
    use makepad_video::{StreamVideoCodec, VideoStreamDecoder};
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: stream_decode_check <in.h264|in.h265> [--hevc] [--cpu]");
        std::process::exit(2);
    };
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(2)
    });
    let hevc = path.ends_with(".h265") || path.ends_with(".hevc") || args.iter().any(|a| a == "--hevc");
    let codec = if hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
    let gpu = !args.iter().any(|a| a == "--cpu");
    let mut decoder = (if gpu { VideoStreamDecoder::new_gpu(codec) } else { VideoStreamDecoder::new(codec) }).unwrap_or_else(|e| {
        eprintln!("decoder: {e:?}");
        std::process::exit(1)
    });
    let (mut unit, mut pictures, mut decoded, mut failed, mut full_chroma) = (Vec::new(), 0i64, 0i64, 0u32, None);
    let mut size = (0, 0);
    let started = std::time::Instant::now();
    for nal in annex_b::split_annex_b(&data) {
        unit.extend_from_slice(&[0, 0, 0, 1]);
        unit.extend_from_slice(nal);
        let vcl = if hevc { annex_b::hevc_nal_unit_type(nal) < 32 } else { matches!(annex_b::nal_unit_type(nal), 1 | 5) };
        if !vcl {
            continue;
        }
        match decoder.push_packet(&unit, pictures) {
            Ok(frames) => {
                for f in frames {
                    decoded += 1;
                    size = (f.width, f.height);
                    if let Some(g) = &f.gpu {
                        full_chroma = Some(g.full_chroma);
                    }
                }
            }
            Err(e) => {
                failed += 1;
                if failed <= 3 {
                    eprintln!("picture {pictures}: {e:?}");
                }
            }
        }
        pictures += 1;
        unit.clear();
    }
    if let Ok(frames) = decoder.flush() {
        decoded += frames.len() as i64;
    }
    println!(
        "{} {}: {pictures} pictures, {decoded} decoded, {failed} failed, {}x{}, chroma {}, {:.2} ms per picture, hardware decoder: {}",
        if hevc { "H.265" } else { "H.264" },
        path,
        size.0,
        size.1,
        match full_chroma {
            Some(true) => "4:4:4",
            Some(false) => "4:2:0",
            None => "(cpu nv12)",
        },
        started.elapsed().as_secs_f64() * 1000.0 / pictures.max(1) as f64,
        match decoder.hardware() {
            Some(true) => "yes",
            Some(false) => "NO (software)",
            None => "not reported",
        }
    );
    std::process::exit(if decoded > 0 && decoded == pictures { 0 } else { 1 });
}
