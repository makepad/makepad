//! Wire formats: UFACTORY's "Data Description of TCP port" and private Modbus/TCP manual.
//! https://docs.supportarticle.ufactory.cc/support_articles/developer/firmware/data-description-of-tcp-port.html
//! https://docs.api.ufactory.cc/private_modbus_tcp/p12-other-robotic-arm-function-51-100.html
//! https://github.com/xArm-Developer/xArm-Python-SDK/blob/master/xarm/core/config/x_config.py
//! Only the explicit speed-limit and stop actions open the command port.

use crate::recording::{FrameSample, MAX_SAMPLE_GAP_US};
use crate::workspace::valid_pose;
use makepad_widgets::{ThreadOptions, ThreadSpawner};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

const ACTUAL_PORT: u16 = 30000;
const RICH_PORT: u16 = 30002;
const COMMAND_PORT: u16 = 502;
const RICH_AGE: Duration = Duration::from_millis(700); // rich reports run at 5 Hz
const ACTUAL_AGE: Duration = Duration::from_millis(200); // actual measurements run at 200–250 Hz
const SOCKET_TIMEOUT: Duration = Duration::from_secs(1);
const SLOW_TCP: f32 = 10.0;
const SLOW_JOINT: f32 = 2.0_f32 * std::f32::consts::PI / 180.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlowLimits {
    pub enabled: bool,
    pub tcp_mm_s: f64,
    pub joint_deg_s: f64,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub joints: [f64; 6],
    pub tcp_mm: [f64; 3],
    pub tcp_offset: [f64; 6],
    pub mode: u8,
    pub state: u8,
    pub error: u8,
    pub warning: u8,
    pub slow: Option<SlowLimits>,
    pub notice: String,
}

#[derive(Clone, Debug)]
pub enum Link {
    Disconnected,
    Connecting,
    Live(Sample),
    Fault(String),
}

#[derive(Clone, Copy)]
struct Measured {
    time_us: u64,
    joints: [f64; 6],
    tcp_mm: [f64; 3],
    mode: u8,
    state: u8,
}

#[derive(Clone, Copy)]
struct Metadata {
    tcp_offset: [f64; 6],
    payload: [f64; 4],
    gravity: [f64; 3],
    mode: u8,
    state: u8,
    error: u8,
    warning: u8,
    slow: Option<SlowLimits>,
}

pub struct CompletedCapture {
    pub samples: Vec<FrameSample>,
    pub tool_offset: [f64; 6],
    pub reason: String,
}

// Only the actual-report reader appends. It may grow this Vec outside the feed lock,
// so a long take cannot hold up painting, stopping, or the other report reader.
type SampleBuffer = Arc<Mutex<Vec<FrameSample>>>;

struct Capture {
    samples: SampleBuffer,
    setup: Metadata,
    armed: Instant,
    first_us: Option<u64>,
    prev_us: u64,
    count: usize,
}

struct Finished {
    samples: SampleBuffer,
    tool_offset: [f64; 6],
    reason: String,
}

fn take_finished(slot: &mut Option<Finished>) -> Option<CompletedCapture> {
    // A reader that accepted a packet just before Stop may be finishing its append.
    // Once the take closes no new reader can acquire a reference.
    if Arc::strong_count(&slot.as_ref()?.samples) != 1 {
        return None;
    }
    let finished = slot.take().unwrap();
    let samples = Arc::into_inner(finished.samples).unwrap().into_inner().unwrap();
    Some(CompletedCapture { samples, tool_offset: finished.tool_offset, reason: finished.reason })
}

#[derive(Default)]
struct Feed {
    capture: Option<Capture>,
    finished: Option<Finished>,
    actual: Option<(Instant, Measured)>,
    rich: Option<(Instant, Metadata)>,
    closed: bool,
    pending: Option<Action>,
    fault: Option<String>,
    notice: String,
}

impl Feed {
    fn active(&self) -> bool {
        !self.closed && self.fault.is_none()
    }

    fn finish_capture(&mut self, reason: &str) {
        if let Some(capture) = self.capture.take() {
            debug_assert!(self.finished.is_none());
            self.finished = Some(Finished {
                samples: capture.samples, tool_offset: capture.setup.tcp_offset, reason: reason.to_owned(),
            });
        }
    }

    fn report_fault(&mut self, reason: String) {
        if self.active() {
            self.finish_capture(&reason);
            self.fault = Some(reason);
        }
    }

