//! The temperature tray: what the machine's sensors read, kept as history, and the over-limit
//! warning.
//!
//! A worker thread (`start_sampler`) reads every sensor once a second: on Linux the hwmon
//! drivers in `/sys/class/hwmon` (k10temp / coretemp / zenpower, amdgpu, nvme, drivetemp, the
//! DIMM and board chips, fans, power) and NVIDIA GPUs through the driver's own NVML library
//! (`libnvidia-ml.so.1`, loaded at runtime; no tool is run per sample). The thermal zones of
//! `/sys/class/thermal` stand in when no CPU driver reports. The UI thread folds each frame into
//! [`Temps`]: an hour at 1 Hz and a day of 30 s peaks, the per-sensor alarm state and the bar
//! badge. Sampling never stops (the warning needs it, and the reads are a few dozen small sysfs
//! files); only the panel's drawing waits for the panel to be open.
//!
//! The warning: a temperature at or over its sensor's limit for [`SUSTAIN_S`] turns the badge red
//! and opens the panel once per episode (until every alarm has cleared, [`HYSTERESIS`] under its
//! limit). Limits come from the sensor's own critical point when it reports one, else from the
//! kind of part ([`limit_for`]).

// Only Linux reads sensors yet: elsewhere the history and warning are compiled but never fed.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::Arc;

/// Samples at 1 Hz kept for an hour.
pub const FINE_CAP: usize = 3600;
/// Then one peak per 30 s for a day.
pub const COARSE_STEP: f64 = 30.0;
pub const COARSE_CAP: usize = 2880;
/// An alarm needs this long over its limit (no flicker on a spike) ...
pub const SUSTAIN_S: f64 = 5.0;
/// ... and clears this far under it.
pub const HYSTERESIS: f32 = 5.0;
/// The badge warms this close to a limit.
pub const WARM_MARGIN: f32 = 8.0;

/// Which well of the panel a sensor is drawn in, in panel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Cpu,
    Gpu,
    Storage,
    Memory,
    Board,
    Fans,
    Power,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Cpu => "CPU",
            Group::Gpu => "GPU",
            Group::Storage => "Storage",
            Group::Memory => "Memory",
            Group::Board => "Board & devices",
            Group::Fans => "Fans",
            Group::Power => "Power draw",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unit {
    Celsius,
    Rpm,
    Percent,
    Watt,
}

impl Unit {
    pub fn format(self, v: f32) -> String {
        if v.is_nan() {
            return "\u{2014}".into();
        }
        match self {
            Unit::Celsius => format!("{:.0}\u{b0}", v),
            Unit::Rpm => format!("{:.0}", v),
            Unit::Percent => format!("{:.0}%", v),
            Unit::Watt if v < 10.0 => format!("{:.1} W", v),
            Unit::Watt => format!("{:.0} W", v),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Unit::Celsius => "\u{b0}C",
            Unit::Rpm => "RPM",
            Unit::Percent => "%",
            Unit::Watt => "W",
        }
    }
}

/// What a sensor measures, for its limit and its place in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Tctl / Tdie / Package id: the CPU's own control temperature.
    CpuPackage,
    /// A chiplet (Tccd).
    CpuDie,
    /// One core (coretemp): drawn as the core strip, not as a trace.
    CpuCore,
    GpuCore,
    GpuHotspot,
    GpuMemory,
    /// An NVMe drive's composite temperature (the one it throttles on).
    Nvme,
    /// A SATA/SAS drive (drivetemp).
    Disk,
    /// A drive's extra sensors: drawn, never alarmed.
    DriveOther,
    Dimm,
    Other,
    Fan,
    Power,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    /// Stable across restarts: driver, device and channel.
    pub key: String,
    /// The part, e.g. "AMD Ryzen 7 7800X3D" or "NVIDIA GeForce RTX 4090".
    pub device: String,
    /// The channel, e.g. "Tctl", "Composite", "edge".
    pub label: String,
    pub group: Group,
    pub unit: Unit,
    pub kind: Kind,
    /// Where the warning starts; `None` never warns.
    pub limit: Option<f32>,
    /// The part's own critical point when it reports one (the top of the warning band).
    pub crit: Option<f32>,
}

impl Sensor {
    /// "Tctl", or "RTX 4090 · GPU" where a well holds several parts.
    pub fn name(&self, with_device: bool) -> String {
        if with_device {
            format!("{} \u{b7} {}", short_device(&self.device), self.label)
        } else {
            self.label.clone()
        }
    }
}

/// The part's name without its maker's boilerplate.
pub fn short_device(device: &str) -> String {
    let mut s = device.to_string();
    for noise in ["(R)", "(TM)", "(tm)", "NVIDIA ", "GeForce ", "Processor", "with Radeon Graphics"] {
        s = s.replace(noise, "");
    }
    // "8-Core", "16-Core" and Intel's "CPU @ 3.60GHz"
    let words: Vec<&str> = s.split_whitespace().take_while(|w| *w != "CPU" && *w != "@").filter(|w| !w.ends_with("-Core")).collect();
    let s = words.join(" ");
    if s.is_empty() { device.trim().to_string() } else { s }
}

/// Where a sensor's warning starts: the part's critical point less a margin when it reports a
/// sane one, else the usual limit for its kind (CPU 95 °C, GPU core 88 °C, GPU hotspot and memory
/// 100 °C, NVMe 70 °C). `slowdown` is a GPU's own throttle point (NVML).
pub fn limit_for(kind: Kind, crit: Option<f32>, max: Option<f32>, slowdown: Option<f32>) -> Option<f32> {
    let sane = |v: Option<f32>| v.filter(|v| *v > 40.0 && *v < 150.0);
    let (crit, max, slowdown) = (sane(crit), sane(max), sane(slowdown));
    Some(match kind {
        Kind::CpuPackage | Kind::CpuDie | Kind::CpuCore => crit.map_or(95.0, |c| c - 5.0),
        Kind::GpuCore => slowdown.map(|s| s - 3.0).or(crit.map(|c| c - 5.0)).unwrap_or(88.0).min(92.0),
        Kind::GpuHotspot => crit.map_or(103.0, |c| c - 5.0),
        Kind::GpuMemory => slowdown.map(|s| s - 3.0).or(crit.map(|c| c - 5.0)).unwrap_or(100.0),
        // NVMe drives throttle near their "max"; 70 °C unless the drive says lower.
        Kind::Nvme => max.map_or(70.0, |m| (m - 5.0).min(70.0)),
        Kind::Disk => crit.or(max).map_or(60.0, |c| (c - 5.0).min(60.0)),
        Kind::Dimm => crit.map_or(80.0, |c| c - 5.0),
        Kind::Other => return crit.or(max).map(|c| c - 5.0),
        Kind::DriveOther | Kind::Fan | Kind::Power => return None,
    })
}

