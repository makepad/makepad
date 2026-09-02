//! Body measurements from a body mesh.
//!
//! Input: a closed triangle mesh of a standing body in a rest pose (arms
//! away from the torso), Y up, in CENTIMETRES, plus optional landmarks.
//! Output: the 25-key measurement schema a pattern drafter needs, and the
//! rings and lines that were measured so a viewer can draw them on the body.
//!
//! No mesh topology knowledge is assumed beyond "closed manifold": every
//! landmark is found geometrically (see `measure`).

use std::{cmp::Ordering, collections::HashMap, fmt};

/// A closed triangle mesh in centimetres, Y up, standing on y ≈ min.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyMesh {
    pub vertices: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
    /// Known joint positions, when the producer has them. Purely a hint:
    /// `measure` must work with `None` (geometric landmarking).
    pub landmarks: Option<Landmarks>,
}

/// Joint positions in the mesh's frame, centimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landmarks {
    pub neck: [f32; 3],
    pub left_shoulder: [f32; 3],
    pub right_shoulder: [f32; 3],
    pub left_elbow: [f32; 3],
    pub right_elbow: [f32; 3],
    pub left_wrist: [f32; 3],
    pub right_wrist: [f32; 3],
    pub left_hip: [f32; 3],
    pub right_hip: [f32; 3],
    pub left_knee: [f32; 3],
    pub right_knee: [f32; 3],
    pub left_ankle: [f32; 3],
    pub right_ankle: [f32; 3],
}

/// The 25 measurements, all in centimetres. Circumferences are TAPE
/// measurements (convex hull of the slice, not the skin path).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measurements {
    pub height: f32,
    pub neck: f32,
    pub shoulder_width: f32,
    pub bust: f32,
    pub underbust: f32,
    pub waist: f32,
    pub high_hip: f32,
    pub hip: f32,
    pub bicep: f32,
    pub wrist: f32,
    pub thigh: f32,
    pub knee: f32,
    pub calf: f32,
    pub ankle: f32,
    pub arm_length: f32,
    pub upper_arm_length: f32,
    pub back_waist_length: f32,
    pub front_waist_length: f32,
    pub shoulder_to_bust: f32,
    pub inseam: f32,
    pub outseam: f32,
    pub crotch_depth: f32,
    pub waist_to_hip: f32,
    pub waist_to_knee: f32,
    pub waist_to_floor: f32,
}

/// Stable key names, in schema order.
pub const MEASUREMENT_KEYS: [&str; 25] = [
    "height",
    "neck",
    "shoulder_width",
    "bust",
    "underbust",
    "waist",
    "high_hip",
    "hip",
    "bicep",
    "wrist",
    "thigh",
    "knee",
    "calf",
    "ankle",
    "arm_length",
    "upper_arm_length",
    "back_waist_length",
    "front_waist_length",
    "shoulder_to_bust",
    "inseam",
    "outseam",
    "crotch_depth",
    "waist_to_hip",
    "waist_to_knee",
    "waist_to_floor",
];

impl Measurements {
    /// `(key, value)` in schema order.
    pub fn entries(&self) -> [(&'static str, f32); 25] {
        [
            ("height", self.height),
            ("neck", self.neck),
            ("shoulder_width", self.shoulder_width),
            ("bust", self.bust),
            ("underbust", self.underbust),
            ("waist", self.waist),
            ("high_hip", self.high_hip),
            ("hip", self.hip),
            ("bicep", self.bicep),
            ("wrist", self.wrist),
            ("thigh", self.thigh),
            ("knee", self.knee),
            ("calf", self.calf),
            ("ankle", self.ankle),
            ("arm_length", self.arm_length),
            ("upper_arm_length", self.upper_arm_length),
            ("back_waist_length", self.back_waist_length),
            ("front_waist_length", self.front_waist_length),
            ("shoulder_to_bust", self.shoulder_to_bust),
            ("inseam", self.inseam),
            ("outseam", self.outseam),
            ("crotch_depth", self.crotch_depth),
            ("waist_to_hip", self.waist_to_hip),
            ("waist_to_knee", self.waist_to_knee),
            ("waist_to_floor", self.waist_to_floor),
        ]
    }

    pub fn get(&self, key: &str) -> Option<f32> {
        self.entries().iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    pub fn set(&mut self, key: &str, value: f32) -> bool {
        let slot = match key {
            "height" => &mut self.height,
            "neck" => &mut self.neck,
            "shoulder_width" => &mut self.shoulder_width,
            "bust" => &mut self.bust,
            "underbust" => &mut self.underbust,
            "waist" => &mut self.waist,
            "high_hip" => &mut self.high_hip,
            "hip" => &mut self.hip,
            "bicep" => &mut self.bicep,
            "wrist" => &mut self.wrist,
            "thigh" => &mut self.thigh,
            "knee" => &mut self.knee,
            "calf" => &mut self.calf,
            "ankle" => &mut self.ankle,
            "arm_length" => &mut self.arm_length,
            "upper_arm_length" => &mut self.upper_arm_length,
            "back_waist_length" => &mut self.back_waist_length,
            "front_waist_length" => &mut self.front_waist_length,
            "shoulder_to_bust" => &mut self.shoulder_to_bust,
            "inseam" => &mut self.inseam,
            "outseam" => &mut self.outseam,
            "crotch_depth" => &mut self.crotch_depth,
            "waist_to_hip" => &mut self.waist_to_hip,
            "waist_to_knee" => &mut self.waist_to_knee,
            "waist_to_floor" => &mut self.waist_to_floor,
            _ => return false,
        };
        *slot = value;
        true
    }

    /// A plausible average adult body, for previews before any photo.
    pub fn sample() -> Self {
        Measurements {
            height: 172.0,
            neck: 38.0,
            shoulder_width: 42.0,
            bust: 96.0,
            underbust: 84.0,
            waist: 82.0,
            high_hip: 90.0,
            hip: 100.0,
            bicep: 31.0,
            wrist: 17.0,
            thigh: 58.0,
            knee: 39.0,
            calf: 38.0,
            ankle: 24.0,
            arm_length: 60.0,
            upper_arm_length: 33.0,
            back_waist_length: 44.0,
            front_waist_length: 42.0,
            shoulder_to_bust: 26.0,
            inseam: 78.0,
            outseam: 104.0,
            crotch_depth: 26.0,
            waist_to_hip: 20.0,
            waist_to_knee: 58.0,
            waist_to_floor: 104.0,
        }
    }
}

/// How to measure.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeasureOptions {
    /// The person's real height in cm. When given, the mesh is rescaled so
    /// its height matches before anything is measured (monocular
    /// reconstructions get scale wrong; the tape does not).
    pub height_cm: Option<f32>,
}

/// One horizontal slice through the body: the closed loop, its height,
/// and the two perimeters (skin path and tape/convex hull).
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    pub key: &'static str,
    pub y_cm: f32,
    /// Ordered closed polyline, centimetres.
    pub points: Vec<[f32; 3]>,
    pub skin_perimeter_cm: f32,
    pub tape_perimeter_cm: f32,
}

