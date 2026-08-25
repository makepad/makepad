//! `CameraTrack` — what a shot generator produces and everyone else consumes.
//!
//! This is the type lane F renders an image sequence from and lane C plays in
//! the realtime viewport, so it is deliberately dumb: a dense list of sampled
//! cameras with absolute times. Densely enough sampled (≥ 30 keys/s) that a
//! consumer interpolating linearly between keys sees the same C2 curve the
//! generator built.
//!
//! The app mirrors this as `libs/fab/src/api.rs::{CameraKey, CameraTrack}`
//! so the viewer never has to depend on this crate; `libs/fab/src/tour`
//! converts. Keep the two in step.

use makepad_math::{vec3, Vec3f};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShotKind {
    /// Rising orbit around the whole building.
    DroneReveal,
    /// From afar, down the best façade axis, to the front door.
    Approach,
    /// On foot, room to room through the door graph.
    Walkthrough,
    /// The same graph, flying, through the biggest openings.
    DroneFlythrough,
    /// One element or room, orbited.
    Orbit,
    /// A level at a time, with the storeys above lifted away.
    StoreyReveal,
    /// Reveal → approach → walkthrough → exit → pull-back.
    FullTour,
}

impl ShotKind {
    pub fn label(self) -> &'static str {
        match self {
            ShotKind::DroneReveal => "Drone reveal",
            ShotKind::Approach => "Approach",
            ShotKind::Walkthrough => "Walkthrough",
            ShotKind::DroneFlythrough => "Drone fly-through",
            ShotKind::Orbit => "Orbit",
            ShotKind::StoreyReveal => "Storey reveal",
            ShotKind::FullTour => "Full tour",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TourKey {
    pub t: f32,
    pub pos: Vec3f,
    pub look_at: Vec3f,
    pub up: Vec3f,
    pub fov_y_deg: f32,
}

impl TourKey {
    pub fn dir(&self) -> Vec3f {
        let d = self.look_at - self.pos;
        if d.length_squared() < 1e-9 {
            vec3(1.0, 0.0, 0.0)
        } else {
            d.normalize()
        }
    }
}

/// A labelled moment: which room the camera is in, which door it just went
/// through. Drives the Tours panel keyframe list and picks the frames a
/// contact sheet shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackNote {
    pub t: f32,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraTrack {
    pub name: String,
    pub kind_label: String,
    pub keys: Vec<TourKey>,
    pub fps: f32,
    pub notes: Vec<TrackNote>,
}

impl CameraTrack {
    pub fn duration(&self) -> f32 {
        self.keys.last().map(|k| k.t).unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn sample(&self, t: f32) -> Option<TourKey> {
        let n = self.keys.len();
        if n == 0 {
            return None;
        }
        if n == 1 || t <= self.keys[0].t {
            return Some(self.keys[0]);
        }
        if t >= self.keys[n - 1].t {
            return Some(self.keys[n - 1]);
        }
        let i = self.keys.partition_point(|k| k.t <= t).max(1);
        let a = self.keys[i - 1];
        let b = self.keys[i];
        let f = ((t - a.t) / (b.t - a.t).max(1e-6)).clamp(0.0, 1.0);
        Some(TourKey {
            t,
            pos: Vec3f::from_lerp(a.pos, b.pos, f),
            look_at: Vec3f::from_lerp(a.look_at, b.look_at, f),
            up: Vec3f::from_lerp(a.up, b.up, f).normalize(),
            fov_y_deg: a.fov_y_deg + (b.fov_y_deg - a.fov_y_deg) * f,
        })
    }

    /// Total distance flown, metres.
    pub fn path_length(&self) -> f32 {
        self.keys
            .windows(2)
            .map(|w| (w[1].pos - w[0].pos).length())
            .sum()
    }

    /// Append `other` after this track, shifting its times. Used to assemble
    /// the full tour out of its legs.
    pub fn append(&mut self, other: &CameraTrack, gap: f32) {
        if other.keys.is_empty() {
            return;
        }
        let base = if self.keys.is_empty() {
            0.0
        } else {
            self.duration() + gap
        };
        let skip = usize::from(!self.keys.is_empty());
        for k in other.keys.iter().skip(skip) {
            let mut k = *k;
            k.t += base;
            self.keys.push(k);
        }
        for n in &other.notes {
            self.notes.push(TrackNote {
                t: n.t + base,
                text: n.text.clone(),
            });
        }
    }
}

impl CameraTrack {
    /// Re-sample onto a fixed frame rate: exactly one camera per frame, which
    /// is what an offline renderer wants — it asks for frame *n*, not for a
    /// time. The last frame lands on the track's end.
    pub fn resampled(&self, fps: f32) -> CameraTrack {
        let fps = fps.max(1.0);
        let d = self.duration();
        let frames = ((d * fps).round() as usize).max(1);
        let keys = (0..=frames)
            .filter_map(|i| {
                let t = (i as f32 / fps).min(d);
                self.sample(t).map(|mut k| {
                    k.t = t;
                    k
                })
            })
            .collect();
        CameraTrack {
            name: self.name.clone(),
            kind_label: self.kind_label.clone(),
            keys,
            fps,
            notes: self.notes.clone(),
        }
    }

    /// Serialise as JSON for the path tracer and any other consumer.
    ///
    /// Hand-rolled rather than derived: this crate has no serde dependency and
    /// the schema is small enough that adding one to emit twelve floats per
    /// key would cost more than it saves. Coordinates are the crate's own —
    /// right-handed, Z up, metres — and the header says so, because a camera
    /// track that does not declare its axis convention is a bug waiting for
    /// the next reader.
    pub fn to_json(&self) -> String {
        fn f(v: f32) -> String {
            // Short but exact enough to round-trip at render scale.
            let s = format!("{v:.5}");
            let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
            if s.is_empty() || s == "-0" {
                "0".into()
            } else {
                s
            }
        }
        fn esc(s: &str) -> String {
            s.chars()
                .flat_map(|c| match c {
                    '"' => vec!['\\', '"'],
                    '\\' => vec!['\\', '\\'],
                    '\n' => vec!['\\', 'n'],
                    c if (c as u32) < 0x20 => vec![' '],
                    c => vec![c],
                })
                .collect()
        }
        let mut s = String::with_capacity(self.keys.len() * 120 + 256);
        s.push_str("{\n");
        s.push_str(&format!("  \"name\": \"{}\",\n", esc(&self.name)));
        s.push_str(&format!("  \"kind\": \"{}\",\n", esc(&self.kind_label)));
        s.push_str("  \"up_axis\": \"z\",\n  \"handedness\": \"right\",\n  \"units\": \"m\",\n");
        s.push_str(&format!("  \"fps\": {},\n", f(self.fps)));
        s.push_str(&format!("  \"duration\": {},\n", f(self.duration())));
        s.push_str(&format!("  \"key_count\": {},\n", self.keys.len()));
        s.push_str("  \"notes\": [\n");
        for (i, n) in self.notes.iter().enumerate() {
            s.push_str(&format!(
                "    {{\"t\": {}, \"text\": \"{}\"}}{}\n",
                f(n.t),
                esc(&n.text),
                if i + 1 == self.notes.len() { "" } else { "," }
            ));
        }
        s.push_str("  ],\n  \"keys\": [\n");
        for (i, k) in self.keys.iter().enumerate() {
            s.push_str(&format!(
                "    {{\"t\": {}, \"pos\": [{}, {}, {}], \"look_at\": [{}, {}, {}], \"up\": [{}, {}, {}], \"fov_y_deg\": {}}}{}\n",
                f(k.t),
                f(k.pos.x), f(k.pos.y), f(k.pos.z),
                f(k.look_at.x), f(k.look_at.y), f(k.look_at.z),
                f(k.up.x), f(k.up.y), f(k.up.z),
                f(k.fov_y_deg),
                if i + 1 == self.keys.len() { "" } else { "," }
            ));
        }
        s.push_str("  ]\n}\n");
        s
    }

    /// Renderer schema: `{"fps":24,"keys":[...]}` and nothing else.
    pub fn to_track_json(&self) -> String {
        fn f(v: f32) -> String {
            let s = format!("{v:.5}");
            let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
            if s.is_empty() || s == "-0" {
                "0".into()
            } else {
                s
            }
        }
        let mut s = String::with_capacity(self.keys.len() * 110 + 32);
        s.push_str(&format!("{{\"fps\": {}, \"keys\": [", f(self.fps)));
        for (i, k) in self.keys.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&format!(
                "{{\"t\": {}, \"pos\": [{}, {}, {}], \"look_at\": [{}, {}, {}], \"up\": [{}, {}, {}], \"fov_y_deg\": {}}}",
                f(k.t),
                f(k.pos.x), f(k.pos.y), f(k.pos.z),
                f(k.look_at.x), f(k.look_at.y), f(k.look_at.z),
                f(k.up.x), f(k.up.y), f(k.up.z),
                f(k.fov_y_deg),
            ));
        }
        s.push_str("]}");
        s
    }
}
