//! All coordinates are millimetres in the robot base frame. These checks review
//! nominal geometry locally. The controller does not enforce this scene.
use crate::kinematics::{frames, Frame, JOINT_LIMITS, LINK_LOCAL_BOUNDS};
use crate::recording::Recording;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

pub type Point = [f64; 3];
pub type Pose = [f64; 6];
pub type Bounds = (Point, Point);

pub const STUDIO_POSE: Pose = [0.0, -0.52, -1.25, 0.20, 0.77, 0.25];
const CLEARANCE_MM: f64 = 12.0;
// Upper bound on the distance of ANY bare-arm mesh vertex to any upstream
// joint, including fully extended chain and housing. This converts joint
// angle travel into an upper bound on vertex displacement during a sweep.
const MAX_RADIUS_MM: f64 = 1500.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub enclosure: bool,
    pub bounds: Bounds,
}

impl Zone {
    pub fn from_corners(enclosure: bool, a: Point, b: Point) -> Result<Self, String> {
        if a.iter().chain(b.iter()).any(|x| !x.is_finite()) {
            return Err("Enter finite base-frame coordinates.".into());
        }
        if (0..3).any(|i| (a[i] - b[i]).abs() < 1.0) {
            return Err("Choose opposite corners at least 1 mm apart on all three axes.".into());
        }
        Ok(Self {
            enclosure,
            bounds: (
                std::array::from_fn(|i| a[i].min(b[i])),
                std::array::from_fn(|i| a[i].max(b[i])),
            ),
        })
    }
    fn label(&self) -> &'static str {
        if self.enclosure {
            "Enclosure"
        } else {
            "Keep-out"
        }
    }
}

pub fn parse_numbers<const N: usize>(text: &str) -> Result<[f64; N], String> {
    let mut fields = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty());
    let mut values = [0.0; N];
    for value in &mut values {
        *value = fields
            .next()
            .ok_or_else(|| format!("Enter exactly {N} coordinates."))?
            .parse::<f64>()
            .map_err(|_| "Enter numbers separated by spaces or commas.".to_owned())?;
        if !value.is_finite() {
            return Err("All coordinates must be finite.".into());
        }
    }
    if fields.next().is_some() {
        return Err(format!("Enter exactly {N} coordinates."));
    }
    Ok(values)
}

pub fn valid_pose(q: Pose) -> bool {
    q.iter()
        .zip(JOINT_LIMITS.iter())
        .all(|(&a, &(lo, hi))| a.is_finite() && (lo..=hi).contains(&a))
}

#[derive(Debug)]
pub struct Conflict {
    pub link: Option<usize>,
    pub message: String,
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

const BASE_AXES: [Point; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

/// Separating-axis test between an oriented link box and a base-frame zone.
/// The Euclidean margin also encloses the maximum travel to each end of a
/// swept interval. Touching the expanded forbidden zone counts as a conflict.
fn bounds_violate(zone: &Zone, frame: Frame, local: Bounds, margin: f64) -> bool {
    let (a, b) = zone.bounds;
    let (lo, hi) = local;
    let link_center = frame.point(std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5));
    let link_half: Point = std::array::from_fn(|i| (hi[i] - lo[i]) * 0.5);
    let zone_half: Point = std::array::from_fn(|i| (b[i] - a[i]) * 0.5);
    let delta: Point = std::array::from_fn(|i| link_center[i] - (a[i] + b[i]) * 0.5);
    let radius = |axis| {
        (0..3)
            .map(|i| link_half[i] * dot(frame.axes[i], axis).abs())
            .sum::<f64>()
    };
    if zone.enclosure {
        return (0..3).any(|i| delta[i].abs() + radius(BASE_AXES[i]) + margin > zone_half[i]);
    }
    let separated = |axis: Point| {
        let len = dot(axis, axis).sqrt();
        if len < 1e-9 {
            return false;
        }
        dot(delta, axis).abs()
            > radius(axis) + dot(zone_half, axis.map(f64::abs)) + (margin + 1e-9) * len
    };
    !BASE_AXES.iter().chain(&frame.axes).copied().any(separated)
        && !BASE_AXES
            .iter()
            .any(|&a| frame.axes.iter().any(|&b| separated(cross(a, b))))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    pub zones: Vec<Zone>,
    pub poses: Vec<Pose>,
    pub recordings: Vec<Arc<Recording>>,
}

impl Workspace {
    pub fn add_zone(&mut self, zone: Zone) -> Result<(), String> {
        if zone.enclosure && self.zones.iter().any(|z| z.enclosure) {
            return Err("Remove the existing enclosure before teaching its replacement.".into());
        }
        self.zones.push(zone);
        Ok(())
    }
    pub fn zone_names(&self) -> Vec<String> {
        self.zones
            .iter()
            .enumerate()
            .map(|(i, z)| format!("{}  ·  {:02}", z.label(), i + 1))
            .collect()
    }
    pub fn has_enclosure(&self) -> bool {
        self.zones.iter().any(|z| z.enclosure)
    }