/// A straight measured length, for drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub key: &'static str,
    pub from: [f32; 3],
    pub to: [f32; 3],
}

/// Everything `measure` found.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    pub values: Measurements,
    /// The scale applied to the mesh (1.0 when no height was given).
    pub scale: f32,
    pub rings: Vec<Ring>,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MeasureError {
    EmptyMesh,
    NotAStandingBody(String),
    NotImplemented,
}

impl fmt::Display for MeasureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeasureError::EmptyMesh => write!(f, "the body mesh is empty"),
            MeasureError::NotAStandingBody(why) => write!(f, "not a standing body: {why}"),
            MeasureError::NotImplemented => write!(f, "measuring is not implemented yet"),
        }
    }
}

impl std::error::Error for MeasureError {}

/// Measure a standing body. See the crate docs for the method.
pub fn measure(mesh: &BodyMesh, options: &MeasureOptions) -> Result<Measured, MeasureError> {
    if mesh.vertices.is_empty() || mesh.faces.is_empty() {
        return Err(MeasureError::EmptyMesh);
    }

    let (mesh, scale, floor, top) = scaled_mesh(mesh, options)?;
    let height = top - floor;
    if !(100.0..=250.0).contains(&height) || !height.is_finite() {
        return Err(MeasureError::NotAStandingBody(format!(
            "height {height:.1} cm is outside 100..=250 cm"
        )));
    }

    // First estimate the vertical axis from the whole mesh, then move it to
    // the centre of the largest mid-hip section.  Translation of the input is
    // deliberately irrelevant from this point onward.
    let mut axis = mesh_xz_centroid(&mesh);
    let hip_seed_y = floor + height * 0.55;
    if let Some(info) = largest_loop_at(&mesh, hip_seed_y) {
        axis = info.centroid;
    }

    let mut scan = Vec::with_capacity(height.ceil() as usize + 1);
    let mut y = floor;
    while y < top {
        scan.push(ScanSlice::new(&mesh, y, axis));
        y += 1.0;
    }
    if scan.last().is_none_or(|slice| top - slice.y > 0.25) {
        scan.push(ScanSlice::new(&mesh, top - SLICE_NUDGE, axis));
    }

    let crotch_index = find_crotch(&scan, floor + height * 0.55, axis).ok_or_else(|| {
        MeasureError::NotAStandingBody("no crotch split was found".to_string())
    })?;
    let crotch_y = scan[crotch_index].y;

    let armpit_index = find_armpit(&scan, crotch_index, axis, mesh.landmarks.as_ref()).ok_or_else(
        || MeasureError::NotAStandingBody("no arm merge was found".to_string()),
    )?;
    let armpit_y = scan[armpit_index].y;

    let shoulder = torso_extreme(
        &scan,
        armpit_y,
        (armpit_y + 12.0).min(top),
        Extreme::XExtent,
    )
    .or_else(|| nearest_torso(&scan, armpit_y))
    .expect("an arm merge always has an enclosing loop");
    let shoulder_y = scan[shoulder.0].y;
    let shoulder_loop = &scan[shoulder.0].loops[shoulder.1];
    let shoulder_width = (shoulder_loop.max_x - shoulder_loop.min_x - 3.0).max(0.0);
    let left_shoulder = inset_extreme_x(shoulder_loop, true, 1.5, shoulder_y);
    let right_shoulder = inset_extreme_x(shoulder_loop, false, 1.5, shoulder_y);

    let neck = torso_extreme(
        &scan,
        shoulder_y,
        (shoulder_y + 18.0).min(top),
        Extreme::MinTape,
    )
    .or_else(|| nearest_torso(&scan, shoulder_y))
    .expect("the shoulder slice has an enclosing loop");
    let neck_y = scan[neck.0].y;

    let bust = torso_extreme(
        &scan,
        armpit_y - 22.0,
        armpit_y - 4.0,
        Extreme::MaxTape,
    )
    .or_else(|| torso_extreme(&scan, crotch_y + 5.0, armpit_y, Extreme::MaxTape))
    .expect("a standing torso has a bust section");
    let bust_y = scan[bust.0].y;

    let underbust = torso_extreme(
        &scan,
        bust_y - 14.0,
        bust_y - 3.0,
        Extreme::MinTape,
    )
    .or_else(|| nearest_torso(&scan, bust_y - 7.0))
    .expect("a standing torso has an underbust section");
    let underbust_y = scan[underbust.0].y;

    let waist = torso_extreme(
        &scan,
        underbust_y - 20.0,
        underbust_y - 3.0,
        Extreme::MinTape,
    )
    .or_else(|| {
        torso_extreme(
            &scan,
            crotch_y + 10.0,
            bust_y,
            Extreme::MinTape,
        )
    })
    .or_else(|| nearest_torso(&scan, (crotch_y + bust_y) * 0.5))
    .expect("a standing torso has a waist section");
    let waist_y = scan[waist.0].y;

    // At coarse one-centimetre resolution the literal hip window can contain
    // only the first post-split slice.  Use it when it has enough samples;
    // otherwise extend the same search upward to the waist.
    let hip_low = waist_y - 30.0;
    let literal_hip_candidates = torso_candidates(&scan, hip_low, crotch_y).count();
    let hip_hint = mesh
        .landmarks
        .as_ref()
        .map(|lm| (lm.left_hip[1] + lm.right_hip[1]) * 0.5);
    let hip = hip_hint
        .and_then(|hint| {
            let low = (hint - 10.0).max(hip_low);
            let high = (hint + 10.0).min(waist_y);
            (low <= high)
                .then(|| torso_extreme(&scan, low, high, Extreme::MaxTape))
                .flatten()
        })
        .or_else(|| {
            if literal_hip_candidates >= 3 {
                torso_extreme(&scan, hip_low, crotch_y, Extreme::MaxTape)
            } else {
                torso_extreme(&scan, hip_low.max(crotch_y), waist_y, Extreme::MaxTape)
            }
        })
    .or_else(|| torso_extreme(&scan, hip_low, waist_y, Extreme::MaxTape))
    .or_else(|| nearest_torso(&scan, crotch_y + 1.0))
    .expect("a standing torso has a hip section");
    let hip_y = scan[hip.0].y;
    let high_hip_y = waist_y - (waist_y - hip_y) * 0.5;
    let high_hip = nearest_torso(&scan, high_hip_y).expect("hip and waist bound a torso section");

    let leg_length = (crotch_y - floor).max(1.0);
    let thigh = leg_extreme(
        &scan,
        crotch_y - 10.0,
        crotch_y - 1.0,
        axis,
        Extreme::MaxTape,
    )
    .or_else(|| leg_nearest(&scan, crotch_y - 2.0, axis))
    .expect("a crotch split has a left leg");
    let knee_hint = mesh
        .landmarks
        .as_ref()
        .map(|lm| lm.left_knee[1].min(lm.right_knee[1]));
    let knee_low = floor + 0.40 * leg_length;
    let knee_high = floor + 0.60 * leg_length;
    let knee = knee_hint
        .and_then(|hint| {
            let low = (hint - 8.0).max(knee_low);
            let high = (hint + 8.0).min(knee_high);
            (low <= high)
                .then(|| leg_extreme(&scan, low, high, axis, Extreme::MinTape))
                .flatten()
        })
        .or_else(|| leg_extreme(&scan, knee_low, knee_high, axis, Extreme::MinTape))
    .expect("a standing body has a knee section");
    let knee_y = scan[knee.0].y;
    let calf = leg_extreme(
        &scan,
        floor + 0.18 * leg_length,
        knee_y,
        axis,
        Extreme::MaxTape,
    )
    .or_else(|| leg_nearest(&scan, (floor + knee_y) * 0.5, axis))
    .expect("a standing body has a calf section");
    let ankle_hint = mesh
        .landmarks
        .as_ref()
        .map(|lm| lm.left_ankle[1].min(lm.right_ankle[1]));
    let ankle_low = floor + 5.0;
    let ankle_high = floor + 0.15 * leg_length;
    let ankle = ankle_hint
        .and_then(|hint| {
            let low = (hint - 4.0).max(ankle_low);
            let high = (hint + 4.0).min(ankle_high);
            (low <= high)
                .then(|| leg_extreme(&scan, low, high, axis, Extreme::MinTape))
                .flatten()
        })
        .or_else(|| leg_extreme(&scan, ankle_low, ankle_high, axis, Extreme::MinTape))
    .expect("a standing body has an ankle section");

    let bicep_seed = arm_extreme(
        &scan,
        armpit_y - 12.0,
        armpit_y - 2.0,
        axis,
        Extreme::MaxTape,
    )
    .ok_or_else(|| MeasureError::NotAStandingBody("no left arm section was found".to_string()))?;
    let arm_track = trace_arm(&scan, bicep_seed, axis);
    let bicep = track_extreme(
        &scan,
        &arm_track,
        armpit_y - 12.0,
        armpit_y - 2.0,
        Extreme::MaxTape,
    )
    .unwrap_or(bicep_seed);
    let bicep_y = scan[bicep.0].y;
    let lowest_arm_y = arm_track
        .iter()
        .map(|(index, _)| scan[*index].y)
        .fold(bicep_y, f32::min);
    let wrist_hint = mesh
        .landmarks
        .as_ref()
        .map(|lm| lm.left_wrist[1].min(lm.right_wrist[1]));
    let wrist = wrist_hint
        .and_then(|hint| {
            track_extreme(
                &scan,
                &arm_track,
                hint - 5.0,
                hint + 5.0,
                Extreme::MinTape,
            )
        })
        .or_else(|| {
            track_extreme(
                &scan,
                &arm_track,
                lowest_arm_y + 12.0,
                lowest_arm_y + 30.0,
                Extreme::MinTape,
            )
        })
    .unwrap_or_else(|| *arm_track.first().unwrap_or(&bicep));
    let wrist_y = scan[wrist.0].y;
    let elbow = track_extreme(
        &scan,
        &arm_track,
        wrist_y + 8.0,
        bicep_y,
        Extreme::MinTape,
    )
    .or_else(|| {
        mesh.landmarks.as_ref().and_then(|lm| {
            let hint = lm.left_elbow[1].min(lm.right_elbow[1]);
            track_nearest(&scan, &arm_track, hint)
        })
    })
    .unwrap_or(bicep);

    let elbow_point = loop_center_point(&scan[elbow.0], elbow.1);
    let wrist_point = loop_center_point(&scan[wrist.0], wrist.1);
    let upper_arm_length = distance3(left_shoulder, elbow_point);
    let arm_length = upper_arm_length + distance3(elbow_point, wrist_point);

    let front_sign = detect_front(&mesh, &scan, floor, axis, ankle);
    let back_path = torso_surface_path(&scan, waist_y, neck_y, front_sign, SurfaceSide::Back);
    let front_path = torso_surface_path(&scan, waist_y, neck_y, front_sign, SurfaceSide::Front);
    let back_waist_length = path_length(&back_path);
    let front_waist_length = path_length(&front_path);
    let bust_front = side_front_point(&scan[bust.0].loops[bust.1], axis, front_sign);
    let shoulder_bust_path = torso_surface_at_x(
        &scan,
        bust_y,
        shoulder_y,
        bust_front[0],
        front_sign,
    );
    let shoulder_to_bust = path_length(&shoulder_bust_path);

    let values = Measurements {
        height,
        neck: loop_at(&scan, neck).tape,
        shoulder_width,
        bust: loop_at(&scan, bust).tape,
        underbust: loop_at(&scan, underbust).tape,
        waist: loop_at(&scan, waist).tape,
        high_hip: loop_at(&scan, high_hip).tape,
        hip: loop_at(&scan, hip).tape,
        bicep: loop_at(&scan, bicep).tape,
        wrist: loop_at(&scan, wrist).tape,
        thigh: loop_at(&scan, thigh).tape,
        knee: loop_at(&scan, knee).tape,
        calf: loop_at(&scan, calf).tape,
        ankle: loop_at(&scan, ankle).tape,
        arm_length,
        upper_arm_length,
        back_waist_length,
        front_waist_length,
        shoulder_to_bust,
        inseam: crotch_y - floor,
        outseam: waist_y - floor,
        crotch_depth: waist_y - crotch_y,
        waist_to_hip: waist_y - hip_y,
        waist_to_knee: waist_y - knee_y,
        waist_to_floor: waist_y - floor,
    };

    let rings = [
        ("neck", neck),
        ("bust", bust),
        ("underbust", underbust),
        ("waist", waist),
        ("high_hip", high_hip),
        ("hip", hip),
        ("bicep", bicep),
        ("wrist", wrist),
        ("thigh", thigh),
        ("knee", knee),
        ("calf", calf),
        ("ankle", ankle),
    ]
    .into_iter()
    .map(|(key, at)| make_ring(key, &scan, at))
    .collect();

    let neck_back = back_path.last().copied().unwrap_or([axis[0], neck_y, axis[1]]);
    let waist_back = back_path.first().copied().unwrap_or([axis[0], waist_y, axis[1]]);
    let lines = vec![
        Line {
            key: "shoulder_width",
            from: left_shoulder,
            to: right_shoulder,
        },
        Line {
            key: "arm_length",
            from: left_shoulder,
            to: elbow_point,
        },
        Line {
            key: "arm_length",
            from: elbow_point,
            to: wrist_point,
        },
        Line {
            key: "inseam",
            from: [axis[0], floor, axis[1]],
            to: [axis[0], crotch_y, axis[1]],
        },
        Line {
            key: "crotch_depth",
            from: [axis[0], crotch_y, axis[1]],
            to: [axis[0], waist_y, axis[1]],
        },
        Line {
            key: "back_waist_length",
            from: neck_back,
            to: waist_back,
        },
        Line {
            key: "height",
            from: [axis[0], floor, axis[1]],
            to: [axis[0], top, axis[1]],
        },
    ];

    Ok(Measured {
        values,
        scale,
        rings,
        lines,
    })
}

