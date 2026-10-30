//! The server's debug tools, each behind an env switch read once: a headset on a loop
//! (`MAKEPAD_XR_REMOTE_SELFTEST=1`), a dump of a packed picture (`MAKEPAD_XR_REMOTE_DUMP`) and
//! the live tuning file (a QP ceiling for sweeps).

use super::pipeline::{Core, Out, Picture};
use super::session::PumpStats;
use super::{Log, ServerShared};
use makepad_desk_link::xr::*;
use makepad_widgets::makepad_platform::makepad_hal::{self, xr_remote as hal};
use makepad_widgets::makepad_platform::video_file::{StreamVideoCodec, VideoStreamEncoder};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A headset on a loop (`MAKEPAD_XR_REMOTE_SELFTEST=1`): synthetic poses a display period apart (the head turning
/// slowly, eyes 64 mm apart, ±45° fields) drive the app's XR window exactly as a headset's would, and
/// every picture is encoded and measured, then dropped (and counted decoded at once). The server
/// side's numbers without a headset. It ends like a session when the server is switched off
/// (`stop`: its listener's). A headset that goes away, for tests: `MAKEPAD_XR_REMOTE_SELFTEST_POSES_S=n`
/// sends poses for n seconds and then none (linked, idle: taken off, asleep), and
/// `MAKEPAD_XR_REMOTE_SELFTEST_LINK_S=m` ends the session after m seconds (gone). Its eyes are
/// 1680x1760 px unless `MAKEPAD_XR_REMOTE_SELFTEST_EYE=<w>x<h>` says (a Quest 3: 2016x2112), its
/// display rate 72 Hz unless `MAKEPAD_XR_REMOTE_SELFTEST_HZ=<hz>` says.
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
    *shared.live_headset.lock().unwrap() = Some(super::LiveHeadset { instance: 0, name: "Selftest headset".into(), session: 0 });
    let secs = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok()).map(Duration::from_secs_f64);
    let (poses_for, link_for) = (secs("MAKEPAD_XR_REMOTE_SELFTEST_POSES_S"), secs("MAKEPAD_XR_REMOTE_SELFTEST_LINK_S"));
    let _ = std::thread::Builder::new().name("xr-selftest".into()).spawn(move || {
        let start = Instant::now();
        let mut seq = 0u64;
        let mut stats = PumpStats::new();
        let mut idle_told = false;
        while !stop.load(Ordering::Acquire) {
            if link_for.is_some_and(|d| start.elapsed() >= d) {
                log("xr selftest: the headset went away");
                break;
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
            let yaw = (t * 0.5).sin() * 0.6;
            let q = [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()];
            let right = [yaw.cos(), 0.0, -yaw.sin()];
            let at = |s: f32| XrPose { orientation: q, position: [right[0] * s * 0.032, 1.6, right[2] * s * 0.032] };
            let fov = [-0.785, 0.785, 0.785, -0.785];
            let frame = XrFrame { head: at(0.0), eyes: [XrEye { pose: at(-1.0), fov }, XrEye { pose: at(1.0), fov }], ..Default::default() };
            let mut wire = Vec::with_capacity(<XrFrame as Wire>::SIZE);
            frame.write(&mut wire);
            stats.poses += 1;
            if !core.pose(seq, makepad_hal::clock_ns() + 30_000_000, &wire) {
                stats.refused_poses += 1;
            }
            // drained as they come (a headset decodes on arrival), the next pose a display period on
            let next = Instant::now() + Duration::from_micros(1_000_000 / hz as u64);
            loop {
                while let Ok(m) = core.out.try_recv() {
                    let Out::Video(mut head, data, encoded_at) = m;
                    head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                    stats.sent(&head, data.len(), Duration::ZERO);
                    core.tile_decoded();
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn dump_picture(dir: &std::ffi::OsStr, p: &Picture, size: [u32; 2], log: &Log) {
    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::AcqRel) != 300 {
        return;
    }
    if p.ready.wait(Duration::from_secs(2)).is_err() {
        return log("xr host: dump: the picture's GPU work did not complete");
    }
    let [w, h] = size;
    let Some(px) = read_packed(p.surfaces[0].id, w, h) else {
        return log("xr host: dump: the picture could not be read");
    };
    // a BGRA texel holds luma samples 0..3 in its bytes as they lie: the row is the plane's row
    let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
    pgm.extend_from_slice(&px);
    let path = std::path::Path::new(dir).join("picture.pgm");
    let _ = std::fs::write(&path, &pgm);
    // per eye column: its colour's luma, its band's coverage (left half) and inverse depth (right half)
    let (ew, eh) = (w / 2, h * 2 / 3);
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
    let mut line = format!("xr host: dumped {} ({w}x{h})", path.display());
    for (e, name) in ["left", "right"].iter().enumerate() {
        let x = e as u32 * ew;
        let (l_mean, _, _) = area(x, x + ew, 0, eh);
        let (a_mean, a_max, a_cov) = area(x, x + ew / 2, eh, h);
        let (d_mean, d_max, d_cov) = area(x + ew / 2, x + ew, eh, h);
        let near = if d_max > 0 { DEPTH_NEAR_M * 255.0 / d_max as f32 } else { 0.0 };
        line += &format!("; {name} eye: luma mean {l_mean:.1}, coverage mean {a_mean:.1} max {a_max} drawn {a_cov:.0}%, inverse depth mean {d_mean:.1} max {d_max} (nearest {near:.2} m) drawn {d_cov:.0}%");
    }
    log(&line);
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