    fn teaching_setup(&self, now: Instant) -> Result<Metadata, &'static str> {
        if !self.active() { return Err("Controller is disconnected"); }
        let (Some((at, actual)), Some((rt, rich))) = (self.actual, self.rich) else {
            return Err("Waiting for measured joints and controller settings");
        };
        if now.saturating_duration_since(at) > ACTUAL_AGE || now.saturating_duration_since(rt) > RICH_AGE {
            return Err("Controller telemetry is stale");
        }
        if rich.error != 0 || rich.warning != 0 { return Err("Controller reported an alarm or warning"); }
        if rich.mode != 2 || actual.mode != 2 { return Err("Hand-guided recording requires Manual Mode"); }
        if !matches!(rich.state, 0..=2) || !matches!(actual.state, 0..=2) {
            return Err("Controller left its active teaching state");
        }
        if !valid_pose(actual.joints) { return Err("Measured joint outside the UF850 range"); }
        if let Some(capture) = &self.capture {
            if capture.setup.tcp_offset != rich.tcp_offset { return Err("Tool offset changed"); }
            if capture.setup.gravity != rich.gravity { return Err("Mounting direction changed"); }
            if capture.setup.payload != rich.payload { return Err("Tool payload changed"); }
        }
        Ok(rich)
    }

    fn check_capture(&mut self, now: Instant) {
        if self.capture.is_some() {
            if let Err(reason) = self.teaching_setup(now) { self.finish_capture(reason); }
        }
    }

    fn on_measurement(&mut self, now: Instant, actual: Measured) -> Option<(SampleBuffer, FrameSample)> {
        self.actual = Some((now, actual));
        self.check_capture(now);
        let capture = self.capture.as_mut()?;
        if now < capture.armed {
            capture.prev_us = actual.time_us;
            return None;
        }
        let step = actual.time_us.checked_sub(capture.prev_us);
        if !matches!(step, Some(1..=MAX_SAMPLE_GAP_US)) {
            self.finish_capture("Gap in the controller sample clock");
            return None;
        }
        let first_us = *capture.first_us.get_or_insert(actual.time_us);
        capture.prev_us = actual.time_us;
        capture.count += 1;
        Some((capture.samples.clone(), FrameSample {
            time_us: actual.time_us - first_us, joints: actual.joints, tcp_mm: actual.tcp_mm,
        }))
    }

    fn link(&self) -> Link {
        if self.closed {
            return Link::Disconnected;
        }
        if let Some(reason) = &self.fault {
            return Link::Fault(reason.clone());
        }
        let (Some((at, actual)), Some((rt, rich))) = (self.actual, self.rich) else {
            return Link::Connecting;
        };
        if at.elapsed() > ACTUAL_AGE || rt.elapsed() > RICH_AGE {
            return Link::Fault("Controller telemetry is stale".into());
        }
        // The independently timed reports must agree on the teach mode before admitting a capture.
        if actual.mode != rich.mode {
            return Link::Connecting;
        }
        Link::Live(Sample {
            joints: actual.joints,
            tcp_mm: actual.tcp_mm,
            tcp_offset: rich.tcp_offset,
            mode: actual.mode,
            state: actual.state,
            error: rich.error,
            warning: rich.warning,
            slow: rich.slow,
            notice: self.notice.clone(),
        })
    }
}

#[derive(Clone, Copy)]
enum Action {
    LowerLimits,
    Stop,
}