/// All closed loops where the horizontal plane `y = y_cm` cuts the mesh,
/// as ordered polylines. Public so a viewer can show any slice.
pub fn slice_loops(mesh: &BodyMesh, y_cm: f32) -> Vec<Vec<[f32; 3]>> {
    if mesh.vertices.is_empty() || mesh.faces.is_empty() || !y_cm.is_finite() {
        return Vec::new();
    }

    let mut y = y_cm;
    if mesh
        .vertices
        .iter()
        .any(|point| (point[1] - y).abs() <= f32::EPSILON * point[1].abs().max(1.0))
    {
        y += SLICE_NUDGE;
    }

    let mut edge_nodes: HashMap<(u32, u32), usize> = HashMap::new();
    let mut points = Vec::<[f32; 3]>::new();
    let mut segments = Vec::<(usize, usize)>::new();

    for face in &mesh.faces {
        let ids = [face[0], face[1], face[2]];
        if ids.iter().any(|id| *id as usize >= mesh.vertices.len()) {
            continue;
        }
        let mut crossings = [usize::MAX; 2];
        let mut crossing_count = 0;
        for &(a_pos, b_pos) in &[(0usize, 1usize), (1, 2), (2, 0)] {
            let a_id = ids[a_pos];
            let b_id = ids[b_pos];
            let a = mesh.vertices[a_id as usize];
            let b = mesh.vertices[b_id as usize];
            if (a[1] < y && b[1] > y) || (a[1] > y && b[1] < y) {
                if crossing_count == 2 {
                    break;
                }
                let key = if a_id < b_id {
                    (a_id, b_id)
                } else {
                    (b_id, a_id)
                };
                let node = *edge_nodes.entry(key).or_insert_with(|| {
                    let t = (y - a[1]) / (b[1] - a[1]);
                    let point = [
                        a[0] + (b[0] - a[0]) * t,
                        y,
                        a[2] + (b[2] - a[2]) * t,
                    ];
                    points.push(point);
                    points.len() - 1
                });
                crossings[crossing_count] = node;
                crossing_count += 1;
            }
        }
        if crossing_count == 2 && crossings[0] != crossings[1] {
            segments.push((crossings[0], crossings[1]));
        }
    }

    if segments.is_empty() {
        return Vec::new();
    }
    let mut incident = vec![Vec::<usize>::new(); points.len()];
    for (segment_id, &(a, b)) in segments.iter().enumerate() {
        incident[a].push(segment_id);
        incident[b].push(segment_id);
    }
    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();
    for first_segment in 0..segments.len() {
        if used[first_segment] {
            continue;
        }
        let start = segments[first_segment].0;
        let mut current = start;
        let mut segment = first_segment;
        let mut ordered = Vec::new();
        loop {
            if used[segment] {
                break;
            }
            used[segment] = true;
            ordered.push(points[current]);
            let (a, b) = segments[segment];
            let next = if a == current { b } else { a };
            if next == start {
                if ordered.len() >= 3 {
                    loops.push(ordered);
                }
                break;
            }
            let Some(next_segment) = incident[next].iter().copied().find(|id| !used[*id]) else {
                break;
            };
            current = next;
            segment = next_segment;
        }
    }
    loops
}

