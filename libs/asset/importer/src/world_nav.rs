//! Navigation facts of a converted World: where a player spawns, how high the
//! floor is under them, how far they may step up, where the eye sits.
//!
//! The classic formats all know this (Doom `THINGS` type 1 with its angle,
//! Quake/Quake2/Quake3 `info_player_*` entities, Duke's BUILD map header), but
//! it used to stop at the `.spawn` sidecar the local library reads — a catalog
//! World arrived with no way to stand on its floor, so a walker starts on the
//! ceiling outside the map.
//!
//! Two consumers, one source:
//! - the library keeps reading the `.spawn` sidecar, whose first three lines
//!   are byte-identical to the original `world-spawn 1` format;
//! - the catalog publishes [`WorldNav::anchors`] on the World asset.
//!
//! Everything is in the GLB's own space: metres, Y-up, −Z forward, and yaw in
//! radians about +Y. `player_start` is the EYE position (what a camera wants);
//! the feet are `player_start.y - eye_height`, which is `floor_height`.

use makepad_asset_data::limits::MAX_ANCHORS_PER_ASSET;
use makepad_asset_data::{Anchor, Quat, Transform, Vec3};

pub const MAGIC: &str = "world-spawn";
pub const VERSION: u32 = 1;

/// Anchor name of the primary spawn. Extra player/coop starts are
/// `player_start_2`, `player_start_3`, … in map order.
pub const PRIMARY: &str = "player_start";
/// Anchor names of deathmatch starts: `deathmatch_1`, `deathmatch_2`, …
pub const DEATHMATCH: &str = "deathmatch";

/// One spawn point. `pos` is the eye, `yaw` 0 looks down −Z and grows toward
/// −X (the engine convention the classic converters already write).
#[derive(Clone, Debug, PartialEq)]
pub struct NavStart {
    pub name: String,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// One door of a converted map. The moving geometry and its animation live
/// in the GLB (node + clip `name`); this is the cheap catalog summary.
#[derive(Clone, Debug, PartialEq)]
pub struct NavDoor {
    /// Same string as the glTF node and animation: `door_1`, `door_2`, …
    pub name: String,
    /// Doorway centre at the CLOSED height (metres, GLB space).
    pub pos: [f32; 3],
    pub closed_y: f32,
    pub open_y: f32,
}

/// One teleporter: step anywhere in the pad AABB and arrive at `dst`
/// facing `yaw`. The pad is flat (x/z of the GLB); `dst` is an eye
/// position like [`NavStart::pos`].
#[derive(Clone, Debug, PartialEq)]
pub struct NavTeleport {
    /// `teleport_1`, `teleport_2`, … in map order.
    pub name: String,
    /// Pad corners in the GLB's x/z plane, metres.
    pub pad_min: [f32; 2],
    pub pad_max: [f32; 2],
    /// Where the walker lands (eye height above the destination floor).
    pub dst: [f32; 3],
    pub yaw: f32,
}

/// What a walker needs to stand in a converted map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldNav {
    /// Primary start first; the rest in map order.
    pub starts: Vec<NavStart>,
    /// Floor height under the primary start (metres, GLB space).
    pub floor_y: Option<f32>,
    /// Tallest step-up the source engine allowed, in metres.
    pub step_height: Option<f32>,
    /// Eye height above the floor, in metres.
    pub eye_height: Option<f32>,
    /// Doors the exporter turned into animated `door_N` nodes.
    pub doors: Vec<NavDoor>,
    /// Lifts, same shape as doors: `closed_y` is the UP floor it rests at,
    /// `open_y` the DOWN floor it travels to (so travel is negative).
    pub lifts: Vec<NavDoor>,
    /// Teleport pads and where they land.
    pub teleports: Vec<NavTeleport>,
    /// Named points a navigator needs but cannot see: `exit`, `exit_secret`,
    /// `key_blue` / `key_yellow` / `key_red` (Doom), `key_silver` /
    /// `key_gold` (Quake). Position is eye height above the floor; rotation
    /// is the facing that uses it (a switch's press direction).
    pub markers: Vec<NavStart>,
}

impl WorldNav {
    /// One start and nothing else — the shape a converter that only knows
    /// the map's own spawn point can honestly publish.
    pub fn single(pos: [f32; 3], yaw: f32, pitch: f32) -> Self {
        Self {
            starts: vec![NavStart {
                name: PRIMARY.into(),
                pos,
                yaw,
                pitch,
            }],
            ..Self::default()
        }
    }