#[derive(Default)]
pub struct Controller {
    detached: Option<Finished>,
    session: Option<(Arc<Mutex<Feed>>, mpsc::SyncSender<()>)>,
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl Controller {
    pub fn disconnect(&mut self) {
        if let Some((feed, _)) = self.session.take() {
            let mut feed = feed.lock().unwrap();
            feed.finish_capture("Controller disconnected");
            feed.closed = true;
            if let Some(finished) = feed.finished.take() {
                debug_assert!(self.detached.is_none());
                self.detached = Some(finished);
            }
        }
    }

    pub fn start_capture(&mut self) -> Result<(), String> {
        let (feed, _) = self.session.as_ref().ok_or("Connect to the UF850 first")?;
        let mut feed = feed.lock().unwrap();
        if self.detached.is_some() || feed.finished.is_some() {
            return Err("Collect the previous recording first".into());
        }
        if feed.capture.is_some() { return Err("Already recording".into()); }
        let armed = Instant::now();
        let setup = feed.teaching_setup(armed)?;
        let (_, previous) = feed.actual.unwrap();
        feed.capture = Some(Capture {
            samples: Arc::default(), setup, armed, first_us: None, prev_us: previous.time_us, count: 0,
        });
        Ok(())
    }

    pub fn stop_capture(&mut self) {
        if let Some((feed, _)) = &self.session {
            let mut feed = feed.lock().unwrap();
            feed.check_capture(Instant::now());
            feed.finish_capture("Stopped by operator");
        }
    }

    /// Sample count and elapsed robot microseconds while a take is active.
    pub fn capture_status(&self) -> Option<(usize, u64)> {
        let mut feed = self.session.as_ref()?.0.lock().unwrap();
        feed.check_capture(Instant::now());
        let capture = feed.capture.as_ref()?;
        Some((capture.count, capture.first_us.map_or(0, |t| capture.prev_us - t)))
    }

    pub fn take_completed_capture(&mut self) -> Option<CompletedCapture> {
        if self.detached.is_some() { return take_finished(&mut self.detached); }
        let mut feed = self.session.as_ref()?.0.lock().unwrap();
        feed.check_capture(Instant::now());
        take_finished(&mut feed.finished)
    }

    pub fn status(&self) -> Link {
        self.session.as_ref().map_or(Link::Disconnected, |(feed, _)| {
            feed.lock().unwrap().link()
        })
    }

    pub fn connect(&mut self, spawner: &ThreadSpawner, ip: IpAddr) {
        self.disconnect();
        let feed = Arc::new(Mutex::new(Feed::default()));
        let (send, actions) = mpsc::sync_channel(1);
        self.session = Some((feed.clone(), send));
        let launch = || -> Result<(), String> {
            if ip.is_unspecified() || ip.is_multicast() || matches!(ip, IpAddr::V4(v) if v.is_broadcast()) {
                return Err("Enter the unicast IP of one UF850 controller".into());
            }
            for port in [ACTUAL_PORT, RICH_PORT] {
                let feed = feed.clone();
                spawner.spawn_worker(ThreadOptions::default(), move || {
                    if let Err(e) = stream_reports(ip, port, &feed) {
                        let mut feed = feed.lock().unwrap();
                        feed.report_fault(format!("Port {port}: {e}"));
                    }
                }).map_err(|e| format!("Could not start report worker: {e:?}"))?.detach();
            }
            let feed = feed.clone();
            spawner.spawn_worker(ThreadOptions::default(), move || {
                while actions.recv().is_ok() {
                    let Some(action) = feed.lock().unwrap().pending.take() else { continue };
                    // Bind every action to this particular link; never retry after a lost response.
                    let notice = command(ip, &feed, action).unwrap_or_else(|e| format!("Unconfirmed: {e}"));
                    feed.lock().unwrap().notice = notice;
                }
            }).map_err(|e| format!("Could not start control worker: {e:?}"))?.detach();
            Ok(())
        };
        if let Err(e) = launch() {
            feed.lock().unwrap().report_fault(e);
        }
    }

    fn queue(&self, action: Action) -> Result<(), String> {
        let (feed, send) = self.session.as_ref().ok_or("Connect to the UF850 first")?;
        let mut guard = feed.lock().unwrap();
        eligible(&guard, action)?;
        if guard.pending.is_some() && matches!(action, Action::LowerLimits) {
            return Err("Another controller action is pending".into());
        }
        // A stop replaces a pending speed change and interrupts a change between requests.
        guard.pending = Some(action);
        match send.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(())) => {
                guard.pending = None;
                Err("Controller worker is unavailable".into())
            }
        }
    }

    pub fn apply_slow_limits(&self) -> Result<(), String> {
        self.queue(Action::LowerLimits)
    }

    /// Requests UFACTORY stop state 4 over Ethernet. The physical E-stop is independent.
    pub fn request_stop(&self) -> Result<(), String> {
        self.queue(Action::Stop)
    }
}

fn eligible(feed: &Feed, action: Action) -> Result<(), String> {
    let Link::Live(sample) = feed.link() else {
        return Err("Waiting for live, matching UF850 reports".into());
    };
    if matches!(action, Action::LowerLimits) {
        if matches!(feed.pending, Some(Action::Stop)) {
            return Err("Speed change superseded by stop request".into());
        }
        if sample.error != 0 || sample.warning != 0 {
            return Err("Inspect controller alarms before changing its limits".into());
        }
        if !matches!(sample.state, 2 | 4) {
            return Err("Wait for the controller to be idle or stopped before applying limits".into());
        }
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn socket(ip: IpAddr, port: u16) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&SocketAddr::new(ip, port), SOCKET_TIMEOUT)?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    Ok(stream)
}

fn read_frame(reader: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<()> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    // Rich and real-time reports occupy <1 KiB today. Bound allocation from an untrusted endpoint.
    if !(4..=u16::MAX as usize).contains(&size) {
        return Err(invalid("Invalid report length"));
    }
    buf.resize(size, 0);
    buf[..4].copy_from_slice(&header);
    reader.read_exact(&mut buf[4..])
}