    pub fn conflict(&self, q: Pose, extra_mm: f64) -> Option<Conflict> {
        if !valid_pose(q) {
            return Some(Conflict {
                link: None,
                message: "Joint outside the nominal UF850 range".into(),
            });
        }
        if self.zones.is_empty() {
            return None;
        }
        self.violations(q, extra_mm)
            .next()
            .map(|(zone, link)| Conflict {
                link: Some(link),
                message: self.conflict_label(zone, link),
            })
    }

    /// The measured and waypoint timelines both use the same nominal geometry.
    /// Enumerating every hit lets overlapping zones and links have independent spans.
    pub(crate) fn violations(
        &self,
        q: Pose,
        extra_mm: f64,
    ) -> impl Iterator<Item = (usize, usize)> + '_ {
        let links = frames(q);
        (0..self.zones.len())
            .flat_map(|zone| (0..LINK_LOCAL_BOUNDS.len()).map(move |link| (zone, link)))
            .filter(move |&(zone, link)| {
                bounds_violate(
                    &self.zones[zone],
                    links[link],
                    LINK_LOCAL_BOUNDS[link],
                    CLEARANCE_MM + extra_mm,
                )
            })
    }

    pub(crate) fn conflict_label(&self, zone: usize, link: usize) -> String {
        let part = if link == 0 {
            "base".to_string()
        } else {
            format!("link {link}")
        };
        if self.zones[zone].enclosure {
            format!("{part} exceeds enclosure {:02} clearance", zone + 1)
        } else {
            format!("Keep-out {:02} intersects the {part} clearance", zone + 1)
        }
    }

    /// Chords for the on-screen flange trail. The clearance checker uses its
    /// own conservative swept volumes, independently of drawing resolution.
    pub fn flange_trail(&self) -> Vec<Point> {
        let Some(&first) = self.poses.first() else {
            return Vec::new();
        };
        let mut trail = vec![crate::kinematics::joint_origins(first)[6]];
        for pair in self.poses.windows(2) {
            let steps = (total_angle(pair[0], pair[1]).to_degrees() / 3.)
                .ceil()
                .max(1.) as usize;
            for i in 1..=steps {
                let q = interpolate(pair[0], pair[1], i as f64 / steps as f64);
                trail.push(crate::kinematics::joint_origins(q)[6]);
            }
        }
        trail
    }

    pub fn review(&self) -> Result<(), String> {
        if self.poses.len() < 2 {
            return Err("Record two or more poses to review a path.".into());
        }
        for (index, pair) in self.poses.windows(2).enumerate() {
            if !valid_pose(pair[0]) || !valid_pose(pair[1]) {
                return Err("Invalid waypoint.".into());
            }
            for (mid, margin) in sweep(pair[0], pair[1]) {
                if let Some(reason) = self.conflict(mid, margin) {
                    return Err(format!("Leg {} · {}", index + 1, reason.message));
                }
            }
        }
        Ok(())
    }