    /// Same, plus the engine constants the caller already scaled to metres.
    pub fn with_heights(mut self, floor_y: f32, eye_height: f32, step_height: f32) -> Self {
        self.floor_y = Some(floor_y);
        self.eye_height = Some(eye_height);
        self.step_height = Some(step_height);
        self
    }

    pub fn primary(&self) -> Option<&NavStart> {
        self.starts.first()
    }

    /// Sidecar text. Lines 1–3 are the original `world-spawn 1` format the
    /// library parser reads; everything after is additive and ignored by it.
    pub fn to_text(&self) -> String {
        let p = self.primary();
        let (pos, yaw, pitch) = p.map_or(([0.0; 3], 0.0, 0.0), |s| (s.pos, s.yaw, s.pitch));
        let mut out = format!(
            "{MAGIC} {VERSION}\n{:.4} {:.4} {:.4}\n{:.5} {:.5}\n",
            pos[0], pos[1], pos[2], yaw, pitch
        );
        for s in &self.starts {
            out.push_str(&format!(
                "start {} {:.4} {:.4} {:.4} {:.5} {:.5}\n",
                s.name, s.pos[0], s.pos[1], s.pos[2], s.yaw, s.pitch
            ));
        }
        if let Some(v) = self.floor_y {
            out.push_str(&format!("floor {v:.4}\n"));
        }
        if let Some(v) = self.step_height {
            out.push_str(&format!("step {v:.4}\n"));
        }
        if let Some(v) = self.eye_height {
            out.push_str(&format!("eye {v:.4}\n"));
        }
        for d in &self.doors {
            out.push_str(&format!(
                "door {} {:.4} {:.4} {:.4} {:.4} {:.4}\n",
                d.name, d.pos[0], d.pos[1], d.pos[2], d.closed_y, d.open_y
            ));
        }
        for l in &self.lifts {
            out.push_str(&format!(
                "lift {} {:.4} {:.4} {:.4} {:.4} {:.4}\n",
                l.name, l.pos[0], l.pos[1], l.pos[2], l.closed_y, l.open_y
            ));
        }
        for m in &self.markers {
            out.push_str(&format!(
                "marker {} {:.4} {:.4} {:.4} {:.5}\n",
                m.name, m.pos[0], m.pos[1], m.pos[2], m.yaw
            ));
        }
        for t in &self.teleports {
            out.push_str(&format!(
                "teleport {} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.5}\n",
                t.name,
                t.pad_min[0],
                t.pad_min[1],
                t.pad_max[0],
                t.pad_max[1],
                t.dst[0],
                t.dst[1],
                t.dst[2],
                t.yaw
            ));
        }
        out
    }

