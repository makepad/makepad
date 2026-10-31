//! The tracking log and its replay (`hal_xr_track_log`, `hal_xr_replay`): what the tracking
//! thread sent upstream (kind [`SENT`]: the sample time, the display time the poses were located
//! for, the frame) and the poses as they were tracked (kind [`TRACKED`]: a display time already
//! past, located after it), a few thousand of each, newest last. A recorded log played back
//! replaces the runtime's poses: the uplink gets the recorded predictions (moved to the replay's
//! own display times along the tracked path), the XR window's frames get the tracked path, so a
//! whole stream (uplink, the remote render, encode, decode, the client's warp) runs a repeatable
//! motion with the headset on the desk.
//!
//! Kind [`GRIP`]: the hands' grip action space (see there).
//!
//! An entry on the wire: kind, time, aux (u64 LE each), then the `XrFrame` (`buffers::Wire`).

use super::super::shell::lock;
use crate::buffers::{read_items, Wire};
use crate::{XrFrame, XrPose};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub(super) const SENT: u64 = 0;
pub(super) const TRACKED: u64 = 1;
/// The hands' grip action space (kind 2: the sample time, the target): per hand the grip at the
/// target in `controllers[h].grip_pose`, at the sample time in `aim_pose`, that one's linear and
/// angular velocity in `hands[h].joints[0..2].position`, the palm joint located at the sample
/// time, at the target + 40 ms and at the newest display frame's own time (xrWaitFrame's) in
/// `joints[2..5].position` (whether the runtime extrapolates, and how far), the location | velocity flags << 8 at
/// the sample time in `hands[h].flags` and at the target in `tips_active`.
pub(super) const GRIP: u64 = 2;
/// Entries kept (all kinds): 60 s at 90 Hz.
const KEEP: usize = 16_200;

struct Log {
    /// The absolute index of `entries[0]`.
    base: u64,
    entries: VecDeque<(u64, u64, u64, XrFrame)>,
}
static LOG: Mutex<Log> = Mutex::new(Log { base: 0, entries: VecDeque::new() });

pub(super) fn entry_size() -> usize { 24 + <XrFrame as Wire>::SIZE }

pub(super) fn push(kind: u64, t: u64, aux: u64, frame: XrFrame) {
    let mut l = lock(&LOG);
    l.entries.push_back((kind, t, aux, frame));
    while l.entries.len() > KEEP {
        l.entries.pop_front();
        l.base += 1;
    }
}

/// Writes the entries from absolute index `cursor` on (the oldest kept when older) into `out`,
/// as many whole ones as fit; returns the index after the last written. Bytes written = (the
/// result - max(cursor, oldest)) x `entry_size`.
pub(super) fn read(cursor: u64, out: &mut [u8]) -> (u64, usize) {
    let l = lock(&LOG);
    let from = cursor.max(l.base);
    let size = entry_size();
    let (mut at, mut i) = (0, from);
    let mut buf = Vec::with_capacity(<XrFrame as Wire>::SIZE);
    while i < l.base + l.entries.len() as u64 && at + size <= out.len() {
        let (kind, t, aux, frame) = &l.entries[(i - l.base) as usize];
        out[at..at + 8].copy_from_slice(&kind.to_le_bytes());
        out[at + 8..at + 16].copy_from_slice(&t.to_le_bytes());
        out[at + 16..at + 24].copy_from_slice(&aux.to_le_bytes());
        buf.clear();
        frame.write(&mut buf);
        out[at + 24..at + size].copy_from_slice(&buf);
        at += size;
        i += 1;
    }
    (i, at)
}

/// A recorded log played back.
pub(super) struct Replay {
    /// (sample time, display time located for, frame), by sample time.
    sent: Vec<(u64, u64, XrFrame)>,
    /// (display time, frame), by time.
    tracked: Vec<(u64, XrFrame)>,
    /// The replay's clock against the recording's: recorded time = start_rec + (now - start) mod span.
    start: u64,
    start_rec: u64,
    span: u64,
}
static REPLAY: Mutex<Option<Arc<Replay>>> = Mutex::new(None);

/// Plays `data` (entries as `read` writes them) from now on, looping; empty: the runtime's
/// poses again. Whether a replay runs.
pub(super) fn set_replay(data: &[u8], now: u64) -> bool {
    let size = entry_size();
    let (mut sent, mut tracked) = (Vec::new(), Vec::new());
    for e in data.chunks_exact(size) {
        let u = |k: usize| u64::from_le_bytes(e[k..k + 8].try_into().unwrap());
        let Some(frame) = read_items::<XrFrame>(&e[24..], 1).next() else { continue };
        match u(0) {
            SENT => sent.push((u(8), u(16), frame)),
            TRACKED => tracked.push((u(8), frame)),
            _ => {}
        }
    }
    sent.sort_by_key(|s| s.0);
    tracked.sort_by_key(|s| s.0);
    tracked.dedup_by_key(|s| s.0);
    let replay = match (sent.first(), sent.last(), tracked.first(), tracked.last()) {
        (Some(s0), Some(s1), Some(t0), Some(t1)) => {
            // (the span keeps every target and its tracked path inside the recording)
            let start_rec = s0.0.max(t0.0);
            let end = s1.0.min(t1.0.saturating_sub(200_000_000));
            (end > start_rec + 1_000_000_000).then(|| Arc::new(Replay { span: end - start_rec, sent, tracked, start: now, start_rec }))
        }
        _ => None,
    };
    let on = replay.is_some();
    *lock(&REPLAY) = replay;
    on
}

