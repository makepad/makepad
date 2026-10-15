//! XREAL One Pro (runtime USB 3318:0436), over its USB Ethernet link.
//!
//! Protocol references: Skarian/one-xr's XrControlProtocol and report parser,
//! SamiMitwalli/One-Pro-IMU-Retriever-Demo, and rohitsangwan01/xreal_one_driver.
//! The framing, sensor flags, nanosecond clock and mode readback were also
//! checked on a One Pro. This is a native implementation, with no vendor SDK.
//!
//! Opening the IMU socket starts the stream. Only a selected headset display
//! enters SBS; a preview reads the IMU without changing the glasses' mode.
//! Set the glasses to Follow with stabilization off; the WM supplies the
//! head tracking (the scene-mode RPC only enables/disables device buttons).
//! All discovery, TCP I/O, parsing and mode restoration run on the IMU worker.

use super::{fusion::ImuSample, BackendKind, HeadsetImuSource, ImuBackend, Opened, Pumped};
use crate::stereo::{calibration::{DisplayCalibration, EyeCalibration, LensModel}, XREAL_ONE_PRO_EDID};
use makepad_widgets::{log, Vec3f};
use std::{
    fs,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

pub const BACKEND: BackendKind = BackendKind { name: "XREAL One Pro", open };
const ADDRESS: Ipv4Addr = Ipv4Addr::new(169, 254, 2, 1);
const IMU_PORT: u16 = 52998;
const CONTROL_PORT: u16 = 52999;
const GET_EDID: u16 = 0x275e;
const GET_INPUT_MODE: u16 = 0x2821;
const SET_INPUT_MODE: u16 = 0x2822;
const FULL_SBS: u32 = 1;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
// The wire format is a six-byte big-endian envelope and 128-byte LE carrier.
const REPORT_BYTES: usize = 134;
// Across a gap as long as the tracker's stale-pose threshold, do not assume
// the newest rate describes the missing motion. Rebase the device clock.
const SAMPLE_GAP_NS: u64 = 100_000_000;

fn present() -> bool {
    let Ok(devices) = fs::read_dir("/sys/bus/usb/devices") else { return false; };
    devices.flatten().any(|device| {
        let p = device.path();
        fs::read_to_string(p.join("idVendor")).is_ok_and(|v| v.trim() == "3318")
            && fs::read_to_string(p.join("idProduct")).is_ok_and(|v| v.trim() == "0436")
    })
}

fn connect(port: u16) -> io::Result<TcpStream> {
    let socket = TcpStream::connect_timeout(&SocketAddr::from((ADDRESS, port)), CONTROL_TIMEOUT)?;
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    socket.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    Ok(socket)
}

fn open(source: &HeadsetImuSource) -> Opened {
    let stereo = match source {
        HeadsetImuSource::Auto => false,
        HeadsetImuSource::Display(vendor, product) if (*vendor, *product) == XREAL_ONE_PRO_EDID => true,
        _ => return Opened::NotFound,
    };
    if !present() { return Opened::NotFound; }
    match XrealImu::open(stereo) {
        Ok(imu) => Opened::Ready(Box::new(imu)),
        Err(error) => Opened::Unavailable(format!("{ADDRESS}: {error}; USB Ethernet/TCP-IP must be enabled")),
    }
}

struct XrealImu {
    socket: TcpStream,
    control: Option<StereoMode>,
    pending: Vec<u8>,
    last_device_time: Option<u64>,
    samples: u64,
    other_reports: u64,
    invalid_reports: u64,
    started: Instant,
    rate_logged: bool,
}

impl XrealImu {
    fn open(stereo: bool) -> Result<Self, String> {
        // Configure before connecting the stream: a mode switch can briefly
        // interrupt the display and the USB network on some firmware.
        let control = if stereo { Some(StereoMode::enter()?) } else { None };
        let socket = connect(IMU_PORT).map_err(|e| format!("IMU connect: {e}"))?;
        Ok(Self {
            socket, control, pending: Vec::new(), last_device_time: None,
            samples: 0, other_reports: 0, invalid_reports: 0,
            started: Instant::now(), rate_logged: false,
        })
    }

    fn decode(&mut self, frame: &[u8], out: &mut dyn FnMut(ImuSample)) {
        let flags = u32::from_le_bytes(frame[30..34].try_into().unwrap());
        // Bits 0/1 indicate gyro/accel. In particular 0x04 is a magnetic
        // report whose gyro/accel slots can be NaN; it is not head motion.
        if flags & 3 != 3 {
            self.other_reports += 1;
            return;
        }
        let values: [f32; 6] = std::array::from_fn(|i| {
            let offset = 34 + i * 4;
            f32::from_le_bytes(frame[offset..offset + 4].try_into().unwrap())
        });
        if !values.iter().all(|v| v.is_finite()) {
            self.invalid_reports += 1;
            return;
        }
        let timestamp = u64::from_le_bytes(frame[14..22].try_into().unwrap());
        if self.last_device_time == Some(timestamp) { return; }
        let delta = self.last_device_time.and_then(|last| timestamp.checked_sub(last));
        let dt = delta.filter(|ns| *ns <= SAMPLE_GAP_NS).map_or(0.0, |ns| ns as f32 * 1e-9);
        if self.last_device_time.is_some() && dt == 0.0 {
            log!("headset imu: XREAL clock gap/reset; holding orientation across the gap");
        }
        self.last_device_time = Some(timestamp);
        let gyro = head_frame([values[0], values[1], values[2]]);
        let accel = head_frame([values[3], values[4], values[5]]);
        out(ImuSample { gyro, accel, dt });
        self.samples += 1;
        makepad_widgets::makepad_platform::trace!("wm.imu", "xreal sample={} dt_ms={:.3} gyro={:?} accel={:?}", self.samples, dt * 1000.0, gyro, accel);
    }
}

impl ImuBackend for XrealImu {
    fn name(&self) -> &'static str { "XREAL One Pro" }

    fn describe(&self) -> String {
        format!("{ADDRESS}:{IMU_PORT}, gyro rad/s, accel m/s², device clock ns; {}",
            if self.control.is_some() { "stereo input verified (full SBS)" } else { "tracking preview; display mode unchanged" })
    }

    fn pump(&mut self, timeout_ms: i32, out: &mut dyn FnMut(ImuSample)) -> Result<Pumped, String> {
        self.socket.set_read_timeout(Some(Duration::from_millis(timeout_ms.max(1) as u64)))
            .map_err(|e| e.to_string())?;
        // This is a read chunk, not a backlog limit. Every complete report
        // is consumed; at most one partial report remains for the next read.
        let mut chunk = [0u8; 4096];
        let n = match self.socket.read(&mut chunk) {
            Ok(0) => return Err("IMU stream closed".into()),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {
                return if present() { Ok(Pumped::Idle) } else { Err("USB device disconnected".into()) };
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(Pumped::Idle),
            Err(e) => return Err(format!("IMU read: {e}")),
        };
        self.pending.extend_from_slice(&chunk[..n]);
        let mut offset = 0;
        while self.pending.len() - offset >= 6 {
            let header = &self.pending[offset..offset + 6];
            if !matches!(header[0], 0x27 | 0x28) || header[1..] != [0x36, 0, 0, 0, 128] {
                offset += 1;
                self.invalid_reports += 1;
                continue;
            }
            if self.pending.len() - offset < REPORT_BYTES { break; }
            let mut frame = [0u8; REPORT_BYTES];
            frame.copy_from_slice(&self.pending[offset..offset + REPORT_BYTES]);
            self.decode(&frame, out);
            offset += REPORT_BYTES;
        }
        self.pending.drain(..offset);
        let elapsed = self.started.elapsed().as_secs_f64();
        if !self.rate_logged && elapsed >= 2.0 && self.samples > 0 {
            self.rate_logged = true;
            log!("headset imu: XREAL {:.0} motion samples/s, {} other sensor reports, {} invalid reports/bytes; nominal One Pro mount rotation",
                self.samples as f64 / elapsed, self.other_reports, self.invalid_reports);
        }
        Ok(Pumped::Data)
    }

    fn display_calibration(&self) -> Option<DisplayCalibration> {
        self.control.as_ref()?;
        // Nominal panel geometry, not factory calibration. One Pro has a
        // 57-degree diagonal FOV and one 1920x1080 panel per eye. Its optics
        // do not use the Beyond's radial or chromatic pre-distortion.
        let aspect = 16.0f32 / 9.0;
        let y = (57.0f32.to_radians() * 0.5).tan() / (1.0 + aspect * aspect).sqrt();
        let x = y * aspect;
        let eye = EyeCalibration {
            eye_to_head: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            fov_tan: [-x, x, y, -y], lens: LensModel::Identity,
        };
        Some(DisplayCalibration {
            source: "XREAL One Pro: nominal 57° diagonal FOV, identity lens (not factory calibration)".into(),
            edid_vendor: Some(XREAL_ONE_PRO_EDID.0), lens_separation: None,
            vsync_to_photons: None, eyes: [eye.clone(), eye],
        })
    }
}

/// Carrier axes -> head axes (+x right, +y up, -z forward). The One Pro's
/// factory display-to-IMU quaternion is about -35 degrees around +x;
/// display calibration uses optical axes (+y down, +z forward). Invert
/// that mount rotation and flip optical y/z into our head frame. Use the
/// nominal angle (also in XRLinuxDriver's One Pro profile), not one unit's
/// calibration constants. An upright unit reads roughly (0,-8,+5.6) m/s²
/// on the carrier: it must become +y, not +z. Apply the same proper rotation
/// to gyro and accel; the firmware already applies sensor calibration.
fn head_frame([x, y, z]: [f32; 3]) -> Vec3f {
    const SIN: f32 = 0.57357645;
    const COS: f32 = 0.81915206;
    Vec3f { x, y: -COS * y + SIN * z, z: -SIN * y - COS * z }
}

/// Keep the original mode for a normal VR exit. A disconnect is harmless;
/// a person changing the mode themselves is respected on restoration.
struct StereoMode { control: Control, previous_mode: u32 }

impl StereoMode {
    fn enter() -> Result<Self, String> {
        let mut control = Control { socket: connect(CONTROL_PORT).map_err(|e| format!("display control: {e}"))?, sequence: 0 };
        let previous_mode = control.get(GET_INPUT_MODE)?;
        let previous_edid = control.get(GET_EDID)?;
        let mut mode = Self { control, previous_mode };
        if previous_mode != FULL_SBS { mode.control.set(SET_INPUT_MODE, FULL_SBS)?; }
        let input = mode.control.get(GET_INPUT_MODE)?;
        let edid = mode.control.get(GET_EDID)?;
        if input != FULL_SBS { return Err(format!("stereo request not applied (input mode {input}, EDID {edid})")); }
        // Firmware 1.8.1 chooses EDID 0 here (3840x1080@60). Do not force a
        // guessed EDID index: mode tables differ between One-family models.
        log!("headset imu: XREAL stereo input={input}, EDID={edid} (was input={previous_mode}, EDID={previous_edid}); waiting for full-SBS display timing");
        Ok(mode)
    }
}

impl Drop for StereoMode {
    fn drop(&mut self) {
        if self.previous_mode == FULL_SBS { return; }
        let restore = (|| {
            if self.control.get(GET_INPUT_MODE)? != FULL_SBS { return Ok(()); }
            self.control.set(SET_INPUT_MODE, self.previous_mode)?;
            // The firmware chooses the mode's EDID asynchronously. During
            // the transition GET_EDID can still return a temporary zero;
            // forcing an EDID then is rejected (5001 on firmware 1.9.1).
            log!("headset imu: XREAL restored input mode {}; firmware selects its display timing", self.previous_mode);
            Ok::<(), String>(())
        })();
        if let Err(e) = restore { log!("headset imu: XREAL mode restore unavailable: {e}"); }
    }
}

struct Control { socket: TcpStream, sequence: u32 }

impl Control {
    fn get(&mut self, command: u16) -> Result<u32, String> { self.request(command, &[0x18, 0]) }

    fn set(&mut self, command: u16, value: u32) -> Result<(), String> {
        let mut integer = Vec::new();
        let mut left = value;
        while left >= 128 { integer.push((left as u8 & 127) | 128); left >>= 7; }
        integer.push(left as u8);
        let mut body = vec![0x1a, (integer.len() + 1) as u8, 0x08];
        body.extend_from_slice(&integer);
        let status = self.request(command, &body)?;
        if status != 0 { return Err(format!("command {command:#06x} rejected: {status}")); }
        Ok(())
    }

    fn request(&mut self, command: u16, payload: &[u8]) -> Result<u32, String> {
        self.sequence = (self.sequence.wrapping_add(1) & 0x7fff_ffff).max(1);
        let mut frame = Vec::with_capacity(10 + payload.len());
        frame.extend_from_slice(&command.to_be_bytes());
        frame.extend_from_slice(&((4 + payload.len()) as u32).to_be_bytes());
        frame.extend_from_slice(&(self.sequence | 0x8000_0000).to_be_bytes());
        frame.extend_from_slice(payload);
        self.socket.write_all(&frame).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        loop {
            let mut header = [0; 6];
            self.read_exact(&mut header, deadline)?;
            let magic = u16::from_be_bytes(header[..2].try_into().unwrap());
            let length = u32::from_be_bytes(header[2..].try_into().unwrap()) as usize;
            // Read in chunks so unsolicited messages never allocate according
            // to their advertised size. Keep only the matching RPC payload.
            let mut chunk = [0; 1024];
            let mut reply = Vec::new();
            let mut remaining = length;
            while remaining > 0 {
                let n = remaining.min(chunk.len());
                self.read_exact(&mut chunk[..n], deadline)?;
                if magic == command { reply.extend_from_slice(&chunk[..n]); }
                remaining -= n;
            }
            if magic != command { continue; }
            if reply.len() < 4 { return Err("short control transaction".into()); }
            let sequence = u32::from_be_bytes(reply[..4].try_into().unwrap()) & 0x7fff_ffff;
            if sequence != self.sequence { continue; }
            return property(&reply[4..]);
        }
    }

    fn read_exact(&mut self, mut bytes: &mut [u8], deadline: Instant) -> Result<(), String> {
        while !bytes.is_empty() {
            let time = deadline.checked_duration_since(Instant::now()).ok_or("control response timed out")?;
            self.socket.set_read_timeout(Some(time)).map_err(|e| e.to_string())?;
            match self.socket.read(bytes) {
                Ok(0) => return Err("control connection closed".into()),
                Ok(n) => bytes = &mut bytes[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {},
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
}

fn varint(bytes: &mut &[u8]) -> Result<u32, String> {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or("short property varint")?;
        *bytes = rest;
        if shift == 28 && byte & 0xf0 != 0 { return Err("property integer overflow".into()); }
        value |= u32::from(byte & 127) << shift;
        if byte & 128 == 0 { return Ok(value); }
    }
    Err("invalid property varint".into())
}

fn property(mut bytes: &[u8]) -> Result<u32, String> {
    if varint(&mut bytes)? != 0x22 { return Err("unexpected property response".into()); }
    let length = varint(&mut bytes)? as usize;
    if length != bytes.len() { return Err("invalid property response length".into()); }
    let mut status = 0;
    let mut value = 0;
    while !bytes.is_empty() {
        match varint(&mut bytes)? {
            0x08 => status = varint(&mut bytes)?,
            0x10 => value = varint(&mut bytes)?,
            tag => return Err(format!("unexpected property field {tag:#x}")),
        }
    }
    if status != 0 { return Err(format!("device rejected property command: {status}")); }
    // Protobuf omits a zero-valued field: 22 00 is a valid numeric zero.
    Ok(value)
}
