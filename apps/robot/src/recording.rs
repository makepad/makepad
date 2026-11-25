//! A take contains one continuous run of measured actual-port samples. Playback
//! is addressed by controller time. Its interpolated poses are for preview only.
use crate::kinematics::joint_origins;
use crate::workspace::{
    interpolate, parse_numbers, read_line, sweep, valid_pose, Point, Pose, Workspace, Zone,
    STUDIO_POSE,
};
use std::io::{self, BufRead, Write};

// Port 30000 is 250 Hz, or 200 Hz with the FT sensor. Use controller time so
// bursts of TCP deliveries and UI scheduling never split a continuous capture.
// https://docs.supportarticle.ufactory.cc/support_articles/developer/firmware/data-description-of-tcp-port.html
pub const MAX_SAMPLE_GAP_US: u64 = 7_500;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    pub time_us: u64,
    pub joints: Pose,
    pub tcp_mm: Point,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub samples: Vec<FrameSample>,
    pub tool_offset: Pose,
    pub reason: String,
    pub example: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClearanceEvent {
    pub time_us: u64,
    pub end_us: u64,
    pub link: Option<usize>,
    pub message: String,
}

impl Recording {
    pub fn validate(&self) -> Result<(), String> {
        if self.samples.first().map(|s| s.time_us) != Some(0) {
            return Err("A recording must start with a sample at time zero.".into());
        }
        if !self.tool_offset.iter().all(|v| v.is_finite()) {
            return Err("Invalid recorded tool offset.".into());
        }
        for sample in &self.samples {
            if !valid_pose(sample.joints) || !sample.tcp_mm.iter().all(|v| v.is_finite()) {
                return Err("Invalid recorded joint angles or tool position.".into());
            }
        }
        for pair in self.samples.windows(2) {
            let delta = pair[1].time_us.checked_sub(pair[0].time_us);
            if !matches!(delta, Some(1..=MAX_SAMPLE_GAP_US)) {
                return Err("Recording contains a timestamp discontinuity.".into());
            }
        }
        Ok(())
    }

    pub fn duration_us(&self) -> u64 {
        self.samples.last().map_or(0, |s| s.time_us)
    }

    /// Call on a validated take. Exact sample times preserve all measured
    /// values; positions between adjacent packets use time-linear interpolation.
    pub fn at(&self, time_us: u64) -> FrameSample {
        match self.samples.binary_search_by_key(&time_us, |s| s.time_us) {
            Ok(i) => self.samples[i],
            Err(0) => self.samples[0],
            Err(i) if i == self.samples.len() => self.samples[i - 1],
            Err(i) => {
                let (a, b) = (self.samples[i - 1], self.samples[i]);
                let t = (time_us - a.time_us) as f64 / (b.time_us - a.time_us) as f64;
                FrameSample {
                    time_us,
                    joints: interpolate(a.joints, b.joints, t),
                    tcp_mm: std::array::from_fn(|j| a.tcp_mm[j] * (1.0 - t) + b.tcp_mm[j] * t),
                }
            }
        }
    }

    /// Draw a bounded trail of measured TCP samples while preserving endpoints.
    pub fn tcp_trail(&self, max_points: usize) -> Vec<Point> {
        let count = self.samples.len().min(max_points);
        if count < 2 {
            return self.samples.iter().take(count).map(|s| s.tcp_mm).collect();
        }
        (0..count)
            .map(|i| {
                self.samples
                    [(i as u128 * (self.samples.len() - 1) as u128 / (count - 1) as u128) as usize]
                    .tcp_mm
            })
            .collect()
    }

    /// Check every joint-linear subinterval. The TCP point is drawn separately;
    /// the verified geometry here is the nominal bare arm plus its clearance.
    pub fn events(&self, workspace: &Workspace) -> Vec<ClearanceEvent> {
        let mut events: Vec<ClearanceEvent> = Vec::new();
        if workspace.zones.is_empty() || self.samples.is_empty() {
            return events;
        }
        let mut recent = vec![[None::<usize>; 7]; workspace.zones.len()];
        let mut check = |q, margin, start, end| {
            for (zone, link) in workspace.violations(q, margin) {
                if let Some(previous) = recent[zone][link] {
                    if events[previous].end_us == start {
                        events[previous].end_us = end;
                        continue;
                    }
                }
                recent[zone][link] = Some(events.len());
                events.push(ClearanceEvent {
                    time_us: start,
                    end_us: end,
                    link: Some(link),
                    message: workspace.conflict_label(zone, link),
                });
            }
        };
        if self.samples.len() == 1 {
            check(self.samples[0].joints, 0.0, 0, 0);
        }
        let mut i = 0;
        while i + 1 < self.samples.len() {
            let a = self.samples[i];
            i += 1;
            let b = self.samples[i];
            if a.joints == b.joints {
                // The controller keeps reporting while the arm is held still.
                // Check this one fixed transform for the whole stationary run.
                while i + 1 < self.samples.len() && self.samples[i + 1].joints == a.joints {
                    i += 1;
                }
                check(a.joints, 0.0, a.time_us, self.samples[i].time_us);
                continue;
            }
            let intervals = sweep(a.joints, b.joints);
            let steps = intervals.len() as u64;
            let span = b.time_us - a.time_us;
            for (i, (q, margin)) in intervals.enumerate() {
                // Validated report spacing is <= MAX_SAMPLE_GAP_US. The
                // integer division makes the final boundary the exact stamp.
                let start = a.time_us + i as u64 * span / steps;
                let end = a.time_us + (i as u64 + 1) * span / steps;
                check(q, margin, start, end);
            }
        }
        events
    }