fn numbers<const N: usize>(frame: &[u8], offset: usize) -> io::Result<[f64; N]> {
    let data = frame.get(offset..offset + N * 4).ok_or_else(|| invalid("Incomplete measurement"))?;
    let mut values = [0.0; N];
    for (value, bytes) in values.iter_mut().zip(data.chunks_exact(4)) {
        *value = f32::from_le_bytes(bytes.try_into().unwrap()) as f64;
        if !value.is_finite() {
            return Err(invalid("Non-finite controller measurement"));
        }
    }
    Ok(values)
}

fn measured(frame: &[u8]) -> io::Result<Measured> {
    if frame.len() < 496 {
        return Err(invalid("Unsupported actual-position report format"));
    }
    Ok(Measured {
        time_us: u64::from_be_bytes(frame[4..12].try_into().unwrap()),
        joints: numbers(frame, 116)?,
        tcp_mm: numbers(frame, 472)?,
        mode: frame[12] >> 4,
        state: frame[12] & 0x0f,
    })
}

fn limits(mode: u8, tcp: f64, joint: f64) -> io::Result<SlowLimits> {
    if mode > 1 || !tcp.is_finite() || tcp <= 0.0 || !joint.is_finite() || joint <= 0.0 {
        return Err(invalid("Invalid reduced-mode limits; inspect UFACTORY Studio"));
    }
    Ok(SlowLimits { enabled: mode == 1, tcp_mm_s: tcp, joint_deg_s: joint.to_degrees() })
}

fn metadata(frame: &[u8]) -> io::Result<Metadata> {
    if frame.len() < 315 {
        return Err(invalid("Firmware omits the base frame or simulation status"));
    }
    // Vendor XArm.is_850 uses both fields; other six-axis models have different kinematics.
    if (frame[145], frame[146]) != (12, 6) {
        return Err(invalid("Controller is not a six-axis UFACTORY 850"));
    }
    if frame[314] != 0 {
        return Err(invalid("Controller is in simulation mode"));
    }
    if numbers::<6>(frame, 288)?.iter().any(|&n| n != 0.0) {
        return Err(invalid("Select the base coordinate system in UFACTORY Studio before teaching"));
    }
    let slow = if frame.len() >= 516 {
        let [tcp, joint] = numbers(frame, 508)?;
        limits(frame[495], tcp, joint).ok()
    } else {
        None
    };
    Ok(Metadata {
        mode: frame[4] >> 4,
        state: frame[4] & 0x0f,
        tcp_offset: numbers(frame, 91)?,
        payload: numbers(frame, 115)?,
        gravity: numbers(frame, 133)?,
        error: frame[89],
        warning: frame[90],
        slow,
    })
}

fn stream_reports(ip: IpAddr, port: u16, feed: &Mutex<Feed>) -> io::Result<()> {
    receive_reports(&mut socket(ip, port)?, port, feed)
}

fn receive_reports(stream: &mut impl Read, port: u16, feed: &Mutex<Feed>) -> io::Result<()> {
    let mut buf = Vec::new();
    let mut previous_timestamp = None;
    let mut clock: Option<(u64, Instant)> = None;
    let max_gap = if port == ACTUAL_PORT { ACTUAL_AGE } else { RICH_AGE };
    let mut last_arrival: Option<Instant> = None;
    while feed.lock().unwrap().active() {
        read_frame(stream, &mut buf)?;
        let now = Instant::now();
        if last_arrival.is_some_and(|last| now.duration_since(last) > max_gap) {
            return Err(invalid("Report gap; reconnect to acquire a fresh session"));
        }
        last_arrival = Some(now);
        if port == ACTUAL_PORT {
            let actual = measured(&buf)?;
            if previous_timestamp.is_some_and(|t| t >= actual.time_us) {
                return Err(invalid("Controller timestamp stalled or restarted"));
            }
            previous_timestamp = Some(actual.time_us);
            let (start_us, start) = *clock.get_or_insert((actual.time_us, now));
            let elapsed = Duration::from_micros(actual.time_us - start_us);
            let local = now.duration_since(start);
            if local.saturating_sub(elapsed) > ACTUAL_AGE {
                return Err(invalid("Measured position stream fell behind"));
            }
            // Re-anchor when the robot clock runs ahead or connection latency improves.
            if elapsed > local {
                clock = Some((actual.time_us, now));
            }
            let append = feed.lock().unwrap().on_measurement(now, actual);
            if let Some((samples, packet)) = append {
                let mut samples = samples.lock().unwrap();
                samples.try_reserve(1).map_err(|_| invalid("Insufficient memory for the next measured packet"))?;
                samples.push(packet);
            }
        } else {
            let rich = metadata(&buf)?;
            let mut feed = feed.lock().unwrap();
            feed.rich = Some((now, rich));
            feed.check_capture(now);
        }
    }
    Ok(())
}