// ======================================================================
// History
// ======================================================================

/// Timestamps and one column per sensor, oldest first, `cap` deep.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    cap: usize,
    pub times: VecDeque<f64>,
    pub cols: Vec<VecDeque<f32>>,
}

impl Ring {
    fn new(cap: usize, sensors: usize) -> Ring {
        Ring { cap, times: VecDeque::new(), cols: vec![VecDeque::new(); sensors] }
    }

    fn push(&mut self, at: f64, values: &[f32]) {
        if self.times.len() == self.cap {
            self.times.pop_front();
            for c in &mut self.cols {
                c.pop_front();
            }
        }
        self.times.push_back(at);
        for (i, c) in self.cols.iter_mut().enumerate() {
            c.push_back(values.get(i).copied().unwrap_or(f32::NAN));
        }
    }

    /// The same history under a new sensor list: columns follow their sensor's key.
    fn remap(&self, old: &[Sensor], new: &[Sensor]) -> Ring {
        let mut r = Ring::new(self.cap, 0);
        r.times = self.times.clone();
        r.cols = new.iter().map(|s| match old.iter().position(|o| o.key == s.key) {
            Some(i) => self.cols[i].clone(),
            None => vec![f32::NAN; self.times.len()].into(),
        }).collect();
        r
    }

    pub fn first_time(&self) -> Option<f64> {
        self.times.front().copied()
    }

    /// The index of the first sample at or after `t`.
    pub fn index_at(&self, t: f64) -> usize {
        self.times.partition_point(|x| *x < t)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Alarm {
    over_since: Option<f64>,
    on: bool,
}

impl Alarm {
    /// One sample against `limit`; true when this sample turned the alarm on.
    fn step(&mut self, at: f64, v: f32, limit: f32) -> bool {
        if v.is_nan() {
            self.over_since = None;
            return false;
        }
        if self.on {
            if v < limit - HYSTERESIS {
                self.on = false;
                self.over_since = None;
            }
            return false;
        }
        if v >= limit {
            let since = *self.over_since.get_or_insert(at);
            if at - since >= SUSTAIN_S - 0.25 {
                self.on = true;
                return true;
            }
        } else {
            self.over_since = None;
        }
        false
    }
}

/// How the bar's thermometer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Heat {
    #[default]
    Normal,
    /// Within [`WARM_MARGIN`] of a limit.
    Warm,
    /// A sensor is in alarm.
    Alarm,
}

/// The bar's thermometer: its state, its column (the hottest sensor's place between 30 °C and its
/// limit, 0..1) and its tooltip.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Badge {
    pub heat: Heat,
    pub level: f32,
    pub tooltip: String,
}

/// One second's readings from the sampler, index-aligned with `sensors` (NaN: no reading). A
/// new sensor list rides along when it changed.
#[derive(Clone, Debug)]
pub struct Frame {
    pub at: f64,
    pub sensors: Option<Arc<Vec<Sensor>>>,
    pub values: Vec<f32>,
}

/// What a frame did that the WM acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pushed {
    /// A warning episode began: open the panel (once).
    pub episode_started: bool,
    /// The badge changed: redraw the bar.
    pub badge_changed: bool,
}

/// Everything the panel and the bar show: the sensors, their history and alarms.
#[derive(Clone, Debug, Default)]
pub struct Temps {
    pub sensors: Arc<Vec<Sensor>>,
    pub fine: Ring,
    pub coarse: Ring,
    /// the running peak of the coarse bucket being filled, and when it began
    peak: Vec<f32>,
    peak_from: Option<f64>,
    alarms: Vec<Alarm>,
    episode: bool,
    pub badge: Badge,
    /// the latest reading of each sensor
    pub now: Vec<f32>,
    pub last_at: f64,
}

impl Temps {
    pub fn is_empty(&self) -> bool {
        self.sensors.is_empty()
    }

    pub fn alarmed(&self, i: usize) -> bool {
        self.alarms.get(i).is_some_and(|a| a.on)
    }

    pub fn any_alarm(&self) -> bool {
        self.alarms.iter().any(|a| a.on)
    }

    fn set_sensors(&mut self, sensors: Arc<Vec<Sensor>>) {
        let n = sensors.len();
        if self.sensors.is_empty() {
            self.fine = Ring::new(FINE_CAP, n);
            self.coarse = Ring::new(COARSE_CAP, n);
        } else {
            self.fine = self.fine.remap(&self.sensors, &sensors);
            self.coarse = self.coarse.remap(&self.sensors, &sensors);
        }
        let old_alarms = std::mem::take(&mut self.alarms);
        self.alarms = sensors.iter().map(|s| self.sensors.iter().position(|o| o.key == s.key).and_then(|i| old_alarms.get(i).copied()).unwrap_or_default()).collect();
        self.peak = vec![f32::NAN; n];
        self.sensors = sensors;
    }