    /// Parse a sidecar, old (three lines) or new. An old one still yields the
    /// primary start, so worlds converted before this existed still publish
    /// where their player spawns.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if !lines.next()?.trim().starts_with(MAGIC) {
            return None;
        }
        let mut xyz = lines.next()?.split_whitespace();
        let pos = [
            xyz.next()?.parse().ok()?,
            xyz.next()?.parse().ok()?,
            xyz.next()?.parse().ok()?,
        ];
        let mut yp = lines.next()?.split_whitespace();
        let yaw = yp.next()?.parse().ok()?;
        let pitch = yp.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let mut nav = Self::default();
        for line in lines {
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("start") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let nums: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                    if name.is_empty() || nums.len() < 4 {
                        continue;
                    }
                    nav.starts.push(NavStart {
                        name,
                        pos: [nums[0], nums[1], nums[2]],
                        yaw: nums[3],
                        pitch: nums.get(4).copied().unwrap_or(0.0),
                    });
                }
                Some("door") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let nums: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                    if name.is_empty() || nums.len() < 5 {
                        continue;
                    }
                    nav.doors.push(NavDoor {
                        name,
                        pos: [nums[0], nums[1], nums[2]],
                        closed_y: nums[3],
                        open_y: nums[4],
                    });
                }
                Some("marker") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let nums: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                    if name.is_empty() || nums.len() < 4 {
                        continue;
                    }
                    nav.markers.push(NavStart {
                        name,
                        pos: [nums[0], nums[1], nums[2]],
                        yaw: nums[3],
                        pitch: 0.0,
                    });
                }
                Some("lift") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let nums: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                    if name.is_empty() || nums.len() < 5 {
                        continue;
                    }
                    nav.lifts.push(NavDoor {
                        name,
                        pos: [nums[0], nums[1], nums[2]],
                        closed_y: nums[3],
                        open_y: nums[4],
                    });
                }
                Some("teleport") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let nums: Vec<f32> = parts.filter_map(|s| s.parse().ok()).collect();
                    if name.is_empty() || nums.len() < 8 {
                        continue;
                    }
                    nav.teleports.push(NavTeleport {
                        name,
                        pad_min: [nums[0], nums[1]],
                        pad_max: [nums[2], nums[3]],
                        dst: [nums[4], nums[5], nums[6]],
                        yaw: nums[7],
                    });
                }
                Some("floor") => nav.floor_y = parts.next().and_then(|s| s.parse().ok()),
                Some("step") => nav.step_height = parts.next().and_then(|s| s.parse().ok()),
                Some("eye") => nav.eye_height = parts.next().and_then(|s| s.parse().ok()),
                _ => {}
            }
        }
        if nav.starts.is_empty() {
            nav.starts.push(NavStart {
                name: PRIMARY.into(),
                pos,
                yaw,
                pitch,
            });
        }
        Some(nav)
    }

    /// Manifest anchors, capped at [`MAX_ANCHORS_PER_ASSET`]: the height
    /// hints first (a walker is useless without them), then the starts in
    /// order, so a map with hundreds of deathmatch spots truncates its tail
    /// instead of losing `player_start`.
    pub fn anchors(&self) -> Vec<Anchor> {
        let mut out = Vec::new();
        for (name, value) in [
            ("floor_height", self.floor_y),
            ("step_height", self.step_height),
            ("eye_height", self.eye_height),
        ] {
            if let Some(v) = value.filter(|v| v.is_finite()) {
                out.push(Anchor {
                    name: name.into(),
                    transform: height_transform(v),
                });
            }
        }
        // Player starts first, deathmatch spots last: on a crowded map the
        // spot a single player uses must not be crowded out by 16 he never
        // will.
        let (players, deathmatch): (Vec<&NavStart>, Vec<&NavStart>) = self
            .starts
            .iter()
            .partition(|s| !s.name.starts_with(DEATHMATCH));
        for start in players.into_iter().chain(&self.markers) {
            if out.len() >= MAX_ANCHORS_PER_ASSET {
                break;
            }
            let _ = &start.name;
            if !start.pos.iter().all(|v| v.is_finite()) || !start.yaw.is_finite() {
                continue;
            }
            if out.iter().any(|a: &Anchor| a.name == start.name) {
                continue;
            }
            out.push(Anchor {
                name: start.name.clone(),
                transform: Transform {
                    pos: Vec3 {
                        x: start.pos[0],
                        y: start.pos[1],
                        z: start.pos[2],
                    },
                    rot: yaw_quat(start.yaw),
                    scale: Vec3::ONE,
                },
            });
        }
        // A door anchor: `pos` is the doorway centre at the CLOSED height,
        // `scale.y` the travel to the open height (`open = pos.y + scale.y`).
        // The moving geometry and its clip live in the GLB under the same
        // name; this is what a catalog consumer sees without downloading it.
        for door in self.doors.iter().chain(&self.lifts) {
            if out.len() >= MAX_ANCHORS_PER_ASSET {
                break;
            }
            let travel = door.open_y - door.closed_y;
            if travel == 0.0 || !travel.is_finite() || !door.pos.iter().all(|v| v.is_finite()) {
                continue;
            }
            if out.iter().any(|a: &Anchor| a.name == door.name) {
                continue;
            }
            out.push(Anchor {
                name: door.name.clone(),
                transform: Transform {
                    pos: Vec3 {
                        x: door.pos[0],
                        y: door.closed_y,
                        z: door.pos[2],
                    },
                    rot: Quat::IDENTITY,
                    scale: Vec3 {
                        x: 1.0,
                        // Scale must stay positive: a lift travels DOWN, so
                        // its direction rides in `pos.y` vs the GLB clip.
                        y: travel.abs(),
                        z: 1.0,
                    },
                },
            });
        }
        // A teleport anchor: `pos` is the pad centre at the destination's
        // eye height, `rot` the arrival yaw, `scale.x`/`scale.z` the pad
        // size. `dst` itself is in the `.spawn` sidecar (and the VJ's
        // NavGrid reads it from there as `(pad_min, pad_max, dst, yaw)`).
        for start in deathmatch {
            if out.len() >= MAX_ANCHORS_PER_ASSET {
                break;
            }
            if out.iter().any(|a: &Anchor| a.name == start.name) {
                continue;
            }
            out.push(Anchor {
                name: start.name.clone(),
                transform: Transform {
                    pos: Vec3 {
                        x: start.pos[0],
                        y: start.pos[1],
                        z: start.pos[2],
                    },
                    rot: yaw_quat(start.yaw),
                    scale: Vec3::ONE,
                },
            });
        }
        for tp in &self.teleports {
            if out.len() >= MAX_ANCHORS_PER_ASSET {
                break;
            }
            let size = [
                (tp.pad_max[0] - tp.pad_min[0]).abs().max(0.01),
                (tp.pad_max[1] - tp.pad_min[1]).abs().max(0.01),
            ];
            if !size.iter().all(|v| v.is_finite()) || !tp.dst.iter().all(|v| v.is_finite()) {
                continue;
            }
            if out.iter().any(|a: &Anchor| a.name == tp.name) {
                continue;
            }
            out.push(Anchor {
                name: tp.name.clone(),
                transform: Transform {
                    pos: Vec3 {
                        x: (tp.pad_min[0] + tp.pad_max[0]) * 0.5,
                        y: tp.dst[1],
                        z: (tp.pad_min[1] + tp.pad_max[1]) * 0.5,
                    },
                    rot: yaw_quat(tp.yaw),
                    scale: Vec3 {
                        x: size[0],
                        y: 1.0,
                        z: size[1],
                    },
                },
            });
        }
        out
    }
}

