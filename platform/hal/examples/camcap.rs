//! Windowless capture test: `camcap <name substring> [frames] [WxH]`. Lists the cameras, picks one by
//! name (the deck's rule: case-insensitive substring, else the first), opens it, grabs frames, prints
//! sizes and timing (`camcap OBSBOT 30 1920x1080 5` repeats open/stream/release 5 times, the slide
//! being left and entered again), releases it and checks the device is no longer held by this process.
#[cfg(target_os = "linux")]
fn main() {
    use makepad_hal::backend::linux::media::v4l2_camera::Camera;
    use makepad_hal::media::VideoPixel;
    let args: Vec<String> = std::env::args().collect();
    let wanted = args.get(1).cloned().unwrap_or_default().to_lowercase();
    let count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let (tw, th) = args
        .get(3)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
        .unwrap_or((1920, 1080));
    let mut cam = Camera::default();
    let list = cam.enumerate();
    for d in &list {
        println!("camera {:?}: {} formats", d.name, d.formats.len());
    }
    let Some(dev) = list.iter().find(|d| d.name.to_lowercase().contains(&wanted)).or(list.first()) else {
        println!("no camera");
        std::process::exit(2);
    };
    let best = |mjpeg: bool| {
        dev.formats
            .iter()
            .filter(|f| (f.pixel == VideoPixel::MJPEG) == mjpeg && matches!(f.pixel, VideoPixel::MJPEG | VideoPixel::YUY2))
            .min_by_key(|f| {
                (f.width as i64 - tw as i64).abs() + (f.height as i64 - th as i64).abs() + (30.0 - f.frame_rate.unwrap_or(30.0)).abs() as i64 * 1000
            })
    };
    let Some(format) = best(true).or(best(false)) else {
        println!("{:?}: no usable format", dev.name);
        std::process::exit(2);
    };
    println!("picked {:?} at {}x{} {:?} {:?} fps", dev.name, format.width, format.height, format.pixel, format.frame_rate);
    let rounds: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
    for round in 1..=rounds {
        println!("--- round {round}/{rounds}");
        let key = (dev.id, format.id);
        let t0 = std::time::Instant::now();
        cam.use_inputs(&[key]);
        println!("open+stream took {:?}", t0.elapsed());
        let mut got = 0;
        let mut last = std::time::Instant::now();
        let mut gaps = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while got < count && std::time::Instant::now() < deadline {
            for (_, bytes) in cam.poll() {
                got += 1;
                gaps.push(last.elapsed().as_secs_f64() * 1000.0);
                last = std::time::Instant::now();
                if got <= 3 || got == count {
                    println!("frame {got}: {} bytes, head {:02x?}", bytes.len(), &bytes[..bytes.len().min(4)]);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        let total = t0.elapsed().as_secs_f64();
        println!("{got} frames in {total:.2} s ({:.1} fps)", got as f64 / total);
        if !gaps.is_empty() {
            gaps.sort_by(|a, b| a.total_cmp(b));
            println!("gap ms: median {:.1} max {:.1}", gaps[gaps.len() / 2], gaps[gaps.len() - 1]);
        }
        let held = |tag: &str| {
            let n = std::fs::read_dir("/proc/self/fd")
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|e| std::fs::read_link(e.path()).ok())
                .filter(|p| p.to_string_lossy().starts_with("/dev/video"))
                .count();
            println!("{tag}: {n} /dev/video fds held by this process");
        };
        held("streaming");
        cam.use_inputs(&[]);
        held("after release");
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {}