    pub fn push(&mut self, frame: Frame) -> Pushed {
        if let Some(sensors) = frame.sensors {
            if *sensors != *self.sensors {
                self.set_sensors(sensors);
            }
        }
        let at = frame.at;
        let values = frame.values;
        if values.len() != self.sensors.len() {
            return Pushed::default();
        }
        self.fine.push(at, &values);
        // the coarse tier keeps each bucket's peak (heat is about the highs)
        let from = *self.peak_from.get_or_insert(at);
        for (p, v) in self.peak.iter_mut().zip(&values) {
            if !v.is_nan() && (p.is_nan() || v > p) {
                *p = *v;
            }
        }
        // a bucket is COARSE_STEP samples at 1 Hz: its first and last are COARSE_STEP - 1 apart
        if at - from >= COARSE_STEP - 1.5 {
            let peak = std::mem::replace(&mut self.peak, vec![f32::NAN; values.len()]);
            self.coarse.push(at, &peak);
            self.peak_from = None;
        }
        let mut started = false;
        for (i, s) in self.sensors.iter().enumerate() {
            if let Some(limit) = s.limit {
                started |= self.alarms[i].step(at, values[i], limit);
            }
        }
        let any = self.any_alarm();
        let episode_started = started && !self.episode;
        self.episode = any;
        self.now = values;
        self.last_at = at;
        let badge = self.compute_badge();
        let badge_changed = badge != self.badge;
        self.badge = badge;
        Pushed { episode_started, badge_changed }
    }

    fn compute_badge(&self) -> Badge {
        let mut heat = if self.any_alarm() { Heat::Alarm } else { Heat::Normal };
        let mut level = 0.0f32;
        for (s, v) in self.sensors.iter().zip(&self.now) {
            let (Some(limit), false) = (s.limit, v.is_nan()) else { continue };
            if s.unit != Unit::Celsius {
                continue;
            }
            level = level.max(((v - 30.0) / (limit - 30.0).max(1.0)).clamp(0.0, 1.0));
            if heat == Heat::Normal && *v >= limit - WARM_MARGIN {
                heat = Heat::Warm;
            }
        }
        // the tooltip: each group's hottest temperature, alarms named
        let mut parts: Vec<String> = Vec::new();
        for g in [Group::Cpu, Group::Gpu, Group::Storage] {
            let hottest = self.sensors.iter().zip(&self.now).filter(|(s, v)| s.group == g && s.unit == Unit::Celsius && !v.is_nan()).map(|(_, v)| *v).fold(f32::NAN, |a, b| if a.is_nan() || b > a { b } else { a });
            if !hottest.is_nan() {
                parts.push(format!("{} {:.0}\u{b0}C", g.title(), hottest));
            }
        }
        let mut tooltip = if parts.is_empty() { "Temperatures".to_string() } else { parts.join(" \u{b7} ") };
        let hot: Vec<String> = self.sensors.iter().enumerate().filter(|(i, _)| self.alarmed(*i)).map(|(_, s)| s.name(true)).collect();
        if !hot.is_empty() {
            tooltip = format!("Too hot: {} \u{2014} {}", hot.join(", "), tooltip);
        }
        Badge { heat, level, tooltip }
    }

    /// The ring that covers `t0..t1`: the 1 Hz hour when it reaches back far enough (or there is
    /// nothing coarser yet), else the 30 s day.
    pub fn ring_for(&self, t0: f64, t1: f64) -> &Ring {
        let fine_from = self.fine.first_time().unwrap_or(f64::MAX);
        if t1 - t0 <= FINE_CAP as f64 + 1.0 && (fine_from <= t0 + 2.0 || self.fine.times.len() < FINE_CAP || self.coarse.times.is_empty()) {
            &self.fine
        } else {
            &self.coarse
        }
    }

    /// The sensor's lowest and highest reading in `t0..t1`.
    pub fn min_max(&self, i: usize, t0: f64, t1: f64) -> (f32, f32) {
        let ring = self.ring_for(t0, t1);
        let (a, b) = (ring.index_at(t0), ring.index_at(t1 + 0.001));
        let mut lo = f32::NAN;
        let mut hi = f32::NAN;
        if let Some(col) = ring.cols.get(i) {
            for v in col.range(a..b) {
                if v.is_nan() {
                    continue;
                }
                if lo.is_nan() || *v < lo { lo = *v; }
                if hi.is_nan() || *v > hi { hi = *v; }
            }
        }
        (lo, hi)
    }

    /// The reading nearest `t` (within a ring step), for the hover readout.
    pub fn value_at(&self, i: usize, t: f64, t0: f64, t1: f64) -> f32 {
        let ring = self.ring_for(t0, t1);
        let k = ring.index_at(t);
        let pick = [k.checked_sub(1), Some(k)].into_iter().flatten().filter(|k| *k < ring.times.len()).min_by(|a, b| (ring.times[*a] - t).abs().total_cmp(&(ring.times[*b] - t).abs()));
        match pick {
            Some(k) if (ring.times[k] - t).abs() <= COARSE_STEP => ring.cols.get(i).map_or(f32::NAN, |c| c[k]),
            _ => f32::NAN,
        }
    }

    /// The sensor's samples in `t0..t1`, at most about one per `px` s: the peak of each column
    /// (a long view of an hour of 1 Hz samples draws a few hundred segments, not thousands).
    pub fn series(&self, i: usize, t0: f64, t1: f64, px: f64) -> Vec<(f64, f32)> {
        let ring = self.ring_for(t0, t1);
        let Some(col) = ring.cols.get(i) else { return Vec::new() };
        // one sample either side, so the line reaches the edges
        let a = ring.index_at(t0).saturating_sub(1);
        let b = (ring.index_at(t1) + 1).min(ring.times.len());
        let mut out: Vec<(f64, f32)> = Vec::with_capacity(((t1 - t0) / px.max(0.001)).min((b - a) as f64) as usize + 2);
        let mut bucket: Option<(f64, f64, f32)> = None; // (column start, last t, peak)
        for k in a..b {
            let (t, v) = (ring.times[k], col[k]);
            if v.is_nan() {
                if let Some((_, bt, bv)) = bucket.take() { out.push((bt, bv)); }
                out.push((t, f32::NAN));
                continue;
            }
            match &mut bucket {
                Some((start, bt, bv)) if t - *start < px => {
                    *bt = t;
                    if v > *bv { *bv = v; }
                }
                _ => {
                    if let Some((_, bt, bv)) = bucket.take() { out.push((bt, bv)); }
                    bucket = Some((t, t, v));
                }
            }
        }
        if let Some((_, bt, bv)) = bucket { out.push((bt, bv)); }
        out
    }

    /// The ring step at `t0..t1`: how far apart samples may be and still be joined.
    pub fn gap_for(&self, t0: f64, t1: f64) -> f64 {
        if std::ptr::eq(self.ring_for(t0, t1), &self.coarse) { COARSE_STEP * 2.5 } else { 3.5 }
    }

