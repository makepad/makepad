//! IMU backend: the Valve lighthouse board inside a Bigscreen Beyond, read
//! through Linux hidraw.
//!
//! The Beyond's own HID (35bd:0101, interface 2) is its control channel
//! (brightness, fan, LED) and carries no motion data. The IMU is on the
//! Valve receiver board, which enumerates on the same hub as 28de:2300
//! ("Tundra Tracker", serial `LHR-…`). Its interface 0 streams input
//! report 0x20 by itself at 1 kHz (`firmware_config.imu_rate`), with no
//! enable command (measured on a Beyond 2, 2026-10-09):
//!
//! ```text
//! 0x20, then 3 × { i16 acc[3], i16 gyro[3], u32 ticks, u8 seq }   little-endian
//! ```
//!
//! The three samples are a round-robin window, so a report repeats up to
//! two samples already seen; `seq` tells them apart and `ticks` is the
//! board's 48 MHz clock. Feature 0x01 gives the full-scale ranges (gyro
//! ±250·2^g °/s, accel ±2·2^a g, /32768 per LSB; this headset reports
//! ±2000 °/s and ±8 g). Features 0x10, then 0x11 until an empty chunk,
//! give the zlib-compressed JSON config: the factory IMU bias and scale
//! and the IMU and head frames as `plus_x`/`plus_z` in the board's
//! tracking reference. Raw axes are the config's IMU frame (libsurvive's
//! `imu2trackref`/`head2imu`). Layout and units follow Monado
//! (`drivers/vive`) and libsurvive (`driver_vive.c`,
//! `survive_default_devices.c`). Only GET_FEATURE and input reads are
//! used; nothing is written to the device.
//!
//! This board stalls (EPIPE) about every other GET_FEATURE and answers the
//! next one, so each feature read is retried a few times.
use {
    super::{fusion::ImuSample, BackendKind, HeadsetImuSource, ImuBackend, Opened, Pumped},
    crate::stereo::calibration::{DisplayCalibration, EyeCalibration, LensModel, ValveRadial},
    makepad_widgets::Vec3f,
    std::{
        ffi::{c_int, c_short, c_ulong},
        fs::{self, File, OpenOptions},
        io::{ErrorKind, Read},
        os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
        path::{Path, PathBuf},
    },
};

pub const BACKEND: BackendKind = BackendKind { name: "valve-imu", open };

/// Board clock of the `ticks` field.
const TICKS_PER_SECOND: f64 = 48_000_000.0;
/// A step longer than this between samples is a gap (replug, stall); the
/// filter does not integrate across it.
const MAX_STEP: f32 = 0.1;
/// GET_FEATURE attempts per report. This board stalls about every other
/// request and has never stalled twice in a row (2026-10-09: 38 config
/// chunks, one stall each at most); a device that keeps stalling is
/// reported and opened again a second later.
const FEATURE_ATTEMPTS: usize = 8;
/// A compressed config this large is not a config (this headset's is
/// 2.3 KB; the stream ends with an empty chunk).
const CONFIG_MAX: usize = 1 << 20;

const POLLIN: c_short = 0x001;
const POLLERR: c_short = 0x008;
const POLLHUP: c_short = 0x010;
const POLLNVAL: c_short = 0x020;
const O_NONBLOCK: c_int = 0o4000;
const EINTR: i32 = 4;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