    /// Synthetic eight-second teaching session at 250 Hz for offline review.
    pub fn example() -> Self {
        let samples = (0..=2000)
            .map(|i| {
                let t = i as f64 / 2000.0;
                let bend = (t * std::f64::consts::TAU).sin();
                let mut joints = STUDIO_POSE;
                joints[0] = -0.8 * (t * std::f64::consts::PI).cos();
                joints[1] += 0.16 * bend;
                joints[2] -= 0.22 * bend;
                joints[4] += 0.12 * bend;
                FrameSample {
                    time_us: i * 4_000,
                    joints,
                    tcp_mm: joint_origins(joints)[6],
                }
            })
            .collect();
        Self {
            samples,
            tool_offset: [0.0; 6],
            reason: "End of synthetic example".into(),
            example: true,
        }
    }

    pub fn example_zone() -> Zone {
        let center = joint_origins(STUDIO_POSE)[6];
        Zone {
            enclosure: false,
            bounds: (center.map(|v| v - 26.), center.map(|v| v + 26.)),
        }
    }

    pub(crate) fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        self.validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(
            out,
            "take {} {}",
            if self.example { "example" } else { "measured" },
            self.samples.len()
        )?;
        write!(out, "offset")?;
        for v in self.tool_offset {
            write!(out, " {v}")?;
        }
        writeln!(out)?;
        let reason = self
            .reason
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        writeln!(out, "reason {reason}")?;
        for s in &self.samples {
            write!(out, "sample {}", s.time_us)?;
            for v in s.joints.into_iter().chain(s.tcp_mm) {
                write!(out, " {v}")?;
            }
            writeln!(out)?;
        }
        writeln!(out, "end-take")
    }

    pub(crate) fn read_from(header: &str, reader: &mut impl BufRead) -> Result<Self, String> {
        let (kind, count) = header
            .split_once(' ')
            .ok_or("Incomplete recording header.")?;
        let example = match kind {
            "measured" => false,
            "example" => true,
            _ => return Err("Unknown recording source.".into()),
        };
        let count: usize = count
            .parse()
            .map_err(|_| "Invalid recording sample count.")?;
        let mut line = String::new();
        let offset = read_line(reader, &mut line)?
            .and_then(|s| s.strip_prefix("offset "))
            .ok_or("Missing recorded tool offset.")?;
        let tool_offset = parse_numbers(offset)?;
        let encoded = read_line(reader, &mut line)?
            .and_then(|s| s.strip_prefix("reason "))
            .ok_or("Missing recording stop reason.")?;
        let mut reason = String::with_capacity(encoded.len());
        let mut chars = encoded.chars();
        while let Some(c) = chars.next() {
            reason.push(if c == '\\' {
                match chars.next() {
                    Some('n') => '\n',
                    Some('r') => '\r',
                    Some('\\') => '\\',
                    _ => return Err("Invalid escape in recording stop reason.".into()),
                }
            } else {
                c
            });
        }
        let mut samples = Vec::new();
        // Stream the actual rows. The untrusted count never determines an allocation.
        for _ in 0..count {
            let row = read_line(reader, &mut line)?
                .and_then(|s| s.strip_prefix("sample "))
                .ok_or("Incomplete recorded session.")?;
            let (stamp, values) = row.split_once(' ').ok_or("Incomplete recorded sample.")?;
            let time_us = stamp.parse().map_err(|_| "Invalid sample timestamp.")?;
            let [j1, j2, j3, j4, j5, j6, x, y, z] = parse_numbers(values)?;
            samples.push(FrameSample {
                time_us,
                joints: [j1, j2, j3, j4, j5, j6],
                tcp_mm: [x, y, z],
            });
        }
        if read_line(reader, &mut line)? != Some("end-take") {
            return Err("Incomplete recording or incorrect sample count.".into());
        }
        let take = Self {
            samples,
            tool_offset,
            reason,
            example,
        };
        take.validate()?;
        Ok(take)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(time_us: u64, base_angle: f64, tcp_mm: Point) -> FrameSample {
        let mut joints = STUDIO_POSE;
        joints[0] = base_angle;
        FrameSample {
            time_us,
            joints,
            tcp_mm,
        }
    }

    fn measured(samples: Vec<FrameSample>) -> Recording {
        Recording {
            samples,
            tool_offset: [0.; 6],
            reason: "Stopped by operator".into(),
            example: false,
        }
    }

    #[test]
    fn replay_uses_packet_time_and_measured_tcp() {
        let first = frame(0, 0., [1., 2., 3.]);
        let second = frame(4000, 1., [21., 42., 63.]);
        let third = frame(9000, 2., [31., 52., 83.]);
        let take = measured(vec![first, second, third]);
        assert!(take.validate().is_ok());
        assert_eq!(take.at(0), first);
        assert_eq!(take.at(4000), second);
        assert_eq!(take.at(90_000), third);
        assert_eq!(take.at(2000).tcp_mm, [11., 22., 33.]);
        assert_eq!(take.at(6500).joints[0], 1.5);
        assert_eq!(take.tcp_trail(2), vec![first.tcp_mm, third.tcp_mm]);
        assert_eq!(take.tcp_trail(1), vec![first.tcp_mm]);
        assert!(take.tcp_trail(0).is_empty());
    }

    #[test]
    fn recordings_require_contiguous_time_and_valid_measurements() {
        let f = frame(0, 0., [0.; 3]);
        for t in [0, MAX_SAMPLE_GAP_US + 1] {
            assert!(measured(vec![f, FrameSample { time_us: t, ..f }])
                .validate()
                .is_err());
        }
        assert!(measured(vec![FrameSample { time_us: 4000, ..f }])
            .validate()
            .is_err());
        assert!(measured(vec![FrameSample {
            tcp_mm: [f64::NAN; 3],
            ..f
        }])
        .validate()
        .is_err());
        assert!(measured(vec![FrameSample {
            joints: [100.; 6],
            ..f
        }])
        .validate()
        .is_err());
        assert!(measured(vec![]).validate().is_err());
        let mut take = measured(vec![f]);
        take.tool_offset[0] = f64::INFINITY;
        assert!(take.validate().is_err());
    }

    #[test]
    fn timeline_finds_swept_conflicts_between_clear_samples_and_rechecks_zones() {
        let mut cell = Workspace::default();
        let take = measured(vec![frame(0, -1., [1.; 3]), frame(5000, 1., [7.; 3])]);
        assert!(take.events(&cell).is_empty());
        cell.add_zone(Recording::example_zone()).unwrap();
        assert!(take
            .samples
            .iter()
            .all(|f| cell.conflict(f.joints, 0.).is_none()));
        let events = take.events(&cell);
        let middle = events
            .iter()
            .find(|e| e.time_us <= 2500 && e.end_us >= 2500)
            .unwrap();
        assert!(middle.link.is_some() && middle.time_us > 0 && middle.end_us < 5000);
        assert!(middle.message.contains("Keep-out 01"));
        cell.zones.clear();
        assert!(take.events(&cell).is_empty());
    }

    #[test]
    fn overlapping_link_and_zone_conflicts_merge_into_independent_spans() {
        let f = frame(0, 0., [0.; 3]);
        let take = measured(vec![
            f,
            FrameSample { time_us: 4000, ..f },
            FrameSample { time_us: 9000, ..f },
        ]);
        let mut cell = Workspace::default();
        cell.add_zone(Zone::from_corners(false, [-3000.; 3], [3000.; 3]).unwrap())
            .unwrap();
        cell.add_zone(Recording::example_zone()).unwrap();
        let events = take.events(&cell);
        assert!(events.len() > 7);
        assert!(events.iter().all(|e| e.time_us == 0 && e.end_us == 9000));
        assert!(events.iter().any(|e| e.message.contains("Keep-out 02")));
        let single = measured(vec![f]).events(&cell);
        assert_eq!(single.len(), events.len());
        assert!(single.iter().all(|e| e.time_us == 0 && e.end_us == 0));
    }

    #[test]
    fn example_produces_time_localized_events() {
        let take = Recording::example();
        assert!(take.example && take.validate().is_ok());
        assert_eq!(take.duration_us(), 8_000_000);
        let mut cell = Workspace::default();
        cell.add_zone(Recording::example_zone()).unwrap();
        assert!(take
            .events(&cell)
            .iter()
            .any(|e| e.time_us > 0 && e.end_us < take.duration_us()));
    }
}