// Command allowlist. No generic register or user-configurable payload goes through the UI.
enum Request {
    ReadLimits,
    TcpLimit,
    JointLimit,
    EnableReduced,
    Stop,
}

impl Request {
    fn wire(&self) -> (u8, Vec<u8>) {
        match self {
            Self::ReadLimits => (53, vec![]),
            Self::TcpLimit => (47, SLOW_TCP.to_le_bytes().to_vec()),
            Self::JointLimit => (48, SLOW_JOINT.to_le_bytes().to_vec()),
            Self::EnableReduced => (50, vec![1]),
            Self::Stop => (12, vec![4]),
        }
    }
}

fn exchange(stream: &mut (impl Read + Write), id: u16, request: Request) -> io::Result<Vec<u8>> {
    let reading = matches!(request, Request::ReadLimits);
    let (reg, value) = request.wire();
    let mut bytes = Vec::from(id.to_be_bytes());
    bytes.extend_from_slice(&2u16.to_be_bytes());
    bytes.extend_from_slice(&(1 + value.len() as u16).to_be_bytes());
    bytes.push(reg);
    bytes.extend_from_slice(&value);
    stream.write_all(&bytes)?;
    let mut header = [0; 8];
    stream.read_exact(&mut header)?;
    let len = u16::from_be_bytes([header[4], header[5]]);
    // Bit 0x10 means "not ready to move" (e.g. STOP state), which the SDK accepts
    // for these non-motion registers. An invalid command, alarm or warning is unconfirmed.
    if header[..4] != bytes[..4] || header[6] != reg || header[7] & 0x68 != 0
        || (reading && len != 81) || (!reading && len != 2)
    {
        return Err(invalid("Unexpected or rejected controller response; inspect UFACTORY Studio"));
    }
    let mut data = vec![0; (len - 2) as usize];
    stream.read_exact(&mut data)?;
    Ok(data)
}

struct Settings {
    bytes: Vec<u8>,
    tcp: f32,
    joint: f32,
    enabled: bool,
}

impl Settings {
    fn parse(bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.len() != 79 {
            return Err(invalid("Firmware omits reduced-mode safety settings"));
        }
        let [tcp, joint] = numbers(&bytes, 13)?;
        let enabled = limits(bytes[0], tcp, joint)?.enabled;
        Ok(Self { bytes, tcp: tcp as f32, joint: joint as f32, enabled })
    }

    fn same_boundaries(&self, other: &Self) -> bool {
        self.bytes[1..13] == other.bytes[1..13] && self.bytes[21..] == other.bytes[21..]
    }
}

fn submit_action(action: Action, mut send: impl FnMut(Request) -> io::Result<Vec<u8>>) -> io::Result<String> {
    if matches!(action, Action::Stop) {
        send(Request::Stop)?;
        return Ok("Controller acknowledged stop request".into());
    }
    let before = Settings::parse(send(Request::ReadLimits)?)?;
    if before.tcp > SLOW_TCP {
        send(Request::TcpLimit)?;
    }
    if before.joint > SLOW_JOINT {
        send(Request::JointLimit)?;
    }
    // Verify stored limits before turning them on. If a controller only supplies partial
    // settings or clamps a value above our cap, leave activation to the operator in Studio.
    if before.tcp > SLOW_TCP || before.joint > SLOW_JOINT {
        let staged = Settings::parse(send(Request::ReadLimits)?)?;
        if staged.tcp > before.tcp.min(SLOW_TCP) || staged.joint > before.joint.min(SLOW_JOINT)
            || !before.same_boundaries(&staged)
        {
            return Err(invalid("Controller did not stage both requested caps; inspect settings in Studio"));
        }
    }
    // Reassert ON even if already active after setting a cap, as required by UFACTORY.
    if !before.enabled || before.tcp > SLOW_TCP || before.joint > SLOW_JOINT {
        send(Request::EnableReduced)?;
    }
    let after = Settings::parse(send(Request::ReadLimits)?)?;
    if !after.enabled || after.tcp > before.tcp.min(SLOW_TCP) || after.joint > before.joint.min(SLOW_JOINT)
        || !before.same_boundaries(&after)
    {
        return Err(invalid("Limit readback differs; check speeds and boundaries in Studio"));
    }
    Ok(format!("Controller verified {:.1} mm/s and {:.2}°/s", after.tcp, after.joint.to_degrees()))
}