extern "C" {
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

/// `HIDIOCGFEATURE(len)`: `_IOC(_IOC_READ | _IOC_WRITE, 'H', 0x07, len)`.
fn hidiocgfeature(len: usize) -> c_ulong {
    (3 << 30) | ((len as c_ulong) << 16) | ((b'H' as c_ulong) << 8) | 0x07
}

fn open(source: &HeadsetImuSource) -> Opened {
    let path = match source {
        HeadsetImuSource::Path(path) => match identify(path) {
            Some(_) => path.clone(),
            None => return Opened::NotFound,
        },
        HeadsetImuSource::Display(vendor, product) if (*vendor, *product) != (*b"BIG", 0x1234) => {
            return Opened::NotFound;
        }
        HeadsetImuSource::Auto | HeadsetImuSource::Display(..) => match discover() {
            Some(path) => path,
            None => return Opened::NotFound,
        },
    };
    let file = match OpenOptions::new().read(true).write(true).custom_flags(O_NONBLOCK).open(&path) {
        Ok(file) => file,
        Err(error) => {
            return Opened::Unavailable(format!(
                "{}: {error} (hidraw is root-only unless a udev rule grants 28de:2300)",
                path.display()
            ))
        }
    };
    let serial = identify(&path).unwrap_or_default();
    match Calibration::read(&file) {
        Ok(calibration) => Opened::Ready(Box::new(ValveImu {
            path,
            serial,
            file,
            calibration,
            last_seq: None,
            last_ticks: 0,
        })),
        Err(error) => Opened::Unavailable(format!("{}: {error}", path.display())),
    }
}

// ----------------------------------------------------------------------
// Discovery
// ----------------------------------------------------------------------

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// The USB device directory (e.g. `.../3-5.1.3.3`) behind a hidraw node,
/// when that node is 28de:2300 interface 0; its serial.
fn usb_device(node: &Path) -> Option<(PathBuf, String)> {
    let name = node.file_name()?;
    let hid = fs::canonicalize(Path::new("/sys/class/hidraw").join(name).join("device")).ok()?;
    let iface = hid.parent()?;
    let device = iface.parent()?;
    let id = |f: &str| read_trimmed(&device.join(f));
    if id("idVendor").as_deref() != Some("28de")
        || id("idProduct").as_deref() != Some("2300")
        || read_trimmed(&iface.join("bInterfaceNumber")).as_deref() != Some("00")
    {
        return None;
    }
    Some((device.to_path_buf(), id("serial").unwrap_or_default()))
}

/// The serial of a hidraw node that is this board's IMU interface.
fn identify(node: &Path) -> Option<String> {
    usb_device(node).map(|(_, serial)| serial)
}

/// Ports shared by two USB device names (`3-5.1.3.3` and `3-5.1.1.3`
/// share 2: bus 3 port 5, then port 1).
fn shared_ports(a: &str, b: &str) -> usize {
    let split = |s: &str| s.split_once('-').map(|(bus, ports)| (bus.to_string(), ports.split('.').map(str::to_string).collect::<Vec<_>>()));
    match (split(a), split(b)) {
        (Some((ba, pa)), Some((bb, pb))) if ba == bb => pa.iter().zip(&pb).take_while(|(x, y)| x == y).count(),
        _ => 0,
    }
}

/// The hidraw of a 28de:2300 interface 0 sharing at least its root port
/// with a Bigscreen Beyond (35bd:0101); the closest wins. A lone Valve
/// tracker or Index is not taken.
fn discover() -> Option<PathBuf> {
    let beyonds: Vec<String> = fs::read_dir("/sys/bus/usb/devices")
        .ok()?
        .flatten()
        .filter(|e| {
            read_trimmed(&e.path().join("idVendor")).as_deref() == Some("35bd")
                && read_trimmed(&e.path().join("idProduct")).as_deref() == Some("0101")
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if beyonds.is_empty() {
        return None;
    }
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in fs::read_dir("/sys/class/hidraw").ok()?.flatten() {
        let node = Path::new("/dev").join(entry.file_name());
        let Some((device, _)) = usb_device(&node) else {
            continue;
        };
        let name = device.file_name()?.to_string_lossy().into_owned();
        let score = beyonds.iter().map(|b| shared_ports(&name, b)).max().unwrap_or(0);
        if score >= 1 && best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, node));
        }
    }
    best.map(|(_, node)| node)
}

// ----------------------------------------------------------------------
// Calibration and the IMU-to-head frame
// ----------------------------------------------------------------------