/// A scalar hint rides in `pos.y` (the up axis), rotation identity: there is
/// no other numeric field on an anchor, and inventing a role or a new asset
/// field for three floats would be worse.
fn height_transform(v: f32) -> Transform {
    Transform {
        pos: Vec3 { x: 0.0, y: v, z: 0.0 },
        rot: Quat::IDENTITY,
        scale: Vec3::ONE,
    }
}

/// Yaw about +Y (the GLB up axis).
fn yaw_quat(yaw: f32) -> Quat {
    let h = yaw * 0.5;
    Quat {
        x: 0.0,
        y: h.sin(),
        z: 0.0,
        w: h.cos(),
    }
}

/// `player_start`, then `player_start_2`, `player_start_3`, … for coop and
/// player-2..4 starts in map order.
pub fn player_start_name(index: usize) -> String {
    if index == 0 {
        PRIMARY.to_string()
    } else {
        format!("{PRIMARY}_{}", index + 1)
    }
}

/// `deathmatch_1`, `deathmatch_2`, … in map order.
pub fn deathmatch_name(index: usize) -> String {
    format!("{DEATHMATCH}_{}", index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_three_line_sidecars_still_yield_the_primary_start() {
        let nav = WorldNav::parse("world-spawn 1\n1.0 2.0 3.0\n0.5 0.0\n").unwrap();
        assert_eq!(nav.starts.len(), 1);
        assert_eq!(nav.starts[0].name, PRIMARY);
        assert_eq!(nav.starts[0].pos, [1.0, 2.0, 3.0]);
        assert_eq!(nav.starts[0].yaw, 0.5);
        assert_eq!(nav.floor_y, None);
    }

    #[test]
    fn text_round_trips_and_keeps_the_library_header() {
        let nav = WorldNav {
            starts: vec![
                NavStart {
                    name: PRIMARY.into(),
                    pos: [1.5, 0.75, -2.25],
                    yaw: 1.5708,
                    pitch: 0.0,
                },
                NavStart {
                    name: "deathmatch_1".into(),
                    pos: [4.0, 0.75, 1.0],
                    yaw: -1.5708,
                    pitch: 0.0,
                },
            ],
            floor_y: Some(0.109_4),
            step_height: Some(0.375),
            eye_height: Some(0.640_6),
            doors: vec![NavDoor {
                name: "door_1".into(),
                pos: [3.0, 0.0, 1.0],
                closed_y: 0.0,
                open_y: 1.9375,
            }],
            lifts: vec![NavDoor {
                name: "lift_1".into(),
                pos: [6.0, 1.0, 2.0],
                closed_y: 1.0,
                open_y: 0.0,
            }],
            teleports: vec![NavTeleport {
                name: "teleport_1".into(),
                pad_min: [1.0, 1.0],
                pad_max: [2.0, 3.0],
                dst: [9.0, 0.64, 4.0],
                yaw: 1.5708,
            }],
            markers: vec![NavStart {
                name: "exit".into(),
                pos: [7.0, 0.64, 2.0],
                yaw: 0.0,
                pitch: 0.0,
            }],
        };
        let text = nav.to_text();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), "world-spawn 1");
        assert_eq!(lines.next().unwrap(), "1.5000 0.7500 -2.2500");
        assert_eq!(lines.next().unwrap(), "1.57080 0.00000");
        let back = WorldNav::parse(&text).unwrap();
        assert_eq!(back.starts.len(), 2);
        assert_eq!(back.starts[1].name, "deathmatch_1");
        assert_eq!(back.step_height, Some(0.375));
        assert_eq!(back.eye_height, Some(0.6406));
        assert_eq!(back.doors, nav.doors, "doors survive the round trip");
        assert_eq!(back.lifts, nav.lifts);
        assert_eq!(back.teleports, nav.teleports);
        assert_eq!(back.markers, nav.markers, "exit/key markers round trip");
        let anchors = back.anchors();
        let lift = anchors.iter().find(|a| a.name == "lift_1").unwrap();
        assert_eq!(lift.transform.pos.y, 1.0, "anchor sits at the UP floor");
        assert!((lift.transform.scale.y - 1.0).abs() < 1e-4, "travel magnitude");
        assert!(
            anchors.iter().any(|a| a.name == "exit"),
            "the exit is an anchor of its own"
        );
        let tp = anchors.iter().find(|a| a.name == "teleport_1").unwrap();
        assert_eq!(tp.transform.pos.x, 1.5, "pad centre");
        assert_eq!(tp.transform.pos.z, 2.0);
        assert!((tp.transform.scale.x - 1.0).abs() < 1e-4, "pad size x");
        assert!((tp.transform.scale.z - 2.0).abs() < 1e-4, "pad size z");
        // The door anchor carries closed height in pos.y and travel in scale.y.
        let door = back
            .anchors()
            .into_iter()
            .find(|a| a.name == "door_1")
            .expect("door anchor");
        assert_eq!(door.transform.pos.y, 0.0);
        assert!((door.transform.scale.y - 1.9375).abs() < 1e-4);
    }

    #[test]
    fn anchors_carry_yaw_as_rotation_and_heights_in_y() {
        let nav = WorldNav::single([2.0, 1.0, -3.0], std::f32::consts::FRAC_PI_2, 0.0)
            .with_heights(0.5, 0.5, 0.375);
        let anchors = nav.anchors();
        let by = |n: &str| anchors.iter().find(|a| a.name == n).unwrap().clone();
        let start = by("player_start");
        assert_eq!(start.transform.pos.x, 2.0);
        assert_eq!(start.transform.pos.z, -3.0);
        // 90° about +Y.
        assert!((start.transform.rot.y - (std::f32::consts::FRAC_PI_4).sin()).abs() < 1e-5);
        assert!((start.transform.rot.w - (std::f32::consts::FRAC_PI_4).cos()).abs() < 1e-5);
        assert_eq!(by("step_height").transform.pos.y, 0.375);
        assert_eq!(by("eye_height").transform.pos.y, 0.5);
        assert_eq!(by("floor_height").transform.pos.y, 0.5);
        for a in &anchors {
            let q = a.transform.rot;
            let len2 = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
            assert!((len2 - 1.0).abs() < 1e-3, "anchor rotations must be unit");
        }
    }

    #[test]
    fn a_crowded_map_keeps_its_player_start() {
        let mut nav = WorldNav::single([0.0, 0.0, 0.0], 0.0, 0.0).with_heights(0.0, 0.5, 0.3);
        for i in 0..64 {
            nav.starts.push(NavStart {
                name: deathmatch_name(i),
                pos: [i as f32, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            });
        }
        let anchors = nav.anchors();
        assert_eq!(anchors.len(), MAX_ANCHORS_PER_ASSET);
        assert!(anchors.iter().any(|a| a.name == PRIMARY));
        assert!(anchors.iter().any(|a| a.name == "deathmatch_1"));
    }

    #[test]
    fn start_names_are_legal_anchor_names() {
        assert_eq!(player_start_name(0), "player_start");
        assert_eq!(player_start_name(1), "player_start_2");
        assert_eq!(deathmatch_name(0), "deathmatch_1");
        for name in [player_start_name(3), deathmatch_name(9)] {
            assert!(name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
            assert!(name.as_bytes()[0].is_ascii_lowercase());
        }
    }
}