    /// The text the WM's remote snapshot reads off the open panel.
    pub fn summary(&self) -> String {
        let mut text = String::from("Temperatures");
        for (i, (s, v)) in self.sensors.iter().zip(&self.now).enumerate() {
            text.push_str(&format!("\n{:?} {}: {}{}", s.group, s.name(true), s.unit.format(*v), if self.alarmed(i) { " ALARM" } else { "" }));
        }
        text
    }
}

// ======================================================================
// The sampler
// ======================================================================

#[cfg(target_os = "linux")]
/// Spawn the lifetime sampler worker: one [`Frame`] a second. Dropping the receiver ends it.
/// Linux only for now: elsewhere no sensor is read and the bar shows no thermometer.
#[allow(clippy::type_complexity)]
pub fn start_sampler(
    spawner: &makepad_widgets::makepad_platform::thread::ThreadSpawner,
) -> Result<
    (
        std::sync::mpsc::Receiver<Frame>,
        makepad_widgets::makepad_platform::thread::TaskHandle<()>,
    ),
    makepad_widgets::makepad_platform::thread::SpawnError,
> {
    use makepad_widgets::makepad_platform::thread::{CancellationToken, SignalToUI, ThreadOptions};
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = spawner.spawn_worker(
        ThreadOptions { name: Some("wm-temps".into()), ..Default::default() },
        move || {
            let wait = CancellationToken::new();
            let mut reader = Reader::open();
            let mut sent: Option<Arc<Vec<Sensor>>> = None;
            let mut round: u32 = 0;
            loop {
                // a new device (a GPU driver loaded, a drive plugged in) is found within 30 s
                if round % 30 == 29 {
                    reader.rescan();
                }
                let at = makepad_widgets::Cx::monotonic_now();
                let values = reader.read(at);
                let sensors = reader.sensors();
                let changed = sent.as_ref().is_none_or(|s| !Arc::ptr_eq(s, &sensors));
                let frame = Frame { at, sensors: changed.then(|| sensors.clone()), values };
                sent = Some(sensors);
                if tx.send(frame).is_err() {
                    break;
                }
                SignalToUI::set_ui_signal();
                round = round.wrapping_add(1);
                let _ = wait.wait_until(at + 1.0);
            }
        },
    )?;
    Ok((rx, worker))
}