fn get_feature(file: &File, id: u8) -> Result<Vec<u8>, String> {
    let mut last = String::new();
    for _ in 0..FEATURE_ATTEMPTS {
        let mut buf = [0u8; 64];
        buf[0] = id;
        // SAFETY: the request names `buf`'s length; the kernel writes at
        // most that many bytes into it.
        let n = unsafe { ioctl(file.as_raw_fd(), hidiocgfeature(buf.len()), buf.as_mut_ptr()) };
        if n > 0 {
            return Ok(buf[..n as usize].to_vec());
        }
        last = std::io::Error::last_os_error().to_string();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Err(format!("feature 0x{id:02x}: {last}"))
}

#[derive(Clone, Debug)]
struct Calibration {
    /// rad/s and m/s² per LSB (full-scale range / 32768).
    gyro_lsb: f32,
    acc_lsb: f32,
    gyro_scale: Vec3f,
    gyro_bias: Vec3f,
    acc_scale: Vec3f,
    acc_bias: Vec3f,
    /// Rows of the IMU-to-head rotation.
    imu_to_head: [Vec3f; 3],
    /// Ranges and frame, for the status line.
    summary: String,
    /// The display part of the same factory config, when it has one.
    display: Option<DisplayCalibration>,
}

impl Calibration {
    fn read(file: &File) -> Result<Self, String> {
        let range = get_feature(file, 0x01)?;
        let (gyro_code, acc_code) = match range.as_slice() {
            [_, g, a, ..] if *g <= 4 && *a <= 4 => (*g, *a),
            _ => return Err(format!("range report {:02x?} not understood", &range[..range.len().min(3)])),
        };
        let gyro_range = std::f64::consts::PI / 180.0 * (250u32 << gyro_code) as f64;
        let acc_range = 9.806_65 * (2u32 << acc_code) as f64;
        let json = read_config(file)?;
        let config = Config::parse(&json)?;
        let display = display_calibration(&json);
        Ok(Self {
            gyro_lsb: (gyro_range / 32768.0) as f32,
            acc_lsb: (acc_range / 32768.0) as f32,
            gyro_scale: config.gyro_scale,
            gyro_bias: config.gyro_bias,
            acc_scale: config.acc_scale,
            acc_bias: config.acc_bias,
            imu_to_head: config.imu_to_head,
            summary: format!(
                "±{} °/s ±{} g, {} {} frame, display optics {}",
                250u32 << gyro_code,
                2u32 << acc_code,
                config.manufacturer,
                config.model,
                if display.is_some() { "from the factory config" } else { "absent" }
            ),
            display,
        })
    }

    /// One raw sample to the head frame (rad/s, m/s²), the factory bias
    /// removed (Monado's order: scale, then bias).
    fn convert(&self, gyro: [i16; 3], acc: [i16; 3]) -> (Vec3f, Vec3f) {
        let g = Vec3f {
            x: gyro[0] as f32 * self.gyro_lsb * self.gyro_scale.x - self.gyro_bias.x,
            y: gyro[1] as f32 * self.gyro_lsb * self.gyro_scale.y - self.gyro_bias.y,
            z: gyro[2] as f32 * self.gyro_lsb * self.gyro_scale.z - self.gyro_bias.z,
        };
        let a = Vec3f {
            x: acc[0] as f32 * self.acc_lsb * self.acc_scale.x - self.acc_bias.x,
            y: acc[1] as f32 * self.acc_lsb * self.acc_scale.y - self.acc_bias.y,
            z: acc[2] as f32 * self.acc_lsb * self.acc_scale.z - self.acc_bias.z,
        };
        let m = &self.imu_to_head;
        let map = |v: Vec3f| Vec3f { x: m[0].dot(v), y: m[1].dot(v), z: m[2].dot(v) };
        (map(g), map(a))
    }
}

fn read_config(file: &File) -> Result<Vec<u8>, String> {
    get_feature(file, 0x10)?;
    let mut z = Vec::new();
    loop {
        let chunk = get_feature(file, 0x11)?;
        let len = chunk.get(1).copied().unwrap_or(0) as usize;
        if len > 62 || 2 + len > chunk.len() {
            return Err(format!("config chunk length {len} invalid"));
        }
        if len == 0 {
            break;
        }
        z.extend_from_slice(&chunk[2..2 + len]);
        if z.len() > CONFIG_MAX {
            return Err("config stream does not end".into());
        }
    }
    makepad_fast_inflate::zlib_decompress_vec(&z).map_err(|e| format!("config inflate: {e:?}"))
}

struct Config {
    manufacturer: String,
    model: String,
    gyro_scale: Vec3f,
    gyro_bias: Vec3f,
    acc_scale: Vec3f,
    acc_bias: Vec3f,
    imu_to_head: [Vec3f; 3],
}

impl Config {
    /// `imu.{acc,gyro}_{bias,scale}` (absent: no correction) and the `imu`
    /// and `head` frames (required: without them the axes are unknown).
    fn parse(json: &[u8]) -> Result<Self, String> {
        use makepad_strict_json::{parse_depth, Value};
        let root = parse_depth(json, 16).map_err(|e| format!("config json: {e}"))?;
        fn vec3(v: Option<&Value>) -> Option<Vec3f> {
            let num = |v: &Value| match v {
                Value::F64(f) => Some(*f as f32),
                Value::Int(i) => Some(*i as f32),
                _ => None,
            };
            match v?.as_arr()? {
                [x, y, z] => Some(Vec3f { x: num(x)?, y: num(y)?, z: num(z)? }),
                _ => None,
            }
        }
        let frame = |key: &str| {
            let obj = root.get(key)?;
            frame_from_plus_x_z(vec3(obj.get("plus_x"))?, vec3(obj.get("plus_z"))?)
        };
        let (Some(imu_frame), Some(head_frame)) = (frame("imu"), frame("head")) else {
            return Err("config has no imu/head frame".into());
        };
        // Both map their local axes into the tracking reference; IMU to
        // head is headᵀ · imu.
        let col = |m: &[Vec3f; 3], i: usize| match i {
            0 => Vec3f { x: m[0].x, y: m[1].x, z: m[2].x },
            1 => Vec3f { x: m[0].y, y: m[1].y, z: m[2].y },
            _ => Vec3f { x: m[0].z, y: m[1].z, z: m[2].z },
        };
        let mut imu_to_head = [Vec3f::default(); 3];
        for (r, row) in imu_to_head.iter_mut().enumerate() {
            let h = col(&head_frame, r);
            *row = Vec3f { x: h.dot(col(&imu_frame, 0)), y: h.dot(col(&imu_frame, 1)), z: h.dot(col(&imu_frame, 2)) };
        }
        let imu = root.get("imu");
        let field = |key: &str, default: Vec3f| vec3(imu.and_then(|i| i.get(key))).unwrap_or(default);
        let ones = Vec3f { x: 1.0, y: 1.0, z: 1.0 };
        let text = |key: &str| root.get(key).and_then(Value::as_str).unwrap_or("?").to_string();
        Ok(Self {
            manufacturer: text("manufacturer"),
            model: text("model_number"),
            gyro_scale: field("gyro_scale", ones),
            gyro_bias: field("gyro_bias", Vec3f::default()),
            acc_scale: field("acc_scale", ones),
            acc_bias: field("acc_bias", Vec3f::default()),
            imu_to_head,
        })
    }
}

/// Rows of the rotation whose columns are +x, +y = +z × +x and +z (Monado's
/// `math_quat_from_plus_x_z`), re-orthonormalised.
fn frame_from_plus_x_z(plus_x: Vec3f, plus_z: Vec3f) -> Option<[Vec3f; 3]> {
    let x = plus_x.normalize();
    let y = Vec3f::cross(plus_z, x).normalize();
    let z = Vec3f::cross(x, y);
    if x.length_squared() < 0.5 || y.length_squared() < 0.5 {
        return None;
    }
    Some([Vec3f { x: x.x, y: y.x, z: z.x }, Vec3f { x: x.y, y: y.y, z: z.y }, Vec3f { x: x.z, y: y.z, z: z.z }])
}

// ----------------------------------------------------------------------
// The report stream
// ----------------------------------------------------------------------

struct ValveImu {
    path: PathBuf,
    serial: String,
    file: File,
    calibration: Calibration,
    last_seq: Option<u8>,
    last_ticks: u32,
}

#[derive(Clone, Copy)]
struct RawSample {
    acc: [i16; 3],
    gyro: [i16; 3],
    ticks: u32,
    seq: u8,
}

impl ValveImu {
    /// The new samples of one 0x20 report, oldest first.
    fn report(&mut self, report: &[u8], out: &mut dyn FnMut(ImuSample)) {
        if report.len() < 52 || report[0] != 0x20 {
            return;
        }
        let mut s = [RawSample { acc: [0; 3], gyro: [0; 3], ticks: 0, seq: 0 }; 3];
        for (i, sample) in s.iter_mut().enumerate() {
            let b = &report[1 + 17 * i..1 + 17 * (i + 1)];
            let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
            sample.acc = [i16_at(0), i16_at(2), i16_at(4)];
            sample.gyro = [i16_at(6), i16_at(8), i16_at(10)];
            sample.ticks = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
            sample.seq = b[16];
        }
        // The window is round-robin: order by age behind the newest seq.
        let newest = s.iter().map(|x| x.seq).fold(s[0].seq, |m, q| if q.wrapping_sub(m) < 128 { q } else { m });
        s.sort_by_key(|x| std::cmp::Reverse(newest.wrapping_sub(x.seq)));
        for sample in s {
            let dt = match self.last_seq {
                None => 0.0,
                Some(last) => {
                    let ahead = sample.seq.wrapping_sub(last);
                    if ahead == 0 || ahead >= 128 {
                        continue;
                    }
                    let dt = (sample.ticks.wrapping_sub(self.last_ticks) as f64 / TICKS_PER_SECOND) as f32;
                    if dt > MAX_STEP { 0.0 } else { dt }
                }
            };
            self.last_seq = Some(sample.seq);
            self.last_ticks = sample.ticks;
            let (gyro, accel) = self.calibration.convert(sample.gyro, sample.acc);
            out(ImuSample { gyro, accel, dt });
        }
    }
}

impl ImuBackend for ValveImu {
    fn name(&self) -> &'static str {
        BACKEND.name
    }

    fn describe(&self) -> String {
        format!("{} ({}): {}", self.path.display(), self.serial, self.calibration.summary)
    }

    fn display_calibration(&self) -> Option<DisplayCalibration> {
        self.calibration.display.clone()
    }

    fn pump(&mut self, timeout_ms: i32, out: &mut dyn FnMut(ImuSample)) -> Result<Pumped, String> {
        let mut pfd = PollFd { fd: self.file.as_raw_fd(), events: POLLIN, revents: 0 };
        // SAFETY: one valid pollfd for the duration of the call.
        let ready = unsafe { poll(&mut pfd, 1, timeout_ms) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(EINTR) {
                return Ok(Pumped::Idle);
            }
            return Err(format!("poll: {error}"));
        }
        if pfd.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            return Err("device gone".into());
        }
        if ready == 0 {
            return Ok(Pumped::Idle);
        }
        let mut got = false;
        loop {
            let mut buf = [0u8; 64];
            let n = match (&self.file).read(&mut buf) {
                Ok(0) => return Err("device gone".into()),
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("read: {e}")),
            };
            got = true;
            self.report(&buf[..n], out);
        }
        Ok(if got { Pumped::Data } else { Pumped::Idle })
    }
}

