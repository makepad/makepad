//! Balls against fields: a ball's centre never enters a field's solid. The contacts stop a ball at
//! the surface; this holds when they can't, after the step, on the way the ball went. A wall
//! thinner than one step of a ball against it, or a ball pressed in harder than its soft contact
//! holds (a hand pulling a trapped piece), would otherwise take it past the wall's middle, where
//! the field's nearest surface is the far one and pushes it out there.
//!
//! Each ball keeps where it last was known to be outside the solid and how far it was from it. A
//! step whose way from there may cross the solid is walked; one that went in is moved back out of
//! the surface it went in through, so it stays on its side whatever its depth. Pieces and parts
//! that go together cost one comparison a step.

use makepad_box3d::body::{body_get_transform, body_set_transform};
use makepad_box3d::field::DistanceField;
use makepad_box3d::id::BodyId;
use makepad_box3d::math_functions::{add, distance, dot, inv_mul_quat, inv_mul_world_transforms, length, mul_add, mul_quat, mul_sv, offset_pos, rotate_vector, sub, transform_point, transform_world_point, Transform, Vec3, WorldTransform};
use makepad_box3d::physics_world::World;
use std::sync::Arc;

/// A walk across a ball's way steps at least this far (mm): a solid thinner than this can be missed,
/// as it can by the field's own grid.
const LEAST_STEP: f32 = 0.05;
/// A ball moved back out stands this far outside the surface (mm).
const OUTSIDE: f32 = 0.01;
/// The crossing of the surface is found to a step over 2^this.
const CROSSING_HALVINGS: usize = 8;

/// What a ball knows of one field: where its centre last was outside, and a distance the solid is at
/// least from there (zero or less: it hasn't been outside yet, and has no side).
#[derive(Clone, Copy)]
struct Known {
    at: Vec3,
    room: f32,
}

struct Track {
    /// The piece in the field's frame when its balls were last looked at, and the last such place
    /// where all of them were outside.
    last: Transform,
    clear: Transform,
    /// How far any ball may have gone since, and the least room of any ball then.
    moved: f32,
    least: f32,
    balls: Vec<Known>,
}

impl Track {
    fn of(balls: &[Vec3], rel: Transform, field: &dyn DistanceField) -> Track {
        let balls: Vec<Known> = balls.iter().map(|b| {
            let p = transform_point(rel, *b);
            Known { at: p, room: field.distance(p).0 }
        }).collect();
        let least = balls.iter().map(|b| b.room).fold(f32::MAX, f32::min);
        Track { last: rel, clear: rel, moved: 0.0, least, balls }
    }
}

struct Piece {
    body: usize,
    /// Ball centres in the body's frame.
    balls: Vec<Vec3>,
    /// The farthest ball centre from the body's origin.
    reach: f32,
    /// One per field; none until the next step starts, as after the piece was put somewhere.
    tracks: Vec<Option<Track>>,
}

/// A ball that went in: which, where it is, where it was last outside on its side, and the way
/// back out (a unit direction and how far).
struct Fix {
    ball: usize,
    at: Vec3,
    from: Vec3,
    way: Vec3,
    long: f32,
}

pub(crate) struct KeepOut {
    /// Bodies with a field: the body and its solid in the body's frame.
    fields: Vec<(usize, Arc<dyn DistanceField>)>,
    pieces: Vec<Piece>,
}

impl KeepOut {
    /// `fields`: (body, solid in its frame); `pieces`: (body, ball centres in its frame).
    pub(crate) fn new(fields: Vec<(usize, Arc<dyn DistanceField>)>, pieces: Vec<(usize, Vec<Vec3>)>) -> Option<KeepOut> {
        if fields.is_empty() || pieces.is_empty() {
            return None;
        }
        let n = fields.len();
        let pieces = pieces
            .into_iter()
            .map(|(body, balls)| {
                let reach = balls.iter().map(|b| length(*b)).fold(0.0, f32::max);
                Piece { body, balls, reach, tracks: (0..n).map(|_| None).collect() }
            })
            .collect();
        Some(KeepOut { fields, pieces })
    }

    /// `body` was put somewhere, not moved there: its balls start over from where they are.
    pub(crate) fn forget(&mut self, body: usize) {
        let field = self.fields.iter().any(|f| f.0 == body);
        for p in &mut self.pieces {
            if p.body == body || field {
                p.tracks.iter_mut().for_each(|t| *t = None);
            }
        }
    }

    /// Before a step: pieces without a track start one where they are.
    pub(crate) fn start(&mut self, world: &World, bodies: &[BodyId]) {
        for piece in &mut self.pieces {
            for (f, (body, field)) in self.fields.iter().enumerate() {
                if *body != piece.body && piece.tracks[f].is_none() {
                    let rel = inv_mul_world_transforms(body_get_transform(world, bodies[*body]), body_get_transform(world, bodies[piece.body]));
                    piece.tracks[f] = Some(Track::of(&piece.balls, rel, field.as_ref()));
                }
            }
        }
    }