#[cfg(target_os = "linux")]
use linux::Reader;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
    use std::path::{Path, PathBuf};

    /// Where one sensor's reading comes from.
    enum Source {
        /// A sysfs file holding an integer, times `scale`.
        File(PathBuf, f32),
        /// A RAPL energy counter (µJ): watts from the change since the last read.
        Energy { path: PathBuf, last: Option<(f64, u64)>, wrap: u64 },
        Nvml(usize, NvmlWhat),
    }

    #[derive(Clone, Copy)]
    enum NvmlWhat {
        Temp,
        MemTemp,
        Power,
        Fan,
    }

    pub struct Reader {
        sensors: Arc<Vec<Sensor>>,
        sources: Vec<Source>,
        nvml: Option<Nvml>,
    }

    fn read_int(path: &Path) -> Option<i64> {
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    fn read_str(path: &Path) -> Option<String> {
        let s = std::fs::read_to_string(path).ok()?.trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn cpu_model() -> String {
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_string()))
            .unwrap_or_else(|| "CPU".into())
    }

    impl Reader {
        pub fn open() -> Reader {
            let mut r = Reader { sensors: Arc::new(Vec::new()), sources: Vec::new(), nvml: Nvml::open() };
            r.rescan();
            r
        }

        pub fn sensors(&self) -> Arc<Vec<Sensor>> {
            self.sensors.clone()
        }

        /// Find every sensor again; the list (and its `Arc`) only changes when what was found did.
        pub fn rescan(&mut self) {
            let mut found: Vec<(Sensor, Source)> = Vec::new();
            hwmon(&mut found);
            if !found.iter().any(|(s, _)| s.group == Group::Cpu) {
                thermal_zones(&mut found);
            }
            rapl(&mut found);
            if let Some(nvml) = &self.nvml {
                nvml.sensors(&mut found);
            }
            // panel order: by group, then as found (drivers in hwmon order, channels in order)
            found.sort_by_key(|(s, _)| s.group);
            // MAKEPAD_WM_TEMP_LIMIT=<°C>: every warning limit at that temperature, to try the
            // warning on a cool machine
            if let Some(test) = std::env::var("MAKEPAD_WM_TEMP_LIMIT").ok().and_then(|v| v.parse::<f32>().ok()) {
                for (s, _) in &mut found {
                    if s.limit.is_some() {
                        s.limit = Some(test);
                    }
                }
            }
            let sensors: Vec<Sensor> = found.iter().map(|(s, _)| s.clone()).collect();
            if sensors != *self.sensors {
                self.sensors = Arc::new(sensors);
                self.sources = found.into_iter().map(|(_, src)| src).collect();
            }
        }

        pub fn read(&mut self, at: f64) -> Vec<f32> {
            let nvml = self.nvml.as_ref();
            self.sources.iter_mut().map(|src| match src {
                Source::File(path, scale) => read_int(path).map_or(f32::NAN, |v| v as f32 * *scale),
                Source::Energy { path, last, wrap } => {
                    let Some(e) = read_int(path) else { return f32::NAN };
                    let e = e as u64;
                    let w = match *last {
                        Some((t, prev)) if at > t => {
                            let de = if e >= prev { e - prev } else { e + *wrap - prev };
                            de as f64 / 1e6 / (at - t)
                        }
                        _ => f64::NAN,
                    };
                    *last = Some((at, e));
                    w as f32
                }
                Source::Nvml(dev, what) => nvml.map_or(f32::NAN, |n| n.read(*dev, *what)),
            }).collect()
        }
    }

    /// The hwmon drivers: one sensor per `temp*_input`, `fan*_input` and `power*_input`.
    fn hwmon(found: &mut Vec<(Sensor, Source)>) {
        let Ok(dir) = std::fs::read_dir("/sys/class/hwmon") else { return };
        let mut dirs: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
        // hwmonN in numeric order (hwmon10 after hwmon9)
        dirs.sort_by_key(|p| p.file_name().and_then(|n| n.to_str()).and_then(|n| n.trim_start_matches("hwmon").parse::<u32>().ok()).unwrap_or(u32::MAX));
        let cpu = cpu_model();
        for h in dirs {
            let Some(driver) = read_str(&h.join("name")) else { continue };
            let device_path = std::fs::canonicalize(h.join("device")).ok();
            let device_id = device_path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("").to_string();
            let (group, device) = match driver.as_str() {
                "k10temp" | "coretemp" | "zenpower" | "cpu_thermal" => (Group::Cpu, cpu.clone()),
                "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" => {
                    let vram = device_path.as_ref().and_then(|p| read_int(&p.join("mem_info_vram_total"))).unwrap_or(0);
                    // an APU's GPU sits on the root complex's internal bridge (00:08.1 on Zen) and
                    // its "VRAM" is a carve-out the BIOS sizes, so the bridge is the tell
                    let apu = device_path.as_ref().is_some_and(|p| p.to_string_lossy().contains("/0000:00:08.1/")) || (vram > 0 && vram < 1 << 30);
                    let name = if driver == "amdgpu" && apu { "Radeon iGPU" } else if driver == "amdgpu" || driver == "radeon" { "Radeon GPU" } else if driver == "nouveau" { "NVIDIA GPU" } else { "Intel GPU" };
                    (Group::Gpu, name.to_string())
                }
                "nvme" => (Group::Storage, device_path.as_ref().and_then(|p| read_str(&p.join("model"))).unwrap_or_else(|| "NVMe".into())),
                "drivetemp" => (Group::Storage, device_path.as_ref().and_then(|p| read_str(&p.join("model"))).unwrap_or_else(|| "Disk".into())),
                "spd5118" => (Group::Memory, "DDR5".to_string()),
                "jc42" | "ee1004" => (Group::Memory, "DIMM".to_string()),
                d if d.starts_with("hidpp_battery") || d.starts_with("BAT") || d.starts_with("AC") || d.starts_with("ucsi") => continue,
                d if d.starts_with("r8169") || d.starts_with("mt79") || d.starts_with("iwlwifi") || d.starts_with("ath") => (Group::Board, "Network".to_string()),
                d if d.starts_with("nct") || d.starts_with("it8") || d.starts_with("asus") || d.contains("gigabyte") || d.starts_with("acpitz") || d.starts_with("pch") => (Group::Board, "Board".to_string()),
                _ => (Group::Board, driver.clone()),
            };
            // what an unlabelled channel is called
            let fallback = if driver.starts_with("r8169") { Some("Ethernet") } else if driver.starts_with("mt79") || driver.starts_with("iwlwifi") || driver.starts_with("ath") { Some("Wi-Fi") } else { None };
            let mut channels: Vec<(String, u32)> = std::fs::read_dir(&h)
                .map(|d| d.flatten().filter_map(|e| {
                    let n = e.file_name().into_string().ok()?;
                    let stem = n.strip_suffix("_input")?;
                    let (kind, num) = stem.split_at(stem.find(|c: char| c.is_ascii_digit())?);
                    Some((kind.to_string(), num.parse().ok()?))
                }).collect())
                .unwrap_or_default();
            channels.sort_by(|a, b| a.0.cmp(&b.0).reverse().then(a.1.cmp(&b.1))); // temp, power, fan; by number
            // DIMMs are numbered in hwmon order
            let dimms = found.iter().filter(|(s, _)| s.kind == Kind::Dimm).count();
            for (what, n) in channels {
                let input = h.join(format!("{what}{n}_input"));
                let label = read_str(&h.join(format!("{what}{n}_label")));
                let milli = |suffix: &str| read_int(&h.join(format!("{what}{n}_{suffix}"))).map(|v| v as f32 / 1000.0);
                let key = format!("hwmon:{driver}:{device_id}:{what}{n}");
                match what.as_str() {
                    "temp" => {
                        // a channel that cannot be read now (asleep, absent) is left out
                        if read_int(&input).is_none() {
                            continue;
                        }
                        let label = label.unwrap_or_else(|| if group == Group::Memory { format!("DIMM {}", dimms + 1) } else if let Some(f) = fallback { if n == 1 { f.into() } else { format!("{f} {n}") } } else if n == 1 { "Temp".into() } else { format!("Temp {n}") });
                        let kind = match (driver.as_str(), label.as_str()) {
                            (_, l) if group == Group::Cpu && (l == "Tctl" || l == "Tdie" || l.starts_with("Package") || l == "Temp") => Kind::CpuPackage,
                            (_, l) if group == Group::Cpu && l.starts_with("Tccd") => Kind::CpuDie,
                            (_, l) if group == Group::Cpu && l.starts_with("Core") => Kind::CpuCore,
                            (_, "junction") => Kind::GpuHotspot,
                            (_, "mem") => Kind::GpuMemory,
                            _ if group == Group::Gpu => Kind::GpuCore,
                            ("nvme", "Composite") => Kind::Nvme,
                            ("nvme", _) => Kind::DriveOther,
                            ("drivetemp", _) => Kind::Disk,
                            _ if group == Group::Memory => Kind::Dimm,
                            _ => Kind::Other,
                        };
                        let crit = milli("crit").or(milli("emergency")).filter(|c| *c > 40.0 && *c < 150.0);
                        let max = milli("max").filter(|c| *c > 30.0 && *c < 150.0);
                        found.push((Sensor { key, device: device.clone(), label, group, unit: Unit::Celsius, kind, limit: limit_for(kind, crit, max, None), crit: crit.or(max) }, Source::File(input, 0.001)));
                    }
                    "fan" => {
                        // a header with nothing on it reads 0: left out
                        if read_int(&input).unwrap_or(0) <= 0 {
                            continue;
                        }
                        let label = label.unwrap_or_else(|| format!("Fan {n}"));
                        let device = if group == Group::Gpu { device.clone() } else { "Board".into() };
                        found.push((Sensor { key, device, label, group: Group::Fans, unit: Unit::Rpm, kind: Kind::Fan, limit: None, crit: None }, Source::File(input, 1.0)));
                    }
                    "power" => {
                        let label = label.unwrap_or_else(|| format!("Power {n}"));
                        found.push((Sensor { key, device: device.clone(), label, group: Group::Power, unit: Unit::Watt, kind: Kind::Power, limit: None, crit: None }, Source::File(input, 1e-6)));
                    }
                    _ => {}
                }
            }
            // amdgpu dGPUs report average power instead of input on some boards
            if group == Group::Gpu && !found.iter().any(|(s, _)| s.key.starts_with(&format!("hwmon:{driver}:{device_id}:power"))) {
                let avg = h.join("power1_average");
                if read_int(&avg).is_some() {
                    let key = format!("hwmon:{driver}:{device_id}:power1");
                    let label = read_str(&h.join("power1_label")).unwrap_or_else(|| "Power".into());
                    found.push((Sensor { key, device: device.clone(), label, group: Group::Power, unit: Unit::Watt, kind: Kind::Power, limit: None, crit: None }, Source::File(avg, 1e-6)));
                }
            }
        }
    }

    /// The kernel's thermal zones: only when no CPU driver reports (ARM boards, some laptops).
    fn thermal_zones(found: &mut Vec<(Sensor, Source)>) {
        let Ok(dir) = std::fs::read_dir("/sys/class/thermal") else { return };
        let mut zones: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("thermal_zone"))).collect();
        zones.sort();
        for z in zones {
            let Some(kind_name) = read_str(&z.join("type")) else { continue };
            let temp = z.join("temp");
            if read_int(&temp).is_none() {
                continue;
            }
            let mut crit = None;
            for t in 0..16 {
                if read_str(&z.join(format!("trip_point_{t}_type"))).as_deref() == Some("critical") {
                    crit = read_int(&z.join(format!("trip_point_{t}_temp"))).map(|v| v as f32 / 1000.0);
                }
            }
            let cpu = kind_name.contains("cpu") || kind_name.contains("x86_pkg") || kind_name.contains("soc");
            let (group, kind) = if cpu { (Group::Cpu, Kind::CpuPackage) } else { (Group::Board, Kind::Other) };
            let key = format!("thermal:{}", kind_name);
            let device = if cpu { cpu_model() } else { "Thermal zone".into() };
            found.push((Sensor { key, device, label: kind_name, group, unit: Unit::Celsius, kind, limit: limit_for(kind, crit, None, None), crit }, Source::File(temp, 0.001)));
        }
    }

    /// The CPU package's power from RAPL, where the counter is readable (often root only).
    fn rapl(found: &mut Vec<(Sensor, Source)>) {
        let base = Path::new("/sys/class/powercap/intel-rapl:0");
        let path = base.join("energy_uj");
        if read_int(&path).is_none() {
            return;
        }
        let wrap = read_int(&base.join("max_energy_range_uj")).unwrap_or(1 << 32) as u64;
        found.push((Sensor { key: "rapl:package".into(), device: cpu_model(), label: "Package".into(), group: Group::Power, unit: Unit::Watt, kind: Kind::Power, limit: None, crit: None }, Source::Energy { path, last: None, wrap }));
    }

    // ------------------------------------------------------------------
    // NVML, from the driver's own library
    // ------------------------------------------------------------------

    type Device = *mut c_void;
    const NVML_SUCCESS: c_int = 0;
    const NVML_TEMPERATURE_GPU: c_int = 0;
    const NVML_TEMPERATURE_THRESHOLD_SHUTDOWN: c_int = 0;
    const NVML_TEMPERATURE_THRESHOLD_SLOWDOWN: c_int = 1;
    const NVML_TEMPERATURE_THRESHOLD_MEM_MAX: c_int = 2;
    /// `NVML_FI_DEV_MEMORY_TEMP`: the memory's temperature where the board exposes it.
    const NVML_FI_DEV_MEMORY_TEMP: c_uint = 82;

    /// `nvmlFieldValue_t`.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FieldValue {
        field_id: c_uint,
        scope_id: c_uint,
        timestamp: i64,
        latency_usec: i64,
        value_type: c_uint,
        nvml_return: c_int,
        value: u64,
    }

    struct Fns {
        get_name: unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int,
        get_temperature: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> c_int,
        get_threshold: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> c_int,
        get_power: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
        get_fan: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
        get_fields: Option<unsafe extern "C" fn(Device, c_int, *mut FieldValue) -> c_int>,
    }

    struct Nvml {
        fns: Fns,
        devices: Vec<Device>,
    }

    impl Nvml {
        fn open() -> Option<Nvml> {
            use crate::sdbus::{dlopen, dlsym, RTLD_NOW};
            let lib = unsafe { dlopen(c"libnvidia-ml.so.1".as_ptr(), RTLD_NOW) };
            if lib.is_null() {
                return None;
            }
            let sym = |name: &CStr| -> Option<*mut c_void> {
                let p = unsafe { dlsym(lib, name.as_ptr()) };
                (!p.is_null()).then_some(p)
            };
            unsafe {
                let init: unsafe extern "C" fn() -> c_int = std::mem::transmute(sym(c"nvmlInit_v2")?);
                let count: unsafe extern "C" fn(*mut c_uint) -> c_int = std::mem::transmute(sym(c"nvmlDeviceGetCount_v2")?);
                let by_index: unsafe extern "C" fn(c_uint, *mut Device) -> c_int = std::mem::transmute(sym(c"nvmlDeviceGetHandleByIndex_v2")?);
                let fns = Fns {
                    get_name: std::mem::transmute(sym(c"nvmlDeviceGetName")?),
                    get_temperature: std::mem::transmute(sym(c"nvmlDeviceGetTemperature")?),
                    get_threshold: std::mem::transmute(sym(c"nvmlDeviceGetTemperatureThreshold")?),
                    get_power: std::mem::transmute(sym(c"nvmlDeviceGetPowerUsage")?),
                    get_fan: std::mem::transmute(sym(c"nvmlDeviceGetFanSpeed")?),
                    get_fields: sym(c"nvmlDeviceGetFieldValues").map(|p| std::mem::transmute(p)),
                };
                if init() != NVML_SUCCESS {
                    return None;
                }
                let mut n: c_uint = 0;
                if count(&mut n) != NVML_SUCCESS {
                    return None;
                }
                let devices: Vec<Device> = (0..n).filter_map(|i| {
                    let mut d: Device = std::ptr::null_mut();
                    (by_index(i, &mut d) == NVML_SUCCESS).then_some(d)
                }).collect();
                Some(Nvml { fns, devices })
            }
        }

        fn uint(&self, f: unsafe extern "C" fn(Device, *mut c_uint) -> c_int, d: Device) -> Option<u32> {
            let mut v: c_uint = 0;
            (unsafe { f(d, &mut v) } == NVML_SUCCESS).then_some(v)
        }

        fn threshold(&self, d: Device, which: c_int) -> Option<f32> {
            let mut v: c_uint = 0;
            (unsafe { (self.fns.get_threshold)(d, which, &mut v) } == NVML_SUCCESS).then_some(v as f32)
        }

        fn mem_temp(&self, d: Device) -> Option<f32> {
            let f = self.fns.get_fields?;
            let mut fv = FieldValue { field_id: NVML_FI_DEV_MEMORY_TEMP, ..Default::default() };
            if unsafe { f(d, 1, &mut fv) } != NVML_SUCCESS || fv.nvml_return != NVML_SUCCESS {
                return None;
            }
            // value types: 0 double, 1 uint, 2 ulong, 3 ulonglong, 4 slonglong, 5 sint
            let v = match fv.value_type {
                0 => f64::from_bits(fv.value) as f32,
                1 | 5 => fv.value as u32 as f32,
                _ => fv.value as f32,
            };
            (v > 0.0 && v < 150.0).then_some(v)
        }

        fn sensors(&self, found: &mut Vec<(Sensor, Source)>) {
            for (i, d) in self.devices.iter().enumerate() {
                let d = *d;
                let mut buf = [0 as c_char; 96];
                let name = if unsafe { (self.fns.get_name)(d, buf.as_mut_ptr(), buf.len() as c_uint) } == NVML_SUCCESS {
                    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
                } else {
                    format!("NVIDIA GPU {i}")
                };
                let key = |what: &str| format!("nvml:{i}:{what}");
                let mut t: c_uint = 0;
                if unsafe { (self.fns.get_temperature)(d, NVML_TEMPERATURE_GPU, &mut t) } == NVML_SUCCESS {
                    let slowdown = self.threshold(d, NVML_TEMPERATURE_THRESHOLD_SLOWDOWN).filter(|v| *v > 50.0);
                    let shutdown = self.threshold(d, NVML_TEMPERATURE_THRESHOLD_SHUTDOWN).filter(|v| *v > 50.0);
                    found.push((Sensor { key: key("gpu"), device: name.clone(), label: "GPU".into(), group: Group::Gpu, unit: Unit::Celsius, kind: Kind::GpuCore, limit: limit_for(Kind::GpuCore, shutdown, None, slowdown), crit: slowdown.or(shutdown) }, Source::Nvml(i, NvmlWhat::Temp)));
                }
                if self.mem_temp(d).is_some() {
                    let max = self.threshold(d, NVML_TEMPERATURE_THRESHOLD_MEM_MAX).filter(|v| *v > 50.0);
                    found.push((Sensor { key: key("mem"), device: name.clone(), label: "Memory".into(), group: Group::Gpu, unit: Unit::Celsius, kind: Kind::GpuMemory, limit: limit_for(Kind::GpuMemory, None, None, max), crit: max }, Source::Nvml(i, NvmlWhat::MemTemp)));
                }
                if self.uint(self.fns.get_power, d).is_some() {
                    found.push((Sensor { key: key("power"), device: name.clone(), label: "Board".into(), group: Group::Power, unit: Unit::Watt, kind: Kind::Power, limit: None, crit: None }, Source::Nvml(i, NvmlWhat::Power)));
                }
                if self.uint(self.fns.get_fan, d).is_some() {
                    found.push((Sensor { key: key("fan"), device: name.clone(), label: "Fan".into(), group: Group::Fans, unit: Unit::Percent, kind: Kind::Fan, limit: None, crit: None }, Source::Nvml(i, NvmlWhat::Fan)));
                }
            }
        }

        fn read(&self, i: usize, what: NvmlWhat) -> f32 {
            let Some(d) = self.devices.get(i).copied() else { return f32::NAN };
            let v = match what {
                NvmlWhat::Temp => {
                    let mut t: c_uint = 0;
                    (unsafe { (self.fns.get_temperature)(d, NVML_TEMPERATURE_GPU, &mut t) } == NVML_SUCCESS).then_some(t as f32)
                }
                NvmlWhat::MemTemp => self.mem_temp(d),
                NvmlWhat::Power => self.uint(self.fns.get_power, d).map(|mw| mw as f32 / 1000.0),
                NvmlWhat::Fan => self.uint(self.fns.get_fan, d).map(|p| p as f32),
            };
            v.unwrap_or(f32::NAN)
        }
    }

    /// What this machine reports, for a look on a node:
    /// `cargo test --release -p makepad-app-wm temps_here -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn temps_here() {
        let mut r = Reader::open();
        let _ = r.read(0.0);
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let v = r.read(1.0);
        for (s, v) in r.sensors().iter().zip(v) {
            println!("{:?}/{:?} {} [{}] = {} limit {:?} crit {:?} ({})", s.group, s.kind, s.name(true), s.device, s.unit.format(v), s.limit, s.crit, s.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(key: &str, kind: Kind, limit: Option<f32>) -> Sensor {
        Sensor { key: key.into(), device: "AMD Ryzen 7 7800X3D 8-Core Processor".into(), label: key.into(), group: Group::Cpu, unit: Unit::Celsius, kind, limit, crit: None }
    }

    fn frame(at: f64, sensors: Option<Arc<Vec<Sensor>>>, values: &[f32]) -> Frame {
        Frame { at, sensors, values: values.to_vec() }
    }

    #[test]
    fn limits_follow_the_part_and_its_own_critical_point() {
        assert_eq!(limit_for(Kind::CpuPackage, None, None, None), Some(95.0));
        assert_eq!(limit_for(Kind::CpuCore, Some(100.0), None, None), Some(95.0));
        assert_eq!(limit_for(Kind::GpuCore, None, None, None), Some(88.0));
        assert_eq!(limit_for(Kind::GpuCore, None, None, Some(90.0)), Some(87.0));
        // a throttle point in T.Limit terms (a small or negative number) is not a temperature
        assert_eq!(limit_for(Kind::GpuCore, None, None, Some(-2.0)), Some(88.0));
        assert_eq!(limit_for(Kind::GpuHotspot, None, None, None), Some(103.0));
        assert_eq!(limit_for(Kind::GpuHotspot, Some(110.0), None, None), Some(105.0));
        assert_eq!(limit_for(Kind::Nvme, Some(94.85), Some(89.85), None), Some(70.0));
        assert_eq!(limit_for(Kind::Nvme, None, Some(70.0), None), Some(65.0));
        assert_eq!(limit_for(Kind::Dimm, Some(85.0), Some(55.0), None), Some(80.0));
        assert_eq!(limit_for(Kind::Other, None, None, None), None);
        assert_eq!(limit_for(Kind::DriveOther, Some(90.0), None, None), None);
    }

    #[test]
    fn device_names_lose_the_boilerplate() {
        assert_eq!(short_device("AMD Ryzen 7 7800X3D 8-Core Processor"), "AMD Ryzen 7 7800X3D");
        assert_eq!(short_device("Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz"), "Intel Core i9-9900K");
        assert_eq!(short_device("NVIDIA GeForce RTX 4090"), "RTX 4090");
    }

    #[test]
    fn an_alarm_needs_five_seconds_and_clears_under_the_hysteresis() {
        let s = Arc::new(vec![sensor("Tctl", Kind::CpuPackage, Some(95.0))]);
        let mut t = Temps::default();
        t.push(frame(0.0, Some(s), &[60.0]));
        // a spike does not alarm
        t.push(frame(1.0, None, &[99.0]));
        t.push(frame(2.0, None, &[60.0]));
        assert!(!t.any_alarm());
        let mut opened = 0;
        for k in 3..=8 {
            opened += t.push(frame(k as f64, None, &[96.0])).episode_started as u32;
        }
        assert!(t.any_alarm() && t.alarmed(0));
        assert_eq!(t.badge.heat, Heat::Alarm);
        assert_eq!(opened, 1, "the panel opens once per episode");
        // just under the limit is still the same episode
        t.push(frame(9.0, None, &[92.0]));
        assert!(t.any_alarm());
        t.push(frame(10.0, None, &[89.0]));
        assert!(!t.any_alarm());
        // cleared, though still warm this close to the limit
        assert_eq!(t.badge.heat, Heat::Warm);
        // a later episode opens again
        let mut opened = 0;
        for k in 11..=17 {
            opened += t.push(frame(k as f64, None, &[97.0])).episode_started as u32;
        }
        assert_eq!(opened, 1);
    }

    #[test]
    fn a_second_sensor_joining_an_episode_does_not_reopen() {
        let s = Arc::new(vec![sensor("a", Kind::CpuPackage, Some(95.0)), sensor("b", Kind::CpuDie, Some(95.0))]);
        let mut t = Temps::default();
        let mut opened = 0;
        for k in 0..=6 {
            opened += t.push(frame(k as f64, if k == 0 { Some(s.clone()) } else { None }, &[96.0, 60.0])).episode_started as u32;
        }
        for k in 7..=14 {
            opened += t.push(frame(k as f64, None, &[96.0, 97.0])).episode_started as u32;
        }
        assert!(t.alarmed(0) && t.alarmed(1));
        assert_eq!(opened, 1);
    }

    #[test]
    fn the_badge_warms_near_a_limit() {
        let s = Arc::new(vec![sensor("Tctl", Kind::CpuPackage, Some(95.0))]);
        let mut t = Temps::default();
        t.push(frame(0.0, Some(s), &[89.0]));
        assert_eq!(t.badge.heat, Heat::Warm);
        assert!(t.badge.level > 0.8 && t.badge.level < 1.0);
        assert!(t.badge.tooltip.contains("CPU 89"));
    }

    #[test]
    fn history_keeps_an_hour_fine_and_peaks_coarse() {
        let s = Arc::new(vec![sensor("Tctl", Kind::CpuPackage, None)]);
        let mut t = Temps::default();
        for k in 0..4000 {
            let v = if k % 100 == 50 { 90.0 } else { 50.0 };
            t.push(frame(k as f64, if k == 0 { Some(s.clone()) } else { None }, &[v]));
        }
        assert_eq!(t.fine.times.len(), FINE_CAP);
        assert_eq!(t.fine.first_time(), Some(400.0));
        assert_eq!(t.coarse.times.len(), 4000 / 30);
        // every bucket holding a spike keeps it
        assert!(t.coarse.cols[0].iter().filter(|v| **v == 90.0).count() >= 39);
        // ten minutes from the fine ring, a day from the coarse one
        assert!(std::ptr::eq(t.ring_for(3399.0, 3999.0), &t.fine));
        assert!(std::ptr::eq(t.ring_for(0.0, 3999.0), &t.coarse));
        assert_eq!(t.min_max(0, 3399.0, 3999.0), (50.0, 90.0));
        // a long view draws one point per column, keeping the peaks
        let pts = t.series(0, 3399.0, 3999.0, 10.0);
        assert!(pts.len() <= 62, "{}", pts.len());
        assert!(pts.iter().any(|p| p.1 == 90.0));
    }

    #[test]
    fn a_new_sensor_list_keeps_each_sensors_history() {
        let a = Arc::new(vec![sensor("a", Kind::CpuPackage, None), sensor("b", Kind::CpuDie, None)]);
        let mut t = Temps::default();
        t.push(frame(0.0, Some(a), &[40.0, 50.0]));
        let b = Arc::new(vec![sensor("b", Kind::CpuDie, None), sensor("c", Kind::Other, None)]);
        t.push(frame(1.0, Some(b), &[51.0, 30.0]));
        assert_eq!(t.fine.cols[0].iter().copied().collect::<Vec<_>>(), vec![50.0, 51.0]);
        assert!(t.fine.cols[1][0].is_nan());
        assert_eq!(t.fine.cols[1][1], 30.0);
    }
}