const SLICE_NUDGE: f32 = 1.0e-4;

#[derive(Clone, Debug)]
struct LoopInfo {
    points: Vec<[f32; 3]>,
    area: f32,
    centroid: [f32; 2],
    skin: f32,
    tape: f32,
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
}

impl LoopInfo {
    fn new(points: Vec<[f32; 3]>) -> Self {
        let mut twice_area = 0.0;
        let mut centroid_x = 0.0;
        let mut centroid_z = 0.0;
        let mut skin = 0.0;
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_z = f32::NEG_INFINITY;
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            let cross = a[0] * b[2] - b[0] * a[2];
            twice_area += cross;
            centroid_x += (a[0] + b[0]) * cross;
            centroid_z += (a[2] + b[2]) * cross;
            skin += ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
            min_x = min_x.min(a[0]);
            max_x = max_x.max(a[0]);
            min_z = min_z.min(a[2]);
            max_z = max_z.max(a[2]);
        }
        let centroid = if twice_area.abs() > 1.0e-6 {
            [
                centroid_x / (3.0 * twice_area),
                centroid_z / (3.0 * twice_area),
            ]
        } else {
            let count = points.len().max(1) as f32;
            [
                points.iter().map(|point| point[0]).sum::<f32>() / count,
                points.iter().map(|point| point[2]).sum::<f32>() / count,
            ]
        };
        let tape = convex_hull_perimeter(&points);
        Self {
            points,
            area: 0.5 * twice_area.abs(),
            centroid,
            skin,
            tape,
            min_x,
            max_x,
            min_z,
            max_z,
        }
    }

    fn contains(&self, point: [f32; 2]) -> bool {
        let mut inside = false;
        for index in 0..self.points.len() {
            let a = self.points[index];
            let b = self.points[(index + 1) % self.points.len()];
            if (a[2] > point[1]) != (b[2] > point[1])
                && point[0]
                    < (b[0] - a[0]) * (point[1] - a[2]) / (b[2] - a[2]) + a[0]
            {
                inside = !inside;
            }
        }
        inside
    }
}

#[derive(Clone, Debug)]
struct ScanSlice {
    y: f32,
    loops: Vec<LoopInfo>,
    torso: Option<usize>,
}

impl ScanSlice {
    fn new(mesh: &BodyMesh, y: f32, axis: [f32; 2]) -> Self {
        let loops: Vec<_> = slice_loops(mesh, y).into_iter().map(LoopInfo::new).collect();
        let torso = loops
            .iter()
            .enumerate()
            .filter(|(_, info)| info.contains(axis))
            .max_by(|(_, a), (_, b)| cmp_f32(a.area, b.area))
            .map(|(index, _)| index);
        let actual_y = loops
            .first()
            .and_then(|info| info.points.first())
            .map_or(y, |point| point[1]);
        Self {
            y: actual_y,
            loops,
            torso,
        }
    }
}

#[derive(Clone, Copy)]
enum Extreme {
    MinTape,
    MaxTape,
    XExtent,
}

#[derive(Clone, Copy)]
enum SurfaceSide {
    Front,
    Back,
}