    fn encode(&self, mut out: impl Write) -> io::Result<()> {
        writeln!(out, "UF850-STUDY 2")?;
        for zone in &self.zones {
            write!(
                out,
                "{}",
                if zone.enclosure {
                    "enclosure"
                } else {
                    "keepout"
                }
            )?;
            for value in zone.bounds.0.into_iter().chain(zone.bounds.1) {
                write!(out, " {value}")?;
            }
            writeln!(out)?;
        }
        for q in &self.poses {
            write!(out, "pose")?;
            for value in q {
                write!(out, " {value}")?;
            }
            writeln!(out)?;
        }
        for recording in &self.recordings {
            recording.write_to(&mut out)?;
        }
        writeln!(out, "end-study")?;
        out.flush()
    }
    fn decode(mut source: impl BufRead) -> Result<Self, String> {
        let mut line = String::new();
        let header = read_line(&mut source, &mut line)?;
        let legacy = match header {
            Some("UF850-STUDY 1") => true,
            Some("UF850-STUDY 2") => false,
            _ => return Err("Unknown study format.".into()),
        };
        let mut cell = Self::default();
        while let Some(row) = read_line(&mut source, &mut line)? {
            if !legacy && row == "end-study" {
                return if read_line(&mut source, &mut line)?.is_none() {
                    Ok(cell)
                } else {
                    Err("Unexpected data after end of study.".into())
                };
            }
            let (kind, coords) = row.split_once(' ').ok_or("Incomplete study record.")?;
            if !legacy && kind == "take" {
                cell.recordings
                    .push(Arc::new(Recording::read_from(coords, &mut source)?));
                continue;
            }
            let v = parse_numbers::<6>(coords)?;
            match kind {
                "pose" if valid_pose(v) => cell.poses.push(v),
                "enclosure" | "keepout" => cell.add_zone(Zone::from_corners(
                    kind == "enclosure",
                    [v[0], v[1], v[2]],
                    [v[3], v[4], v[5]],
                )?)?,
                _ => return Err("Unrecognised or out-of-range study record.".into()),
            }
        }
        if legacy {
            Ok(cell)
        } else {
            Err("Incomplete study.".into())
        }
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|e| format!("Could not open study · {e}"))?;
        Self::decode(BufReader::new(file))
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().ok_or("Missing study directory.")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = path.with_extension(format!("{}.pending", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        let result = self
            .encode(BufWriter::new(&file))
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&tmp, path));
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result.map_err(|e| e.to_string())
    }
}

pub(crate) fn read_line<'a>(
    reader: &mut impl BufRead,
    buf: &'a mut String,
) -> Result<Option<&'a str>, String> {
    buf.clear();
    let len = reader.read_line(buf).map_err(|e| e.to_string())?;
    Ok((len != 0).then(|| buf.trim_end_matches(['\r', '\n'])))
}

/// Inflate the midpoint geometry in each joint-linear subinterval by the
/// maximum distance any link vertex can travel to either end.
pub(crate) fn sweep(a: Pose, b: Pose) -> impl ExactSizeIterator<Item = (Pose, f64)> {
    let travel = total_angle(a, b) * MAX_RADIUS_MM;
    let steps = (travel / 10.).ceil().max(1.) as usize;
    let margin = travel / (2. * steps as f64);
    (0..steps).map(move |i| (interpolate(a, b, (i as f64 + 0.5) / steps as f64), margin))
}