fn command(ip: IpAddr, feed: &Mutex<Feed>, action: Action) -> Result<String, String> {
    eligible(&feed.lock().unwrap(), action)?;
    let mut stream = socket(ip, COMMAND_PORT).map_err(|e| e.to_string())?;
    let mut counter = 0;
    submit_action(action, |request| {
        eligible(&feed.lock().unwrap(), action).map_err(invalid)?;
        counter += 1;
        exchange(&mut stream, counter, request)
    }).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich_frame() -> Vec<u8> {
        let mut frame = vec![0; 574];
        frame[..4].copy_from_slice(&574u32.to_be_bytes());
        frame[145..147].copy_from_slice(&[12, 6]);
        frame[508..512].copy_from_slice(&10.0f32.to_le_bytes());
        frame[512..516].copy_from_slice(&SLOW_JOINT.to_le_bytes());
        frame
    }

    #[test]
    fn only_measured_pose_in_the_uf850_base_is_eligible() {
        let mut rich = rich_frame();
        let at = Instant::now();
        let mut rt = vec![0; 784];
        // Target deliberately differs from the measured encoder and TCP.
        rt[32..36].copy_from_slice(&0.7f32.to_le_bytes());
        rt[424..428].copy_from_slice(&999.0f32.to_le_bytes());
        rt[116..120].copy_from_slice(&0.25f32.to_le_bytes());
        rt[472..476].copy_from_slice(&120.0f32.to_le_bytes());
        let mut feed = Feed { actual: Some((at, measured(&rt).unwrap())), ..Default::default() };
        assert!(matches!(feed.link(), Link::Connecting));
        feed.rich = Some((at, metadata(&rich).unwrap()));
        let Link::Live(sample) = feed.link() else { panic!("valid pose") };
        assert_eq!(sample.joints[0], 0.25);
        assert_eq!(sample.tcp_mm[0], 120.0);
        feed.actual.as_mut().unwrap().0 -= Duration::from_secs(1);
        assert!(matches!(feed.link(), Link::Fault(_)));
        assert!(eligible(&feed, Action::LowerLimits).is_err());
        rt[116..120].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(measured(&rt).is_err());
        rich[145] = 9;
        assert!(metadata(&rich).is_err()); // Lite6 has six axes too
        rich[145] = 12;
        rich[314] = 1;
        assert!(metadata(&rich).is_err());
        rich[314] = 0;
        rich[288..292].copy_from_slice(&100.0f32.to_le_bytes());
        assert!(metadata(&rich).is_err());
        assert!(metadata(&rich[..146]).is_err());
    }

    #[test]
    fn stop_supersedes_pending_configuration_and_disconnect_invalidates_it() {
        let at = Instant::now();
        let mut rt = vec![0; 784];
        rt[12] = 2;
        let feed = Arc::new(Mutex::new(Feed {
            actual: Some((at, measured(&rt).unwrap())), rich: Some((at, metadata(&rich_frame()).unwrap())),
            ..Default::default()
        }));
        let (send, receive) = mpsc::sync_channel(1);
        let mut app = Controller { session: Some((feed.clone(), send)), detached: None };
        app.apply_slow_limits().unwrap();
        app.request_stop().unwrap();
        assert!(matches!(feed.lock().unwrap().pending, Some(Action::Stop)));
        assert!(app.apply_slow_limits().is_err());
        receive.try_recv().unwrap(); // one wake is enough for the latest, higher-priority stop
        feed.lock().unwrap().pending = None;
        feed.lock().unwrap().actual.as_mut().unwrap().1.state = 1;
        assert!(app.apply_slow_limits().is_err());
        assert!(app.request_stop().is_ok());
        app.disconnect();
        assert!(eligible(&feed.lock().unwrap(), Action::Stop).is_err());
        assert!(matches!(app.status(), Link::Disconnected));
    }

    fn actual_frame(time_us: u64, angle: f32) -> Vec<u8> {
        let mut f = vec![0; 784];
        f[..4].copy_from_slice(&784u32.to_be_bytes());
        f[4..12].copy_from_slice(&time_us.to_be_bytes());
        f[12] = 0x21; // hand guiding in motion
        f[116..120].copy_from_slice(&angle.to_le_bytes());
        f[472..476].copy_from_slice(&(100.0 + angle).to_le_bytes());
        f
    }

    fn teaching_session() -> (Controller, Arc<Mutex<Feed>>, mpsc::Receiver<()>) {
        let now = Instant::now();
        let mut rich = rich_frame();
        rich[4] = 0x22;
        rich[91..95].copy_from_slice(&25.0f32.to_le_bytes());
        rich[141..145].copy_from_slice(&(-1.0f32).to_le_bytes()); // base gravity -Z
        let feed = Arc::new(Mutex::new(Feed {
            actual: Some((now, measured(&actual_frame(12_000, 0.)).unwrap())),
            rich: Some((now, metadata(&rich).unwrap())),
            ..Default::default()
        }));
        let (tx, rx) = mpsc::sync_channel(1);
        (Controller { session: Some((feed.clone(), tx)), detached: None }, feed, rx)
    }

    #[test]
    fn hand_guiding_records_each_wire_packet_and_preserves_it_after_disconnect() {
        let (mut app, feed, commands) = teaching_session();
        app.start_capture().unwrap();
        assert!(app.start_capture().is_err());
        let mut frames = Vec::new();
        for i in 1..=4 {
            frames.extend(actual_frame(12_000 + 4_000*i, 0.25*i as f32));
        }
        // Four TCP packets reach the actual-report reader between two UI calls.
        assert_eq!(receive_reports(&mut frames.as_slice(), ACTUAL_PORT, &feed).unwrap_err().kind(),
                   io::ErrorKind::UnexpectedEof);
        assert_eq!(app.capture_status(), Some((4, 12_000)));
        assert!(app.take_completed_capture().is_none());
        app.disconnect();
        let data = app.take_completed_capture().unwrap();
        assert!(data.reason.contains("disconnected"));
        assert_eq!(data.tool_offset[0], 25.);
        for (i, frame) in data.samples.iter().enumerate() {
            assert_eq!(frame.time_us, 4000 * i as u64);
            assert_eq!(frame.joints[0], 0.25 * (i+1) as f64);
            assert_eq!(frame.tcp_mm[0], 100. + frame.joints[0]);
        }
        assert!(commands.try_recv().is_err()); // no stop, resume, or movement was queued
        assert!(app.take_completed_capture().is_none());
    }

    #[test]
    fn capture_closes_at_the_first_gap_and_keeps_its_prefix_until_collected() {
        let (mut app, feed, _) = teaching_session();
        app.start_capture().unwrap();
        let t = 16_000 + MAX_SAMPLE_GAP_US + 1;
        let packets = [actual_frame(16_000, 0.1), actual_frame(t, 0.2), actual_frame(t+4000, 0.3)].concat();
        let _ = receive_reports(&mut packets.as_slice(), ACTUAL_PORT, &feed);
        assert_eq!(app.capture_status(), None);
        assert!(app.start_capture().is_err()); // completed output cannot be overwritten
        let data = app.take_completed_capture().unwrap();
        assert!(data.reason.contains("Gap"));
        assert_eq!(data.samples.len(), 1);
        assert_eq!(data.samples[0].time_us, 0);
        app.start_capture().unwrap();
        app.stop_capture();
        assert!(app.take_completed_capture().unwrap().samples.is_empty());
    }

    #[test]
    fn changed_setup_alarms_stale_reports_and_invalid_joints_end_the_recording() {
        for (which, reason) in [
            (0, "Tool offset"), (1, "Mounting"), (2, "payload"), (3, "alarm"),
            (4, "stale"), (5, "joint"), (6, "Manual"), (7, "state"), (8, "Port"),
        ] {
            let (mut app, shared, _) = teaching_session();
            app.start_capture().unwrap();
            let _ = receive_reports(&mut actual_frame(16_000, 0.1).as_slice(), ACTUAL_PORT, &shared);
            let mut feed = shared.lock().unwrap();
            let (at, rich) = feed.rich.as_mut().unwrap();
            match which {
                0 => rich.tcp_offset[0] += 1.,
                1 => rich.gravity[2] = 1.,
                2 => rich.payload[0] = 1.,
                3 => rich.warning = 20,
                4 => *at -= Duration::from_secs(1),
                5 => feed.actual.as_mut().unwrap().1.joints[2] = 9.0,
                6 => rich.mode = 1,
                7 => rich.state = 4,
                _ => feed.report_fault("Port closed".into()),
            }
            drop(feed);
            assert_eq!(app.capture_status(), None);
            let data = app.take_completed_capture().unwrap();
            assert!(data.reason.contains(reason), "{}", data.reason);
            assert_eq!(data.samples.len(), 1);
            assert!(app.capture_status().is_none());
            // An explicit new take may use a newly configured tool, payload or mounting.
            assert_eq!(app.start_capture().is_ok(), which < 3);
        }
    }

    #[test]
    fn stopping_during_a_pending_append_waits_for_the_last_accepted_packet() {
        let (mut app, feed, _) = teaching_session();
        app.start_capture().unwrap();
        let (buffer, packet) = feed.lock().unwrap().on_measurement(
            Instant::now(), measured(&actual_frame(16_000, 0.4)).unwrap()).unwrap();
        app.disconnect();
        assert!(app.take_completed_capture().is_none());
        buffer.lock().unwrap().push(packet);
        drop(buffer);
        assert_eq!(app.take_completed_capture().unwrap().samples.len(), 1);
    }

    #[test]
    fn partial_reports_and_bad_length_are_rejected() {
        let frame = rich_frame();
        let mut data = Vec::new();
        read_frame(&mut &frame[..], &mut data).unwrap();
        assert_eq!(data, frame);
        assert!(read_frame(&mut &frame[..99], &mut data).is_err());
        assert!(read_frame(&mut &[255, 255, 255, 255][..], &mut data).is_err());
    }

    #[test]
    fn passive_loopback_session_reads_both_streams_without_writing() {
        use std::net::{Shutdown, TcpListener};

        // Keep both endpoints on ephemeral loopback ports; a test cannot reach lab hardware.
        let feed = Mutex::new(Feed::default());
        let mut rt = vec![0; 784];
        rt[..4].copy_from_slice(&784u32.to_be_bytes());
        rt[4..12].copy_from_slice(&1000u64.to_be_bytes());
        rt[12] = 0x22;
        rt[116..120].copy_from_slice(&0.5f32.to_le_bytes());
        rt[472..476].copy_from_slice(&72.0f32.to_le_bytes());
        let mut rich = rich_frame();
        rich[4] = 0x22;
        for (port, frame) in [(RICH_PORT, rich), (ACTUAL_PORT, rt)] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let mut receiver = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (mut controller, _) = listener.accept().unwrap();
            controller.write_all(&frame).unwrap();
            controller.shutdown(Shutdown::Write).unwrap();
            assert_eq!(receive_reports(&mut receiver, port, &feed).unwrap_err().kind(),
                       io::ErrorKind::UnexpectedEof);
            drop(receiver);
            let mut sent_by_app = Vec::new();
            controller.read_to_end(&mut sent_by_app).unwrap();
            assert!(sent_by_app.is_empty());
        }
        let Link::Live(s) = feed.lock().unwrap().link() else { panic!("two fresh streams required") };
        assert_eq!((s.joints[0], s.tcp_mm[0], s.mode, s.state), (0.5, 72., 2, 2));
    }

    // Local register-level test controller. It can acknowledge while ignoring a setting,
    // as a mismatched firmware might; the client must detect this before enabling limits.
    fn emulated(tcp: f32, joint: f32, apply: bool) -> (io::Result<String>, Vec<u8>, Vec<u8>) {
        let mut settings = vec![0u8; 79];
        settings[13..17].copy_from_slice(&tcp.to_le_bytes());
        settings[17..21].copy_from_slice(&joint.to_le_bytes());
        settings[77] = 1;
        let mut registers = Vec::new();
        let result = submit_action(Action::LowerLimits, |request| {
            let (register, data) = request.wire();
            registers.push(register);
            match register {
                53 => return Ok(settings.clone()),
                47 if apply => settings[13..17].copy_from_slice(&data),
                48 if apply => settings[17..21].copy_from_slice(&data),
                50 => settings[0] = data[0],
                _ => (),
            }
            Ok(Vec::new())
        });
        (result, registers, settings)
    }

    #[test]
    fn lowering_skips_stricter_values_preserves_fence_and_reads_back_activation() {
        let (result, registers, settings) = emulated(1.0, 0.8, true);
        assert!(result.is_ok());
        assert_eq!(registers, [53, 48, 53, 50, 53]);
        assert_eq!(numbers::<1>(&settings, 13).unwrap(), [1.0]);
        assert_eq!(settings[77], 1);
        let (result, registers, _) = emulated(3.0, 0.02, true);
        assert!(result.is_ok());
        assert_eq!(registers, [53, 50, 53]);
        let (result, registers, _) = emulated(500.0, 0.02, false);
        assert!(result.is_err());
        assert_eq!(registers, [53, 47, 53]);
        let (result, registers, _) = emulated(0.0, 0.2, true);
        assert!(result.is_err());
        assert_eq!(registers, [53]);
    }

    #[test]
    fn stop_request_and_response_are_bound_to_the_same_transaction() {
        struct Wire {
            rx: io::Cursor<Vec<u8>>,
            tx: Vec<u8>,
        }
        impl Read for Wire {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> { self.rx.read(out) }
        }
        impl Write for Wire {
            fn write(&mut self, src: &[u8]) -> io::Result<usize> { self.tx.write(src) }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let ack = vec![0, 1, 0, 2, 0, 2, 12, 0x10];
        let mut wire = Wire { rx: io::Cursor::new(ack.clone()), tx: Vec::new() };
        assert!(exchange(&mut wire, 1, Request::Stop).unwrap().is_empty());
        assert_eq!(wire.tx, [0, 1, 0, 2, 0, 2, 12, 4]);
        for (field, value) in [(1, 2), (3, 0), (5, 4), (6, 13), (7, 0x40)] {
            let mut bad = ack.clone();
            bad[field] = value;
            wire.rx = io::Cursor::new(bad);
            assert!(exchange(&mut wire, 1, Request::Stop).is_err());
        }
    }
}
