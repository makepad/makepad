//! The server's debug tools, each behind an env switch read once: a headset on a loop
//! (`MAKEPAD_XR_REMOTE_SELFTEST=1`), a dump of a packed picture (`MAKEPAD_XR_REMOTE_DUMP`) and
//! the live tuning file (a QP ceiling for sweeps).

use super::pipeline::{Core, Out, Picture};
use super::session::PumpStats;
use super::{Log, ServerShared};
use makepad_desk_link::xr::*;
use makepad_widgets::makepad_platform::makepad_hal::{self, xr_remote as hal};
use makepad_widgets::makepad_platform::video_file::{StreamVideoCodec, VideoStreamEncoder};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A headset on a loop (`MAKEPAD_XR_REMOTE_SELFTEST=1`): synthetic poses a display period apart (the head turning
/// slowly, eyes 64 mm apart, ±45° fields) drive the app's XR window exactly as a headset's would, and
/// every picture is encoded and measured, then dropped (and counted decoded a link trip later). The server
/// side's numbers without a headset. It ends like a session when the server is switched off
/// (`stop`: its listener's). A headset that goes away, for tests: `MAKEPAD_XR_REMOTE_SELFTEST_POSES_S=n`
/// sends poses for n seconds and then none (linked, idle: taken off, asleep), and
/// `MAKEPAD_XR_REMOTE_SELFTEST_LINK_S=m` ends the session after m seconds (gone). Its eyes are
/// 1680x1760 px unless `MAKEPAD_XR_REMOTE_SELFTEST_EYE=<w>x<h>` says (a Quest 3: 2016x2112), its
/// display rate 72 Hz unless `MAKEPAD_XR_REMOTE_SELFTEST_HZ=<hz>` says. Its fields of view are
/// ±45° unless `MAKEPAD_XR_REMOTE_SELFTEST_FOV=<left>,<right>,<up>,<down>` (the left eye's
/// tangent angles, radians; the right eye mirrored) says (a Quest 3: `-0.94,0.72,0.84,-0.96`),
/// `MAKEPAD_XR_REMOTE_SELFTEST_STILL=1` keeps the head from turning (pictures to compare) and
/// `MAKEPAD_XR_REMOTE_SELFTEST_PITCH=<radians>` tilts it (negative: looking down). While the file
/// `MAKEPAD_XR_REMOTE_SELFTEST_PAUSE_FILE=<path>` names exists, the headset has paused the stream
/// (as its menu button does) and drops the pictures that still reach it (never reported); it
/// resumes the stream when the file goes. Words in the file: `sleep`, its poses stop meanwhile
/// (power save), and an id of `STREAM_AA_MODES`, the anti-aliasing the stream takes meanwhile
/// (as Auto steps it while a headset is paused). Its tracking space, for tests: when the file
/// `MAKEPAD_XR_REMOTE_SELFTEST_SPACE_FILE=<path>` names appears, the headset takes its words and
/// removes it: `head=<x>,<y>,<z>,<yaw>` puts the head there in the tracking space from then on
/// (turned `yaw` radians about y; it starts at 1.6 m over the origin), `session` links it again
/// (a session of its own) and `recenter` is its wearer recentering, each a new tracking space.
pub(crate) fn selftest(shared: Arc<ServerShared>, eye: [u32; 2], stop: Arc<AtomicBool>) {
    let log = shared.log.clone();
    use makepad_hal::{buffers::Wire, XrEye, XrFrame, XrPose};
    let codec = if VideoStreamEncoder::caps().hevc { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
    let hz: u32 = std::env::var("MAKEPAD_XR_REMOTE_SELFTEST_HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(72).clamp(30, 120);
    let core = match Core::start(shared.clone(), eye, hz, codec) {
        Ok(c) => c,
        Err(e) => return log(&format!("xr selftest: {e}")),
    };
    log(&format!("xr selftest: {}x{} {codec:?} in {} tile(s) on {}", core.size[0], core.size[1], core.tiles, core.description));
    hal::hal_xr_remote_enable();
    hal::hal_xr_remote_attach(eye[0], eye[1], hz * 1000);
    shared.new_space("the headset linked");
    *shared.live_headset.lock().unwrap() = Some(super::LiveHeadset { instance: 0, name: "Selftest headset".into(), session: 0 });
    let secs = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok()).map(Duration::from_secs_f64);
    let (poses_for, link_for) = (secs("MAKEPAD_XR_REMOTE_SELFTEST_POSES_S"), secs("MAKEPAD_XR_REMOTE_SELFTEST_LINK_S"));
    let fov = std::env::var("MAKEPAD_XR_REMOTE_SELFTEST_FOV").ok().and_then(|v| {
        let a: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (a.len() == 4).then(|| [a[0], a[1], a[2], a[3]])
    });
    let pause_file = std::env::var_os("MAKEPAD_XR_REMOTE_SELFTEST_PAUSE_FILE").map(std::path::PathBuf::from);
    let space_file = std::env::var_os("MAKEPAD_XR_REMOTE_SELFTEST_SPACE_FILE").map(std::path::PathBuf::from);
    let fov_l = fov.unwrap_or([-0.785, 0.785, 0.785, -0.785]);
    let fov_r = [-fov_l[1], -fov_l[0], fov_l[2], fov_l[3]];
    let still = std::env::var("MAKEPAD_XR_REMOTE_SELFTEST_STILL").as_deref() == Ok("1");
    let pitch: f32 = std::env::var("MAKEPAD_XR_REMOTE_SELFTEST_PITCH").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    if fov.is_some() || still || pitch != 0.0 {
        log(&format!("xr selftest: left eye fov {fov_l:?}, right {fov_r:?}{}, pitch {pitch}", if still { ", the head still" } else { "" }));
    }
    let _ = std::thread::Builder::new().name("xr-selftest".into()).spawn(move || {
        let start = Instant::now();
        let mut seq = 0u64;
        // MAKEPAD_XR_REMOTE_SELFTEST_DECODE=1 (macOS): the encoded stream decoded here, the 300th
        // picture's numbers beside the dump's (what the encode and decode did to them)
        #[cfg(target_os = "macos")]
        let mut decoded = (std::env::var("MAKEPAD_XR_REMOTE_SELFTEST_DECODE").as_deref() == Ok("1")).then(Decoded::default);
        let mut stats = PumpStats::new();
        let mut idle_told = false;
        let (mut paused, mut asleep) = (false, false);
        // where the head stands in the tracking space, and its turn about y (`SPACE_FILE`)
        let (mut head_at, mut head_yaw) = ([0.0f32, 1.6, 0.0], 0.0f32);
        // the tiles that reached the headset and are not reported yet: it reports a link trip
        // after they arrived (decoded meanwhile), and drops them while it has paused the stream
        let mut arrived = std::collections::VecDeque::<(u64, Instant)>::new();
        const TRIP: Duration = Duration::from_millis(20);
        while !stop.load(Ordering::Acquire) {
            if let Some(file) = &pause_file {
                if file.exists() != paused {
                    paused = !paused;
                    let says = if paused { std::fs::read_to_string(file).unwrap_or_default() } else { String::new() };
                    asleep = says.split_whitespace().any(|w| w == "sleep");
                    log(if !paused { "xr selftest: the headset resumed the stream" } else if asleep { "xr selftest: the headset paused the stream and sleeps (no poses)" } else { "xr selftest: the headset paused the stream" });
                    shared.set_streaming(!paused);
                    core.tiles_out(stats.forget_unreported());
                    if let Some(i) = says.split_whitespace().find_map(|w| crate::settings_types::STREAM_AA_MODES.iter().position(|m| m.id == w)) {
                        shared.aa.store(i as u8, Ordering::Release);
                        log(&format!("xr selftest: anti-aliasing {} while paused", crate::settings_types::STREAM_AA_MODES[i].name));
                    }
                }
            }
            // its tracking space (tests): the file's words once it appears, and it goes
            let says = space_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok().inspect(|_| {
                let _ = std::fs::remove_file(f);
            }));
            if let Some(says) = says {
                let words: Vec<&str> = says.split_whitespace().collect();
                for w in &words {
                    let a: Vec<f32> = w.strip_prefix("head=").map_or(Vec::new(), |v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect());
                    if a.len() == 4 {
                        (head_at, head_yaw) = ([a[0], a[1], a[2]], a[3]);
                        log(&format!("xr selftest: the head at ({:.3}, {:.3}, {:.3}) turned {:.3}", a[0], a[1], a[2], a[3]));
                    }
                }
                if words.contains(&"session") {
                    shared.new_space("the headset linked again");
                }
                if words.contains(&"recenter") {
                    shared.new_space("the wearer recentered");
                }
            }
            if link_for.is_some_and(|d| start.elapsed() >= d) {
                log("xr selftest: the headset went away");
                break;
            }
            if asleep {
                // no poses; what still reaches it is dropped, never reported
                while let Ok(Out::Video(head, data, _)) = core.out.try_recv() {
                    core.tiles_out(stats.sent(&head, data.len(), Duration::ZERO));
                }
                arrived.clear();
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if poses_for.is_some_and(|d| start.elapsed() >= d) {
                // linked and idle: no poses (the app is woken once, as by a session's loop)
                if !idle_told {
                    idle_told = true;
                    log("xr selftest: the headset went idle (no poses)");
                    std::thread::sleep(Duration::from_millis(super::FLOWING_NS / 1_000_000 + 10));
                    shared.wake();
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            seq += 1;
            let t = start.elapsed().as_secs_f32();
            let yaw = head_yaw + if still { 0.0 } else { (t * 0.5).sin() * 0.6 };
            // the turn about y after the tilt about x
            let (sy, cy, sx, cx) = ((yaw * 0.5).sin(), (yaw * 0.5).cos(), (pitch * 0.5).sin(), (pitch * 0.5).cos());
            let q = [cy * sx, sy * cx, -sy * sx, cy * cx];
            let right = [yaw.cos(), 0.0, -yaw.sin()];
            let at = |s: f32| XrPose { orientation: q, position: [head_at[0] + right[0] * s * 0.032, head_at[1], head_at[2] + right[2] * s * 0.032] };
            let frame = XrFrame { head: at(0.0), eyes: [XrEye { pose: at(-1.0), fov: fov_l }, XrEye { pose: at(1.0), fov: fov_r }], ..Default::default() };
            let mut wire = Vec::with_capacity(<XrFrame as Wire>::SIZE);
            frame.write(&mut wire);
            stats.poses += 1;
            if !core.pose(seq, makepad_hal::clock_ns(), makepad_hal::clock_ns() + 30_000_000, &wire) {
                stats.refused_poses += 1;
            }
            // drained as they come, the next pose a display period on
            let next = Instant::now() + Duration::from_micros(1_000_000 / hz as u64);
            loop {
                while let Ok(m) = core.out.try_recv() {
                    let Out::Video(mut head, data, encoded_at) = m;
                    #[cfg(target_os = "macos")]
                    if let Some(d) = decoded.as_mut() {
                        d.take(&head, &data, &log);
                    }
                    head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                    core.tiles_out(stats.sent(&head, data.len(), Duration::ZERO));
                    arrived.push_back((head.seq, Instant::now()));
                }
                if paused {
                    arrived.clear();
                }
                while arrived.front().is_some_and(|(_, at)| at.elapsed() >= TRIP) {
                    let (s, _) = arrived.pop_front().unwrap();
                    core.tiles_out(stats.reported(s, 0));
                }
                let now = Instant::now();
                if now >= next {
                    break;
                }
                std::thread::sleep((next - now).min(Duration::from_millis(1)));
            }
            stats.window(&core);
            // a headset's report once a second (synthetic: a stats panel shows the selftest)
            if seq % hz as u64 == 0 {
                let mut v = vec![0.0f32; report::COUNT];
                let k = (seq / hz as u64) as f32;
                v[report::DISPLAY] = hz as f32;
                v[report::SHOWN] = hz as f32 - (k % 7.0 == 0.0) as u8 as f32 * 3.0;
                v[report::REPEATS] = (k % 7.0 == 0.0) as u8 as f32 * 3.0;
                v[report::BLANK] = (k % 13.0 == 0.0) as u8 as f32;
                v[report::M2P_MS] = 70.0 + (k * 0.7).sin() * 20.0;
                v[report::BATTERY_PCT] = 80.0;
                let codes: Vec<u8> = (0..hz).map(|i| match (i + seq as u32 / hz) % 37 { 0 => report::FRAME_REPEAT, 9 => report::FRAME_LATE, 21 => report::FRAME_SKIP, _ => report::FRAME_NEW }).collect();
                report::pack_frames(&mut v, &codes);
                shared.push_report(v);
            }
        }
        // switched off: ended like a session (its encoders go with the core)
        *shared.live_headset.lock().unwrap() = None;
        shared.reports.lock().unwrap().clear();
        core.stop();
        shared.wake();
        log("xr selftest: ended");
    });
}

/// `MAKEPAD_XR_REMOTE_DUMP=<dir>`: the 300th picture's planes as `<dir>/picture.pgm` (its luma:
/// both eye columns, each its colour over its band of coverage and inverse depth) once its GPU
/// work completed, and a line of each eye's numbers. A check of what the GPU packed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn dump_picture(_dir: &std::ffi::OsStr, _p: &Picture, _size: [u32; 2], _log: &Log) {}

/// The display time of the picture the dump measured: the selftest's decoder measures that one.
#[cfg(any(target_os = "linux", target_os = "macos"))]
static DUMPED_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn dump_picture(dir: &std::ffi::OsStr, p: &Picture, size: [u32; 2], log: &Log) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, Ordering::AcqRel) != 300 {
        return;
    }
    if p.ready.wait(Duration::from_secs(2)).is_err() {
        return log("xr host: dump: the picture's GPU work did not complete");
    }
    DUMPED_NS.store(p.display_ns, Ordering::Release);
    let [w, h] = size;
    let Some(px) = read_packed(p.surfaces[0].id, w, h * 3 / 2) else {
        return log("xr host: dump: the picture could not be read");
    };
    // a BGRA texel holds luma samples 0..3 in its bytes as they lie: the row is the plane's row
    let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
    pgm.extend_from_slice(&px[..(w * h) as usize]);
    let path = std::path::Path::new(dir).join("picture.pgm");
    let _ = std::fs::write(&path, &pgm);
    let line = format!("xr host: dumped {} ({w}x{h}){}", path.display(), picture_numbers(&px, w, h, 2));
    log(&line);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// Per eye of a packed picture (`w` bytes a row: `h` luma rows, then the chroma rows, `eyes` eye columns of it): its
/// colour's luma and chroma, its band's coverage and inverse depth, as numbers.
fn picture_numbers(px: &[u8], w: u32, h: u32, eyes: usize) -> String {
    // per eye column: its colour's luma, its band's coverage (left half) and inverse depth (right half)
    let (ew, eh) = (w / eyes as u32, h * 2 / 3);
    let area = |x0: u32, x1: u32, y0: u32, y1: u32| {
        let (mut n, mut sum, mut max, mut nonzero) = (0u64, 0u64, 0u8, 0u64);
        for y in y0..y1 {
            for x in x0..x1 {
                let v = px[(y * w + x) as usize];
                n += 1;
                sum += v as u64;
                max = max.max(v);
                nonzero += (v > 0) as u64;
            }
        }
        (sum as f64 / n.max(1) as f64, max, nonzero as f64 * 100.0 / n.max(1) as f64)
    };
    let mut line = String::new();
    for (e, name) in ["left", "right"].iter().take(eyes).enumerate() {
        let x = e as u32 * ew;
        let (l_mean, _, _) = area(x, x + ew, 0, eh);
        let (a_mean, a_max, a_cov) = area(x, x + ew / 2, eh, h);
        // the band's coverage samples about half (a see-through surface): their share
        let half = (eh..h).flat_map(|y| (x..x + ew / 2).map(move |x| (x, y))).filter(|(x, y)| (96..160).contains(&px[(y * w + x) as usize])).count() as f64 * 100.0 / (((h - eh) * (ew / 2)) as f64).max(1.0);
        let (d_mean, d_max, d_cov) = area(x + ew / 2, x + ew, eh, h);
        let near = if d_max > 0 { DEPTH_NEAR_M * 255.0 / d_max as f32 } else { 0.0 };
        // the colour's chroma (rows h.., U V pairs): the samples off neutral, and the largest
        let mut off = 0u64;
        let mut dev = 0u8;
        for y in h..h + eh / 2 {
            for x in x..x + ew {
                let d = px[(y * w + x) as usize].abs_diff(128);
                off += (d > 16) as u64;
                dev = dev.max(d);
            }
        }
        line += &format!("; {name} eye chroma: {:.1}% of samples off neutral by 16+, largest {dev}", off as f64 * 100.0 / ((eh / 2 * ew) as f64).max(1.0));
        line += &format!("; {name} eye: luma mean {l_mean:.1}, coverage mean {a_mean:.1} max {a_max} drawn {a_cov:.0}% half {half:.1}%, inverse depth mean {d_mean:.1} max {d_max} (nearest {near:.2} m) drawn {d_cov:.0}%");
    }
    line
}

/// The packed picture's bytes (`w / 4` BGRA texels a row, `h` rows), tight. Linux: a DMA-BUF.
#[cfg(target_os = "linux")]
fn read_packed(id: u64, w: u32, h: u32) -> Option<Vec<u8>> {
    let mut px = vec![0u8; (w / 4 * h * 4) as usize];
    (makepad_hal::dmabuf::hal_dmabuf_read(id, &[0, 0, w / 4, h], &mut px) >= 0).then_some(px)
}

/// The packed picture's bytes (`w / 4` BGRA texels a row, `h` rows), tight. macOS: an IOSurface
/// (its id), read on the CPU under its lock.
#[cfg(target_os = "macos")]
fn read_packed(id: u64, w: u32, h: u32) -> Option<Vec<u8>> {
    use std::ffi::c_void;
    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceLookup(id: u32) -> *mut c_void;
        fn IOSurfaceLock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceUnlock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
        fn IOSurfaceGetBaseAddress(surface: *mut c_void) -> *mut c_void;
        fn IOSurfaceGetBytesPerRow(surface: *mut c_void) -> usize;
        fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    const READ_ONLY: u32 = 1;
    let (row, rows) = ((w / 4 * 4) as usize, h as usize);
    unsafe {
        let surface = IOSurfaceLookup(u32::try_from(id).ok()?);
        if surface.is_null() {
            return None;
        }
        let mut out = None;
        if IOSurfaceLock(surface, READ_ONLY, std::ptr::null_mut()) == 0 {
            let (base, stride) = (IOSurfaceGetBaseAddress(surface) as *const u8, IOSurfaceGetBytesPerRow(surface));
            if !base.is_null() && stride >= row && IOSurfaceGetHeight(surface) >= rows {
                let mut px = Vec::with_capacity(row * rows);
                for y in 0..rows {
                    px.extend_from_slice(std::slice::from_raw_parts(base.add(y * stride), row));
                }
                out = Some(px);
            }
            IOSurfaceUnlock(surface, READ_ONLY, std::ptr::null_mut());
        }
        CFRelease(surface as *const c_void);
        out
    }
}

/// The stream's live tuning (`$TMPDIR/xr-remote-tune`, line `maxqp=N`; read once a second by a
/// thread of the session's): a QP ceiling (every encoder of the process). For sweeps on a running server;
/// absent: none. (The bitrate is the app's setting, `set_bitrate_kbps`.)
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct Tune {
    pub(crate) max_qp: u32,
}

pub(crate) fn tune() -> Tune {
    let Ok(text) = std::fs::read_to_string(std::env::temp_dir().join("xr-remote-tune")) else { return Tune::default() };
    let mut t = Tune::default();
    for line in text.lines() {
        match line.trim().split_once('=') {
            Some(("maxqp", v)) => t.max_qp = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    t
}

/// The selftest's own decoder (`MAKEPAD_XR_REMOTE_SELFTEST_DECODE=1`).
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Decoded {
    decoder: Option<makepad_widgets::makepad_platform::video_file::VideoStreamDecoder>,
    measured: bool,
}

#[cfg(target_os = "macos")]
impl Decoded {
    fn take(&mut self, head: &VideoHead, data: &[u8], log: &Log) {
        let codec = if head.codec == CODEC_HEVC { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
        if self.decoder.is_none() {
            match makepad_widgets::makepad_platform::video_file::VideoStreamDecoder::new(codec) {
                Ok(d) => self.decoder = Some(d),
                Err(e) => return log(&format!("xr selftest: no decoder: {e:?}")),
            }
        }
        let Some(decoder) = self.decoder.as_mut() else { return };
        let frames = match decoder.push_packet(data, head.seq as i64 * 10) {
            Ok(f) => f,
            Err(e) => return log(&format!("xr selftest: decode: {e:?}")),
        };
        // the picture the dump measured (the first decoded at or after its display time)
        let dumped = DUMPED_NS.load(Ordering::Acquire);
        for f in frames {
            if !self.measured && dumped != 0 && head.display_ns >= dumped {
                self.measured = true;
                log(&format!("xr selftest: decoded ({}x{}){}", f.width, f.height, picture_numbers(&f.nv12, f.width, f.height, ((f.width * 2 / head.size[0].max(1)) as usize).clamp(1, 2))));
            }
        }
    }
}