fn scaled_mesh(
    mesh: &BodyMesh,
    options: &MeasureOptions,
) -> Result<(BodyMesh, f32, f32, f32), MeasureError> {
    let (input_floor, input_top) = y_bounds(mesh).ok_or(MeasureError::EmptyMesh)?;
    let input_height = input_top - input_floor;
    if input_height <= 0.0 || !input_height.is_finite() {
        return Err(MeasureError::NotAStandingBody(
            "mesh has no positive vertical extent".to_string(),
        ));
    }
    let scale = options.height_cm.map_or(1.0, |height| height / input_height);
    if scale <= 0.0 || !scale.is_finite() {
        return Err(MeasureError::NotAStandingBody(
            "requested height is not positive and finite".to_string(),
        ));
    }
    let centre = mesh_xz_centroid(mesh);
    let floor_point = [centre[0], input_floor, centre[1]];
    let transform = |point: [f32; 3]| {
        [
            floor_point[0] + (point[0] - floor_point[0]) * scale,
            floor_point[1] + (point[1] - floor_point[1]) * scale,
            floor_point[2] + (point[2] - floor_point[2]) * scale,
        ]
    };
    let landmarks = mesh.landmarks.map(|landmarks| Landmarks {
        neck: transform(landmarks.neck),
        left_shoulder: transform(landmarks.left_shoulder),
        right_shoulder: transform(landmarks.right_shoulder),
        left_elbow: transform(landmarks.left_elbow),
        right_elbow: transform(landmarks.right_elbow),
        left_wrist: transform(landmarks.left_wrist),
        right_wrist: transform(landmarks.right_wrist),
        left_hip: transform(landmarks.left_hip),
        right_hip: transform(landmarks.right_hip),
        left_knee: transform(landmarks.left_knee),
        right_knee: transform(landmarks.right_knee),
        left_ankle: transform(landmarks.left_ankle),
        right_ankle: transform(landmarks.right_ankle),
    });
    let scaled = BodyMesh {
        vertices: mesh.vertices.iter().copied().map(transform).collect(),
        faces: mesh.faces.clone(),
        landmarks,
    };
    let floor = input_floor;
    let top = input_floor + input_height * scale;
    Ok((scaled, scale, floor, top))
}

fn y_bounds(mesh: &BodyMesh) -> Option<(f32, f32)> {
    let mut floor = f32::INFINITY;
    let mut top = f32::NEG_INFINITY;
    for point in &mesh.vertices {
        if !point.iter().all(|value| value.is_finite()) {
            continue;
        }
        floor = floor.min(point[1]);
        top = top.max(point[1]);
    }
    (floor.is_finite() && top.is_finite()).then_some((floor, top))
}

fn mesh_xz_centroid(mesh: &BodyMesh) -> [f32; 2] {
    let count = mesh.vertices.len().max(1) as f32;
    [
        mesh.vertices.iter().map(|point| point[0]).sum::<f32>() / count,
        mesh.vertices.iter().map(|point| point[2]).sum::<f32>() / count,
    ]
}

fn largest_loop_at(mesh: &BodyMesh, y: f32) -> Option<LoopInfo> {
    slice_loops(mesh, y)
        .into_iter()
        .map(LoopInfo::new)
        .max_by(|a, b| cmp_f32(a.area, b.area))
}

fn find_crotch(scan: &[ScanSlice], from_y: f32, axis: [f32; 2]) -> Option<usize> {
    let start = scan
        .iter()
        .rposition(|slice| slice.y <= from_y)
        .unwrap_or(scan.len().saturating_sub(1));
    let mut saw_torso = scan[start..]
        .iter()
        .take(3)
        .any(|slice| slice.torso.is_some());
    for index in (0..=start).rev() {
        let slice = &scan[index];
        if slice.torso.is_some() {
            saw_torso = true;
            continue;
        }
        if saw_torso && has_two_legs(slice, axis) {
            return Some(index);
        }
    }
    None
}

fn has_two_legs(slice: &ScanSlice, axis: [f32; 2]) -> bool {
    let legs = two_largest_separate_loops(slice);
    legs.len() == 2
        && slice.loops[legs[0]].area > 20.0
        && slice.loops[legs[1]].area > 20.0
        && (slice.loops[legs[0]].centroid[0] - axis[0])
            * (slice.loops[legs[1]].centroid[0] - axis[0])
            < 0.0
}

fn find_armpit(
    scan: &[ScanSlice],
    crotch_index: usize,
    axis: [f32; 2],
    landmarks: Option<&Landmarks>,
) -> Option<usize> {
    let shoulder_hint = landmarks.map(|landmarks| {
        (landmarks.left_shoulder[1] + landmarks.right_shoulder[1]) * 0.5
    });
    let mut saw_two_arms = false;
    let mut previous_count = 0;
    let end_y = shoulder_hint.map_or(f32::INFINITY, |hint| hint + 10.0);
    for index in crotch_index.saturating_add(1)..scan.len() {
        let slice = &scan[index];
        if slice.y > end_y {
            break;
        }
        let count = arm_candidates(slice, axis).count();
        if count >= 2 {
            saw_two_arms = true;
        } else if saw_two_arms
            && slice.torso.is_some()
            && (count < previous_count || count == 0)
        {
            return Some(index);
        }
        previous_count = count;
    }
    None
}

fn arm_candidates<'a>(
    slice: &'a ScanSlice,
    axis: [f32; 2],
) -> impl Iterator<Item = (usize, &'a LoopInfo)> + 'a {
    let torso_half_width = slice
        .torso
        .map(|index| (slice.loops[index].max_x - slice.loops[index].min_x) * 0.5)
        .unwrap_or(12.0);
    slice
        .loops
        .iter()
        .enumerate()
        .filter(move |(index, info)| {
            Some(*index) != slice.torso
                && info.area > 1.0
                && (info.centroid[0] - axis[0]).abs() > torso_half_width * 0.5
        })
}

fn torso_candidates(
    scan: &[ScanSlice],
    low: f32,
    high: f32,
) -> impl Iterator<Item = (usize, usize)> + '_ {
    let (low, high) = ordered_bounds(low, high);
    scan.iter().enumerate().filter_map(move |(index, slice)| {
        (slice.y >= low && slice.y <= high)
            .then_some(slice.torso.map(|loop_index| (index, loop_index)))
            .flatten()
    })
}

fn torso_extreme(
    scan: &[ScanSlice],
    low: f32,
    high: f32,
    extreme: Extreme,
) -> Option<(usize, usize)> {
    select_extreme(scan, torso_candidates(scan, low, high), extreme)
}

fn nearest_torso(scan: &[ScanSlice], y: f32) -> Option<(usize, usize)> {
    scan.iter()
        .enumerate()
        .filter_map(|(index, slice)| {
            slice
                .torso
                .map(|loop_index| ((slice.y - y).abs(), index, loop_index))
        })
        .min_by(|a, b| cmp_f32(a.0, b.0))
        .map(|(_, index, loop_index)| (index, loop_index))
}

fn leg_loop(slice: &ScanSlice, axis: [f32; 2]) -> Option<usize> {
    let _ = axis;
    two_largest_separate_loops(slice)
        .into_iter()
        .filter(|index| slice.loops[*index].area > 1.0)
        .min_by(|a, b| {
            cmp_f32(
                slice.loops[*a].centroid[0],
                slice.loops[*b].centroid[0],
            )
        })
}

fn two_largest_separate_loops(slice: &ScanSlice) -> Vec<usize> {
    let mut loops: Vec<_> = (0..slice.loops.len())
        .filter(|index| Some(*index) != slice.torso)
        .collect();
    loops.sort_unstable_by(|a, b| cmp_f32(slice.loops[*b].area, slice.loops[*a].area));
    loops.truncate(2);
    loops
}