fn total_angle(a: Pose, b: Pose) -> f64 {
    (0..6).map(|i| (a[i] - b[i]).abs()).sum()
}
pub fn interpolate(a: Pose, b: Pose, t: f64) -> Pose {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t.clamp(0., 1.))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corners_and_clearance() {
        let obstacle = Zone::from_corners(false, [10.; 3], [-10.; 3]).unwrap();
        assert!(bounds_violate(
            &obstacle,
            frames([0.; 6])[0],
            ([15.; 3], [23.; 3]),
            12.
        ));
        assert!(!bounds_violate(
            &obstacle,
            frames([0.; 6])[0],
            ([23.; 3], [24.; 3]),
            12.
        ));
        assert!(Zone::from_corners(false, [0.; 3], [2., 0., 5.]).is_err());
    }
    #[test]
    fn rotated_link_leaves_empty_corners_of_its_world_bounds_free() {
        let c = std::f64::consts::FRAC_1_SQRT_2;
        let frame = Frame {
            origin: [0.; 3],
            axes: [[c, c, 0.], [-c, c, 0.], [0., 0., 1.]],
        };
        let link = ([-100., -5., -5.], [100., 5., 5.]);
        let clear = Zone::from_corners(false, [45., -55., -5.], [55., -45., 5.]).unwrap();
        let blocked = Zone::from_corners(false, [45., 45., -5.], [55., 55., 5.]).unwrap();
        assert!(!bounds_violate(&clear, frame, link, 12.));
        assert!(bounds_violate(&blocked, frame, link, 12.));
    }
    #[test]
    fn studies_validate_before_replacing() {
        let mut a = Workspace::default();
        a.poses.push(STUDIO_POSE);
        a.add_zone(Zone::from_corners(true, [-1000.; 3], [1000.; 3]).unwrap())
            .unwrap();
        let mut bytes = Vec::new();
        a.encode(&mut bytes).unwrap();
        assert_eq!(Workspace::decode(bytes.as_slice()).unwrap(), a);
        assert!(Workspace::decode("UF850-STUDY 1\npose NaN 1 2 3 4 5\n".as_bytes()).is_err());
        assert!(a.add_zone(a.zones[0].clone()).is_err());
    }
    #[test]
    fn saves_multiple_takes_and_loads_old_studies() {
        let v1 = b"UF850-STUDY 1\nenclosure -1000 -1000 -1000 1000 1000 1000\npose 0 0 -1 0 0 0\n";
        let mut study = Workspace::decode(&v1[..]).unwrap();
        assert!(study.recordings.is_empty());
        let mut take = Recording::example();
        take.samples.truncate(4);
        take.reason = "manual stop\r\nreason \\ x · 1".into();
        study.recordings.push(Arc::new(take.clone()));
        take.example = false;
        take.samples.truncate(1);
        study.recordings.push(Arc::new(take));
        let mut bytes = Vec::new();
        study.encode(&mut bytes).unwrap();
        assert_eq!(Workspace::decode(&bytes[..]).unwrap(), study);
        let valid = String::from_utf8(bytes).unwrap();
        let mut incomplete = valid.replace("take example 4", "take example 2");
        assert!(Workspace::decode(incomplete.as_bytes()).is_err());
        incomplete = valid.replace("end-study", "");
        assert!(Workspace::decode(incomplete.as_bytes()).is_err());
        incomplete = valid.replace("sample 4000 ", "sample 0 ");
        assert!(Workspace::decode(incomplete.as_bytes()).is_err());
        incomplete = valid.replace("take example 4", "take example 99999999999999");
        assert!(Workspace::decode(incomplete.as_bytes()).is_err());
        incomplete = valid + "extra data";
        assert!(Workspace::decode(incomplete.as_bytes()).is_err());
    }

    #[test]
    fn review_checks_intermediate_geometry() {
        let mut start = STUDIO_POSE;
        let mut end = STUDIO_POSE;
        start[0] = -1.0;
        end[0] = 1.0;
        let tip = crate::kinematics::joint_origins(interpolate(start, end, 0.5))[6];
        let zone = Zone::from_corners(false, tip.map(|v| v - 2.), tip.map(|v| v + 2.)).unwrap();
        let w = Workspace {
            zones: vec![zone],
            poses: vec![start, end],
            ..Workspace::default()
        };
        assert!(w.conflict(start, 0.).is_none() && w.conflict(end, 0.).is_none());
        assert!(w.review().is_err());
    }
}