// ----------------------------------------------------------------------
// The display part of the factory config
// ----------------------------------------------------------------------

/// The headset's display optics from the same lighthouse config: field
/// layout and the field-of-view derivation as Monado reads them
/// (src/xrt/auxiliary/vive/vive_config.c, main ec188bb1:
/// `_get_distortion_properties`, `_calculate_fov`, `ipd.default_mm` as the
/// lens separation of fixed-IPD devices), the distortion `type` and the
/// normalisation as Valve documents them (OpenVR wiki, "The JSON File
/// (Lighthouse Devices)"). `None` when the config has no
/// `tracking_to_eye_transform` for both eyes: the view then stays
/// uncalibrated.
fn display_calibration(json: &[u8]) -> Option<DisplayCalibration> {
    use makepad_strict_json::{parse_depth, Value};
    let root = parse_depth(json, 16).ok()?;
    let num = |v: Option<&Value>| match v? {
        Value::F64(f) => Some(*f as f32),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    };
    let device = root.get("device");
    let width = num(device.and_then(|d| d.get("eye_target_width_in_pixels")))?;
    let height = num(device.and_then(|d| d.get("eye_target_height_in_pixels")))?;
    // Valve normalises both lens axes by half the panel width, so the y
    // scale is the panel's width over height: `physical_aspect_x_over_y`
    // when given, else the eye target's pixel ratio (square pixels). This
    // headset's config has none and 2544x2544 eyes, so 1.0. (Monado's
    // fallback, 0.9, is the original Vive's 1080x1200 panel.)
    let aspect = num(device.and_then(|d| d.get("physical_aspect_x_over_y"))).unwrap_or(width / height);
    let matrix = |v: Option<&Value>| -> Option<[[f32; 3]; 3]> {
        let rows = v?.as_arr()?;
        let mut m = [[0.0; 3]; 3];
        for (r, row) in rows.get(..3)?.iter().enumerate() {
            let row = row.as_arr()?;
            for (c, x) in row.get(..3)?.iter().enumerate() {
                m[r][c] = num(Some(x))?;
            }
        }
        Some(m)
    };
    let transforms = root.get("tracking_to_eye_transform")?.as_arr()?;
    let mut eyes = Vec::with_capacity(2);
    for eye in transforms.get(..2)? {
        let eye_to_head = matrix(eye.get("eye_to_head"))?;
        let intrinsics = matrix(eye.get("intrinsics"))?;
        // Monado's default before the config is read.
        let grow = num(eye.get("grow_for_undistort")).unwrap_or(0.5);
        // `_calculate_fov`: intrinsics divided by (1 + grow), then pixel
        // focus and centre, then the four edge tangents.
        let scale = 1.0 + grow;
        let focus_x = intrinsics[0][0] / scale * width / 2.0;
        let focus_y = intrinsics[1][1] / scale * height / 2.0;
        let center_x = (intrinsics[0][2] / scale - 1.0) * -1.0 * width / 2.0;
        let center_y = (intrinsics[1][2] / scale + 1.0) * height / 2.0;
        if !(focus_x > 0.0 && focus_y > 0.0) {
            return None;
        }
        let fov_tan = [
            -center_x / focus_x,
            (width - center_x) / focus_x,
            center_y / focus_y,
            -(height - center_y) / focus_y,
        ];
        // R, G, B as `distortion_red`, `distortion`, `distortion_blue`; a
        // missing channel stays zero (no correction), as in Monado. A type
        // Valve does not document (or that this code does not know) leaves
        // the eye uncorrected rather than guessing its form.
        let mut radial = [ValveRadial::Poly3; 3];
        let mut center = [[0.0f32; 2]; 3];
        let mut k = [[0.0f32; 3]; 3];
        let mut known = true;
        for (c, name) in ["distortion_red", "distortion", "distortion_blue"].into_iter().enumerate() {
            let Some(d) = eye.get(name) else { continue };
            radial[c] = match d.get("type").and_then(Value::as_str) {
                Some("DISTORT_POLY3") => ValveRadial::Poly3,
                Some("DISTORT_DPOLY3") => ValveRadial::DPoly3,
                _ => {
                    known = false;
                    ValveRadial::Poly3
                }
            };
            center[c] = [num(d.get("center_x")).unwrap_or(0.0), num(d.get("center_y")).unwrap_or(0.0)];
            if let Some(coeffs) = d.get("coeffs").and_then(Value::as_arr) {
                for (i, x) in coeffs.iter().take(3).enumerate() {
                    k[c][i] = num(Some(x)).unwrap_or(0.0);
                }
            }
        }
        let lens = if known {
            LensModel::ValveRadial { radial, center, k, grow_for_undistort: grow, aspect_x_over_y: aspect }
        } else {
            LensModel::Identity
        };
        eyes.push(EyeCalibration { eye_to_head, fov_tan, lens });
    }
    let [left, right]: [EyeCalibration; 2] = eyes.try_into().ok()?;
    // `lens_separation` (original Vive) or, for fixed-IPD devices such as
    // the Beyond, `ipd.default_mm`.
    let lens_separation = num(root.get("lens_separation"))
        .or_else(|| num(root.get("ipd").and_then(|i| i.get("default_mm"))).map(|mm| mm / 1000.0));
    let text = |key: &str| root.get(key).and_then(Value::as_str).unwrap_or("?").to_string();
    let edid_vendor = root
        .get("direct_mode_edid_vid")
        .and_then(Value::as_str)
        .and_then(|v| <[u8; 3]>::try_from(v.as_bytes()).ok());
    Some(DisplayCalibration {
        vsync_to_photons: num(root.get("seconds_from_vsync_to_photons"))
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(f64::from),
        source: format!("{} {} {} factory config", text("manufacturer"), text("model_number"), text("device_serial_number")),
        edid_vendor,
        lens_separation,
        eyes: [left, right],
    })
}