fn leg_extreme(
    scan: &[ScanSlice],
    low: f32,
    high: f32,
    axis: [f32; 2],
    extreme: Extreme,
) -> Option<(usize, usize)> {
    let (low, high) = ordered_bounds(low, high);
    let candidates = scan.iter().enumerate().filter_map(|(index, slice)| {
        (slice.y >= low && slice.y <= high)
            .then(|| leg_loop(slice, axis).map(|loop_index| (index, loop_index)))
            .flatten()
    });
    select_extreme(scan, candidates, extreme)
}

fn leg_nearest(scan: &[ScanSlice], y: f32, axis: [f32; 2]) -> Option<(usize, usize)> {
    scan.iter()
        .enumerate()
        .filter_map(|(index, slice)| {
            leg_loop(slice, axis).map(|loop_index| ((slice.y - y).abs(), index, loop_index))
        })
        .min_by(|a, b| cmp_f32(a.0, b.0))
        .map(|(_, index, loop_index)| (index, loop_index))
}

fn arm_extreme(
    scan: &[ScanSlice],
    low: f32,
    high: f32,
    axis: [f32; 2],
    extreme: Extreme,
) -> Option<(usize, usize)> {
    let (low, high) = ordered_bounds(low, high);
    let candidates = scan.iter().enumerate().filter_map(|(index, slice)| {
        if slice.y < low || slice.y > high {
            return None;
        }
        arm_candidates(slice, axis)
            .filter(|(_, info)| info.centroid[0] < axis[0])
            .min_by(|(_, a), (_, b)| cmp_f32(a.centroid[0], b.centroid[0]))
            .map(|(loop_index, _)| (index, loop_index))
    });
    select_extreme(scan, candidates, extreme)
}

fn trace_arm(
    scan: &[ScanSlice],
    seed: (usize, usize),
    axis: [f32; 2],
) -> Vec<(usize, usize)> {
    let mut track = vec![seed];
    let mut previous = scan[seed.0].loops[seed.1].centroid;
    let mut missed = 0;
    for index in (0..seed.0).rev() {
        let best = arm_candidates(&scan[index], axis)
            .filter(|(_, info)| info.centroid[0] < axis[0])
            .map(|(loop_index, info)| {
                let distance = distance2(info.centroid, previous);
                (distance, loop_index, info.centroid)
            })
            .min_by(|a, b| cmp_f32(a.0, b.0));
        if let Some((distance, loop_index, centre)) = best.filter(|best| best.0 <= 5.0) {
            let _ = distance;
            track.push((index, loop_index));
            previous = centre;
            missed = 0;
        } else {
            missed += 1;
            if missed >= 2 {
                break;
            }
        }
    }
    track.sort_unstable_by_key(|entry| entry.0);
    track
}

fn track_extreme(
    scan: &[ScanSlice],
    track: &[(usize, usize)],
    low: f32,
    high: f32,
    extreme: Extreme,
) -> Option<(usize, usize)> {
    let (low, high) = ordered_bounds(low, high);
    select_extreme(
        scan,
        track
            .iter()
            .copied()
            .filter(|(index, _)| scan[*index].y >= low && scan[*index].y <= high),
        extreme,
    )
}

fn track_nearest(
    scan: &[ScanSlice],
    track: &[(usize, usize)],
    y: f32,
) -> Option<(usize, usize)> {
    track
        .iter()
        .copied()
        .min_by(|a, b| cmp_f32((scan[a.0].y - y).abs(), (scan[b.0].y - y).abs()))
}

fn select_extreme(
    scan: &[ScanSlice],
    candidates: impl Iterator<Item = (usize, usize)>,
    extreme: Extreme,
) -> Option<(usize, usize)> {
    candidates.max_by(|a, b| {
        let a = &scan[a.0].loops[a.1];
        let b = &scan[b.0].loops[b.1];
        match extreme {
            Extreme::MinTape => cmp_f32(b.tape, a.tape),
            Extreme::MaxTape => cmp_f32(a.tape, b.tape),
            Extreme::XExtent => cmp_f32(a.max_x - a.min_x, b.max_x - b.min_x),
        }
    })
}

fn loop_at(scan: &[ScanSlice], at: (usize, usize)) -> &LoopInfo {
    &scan[at.0].loops[at.1]
}

fn loop_center_point(slice: &ScanSlice, loop_index: usize) -> [f32; 3] {
    let centre = slice.loops[loop_index].centroid;
    [centre[0], slice.y, centre[1]]
}

fn make_ring(key: &'static str, scan: &[ScanSlice], at: (usize, usize)) -> Ring {
    let info = loop_at(scan, at);
    Ring {
        key,
        y_cm: scan[at.0].y,
        points: info.points.clone(),
        skin_perimeter_cm: info.skin,
        tape_perimeter_cm: info.tape,
    }
}

fn inset_extreme_x(info: &LoopInfo, left: bool, inset: f32, y: f32) -> [f32; 3] {
    let extreme = if left { info.min_x } else { info.max_x };
    let point = info
        .points
        .iter()
        .min_by(|a, b| cmp_f32((a[0] - extreme).abs(), (b[0] - extreme).abs()))
        .copied()
        .unwrap_or([extreme, y, info.centroid[1]]);
    [
        point[0] + if left { inset } else { -inset },
        y,
        point[2],
    ]
}

fn detect_front(
    mesh: &BodyMesh,
    scan: &[ScanSlice],
    floor: f32,
    axis: [f32; 2],
    ankle: (usize, usize),
) -> f32 {
    let ankle_centre = loop_at(scan, ankle).centroid[1];
    let foot_loops = slice_loops(mesh, floor + 3.0);
    let mut positive: f32 = 0.0;
    let mut negative: f32 = 0.0;
    for points in foot_loops {
        let info = LoopInfo::new(points);
        if (info.centroid[0] - axis[0]).abs() < 30.0 {
            positive = positive.max(info.max_z - ankle_centre);
            negative = negative.max(ankle_centre - info.min_z);
        }
    }
    if negative > positive { -1.0 } else { 1.0 }
}

fn torso_surface_path(
    scan: &[ScanSlice],
    low: f32,
    high: f32,
    front_sign: f32,
    side: SurfaceSide,
) -> Vec<[f32; 3]> {
    torso_candidates(scan, low, high)
        .map(|(slice_index, loop_index)| {
            let info = &scan[slice_index].loops[loop_index];
            let direction = match side {
                SurfaceSide::Front => front_sign,
                SurfaceSide::Back => -front_sign,
            };
            info.points
                .iter()
                .max_by(|a, b| cmp_f32(a[2] * direction, b[2] * direction))
                .copied()
                .unwrap_or([info.centroid[0], scan[slice_index].y, info.centroid[1]])
        })
        .collect()
}

fn side_front_point(info: &LoopInfo, axis: [f32; 2], front_sign: f32) -> [f32; 3] {
    let side_limit = axis[0] - (info.max_x - info.min_x) * 0.05;
    info.points
        .iter()
        .filter(|point| point[0] < side_limit)
        .max_by(|a, b| cmp_f32(a[2] * front_sign, b[2] * front_sign))
        .copied()
        .or_else(|| {
            info.points
                .iter()
                .max_by(|a, b| cmp_f32(a[2] * front_sign, b[2] * front_sign))
                .copied()
        })
        .unwrap_or([axis[0], 0.0, axis[1]])
}