pub(super) fn replay() -> Option<Arc<Replay>> { lock(&REPLAY).clone() }

impl Replay {
    /// The recording's time at `now`.
    fn rec(&self, now: u64) -> u64 { self.start_rec + (now.saturating_sub(self.start)) % self.span }

    /// The tracked path at recorded time `t` (interpolated between the recorded display times).
    fn tracked_rec(&self, t: u64) -> XrFrame {
        let i = self.tracked.partition_point(|s| s.0 <= t);
        if i == 0 { return self.tracked[0].1; }
        if i >= self.tracked.len() { return self.tracked[self.tracked.len() - 1].1; }
        let (a, b) = (&self.tracked[i - 1], &self.tracked[i]);
        let s = (t - a.0) as f32 / (b.0 - a.0).max(1) as f32;
        lerp_frame(&a.1, &b.1, s)
    }

    /// The tracked path at the replay's time `t`.
    pub(super) fn tracked_at(&self, t: u64) -> XrFrame {
        self.tracked_rec(self.rec(t))
    }

    /// What the runtime predicted, sampled at `now` for display time `target`: the recorded
    /// prediction sampled nearest the same point of the recording, moved from the display time it
    /// was made for to `target` along the tracked path (its error stays the runtime's).
    pub(super) fn sent_at(&self, now: u64, target: u64) -> XrFrame {
        let rn = self.rec(now);
        let i = self.sent.partition_point(|s| s.0 <= rn);
        let i = if i == 0 { 0 } else if i >= self.sent.len() { self.sent.len() - 1 } else if rn - self.sent[i - 1].0 <= self.sent[i].0 - rn { i - 1 } else { i };
        let (_, made_for, mut frame) = self.sent[i];
        let rt = rn + target.saturating_sub(now);
        let (from, to) = (self.tracked_rec(made_for), self.tracked_rec(rt));
        move_frame(&mut frame, &from, &to);
        frame
    }
}

/// Every pose of a frame, in one order (head, eyes, controllers' grip and aim, hands' joints and aim).
fn poses(f: &XrFrame) -> Vec<XrPose> {
    let mut v = vec![f.head, f.eyes[0].pose, f.eyes[1].pose];
    for c in &f.controllers { v.push(c.grip_pose); v.push(c.aim_pose); }
    for h in &f.hands { v.extend(h.joints.iter().copied()); v.push(h.aim_pose); }
    v
}

fn set_poses(f: &mut XrFrame, v: &[XrPose]) {
    let mut it = v.iter().copied();
    let mut next = || it.next().unwrap_or_default();
    f.head = next();
    f.eyes[0].pose = next();
    f.eyes[1].pose = next();
    for c in f.controllers.iter_mut() { c.grip_pose = next(); c.aim_pose = next(); }
    for h in f.hands.iter_mut() { for j in h.joints.iter_mut() { *j = next(); } h.aim_pose = next(); }
}

fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz]
}
fn qconj(q: [f32; 4]) -> [f32; 4] { [-q[0], -q[1], -q[2], q[3]] }
fn qnorm(q: [f32; 4]) -> [f32; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n > 1e-9 { q.map(|c| c / n) } else { [0., 0., 0., 1.] }
}

fn lerp_pose(a: &XrPose, b: &XrPose, s: f32) -> XrPose {
    let dot: f32 = (0..4).map(|k| a.orientation[k] * b.orientation[k]).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let q = qnorm([0, 1, 2, 3].map(|k| a.orientation[k] + (b.orientation[k] * sign - a.orientation[k]) * s));
    XrPose { orientation: q, position: [0, 1, 2].map(|k| a.position[k] + (b.position[k] - a.position[k]) * s) }
}

/// Between two frames (poses interpolated; flags, buttons and fields of the nearer).
fn lerp_frame(a: &XrFrame, b: &XrFrame, s: f32) -> XrFrame {
    let mut out = if s < 0.5 { *a } else { *b };
    let (pa, pb) = (poses(a), poses(b));
    let v: Vec<XrPose> = pa.iter().zip(pb.iter()).map(|(x, y)| lerp_pose(x, y, s)).collect();
    set_poses(&mut out, &v);
    out
}

/// Moves every pose of `f` by the tracked path's motion from `from` to `to` (per pose: the
/// position by its translation, the orientation by its rotation).
fn move_frame(f: &mut XrFrame, from: &XrFrame, to: &XrFrame) {
    let (pf, pa, pb) = (poses(f), poses(from), poses(to));
    let v: Vec<XrPose> = pf.iter().zip(pa.iter().zip(pb.iter())).map(|(p, (a, b))| XrPose {
        orientation: qnorm(qmul(qmul(b.orientation, qconj(a.orientation)), p.orientation)),
        position: [0, 1, 2].map(|k| p.position[k] + b.position[k] - a.position[k]),
    }).collect();
    set_poses(f, &v);
}