    /// After a step: a piece whose ball went into a solid is moved back out the way it came.
    /// Answers how many pieces were moved.
    pub(crate) fn step(&mut self, world: &mut World, bodies: &[BodyId]) -> usize {
        let mut moved = 0;
        let mut fixes: Vec<Fix> = Vec::new();
        for piece in &mut self.pieces {
            let at = body_get_transform(world, bodies[piece.body]);
            let mut push = Vec3::ZERO;
            let mut back_to: Option<WorldTransform> = None;
            for (f, (body, field)) in self.fields.iter().enumerate() {
                let Some(track) = piece.tracks[f].as_mut().filter(|_| *body != piece.body) else { continue };
                let frame = body_get_transform(world, bodies[*body]);
                let rel = inv_mul_world_transforms(frame, at);
                // a turn moves a point at r by its chord, 2 r |sin(angle / 2)|
                let turn = inv_mul_quat(track.last.q, rel.q);
                track.moved += distance(rel.p, track.last.p) + 2.0 * length(turn.v) * piece.reach;
                track.last = rel;
                if track.moved <= 0.0 || track.moved < track.least {
                    continue;
                }
                track.moved = 0.0;
                fixes.clear();
                for (i, (b, known)) in piece.balls.iter().zip(track.balls.iter_mut()).enumerate() {
                    if let Some(fix) = look(field.as_ref(), known, transform_point(rel, *b), i) {
                        fixes.push(fix);
                    }
                }
                track.least = track.balls.iter().map(|b| b.room).fold(f32::MAX, f32::min);
                if fixes.is_empty() {
                    track.clear = rel;
                    continue;
                }
                // the piece moves once, as far as its deepest ball needs; one that needs a way another
                // already took adds only what is left of it
                fixes.sort_by(|a, b| b.long.total_cmp(&a.long));
                let mut back = Vec3::ZERO;
                for fix in &fixes {
                    let left = fix.long - dot(back, fix.way);
                    if left > 0.0 {
                        back = mul_add(back, left, fix.way);
                    }
                }
                // each ball gets there without going through anything: those that went in from
                // where they were last out on their side, the others from where they are
                let long = length(back);
                let holds = piece.balls.iter().zip(&track.balls).enumerate().all(|(i, (b, known))| match fixes.iter().find(|fix| fix.ball == i) {
                    Some(fix) => went_in(field.as_ref(), fix.from, add(fix.at, back)).is_none(),
                    None => known.room > long || {
                        let p = transform_point(rel, *b);
                        went_in(field.as_ref(), p, add(p, back)).is_none()
                    },
                });
                if holds {
                    push = add(push, rotate_vector(frame.q, back));
                    track.clear = Transform { p: add(rel.p, back), q: rel.q };
                } else if back_to.is_none() {
                    // balls caught in different corners: back to where they all were out
                    back_to = Some(WorldTransform { p: transform_world_point(frame, track.clear.p), q: mul_quat(frame.q, track.clear.q) });
                }
            }
            if let Some(to) = back_to {
                body_set_transform(world, bodies[piece.body], to.p, to.q);
                piece.tracks.iter_mut().for_each(|t| *t = None);
                moved += 1;
            } else if length(push) > 0.0 {
                body_set_transform(world, bodies[piece.body], offset_pos(at.p, push), at.q);
                // its tracks last saw it before the push: what it goes next is from past there
                let by = length(push);
                piece.tracks.iter_mut().flatten().for_each(|t| t.moved += by);
                moved += 1;
            }
        }
        moved
    }
}

/// Where ball `ball` (`known` of it, now at `p`) has gone: none while it stays out, else the way
/// out of the surface it went in through, keeping how it went along that surface; where that
/// isn't on its side (a corner, or past another wall), back to where it went in.
fn look(field: &dyn DistanceField, known: &mut Known, p: Vec3, ball: usize) -> Option<Fix> {
    let way = distance(p, known.at);
    if known.room <= 0.0 {
        // no side yet: it has one from where it is first outside
        *known = Known { at: p, room: field.distance(p).0 };
        return None;
    }
    if known.room > way {
        *known = Known { at: p, room: known.room - way };
        return None;
    }
    let room = field.distance(p).0;
    if room > 0.0 && way <= known.room + room {
        *known = Known { at: p, room };
        return None;
    }
    let Some((face, normal, out)) = went_in(field, known.at, p) else {
        *known = Known { at: p, room };
        return None;
    };
    *known = out;
    let along = mul_add(p, dot(sub(face, p), normal) + OUTSIDE, normal);
    let back = sub(if went_in(field, out.at, along).is_none() { along } else { out.at }, p);
    let long = length(back);
    (long > 0.0).then(|| Fix { ball, at: p, from: out.at, way: mul_sv(1.0 / long, back), long })
}

/// Walks the way from `from` (outside) to `to`: where it first goes in, the surface it went in
/// through (a point on it and its outward normal) and the last place it was outside, `OUTSIDE`
/// short of the crossing where the walk was out there; none when it stays out.
fn went_in(field: &dyn DistanceField, from: Vec3, to: Vec3) -> Option<(Vec3, Vec3, Known)> {
    let long = distance(from, to);
    let way = if long > 0.0 { mul_sv(1.0 / long, sub(to, from)) } else { Vec3::ZERO };
    let mut t = 0.0f32;
    // the last place outside, along the way
    let mut out: Option<(f32, f32)> = None;
    loop {
        let (d, _) = field.distance(mul_add(from, t.min(long), way));
        if d <= 0.0 {
            let (last, room) = out?;
            let mut a = last;
            // the surface where the way crosses it, not the nearest one to where it was last out
            // (beside a wall on a shelf, the shelf): halve the step in and out of it
            let mut b = t.min(long);
            for _ in 0..CROSSING_HALVINGS {
                let m = 0.5 * (a + b);
                if field.distance(mul_add(from, m, way)).0 > 0.0 {
                    a = m;
                } else {
                    b = m;
                }
            }
            let at = mul_add(from, a, way);
            let (d, normal) = field.distance(at);
            let short = mul_add(from, (a - OUTSIDE).max(last), way);
            let known = match field.distance(short).0 {
                off if off > 0.0 => Known { at: short, room: off },
                _ => Known { at: mul_add(from, last, way), room },
            };
            return Some((mul_add(at, -d.max(0.0), normal), normal, known));
        }
        out = Some((t.min(long), d));
        if t >= long {
            return None;
        }
        t += d.max(LEAST_STEP);
    }
}