fn torso_surface_at_x(
    scan: &[ScanSlice],
    low: f32,
    high: f32,
    x: f32,
    front_sign: f32,
) -> Vec<[f32; 3]> {
    torso_candidates(scan, low, high)
        .map(|(slice_index, loop_index)| {
            let info = &scan[slice_index].loops[loop_index];
            let mut intersections = Vec::new();
            for index in 0..info.points.len() {
                let a = info.points[index];
                let b = info.points[(index + 1) % info.points.len()];
                if (a[0] <= x && b[0] >= x) || (a[0] >= x && b[0] <= x) {
                    let dx = b[0] - a[0];
                    if dx.abs() > 1.0e-6 {
                        let t = (x - a[0]) / dx;
                        if (0.0..=1.0).contains(&t) {
                            intersections.push([x, scan[slice_index].y, a[2] + (b[2] - a[2]) * t]);
                        }
                    }
                }
            }
            intersections
                .into_iter()
                .max_by(|a, b| cmp_f32(a[2] * front_sign, b[2] * front_sign))
                .or_else(|| {
                    info.points
                        .iter()
                        .min_by(|a, b| cmp_f32((a[0] - x).abs(), (b[0] - x).abs()))
                        .copied()
                })
                .unwrap_or([x, scan[slice_index].y, info.centroid[1]])
        })
        .collect()
}

fn path_length(points: &[[f32; 3]]) -> f32 {
    points
        .windows(2)
        .map(|pair| distance3(pair[0], pair[1]))
        .sum()
}

fn convex_hull_perimeter(points: &[[f32; 3]]) -> f32 {
    let mut planar: Vec<[f32; 2]> = points.iter().map(|point| [point[0], point[2]]).collect();
    planar.sort_by(|a, b| cmp_f32(a[0], b[0]).then_with(|| cmp_f32(a[1], b[1])));
    planar.dedup_by(|a, b| (a[0] - b[0]).abs() < 1.0e-6 && (a[1] - b[1]).abs() < 1.0e-6);
    if planar.len() < 2 {
        return 0.0;
    }
    let mut hull = Vec::with_capacity(planar.len() * 2);
    for point in planar.iter().chain(planar.iter().rev()) {
        while hull.len() >= 2
            && cross2(hull[hull.len() - 2], hull[hull.len() - 1], *point) <= 0.0
        {
            hull.pop();
        }
        hull.push(*point);
    }
    hull.pop();
    if hull.len() < 2 {
        return 0.0;
    }
    (0..hull.len())
        .map(|index| distance2(hull[index], hull[(index + 1) % hull.len()]))
        .sum()
}

fn cross2(origin: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - origin[0]) * (b[1] - origin[1])
        - (a[1] - origin[1]) * (b[0] - origin[0])
}

fn distance2(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn distance3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn ordered_bounds(a: f32, b: f32) -> (f32, f32) {
    (a.min(b), a.max(b))
}

fn cmp_f32(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SIDES: usize = 96;

    #[derive(Clone, Copy)]
    struct SyntheticSpec {
        height: f32,
        crotch_y: f32,
        waist_y: f32,
        shoulder_width: f32,
        neck_radii: [f32; 2],
        bust_radii: [f32; 2],
        waist_radii: [f32; 2],
        hip_radii: [f32; 2],
        thigh_radii: [f32; 2],
        bicep_radii: [f32; 2],
        wrist_radii: [f32; 2],
    }

    impl Default for SyntheticSpec {
        fn default() -> Self {
            Self {
                height: 172.0,
                crotch_y: 80.0,
                waist_y: 102.0,
                shoulder_width: 42.0,
                neck_radii: [6.0, 5.5],
                bust_radii: [16.0, 13.5],
                waist_radii: [13.0, 10.5],
                hip_radii: [17.0, 15.0],
                thigh_radii: [9.0, 8.0],
                bicep_radii: [5.0, 4.5],
                wrist_radii: [3.0, 2.5],
            }
        }
    }

    fn synthetic_body(spec: SyntheticSpec) -> BodyMesh {
        let mut mesh = BodyMesh {
            vertices: Vec::new(),
            faces: Vec::new(),
            landmarks: None,
        };

        let torso_controls = [
            (spec.crotch_y, spec.hip_radii),
            (84.0, spec.hip_radii),
            (88.0, spec.hip_radii),
            (98.0, [14.0, 11.5]),
            (102.0, spec.waist_radii),
            (106.0, [14.0, 11.5]),
            (114.0, [14.0, 11.5]),
            (124.0, spec.bust_radii),
            (128.0, spec.bust_radii),
            (138.0, [16.0, 11.5]),
            (141.0, [(spec.shoulder_width + 3.0) * 0.5, 10.5]),
            (143.0, [(spec.shoulder_width + 3.0) * 0.5, 10.5]),
            (145.0, spec.neck_radii),
            (154.0, spec.neck_radii),
            (158.0, [8.0, 7.0]),
            (165.0, [9.0, 8.0]),
            (spec.height, [0.05, 0.05]),
        ];
        add_tube(&mut mesh, &torso_controls, |_| [0.0, 0.0]);

        let leg_controls = [
            (0.0, [5.0, 9.0]),
            (5.0, [4.0, 3.0]),
            (12.0, [4.0, 3.0]),
            (24.0, [6.0, 5.5]),
            (34.0, [6.5, 5.8]),
            (42.0, [5.5, 4.8]),
            (48.0, [5.5, 4.8]),
            (64.0, [7.5, 6.8]),
            (70.0, spec.thigh_radii),
            (spec.crotch_y, spec.thigh_radii),
        ];
        add_tube(&mut mesh, &leg_controls, |_| [-10.5, 0.0]);
        add_tube(&mut mesh, &leg_controls, |_| [10.5, 0.0]);

        let arm_controls = [
            (82.0, [5.2, 5.0]),
            (90.0, [4.0, 3.7]),
            (95.0, spec.wrist_radii),
            (98.0, spec.wrist_radii),
            (104.0, [4.5, 4.1]),
            (112.0, [3.8, 3.4]),
            (128.0, spec.bicep_radii),
            (132.0, spec.bicep_radii),
            (139.0, [4.5, 4.0]),
        ];
        let arm_centre = |left: bool, y: f32| {
            let shoulder_x = if left { -23.0 } else { 23.0 };
            let outward = (139.0 - y) * 12.0f32.to_radians().tan();
            [shoulder_x + if left { -outward } else { outward }, 0.0]
        };
        add_tube(&mut mesh, &arm_controls, |y| arm_centre(true, y));
        add_tube(&mut mesh, &arm_controls, |y| arm_centre(false, y));
        mesh
    }

    fn add_tube(
        mesh: &mut BodyMesh,
        controls: &[(f32, [f32; 2])],
        centre: impl Fn(f32) -> [f32; 2],
    ) {
        let first_y = controls.first().unwrap().0;
        let last_y = controls.last().unwrap().0;
        let steps = (last_y - first_y).round() as usize;
        let base = mesh.vertices.len() as u32;
        for step in 0..=steps {
            let y = first_y + (last_y - first_y) * step as f32 / steps as f32;
            let radii = interpolate_controls(controls, y);
            let centre = centre(y);
            for side in 0..SIDES {
                let angle = TAU * side as f32 / SIDES as f32;
                mesh.vertices.push([
                    centre[0] + radii[0] * angle.cos(),
                    y,
                    centre[1] + radii[1] * angle.sin(),
                ]);
            }
        }
        for step in 0..steps {
            for side in 0..SIDES {
                let next = (side + 1) % SIDES;
                let a = base + (step * SIDES + side) as u32;
                let b = base + (step * SIDES + next) as u32;
                let c = base + ((step + 1) * SIDES + side) as u32;
                let d = base + ((step + 1) * SIDES + next) as u32;
                mesh.faces.push([a, c, b]);
                mesh.faces.push([b, c, d]);
            }
        }
        let bottom = mesh.vertices.len() as u32;
        let bottom_centre = centre(first_y);
        mesh.vertices
            .push([bottom_centre[0], first_y, bottom_centre[1]]);
        let top = mesh.vertices.len() as u32;
        let top_centre = centre(last_y);
        mesh.vertices.push([top_centre[0], last_y, top_centre[1]]);
        let top_ring = base + (steps * SIDES) as u32;
        for side in 0..SIDES {
            let next = (side + 1) % SIDES;
            mesh.faces
                .push([bottom, base + next as u32, base + side as u32]);
            mesh.faces
                .push([top, top_ring + side as u32, top_ring + next as u32]);
        }
    }

    fn interpolate_controls(controls: &[(f32, [f32; 2])], y: f32) -> [f32; 2] {
        for pair in controls.windows(2) {
            if y <= pair[1].0 {
                let t = (y - pair[0].0) / (pair[1].0 - pair[0].0);
                return [
                    pair[0].1[0] + (pair[1].1[0] - pair[0].1[0]) * t,
                    pair[0].1[1] + (pair[1].1[1] - pair[0].1[1]) * t,
                ];
            }
        }
        controls.last().unwrap().1
    }

    fn ellipse_perimeter(radii: [f32; 2]) -> f32 {
        let [a, b] = radii;
        let h = ((a - b) / (a + b)).powi(2);
        std::f32::consts::PI * (a + b) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()))
    }

    fn cube() -> BodyMesh {
        BodyMesh {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            faces: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
            landmarks: None,
        }
    }

    fn relative_error(actual: f32, expected: f32) -> f32 {
        (actual - expected).abs() / expected
    }

    #[test]
    fn cube_slice_is_one_unit_square() {
        let mesh = cube();
        let loops = slice_loops(&mesh, 0.5);
        assert_eq!(loops.len(), 1);
        let info = LoopInfo::new(loops[0].clone());
        assert!((info.skin - 4.0).abs() < 1.0e-5, "{}", info.skin);
        assert!(slice_loops(&mesh, 2.0).is_empty());
    }

    #[test]
    fn synthetic_slices_have_expected_components() {
        let mesh = synthetic_body(SyntheticSpec::default());
        assert_eq!(slice_loops(&mesh, 120.5).len(), 3);
        assert_eq!(slice_loops(&mesh, 60.5).len(), 2);
        assert!(mesh.faces.len() >= 30_000);
    }

    #[test]
    fn synthetic_measurements_match_construction() {
        let spec = SyntheticSpec::default();
        let measured = measure(&synthetic_body(spec), &MeasureOptions::default()).unwrap();
        eprintln!(
            "rings: {:?}",
            measured
                .rings
                .iter()
                .map(|ring| (ring.key, ring.y_cm, ring.tape_perimeter_cm))
                .collect::<Vec<_>>()
        );
        let expected = [
            ("neck", measured.values.neck, ellipse_perimeter(spec.neck_radii)),
            ("bust", measured.values.bust, ellipse_perimeter(spec.bust_radii)),
            ("waist", measured.values.waist, ellipse_perimeter(spec.waist_radii)),
            ("hip", measured.values.hip, ellipse_perimeter(spec.hip_radii)),
            ("thigh", measured.values.thigh, ellipse_perimeter(spec.thigh_radii)),
            ("bicep", measured.values.bicep, ellipse_perimeter(spec.bicep_radii)),
            ("wrist", measured.values.wrist, ellipse_perimeter(spec.wrist_radii)),
        ];
        for (key, actual, expected) in expected {
            let error = relative_error(actual, expected);
            eprintln!("{key}: actual={actual:.3} expected={expected:.3} error={:.2}%", error * 100.0);
            assert!(error <= 0.015, "{key} error was {:.2}%", error * 100.0);
        }
        assert!((measured.values.height - spec.height).abs() <= 0.2);
        assert!((measured.values.inseam - spec.crotch_y).abs() <= 1.0);
        assert!((measured.values.crotch_depth - (spec.waist_y - spec.crotch_y)).abs() <= 1.0);
        assert!((measured.values.shoulder_width - spec.shoulder_width).abs() <= 1.5);
        assert_eq!(measured.rings.len(), 12);
        assert_eq!(measured.lines.iter().filter(|line| line.key == "arm_length").count(), 2);
        eprintln!(
            "landmarks: crotch={:.3}/{:.3} waist={:.3}/{:.3} shoulder={:.3}/142.000",
            measured.values.inseam,
            spec.crotch_y,
            measured.values.outseam,
            spec.waist_y,
            measured
                .lines
                .iter()
                .find(|line| line.key == "shoulder_width")
                .unwrap()
                .from[1]
        );
    }

    #[test]
    fn height_rescale_scales_circumferences() {
        let spec = SyntheticSpec::default();
        let mesh = synthetic_body(spec);
        let original = measure(&mesh, &MeasureOptions::default()).unwrap();
        let scaled = measure(
            &mesh,
            &MeasureOptions {
                height_cm: Some(180.0),
            },
        )
        .unwrap();
        let factor = 180.0 / spec.height;
        assert!((scaled.scale - factor).abs() < 1.0e-6);
        assert!(relative_error(scaled.values.bust, original.values.bust * factor) < 0.005);
    }

    #[test]
    fn short_mesh_is_not_a_standing_body() {
        let mut mesh = cube();
        for point in &mut mesh.vertices {
            point[0] *= 10.0;
            point[1] *= 50.0;
            point[2] *= 10.0;
        }
        assert!(matches!(
            measure(&mesh, &MeasureOptions::default()),
            Err(MeasureError::NotAStandingBody(_))
        ));
    }

    #[test]
    #[ignore = "release-only performance smoke test"]
    fn synthetic_measurement_performance() {
        let mesh = synthetic_body(SyntheticSpec::default());
        let start = std::time::Instant::now();
        let measured = measure(&mesh, &MeasureOptions::default()).unwrap();
        std::hint::black_box(measured);
        let elapsed = start.elapsed();
        eprintln!("synthetic measure: {elapsed:?}, faces={}", mesh.faces.len());
        assert!(elapsed.as_millis() < 300, "elapsed {elapsed:?}");
    }
}
